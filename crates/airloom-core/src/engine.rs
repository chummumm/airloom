use crate::model::{
    self, AppError, AuthPrompt, CaptureView, Command, DeviceView, Levels, Metrics, Phase, Snapshot,
    SourceView,
};
use airloom_airplay::{
    AuthenticationKind, ConnectOptions, Device, Error as AirplayError, PairingCredentials,
    PairingSession, Sender, SenderMetrics,
};
use airloom_capture::{
    credentials, AudioChunk, CaptureConfig, CaptureInfo, CaptureSource, CaptureStream,
};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle as Task;
use zeroize::Zeroizing;

const CREDENTIAL_SERVICE: &str = "Airloom";
const DISCOVERY_WINDOW: Duration = Duration::from_secs(4);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(25);
const BLOCK_FRAMES: usize = 441;
const MAX_SELECTED_DEVICES: usize = 8;
const MEDIA_QUEUE_BLOCKS: usize = 16;
const MEDIA_MAX_AGE: Duration = Duration::from_millis(200);
const TEST_TONE_FRAMES: usize = airloom_airplay::SAMPLE_RATE as usize * 3;

pub struct Engine {
    commands: mpsc::Sender<Command>,
    snapshot: Arc<Mutex<Snapshot>>,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Engine {
    pub fn start() -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (commands, receiver) = mpsc::channel(64);
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let shutdown = Arc::new(AtomicBool::new(false));
        let state = snapshot.clone();
        let exit = shutdown.clone();
        let thread = thread::Builder::new()
            .name("airloom-engine".into())
            .spawn(move || {
                runtime.block_on(Actor::new(state, exit).run(receiver));
                runtime.shutdown_timeout(Duration::from_secs(1));
            })?;
        Ok(Self {
            commands,
            snapshot,
            shutdown,
            thread: Some(thread),
        })
    }

    pub fn send(&self, command: Command) -> Result<(), model::CommandError> {
        if !command.validate() {
            return Err(model::CommandError::Invalid);
        }
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => model::CommandError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => model::CommandError::Stopped,
            })
    }

    pub fn snapshot(&self) -> Snapshot {
        lock(&self.snapshot).clone()
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn lock<T>(state: &Mutex<T>) -> MutexGuard<'_, T> {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// Secrets never implement Debug and never enter UI snapshots.
#[derive(Clone, Default, Serialize, Deserialize)]
struct Access {
    password: Option<Zeroizing<String>>,
    credentials: Option<PairingCredentials>,
}

/// One independently paced receiver. It never reads the WASAPI queue itself.
struct ActiveTarget {
    audio: mpsc::Sender<MediaBlock>,
    volume: watch::Sender<f32>,
    stop: watch::Sender<bool>,
    task: Task<()>,
    progress: Arc<Mutex<SenderMetrics>>,
    started: Instant,
}

#[derive(Clone)]
struct MediaBlock {
    samples: Arc<[i16]>,
    captured_at: Instant,
}

impl ActiveTarget {
    fn spawn(
        sender: Sender,
        device_id: String,
        generation: u64,
        initial_volume: f32,
        events: mpsc::UnboundedSender<TargetFailure>,
    ) -> Self {
        let (audio, input) = mpsc::channel(MEDIA_QUEUE_BLOCKS);
        let (volume, volume_changes) = watch::channel(initial_volume);
        let (stop, cancellation) = watch::channel(false);
        let progress = Arc::new(Mutex::new(sender.metrics()));
        let published = progress.clone();
        let task = tokio::spawn(run_target(
            sender,
            input,
            volume_changes,
            cancellation,
            published,
            TargetIdentity {
                device_id,
                generation,
            },
            events,
        ));
        Self {
            audio,
            volume,
            stop,
            task,
            progress,
            started: Instant::now(),
        }
    }

    async fn shutdown(self) {
        let _ = self.stop.send(true);
        drop(self.audio);
        drop(self.volume);
        let mut task = self.task;
        if tokio::time::timeout(Duration::from_millis(900), &mut task)
            .await
            .is_err()
        {
            task.abort();
            let _ = task.await;
        }
    }
}

#[derive(Clone)]
struct TargetIdentity {
    device_id: String,
    generation: u64,
}

struct TargetFailure {
    identity: TargetIdentity,
    message: String,
}

/// Aborting a sender must also abort its separate control task.
struct AbortTask(Task<()>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn run_target(
    mut sender: Sender,
    mut audio: mpsc::Receiver<MediaBlock>,
    mut volume: watch::Receiver<f32>,
    mut stop: watch::Receiver<bool>,
    progress: Arc<Mutex<SenderMetrics>>,
    identity: TargetIdentity,
    events: mpsc::UnboundedSender<TargetFailure>,
) {
    let control = sender.volume_control();
    let volume_identity = identity.clone();
    let volume_events = events.clone();
    let volume_task = AbortTask(tokio::spawn(async move {
        loop {
            let desired = *volume.borrow_and_update();
            if let Err(error) = control.set_volume(desired).await {
                let _ = volume_events.send(TargetFailure {
                    identity: volume_identity,
                    message: format!("设备没有确认音量变化：{}", user_error(&error)),
                });
                break;
            }
            if volume.changed().await.is_err() {
                break;
            }
        }
    }));
    loop {
        let block = tokio::select! {
            biased;
            result = stop.changed() => {
                if result.is_err() || *stop.borrow() { break; }
                continue;
            },
            block = audio.recv() => match block { Some(block) => block, None => break },
        };
        if block.captured_at.elapsed() > MEDIA_MAX_AGE {
            let _ = events.send(TargetFailure {
                identity: identity.clone(),
                message: "这台设备的音频发送已落后，正在重新连接。".into(),
            });
            break;
        }
        // This receiver's pacing cannot delay another receiver or the capture
        // producer. Once selected, send_pcm is completed (or terminally timed
        // out), never cancelled by an ordinary volume/selection command.
        let result =
            tokio::time::timeout(Duration::from_secs(2), sender.send_pcm(&block.samples)).await;
        *lock(&progress) = sender.metrics();
        let message = match result {
            Ok(Ok(())) => continue,
            Ok(Err(error)) => user_error(&error),
            Err(_) => "向这台设备发送超时，正在重新连接。".into(),
        };
        let _ = events.send(TargetFailure { identity, message });
        break;
    }
    drop(volume_task);
    let _ = tokio::time::timeout(Duration::from_millis(750), sender.stop()).await;
}

struct Route {
    phase: Phase,
    message: String,
    error: Option<AppError>,
    access: Access,
    access_loaded: bool,
    remember_after_connect: bool,
    wanted: bool,
    generation: u64,
    retries: u8,
    retry_at: Option<Instant>,
    session: Option<ActiveTarget>,
    cleanup: Option<Task<()>>,
    last_metrics: SenderMetrics,
}

impl Default for Route {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            message: "等待连接".into(),
            error: None,
            access: Access::default(),
            access_loaded: false,
            remember_after_connect: false,
            wanted: false,
            generation: 0,
            retries: 0,
            retry_at: None,
            session: None,
            cleanup: None,
            last_metrics: SenderMetrics::default(),
        }
    }
}

