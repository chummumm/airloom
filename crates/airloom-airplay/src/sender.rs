use crate::{
    audio::PacketEncoder,
    crypto,
    diagnostics::{merge_receiver_info, receiver_diagnostics},
    pairing, protocol,
    rtsp::{Connection, Wire},
    timing::{self, TimingRuntime},
    AuthenticationKind, Device, Error, PairingCredentials, Result, SenderDiagnostics,
    StreamDiagnostics, TransportDiagnostics, CHANNELS, FRAMES_PER_PACKET, SAMPLE_RATE,
};
use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io::Cursor,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};
use tokio::{
    net::{TcpStream, UdpSocket},
    sync::Mutex as AsyncMutex,
    task::JoinHandle,
    time::{interval, timeout, Instant, MissedTickBehavior},
};
use zeroize::Zeroize;

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimingMode {
    #[default]
    Auto,
    Ntp,
    Ptp,
}

pub struct ConnectOptions {
    pub password: Option<String>,
    pub credentials: Option<PairingCredentials>,
    pub latency: Duration,
    pub initial_volume: f32,
    pub timing: TimingMode,
}
impl Default for ConnectOptions {
    fn default() -> Self {
        Self {
            password: None,
            credentials: None,
            latency: Duration::from_millis(750),
            initial_volume: 0.25,
            timing: TimingMode::Auto,
        }
    }
}
impl Drop for ConnectOptions {
    fn drop(&mut self) {
        if let Some(password) = &mut self.password {
            password.zeroize();
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SenderMetrics {
    pub packets_sent: u64,
    pub bytes_sent: u64,
    pub frames_sent: u64,
    pub retransmission_requests: u64,
    pub retransmissions_sent: u64,
    pub timing_exchanges: u64,
    pub reanchors: u64,
    pub buffered_frames: usize,
    pub negotiated_latency_ms: u64,
    pub timing_mode: String,
    pub last_error: Option<String>,
    #[serde(default)]
    pub diagnostics: SenderDiagnostics,
}
#[derive(Default)]
struct Counters {
    packets: AtomicU64,
    bytes: AtomicU64,
    frames: AtomicU64,
    non_silent_packets: AtomicU64,
    last_audio_peak: AtomicU64,
    sync_packets: AtomicU64,
    control_packets: AtomicU64,
    retransmission_requests: AtomicU64,
    retransmissions: AtomicU64,
    reanchors: AtomicU64,
    feedback_requests: AtomicU64,
    feedback_responses: AtomicU64,
    feedback_last_status: AtomicU64,
    feedback_last_response: Mutex<Option<Instant>>,
    // Zero means omitted; an explicitly reported stream count is stored + 1.
    feedback_stream_count: AtomicU64,
    event_requests: AtomicU64,
    event_responses: AtomicU64,
}
struct CachedPacket {
    sequence: u16,
    sent: Instant,
    bytes: Vec<u8>,
}
type PacketCache = Arc<Mutex<VecDeque<CachedPacket>>>;
struct RecordedFailure {
    stage: &'static str,
    message: String,
}
type Failure = Arc<Mutex<Option<RecordedFailure>>>;
fn record_failure(failure: &Failure, stage: &'static str, error: impl ToString) {
    if let Ok(mut failed) = failure.lock() {
        if failed.is_none() {
            *failed = Some(RecordedFailure {
                stage,
                message: error.to_string(),
            });
        }
    }
}

/// A control handle that may run on a separate task from the audio pacer.
/// It does not keep a dropped sender's TCP session alive. Network errors and
/// cancellation fail the session; the next audio call observes that failure.
#[derive(Clone)]
pub struct VolumeControl {
    connection: Weak<AsyncMutex<Connection>>,
    failure: Weak<Mutex<Option<RecordedFailure>>>,
    stopped: Arc<AtomicBool>,
}
impl VolumeControl {
    pub async fn set_volume(&self, volume: f32) -> Result<()> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err(Error::InvalidArgument(
                "volume must be between 0 and 1".into(),
            ));
        }
        if self.stopped.load(Ordering::Acquire) {
            return Err(Error::Stopped);
        }
        let connection = self.connection.upgrade().ok_or(Error::Stopped)?;
        let failure = self.failure.upgrade().ok_or(Error::Stopped)?;
        let mut guard = ControlCancellation {
            failure: failure.clone(),
            completed: false,
        };
        let result = async {
            let mut connection = connection.lock().await;
            if self.stopped.load(Ordering::Acquire) {
                return Err(Error::Stopped);
            }
            if let Some(error) = failure
                .lock()
                .map_err(|_| protocol("session status lock poisoned"))?
                .as_ref()
            {
                return Err(Error::SessionLost(error.message.clone()));
            }
            set_volume_request(&mut connection, volume).await
        }
        .await;
        guard.completed = true;
        if let Err(error) = &result {
            if !matches!(error, Error::Stopped) {
                record_failure(&failure, "volume", error);
            }
        }
        result
    }
}
struct ControlCancellation {
    failure: Failure,
    completed: bool,
}
impl Drop for ControlCancellation {
    fn drop(&mut self) {
        if !self.completed {
            record_failure(
                &self.failure,
                "volume",
                "volume request was cancelled; reconnect required",
            );
        }
    }
}

pub struct Sender {
    connection: Arc<AsyncMutex<Connection>>,
    transport: Arc<UdpSocket>,
    control: Arc<UdpSocket>,
    data_peer: SocketAddr,
    control_peer: SocketAddr,
    encoder: PacketEncoder,
    pending: VecDeque<i16>,
    next_packet: Option<Instant>,
    last_sync: Option<Instant>,
    anchor: Option<(u32, u64)>,
    latency_frames: u32,
    ptp: bool,
    timing: Option<TimingRuntime>,
    cache: PacketCache,
    failure: Failure,
    counters: Arc<Counters>,
    tasks: Vec<JoinHandle<()>>,
    credentials: Option<PairingCredentials>,
    stopped: Arc<AtomicBool>,
    session_diagnostics: SenderDiagnostics,
}
impl Drop for Sender {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::Release);
        for task in &self.tasks {
            task.abort();
        }
        self.timing.take();
    }
}

