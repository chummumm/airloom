use crate::model::{
    self, AppError, AuthPrompt, Command, DeviceView, Levels, Metrics, Phase, Snapshot, SourceView,
};
use airloom_airplay::{
    AuthenticationKind, ConnectOptions, Device, Error as AirplayError, PairingCredentials,
    PairingSession, Sender,
};
use airloom_capture::{credentials, CaptureConfig, CaptureSource, CaptureStream};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use tokio::task::JoinHandle as Task;
use zeroize::Zeroizing;

const CREDENTIAL_SERVICE: &str = "Airloom";
const DISCOVERY_WINDOW: Duration = Duration::from_secs(4);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(25);

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

fn lock(state: &Mutex<Snapshot>) -> MutexGuard<'_, Snapshot> {
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

struct ActiveSession {
    sender: Sender,
    capture: CaptureStream,
    started: Instant,
    ready_at: tokio::time::Instant,
    pending: VecDeque<f32>,
    capture_dropped: u64,
    queue_dropped: u64,
    underrun_frames: u64,
}

enum Work {
    Connected(Result<Sender, AirplayError>),
    PairingStarted(Result<PairingSession, AirplayError>),
    Paired(Result<PairingCredentials, AirplayError>),
}

struct DiscoveryResult {
    devices: Result<Vec<Device>, AirplayError>,
    sources: airloom_capture::Result<Vec<CaptureSource>>,
}

struct Actor {
    state: Arc<Mutex<Snapshot>>,
    shutdown: Arc<AtomicBool>,
    devices: Vec<Device>,
    discovery: Option<Task<DiscoveryResult>>,
    work: Option<Task<Work>>,
    volume_work: Option<Task<Result<(), AirplayError>>>,
    pending_volume: Option<f32>,
    pairing: Option<PairingSession>,
    session: Option<ActiveSession>,
    access: Access,
    remember_after_connect: bool,
    retry_at: Option<Instant>,
    retries: u8,
    auth_generation: u64,
}

impl Actor {
    fn new(state: Arc<Mutex<Snapshot>>, shutdown: Arc<AtomicBool>) -> Self {
        Self {
            state,
            shutdown,
            devices: Vec::new(),
            discovery: None,
            work: None,
            volume_work: None,
            pending_volume: None,
            pairing: None,
            session: None,
            access: Access::default(),
            remember_after_connect: false,
            retry_at: None,
            retries: 0,
            auth_generation: 0,
        }
    }

    fn update(&self, edit: impl FnOnce(&mut Snapshot)) {
        let mut snapshot = lock(&self.state);
        edit(&mut snapshot);
        snapshot.revision = snapshot.revision.wrapping_add(1);
    }

    fn phase(&self, phase: Phase, message: &str) {
        self.update(|s| {
            s.state = phase;
            s.status_message = message.into();
            s.auth = None;
            if s.error
                .as_ref()
                .is_none_or(|error| error.code != "capture_stop")
            {
                s.error = None;
            }
        });
    }

    fn failure(&self, code: &str, message: impl Into<String>) {
        let message = message.into();
        self.update(|s| {
            s.state = Phase::Error;
            s.status_message = message.clone();
            s.error = Some(AppError {
                code: code.into(),
                message,
            });
            s.auth = None;
            s.level = Levels::default();
            s.waveform.fill(0.0);
        });
    }