impl Route {
    fn metrics(&self) -> SenderMetrics {
        self.session
            .as_ref()
            .map(|session| lock(&session.progress).clone())
            .unwrap_or_else(|| self.last_metrics.clone())
    }
}

struct CaptureSession {
    stream: CaptureStream,
    info: CaptureInfo,
    buffer: CaptureBuffer,
    started: Instant,
    ready_at: Instant,
}

/// Only the producer owns this buffer. Every receiver gets the same resulting
/// block; cloning a WASAPI receiver would instead divide the audio between them.
struct CaptureBuffer {
    pending: VecDeque<f32>,
    started: Instant,
    last_packet: Option<Instant>,
    last_signal: Option<Instant>,
    peaks: [f32; 2],
    frames_received: u64,
    chunks_received: u64,
    capture_dropped: u64,
    queue_dropped: u64,
    underrun_frames: u64,
}

impl CaptureBuffer {
    fn new(now: Instant) -> Self {
        Self {
            pending: VecDeque::new(),
            started: now,
            last_packet: None,
            last_signal: None,
            peaks: [0.0; 2],
            frames_received: 0,
            chunks_received: 0,
            capture_dropped: 0,
            queue_dropped: 0,
            underrun_frames: 0,
        }
    }

    fn ingest(&mut self, chunk: AudioChunk, now: Instant) {
        self.capture_dropped = chunk.dropped_frames;
        self.frames_received = self.frames_received.saturating_add(chunk.frames() as u64);
        self.chunks_received = self.chunks_received.saturating_add(1);
        self.last_packet = Some(now);
        self.peaks = chunk.peak;
        if chunk.peak[0].max(chunk.peak[1]) > 0.00001 {
            self.last_signal = Some(now);
        }
        self.pending.extend(chunk.samples);
        const MAX_SAMPLES: usize = BLOCK_FRAMES * 8 * 2;
        if self.pending.len() > MAX_SAMPLES {
            let discard = (self.pending.len() - MAX_SAMPLES) / 2 * 2;
            self.pending.drain(..discard);
            self.queue_dropped = self.queue_dropped.saturating_add((discard / 2) as u64);
        }
    }

    fn next_block(&mut self) -> Vec<f32> {
        self.underrun_frames = self
            .underrun_frames
            .saturating_add(BLOCK_FRAMES.saturating_sub(self.pending.len() / 2) as u64);
        (0..BLOCK_FRAMES * 2)
            .map(|_| self.pending.pop_front().unwrap_or(0.0))
            .collect()
    }

    fn view(&self, info: &CaptureInfo, muted: bool, now: Instant) -> CaptureView {
        let age = self
            .last_packet
            .map(|last| now.saturating_duration_since(last));
        let peaks = if age.is_some_and(|age| age <= Duration::from_millis(300)) {
            self.peaks
        } else {
            [0.0; 2]
        };
        let silent_ms = now
            .saturating_duration_since(self.last_signal.unwrap_or(self.started))
            .as_millis() as u64;
        let peak = peaks[0].max(peaks[1]);
        let (state, message) = if peak >= 0.01 {
            ("signal", "所选输出设备有音频信号。")
        } else if peak > 0.00001 {
            (
                "quiet",
                "所选输出设备的声音较轻；可播放测试音检查扬声器链路。",
            )
        } else if silent_ms >= 3000 {
            (
                "silent",
                if muted {
                    "所选输出暂时没有声音。请检查应用输出设备，也可暂时关闭电脑静音后再试。"
                } else {
                    "所选输出暂时没有声音。请让播放应用使用这个输出设备，或切换声音来源。"
                },
            )
        } else {
            ("waiting", "正在等待所选输出设备的声音…")
        };
        CaptureView {
            state,
            source_id: info.source_id.clone(),
            source_name: info.source_name.clone(),
            message: message.into(),
            frames_received: self.frames_received,
            chunks_received: self.chunks_received,
            last_packet_ms: age.map(|age| age.as_millis() as u64),
            silent_ms,
            level: Levels {
                left: peaks[0],
                right: peaks[1],
            },
        }
    }
}

struct TestTone {
    position: usize,
}
impl TestTone {
    fn new() -> Self {
        Self { position: 0 }
    }
    fn remaining_ms(&self) -> u32 {
        (TEST_TONE_FRAMES.saturating_sub(self.position) * 1000
            / airloom_airplay::SAMPLE_RATE as usize) as u32
    }
    fn apply(&mut self, samples: &mut [f32]) {
        const FADE_FRAMES: usize = 882;
        for pair in samples.chunks_exact_mut(2) {
            if self.position >= TEST_TONE_FRAMES {
                break;
            }
            let fade = (self.position as f32 / FADE_FRAMES as f32)
                .min((TEST_TONE_FRAMES - self.position) as f32 / FADE_FRAMES as f32)
                .min(1.0);
            let phase = std::f64::consts::TAU * 440.0 * self.position as f64
                / airloom_airplay::SAMPLE_RATE as f64;
            let tone = phase.sin() as f32 * 0.12;
            for sample in pair {
                *sample = *sample * (1.0 - fade) + tone * fade;
            }
            self.position += 1;
        }
    }
    fn finished(&self) -> bool {
        self.position >= TEST_TONE_FRAMES
    }
}