impl Sender {
    pub async fn connect(mut device: Device, options: ConnectOptions) -> Result<Self> {
        if options
            .password
            .as_ref()
            .is_some_and(|p| p.len() > 1024 || p.is_empty())
        {
            return Err(Error::InvalidArgument(
                "password must contain 1 to 1024 bytes".into(),
            ));
        }
        if !(0.0..=1.0).contains(&options.initial_volume) || !options.initial_volume.is_finite() {
            return Err(Error::InvalidArgument(
                "volume must be between 0 and 1".into(),
            ));
        }
        if options.latency < Duration::from_millis(250) || options.latency > Duration::from_secs(3)
        {
            return Err(Error::InvalidArgument(
                "receiver buffer must be 250 to 3000 ms".into(),
            ));
        }
        if device.requires_password && options.password.is_none() && options.credentials.is_none() {
            return Err(Error::AuthenticationRequired {
                kind: AuthenticationKind::Password,
            });
        }
        if device.requires_pin && options.credentials.is_none() {
            return Err(Error::AuthenticationRequired {
                kind: AuthenticationKind::Pin,
            });
        }
        let remote = pairing::address(&device)?;
        let mut connection = Connection::connect(remote, options.password.clone()).await?;
        let info = connection.exchange("GET", "/info", "", &[], &[]).await?;
        let mut realtime_formats = None;
        if info.status == 200 && !info.body.is_empty() {
            let value = decode_plist(&info.body)?;
            realtime_formats = merge_receiver_info(&mut device, &value);
        } else if info.status != 404 {
            connection.check(&info, "receiver info")?;
        }
        let shared = if let Some(credentials) = &options.credentials {
            pairing::verify(&mut connection, &device, credentials).await?
        } else {
            let attempt = pairing::transient(&mut connection, options.password.as_deref()).await;
            // Some receivers protect RTSP with Digest but keep the fixed transient
            // SRP secret. A rejected SRP attempt leaves its socket unusable: retry
            // that one alternative on a fresh connection, retaining the password
            // for a later Digest challenge. Never retry malformed/wire failures.
            let attempt = if options.password.is_some()
                && matches!(
                    attempt,
                    Err(Error::AuthenticationRejected {
                        kind: AuthenticationKind::Password
                    })
                ) {
                connection = Connection::connect(remote, options.password.clone()).await?;
                let info = connection.exchange("GET", "/info", "", &[], &[]).await?;
                if info.status != 404 {
                    connection.check(&info, "receiver info")?;
                }
                if info.status == 200 && !info.body.is_empty() {
                    let value = decode_plist(&info.body)?;
                    realtime_formats =
                        merge_receiver_info(&mut device, &value).or(realtime_formats);
                }
                pairing::transient(&mut connection, None).await
            } else {
                attempt
            };
            attempt.map_err(|e| match e {
                Error::AuthenticationRejected { .. } if options.password.is_none() => {
                    Error::AuthenticationRequired {
                        kind: AuthenticationKind::Password,
                    }
                }
                other => other,
            })?
        };
        connection.secure(&shared);
        let local = connection.wire.local_addr()?.ip();
        let identity = u64::from_str_radix(&connection.identity, 16)
            .map_err(|_| protocol("invalid generated clock identity"))?;
        let ptp = match options.timing {
            TimingMode::Auto => device.features & (1u64 << 41) != 0,
            TimingMode::Ptp => true,
            TimingMode::Ntp => false,
        };
        let standalone_homepod = receiver_owns_clock(&device);
        let timing = if ptp {
            TimingRuntime::ptp(local, remote.ip(), identity, standalone_homepod).await?
        } else {
            TimingRuntime::ntp(local, remote.ip(), identity).await?
        };
        // A followed HomePod can remain quiet until SETUP / SETPEERS. Bootstrap
        // the session with the currently known clock, then require the actual
        // receiver clock to settle before returning a sender for its first PCM.
        // The later realtime announcement names the learned clock authority.
        let transport = Arc::new(UdpSocket::bind(SocketAddr::new(local, 0)).await?);
        let control = Arc::new(UdpSocket::bind(SocketAddr::new(local, 0)).await?);
        let session = build_session_setup(
            identity,
            &connection.session_uuid,
            local,
            ptp,
            timing.port,
            timing.clock.master_id.load(Ordering::Acquire),
        );
        let session_response =
            exchange_plist(&mut connection, "SETUP", session, "session SETUP").await?;
        let event_port = optional_port(&session_response, "eventPort")?;
        // Connect the receiver event socket before RECORD. Do not spawn detached
        // workers until the entire setup succeeds, so failed setup has no leaks.
        let event_wire = if let Some(port) = event_port {
            let stream = timeout(
                Duration::from_secs(3),
                TcpStream::connect(SocketAddr::new(remote.ip(), port)),
            )
            .await
            .map_err(|_| Error::Timeout("event channel"))??;
            stream.set_nodelay(true)?;
            Some(Wire::encrypted(
                stream,
                crypto::derive(&shared, b"Events-Salt", b"Events-Write-Encryption-Key"),
                crypto::derive(&shared, b"Events-Salt", b"Events-Read-Encryption-Key"),
            ))
        } else {
            None
        };
        let uri = connection.uri.clone();
        let record = connection.exchange("RECORD", &uri, "", &[], &[]).await?;
        connection.check(&record, "RECORD")?;
        let key: [u8; 32] = shared[..32]
            .try_into()
            .map_err(|_| protocol("short pairing shared secret"))?;
        let desired_frames = (options.latency.as_secs_f64() * SAMPLE_RATE as f64).round() as u32;
        let stream_id = u32::from_le_bytes(crypto::random::<4>());
        let mut stream = Dictionary::new();
        for (name, value) in [
            ("type", 96),
            ("audioFormat", 0x40000),
            ("ct", 2),
            ("spf", FRAMES_PER_PACKET as u64),
            ("sr", SAMPLE_RATE as u64),
            ("controlPort", control.local_addr()?.port() as u64),
            ("dataPort", transport.local_addr()?.port() as u64),
            ("latencyMin", 11025),
            // The receive window is distinct from the desired render lead.
            // Apple's PTP announcement uses an 88200-frame window by default;
            // preserve room for it while scheduling playback at the chosen lead.
            ("latencyMax", desired_frames.max(SAMPLE_RATE * 2) as u64),
            ("streamConnectionID", stream_id as u64),
        ] {
            stream.insert(name.into(), number(value));
        }
        stream.insert("shk".into(), Value::Data(key.to_vec()));
        stream.insert("audioMode".into(), Value::String("default".into()));
        stream.insert("isMedia".into(), Value::Boolean(true));
        stream.insert("supportsDynamicStreamID".into(), Value::Boolean(false));
        let mut body = Dictionary::new();
        body.insert(
            "streams".into(),
            Value::Array(vec![Value::Dictionary(stream)]),
        );
        let setup = exchange_plist(
            &mut connection,
            "SETUP",
            Value::Dictionary(body),
            "audio SETUP",
        )
        .await?;
        let stream = setup
            .as_dictionary()
            .and_then(|d| d.get("streams"))
            .and_then(Value::as_array)
            .and_then(|a| a.first())
            .ok_or_else(|| protocol("receiver returned no audio stream"))?;
        let data_port = optional_port(stream, "dataPort")?
            .ok_or_else(|| protocol("receiver dataPort missing"))?;
        let control_port = optional_port(stream, "controlPort")?
            .ok_or_else(|| protocol("receiver controlPort missing"))?;
        let latency = ReceiverLatency::parse(stream, desired_frames)?;
        let latency_frames = latency.effective_frames;
        if ptp {
            let peers = Value::Array(vec![
                Value::String(remote.ip().to_string()),
                Value::String(local.to_string()),
            ]);
            exchange_plist(&mut connection, "SETPEERS", peers, "SETPEERS").await?;
        }
        set_volume_request(&mut connection, options.initial_volume).await?;
        let connection = Arc::new(AsyncMutex::new(connection));
        let counters = Arc::new(Counters::default());
        let failure = Arc::new(Mutex::new(None));
        let cache = Arc::new(Mutex::new(VecDeque::new()));
        let control_peer = SocketAddr::new(remote.ip(), control_port);
        let mut tasks = vec![
            spawn_feedback(connection.clone(), failure.clone(), counters.clone()),
            spawn_retransmit(
                control.clone(),
                remote.ip(),
                cache.clone(),
                counters.clone(),
                failure.clone(),
            ),
        ];
        if let Some(wire) = event_wire {
            tasks.push(spawn_events(wire, failure.clone(), counters.clone()));
        }
        let session_diagnostics = SenderDiagnostics {
            receiver: receiver_diagnostics(&device, realtime_formats),
            timing: timing.diagnostics(),
            stream: StreamDiagnostics {
                protocol: "airplay2_realtime".into(),
                stream_type: 96,
                codec: "alac".into(),
                audio_format_hex: "0x00040000".into(),
                sample_rate: SAMPLE_RATE,
                sample_bits: 16,
                channels: CHANNELS as u8,
                frames_per_packet: FRAMES_PER_PACKET as u16,
                requested_latency_ms: options.latency.as_millis() as u64,
                receiver_latency_min_frames: latency.minimum_frames,
                receiver_latency_max_frames: latency.maximum_frames,
                effective_latency_frames: latency_frames,
            },
            ..SenderDiagnostics::default()
        };
        let timestamp = u32::from_le_bytes(crypto::random::<4>()).wrapping_add(latency_frames);
        let sender = Self {
            connection,
            transport,
            control,
            data_peer: SocketAddr::new(remote.ip(), data_port),
            control_peer,
            encoder: PacketEncoder::new(key, timestamp, if ptp { 0 } else { stream_id }),
            pending: VecDeque::new(),
            next_packet: None,
            last_sync: None,
            anchor: None,
            latency_frames,
            ptp,
            timing: Some(timing),
            cache,
            failure,
            counters,
            tasks,
            credentials: options.credentials.clone(),
            stopped: Arc::new(AtomicBool::new(false)),
            session_diagnostics,
        };
        if ptp {
            sender
                .timing
                .as_ref()
                .ok_or(Error::Stopped)?
                .wait_for_ptp_exchange()
                .await?;
        }
        Ok(sender)
    }

