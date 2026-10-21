use crate::{
    audio::PacketEncoder,
    crypto,
    discovery::feature_bits,
    pairing, protocol,
    rtsp::{Connection, Wire},
    timing::{self, TimingRuntime},
    AuthenticationKind, Device, Error, PairingCredentials, Result, CHANNELS, FRAMES_PER_PACKET,
    SAMPLE_RATE,
};
use plist::{Dictionary, Value};
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    io::Cursor,
    net::SocketAddr,
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
}
#[derive(Default)]
struct Counters {
    packets: AtomicU64,
    bytes: AtomicU64,
    frames: AtomicU64,
    retransmission_requests: AtomicU64,
    retransmissions: AtomicU64,
    reanchors: AtomicU64,
}
struct CachedPacket {
    sequence: u16,
    sent: Instant,
    bytes: Vec<u8>,
}
type PacketCache = Arc<Mutex<VecDeque<CachedPacket>>>;
type Failure = Arc<Mutex<Option<String>>>;
fn record_failure(failure: &Failure, error: impl ToString) {
    if let Ok(mut failed) = failure.lock() {
        if failed.is_none() {
            *failed = Some(error.to_string());
        }
    }
}

/// A control handle that may run on a separate task from the audio pacer.
/// It does not keep a dropped sender's TCP session alive. Network errors and
/// cancellation fail the session; the next audio call observes that failure.
#[derive(Clone)]
pub struct VolumeControl {
    connection: Weak<AsyncMutex<Connection>>,
    failure: Weak<Mutex<Option<String>>>,
    stopped: Arc<AtomicBool>,
}
impl VolumeControl {
    pub async fn set_volume(&self, volume: f32) -> Result<()> {
        if !volume.is_finite() || !(0.0..=1.0).contains(&volume) {
            return Err(Error::InvalidArgument("volume must be between 0 and 1".into()));
        }
        if self.stopped.load(Ordering::Acquire) { return Err(Error::Stopped); }
        let connection = self.connection.upgrade().ok_or(Error::Stopped)?;
        let failure = self.failure.upgrade().ok_or(Error::Stopped)?;
        let mut guard = ControlCancellation { failure: failure.clone(), completed: false };
        let result = async {
            let mut connection = connection.lock().await;
            if self.stopped.load(Ordering::Acquire) { return Err(Error::Stopped); }
            if let Some(error) = failure.lock().map_err(|_| protocol("session status lock poisoned"))?.as_ref() {
                return Err(Error::SessionLost(error.clone()));
            }
            set_volume_request(&mut connection, volume).await
        }.await;
        guard.completed = true;
        if let Err(error) = &result {
            if !matches!(error, Error::Stopped) { record_failure(&failure, error); }
        }
        result
    }
}
struct ControlCancellation { failure: Failure, completed: bool }
impl Drop for ControlCancellation {
    fn drop(&mut self) {
        if !self.completed { record_failure(&self.failure, "volume request was cancelled; reconnect required"); }
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
    latency_frames: u32,
    ptp: bool,
    timing: Option<TimingRuntime>,
    cache: PacketCache,
    failure: Failure,
    counters: Arc<Counters>,
    tasks: Vec<JoinHandle<()>>,
    credentials: Option<PairingCredentials>,
    stopped: Arc<AtomicBool>,
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
        if info.status == 200 && !info.body.is_empty() {
            let value = decode_plist(&info.body)?;
            if let Some(model) = value
                .as_dictionary()
                .and_then(|d| d.get("model"))
                .and_then(Value::as_string)
            {
                device.model = Some(model.to_owned());
            }
            if let Some(feature) = value.as_dictionary().and_then(|d| d.get("features")) {
                device.features = feature
                    .as_unsigned_integer()
                    .or_else(|| feature.as_string().and_then(feature_bits))
                    .unwrap_or(device.features);
            }
        } else if info.status != 404 {
            connection.check(&info, "receiver info")?;
        }
        let shared = if let Some(credentials) = &options.credentials {
            pairing::verify(&mut connection, &device, credentials).await?
        } else {
            pairing::transient(&mut connection, options.password.as_deref())
                .await
                .map_err(|e| match e {
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
        let standalone_homepod = device
            .model
            .as_deref()
            .is_some_and(|m| m.starts_with("AudioAccessory"))
            && !device.txt.get("tsid").is_some_and(|v| !v.is_empty())
            && !device.txt.get("pgid").is_some_and(|v| !v.is_empty());
        let timing = if ptp {
            TimingRuntime::ptp(local, remote.ip(), identity, standalone_homepod).await?
        } else {
            TimingRuntime::ntp(local, remote.ip(), identity).await?
        };
        let transport = Arc::new(UdpSocket::bind(SocketAddr::new(local, 0)).await?);
        let control = Arc::new(UdpSocket::bind(SocketAddr::new(local, 0)).await?);
        let device_id = connection.identity
            .as_bytes()
            .chunks(2)
            .map(|part| std::str::from_utf8(part).unwrap())
            .collect::<Vec<_>>()
            .join(":");
        let mut session = Dictionary::new();
        session.insert("deviceID".into(), Value::String(device_id.clone()));
        session.insert("macAddress".into(), Value::String(device_id));
        session.insert(
            "sessionUUID".into(),
            Value::String(connection.session_uuid.to_uppercase()),
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
        session.insert("timingPort".into(), number(timing.port as u64));
        if ptp {
            let mut peer = Dictionary::new();
            peer.insert("ClockID".into(), number(identity));
            peer.insert(
                "Addresses".into(),
                Value::Array(vec![Value::String(local.to_string())]),
            );
            session.insert("timingPeerInfo".into(), Value::Dictionary(peer.clone()));
            session.insert(
                "timingPeerList".into(),
                Value::Array(vec![Value::Dictionary(peer)]),
            );
        }
        let session_response = exchange_plist(
            &mut connection,
            "SETUP",
            Value::Dictionary(session),
            "session SETUP",
        )
        .await?;
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
            ("latencyMax", desired_frames as u64),
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
        let receiver_min = stream
            .as_dictionary()
            .and_then(|d| d.get("latencyMin"))
            .and_then(Value::as_unsigned_integer)
            .unwrap_or(11025);
        let receiver_max = stream
            .as_dictionary()
            .and_then(|d| d.get("latencyMax"))
            .and_then(Value::as_unsigned_integer)
            .unwrap_or(desired_frames as u64);
        if receiver_min > receiver_max || receiver_max > SAMPLE_RATE as u64 * 10 {
            return Err(protocol("invalid receiver latency window"));
        }
        let latency_frames = (desired_frames as u64).clamp(receiver_min, receiver_max) as u32;
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
            spawn_feedback(connection.clone(), failure.clone()),
            spawn_retransmit(
                control.clone(),
                remote.ip(),
                cache.clone(),
                counters.clone(),
                failure.clone(),
            ),
        ];
        if let Some(wire) = event_wire {
            tasks.push(spawn_events(wire, failure.clone()));
        }
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
            latency_frames,
            ptp,
            timing: Some(timing),
            cache,
            failure,
            counters,
            tasks,
            credentials: options.credentials.clone(),
            stopped: Arc::new(AtomicBool::new(false)),
        };
        Ok(sender)
    }

    /// Feed interleaved 44.1kHz/16-bit stereo PCM. The caller must supply silence
    /// during source inactivity. A call is bounded to one second of PCM.
    pub async fn send_pcm(&mut self, samples: &[i16]) -> Result<()> {
        self.check_health()?;
        if samples.len() % CHANNELS != 0 || samples.len() > SAMPLE_RATE as usize * CHANNELS {
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
                let sync = timing::sync_packet(
                    self.encoder.timestamp,
                    self.latency_frames,
                    &timing.clock,
                    self.ptp,
                    first_sync,
                );
                self.control.send_to(&sync, self.control_peer).await?;
                self.last_sync = Some(Instant::now());
            }
            let mut chunk = [0i16; FRAMES_PER_PACKET * CHANNELS];
            for sample in &mut chunk {
                *sample = self.pending.pop_front().expect("complete packet checked");
            }
            let (sequence, packet) = self.encoder.packet(&chunk)?;
            self.transport.send_to(&packet, self.data_peer).await?;
            self.counters.packets.fetch_add(1, Ordering::Relaxed);
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
        VolumeControl { connection: Arc::downgrade(&self.connection), failure: Arc::downgrade(&self.failure), stopped: self.stopped.clone() }
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
            last_error: self.failure.lock().ok().and_then(|f| f.clone()),
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
            return Err(Error::SessionLost(failure.clone()));
        }
        Ok(())
    }
}

fn number(value: u64) -> Value {
    Value::Integer(value.into())
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
fn spawn_feedback(connection: Arc<AsyncMutex<Connection>>, failure: Failure) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut ticks = interval(Duration::from_secs(2));
        ticks.set_missed_tick_behavior(MissedTickBehavior::Skip);
        ticks.tick().await;
        loop {
            ticks.tick().await;
            let mut connection = connection.lock().await;
            let result = async {
                let response = connection
                    .exchange("POST", "/feedback", "", &[], &[])
                    .await?;
                connection.check(&response, "feedback")
            }
            .await;
            if let Err(error) = result {
                record_failure(&failure, error);
                return;
            }
        }
    })
}
fn spawn_events(mut wire: Wire, failure: Failure) -> JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let result = async {
                let (line, headers, _body) = wire.read_message().await?;
                if !line.starts_with("POST ") {
                    return Err(protocol("unexpected event-channel message"));
                }
                let mut response = String::from("RTSP/1.0 200 OK\r\nServer: Airloom\r\n");
                if let Some(sequence) = headers.get("cseq") {
                    let number = sequence
                        .parse::<u32>()
                        .map_err(|_| protocol("invalid event sequence"))?;
                    response.push_str(&format!("CSeq: {number}\r\n"));
                }
                response.push_str("\r\n");
                wire.write(response.as_bytes()).await
            }
            .await;
            if let Err(error) = result {
                record_failure(&failure, error);
                return;
            }
        }
    })
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
                    record_failure(&failure, error);
                    return;
                }
            };
            if address.ip() != peer || size < 8 || buffer[1] & 0x7f != 0x55 {
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
                    record_failure(&failure, "packet cache lock poisoned");
                    return;
                }
            };
            for packet in packets {
                let mut response = vec![0x80, 0xd6, buffer[2], buffer[3]];
                response.extend(packet);
                if let Err(error) = socket.send_to(&response, address).await {
                    record_failure(&failure, error);
                    return;
                }
                counters.retransmissions.fetch_add(1, Ordering::Relaxed);
            }
        }
    })
}