enum Work {
    Connected(Result<Sender, AirplayError>),
    PairingStarted(Result<PairingSession, AirplayError>),
    Paired(Result<PairingCredentials, AirplayError>),
}
struct ConnectionWork {
    device_id: String,
    generation: u64,
    task: Task<Work>,
}
struct ConnectionRequest {
    device_id: String,
    pair: bool,
}
struct DiscoveryResult {
    devices: Result<Vec<Device>, AirplayError>,
    sources: airloom_capture::Result<Vec<CaptureSource>>,
}

struct Actor {
    state: Arc<Mutex<Snapshot>>,
    shutdown: Arc<AtomicBool>,
    devices: Vec<Device>,
    online: BTreeSet<String>,
    remembered: BTreeSet<String>,
    selected: Vec<String>,
    routes: BTreeMap<String, Route>,
    running: bool,
    discovery: Option<Task<DiscoveryResult>>,
    work: Option<ConnectionWork>,
    queue: VecDeque<ConnectionRequest>,
    pairing: Option<(String, PairingSession)>,
    auth: Option<AuthPrompt>,
    auth_generation: u64,
    capture: Option<CaptureSession>,
    test_tone: Option<TestTone>,
    global_error: Option<AppError>,
    events: mpsc::UnboundedSender<TargetFailure>,
    failures: mpsc::UnboundedReceiver<TargetFailure>,
}

impl Actor {
    fn new(state: Arc<Mutex<Snapshot>>, shutdown: Arc<AtomicBool>) -> Self {
        let selected = {
            let state = lock(&state);
            if state.selected_device_ids.is_empty() {
                if state.selected_device_id.is_empty() {
                    Vec::new()
                } else {
                    vec![state.selected_device_id.clone()]
                }
            } else {
                state.selected_device_ids.clone()
            }
        };
        let (events, failures) = mpsc::unbounded_channel();
        let routes = selected
            .iter()
            .cloned()
            .map(|id| (id, Route::default()))
            .collect();
        Self {
            state,
            shutdown,
            devices: Vec::new(),
            online: BTreeSet::new(),
            remembered: BTreeSet::new(),
            selected,
            routes,
            running: false,
            discovery: None,
            work: None,
            queue: VecDeque::new(),
            pairing: None,
            auth: None,
            auth_generation: 0,
            capture: None,
            test_tone: None,
            global_error: None,
            events,
            failures,
        }
    }

    fn update(&self, edit: impl FnOnce(&mut Snapshot)) {
        let mut snapshot = lock(&self.state);
        edit(&mut snapshot);
        snapshot.revision = snapshot.revision.wrapping_add(1);
    }

    fn active_count(&self) -> usize {
        self.routes
            .values()
            .filter(|route| route.session.is_some())
            .count()
    }

    fn synchronize(&self) {
        let active = self.active_count();
        let reconnecting = self
            .routes
            .values()
            .any(|route| route.phase == Phase::Reconnecting && route.wanted);
        let has_error = self.global_error.is_some()
            || self
                .routes
                .values()
                .any(|route| route.phase == Phase::Error);
        let phase = if active > 0 {
            Phase::Streaming
        } else if self.auth.is_some() {
            Phase::AuthRequired
        } else if reconnecting {
            Phase::Reconnecting
        } else if self.work.is_some() || !self.queue.is_empty() {
            Phase::Connecting
        } else if has_error {
            Phase::Error
        } else if self.discovery.is_some() {
            Phase::Discovering
        } else {
            Phase::Idle
        };
        let status = if active > 0 {
            let suffix = if self.auth.is_some() {
                "；另有设备等待验证。"
            } else if self.work.is_some() || !self.queue.is_empty() {
                "；正在添加设备。"
            } else if has_error {
                "；部分设备需要处理。"
            } else {
                "。"
            };
            format!("正在向 {active} 台设备发送声音{suffix}")
        } else if let Some(prompt) = &self.auth {
            prompt.message.clone()
        } else if let Some(error) = &self.global_error {
            error.message.clone()
        } else if reconnecting {
            "正在恢复与设备的连接…".into()
        } else if self.work.is_some() || !self.queue.is_empty() {
            "正在建立安全连接…".into()
        } else if has_error {
            "设备连接需要处理，请查看设备卡片后重试。".into()
        } else if self.discovery.is_some() {
            "正在寻找同一局域网中的 AirPlay 设备…".into()
        } else if self.devices.is_empty() {
            "还没有发现设备。请确认扬声器与电脑连接同一局域网。".into()
        } else {
            "可选择一个或多个设备开始投放。".into()
        };
        let views = self
            .devices
            .iter()
            .map(|device| {
                let route = self.routes.get(&device.id);
                let metrics = route.map(Route::metrics).unwrap_or_default();
                DeviceView {
                    id: device.id.clone(),
                    name: device.name.clone(),
                    model: device.model.clone().unwrap_or_else(|| "AirPlay".into()),
                    address: device
                        .addresses
                        .first()
                        .map(ToString::to_string)
                        .unwrap_or_default(),
                    password_required: device.requires_password || device.requires_pin,
                    online: self.online.contains(&device.id)
                        || route.is_some_and(|route| route.session.is_some()),
                    remembered: self.remembered.contains(&device.id),
                    selected: self.selected.contains(&device.id),
                    state: route.map(|route| route.phase).unwrap_or(Phase::Idle),
                    status_message: route
                        .map(|route| route.message.clone())
                        .unwrap_or_else(|| "等待选择".into()),
                    error: route.and_then(|route| route.error.clone()),
                    packets_sent: metrics.packets_sent,
                    timing_exchanges: metrics.timing_exchanges,
                }
            })
            .collect();
        let packets = self
            .routes
            .values()
            .map(|route| route.metrics().packets_sent)
            .sum();
        let error = self.global_error.clone().or_else(|| {
            if active == 0 {
                self.routes.values().find_map(|route| route.error.clone())
            } else {
                None
            }
        });
        self.update(|s| {
            s.state = phase;
            s.status_message = status;
            s.devices = views;
            s.selected_device_ids = self.selected.clone();
            s.selected_device_id = self.selected.first().cloned().unwrap_or_default();
            s.auth = self.auth.clone();
            s.error = error;
            s.metrics.packets_sent = packets;
            s.test_tone_active = self.test_tone.is_some();
            s.test_tone_remaining_ms = self.test_tone.as_ref().map_or(0, TestTone::remaining_ms);
        });
    }