    /// Feed interleaved 44.1kHz/16-bit stereo PCM. The caller must supply silence
    /// during source inactivity. A call is bounded to one second of PCM.
    pub async fn send_pcm(&mut self, samples: &[i16]) -> Result<()> {
        self.check_health()?;
        if !samples.len().is_multiple_of(CHANNELS)
            || samples.len() > SAMPLE_RATE as usize * CHANNELS
        {
            return Err(Error::InvalidArgument(
                "PCM must be complete stereo frames, at most one second per call".into(),
            ));
        }
        self.pending.extend(samples);
        while self.pending.len() >= FRAMES_PER_PACKET * CHANNELS {
            self.check_health()?;
            let mut due = self.next_packet.unwrap_or_else(Instant::now);
            let now = Instant::now();
            if now > due + Duration::from_millis(200) {
                // A suspended process must not burst stale audio or let the media
                // clock stay seconds behind. Discard stale caller backlog, keep
                // packet/key counters, and advance the RTP timeline by elapsed time.
                let missed = ((now - due).as_secs_f64() * SAMPLE_RATE as f64) as u32;
                self.encoder.timestamp = self.encoder.timestamp.wrapping_add(missed);
                let keep = FRAMES_PER_PACKET * CHANNELS;
                while self.pending.len() > keep {
                    self.pending.pop_front();
                }
                self.counters.reanchors.fetch_add(1, Ordering::Relaxed);
                self.last_sync = None;
                self.anchor = None;
                due = now;
            }
            tokio::time::sleep_until(due).await;
            let first_sync = self.last_sync.is_none();
            if first_sync
                || self
                    .last_sync
                    .is_some_and(|last| last.elapsed() >= Duration::from_millis(250))
            {
                let timing = self.timing.as_ref().ok_or(Error::Stopped)?;
                let (timestamp, local_ns) = *self
                    .anchor
                    .get_or_insert_with(|| (self.encoder.timestamp, timing.clock.local_ns()));
                let sync = timing::sync_packet(
                    timestamp,
                    local_ns,
                    self.latency_frames,
                    &timing.clock,
                    self.ptp,
                    first_sync,
                );
                self.control
                    .send_to(&sync, self.control_peer)
                    .await
                    .map_err(|error| {
                        record_failure(&self.failure, "audio_sync", &error);
                        Error::Io(error)
                    })?;
                self.counters.sync_packets.fetch_add(1, Ordering::Relaxed);
                self.last_sync = Some(Instant::now());
            }
            let mut chunk = [0i16; FRAMES_PER_PACKET * CHANNELS];
            let mut peak = 0;
            for sample in &mut chunk {
                *sample = self.pending.pop_front().expect("complete packet checked");
                peak = peak.max(sample.unsigned_abs());
            }
            let (sequence, packet) = self.encoder.packet(&chunk)?;
            self.transport
                .send_to(&packet, self.data_peer)
                .await
                .map_err(|error| {
                    record_failure(&self.failure, "audio_send", &error);
                    Error::Io(error)
                })?;
            self.counters.packets.fetch_add(1, Ordering::Relaxed);
            self.counters
                .last_audio_peak
                .store(peak as u64, Ordering::Relaxed);
            if peak > 0 {
                self.counters
                    .non_silent_packets
                    .fetch_add(1, Ordering::Relaxed);
            }
            self.counters
                .bytes
                .fetch_add(packet.len() as u64, Ordering::Relaxed);
            self.counters
                .frames
                .fetch_add(FRAMES_PER_PACKET as u64, Ordering::Relaxed);
            let mut cache = self
                .cache
                .lock()
                .map_err(|_| protocol("packet cache lock poisoned"))?;
            cache.push_back(CachedPacket {
                sequence,
                sent: Instant::now(),
                bytes: packet,
            });
            while cache.len() > 512 {
                cache.pop_front();
            }
            self.next_packet = Some(
                due + Duration::from_nanos(
                    FRAMES_PER_PACKET as u64 * 1_000_000_000 / SAMPLE_RATE as u64,
                ),
            );
        }
        Ok(())
    }
    pub async fn set_volume(&self, volume: f32) -> Result<()> {
        self.volume_control().set_volume(volume).await
    }
    pub fn volume_control(&self) -> VolumeControl {
        VolumeControl {
            connection: Arc::downgrade(&self.connection),
            failure: Arc::downgrade(&self.failure),
            stopped: self.stopped.clone(),
        }
    }
    pub async fn stop(&mut self) -> Result<()> {
        if self.stopped.swap(true, Ordering::AcqRel) {
            return Ok(());
        }
        for task in self.tasks.drain(..) {
            task.abort();
        }
        let result = timeout(Duration::from_secs(2), async {
            let mut connection = self.connection.lock().await;
            let uri = connection.uri.clone();
            let response = connection.exchange("TEARDOWN", &uri, "", &[], &[]).await?;
            connection.check(&response, "TEARDOWN")
        })
        .await
        .map_err(|_| Error::Timeout("TEARDOWN"));
        self.timing.take();
        self.pending.clear();
        result?
    }
    pub fn credentials(&self) -> Option<&PairingCredentials> {
        self.credentials.as_ref()
    }
    /// Export transport observations without credentials or device identity.
    /// No field in this structure asserts that a receiver rendered sound.
    pub fn diagnostics(&self) -> SenderDiagnostics {
        let mut diagnostics = self.session_diagnostics.clone();
        if let Some(timing) = &self.timing {
            diagnostics.timing = timing.diagnostics();
        }
        let counters = &self.counters;
        let packets = counters.packets.load(Ordering::Relaxed);
        let peak = counters.last_audio_peak.load(Ordering::Relaxed);
        let status = counters.feedback_last_status.load(Ordering::Relaxed);
        let stream_count = counters.feedback_stream_count.load(Ordering::Relaxed);
        diagnostics.transport = TransportDiagnostics {
            audio_packets_sent: packets,
            non_silent_audio_packets_sent: counters.non_silent_packets.load(Ordering::Relaxed),
            last_audio_peak_dbfs: (packets > 0).then(|| {
                if peak == 0 {
                    -96.0
                } else {
                    (20.0 * (peak as f32 / 32768.0).log10()).max(-96.0)
                }
            }),
            sync_packets_sent: counters.sync_packets.load(Ordering::Relaxed),
            control_packets_received: counters.control_packets.load(Ordering::Relaxed),
            retransmission_requests: counters.retransmission_requests.load(Ordering::Relaxed),
            retransmissions_sent: counters.retransmissions.load(Ordering::Relaxed),
            feedback_requests: counters.feedback_requests.load(Ordering::Relaxed),
            feedback_responses: counters.feedback_responses.load(Ordering::Relaxed),
            feedback_last_status: (status != 0).then_some(status as u16),
            feedback_last_response_age_ms: counters
                .feedback_last_response
                .lock()
                .ok()
                .and_then(|last| last.map(|instant| instant.elapsed().as_millis() as u64)),
            feedback_reported_streams: stream_count.checked_sub(1),
            event_requests: counters.event_requests.load(Ordering::Relaxed),
            event_responses: counters.event_responses.load(Ordering::Relaxed),
        };
        diagnostics.last_failure_stage = self
            .failure
            .lock()
            .ok()
            .and_then(|failure| failure.as_ref().map(|f| f.stage.to_owned()));
        diagnostics
    }
    pub fn metrics(&self) -> SenderMetrics {
        SenderMetrics {
            packets_sent: self.counters.packets.load(Ordering::Relaxed),
            bytes_sent: self.counters.bytes.load(Ordering::Relaxed),
            frames_sent: self.counters.frames.load(Ordering::Relaxed),
            retransmission_requests: self
                .counters
                .retransmission_requests
                .load(Ordering::Relaxed),
            retransmissions_sent: self.counters.retransmissions.load(Ordering::Relaxed),
            timing_exchanges: self
                .timing
                .as_ref()
                .map(|t| t.clock.exchanges.load(Ordering::Relaxed))
                .unwrap_or(0),
            reanchors: self.counters.reanchors.load(Ordering::Relaxed),
            buffered_frames: self.pending.len() / CHANNELS,
            negotiated_latency_ms: self.latency_frames as u64 * 1000 / SAMPLE_RATE as u64,
            timing_mode: if self.ptp { "ptp" } else { "ntp" }.into(),
            last_error: self
                .failure
                .lock()
                .ok()
                .and_then(|f| f.as_ref().map(|f| f.message.clone())),
            diagnostics: self.diagnostics(),
        }
    }
    fn check_health(&self) -> Result<()> {
        if self.stopped.load(Ordering::Acquire) {
            return Err(Error::Stopped);
        }
        if let Some(failure) = self
            .failure
            .lock()
            .map_err(|_| protocol("session status lock poisoned"))?
            .as_ref()
        {
            return Err(Error::SessionLost(failure.message.clone()));
        }
        let result = self
            .timing
            .as_ref()
            .ok_or(Error::Stopped)?
            .check_health(self.ptp);
        if let Err(error) = &result {
            record_failure(&self.failure, "timing", error);
        }
        result
    }
}