    async fn run(mut self, mut commands: mpsc::Receiver<Command>) {
        self.refresh();
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            if self.shutdown.load(Ordering::Acquire) {
                break;
            }
            tokio::select! {
                command = commands.recv() => match command {
                    Some(command) => self.command(command).await,
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
                        Err(_) => self.failure("discovery_worker", "设备扫描中断，请重新扫描。"),
                    }
                },
                result = async {
                    match &mut self.work {
                        Some(task) => task.await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.work = None;
                    match result {
                        Ok(work) => self.completed(work).await,
                        Err(_) => self.failure("connection_worker", "连接任务中断，请重新连接。"),
                    }
                },
                result = async {
                    match &mut self.volume_work {
                        Some(task) => task.await,
                        None => std::future::pending().await,
                    }
                } => {
                    self.volume_work = None;
                    if matches!(result, Ok(Ok(()))) {
                        self.start_volume_update();
                    } else {
                        self.pending_volume = None;
                        self.update(|s| s.error = Some(AppError {
                            code: "volume".into(), message: "设备暂时没有确认音量变化。".into(),
                        }));
                    }
                },
                _ = async {
                    match &self.session {
                        Some(session) => {
                            tokio::time::sleep_until(session.ready_at).await;
                            tokio::task::yield_now().await;
                        }
                        None => std::future::pending().await,
                    }
                } => {
                    // Only readiness is raced in select. A selected send_pcm
                    // operation runs to completion, so ordinary commands never
                    // cancel an in-progress packet/crypto-state transition.
                    // Sender alone paces media; a second 10 ms timer would skip
                    // input blocks when one call emits two 352-frame packets.
                    self.audio().await;
                },
                _ = tick.tick() => {
                    if self.retry_at.is_some_and(|at| Instant::now() >= at) {
                        self.retry_at = None;
                        self.begin_connect(true);
                    }
                },
            }
        }
        self.cancel_work();
        if let Some(task) = self.discovery.take() {
            task.abort();
        }
        self.stop_session().await;
    }

    fn refresh(&mut self) {
        if self.discovery.is_some() {
            return;
        }
        if !matches!(
            lock(&self.state).state,
            Phase::Connecting | Phase::Reconnecting | Phase::AuthRequired | Phase::Streaming
        ) {
            self.phase(Phase::Discovering, "正在寻找同一局域网中的 AirPlay 设备…");
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
        self.update(|s| {
            let active = matches!(
                s.state,
                Phase::Connecting | Phase::Reconnecting | Phase::AuthRequired | Phase::Streaming
            );
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
            // Do not retarget an active/reconnecting stream as a side effect of
            // discovery. Idle selection can fall back to an actual endpoint.
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
            if let Some(message) = source_error {
                s.error = Some(AppError {
                    code: "capture_sources".into(),
                    message: message.clone(),
                });
                if !active {
                    s.state = Phase::Error;
                    s.status_message = message;
                }
            } else if s
                .error
                .as_ref()
                .is_some_and(|error| error.code == "capture_sources")
            {
                s.error = None;
            }
        });
        match result.devices {
            Ok(devices) => {
                let views = devices
                    .iter()
                    .map(|device| DeviceView {
                        id: device.id.clone(),
                        name: device.name.clone(),
                        model: device.model.clone().unwrap_or_else(|| "AirPlay".into()),
                        address: device
                            .addresses
                            .first()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                        password_required: device.requires_password || device.requires_pin,
                        online: true,
                        remembered: load_access(&device.id).is_some(),
                    })
                    .collect();
                self.devices = devices;
                self.update(|s| {
                    s.devices = views;
                    if s.selected_device_id.is_empty() {
                        if let Some(device) = s.devices.first() {
                            s.selected_device_id = device.id.clone();
                        }
                    }
                    if s.state == Phase::Discovering {
                        s.state = Phase::Idle;
                        s.status_message = if s.devices.is_empty() {
                            "还没有发现设备。请确认 HomePod 已唤醒，并与电脑连接同一局域网。"
                        } else {
                            "设备已就绪，选择一个设备开始投放。"
                        }
                        .into();
                    }
                });
            }
            Err(error) => {
                if !matches!(
                    lock(&self.state).state,
                    Phase::Connecting
                        | Phase::Reconnecting
                        | Phase::AuthRequired
                        | Phase::Streaming
                ) {
                    self.failure("discovery", user_error(&error));
                }
            }
        }
    }

    fn selected_device(&self) -> Option<Device> {
        let id = &lock(&self.state).selected_device_id;
        self.devices.iter().find(|device| &device.id == id).cloned()
    }

    fn cancel_work(&mut self) {
        if let Some(task) = self.work.take() {
            task.abort();
        }
        self.pairing = None;
        self.retry_at = None;
    }

    async fn stop_session(&mut self) {
        let mut session = self.session.take();
        // Restore local output before waiting for any network teardown.
        let capture_error = session
            .as_mut()
            .and_then(|session| session.capture.stop().err());
        self.pending_volume = None;
        if let Some(task) = self.volume_work.take() {
            task.abort();
            let _ = task.await;
        }
        if let Some(session) = &mut session {
            let _ = tokio::time::timeout(Duration::from_millis(750), session.sender.stop()).await;
        }
        if capture_error.is_some() {
            self.failure(
                "capture_stop",
                "音频采集已停止；请检查本机输出设备的静音状态。",
            );
        }
        self.update(|s| {
            s.level = Levels::default();
            s.waveform.fill(0.0);
            s.metrics = Metrics::default();
            s.elapsed_seconds = 0;
        });
    }

    fn start_volume_update(&mut self) {
        if self.volume_work.is_some() {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        let Some(volume) = self.pending_volume.take() else {
            return;
        };
        let control = session.sender.volume_control();
        // Keep at most one exchange in flight, retaining only the latest slider
        // value. The protocol layer owns its timeout and poisoned-stream policy.
        self.volume_work = Some(tokio::spawn(
            async move { control.set_volume(volume).await },
        ));
    }

    fn matches_auth(&self, request_id: u64) -> bool {
        let state = lock(&self.state);
        state.state == Phase::AuthRequired
            && state.auth.as_ref().is_some_and(|prompt| {
                prompt.request_id == request_id && prompt.device_id == state.selected_device_id
            })
    }

    fn forget_pending_access(&mut self, device_id: &str) {
        if lock(&self.state).selected_device_id == device_id {
            self.remember_after_connect = false;
            self.access = Access::default();
        }
    }

    async fn command(&mut self, command: Command) {
        match command {
            Command::Refresh {} => self.refresh(),
            Command::SelectDevice { device_id } => {
                if !self.devices.iter().any(|device| device.id == device_id) {
                    return;
                }
                self.cancel_work();
                self.stop_session().await;
                self.access = Access::default();
                self.remember_after_connect = false;
                self.retries = 0;
                self.update(|s| s.selected_device_id = device_id);
                self.phase(Phase::Idle, "设备已选择。");
            }
            Command::SelectSource { source_id } => {
                if !lock(&self.state)
                    .sources
                    .iter()
                    .any(|source| source.id == source_id)
                {
                    return;
                }
                self.cancel_work();
                self.stop_session().await;
                self.access = Access::default();
                self.remember_after_connect = false;
                self.retries = 0;
                self.update(|s| s.selected_source_id = source_id);
                self.phase(Phase::Idle, "声音来源已切换，点击开始投放。");
            }
            Command::Connect {} => {
                self.cancel_work();
                self.stop_session().await;
                self.retries = 0;
                self.access = self
                    .selected_device()
                    .and_then(|device| load_access(&device.id))
                    .unwrap_or_default();
                self.remember_after_connect = false;
                self.begin_connect(false);
            }
            Command::Pair { request_id } => {
                if request_id.is_some_and(|id| !self.matches_auth(id)) {
                    return;
                }
                self.cancel_work();
                self.stop_session().await;
                self.access = Access::default();
                self.remember_after_connect = false;
                self.retries = 0;
                self.begin_pairing();
            }
            Command::Disconnect { request_id } => {
                if request_id.is_some_and(|id| !self.matches_auth(id)) {
                    return;
                }
                self.cancel_work();
                self.stop_session().await;
                self.access = Access::default();
                self.remember_after_connect = false;
                self.retries = 0;
                self.phase(Phase::Idle, "投放已停止。");
            }
            Command::SetVolume { volume } => {
                self.update(|s| s.volume = volume);
                self.pending_volume = Some(volume);
                self.start_volume_update();
            }
            Command::SetMuteLocal { enabled } => {
                let result = self
                    .session
                    .as_ref()
                    .map(|session| session.capture.set_local_mute(enabled));
                match result {
                    Some(Err(_)) => self.update(|s| {
                        s.error = Some(AppError {
                            code: "local_mute".into(),
                            message: "无法修改本机静音，请检查输出设备。".into(),
                        })
                    }),
                    _ => self.update(|s| s.mute_local = enabled),
                }
            }
            Command::Authenticate {
                request_id,
                value,
                remember,
            } => {
                let secret = value;
                if !self.matches_auth(request_id) {
                    return;
                }
                self.remember_after_connect = remember;
                if let Some(pairing) = self.pairing.take() {
                    self.phase(Phase::Connecting, "正在验证配对码…");
                    self.work = Some(tokio::spawn(async move {
                        let result = tokio::time::timeout(CONNECT_TIMEOUT, pairing.finish(&secret))
                            .await
                            .unwrap_or(Err(AirplayError::Timeout("pairing")));
                        Work::Paired(result)
                    }));
                } else {
                    self.access.password = Some(secret);
                    self.access.credentials = None;
                    self.begin_connect(false);
                }
            }
            Command::ForgetCredentials { device_id } => {
                // Revocation also applies to an authentication operation already
                // in flight. Its completion must not recreate the deleted entry.
                self.forget_pending_access(&device_id);
                match credentials::delete(CREDENTIAL_SERVICE, &device_id) {
                    Ok(()) => {
                        self.update(|s| {
                            for device in &mut s.devices {
                                if device.id == device_id {
                                    device.remembered = false;
                                }
                            }
                            s.status_message = "已删除这台电脑上保存的设备凭据。".into();
                        });
                    }
                    Err(_) => self.update(|s| {
                        s.error = Some(AppError {
                            code: "credentials_delete".into(),
                            message: "无法删除保存的凭据，请重试。".into(),
                        })
                    }),
                }
            }
        }
    }

    fn begin_connect(&mut self, reconnecting: bool) {
        let source_available = {
            let state = lock(&self.state);
            state
                .sources
                .iter()
                .any(|source| source.id == state.selected_source_id)
        };
        if !source_available {
            self.failure(
                "no_source",
                "请连接或启用一个 Windows 输出设备，重新扫描并选择声音来源。",
            );
            return;
        }
        let Some(device) = self.selected_device() else {
            self.failure("no_device", "请先选择一个已发现的 AirPlay 设备。");
            return;
        };
        if device.requires_pin && self.access.credentials.is_none() {
            self.begin_pairing();
            return;
        }
        if device.requires_password
            && self.access.password.is_none()
            && self.access.credentials.is_none()
        {
            self.prompt(AuthenticationKind::Password, false);
            return;
        }
        let volume = lock(&self.state).volume;
        self.pending_volume = None;
        let options = ConnectOptions {
            password: self.access.password.as_ref().map(|value| value.to_string()),
            credentials: self.access.credentials.clone(),
            initial_volume: volume,
            ..Default::default()
        };
        self.phase(
            if reconnecting {
                Phase::Reconnecting
            } else {
                Phase::Connecting
            },
            if reconnecting {
                "正在重新连接设备…"
            } else {
                "正在建立安全连接…"
            },
        );
        self.work = Some(tokio::spawn(async move {
            let result = tokio::time::timeout(CONNECT_TIMEOUT, Sender::connect(device, options))
                .await
                .unwrap_or(Err(AirplayError::Timeout("connect")));
            Work::Connected(result)
        }));
    }

    fn begin_pairing(&mut self) {
        let Some(device) = self.selected_device() else {
            self.failure("no_device", "请先选择设备。");
            return;
        };
        self.phase(Phase::Connecting, "正在请求设备配对…");
        self.work = Some(tokio::spawn(async move {
            let result = tokio::time::timeout(CONNECT_TIMEOUT, PairingSession::start(&device))
                .await
                .unwrap_or(Err(AirplayError::Timeout("start pairing")));
            Work::PairingStarted(result)
        }));
    }

    fn prompt(&mut self, kind: AuthenticationKind, rejected: bool) {
        self.auth_generation = self.auth_generation.wrapping_add(1).max(1);
        let request_id = self.auth_generation;
        self.update(|s| {
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
                "此设备需要 AirPlay 密码。请使用家庭 App 中设置的扬声器密码。"
            };
            s.state = Phase::AuthRequired;
            s.status_message = message.into();
            s.auth = Some(AuthPrompt {
                request_id,
                kind: if is_pin { "pin" } else { "password" },
                device_id: s.selected_device_id.clone(),
                message: message.into(),
            });
            s.error = rejected.then(|| AppError {
                code: "authentication_rejected".into(),
                message: message.into(),
            });
        });
    }

    async fn completed(&mut self, work: Work) {
        match work {
            Work::Connected(Ok(mut sender)) => {
                let config = {
                    let state = lock(&self.state);
                    CaptureConfig {
                        source_id: state.selected_source_id.clone(),
                        target_sample_rate: airloom_airplay::SAMPLE_RATE,
                        queue_capacity: 8,
                        mute_local: state.mute_local,
                    }
                };
                match CaptureStream::start(config) {
                    Ok(capture) => {
                        if let Some(credentials) = sender.credentials() {
                            self.access.credentials = Some(credentials.clone());
                        }
                        self.session = Some(ActiveSession {
                            sender,
                            capture,
                            started: Instant::now(),
                            ready_at: tokio::time::Instant::now() + Duration::from_millis(30),
                            pending: VecDeque::new(),
                            capture_dropped: 0,
                            queue_dropped: 0,
                            underrun_frames: 0,
                        });
                        self.phase(Phase::Streaming, "正在将电脑声音发送到设备。");
                        self.persist_access();
                        self.start_volume_update();
                    }
                    Err(error) => {
                        let _ =
                            tokio::time::timeout(Duration::from_millis(750), sender.stop()).await;
                        self.failure("capture_start", format!("无法采集所选输出设备：{error}"));
                    }
                }
            }
            Work::Connected(Err(error)) => self.connection_error(error),
            Work::PairingStarted(Ok(pairing)) => {
                self.pairing = Some(pairing);
                self.prompt(AuthenticationKind::Pin, false);
            }
            Work::PairingStarted(Err(error)) => self.failure("pairing_start", user_error(&error)),
            Work::Paired(Ok(credentials)) => {
                self.access.credentials = Some(credentials);
                self.access.password = None;
                self.begin_connect(false);
            }
            Work::Paired(Err(error)) => {
                // A failed SRP exchange cannot be reused. Offer a fresh explicit
                // pairing action rather than sending another PIN on stale state.
                self.failure(
                    "pairing_failed",
                    format!("{} 请重新开始配对。", user_error(&error)),
                );
            }
        }
    }

    fn persist_access(&mut self) {
        if !self.remember_after_connect {
            return;
        }
        let id = lock(&self.state).selected_device_id.clone();
        let result = serde_json::to_vec(&self.access)
            .ok()
            .map(Zeroizing::new)
            .map(|bytes| credentials::save(CREDENTIAL_SERVICE, &id, &bytes));
        if matches!(result, Some(Ok(()))) {
            self.update(|s| {
                for device in &mut s.devices {
                    if device.id == id {
                        device.remembered = true;
                    }
                }
            });
        } else {
            self.update(|s| {
                s.error = Some(AppError {
                    code: "credentials_save".into(),
                    message: "本次连接成功，但无法在此电脑保存凭据。".into(),
                })
            });
        }
        self.remember_after_connect = false;
    }

    fn connection_error(&mut self, error: AirplayError) {
        match error {
            AirplayError::AuthenticationRequired {
                kind: AuthenticationKind::Pin,
            } => self.begin_pairing(),
            AirplayError::AuthenticationRequired { kind } => self.prompt(kind, false),
            AirplayError::AuthenticationRejected { kind } => {
                self.access = Access::default();
                self.remember_after_connect = false;
                if matches!(
                    kind,
                    AuthenticationKind::Pin | AuthenticationKind::Credentials
                ) {
                    // Stored pairing credentials were rejected. A new PIN must
                    // be attached to a fresh SRP session, never used as a password.
                    self.begin_pairing();
                } else {
                    self.prompt(kind, true);
                }
            }
            other => {
                if self.retries > 0
                    && matches!(
                        &other,
                        AirplayError::Timeout(_)
                            | AirplayError::Io(_)
                            | AirplayError::SessionLost(_)
                    )
                {
                    self.retry_stream(user_error(&other));
                } else {
                    self.failure("connect", user_error(&other));
                }
            }
        }
    }

    fn retry_stream(&mut self, message: String) {
        if self.retries < 3 {
            self.retries += 1;
            self.retry_at = Some(Instant::now() + Duration::from_secs(2 * self.retries as u64));
            self.phase(
                Phase::Reconnecting,
                &format!("连接中断，稍后重试（{}/3）。{message}", self.retries),
            );
        } else {
            self.retry_at = None;
            self.failure("stream", message);
        }
    }

    async fn audio(&mut self) {
        let Some(session) = &mut self.session else {
            return;
        };
        let mut failure = None;
        // WASAPI does not necessarily emit packets when no app is rendering.
        // Maintain a continuous media clock, filling missing samples with silence.
        // Both capture's queue and this 80 ms scheduling queue are bounded.
        for _ in 0..8 {
            match session.capture.read_timeout(Duration::ZERO) {
                Ok(Some(chunk)) => {
                    session.capture_dropped = chunk.dropped_frames;
                    session.pending.extend(chunk.samples);
                }
                Ok(None) => break,
                Err(error) => {
                    failure = Some(format!("音频采集已中断：{error}"));
                    break;
                }
            }
        }
        const TICK_FRAMES: usize = 441;
        const MAX_SAMPLES: usize = TICK_FRAMES * 8 * 2;
        if session.pending.len() > MAX_SAMPLES {
            let discard = (session.pending.len() - MAX_SAMPLES) / 2 * 2;
            session.pending.drain(..discard);
            session.queue_dropped += (discard / 2) as u64;
        }
        if failure.is_none() {
            session.underrun_frames = session
                .underrun_frames
                .saturating_add(TICK_FRAMES.saturating_sub(session.pending.len() / 2) as u64);
            let samples: Vec<f32> = (0..TICK_FRAMES * 2)
                .map(|_| session.pending.pop_front().unwrap_or(0.0))
                .collect();
            let levels = Levels {
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
            let pcm = model::pcm_i16(&samples);
            match tokio::time::timeout(Duration::from_secs(2), session.sender.send_pcm(&pcm)).await
            {
                Ok(Ok(())) => {
                    let elapsed = session.started.elapsed().as_secs();
                    let packets = session.sender.metrics().packets_sent;
                    let dropped = session.capture_dropped + session.queue_dropped;
                    let buffered_ms = session.pending.len() as f32 / 2.0 / 44.1;
                    let underrun_frames = session.underrun_frames;
                    if elapsed >= 30 {
                        self.retries = 0;
                    }
                    self.update(|s| {
                        s.level = levels;
                        s.waveform = waveform;
                        s.elapsed_seconds = elapsed;
                        s.metrics.packets_sent = packets;
                        s.metrics.sample_rate = airloom_airplay::SAMPLE_RATE;
                        s.metrics.buffer_ms = Some(buffered_ms);
                        s.metrics.dropped_frames = dropped;
                        s.metrics.underrun_frames = underrun_frames;
                    });
                }
                Ok(Err(error)) => failure = Some(user_error(&error)),
                Err(_) => failure = Some("发送超时，正在恢复本机输出。".into()),
            }
        }
        if let Some(message) = failure {
            self.stop_session().await;
            self.retry_stream(message);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn actor() -> Actor {
        let state = Snapshot {
            selected_device_id: "test-device".into(),
            ..Snapshot::default()
        };
        Actor::new(
            Arc::new(Mutex::new(state)),
            Arc::new(AtomicBool::new(false)),
        )
    }

    #[tokio::test]
    async fn stale_authentication_dialog_cannot_submit_or_cancel_a_new_prompt() {
        let mut actor = actor();
        actor.prompt(AuthenticationKind::Password, false);
        let stale = lock(&actor.state).auth.as_ref().unwrap().request_id;
        actor.prompt(AuthenticationKind::Password, true);
        let current = lock(&actor.state).auth.as_ref().unwrap().request_id;
        actor.access.password = Some(Zeroizing::new("current-attempt".into()));
        actor.remember_after_connect = true;

        actor
            .command(Command::Authenticate {
                request_id: stale,
                value: Zeroizing::new("stale-dialog-secret".into()),
                remember: false,
            })
            .await;
        actor
            .command(Command::Pair {
                request_id: Some(stale),
            })
            .await;
        actor
            .command(Command::Disconnect {
                request_id: Some(stale),
            })
            .await;
        assert!(actor.matches_auth(current));
        assert_eq!(
            actor.access.password.as_ref().map(|value| value.as_str()),
            Some("current-attempt")
        );
        assert!(actor.remember_after_connect);
        assert!(actor.work.is_none());

        actor
            .command(Command::Disconnect {
                request_id: Some(current),
            })
            .await;
        assert!(lock(&actor.state).state == Phase::Idle);
        assert!(lock(&actor.state).auth.is_none());
        assert!(actor.access.password.is_none());
        assert!(!actor.remember_after_connect);
    }

    #[tokio::test]
    async fn refresh_preserves_an_outstanding_password_prompt() {
        let mut actor = actor();
        actor.prompt(AuthenticationKind::Password, false);
        let request = lock(&actor.state).auth.as_ref().unwrap().request_id;
        actor.refresh();
        assert!(actor.matches_auth(request));
        // The current-thread runtime has not polled the scan task; cancel it so
        // this state-regression test performs no actual network or OS discovery.
        actor.discovery.take().unwrap().abort();
    }

    #[tokio::test]
    async fn rejected_saved_pairing_starts_a_fresh_pairing_session() {
        let mut actor = actor();
        let mut device = Device::manual("127.0.0.1".parse().unwrap(), 7000);
        device.id = "test-device".into();
        actor.devices.push(device);
        actor.access.password = Some(Zeroizing::new("obsolete-access".into()));
        actor.remember_after_connect = true;

        actor.connection_error(AirplayError::AuthenticationRejected {
            kind: AuthenticationKind::Credentials,
        });

        assert!(lock(&actor.state).state == Phase::Connecting);
        assert!(lock(&actor.state).auth.is_none());
        assert!(actor.access.password.is_none());
        assert!(!actor.remember_after_connect);
        // No protocol traffic is needed to verify the state transition.
        actor.work.take().expect("fresh pairing task").abort();
    }

    #[test]
    fn forget_revokes_in_flight_remember_consent_for_only_that_device() {
        let mut actor = actor();
        actor.access.password = Some(Zeroizing::new("pending-authentication".into()));
        actor.remember_after_connect = true;
        actor.forget_pending_access("other-device");
        assert!(actor.remember_after_connect);
        actor.forget_pending_access("test-device");
        assert!(!actor.remember_after_connect);
        assert!(actor.access.password.is_none());
        // A later successful connection cannot persist without fresh consent.
        actor.persist_access();
        assert!(lock(&actor.state).error.is_none());
    }

    #[test]
    fn missing_capture_endpoints_never_create_a_fictitious_default_route() {
        let mut actor = actor();
        actor.discovered(DiscoveryResult {
            devices: Ok(Vec::new()),
            sources: Ok(Vec::new()),
        });
        let state = lock(&actor.state);
        assert!(state.sources.is_empty());
        assert!(state.selected_source_id.is_empty());
        assert!(state.state == Phase::Error);
        assert_eq!(state.error.as_ref().unwrap().code, "capture_sources");
    }

    #[test]
    fn idle_selection_uses_a_real_endpoint_if_windows_has_no_default() {
        let mut actor = actor();
        actor.discovered(DiscoveryResult {
            devices: Ok(Vec::new()),
            sources: Ok(vec![CaptureSource {
                id: "endpoint-one".into(),
                name: "Speakers".into(),
                is_default: false,
            }]),
        });
        let state = lock(&actor.state);
        assert_eq!(state.sources.len(), 1);
        assert_eq!(state.selected_source_id, "endpoint-one");
        assert!(!state.sources.iter().any(|source| source.id == "default"));
    }

    #[test]
    fn failed_reconnects_exhaust_the_same_three_attempt_budget() {
        let mut actor = actor();
        actor.retry_stream("stream interrupted".into());
        assert_eq!(actor.retries, 1);
        actor.connection_error(AirplayError::Timeout("connect"));
        assert_eq!(actor.retries, 2);
        assert!(actor.retry_at.is_some());
        actor.connection_error(AirplayError::Timeout("connect"));
        assert_eq!(actor.retries, 3);
        actor.connection_error(AirplayError::Timeout("connect"));
        assert!(actor.retry_at.is_none());
        assert!(lock(&actor.state).state == Phase::Error);
    }
}