    fn error(&mut self, code: &str, message: impl Into<String>) {
        self.global_error = Some(AppError {
            code: code.into(),
            message: message.into(),
        });
    }

    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        self.refresh();
        self.synchronize();
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                break;
            }
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.command(command),
                    None => break,
                },
                result = async {
                    match &mut self.discovery {
                        Some(task) => task.await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.discovery = None;
                    match result {
                        Ok(result) => self.discovered(result),
                        Err(_) => self.error("discovery_worker", "设备扫描中断，请重新扫描。"),
                    }
                },
                result = async {
                    match &mut self.work {
                        Some(work) => {
                            let id = work.device_id.clone();
                            let generation = work.generation;
                            (id, generation, (&mut work.task).await)
                        }
                        None => std::future::pending().await,
                    }
                } => {
                    self.work = None;
                    let (id, generation, result) = result;
                    self.finish_work(&id, generation, result);
                },
                failure = self.failures.recv() => if let Some(failure) = failure {
                    self.target_failed(failure);
                },
                _ = tick.tick() => {
                    self.maintain_routes();
                    self.audio();
                },
            }
            self.begin_next();
            self.synchronize();
        }
        self.stop_all();
        if let Some(task) = self.discovery.take() {
            task.abort();
        }
        for route in self.routes.values_mut() {
            if let Some(task) = route.cleanup.take() {
                let _ = task.await;
            }
        }
    }

    fn refresh(&mut self) {
        if self.discovery.is_some() {
            return;
        }
        self.discovery = Some(tokio::spawn(async {
            let sources = tokio::task::spawn_blocking(airloom_capture::enumerate_sources);
            let devices = airloom_airplay::discover(DISCOVERY_WINDOW).await;
            let sources = sources
                .await
                .unwrap_or(Err(airloom_capture::CaptureError::WorkerPanicked));
            DiscoveryResult { devices, sources }
        }));
    }

    fn discovered(&mut self, result: DiscoveryResult) {
        let (sources, source_error) = match result.sources {
            Ok(sources) if sources.is_empty() => (
                sources,
                Some("没有可用的 Windows 输出设备。请连接或启用一个输出设备后重新扫描。".into()),
            ),
            Ok(sources) => (sources, None),
            Err(error) => (
                Vec::new(),
                Some(format!("无法枚举 Windows 输出设备：{error}")),
            ),
        };
        let active = self.running;
        self.update(|s| {
            s.sources.clear();
            if sources.iter().any(|source| source.is_default) {
                s.sources.push(SourceView {
                    id: "default".into(),
                    name: "系统默认输出".into(),
                    kind: "endpoint",
                    is_default: true,
                });
            }
            s.sources
                .extend(sources.into_iter().map(|source| SourceView {
                    id: source.id,
                    name: source.name,
                    kind: "endpoint",
                    is_default: source.is_default,
                }));
            if !active
                && !s
                    .sources
                    .iter()
                    .any(|source| source.id == s.selected_source_id)
            {
                s.selected_source_id = s
                    .sources
                    .first()
                    .map(|source| source.id.clone())
                    .unwrap_or_default();
            }
        });
        if let Some(message) = source_error {
            if self.capture.is_none() {
                self.error("capture_sources", message);
            }
        } else if self
            .global_error
            .as_ref()
            .is_some_and(|error| error.code == "capture_sources")
        {
            self.global_error = None;
        }
        match result.devices {
            Ok(mut devices) => {
                self.online = devices.iter().map(|device| device.id.clone()).collect();
                for device in &devices {
                    if load_access(&device.id).is_some() {
                        self.remembered.insert(device.id.clone());
                    }
                }
                // A short mDNS scan is not proof an active receiver vanished.
                for existing in &self.devices {
                    if self.selected.contains(&existing.id)
                        && !devices.iter().any(|device| device.id == existing.id)
                    {
                        devices.push(existing.clone());
                    }
                }
                self.devices = devices;
                // Discovery never opts a user into a hidden or newly found target.
            }
            Err(error) if self.active_count() == 0 => self.error("discovery", user_error(&error)),
            Err(_) => {}
        }
    }

    fn known_device(&self, id: &str) -> Option<Device> {
        self.devices.iter().find(|device| device.id == id).cloned()
    }

    fn select(&mut self, id: &str, selected: bool) {
        if self.known_device(id).is_none() {
            return;
        }
        if selected {
            if self.selected.iter().any(|selected| selected == id) {
                return;
            }
            if self.selected.len() >= MAX_SELECTED_DEVICES {
                self.error("device_limit", "当前最多可同时选择 8 台设备。");
                return;
            }
            self.selected.push(id.into());
            self.routes.entry(id.into()).or_default();
            if self.running {
                self.request_connect(id, false, false);
            }
        } else {
            self.stop_route(id);
            self.selected.retain(|selected| selected != id);
            if self.selected.is_empty() {
                self.running = false;
            }
        }
    }

    fn cancel_operation(&mut self, id: &str) {
        self.queue.retain(|request| request.device_id != id);
        if self.work.as_ref().is_some_and(|work| work.device_id == id) {
            if let Some(work) = self.work.take() {
                work.task.abort();
            }
        }
        if self
            .auth
            .as_ref()
            .is_some_and(|prompt| prompt.device_id == id)
        {
            self.auth = None;
        }
        if self
            .pairing
            .as_ref()
            .is_some_and(|(device_id, _)| device_id == id)
        {
            self.pairing = None;
        }
    }

    fn stop_route(&mut self, id: &str) {
        self.cancel_operation(id);
        if let Some(route) = self.routes.get_mut(id) {
            route.generation = route.generation.wrapping_add(1);
            route.wanted = false;
            route.retry_at = None;
            route.retries = 0;
            route.remember_after_connect = false;
            route.phase = Phase::Idle;
            route.message = "已停止".into();
            route.error = None;
            if let Some(session) = route.session.take() {
                route.last_metrics = lock(&session.progress).clone();
                route.cleanup = Some(tokio::spawn(session.shutdown()));
            }
        }
        if self.active_count() == 0 {
            self.stop_capture();
        }
    }

    fn stop_capture(&mut self) {
        let error = self
            .capture
            .take()
            .and_then(|mut capture| capture.stream.stop().err());
        self.test_tone = None;
        self.update(|s| {
            s.level = Levels::default();
            s.waveform.fill(0.0);
            s.metrics = Metrics::default();
            s.elapsed_seconds = 0;
            s.capture = CaptureView::default();
        });
        if error.is_some() {
            self.error(
                "capture_stop",
                "音频采集已停止；请检查本机输出设备的静音状态。",
            );
        }
    }

    fn stop_all(&mut self) {
        // Restore owned endpoint mute before any receiver's network teardown.
        self.stop_capture();
        self.running = false;
        self.queue.clear();
        self.auth = None;
        self.pairing = None;
        if let Some(work) = self.work.take() {
            work.task.abort();
        }
        let ids: Vec<_> = self.routes.keys().cloned().collect();
        for id in ids {
            self.stop_route(&id);
        }
    }

    fn request_connect(&mut self, id: &str, pair: bool, priority: bool) {
        if !self.selected.iter().any(|selected| selected == id) {
            return;
        }
        if self.work.as_ref().is_some_and(|work| work.device_id == id)
            || self.auth.as_ref().is_some_and(|prompt| prompt.device_id == id)
        {
            return;
        }
        let route = self.routes.entry(id.into()).or_default();
        if route.session.is_some() && !pair {
            return;
        }
        route.wanted = true;
        route.retry_at = None;
        if route.phase != Phase::Reconnecting {
            route.phase = Phase::Connecting;
            route.message = "等待建立连接…".into();
        }
        route.error = None;
        self.queue.retain(|request| request.device_id != id);
        let request = ConnectionRequest {
            device_id: id.into(),
            pair,
        };
        if priority {
            self.queue.push_front(request);
        } else {
            self.queue.push_back(request);
        }
    }

    fn matches_auth(&self, request_id: u64) -> bool {
        self.auth.as_ref().is_some_and(|prompt| {
            prompt.request_id == request_id
                && self.selected.contains(&prompt.device_id)
                && self
                    .routes
                    .get(&prompt.device_id)
                    .is_some_and(|route| route.wanted)
        })
    }

    fn command_target(&self, device_id: Option<String>, request_id: Option<u64>) -> Option<String> {
        if let Some(request_id) = request_id {
            if !self.matches_auth(request_id) {
                return None;
            }
            let prompt = self.auth.as_ref()?;
            if device_id.as_ref().is_some_and(|id| id != &prompt.device_id) {
                return None;
            }
            Some(prompt.device_id.clone())
        } else {
            device_id
                .or_else(|| self.auth.as_ref().map(|prompt| prompt.device_id.clone()))
                .or_else(|| self.selected.first().cloned())
        }
    }

    fn forget_pending_access(&mut self, id: &str) {
        if let Some(route) = self.routes.get_mut(id) {
            route.access = Access::default();
            route.access_loaded = true;
            route.remember_after_connect = false;
        }
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Refresh {} => self.refresh(),
            Command::SelectDevice { device_id } => {
                if self.known_device(&device_id).is_none() {
                    return;
                }
                self.stop_all();
                self.selected.clear();
                self.select(&device_id, true);
                self.global_error = None;
            }
            Command::ToggleDevice {
                device_id,
                selected,
            } => {
                let selected = selected.unwrap_or_else(|| !self.selected.contains(&device_id));
                self.select(&device_id, selected);
            }
            Command::SelectSource { source_id } => {
                if !lock(&self.state)
                    .sources
                    .iter()
                    .any(|source| source.id == source_id)
                {
                    return;
                }
                let resume = self.running;
                self.stop_all();
                self.update(|s| s.selected_source_id = source_id);
                self.global_error = None;
                if resume {
                    self.running = true;
                    for id in self.selected.clone() {
                        self.request_connect(&id, false, false);
                    }
                }
            }
            Command::Connect { device_id } => {
                if device_id
                    .as_ref()
                    .is_some_and(|id| self.known_device(id).is_none())
                {
                    self.error("no_device", "设备已不在发现列表中，请重新扫描。");
                    return;
                }
                self.global_error = None;
                self.running = true;
                if let Some(id) = device_id {
                    self.select(&id, true);
                    if let Some(route) = self.routes.get_mut(&id) {
                        route.retries = 0;
                    }
                    self.request_connect(&id, false, false);
                } else {
                    if self.selected.is_empty() {
                        self.running = false;
                        self.error("no_device", "请先选择一个或多个已发现的 AirPlay 设备。");
                    }
                    for id in self.selected.clone() {
                        if self.work.as_ref().is_some_and(|work| work.device_id == id)
                            || self
                                .auth
                                .as_ref()
                                .is_some_and(|prompt| prompt.device_id == id)
                        {
                            continue;
                        }
                        if let Some(route) = self.routes.get_mut(&id) {
                            route.retries = 0;
                        }
                        self.request_connect(&id, false, false);
                    }
                }
            }
            Command::Pair {
                device_id,
                request_id,
            } => {
                let Some(id) = self.command_target(device_id, request_id) else {
                    return;
                };
                self.select(&id, true);
                self.stop_route(&id);
                if let Some(route) = self.routes.get_mut(&id) {
                    route.access = Access::default();
                    route.access_loaded = true;
                }
                self.running = true;
                self.request_connect(&id, true, true);
            }
            Command::Disconnect {
                device_id,
                request_id,
            } => {
                if device_id.is_none() && request_id.is_none() {
                    self.stop_all();
                    if self
                        .global_error
                        .as_ref()
                        .is_none_or(|error| error.code != "capture_stop")
                    {
                        self.global_error = None;
                    }
                } else if let Some(id) = self.command_target(device_id, request_id) {
                    self.stop_route(&id);
                }
            }
            Command::SetVolume { volume } => {
                self.update(|s| s.volume = volume);
                for route in self.routes.values() {
                    if let Some(session) = &route.session {
                        let _ = session.volume.send(volume);
                    }
                }
            }
            Command::SetMuteLocal { enabled } => {
                let result = self
                    .capture
                    .as_ref()
                    .map(|capture| capture.stream.set_local_mute(enabled));
                if matches!(result, Some(Err(_))) {
                    self.error("local_mute", "无法修改本机静音，请检查输出设备。");
                } else {
                    self.update(|s| s.mute_local = enabled);
                }
            }
            Command::TestTone { enabled } => {
                if !enabled.unwrap_or(true) {
                    self.test_tone = None;
                } else if self.active_count() > 0 {
                    self.test_tone = Some(TestTone::new());
                } else {
                    self.error("test_tone", "请先连接至少一台扬声器，再播放测试音。");
                }
            }
            Command::Authenticate {
                request_id,
                value,
                remember,
            } => {
                if !self.matches_auth(request_id) {
                    return;
                }
                let id = self.auth.take().unwrap().device_id;
                let route = self.routes.get_mut(&id).unwrap();
                route.remember_after_connect = remember;
                if let Some((pairing_id, pairing)) = self.pairing.take() {
                    if pairing_id != id {
                        return;
                    }
                    route.phase = Phase::Connecting;
                    route.message = "正在验证配对码…".into();
                    self.work = Some(ConnectionWork {
                        device_id: id,
                        generation: route.generation,
                        task: tokio::spawn(async move {
                            Work::Paired(
                                tokio::time::timeout(CONNECT_TIMEOUT, pairing.finish(&value))
                                    .await
                                    .unwrap_or(Err(AirplayError::Timeout("pairing"))),
                            )
                        }),
                    });
                } else {
                    route.access.password = Some(value);
                    route.access.credentials = None;
                    route.access_loaded = true;
                    self.request_connect(&id, false, true);
                }
            }
            Command::ForgetCredentials { device_id } => {
                self.forget_pending_access(&device_id);
                match credentials::delete(CREDENTIAL_SERVICE, &device_id) {
                    Ok(()) => {
                        self.remembered.remove(&device_id);
                    }
                    Err(_) => self.error("credentials_delete", "无法删除保存的凭据，请重试。"),
                }
            }
        }
    }

    fn source_available(&self) -> bool {
        let state = lock(&self.state);
        state
            .sources
            .iter()
            .any(|source| source.id == state.selected_source_id)
    }

    fn begin_next(&mut self) {
        if self.work.is_some() || self.auth.is_some() {
            return;
        }
        let attempts = self.queue.len();
        for _ in 0..attempts {
            let Some(request) = self.queue.pop_front() else {
                break;
            };
            let Some(route) = self.routes.get_mut(&request.device_id) else {
                continue;
            };
            if !route.wanted || route.session.is_some() {
                continue;
            }
            if route
                .cleanup
                .as_ref()
                .is_some_and(|task| !task.is_finished())
            {
                self.queue.push_back(request);
                continue;
            }
            route.cleanup = None;
            self.begin_connect(&request.device_id, request.pair);
            if self.work.is_some() || self.auth.is_some() {
                break;
            }
        }
    }

    fn begin_connect(&mut self, id: &str, pair: bool) {
        if !self.source_available() {
            self.device_error(
                id,
                "no_source",
                "请先选择一个可用的 Windows 输出设备。".into(),
            );
            return;
        }
        let Some(device) = self.known_device(id) else {
            self.device_error(id, "no_device", "设备已不在发现列表中，请重新扫描。".into());
            return;
        };
        let volume = lock(&self.state).volume;
        let route = self.routes.get_mut(id).unwrap();
        if !route.access_loaded {
            route.access = load_access(id).unwrap_or_default();
            route.access_loaded = true;
        }
        let pair = pair || (device.requires_pin && route.access.credentials.is_none());
        if !pair
            && device.requires_password
            && route.access.password.is_none()
            && route.access.credentials.is_none()
        {
            self.prompt(id, AuthenticationKind::Password, false);
            return;
        }
        route.generation = route.generation.wrapping_add(1).max(1);
        route.error = None;
        route.phase = if route.retries > 0 {
            Phase::Reconnecting
        } else {
            Phase::Connecting
        };
        route.message = if pair {
            "正在请求设备配对…"
        } else {
            "正在建立安全连接…"
        }
        .into();
        let generation = route.generation;
        let task = if pair {
            tokio::spawn(async move {
                Work::PairingStarted(
                    tokio::time::timeout(CONNECT_TIMEOUT, PairingSession::start(&device))
                        .await
                        .unwrap_or(Err(AirplayError::Timeout("start pairing"))),
                )
            })
        } else {
            let options = ConnectOptions {
                password: route
                    .access
                    .password
                    .as_ref()
                    .map(|value| value.to_string()),
                credentials: route.access.credentials.clone(),
                initial_volume: volume,
                ..Default::default()
            };
            tokio::spawn(async move {
                Work::Connected(
                    tokio::time::timeout(CONNECT_TIMEOUT, Sender::connect(device, options))
                        .await
                        .unwrap_or(Err(AirplayError::Timeout("connect"))),
                )
            })
        };
        self.work = Some(ConnectionWork {
            device_id: id.into(),
            generation,
            task,
        });
    }

    fn prompt(&mut self, id: &str, kind: AuthenticationKind, rejected: bool) {
        self.auth_generation = self.auth_generation.wrapping_add(1).max(1);
        let is_pin = kind == AuthenticationKind::Pin;
        let message = if is_pin {
            if rejected {
                "配对码验证失败，请重新配对。"
            } else {
                "请输入设备显示或要求的配对码。"
            }
        } else if rejected {
            "设备拒绝了凭据，请重新输入设备密码，或使用配对码。"
        } else {
            "请输入家庭 App 中设置的 AirPlay 扬声器密码。"
        };
        let device_name = self
            .known_device(id)
            .map(|device| device.name)
            .unwrap_or_else(|| id.into());
        self.auth = Some(AuthPrompt {
            request_id: self.auth_generation,
            kind: if is_pin { "pin" } else { "password" },
            device_id: id.into(),
            device_name,
            message: message.into(),
        });
        if let Some(route) = self.routes.get_mut(id) {
            route.phase = Phase::AuthRequired;
            route.message = message.into();
            route.error = rejected.then(|| AppError {
                code: "authentication_rejected".into(),
                message: message.into(),
            });
        }
    }

    fn start_capture(&mut self) -> airloom_capture::Result<()> {
        if self.capture.is_some() {
            return Ok(());
        }
        let config = {
            let state = lock(&self.state);
            CaptureConfig {
                source_id: state.selected_source_id.clone(),
                target_sample_rate: airloom_airplay::SAMPLE_RATE,
                queue_capacity: 8,
                mute_local: state.mute_local,
            }
        };
        let stream = CaptureStream::start(config)?;
        let info = stream.info()?;
        let now = Instant::now();
        self.capture = Some(CaptureSession {
            stream,
            info,
            buffer: CaptureBuffer::new(now),
            started: now,
            ready_at: now + Duration::from_millis(30),
        });
        Ok(())
    }

    fn completed(&mut self, id: &str, work: Work) {
        match work {
            Work::Connected(Ok(sender)) => {
                if let Err(error) = self.start_capture() {
                    self.stop_all();
                    let cleanup = tokio::spawn(async move {
                        let mut sender = sender;
                        let _ =
                            tokio::time::timeout(Duration::from_millis(750), sender.stop()).await;
                    });
                    if let Some(route) = self.routes.get_mut(id) {
                        route.cleanup = Some(cleanup);
                    }
                    self.device_error(
                        id,
                        "capture_start",
                        format!("无法采集所选输出设备：{error}"),
                    );
                    self.error("capture_start", format!("无法采集所选输出设备：{error}"));
                    return;
                }
                let volume = lock(&self.state).volume;
                let route = self.routes.get_mut(id).unwrap();
                if let Some(credentials) = sender.credentials() {
                    route.access.credentials = Some(credentials.clone());
                }
                route.session = Some(ActiveTarget::spawn(
                    sender,
                    id.into(),
                    route.generation,
                    volume,
                    self.events.clone(),
                ));
                route.phase = Phase::Streaming;
                route.message = "正在发送声音".into();
                route.error = None;
                self.persist_access(id);
            }
            Work::Connected(Err(error)) => self.connection_error(id, error),
            Work::PairingStarted(Ok(pairing)) => {
                self.pairing = Some((id.into(), pairing));
                self.prompt(id, AuthenticationKind::Pin, false);
            }
            Work::PairingStarted(Err(error)) => {
                self.device_error(id, "pairing_start", user_error(&error))
            }
            Work::Paired(Ok(credentials)) => {
                let route = self.routes.get_mut(id).unwrap();
                route.access.credentials = Some(credentials);
                route.access.password = None;
                self.request_connect(id, false, true);
            }
            Work::Paired(Err(error)) => self.device_error(
                id,
                "pairing_failed",
                format!("{} 请重新开始配对。", user_error(&error)),
            ),
        }
    }

    fn finish_work(
        &mut self,
        id: &str,
        generation: u64,
        result: Result<Work, tokio::task::JoinError>,
    ) {
        if !self
            .routes
            .get(id)
            .is_some_and(|route| route.generation == generation && route.wanted)
        {
            return;
        }
        match result {
            Ok(work) => self.completed(id, work),
            Err(_) => self.device_error(id, "connection_worker", "连接任务中断，请重试。".into()),
        }
    }

    fn persist_access(&mut self, id: &str) {
        let Some(route) = self.routes.get_mut(id) else {
            return;
        };
        if !route.remember_after_connect {
            return;
        }
        let result = serde_json::to_vec(&route.access)
            .ok()
            .map(Zeroizing::new)
            .map(|bytes| credentials::save(CREDENTIAL_SERVICE, id, &bytes));
        route.remember_after_connect = false;
        if matches!(result, Some(Ok(()))) {
            self.remembered.insert(id.into());
        } else {
            route.error = Some(AppError {
                code: "credentials_save".into(),
                message: "本次连接成功，但无法保存此设备的凭据。".into(),
            });
        }
    }

    fn device_error(&mut self, id: &str, code: &str, message: String) {
        self.cancel_operation(id);
        if let Some(route) = self.routes.get_mut(id) {
            route.phase = Phase::Error;
            route.message = message.clone();
            route.error = Some(AppError {
                code: code.into(),
                message,
            });
            route.wanted = false;
            route.retry_at = None;
        }
    }

    fn connection_error(&mut self, id: &str, error: AirplayError) {
        match error {
            AirplayError::AuthenticationRequired {
                kind: AuthenticationKind::Pin,
            } => self.request_connect(id, true, true),
            AirplayError::AuthenticationRequired { kind } => self.prompt(id, kind, false),
            AirplayError::AuthenticationRejected { kind } => {
                if let Some(route) = self.routes.get_mut(id) {
                    route.access = Access::default();
                    route.access_loaded = true;
                    route.remember_after_connect = false;
                }
                if matches!(
                    kind,
                    AuthenticationKind::Pin | AuthenticationKind::Credentials
                ) {
                    self.request_connect(id, true, true);
                } else {
                    self.prompt(id, kind, true);
                }
            }
            other => {
                let retry = self.routes.get(id).is_some_and(|route| route.retries > 0)
                    && matches!(
                        &other,
                        AirplayError::Timeout(_)
                            | AirplayError::Io(_)
                            | AirplayError::SessionLost(_)
                    );
                if retry {
                    self.retry_stream(id, user_error(&other));
                } else {
                    self.device_error(id, "connect", user_error(&other));
                }
            }
        }
    }

    fn retry_stream(&mut self, id: &str, message: String) {
        let Some(route) = self.routes.get_mut(id) else {
            return;
        };
        if route.retries < 3 && route.wanted {
            route.retries += 1;
            route.retry_at = Some(Instant::now() + Duration::from_secs(2 * route.retries as u64));
            route.phase = Phase::Reconnecting;
            route.message = format!("连接中断，稍后重试（{}/3）。{message}", route.retries);
            route.error = Some(AppError {
                code: "reconnecting".into(),
                message,
            });
        } else {
            self.device_error(id, "stream", message);
        }
    }

    fn target_failed(&mut self, failure: TargetFailure) {
        let id = failure.identity.device_id;
        let Some(route) = self.routes.get_mut(&id) else {
            return;
        };
        if route.generation != failure.identity.generation || route.session.is_none() {
            return;
        }
        if let Some(session) = route.session.take() {
            route.last_metrics = lock(&session.progress).clone();
            route.cleanup = Some(tokio::spawn(session.shutdown()));
        }
        if self.active_count() == 0 {
            self.stop_capture();
        }
        self.retry_stream(&id, failure.message);
    }

    fn maintain_routes(&mut self) {
        let now = Instant::now();
        let mut ready = Vec::new();
        for (id, route) in &mut self.routes {
            if route.cleanup.as_ref().is_some_and(Task::is_finished) {
                route.cleanup = None;
            }
            if route
                .session
                .as_ref()
                .is_some_and(|session| session.started.elapsed() >= Duration::from_secs(30))
            {
                route.retries = 0;
            }
            if route.retry_at.is_some_and(|at| now >= at) {
                route.retry_at = None;
                ready.push(id.clone());
            }
        }
        for id in ready {
            self.request_connect(&id, false, false);
        }
    }

    fn fan_out(&mut self, block: Arc<[i16]>) {
        let block = MediaBlock {
            samples: block,
            captured_at: Instant::now(),
        };
        let mut failed = Vec::new();
        for (id, route) in &self.routes {
            if let Some(session) = &route.session {
                if let Err(error) = session.audio.try_send(block.clone()) {
                    let message = match error {
                        mpsc::error::TrySendError::Full(_) => {
                            "这台设备的发送队列已满，正在重新连接。"
                        }
                        mpsc::error::TrySendError::Closed(_) => {
                            "这台设备的发送任务已结束，正在重新连接。"
                        }
                    };
                    failed.push(TargetFailure {
                        identity: TargetIdentity {
                            device_id: id.clone(),
                            generation: route.generation,
                        },
                        message: message.into(),
                    });
                }
            }
        }
        for failure in failed {
            self.target_failed(failure);
        }
    }

    fn audio(&mut self) {
        let Some(capture) = &mut self.capture else {
            return;
        };
        let now = Instant::now();
        let mut capture_error = None;
        for _ in 0..8 {
            match capture.stream.read_timeout(Duration::ZERO) {
                Ok(Some(chunk)) => capture.buffer.ingest(chunk, now),
                Ok(None) => break,
                Err(error) => {
                    capture_error = Some(error);
                    break;
                }
            }
        }
        if let Some(error) = capture_error {
            self.stop_all();
            let message = format!("音频采集已中断：{error}");
            self.error("capture", message.clone());
            self.update(|s| {
                s.capture.state = "error";
                s.capture.message = message;
            });
            return;
        }
        if now < capture.ready_at {
            return;
        }
        let mut samples = capture.buffer.next_block();
        let muted = lock(&self.state).mute_local;
        let diagnostic = capture.buffer.view(&capture.info, muted, now);
        let elapsed = capture.started.elapsed().as_secs();
        let dropped = capture.buffer.capture_dropped + capture.buffer.queue_dropped;
        let underrun = capture.buffer.underrun_frames;
        let buffered_ms = capture.buffer.pending.len() as f32 / 2.0 / 44.1;
        if let Some(tone) = &mut self.test_tone {
            tone.apply(&mut samples);
            if tone.finished() {
                self.test_tone = None;
            }
        }
        let level = Levels {
            left: samples
                .iter()
                .step_by(2)
                .map(|sample| sample.abs())
                .fold(0.0, f32::max),
            right: samples
                .iter()
                .skip(1)
                .step_by(2)
                .map(|sample| sample.abs())
                .fold(0.0, f32::max),
        };
        let waveform = model::waveform(&samples);
        let block: Arc<[i16]> = model::pcm_i16(&samples).into();
        self.update(|s| {
            s.level = level;
            s.waveform = waveform;
            s.elapsed_seconds = elapsed;
            s.capture = diagnostic;
            s.metrics.sample_rate = airloom_airplay::SAMPLE_RATE;
            s.metrics.buffer_ms = Some(buffered_ms);
            s.metrics.dropped_frames = dropped;
            s.metrics.underrun_frames = underrun;
        });
        self.fan_out(block);
    }
}