fn number(value: u64) -> Value {
    Value::Integer(value.into())
}

struct ReceiverLatency {
    minimum_frames: Option<u32>,
    maximum_frames: Option<u32>,
    effective_frames: u32,
}
impl ReceiverLatency {
    fn parse(stream: &Value, requested_frames: u32) -> Result<Self> {
        let dictionary = stream
            .as_dictionary()
            .ok_or_else(|| protocol("invalid audio stream"))?;
        let field = |name| -> Result<Option<u32>> {
            dictionary
                .get(name)
                .map(|value| {
                    let frames = value
                        .as_unsigned_integer()
                        .ok_or_else(|| protocol("invalid receiver latency value"))?;
                    if frames > SAMPLE_RATE as u64 * 10 {
                        return Err(protocol("invalid receiver latency window"));
                    }
                    Ok(frames as u32)
                })
                .transpose()
        };
        let minimum_frames = field("latencyMin")?;
        let maximum_frames = field("latencyMax")?;
        // Some receivers omit either bound, or use zero for an unspecified
        // bound. Never fabricate a maximum below a reported minimum.
        let minimum = minimum_frames.unwrap_or(0);
        let maximum = maximum_frames
            .filter(|value| *value != 0)
            .unwrap_or_else(|| requested_frames.max(minimum));
        if minimum > maximum {
            return Err(protocol("invalid receiver latency window"));
        }
        Ok(Self {
            minimum_frames,
            maximum_frames,
            effective_frames: requested_frames.clamp(minimum, maximum),
        })
    }
}

