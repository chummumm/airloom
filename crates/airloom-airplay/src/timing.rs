use crate::{Error, Result};
use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UdpSocket,
    sync::{mpsc, Mutex as AsyncMutex},
    task::JoinHandle,
    time::{interval, MissedTickBehavior},
};

pub struct Clock {
    epoch_ns: u64,
    epoch: Instant,
    offset_ns: AtomicI64,
    pub master_id: AtomicU64,
    pub exchanges: AtomicU64,
    pub following: AtomicBool,
    offset_ready: AtomicBool,
    last_exchange_ms: AtomicU64,
}
impl Clock {
    pub fn new(identity: u64) -> Self {
        Self::with_epoch(
            identity,
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            Instant::now(),
        )
    }
    fn with_epoch(identity: u64, epoch_ns: u64, epoch: Instant) -> Self {
        Self {
            epoch_ns,
            epoch,
            offset_ns: AtomicI64::new(0),
            master_id: AtomicU64::new(identity),
            exchanges: AtomicU64::new(0),
            following: AtomicBool::new(false),
            offset_ready: AtomicBool::new(false),
            last_exchange_ms: AtomicU64::new(0),
        }
    }
    pub fn local_ns(&self) -> u64 {
        self.epoch_ns
            .saturating_add(self.epoch.elapsed().as_nanos() as u64)
    }
    fn follow(&self, id: u64, offset: i64) {
        let previous = self.offset_ns.load(Ordering::Relaxed);
        let delta = offset as i128 - previous as i128;
        let offset =
            if self.last_exchange_ms.load(Ordering::Relaxed) != 0 && delta.abs() < 1_000_000 {
                (previous as i128 + delta / 8) as i64
            } else {
                offset
            };
        self.master_id.store(id, Ordering::Relaxed);
        self.offset_ns.store(offset, Ordering::Relaxed);
        self.following.store(true, Ordering::Release);
        self.offset_ready.store(true, Ordering::Release);
    }
    fn exchanged(&self) {
        self.exchanges.fetch_add(1, Ordering::Relaxed);
        self.last_exchange_ms.store(
            self.epoch.elapsed().as_millis() as u64 + 1,
            Ordering::Release,
        );
    }
    fn last_exchange_age(&self) -> Option<Duration> {
        let last = self.last_exchange_ms.load(Ordering::Acquire);
        (last != 0).then(|| {
            Duration::from_millis(
                (self.epoch.elapsed().as_millis() as u64 + 1).saturating_sub(last),
            )
        })
    }
}

