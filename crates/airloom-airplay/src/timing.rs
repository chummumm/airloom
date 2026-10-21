use crate::{Error, Result};
use std::{
    net::{IpAddr, SocketAddr},
    sync::{
        atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::{
    net::UdpSocket,
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
    last_exchange_ms: AtomicU64,
}
impl Clock {
    pub fn new(identity: u64) -> Self {
        Self {
            epoch_ns: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos() as u64,
            epoch: Instant::now(),
            offset_ns: AtomicI64::new(0),
            master_id: AtomicU64::new(identity),
            exchanges: AtomicU64::new(0),
            following: AtomicBool::new(false),
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
        while self.clock.exchanges.load(Ordering::Acquire) < 3 {
            if self.tasks.iter().any(JoinHandle::is_finished) {
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
        if self.tasks.iter().any(JoinHandle::is_finished) {
            return Err(Error::SessionLost("timing worker ended".into()));
        }
        if ptp
            && self
                .clock
                .last_exchange_age()
                .is_none_or(|age| age > Duration::from_secs(5))
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
        })
    }

    pub async fn ptp(
        local: IpAddr,
        peer: IpAddr,
        identity: u64,
        prefer_receiver: bool,
    ) -> Result<Self> {
        // Ports belong to a single clock authority. Do not silently share them with
        // another application's PTP implementation or fall back to incompatible NTP.
        let event = UdpSocket::bind(SocketAddr::new(local, 319))
            .await
            .map_err(|e| Error::SessionLost(format!("PTP UDP port 319 unavailable: {e}")))?;
        let general = UdpSocket::bind(SocketAddr::new(local, 320))
            .await
            .map_err(|e| Error::SessionLost(format!("PTP UDP port 320 unavailable: {e}")))?;
        let clock = Arc::new(Clock::new(identity));
        let task_clock = clock.clone();
        let task = tokio::spawn(async move {
            run_ptp(
                event,
                general,
                SocketAddr::new(peer, 319),
                SocketAddr::new(peer, 320),
                identity,
                prefer_receiver,
                task_clock,
            )
            .await;
        });
        Ok(Self {
            port: 319,
            clock,
            tasks: vec![task],
        })
    }
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
    // An immutable local-time/RTP relationship prevents packet scheduling jitter
    // from moving the receiver's render timeline at every sync announcement.
    let local_ns = clock.local_ns();
    let elapsed_frames = (local_ns.saturating_sub(anchor_local_ns) as u128
        * crate::SAMPLE_RATE as u128
        / 1_000_000_000) as u32;
    let timestamp = anchor_timestamp.wrapping_add(elapsed_frames);
    if ptp {
        let mut packet = vec![0; 28];
        packet[0] = if first { 0x90 } else { 0x80 };
        packet[1] = 0xd7;
        packet[4..8].copy_from_slice(&timestamp.wrapping_sub(latency_frames).to_be_bytes());
        let master_ns = local_ns.saturating_add_signed(clock.offset_ns.load(Ordering::Acquire));
        packet[8..16].copy_from_slice(&master_ns.to_be_bytes());
        packet[16..20].copy_from_slice(&timestamp.to_be_bytes());
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

async fn run_ptp(
    event: UdpSocket,
    general: UdpSocket,
    event_peer: SocketAddr,
    general_peer: SocketAddr,
    identity: u64,
    prefer_receiver: bool,
    clock: Arc<Clock>,
) {
    let peer = event_peer.ip();
    let mut tick = interval(Duration::from_millis(125));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut event_buffer = [0; 2048];
    let mut general_buffer = [0; 2048];
    let mut sequence = 0u16;
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
                } else if clock.following.load(Ordering::Acquire) && sequence%8==1 {
                    let request=ptp_header(1,44,identity,sequence,0x0400);
                    pending_delay=Some((sequence,clock.local_ns()));
                    if event.send_to(&request,event_peer).await.is_err(){return;}
                }
                continue;
            },
            packet=event.recv_from(&mut event_buffer)=>match packet{Ok((size,address))=>Some((event_buffer[..size].to_vec(),address)),Err(_)=>None},
            packet=general.recv_from(&mut general_buffer)=>match packet{Ok((size,address))=>Some((general_buffer[..size].to_vec(),address)),Err(_)=>None},
        };
        let Some((data, address)) = received else {
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
        let arrived = clock.local_ns();
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
    #[tokio::test]
    async fn ptp_master_sends_gptp_and_answers_delay_requests() {
        let peer_event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let address = event.local_addr().unwrap();
        let general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let clock = Arc::new(Clock::new(1234));
        let task = tokio::spawn(run_ptp(
            event,
            general,
            peer_event.local_addr().unwrap(),
            peer_general.local_addr().unwrap(),
            1234,
            false,
            clock.clone(),
        ));
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
        task.abort();
        let _ = task.await;
    }
    #[tokio::test]
    async fn ptp_follower_uses_receiver_clock_identity_and_timebase() {
        let peer_event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let event = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let event_address = event.local_addr().unwrap();
        let general = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let general_address = general.local_addr().unwrap();
        let clock = Arc::new(Clock::new(1234));
        let remote_id = 0x1122334455667788u64;
        let task = tokio::spawn(run_ptp(
            event,
            general,
            peer_event.local_addr().unwrap(),
            peer_general.local_addr().unwrap(),
            1234,
            true,
            clock.clone(),
        ));
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
        task.abort();
        let _ = task.await;
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