fn receiver_owns_clock(device: &Device) -> bool {
    let version = |key: &str| {
        device
            .txt
            .get(key)
            .and_then(|v| v.split('.').next())
            .and_then(|v| v.parse::<u32>().ok())
    };
    // Explicit OS version takes priority over the firmware fallback. The ov/vs
    // aliases are advertised on RAOP services; osvers/srcvers on AirPlay ones.
    let newer_os = version("osvers")
        .or_else(|| version("ov"))
        .map(|v| v >= 27)
        .unwrap_or_else(|| {
            version("srcvers")
                .or_else(|| version("vs"))
                .is_some_and(|v| v >= 980)
        });
    device
        .model
        .as_deref()
        .is_some_and(|m| m.starts_with("AudioAccessory"))
        && newer_os
        && device.txt.get("igl").is_some_and(|v| v == "1")
        && device.txt.get("tsid").is_none_or(|v| v.is_empty())
        && device.txt.get("pgid").is_none_or(|v| v.is_empty())
}

fn build_session_setup(
    identity: u64,
    session_uuid: &str,
    local: IpAddr,
    ptp: bool,
    timing_port: u16,
    clock_identity: u64,
) -> Value {
    // deviceID follows the 64-bit DACP identity. macAddress is a separate,
    // six-octet field; sending the eight-octet device ID there is malformed.
    let octets = identity
        .to_be_bytes()
        .iter()
        .map(|byte| format!("{byte:02X}"))
        .collect::<Vec<_>>();
    let device_id = octets.join(":");
    let mac_address = octets[..6].join(":");
    let mut session = Dictionary::new();
    session.insert("deviceID".into(), Value::String(device_id));
    session.insert("macAddress".into(), Value::String(mac_address));
    session.insert(
        "sessionUUID".into(),
        Value::String(session_uuid.to_uppercase()),
    );
    session.insert("name".into(), Value::String("Airloom".into()));
    session.insert("model".into(), Value::String("Airloom1,1".into()));
    session.insert("sourceVersion".into(), Value::String("550.10".into()));
    session.insert("isMultiSelectAirPlay".into(), Value::Boolean(true));
    session.insert("groupContainsGroupLeader".into(), Value::Boolean(false));
    session.insert("senderSupportsRelay".into(), Value::Boolean(false));
    session.insert(
        "timingProtocol".into(),
        Value::String(if ptp { "PTP" } else { "NTP" }.into()),
    );
    if ptp {
        let mut peer = Dictionary::new();
        peer.insert("ID".into(), Value::String(timing_peer_uuid()));
        peer.insert("DeviceType".into(), number(0));
        // PTP's identity is a 64-bit bit pattern, but the plist field is signed.
        // A positive u64 above i64::MAX would be serialized as a 128-bit integer.
        peer.insert(
            "ClockID".into(),
            Value::Integer((clock_identity as i64).into()),
        );
        peer.insert(
            "SupportsClockPortMatchingOverride".into(),
            Value::Boolean(false),
        );
        peer.insert(
            "Addresses".into(),
            Value::Array(vec![Value::String(local.to_string())]),
        );
        session.insert("timingPeerInfo".into(), Value::Dictionary(peer.clone()));
        session.insert(
            "timingPeerList".into(),
            Value::Array(vec![Value::Dictionary(peer)]),
        );
    } else {
        session.insert("timingPort".into(), number(timing_port as u64));
    }
    Value::Dictionary(session)
}