pub struct TimingRuntime {
    pub port: u16,
    pub clock: Arc<Clock>,
    tasks: Vec<JoinHandle<()>>,
    ptp_registration: Option<PtpRegistration>,
    following_required: bool,
}
impl Drop for TimingRuntime {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

impl TimingRuntime {
    pub async fn wait_for_ptp_exchange(&self) -> Result<()> {
        let start = Instant::now();
        while self.clock.exchanges.load(Ordering::Acquire) < 3
            || (self.following_required && !self.clock.offset_ready.load(Ordering::Acquire))
        {
            if self.tasks.iter().any(JoinHandle::is_finished)
                || self
                    .ptp_registration
                    .as_ref()
                    .is_some_and(|registration| !registration.hub.healthy())
            {
                return Err(Error::SessionLost("timing worker ended".into()));
            }
            if start.elapsed() >= Duration::from_secs(5) {
                return Err(Error::Timeout("PTP clock exchange"));
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        Ok(())
    }
    pub fn check_health(&self, ptp: bool) -> Result<()> {
        if self.tasks.iter().any(JoinHandle::is_finished)
            || self
                .ptp_registration
                .as_ref()
                .is_some_and(|registration| !registration.hub.healthy())
        {
            return Err(Error::SessionLost("timing worker ended".into()));
        }
        if ptp
            && ((self.following_required && !self.clock.offset_ready.load(Ordering::Acquire))
                || self
                    .clock
                    .last_exchange_age()
                    .is_none_or(|age| age > Duration::from_secs(5)))
        {
            return Err(Error::SessionLost("PTP clock exchange stalled".into()));
        }
        Ok(())
    }
    pub async fn ntp(local: IpAddr, peer: IpAddr, identity: u64) -> Result<Self> {
        let socket = UdpSocket::bind(SocketAddr::new(local, 0)).await?;
        let port = socket.local_addr()?.port();
        let clock = Arc::new(Clock::new(identity));
        let task_clock = clock.clone();
        let task = tokio::spawn(async move {
            let mut buffer = [0; 2048];
            while let Ok((size, address)) = socket.recv_from(&mut buffer).await {
                if address.ip() != peer || size < 32 || buffer[1] & 0x7f != 0x52 {
                    continue;
                }
                let received = ntp_time(task_clock.local_ns());
                let mut response = [0; 32];
                response[0] = 0x80;
                response[1] = 0xd3;
                response[2..4].copy_from_slice(&buffer[2..4]);
                response[8..16].copy_from_slice(&buffer[24..32]);
                response[16..24].copy_from_slice(&received.to_be_bytes());
                response[24..32].copy_from_slice(&ntp_time(task_clock.local_ns()).to_be_bytes());
                if socket.send_to(&response, address).await.is_ok() {
                    task_clock.exchanged();
                }
            }
        });
        Ok(Self {
            port,
            clock,
            tasks: vec![task],
            ptp_registration: None,
            following_required: false,
        })
    }

    pub async fn ptp(
        local: IpAddr,
        peer: IpAddr,
        identity: u64,
        prefer_receiver: bool,
    ) -> Result<Self> {
        // One authority owns the fixed PTP ports per local interface. Reuse that
        // authority inside this process; never SO_REUSEPORT another application.
        // The hub outlives individual sessions so closing/replacing one receiver
        // cannot race the socket ownership of a second receiver.
        static HUBS: OnceLock<AsyncMutex<HashMap<IpAddr, Arc<PtpHub>>>> = OnceLock::new();
        let mut hubs = HUBS
            .get_or_init(|| AsyncMutex::new(HashMap::new()))
            .lock()
            .await;
        if hubs.get(&local).is_some_and(|hub| !hub.healthy()) {
            hubs.remove(&local);
        }
        let hub = if let Some(hub) = hubs.get(&local) {
            hub.clone()
        } else {
            let event = UdpSocket::bind(SocketAddr::new(local, 319))
                .await
                .map_err(|e| Error::SessionLost(format!("PTP UDP port 319 unavailable: {e}")))?;
            let general = UdpSocket::bind(SocketAddr::new(local, 320))
                .await
                .map_err(|e| Error::SessionLost(format!("PTP UDP port 320 unavailable: {e}")))?;
            let hub = PtpHub::from_sockets(event, general, identity);
            hubs.insert(local, hub.clone());
            hub
        };
        drop(hubs);
        hub.attach(
            SocketAddr::new(peer, 319),
            SocketAddr::new(peer, 320),
            prefer_receiver,
        )
    }
}

struct PtpPacket {
    data: Vec<u8>,
    source: SocketAddr,
    arrived_ns: u64,
}
struct PtpRoute {
    token: u64,
    packets: mpsc::Sender<PtpPacket>,
}
type PtpRoutes = Arc<Mutex<HashMap<IpAddr, PtpRoute>>>;

struct PtpHub {
    event: Arc<UdpSocket>,
    general: Arc<UdpSocket>,
    identity: u64,
    epoch_ns: u64,
    epoch: Instant,
    routes: PtpRoutes,
    next_token: AtomicU64,
    tasks: Vec<JoinHandle<()>>,
}
impl Drop for PtpHub {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}
impl PtpHub {
    fn from_sockets(event: UdpSocket, general: UdpSocket, identity: u64) -> Arc<Self> {
        let event = Arc::new(event);
        let general = Arc::new(general);
        let base = Clock::new(identity);
        let routes = Arc::new(Mutex::new(HashMap::new()));
        let tasks = [&event, &general]
            .into_iter()
            .map(|socket| {
                tokio::spawn(route_ptp(
                    socket.clone(),
                    routes.clone(),
                    base.epoch_ns,
                    base.epoch,
                ))
            })
            .collect();
        Arc::new(Self {
            event,
            general,
            identity,
            epoch_ns: base.epoch_ns,
            epoch: base.epoch,
            routes,
            next_token: AtomicU64::new(1),
            tasks,
        })
    }
    fn healthy(&self) -> bool {
        self.tasks.iter().all(|task| !task.is_finished())
    }
    fn attach(
        self: &Arc<Self>,
        event_peer: SocketAddr,
        general_peer: SocketAddr,
        prefer_receiver: bool,
    ) -> Result<TimingRuntime> {
        if event_peer.ip() != general_peer.ip() {
            return Err(Error::InvalidArgument(
                "PTP ports must belong to the same receiver".into(),
            ));
        }
        let peer = event_peer.ip();
        let port = self.event.local_addr()?.port();
        let (packets, received) = mpsc::channel(64);
        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        {
            let mut routes = self
                .routes
                .lock()
                .map_err(|_| crate::protocol("PTP route lock poisoned"))?;
            if routes
                .get(&peer)
                .is_some_and(|route| !route.packets.is_closed())
            {
                return Err(Error::InvalidArgument(
                    "a PTP session for this receiver is already active".into(),
                ));
            }
            routes.insert(peer, PtpRoute { token, packets });
        }
        let clock = Arc::new(Clock::with_epoch(self.identity, self.epoch_ns, self.epoch));
        let transport = PtpTransport {
            event: self.event.clone(),
            general: self.general.clone(),
            event_peer,
            general_peer,
            received,
        };
        let task = tokio::spawn(run_ptp(
            transport,
            self.identity,
            prefer_receiver,
            clock.clone(),
        ));
        Ok(TimingRuntime {
            port,
            clock,
            tasks: vec![task],
            ptp_registration: Some(PtpRegistration {
                hub: self.clone(),
                peer,
                token,
            }),
            following_required: prefer_receiver,
        })
    }
}
struct PtpRegistration {
    hub: Arc<PtpHub>,
    peer: IpAddr,
    token: u64,
}
impl Drop for PtpRegistration {
    fn drop(&mut self) {
        if let Ok(mut routes) = self.hub.routes.lock() {
            if routes
                .get(&self.peer)
                .is_some_and(|route| route.token == self.token)
            {
                routes.remove(&self.peer);
            }
        }
    }
}
async fn route_ptp(socket: Arc<UdpSocket>, routes: PtpRoutes, epoch_ns: u64, epoch: Instant) {
    let mut buffer = [0; 2048];
    while let Ok((size, source)) = socket.recv_from(&mut buffer).await {
        let arrived_ns = epoch_ns.saturating_add(epoch.elapsed().as_nanos() as u64);
        if size < 34 || buffer[0] >> 4 != 1 || buffer[1] & 15 != 2 {
            continue;
        }
        let Ok(routes) = routes.lock() else {
            return;
        };
        if let Some(route) = routes.get(&source.ip()) {
            // This is UDP: bound each peer's work and drop a packet when its
            // queue is full instead of blocking all other receiver clocks.
            let _ = route.packets.try_send(PtpPacket {
                data: buffer[..size].to_vec(),
                source,
                arrived_ns,
            });
        }
    }
}
struct PtpTransport {
    event: Arc<UdpSocket>,
    general: Arc<UdpSocket>,
    event_peer: SocketAddr,
    general_peer: SocketAddr,
    received: mpsc::Receiver<PtpPacket>,
}

pub fn ntp_time(unix_ns: u64) -> u64 {
    let seconds = unix_ns / 1_000_000_000 + 2_208_988_800;
    let fraction = ((unix_ns % 1_000_000_000) as u128 * (1u128 << 32) / 1_000_000_000) as u64;
    (seconds << 32) | fraction
}

pub fn sync_packet(
    anchor_timestamp: u32,
    anchor_local_ns: u64,
    latency_frames: u32,
    clock: &Clock,
    ptp: bool,
    first: bool,
) -> Vec<u8> {
    sync_packet_at(
        anchor_timestamp,
        anchor_local_ns,
        latency_frames,
        clock,
        ptp,
        first,
        clock.local_ns(),
    )
}

fn sync_packet_at(
    anchor_timestamp: u32,
    anchor_local_ns: u64,
    latency_frames: u32,
    clock: &Clock,
    ptp: bool,
    first: bool,
    local_ns: u64,
) -> Vec<u8> {
    // An immutable local-time/RTP relationship prevents packet scheduling jitter
    // from moving the receiver's render timeline at every sync announcement.
    let elapsed_frames = (local_ns.saturating_sub(anchor_local_ns) as u128
        * crate::SAMPLE_RATE as u128
        / 1_000_000_000) as u32;
    let timestamp = anchor_timestamp.wrapping_add(elapsed_frames);
    if ptp {
        let mut packet = vec![0; 28];
        packet[0] = if first { 0x90 } else { 0x80 };
        packet[1] = 0xd7;
        packet[3] = 6; // RTCP length is the number of 32-bit words minus one.
                       // PTP receivers anchor frame_1 - 11025 at the network timestamp.
                       // The published Apple/Spotify wire capture uses a 77175-frame gap
                       // between the two positions. Keep that receive window independent of
                       // shorter render leads, expanding it only for a lead beyond two seconds.
        let frame_1 = timestamp.wrapping_sub(latency_frames).wrapping_add(11025);
        packet[4..8].copy_from_slice(&frame_1.to_be_bytes());
        let master_ns = local_ns.saturating_add_signed(clock.offset_ns.load(Ordering::Acquire));
        packet[8..16].copy_from_slice(&master_ns.to_be_bytes());
        let receive_window = latency_frames.max(crate::SAMPLE_RATE * 2);
        packet[16..20].copy_from_slice(&frame_1.wrapping_add(receive_window - 11025).to_be_bytes());
        packet[20..28].copy_from_slice(&clock.master_id.load(Ordering::Acquire).to_be_bytes());
        packet
    } else {
        let mut packet = vec![0; 20];
        packet[0] = if first { 0x90 } else { 0x80 };
        packet[1] = 0xd4;
        packet[3] = 7;
        packet[4..8].copy_from_slice(&timestamp.wrapping_sub(latency_frames).to_be_bytes());
        packet[8..16].copy_from_slice(&ntp_time(local_ns).to_be_bytes());
        packet[16..20].copy_from_slice(&timestamp.to_be_bytes());
        packet
    }
}

fn ptp_header(kind: u8, length: usize, identity: u64, sequence: u16, flags: u16) -> Vec<u8> {
    let mut out = vec![0; length];
    out[0] = 0x10 | kind;
    out[1] = 2;
    out[2..4].copy_from_slice(&(length as u16).to_be_bytes());
    out[6..8].copy_from_slice(&flags.to_be_bytes());
    out[20..28].copy_from_slice(&identity.to_be_bytes());
    out[29] = 1;
    out[30..32].copy_from_slice(&sequence.to_be_bytes());
    out[32] = match kind {
        0 => 0,
        1 => 1,
        8 => 2,
        9 => 3,
        _ => 5,
    };
    out[33] = if kind == 0 || kind == 8 { 0xfd } else { 0 };
    out
}
fn put_timestamp(out: &mut [u8], ns: u64) {
    let sec = (ns / 1_000_000_000).to_be_bytes();
    out[..6].copy_from_slice(&sec[2..]);
    out[6..10].copy_from_slice(&((ns % 1_000_000_000) as u32).to_be_bytes());
}
fn timestamp(data: &[u8]) -> Option<u64> {
    if data.len() < 10 {
        return None;
    }
    let mut sec = [0; 8];
    sec[2..].copy_from_slice(&data[..6]);
    let nanos = u32::from_be_bytes(data[6..10].try_into().ok()?);
    if nanos >= 1_000_000_000 {
        return None;
    }
    u64::from_be_bytes(sec)
        .checked_mul(1_000_000_000)?
        .checked_add(nanos as u64)
}
fn correction(data: &[u8]) -> i64 {
    i64::from_be_bytes(data[8..16].try_into().unwrap()) / 65536
}
fn difference(remote: u64, local: u64) -> Option<i64> {
    i64::try_from(remote as i128 - local as i128).ok()
}

fn unicast_subscription(
    identity: u64,
    sequence: u16,
    kind: u8,
    target: Option<[u8; 10]>,
) -> Vec<u8> {
    let mut request = ptp_header(12, 54, identity, sequence, 0x0408);
    request[34..44].copy_from_slice(&target.unwrap_or([0xff; 10]));
    request[44..48].copy_from_slice(&[0, 4, 0, 6]); // REQUEST_UNICAST_TRANSMISSION
    request[48] = kind << 4;
    request[49] = if kind == 0 { (-3i8) as u8 } else { 0 };
    request[50..54].copy_from_slice(&30u32.to_be_bytes());
    request
}

async fn run_ptp(transport: PtpTransport, identity: u64, prefer_receiver: bool, clock: Arc<Clock>) {
    let PtpTransport {
        event,
        general,
        event_peer,
        general_peer,
        mut received,
    } = transport;
    let peer = event_peer.ip();
    let mut tick = interval(Duration::from_millis(125));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut sequence = 0u16;
    let mut subscription_sequence = 0u16;
    let mut followed_source = None::<[u8; 10]>;
    let mut sync_received = None::<(u16, u64, i64)>;
    let mut sync_offset = None::<i64>;
    let mut path_delay = 0i64;
    let mut pending_delay = None::<(u16, u64)>;
    let mut last_peer = Instant::now();
    loop {
        let received = tokio::select! {
            _=tick.tick()=>{
                sequence=sequence.wrapping_add(1);
                if clock.following.load(Ordering::Acquire) && last_peer.elapsed()>Duration::from_secs(15){
                    // A stale receiver clock cannot drive audio indefinitely.
                    clock.following.store(false,Ordering::Release);followed_source=None;sync_received=None;
                    clock.offset_ready.store(false,Ordering::Release);
                    clock.master_id.store(identity,Ordering::Relaxed);clock.offset_ns.store(0,Ordering::Relaxed);
                }
                if !prefer_receiver {
                    let sync=ptp_header(0,44,identity,sequence,0x0600);
                    let sent=clock.local_ns();if event.send_to(&sync,event_peer).await.is_err(){return;}
                    let mut follow=ptp_header(8,44,identity,sequence,0x0400);put_timestamp(&mut follow[34..44],sent);
                    if general.send_to(&follow,general_peer).await.is_err(){return;}
                    if sequence%8==1 {
                        let mut announce=ptp_header(11,64,identity,sequence,0x0400);
                        // AirPlay senders use this grandmaster dataset. It is a
                        // wire-compatibility advertisement, not a precision claim.
                        announce[47]=128;announce[48]=6;announce[49]=0x21;
                        announce[50..52].copy_from_slice(&0x436au16.to_be_bytes());announce[52]=128;
                        announce[53..61].copy_from_slice(&identity.to_be_bytes());announce[63]=0x20;
                        if general.send_to(&announce,general_peer).await.is_err(){return;}
                    }
                } else if sequence%8==1 {
                    // A receiver clock may send unicast Sync/Announce only after
                    // subscription. Request it before session SETUP as well as
                    // renewing during streaming; grants alone never mean ready.
                    for kind in [0,9,11] {
                        subscription_sequence=subscription_sequence.wrapping_add(1);
                        let request=unicast_subscription(identity,subscription_sequence,kind,followed_source);
                        if general.send_to(&request,general_peer).await.is_err(){return;}
                    }
                    if clock.following.load(Ordering::Acquire) {
                        let request=ptp_header(1,44,identity,sequence,0x0400);
                        pending_delay=Some((sequence,clock.local_ns()));
                        if event.send_to(&request,event_peer).await.is_err(){return;}
                    }
                }
                continue;
            },
            packet=received.recv()=>packet,
        };
        let Some(PtpPacket {
            data,
            source: address,
            arrived_ns: arrived,
        }) = received
        else {
            return;
        };
        if address.ip() != peer || data.len() < 34 || data[1] & 0x0f != 2 || data[0] >> 4 != 1 {
            continue;
        }
        let length = u16::from_be_bytes([data[2], data[3]]) as usize;
        if length < 34 || length > data.len() {
            continue;
        }
        let kind = data[0] & 15;
        let seq = u16::from_be_bytes([data[30], data[31]]);
        let source: [u8; 10] = data[20..30].try_into().unwrap();
        match kind {
            11 if length >= 64 => {
                if prefer_receiver {
                    followed_source = Some(source);
                    last_peer = Instant::now();
                    clock.master_id.store(
                        u64::from_be_bytes(data[53..61].try_into().unwrap()),
                        Ordering::Relaxed,
                    );
                    clock.following.store(true, Ordering::Release);
                }
            }
            0 if length >= 44 && followed_source == Some(source) => {
                last_peer = Instant::now();
                let flags = u16::from_be_bytes([data[6], data[7]]);
                if flags & 0x0200 != 0 {
                    sync_received = Some((seq, arrived, correction(&data)));
                } else if let Some(origin) = timestamp(&data[34..44]) {
                    if let Some(offset) =
                        difference(origin.saturating_add_signed(correction(&data)), arrived)
                    {
                        sync_offset = Some(offset);
                        clock.follow(
                            clock.master_id.load(Ordering::Relaxed),
                            offset.saturating_add(path_delay),
                        );
                        clock.exchanged();
                    }
                }
            }
            8 if length >= 44 && followed_source == Some(source) => {
                if let (Some((pending, received, sync_correction)), Some(origin)) =
                    (sync_received, timestamp(&data[34..44]))
                {
                    if pending == seq {
                        if let Some(offset) = difference(
                            origin.saturating_add_signed(
                                sync_correction.saturating_add(correction(&data)),
                            ),
                            received,
                        ) {
                            sync_offset = Some(offset);
                            clock.follow(
                                clock.master_id.load(Ordering::Relaxed),
                                offset.saturating_add(path_delay),
                            );
                            clock.exchanged();
                            last_peer = Instant::now();
                        }
                        sync_received = None;
                    }
                }
            }
            9 if length >= 54 && followed_source == Some(source) => {
                if data[44..52] == identity.to_be_bytes() {
                    if let (Some((pending, sent)), Some(remote), Some(forward)) =
                        (pending_delay, timestamp(&data[34..44]), sync_offset)
                    {
                        if seq == pending {
                            if let Some(backward) = difference(remote, sent) {
                                path_delay = ((backward as i128 - forward as i128) / 2)
                                    .clamp(0, 100_000_000)
                                    as i64;
                                let adjusted = ((forward as i128 + backward as i128) / 2) as i64;
                                clock.follow(clock.master_id.load(Ordering::Relaxed), adjusted);
                                clock.exchanged();
                            }
                            pending_delay = None;
                        }
                    }
                }
            }
            1 if length >= 44 => {
                let mut response = ptp_header(9, 54, identity, seq, 0x0400);
                put_timestamp(&mut response[34..44], arrived);
                response[44..54].copy_from_slice(&source);
                if general.send_to(&response, general_peer).await.is_ok() {
                    clock.exchanged();
                }
            }
            2 if length >= 54 => {
                let mut response = ptp_header(3, 54, identity, seq, 0x0600);
                put_timestamp(&mut response[34..44], arrived);
                response[44..54].copy_from_slice(&source);
                let sent = clock.local_ns();
                if event.send_to(&response, event_peer).await.is_err() {
                    return;
                }
                let mut follow = ptp_header(10, 54, identity, seq, 0x0400);
                put_timestamp(&mut follow[34..44], sent);
                follow[44..54].copy_from_slice(&source);
                if general.send_to(&follow, general_peer).await.is_ok() {
                    clock.exchanged();
                }
            }
            12 if length >= 50 => {
                // IEEE 1588 unicast grant, bounded to this selected receiver.
                let mut offset = 44;
                while offset + 4 <= length {
                    let tag = u16::from_be_bytes([data[offset], data[offset + 1]]);
                    let size = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
                    if offset + 4 + size > length {
                        break;
                    }
                    if tag == 4 && size == 6 {
                        let mut grant = ptp_header(12, 56, identity, seq, 0x0400);
                        grant[34..44].copy_from_slice(&source);
                        grant[44..46].copy_from_slice(&5u16.to_be_bytes());
                        grant[46..48].copy_from_slice(&8u16.to_be_bytes());
                        grant[48..54].copy_from_slice(&data[offset + 4..offset + 10]);
                        let _ = general.send_to(&grant, general_peer).await;
                    }
                    offset += 4 + size;
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ptp_announce_matches_independent_spotify_ios_capture() {
        // Published capture: OwnTone src/outputs/rtp_common.c, commit
        // 3aee59740c715d40df8de87e06b7b91badfbe20b, Spotify iOS AirPlay 2.
        // This fixture is not generated by our encoder or our loopback receiver.
        let captured =
            hex::decode("90d70006cbbbfe400001fd1685aea593cbbd2bb7f84288888f4f0008").unwrap();
        let clock = Clock::new(0xf842_8888_8f4f_0008);
        let instant = 0x0001_fd16_85ae_a593;
        let actual = sync_packet_at(0xcbbd_2bb7, instant, 88200, &clock, true, true, instant);
        assert_eq!(actual, captured);
    }

    #[test]
    fn ptp_announce_receiver_mapping_preserves_requested_lead_across_wrap() {
        // Independent receiver contract: Shairport Sync rtp.c anchors
        // frame_1 - 11025 at the announced PTP nanosecond timestamp.
        // The two advertised RTP positions differ by 77175 in Apple's capture.
        let clock = Clock::new(0x1122_3344_5566_7788);
        clock.offset_ns.store(-7_000_000_000, Ordering::Relaxed);
        let anchor_ns = 1_000_000_000_000;
        let first_audio = 100u32; // subtracting the lead must wrap, not saturate.
        let lead_frames = 33075u32;
        for elapsed in [0u64, 1_000_000_000, 7_000_000_000] {
            let packet = sync_packet_at(
                first_audio,
                anchor_ns,
                lead_frames,
                &clock,
                true,
                elapsed == 0,
                anchor_ns + elapsed,
            );
            assert_eq!(
                4 * (u16::from_be_bytes([packet[2], packet[3]]) as usize + 1),
                packet.len()
            );
            let frame_1 = u32::from_be_bytes(packet[4..8].try_into().unwrap());
            let frame_2 = u32::from_be_bytes(packet[16..20].try_into().unwrap());
            assert_eq!(frame_2.wrapping_sub(frame_1), 77175);
            let rendering = frame_1.wrapping_sub(11025);
            let expected = first_audio
                .wrapping_add((elapsed / 1_000_000_000) as u32 * crate::SAMPLE_RATE)
                .wrapping_sub(lead_frames);
            assert_eq!(rendering, expected);
            let network = u64::from_be_bytes(packet[8..16].try_into().unwrap());
            assert_eq!(network, anchor_ns + elapsed - 7_000_000_000);
        }
    }

    async fn next_kind(socket: &UdpSocket, kind: u8) -> Vec<u8> {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                let mut packet = [0; 2048];
                let (size, _) = socket.recv_from(&mut packet).await.unwrap();
                if packet[0] & 15 == kind {
                    return packet[..size].to_vec();
                }
            }
        })
        .await
        .expect("expected PTP message")
    }

    #[test]
    fn shared_ptp_hub_rebinds_after_the_owning_tokio_runtime_is_destroyed() {
        let local: IpAddr = "127.0.0.30".parse().unwrap();
        let peer: IpAddr = "127.0.0.31".parse().unwrap();
        for identity in [0x4444, 0x5555] {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            runtime.block_on(async {
                let timing = TimingRuntime::ptp(local, peer, identity, false)
                    .await
                    .unwrap();
                assert_eq!(timing.clock.master_id.load(Ordering::Acquire), identity);
                drop(timing);
                // The interface authority deliberately survives its final peer.
                assert!(std::net::UdpSocket::bind(SocketAddr::new(local, 319)).is_err());
                tokio::task::yield_now().await;
            });
            // All socket-using futures are destroyed here. On the next pass the
            // registry must remove the stale hub before rebinding the same ports.
            drop(runtime);
        }
    }

    #[tokio::test]
    async fn two_ptp_receivers_share_fixed_ports_but_keep_clock_domains_and_lifetimes_separate() {
        // Distinct loopback addresses model two devices on one interface and
        // exercise the production registry/fixed-port path, not SO_REUSEPORT.
        let local: IpAddr = "127.0.0.10".parse().unwrap();
        let first: IpAddr = "127.0.0.11".parse().unwrap();
        let second: IpAddr = "127.0.0.12".parse().unwrap();
        let first_event = UdpSocket::bind(SocketAddr::new(first, 319)).await.unwrap();
        let first_general = UdpSocket::bind(SocketAddr::new(first, 320)).await.unwrap();
        let second_event = UdpSocket::bind(SocketAddr::new(second, 319)).await.unwrap();
        let second_general = UdpSocket::bind(SocketAddr::new(second, 320)).await.unwrap();
        let one = TimingRuntime::ptp(local, first, 0x1111, false)
            .await
            .unwrap();
        let two = TimingRuntime::ptp(local, second, 0x2222, true)
            .await
            .unwrap();
        assert!(Arc::ptr_eq(
            &one.ptp_registration.as_ref().unwrap().hub,
            &two.ptp_registration.as_ref().unwrap().hub
        ));
        assert_eq!(one.port, 319);
        assert_eq!(two.port, 319);
        assert_eq!(one.clock.epoch_ns, two.clock.epoch_ns);
        assert!(matches!(
            TimingRuntime::ptp(local, first, 0x3333, false).await,
            Err(Error::InvalidArgument(_))
        ));

        let sync = next_kind(&first_event, 0).await;
        assert_eq!(&sync[20..28], &0x1111u64.to_be_bytes());
        let subscription = next_kind(&second_general, 12).await;
        assert_eq!(&subscription[20..28], &0x1111u64.to_be_bytes());
        assert_eq!(&subscription[34..44], &[0xff; 10]);
        assert_eq!(&subscription[44..48], &[0, 4, 0, 6]);
        assert_eq!(subscription[48] >> 4, 0);
        assert!(!two.clock.offset_ready.load(Ordering::Acquire));

        let delay = ptp_header(1, 44, 0xaaaa, 77, 0x0400);
        first_event
            .send_to(&delay, SocketAddr::new(local, 319))
            .await
            .unwrap();
        let response = next_kind(&first_general, 9).await;
        assert_eq!(&response[44..54], &delay[20..30]);
        assert_eq!(one.clock.exchanges.load(Ordering::Acquire), 1);
        assert_eq!(
            two.clock.exchanges.load(Ordering::Acquire),
            0,
            "another peer's delay request must not make this clock ready"
        );

        let remote_id = 0xf123_4567_89ab_cdefu64;
        let mut announce = ptp_header(11, 64, remote_id, 1, 0x0400);
        announce[53..61].copy_from_slice(&remote_id.to_be_bytes());
        second_general
            .send_to(&announce, SocketAddr::new(local, 320))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !two.clock.following.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        for seq in 2..5 {
            let origin = two.clock.local_ns() + 5_000_000_000;
            let sync = ptp_header(0, 44, remote_id, seq, 0x0600);
            second_event
                .send_to(&sync, SocketAddr::new(local, 319))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(5)).await;
            let mut follow = ptp_header(8, 44, remote_id, seq, 0x0400);
            put_timestamp(&mut follow[34..44], origin);
            second_general
                .send_to(&follow, SocketAddr::new(local, 320))
                .await
                .unwrap();
        }
        two.wait_for_ptp_exchange().await.unwrap();
        assert_eq!(one.clock.master_id.load(Ordering::Acquire), 0x1111);
        assert_eq!(one.clock.offset_ns.load(Ordering::Acquire), 0);
        assert_eq!(two.clock.master_id.load(Ordering::Acquire), remote_id);
        assert!(
            (4_950_000_000..5_050_000_000).contains(&two.clock.offset_ns.load(Ordering::Acquire))
        );

        drop(one);
        let before = two.clock.exchanges.load(Ordering::Acquire);
        second_event
            .send_to(
                &ptp_header(1, 44, remote_id, 88, 0x0400),
                SocketAddr::new(local, 319),
            )
            .await
            .unwrap();
        let response = next_kind(&second_general, 9).await;
        assert_eq!(&response[30..32], &88u16.to_be_bytes());
        assert!(two.clock.exchanges.load(Ordering::Acquire) > before);
        two.check_health(true).unwrap();
        let routes = two
            .ptp_registration
            .as_ref()
            .unwrap()
            .hub
            .routes
            .lock()
            .unwrap();
        assert!(!routes.contains_key(&first));
        assert!(routes.contains_key(&second));
        drop(routes);
        drop(two);
    }
    #[tokio::test]
    async fn ptp_master_sends_gptp_and_answers_delay_requests() {
        let peer_event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = event.local_addr().unwrap();
        let general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let hub = PtpHub::from_sockets(event, general, 1234);
        let runtime = hub
            .attach(
                peer_event.local_addr().unwrap(),
                peer_general.local_addr().unwrap(),
                false,
            )
            .unwrap();
        let clock = runtime.clock.clone();
        let sync = next_kind(&peer_event, 0).await;
        assert_eq!(sync[0], 0x10);
        assert_eq!(sync[6] & 2, 2);
        let announce = next_kind(&peer_general, 11).await;
        assert_eq!(announce[47], 128);
        let request = ptp_header(1, 44, 5678, 42, 0x0400);
        peer_event.send_to(&request, address).await.unwrap();
        let response = next_kind(&peer_general, 9).await;
        assert_eq!(&response[30..32], &42u16.to_be_bytes());
        assert_eq!(&response[44..54], &request[20..30]);
        assert!(timestamp(&response[34..44]).unwrap() > 0);
        assert_eq!(clock.exchanges.load(Ordering::Relaxed), 1);
        drop(runtime);
    }
    #[tokio::test]
    async fn ptp_follower_uses_receiver_clock_identity_and_timebase() {
        let peer_event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let event_address = event.local_addr().unwrap();
        let general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let general_address = general.local_addr().unwrap();
        let remote_id = 0x1122334455667788u64;
        let hub = PtpHub::from_sockets(event, general, 1234);
        let runtime = hub
            .attach(
                peer_event.local_addr().unwrap(),
                peer_general.local_addr().unwrap(),
                true,
            )
            .unwrap();
        let clock = runtime.clock.clone();
        let mut announce = ptp_header(11, 64, remote_id, 1, 0x0400);
        announce[47] = 248;
        announce[53..61].copy_from_slice(&remote_id.to_be_bytes());
        peer_general
            .send_to(&announce, general_address)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while !clock.following.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        let sent = clock.local_ns();
        let sync = ptp_header(0, 44, remote_id, 2, 0x0600);
        peer_event.send_to(&sync, event_address).await.unwrap();
        // Separate two-step datagrams, as on the wire; let Sync be observed before Follow_Up.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let mut follow = ptp_header(8, 44, remote_id, 2, 0x0400);
        put_timestamp(&mut follow[34..44], sent + 5_000_000_000);
        peer_general
            .send_to(&follow, general_address)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while clock.exchanges.load(Ordering::Acquire) == 0 {
                tokio::time::sleep(Duration::from_millis(1)).await;
            }
        })
        .await
        .unwrap();
        assert_eq!(clock.master_id.load(Ordering::Relaxed), remote_id);
        assert!((4_950_000_000..5_050_000_000).contains(&clock.offset_ns.load(Ordering::Relaxed)));
        let packet = sync_packet(100_000, clock.local_ns(), 11025, &clock, true, true);
        assert_eq!(&packet[..2], &[0x90, 0xd7]);
        assert_eq!(&packet[20..28], &remote_id.to_be_bytes());
        drop(runtime);
    }
    #[test]
    fn timestamps_and_ptp_frames_use_network_order() {
        let time = 1_700_000_000_123_456_789;
        let mut encoded = [0; 10];
        put_timestamp(&mut encoded, time);
        assert_eq!(timestamp(&encoded), Some(time));
        assert_eq!(ntp_time(0), 2_208_988_800u64 << 32);
        let packet = ptp_header(8, 44, 0x0102030405060708, 9, 0x400);
        assert_eq!(&packet[20..28], &[1, 2, 3, 4, 5, 6, 7, 8]);
        assert_eq!(packet[0], 0x18);
        assert_eq!(&packet[2..4], &44u16.to_be_bytes());
    }
}