fn load_access(device_id: &str) -> Option<Access> {
    let data = Zeroizing::new(credentials::load(CREDENTIAL_SERVICE, device_id).ok()??);
    serde_json::from_slice(&data).ok()
}

fn user_error(error: &AirplayError) -> String {
    match error {
        AirplayError::AuthenticationRequired { .. } => "设备需要验证密码或配对码。".into(),
        AirplayError::AuthenticationRejected { .. } => "设备没有接受本次认证。".into(),
        AirplayError::AccessDenied { status: 403, .. } => {
            "设备拒绝访问。请检查家庭 App 中的扬声器访问范围；这不一定是密码错误。".into()
        }
        AirplayError::AccessDenied { status, .. } => format!("设备拒绝建立会话（状态 {status}）。"),
        AirplayError::Receiver { stage, status } => {
            format!("设备在 {stage} 阶段返回状态 {status}。")
        }
        AirplayError::Timeout(_) => "设备响应超时，请检查同一局域网连接并重试。".into(),
        AirplayError::Unsupported(_) => "设备要求当前版本尚未支持的协议能力。".into(),
        AirplayError::Protocol(_) => "设备返回了无法验证的协议数据，连接已停止。".into(),
        AirplayError::Discovery(_) => "无法扫描局域网。请检查网络权限、组播和防火墙设置。".into(),
        AirplayError::Stopped => "投放已停止。".into(),
        AirplayError::InvalidArgument(_) => "连接参数无效，请重新选择设备。".into(),
        AirplayError::Io(error) => format!("网络连接失败（{:?}）。", error.kind()),
        AirplayError::SessionLost(_) => "与设备的会话已断开。".into(),
    }
}