fn timing_peer_uuid() -> String {
    let mut bytes = crypto::random::<16>();
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let value = hex::encode_upper(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &value[..8],
        &value[8..12],
        &value[12..16],
        &value[16..20],
        &value[20..]
    )
}
fn decode_plist(bytes: &[u8]) -> Result<Value> {
    Value::from_reader(Cursor::new(bytes)).map_err(|_| protocol("invalid receiver plist"))
}
fn optional_port(value: &Value, key: &str) -> Result<Option<u16>> {
    value
        .as_dictionary()
        .and_then(|d| d.get(key))
        .map(|value| {
            value
                .as_unsigned_integer()
                .filter(|p| *p > 0 && *p <= 65535)
                .map(|p| p as u16)
                .ok_or_else(|| protocol(format!("invalid {key}")))
        })
        .transpose()
}
async fn exchange_plist(
    connection: &mut Connection,
    method: &str,
    value: Value,
    stage: &'static str,
) -> Result<Value> {
    let mut body = Vec::new();
    value
        .to_writer_binary(&mut body)
        .map_err(|_| protocol("could not serialize plist"))?;
    let uri = connection.uri.clone();
    let response = connection
        .exchange(method, &uri, "application/x-apple-binary-plist", &body, &[])
        .await?;
    connection.check(&response, stage)?;
    if response.body.is_empty() {
        Ok(Value::Dictionary(Dictionary::new()))
    } else {
        decode_plist(&response.body)
    }
}
async fn set_volume_request(connection: &mut Connection, volume: f32) -> Result<()> {
    if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
        return Err(Error::InvalidArgument(
            "volume must be between 0 and 1".into(),
        ));
    }
    let db = if volume == 0.0 {
        -144.0
    } else {
        -30.0 * (1.0 - volume)
    };
    let uri = connection.uri.clone();
    let body = format!("volume: {db:.6}\r\n");
    let response = connection
        .exchange(
            "SET_PARAMETER",
            &uri,
            "text/parameters",
            body.as_bytes(),
            &[],
        )
        .await?;
    connection.check(&response, "volume")
}
fn spawn_feedback(
    connection: Arc<AsyncMutex<Connection>>,
    failure: Failure,
    counters: Arc<Counters>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticks = interval(Duration::from_secs(2));
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ticks.tick().await;
        loop {
            ticks.tick().await;
            let mut connection = connection.lock().await;
            let result = async {
                counters.feedback_requests.fetch_add(1, Ordering::Relaxed);
                let response = connection
                    .exchange("POST", "/feedback", "", &[], &[])
                    .await?;
                counters.feedback_responses.fetch_add(1, Ordering::Relaxed);
                counters
                    .feedback_last_status
                    .store(response.status as u64, Ordering::Relaxed);
                if let Ok(mut last) = counters.feedback_last_response.lock() {
                    *last = Some(Instant::now());
                }
                let stream_count = decode_plist(&response.body).ok().and_then(|value| {
                    value
                        .as_dictionary()?
                        .get("streams")?
                        .as_array()
                        .map(|streams| streams.len() as u64)
                });
                counters
                    .feedback_stream_count
                    .store(stream_count.map_or(0, |count| count + 1), Ordering::Relaxed);
                connection.check(&response, "feedback")
            }
            .await;
            if let Err(error) = result {
                record_failure(&failure, "feedback", error);
                return;
            }
        }
    })
}
fn spawn_events(mut wire: Wire, failure: Failure, counters: Arc<Counters>) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let result: Result<()> = async {
                let (line, headers, _body) = wire.read_message().await?;
                counters.event_requests.fetch_add(1, Ordering::Relaxed);
                let response = event_response(&line, headers.get("cseq").map(String::as_str))?;
                timeout(Duration::from_secs(1), wire.write(response.as_bytes()))
                    .await
                    .map_err(|_| Error::Timeout("event response"))??;
                counters.event_responses.fetch_add(1, Ordering::Relaxed);
                Ok(())
            }
            .await;
            if let Err(error) = result {
                record_failure(&failure, "event", error);
                return;
            }
        }
    })
}
fn event_response(line: &str, sequence: Option<&str>) -> Result<String> {
    let fields: Vec<_> = line.split_whitespace().collect();
    let version = match fields.as_slice() {
        ["POST", _, version @ ("RTSP/1.0" | "HTTP/1.0" | "HTTP/1.1")] => *version,
        _ => return Err(protocol("unexpected event-channel message")),
    };
    // Reverse events can use HTTP or RTSP. Reply in the incoming protocol,
    // and delimit the empty body so the receiver can process the next event.
    let mut response =
        format!("{version} 200 OK\r\nServer: Airloom\r\nContent-Length: 0\r\nAudio-Latency: 0\r\n");
    if let Some(sequence) = sequence {
        let sequence = sequence
            .parse::<u32>()
            .map_err(|_| protocol("invalid event sequence"))?;
        response.push_str(&format!("CSeq: {sequence}\r\n"));
    }
    response.push_str("\r\n");
    Ok(response)
}
fn spawn_retransmit(
    socket: Arc<UdpSocket>,
    peer: std::net::IpAddr,
    cache: PacketCache,
    counters: Arc<Counters>,
    failure: Failure,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut buffer = [0; 2048];
        loop {
            let (size, address) = match socket.recv_from(&mut buffer).await {
                Ok(value) => value,
                Err(error) => {
                    record_failure(&failure, "control_receive", error);
                    return;
                }
            };
            if address.ip() != peer {
                continue;
            }
            counters.control_packets.fetch_add(1, Ordering::Relaxed);
            if size < 8 || buffer[1] & 0x7f != 0x55 {
                continue;
            }
            let sequence = u16::from_be_bytes([buffer[4], buffer[5]]);
            let count = u16::from_be_bytes([buffer[6], buffer[7]]).min(128);
            counters
                .retransmission_requests
                .fetch_add(1, Ordering::Relaxed);
            let packets = match cache.lock() {
                Ok(cache) => (0..count)
                    .filter_map(|offset| {
                        cache
                            .iter()
                            .find(|packet| {
                                packet.sequence == sequence.wrapping_add(offset)
                                    && packet.sent.elapsed() < Duration::from_secs(3)
                            })
                            .map(|packet| packet.bytes.clone())
                    })
                    .collect::<Vec<_>>(),
                Err(_) => {
                    record_failure(&failure, "retransmission", "packet cache lock poisoned");
                    return;
                }
            };
            for packet in packets {
                let mut response = vec![0x80, 0xd6, buffer[2], buffer[3]];
                response.extend(packet);
                if let Err(error) = socket.send_to(&response, address).await {
                    record_failure(&failure, "retransmission", error);
                    return;
                }
                counters.retransmissions.fetch_add(1, Ordering::Relaxed);
            }
        }
    })
}

