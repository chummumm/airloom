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
        }
    }
    pub fn local_ns(&self) -> u64 {
        self.epoch_ns
            .saturating_add(self.epoch.elapsed().as_nanos() as u64)
    }
    pub fn now_ns(&self) -> u64 {
        self.local_ns()
            .saturating_add_signed(self.offset_ns.load(Ordering::Relaxed))
    }
    fn follow(&self, id: u64, offset: i64) {
        self.master_id.store(id, Ordering::Relaxed);
        self.offset_ns.store(offset, Ordering::Relaxed);
        self.following.store(true, Ordering::Release);
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
                    task_clock.exchanges.fetch_add(1, Ordering::Relaxed);
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
            run_ptp(event, general, peer, identity, prefer_receiver, task_clock).await;
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
    timestamp: u32,
    latency_frames: u32,
    clock: &Clock,
    ptp: bool,
    first: bool,
) -> Vec<u8> {
    if ptp {
        let mut packet = vec![0; 28];
        packet[0] = if first { 0x90 } else { 0x80 };
        packet[1] = 0xd7;
        packet[4..8].copy_from_slice(&timestamp.wrapping_sub(latency_frames).to_be_bytes());
        packet[8..16].copy_from_slice(&clock.now_ns().to_be_bytes());
        packet[16..20].copy_from_slice(&timestamp.to_be_bytes());
        packet[20..28].copy_from_slice(&clock.master_id.load(Ordering::Acquire).to_be_bytes());
        packet
    } else {
        let mut packet = vec![0; 20];
        packet[0] = if first { 0x90 } else { 0x80 };
        packet[1] = 0xd4;
        packet[3] = 7;
        packet[4..8].copy_from_slice(&timestamp.wrapping_sub(latency_frames).to_be_bytes());
        packet[8..16].copy_from_slice(&ntp_time(clock.local_ns()).to_be_bytes());
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
    peer: IpAddr,
    identity: u64,
    prefer_receiver: bool,
    clock: Arc<Clock>,
) {
    let event_peer = SocketAddr::new(peer, 319);
    let general_peer = SocketAddr::new(peer, 320);
    let mut tick = interval(Duration::from_millis(125));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let mut event_buffer = [0; 2048];
    let mut general_buffer = [0; 2048];
    let mut sequence = 0u16;
    let mut followed_source = None::<[u8; 10]>;
    let mut sync_received = None::<(u16, u64, i64)>;
    let mut sync_offset = None::<i64>;
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
                if !clock.following.load(Ordering::Acquire) {
                    let sync=ptp_header(0,44,identity,sequence,0x0600);
                    let sent=clock.local_ns();if event.send_to(&sync,event_peer).await.is_err(){return;}
                    let mut follow=ptp_header(8,44,identity,sequence,0x0400);put_timestamp(&mut follow[34..44],sent);
                    if general.send_to(&follow,general_peer).await.is_err(){return;}
                    if sequence%8==1 {
                        let mut announce=ptp_header(11,64,identity,sequence,0x0400);
                        announce[47]=if prefer_receiver{250}else{128};announce[48]=248;announce[49]=0xfe;
                        announce[50..52].copy_from_slice(&0xffffu16.to_be_bytes());announce[52]=248;
                        announce[53..61].copy_from_slice(&identity.to_be_bytes());announce[63]=0xa0;
                        if general.send_to(&announce,general_peer).await.is_err(){return;}
                    }
                } else if sequence%8==1 {
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
                if prefer_receiver || data[47] < 128 {
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
                        clock.follow(clock.master_id.load(Ordering::Relaxed), offset);
                        clock.exchanges.fetch_add(1, Ordering::Relaxed);
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
                            clock.follow(clock.master_id.load(Ordering::Relaxed), offset);
                            clock.exchanges.fetch_add(1, Ordering::Relaxed);
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
                                let adjusted = ((forward as i128 + backward as i128) / 2) as i64;
                                clock.follow(clock.master_id.load(Ordering::Relaxed), adjusted);
                                clock.exchanges.fetch_add(1, Ordering::Relaxed);
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
                    clock.exchanges.fetch_add(1, Ordering::Relaxed);
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
                    clock.exchanges.fetch_add(1, Ordering::Relaxed);
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