#[cfg(test)]
mod session_setup_contract_tests {
    use super::*;

    const ID: u64 = 0xf842_885f_7175_0008;
    const SESSION: &str = "9dff3e7a-ec6d-4fa7-8666-b416d376897d";

    fn setup(ptp: bool) -> Value {
        build_session_setup(ID, SESSION, "192.0.2.25".parse().unwrap(), ptp, 64123, ID)
    }

    #[test]
    fn receiver_latency_accepts_partial_bounds_and_preserves_reported_values() {
        for (minimum, maximum, expected) in [
            (None, None, 33075),
            (Some(44100), None, 44100),
            (None, Some(22050), 22050),
            (Some(11025), Some(88200), 33075),
            (Some(44100), Some(0), 44100),
        ] {
            let mut stream = Dictionary::new();
            for (key, value) in [("latencyMin", minimum), ("latencyMax", maximum)] {
                if let Some(value) = value {
                    stream.insert(key.into(), number(value));
                }
            }
            let latency = ReceiverLatency::parse(&Value::Dictionary(stream), 33075).unwrap();
            assert_eq!(latency.effective_frames, expected);
            assert_eq!(latency.minimum_frames, minimum.map(|n| n as u32));
            assert_eq!(latency.maximum_frames, maximum.map(|n| n as u32));
        }
        for (minimum, maximum) in [(44100, 22050), (441001, 88200)] {
            let stream = Value::Dictionary(
                [
                    ("latencyMin".to_owned(), number(minimum)),
                    ("latencyMax".to_owned(), number(maximum)),
                ]
                .into_iter()
                .collect(),
            );
            assert!(ReceiverLatency::parse(&stream, 33075).is_err());
        }
    }

    #[test]
    fn missing_discovery_version_uses_info_without_overriding_older_os_or_group_shape() {
        let mut device = Device::manual("192.0.2.25".parse().unwrap(), 7000);
        device.txt.insert("igl".into(), "1".into());
        let info = Value::Dictionary(
            [
                (
                    "model".to_owned(),
                    Value::String("AudioAccessory5,1".into()),
                ),
                ("osVersion".to_owned(), Value::String("27.0".into())),
            ]
            .into_iter()
            .collect(),
        );
        assert!(!receiver_owns_clock(&device));
        merge_receiver_info(&mut device, &info);
        assert!(receiver_owns_clock(&device));
        device.txt.remove("osvers");
        device.txt.insert("ov".into(), "26.4".into());
        merge_receiver_info(&mut device, &info);
        assert!(!receiver_owns_clock(&device));
        device.txt.insert("ov".into(), "27.0".into());
        device
            .txt
            .insert("tsid".into(), "private-pair-identifier".into());
        assert!(!receiver_owns_clock(&device));
    }

    #[test]
    fn event_ack_validates_protocol_and_does_not_accept_header_injection() {
        for protocol in ["HTTP/1.0", "HTTP/1.1", "RTSP/1.0"] {
            let reply = event_response(&format!("POST /command {protocol}"), Some("78")).unwrap();
            assert!(reply.starts_with(&format!("{protocol} 200 OK\r\n")));
            assert!(reply.ends_with("Content-Length: 0\r\nAudio-Latency: 0\r\nCSeq: 78\r\n\r\n"));
        }
        assert!(event_response("POST /command HTTP/2.0", Some("78")).is_err());
        assert!(event_response("POST /command HTTP/1.1 extra", Some("78")).is_err());
        assert!(event_response("POST /command HTTP/1.1", Some("78\r\nInjected: yes")).is_err());
    }

    #[test]
    fn session_setup_uses_selected_clock_without_changing_device_identity() {
        let clock_id = 0xe001_2233_4455_6677u64;
        let value = build_session_setup(
            ID,
            SESSION,
            "192.0.2.25".parse().unwrap(),
            true,
            319,
            clock_id,
        );
        let session = value.as_dictionary().unwrap();
        assert_eq!(
            session["deviceID"].as_string(),
            Some("F8:42:88:5F:71:75:00:08")
        );
        assert_eq!(
            session["timingPeerInfo"].as_dictionary().unwrap()["ClockID"]
                .as_signed_integer()
                .unwrap() as u64,
            clock_id
        );
    }

    #[test]
    fn receiver_clock_rule_uses_version_aliases_without_overriding_an_older_os() {
        let mut device = Device::manual("192.0.2.25".parse().unwrap(), 7000);
        device.model = Some("AudioAccessory5,1".into());
        device.txt.insert("igl".into(), "1".into());
        assert!(!receiver_owns_clock(&device));
        device.txt.insert("srcvers".into(), "980.4".into());
        assert!(receiver_owns_clock(&device));
        device.txt.insert("ov".into(), "26.4".into());
        assert!(!receiver_owns_clock(&device));
        device.txt.insert("osvers".into(), "27.0".into());
        assert!(receiver_owns_clock(&device));
        device.txt.insert("tsid".into(), "stereo-partner".into());
        assert!(!receiver_owns_clock(&device));
        device.txt.remove("tsid");
        device.txt.insert("pgid".into(), "apple-tv-parent".into());
        assert!(!receiver_owns_clock(&device));
    }

    // These wire constraints come from independent senders, not our mock:
    // OwnTone src/outputs/airplay.c payload_make_setup_session_ptp;
    // Music Assistant src/ap2_client.c ap2_make_timing_peer / native SETUP.
    #[test]
    fn session_setup_contract_distinguishes_device_id_from_mac_address() {
        for ptp in [false, true] {
            let value = setup(ptp);
            let session = value.as_dictionary().unwrap();
            assert_eq!(
                session["deviceID"].as_string(),
                Some("F8:42:88:5F:71:75:00:08")
            );
            assert_eq!(session["macAddress"].as_string(), Some("F8:42:88:5F:71:75"));
        }
    }

    #[test]
    fn session_setup_contract_ptp_has_complete_peer_and_no_ntp_port() {
        let value = setup(true);
        let session = value.as_dictionary().unwrap();
        assert_eq!(session["timingProtocol"].as_string(), Some("PTP"));
        assert!(
            !session.contains_key("timingPort"),
            "PTP does not advertise an NTP timingPort"
        );
        let info = session["timingPeerInfo"].as_dictionary().unwrap();
        let id = info["ID"].as_string().unwrap();
        assert_eq!(id.len(), 36);
        assert_eq!(
            id.split('-').map(str::len).collect::<Vec<_>>(),
            [8, 4, 4, 4, 12]
        );
        assert!(id
            .bytes()
            .all(|byte| byte == b'-' || byte.is_ascii_hexdigit()));
        assert_eq!(info["DeviceType"].as_signed_integer(), Some(0));
        assert_eq!(
            info["SupportsClockPortMatchingOverride"].as_boolean(),
            Some(false)
        );
        assert_eq!(
            info["Addresses"],
            Value::Array(vec![Value::String("192.0.2.25".into())])
        );
        assert_eq!(info.len(), 5);
        let peers = session["timingPeerList"].as_array().unwrap();
        assert_eq!(peers.len(), 1);
        assert_eq!(
            peers[0], session["timingPeerInfo"],
            "both peer fields describe the same clock"
        );
    }

    #[test]
    fn session_setup_contract_high_bit_clock_id_uses_signed_eight_byte_binary() {
        let value = setup(true);
        let mut binary = Vec::new();
        value.to_writer_binary(&mut binary).unwrap();
        // The complete serialized request must contain an eight-byte integer
        // object (0x13). The old u64 conversion emitted 0x14 + 16 bytes instead.
        let bytes = ID.to_be_bytes();
        let mut objects = 0;
        for (position, window) in binary.windows(bytes.len()).enumerate() {
            if window == bytes {
                assert!(position > 0);
                assert_eq!(
                    binary[position - 1],
                    0x13,
                    "ClockID must be encoded as signed 64-bit, never 128-bit"
                );
                objects += 1;
            }
        }
        assert!(
            objects > 0,
            "ClockID bit pattern must survive serialization"
        );
        let decoded = Value::from_reader(Cursor::new(binary)).unwrap();
        let session = decoded.as_dictionary().unwrap();
        let clock = &session["timingPeerInfo"].as_dictionary().unwrap()["ClockID"];
        assert_eq!(clock.as_signed_integer(), Some(-557_733_460_333_756_408));
        assert_eq!(clock.as_signed_integer().unwrap() as u64, ID);
        assert_eq!(
            session["timingPeerList"].as_array().unwrap()[0],
            session["timingPeerInfo"]
        );
    }

    #[test]
    fn session_setup_contract_ntp_has_only_its_timing_port() {
        let value = setup(false);
        let session = value.as_dictionary().unwrap();
        assert_eq!(session["timingProtocol"].as_string(), Some("NTP"));
        assert_eq!(session["timingPort"].as_unsigned_integer(), Some(64123));
        assert!(!session.contains_key("timingPeerInfo"));
        assert!(!session.contains_key("timingPeerList"));
    }
}
