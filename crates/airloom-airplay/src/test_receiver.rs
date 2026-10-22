//! An in-process receiver used to exercise real TCP/UDP boundaries. This is not
//! a HomePod emulator and passing these tests is not hardware qualification.
use crate::{audio, crypto, rtsp::Wire, tlv, *};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use md5::{Digest, Md5};
use num_bigint::BigUint;
use plist::{Dictionary, Value};
use sha2::Sha512;
use srp::groups::G_3072;
use std::{
    collections::BTreeMap,
    io::Cursor,
    net::{IpAddr, SocketAddr},
    time::Duration,
};
use tokio::{
    net::{TcpListener, UdpSocket},
    sync::oneshot,
    task::JoinHandle,
    time::timeout,
};
use x25519_dalek::{PublicKey, StaticSecret};

const PASSWORD: &str = "airloom-fixture-not-a-secret";
type Message = (String, BTreeMap<String, String>, Vec<u8>);

fn hash(parts: &[&[u8]]) -> Vec<u8> {
    let mut digest = Sha512::new();
    for part in parts {
        digest.update(part);
    }
    digest.finalize().to_vec()
}
fn padded(integer: &BigUint) -> Vec<u8> {
    let bytes = integer.to_bytes_be();
    let mut out = vec![0; 384];
    out[384 - bytes.len()..].copy_from_slice(&bytes);
    out
}
fn integer(value: u64) -> Value {
    Value::Integer(value.into())
}
fn options(password: &str) -> ConnectOptions {
    ConnectOptions {
        password: Some(password.into()),
        credentials: None,
        latency: Duration::from_millis(750),
        initial_volume: None,
        timing: TimingMode::Ntp,
    }
}
fn plist(fields: impl IntoIterator<Item = (&'static str, Value)>) -> Vec<u8> {
    let mut out = Vec::new();
    Value::Dictionary(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
        .to_writer_binary(&mut out)
        .unwrap();
    out
}
fn decode(body: &[u8]) -> Value {
    Value::from_reader(Cursor::new(body)).unwrap()
}

async fn request(wire: &mut Wire) -> Message {
    timeout(Duration::from_secs(3), wire.read_message())
        .await
        .expect("request deadline")
        .expect("valid request")
}
async fn reply(wire: &mut Wire, request: &Message, status: u16, body: &[u8], extra: &str) {
    let mut bytes = format!(
        "RTSP/1.0 {status} Test\r\nCSeq: {}\r\nContent-Length: {}\r\n{extra}\r\n",
        request.1["cseq"],
        body.len()
    )
    .into_bytes();
    bytes.extend_from_slice(body);
    wire.write(&bytes).await.unwrap();
}

// Server-side modular arithmetic is independent of SrpExchange's client path.
// Its deterministic values are additionally checked against the srptools fixture.
struct ServerSrp {
    salt: Vec<u8>,
    private: BigUint,
    public: Vec<u8>,
    verifier: BigUint,
}
impl ServerSrp {
    fn new(password: &str) -> Self {
        let group = &*G_3072;
        let salt: Vec<u8> = (0..16).collect();
        let private = BigUint::from_bytes_be(&(64..96).collect::<Vec<u8>>());
        let identity = hash(&[b"Pair-Setup:", password.as_bytes()]);
        let x = BigUint::from_bytes_be(&hash(&[&salt, &identity]));
        let verifier = group.g.modpow(&x, &group.n);
        let k = BigUint::from_bytes_be(&hash(&[&group.n.to_bytes_be(), &padded(&group.g)]));
        let public =
            ((&k * &verifier + group.g.modpow(&private, &group.n)) % &group.n).to_bytes_be();
        Self {
            salt,
            private,
            public,
            verifier,
        }
    }
    fn finish(&self, client: &[u8], proof: &[u8]) -> Option<(Vec<u8>, Vec<u8>)> {
        let group = &*G_3072;
        let a = BigUint::from_bytes_be(client);
        assert_ne!(&a % &group.n, BigUint::default());
        let u = BigUint::from_bytes_be(&hash(&[
            &padded(&a),
            &padded(&BigUint::from_bytes_be(&self.public)),
        ]));
        let s = (&a * self.verifier.modpow(&u, &group.n)).modpow(&self.private, &group.n);
        let key = hash(&[&s.to_bytes_be()]);
        let mut ng = hash(&[&group.n.to_bytes_be()]);
        for (n, g) in ng.iter_mut().zip(hash(&[&group.g.to_bytes_be()])) {
            *n ^= g;
        }
        let expected = hash(&[
            &ng,
            &hash(&[b"Pair-Setup"]),
            &self.salt,
            client,
            &self.public,
            &key,
        ]);
        if expected != proof {
            return None;
        }
        let server_proof = hash(&[client, proof, &key]);
        Some((key, server_proof))
    }
}

async fn server_srp(wire: &mut Wire, password: &str, transient: bool) -> Option<Vec<u8>> {
    let first = request(wire).await;
    assert!(first.0.starts_with("POST /pair-setup "));
    assert_eq!(first.1["x-apple-hkp"], if transient { "4" } else { "3" });
    let fields = tlv::decode(&first.2).unwrap();
    assert_eq!(fields[&tlv::STATE], [1]);
    assert_eq!(
        fields.get(&tlv::FLAGS).map(Vec::as_slice),
        if transient { Some(&[0x10][..]) } else { None }
    );
    let server = ServerSrp::new(password);
    let challenge = tlv::encode(&[
        (tlv::STATE, &[2]),
        (tlv::SALT, &server.salt),
        (tlv::PUBLIC_KEY, &server.public),
    ]);
    reply(wire, &first, 200, &challenge, "").await;
    let third = request(wire).await;
    let fields = tlv::decode(&third.2).unwrap();
    assert_eq!(fields[&tlv::STATE], [3]);
    if let Some((key, proof)) = server.finish(&fields[&tlv::PUBLIC_KEY], &fields[&tlv::PROOF]) {
        reply(
            wire,
            &third,
            200,
            &tlv::encode(&[(tlv::STATE, &[4]), (tlv::PROOF, &proof)]),
            "",
        )
        .await;
        Some(key)
    } else {
        reply(
            wire,
            &third,
            200,
            &tlv::encode(&[(tlv::STATE, &[4]), (tlv::ERROR, &[2])]),
            "",
        )
        .await;
        None
    }
}

struct AudioOffer {
    key: [u8; 32],
    control: SocketAddr,
    timing: SocketAddr,
    event_ack: oneshot::Receiver<()>,
    peers_ready: oneshot::Receiver<()>,
    ptp_clock_id: Option<u64>,
}
#[derive(Default, Debug)]
struct Report {
    auth_failures: usize,
    digest_verified: bool,
    event_ack: bool,
    volumes: Vec<String>,
}
struct Receiver {
    device: Device,
    data: UdpSocket,
    control: UdpSocket,
    offer: oneshot::Receiver<AudioOffer>,
    task: JoinHandle<Report>,
}

async fn info(wire: &mut Wire) {
    let message = request(wire).await;
    assert!(message.0.starts_with("GET /info "));
    reply(
        wire,
        &message,
        200,
        &plist([
            ("model", Value::String("AirloomTestReceiver".into())),
            ("features", integer((1u64 << 38) | (1u64 << 48))),
        ]),
        "",
    )
    .await;
}

async fn start_receiver(password: &str, require_digest: bool, max_attempts: usize) -> Receiver {
    start_receiver_on(
        "127.0.0.1".parse().unwrap(),
        password,
        require_digest,
        max_attempts,
    )
    .await
}
async fn start_receiver_on(
    local: IpAddr,
    password: &str,
    require_digest: bool,
    max_attempts: usize,
) -> Receiver {
    start_receiver_with_info(local, password, require_digest, max_attempts, None).await
}
async fn start_receiver_with_info(
    local: IpAddr,
    password: &str,
    require_digest: bool,
    max_attempts: usize,
    receiver_info: Option<Vec<u8>>,
) -> Receiver {
    let listener = TcpListener::bind(SocketAddr::new(local, 0)).await.unwrap();
    let events = TcpListener::bind(SocketAddr::new(local, 0)).await.unwrap();
    let data = UdpSocket::bind(SocketAddr::new(local, 0)).await.unwrap();
    let control = UdpSocket::bind(SocketAddr::new(local, 0)).await.unwrap();
    let data_port = data.local_addr().unwrap().port();
    let control_port = control.local_addr().unwrap().port();
    let address = listener.local_addr().unwrap();
    let device = Device::manual(address.ip(), address.port());
    let password = password.to_owned();
    let (offer_tx, offer) = oneshot::channel();
    let task = tokio::spawn(async move {
        let mut report = Report::default();
        for _ in 0..max_attempts {
            let (stream, peer) = timeout(Duration::from_secs(3), listener.accept())
                .await
                .unwrap()
                .unwrap();
            let mut wire = Wire::new(stream);
            if let Some(body) = &receiver_info {
                let message = request(&mut wire).await;
                assert!(message.0.starts_with("GET /info "));
                reply(&mut wire, &message, 200, body, "").await;
            } else {
                info(&mut wire).await;
            }
            let Some(shared) = server_srp(&mut wire, &password, true).await else {
                report.auth_failures += 1;
                continue;
            };
            wire.enable_encryption(
                crypto::derive(&shared, b"Control-Salt", b"Control-Write-Encryption-Key"),
                crypto::derive(&shared, b"Control-Salt", b"Control-Read-Encryption-Key"),
            );
            return serve_media(
                wire,
                shared,
                events,
                peer,
                data_port,
                control_port,
                offer_tx,
                require_digest,
                &password,
                report,
            )
            .await;
        }
        report
    });
    Receiver {
        device,
        data,
        control,
        offer,
        task,
    }
}

#[allow(clippy::too_many_arguments)]
async fn serve_media(
    mut wire: Wire,
    shared: Vec<u8>,
    events: TcpListener,
    peer: SocketAddr,
    data_port: u16,
    control_port: u16,
    offer: oneshot::Sender<AudioOffer>,
    require_digest: bool,
    password: &str,
    mut report: Report,
) -> Report {
    let mut session = request(&mut wire).await;
    assert!(session.0.starts_with("SETUP "));
    if require_digest {
        reply(
            &mut wire,
            &session,
            401,
            &[],
            "WWW-Authenticate: Digest realm=\"raop\", nonce=\"fixture-nonce\"\r\n",
        )
        .await;
        session = request(&mut wire).await;
        let path = session.0.split_whitespace().nth(1).unwrap();
        let hash = |data: &str| hex::encode(Md5::digest(data.as_bytes()));
        let expected = hash(&format!(
            "{}:fixture-nonce:{}",
            hash(&format!("iTunes:raop:{password}")),
            hash(&format!("SETUP:{path}"))
        ));
        assert!(session.1["authorization"].contains(&format!("response=\"{expected}\"")));
        report.digest_verified = true;
    }
    let setup = decode(&session.2);
    let dictionary = setup.as_dictionary().unwrap();
    // Independent wire contract: deviceID is 64-bit, macAddress is 48-bit.
    assert_eq!(
        dictionary["deviceID"]
            .as_string()
            .unwrap()
            .split(':')
            .count(),
        8
    );
    assert_eq!(
        dictionary["macAddress"]
            .as_string()
            .unwrap()
            .split(':')
            .count(),
        6
    );
    let ptp_clock_id = match dictionary["timingProtocol"].as_string().unwrap() {
        "PTP" => {
            assert!(!dictionary.contains_key("timingPort"));
            assert_eq!(
                dictionary["timingPeerList"].as_array().unwrap()[0],
                dictionary["timingPeerInfo"]
            );
            let info = dictionary["timingPeerInfo"].as_dictionary().unwrap();
            Some(info["ClockID"].as_signed_integer().unwrap() as u64)
        }
        "NTP" => {
            assert!(!dictionary.contains_key("timingPeerInfo"));
            assert!(!dictionary.contains_key("timingPeerList"));
            None
        }
        _ => panic!("unknown timing protocol"),
    };
    let timing_port = if ptp_clock_id.is_some() {
        319
    } else {
        dictionary["timingPort"].as_unsigned_integer().unwrap() as u16
    };
    let timing = SocketAddr::new(peer.ip(), timing_port);
    reply(
        &mut wire,
        &session,
        200,
        &plist([(
            "eventPort",
            integer(events.local_addr().unwrap().port() as u64),
        )]),
        "Session: test-session\r\n",
    )
    .await;
    let (close_event_tx, close_event) = oneshot::channel();
    let (event_ack_tx, event_ack) = oneshot::channel();
    let event_task = tokio::spawn(async move {
        let (stream, _) = events.accept().await.unwrap();
        let mut event = Wire::encrypted(
            stream,
            crypto::derive(&shared, b"Events-Salt", b"Events-Read-Encryption-Key"),
            crypto::derive(&shared, b"Events-Salt", b"Events-Write-Encryption-Key"),
        );
        // pyatv's EventChannel and the independently implemented MA receiver
        // command channel preserve the request's protocol and delimit an empty
        // reply. Exercise both forms, not just the sender's historical RTSP path.
        for (protocol, sequence) in [("RTSP/1.0", 77), ("HTTP/1.1", 78)] {
            event
                .write(
                    format!(
                        "POST /event {protocol}\r\nCSeq: {sequence}\r\nContent-Length: 0\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            let ack = request(&mut event).await;
            assert_eq!(ack.0, format!("{protocol} 200 OK"));
            assert_eq!(ack.1["cseq"], sequence.to_string());
            assert_eq!(ack.1["content-length"], "0");
            assert_eq!(ack.1["audio-latency"], "0");
            assert!(ack.2.is_empty());
        }
        event_ack_tx.send(()).ok();
        let _ = close_event.await;
        true
    });
    let record = request(&mut wire).await;
    assert!(record.0.starts_with("RECORD "));
    reply(&mut wire, &record, 200, &[], "").await;
    let stream = request(&mut wire).await;
    assert!(stream.0.starts_with("SETUP "));
    let value = decode(&stream.2);
    let audio = value.as_dictionary().unwrap()["streams"]
        .as_array()
        .unwrap()[0]
        .as_dictionary()
        .unwrap();
    assert_eq!(audio["type"].as_unsigned_integer(), Some(96));
    assert_eq!(audio["audioFormat"].as_unsigned_integer(), Some(0x40000));
    let key: [u8; 32] = audio["shk"].as_data().unwrap().try_into().unwrap();
    let control = SocketAddr::new(
        peer.ip(),
        audio["controlPort"].as_unsigned_integer().unwrap() as u16,
    );
    // Reverse key order deliberately; the client must parse by key, not position.
    let reply_audio: Dictionary = [
        ("controlPort", integer(control_port as u64)),
        ("dataPort", integer(data_port as u64)),
        ("latencyMin", integer(11025)),
        ("latencyMax", integer(88200)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_owned(), v))
    .collect();
    reply(
        &mut wire,
        &stream,
        200,
        &plist([(
            "streams",
            Value::Array(vec![Value::Dictionary(reply_audio)]),
        )]),
        "",
    )
    .await;
    let (peers_ready, peers_observed) = oneshot::channel();
    let mut peers_ready = Some(peers_ready);
    offer
        .send(AudioOffer {
            key,
            control,
            timing,
            event_ack,
            peers_ready: peers_observed,
            ptp_clock_id,
        })
        .ok();
    loop {
        let message = request(&mut wire).await;
        if message.0.starts_with("SET_PARAMETER ") {
            report
                .volumes
                .push(String::from_utf8(message.2.clone()).unwrap());
        }
        let teardown = message.0.starts_with("TEARDOWN ");
        let body = if message.0.starts_with("POST /feedback ") {
            // An empty receiver stream list is legal even during playback.
            plist([("streams", Value::Array(Vec::new()))])
        } else {
            Vec::new()
        };
        reply(&mut wire, &message, 200, &body, "").await;
        if message.0.starts_with("SETPEERS ") {
            if let Some(ready) = peers_ready.take() {
                ready.send(()).ok();
            }
        }
        if teardown {
            break;
        }
    }
    close_event_tx.send(()).ok();
    report.event_ack = event_task.await.unwrap();
    report
}

#[tokio::test]
async fn password_digest_encrypted_stream_events_ntp_and_retransmission() {
    let receiver = start_receiver(PASSWORD, true, 1).await;
    let mut sender = Sender::connect(receiver.device, options(PASSWORD))
        .await
        .unwrap();
    assert_eq!(sender.volume(), None);
    let offer = receiver.offer.await.unwrap();
    offer.event_ack.await.unwrap();
    let timing = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let mut query = [0u8; 32];
    query[0] = 0x80;
    query[1] = 0xd2;
    query[3] = 15;
    query[24..].copy_from_slice(&0x123456789abcdef0u64.to_be_bytes());
    timing.send_to(&query, offer.timing).await.unwrap();
    let mut buffer = [0u8; 2048];
    let (size, _) = timeout(Duration::from_secs(1), timing.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(size, 32);
    assert_eq!(&buffer[8..16], &query[24..32]);
    assert_eq!(buffer[1], 0xd3);
    let pcm: Vec<i16> = (0..FRAMES_PER_PACKET * CHANNELS * 3)
        .map(|i| (i as i16).wrapping_mul(13))
        .collect();
    sender.send_pcm(&pcm).await.unwrap();
    let mut packets = Vec::new();
    for i in 0..3 {
        let (size, _) = timeout(Duration::from_secs(1), receiver.data.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        let packet = buffer[..size].to_vec();
        let nonce: [u8; 8] = packet[size - 8..].try_into().unwrap();
        let clear = crypto::open(
            &offer.key,
            &crypto::named_nonce(&nonce),
            &packet[4..12],
            &packet[12..size - 8],
        )
        .unwrap();
        assert_eq!(
            clear,
            audio::alac_packet(&pcm[i * 704..(i + 1) * 704]).unwrap()
        );
        packets.push(packet);
    }
    let first_seq = u16::from_be_bytes(packets[0][2..4].try_into().unwrap());
    assert_eq!(
        u16::from_be_bytes(packets[2][2..4].try_into().unwrap()),
        first_seq.wrapping_add(2)
    );
    let (size, _) = timeout(
        Duration::from_secs(1),
        receiver.control.recv_from(&mut buffer),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(size, 20);
    assert_eq!(&buffer[..2], &[0x90, 0xd4]);
    let mut resend = vec![0x80, 0xd5, 0, 25];
    resend.extend_from_slice(&first_seq.to_be_bytes());
    resend.extend_from_slice(&1u16.to_be_bytes());
    receiver
        .control
        .send_to(&resend, offer.control)
        .await
        .unwrap();
    let (size, _) = timeout(
        Duration::from_secs(1),
        receiver.control.recv_from(&mut buffer),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(&buffer[..4], &[0x80, 0xd6, 0, 25]);
    assert_eq!(&buffer[4..size], packets[0]);
    sender.volume_control().set_volume(0.6).await.unwrap();
    assert_eq!(sender.volume(), Some(0.6));
    assert_eq!(sender.volume_control().volume(), Some(0.6));
    let metrics = sender.metrics();
    assert_eq!(metrics.packets_sent, 3);
    assert_eq!(metrics.retransmissions_sent, 1);
    assert_eq!(metrics.timing_exchanges, 1);
    let diagnostic = sender.diagnostics();
    assert_eq!(diagnostic.transport.non_silent_audio_packets_sent, 3);
    assert_eq!(diagnostic.transport.sync_packets_sent, 1);
    assert_eq!(diagnostic.transport.control_packets_received, 1);
    assert_eq!(diagnostic.transport.event_requests, 2);
    assert_eq!(diagnostic.transport.event_responses, 2);
    assert_eq!(diagnostic.stream.receiver_latency_min_frames, Some(11025));
    assert_eq!(diagnostic.stream.receiver_latency_max_frames, Some(88200));
    assert!(diagnostic.transport.last_audio_peak_dbfs.unwrap() > -10.0);
    timeout(Duration::from_secs(3), async {
        while sender.diagnostics().transport.feedback_responses == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let diagnostic = sender.diagnostics();
    assert_eq!(diagnostic.transport.feedback_last_status, Some(200));
    assert_eq!(diagnostic.transport.feedback_reported_streams, Some(0));
    assert!(diagnostic.transport.feedback_last_response_age_ms.unwrap() < 1000);
    assert!(diagnostic.last_failure_stage.is_none());
    sender.stop().await.unwrap();
    let report = receiver.task.await.unwrap();
    assert!(report.digest_verified && report.event_ack);
    assert_eq!(report.volumes, ["volume: -11.999999\r\n"]);
}

#[tokio::test]
async fn connect_reads_receiver_volume_without_changing_it() {
    for (reported, expected) in [
        (Some(Value::Real(-9.0)), Some(0.7)),
        (Some(Value::Integer((-15i64).into())), Some(0.5)),
        (Some(Value::Real(-144.0)), Some(0.0)),
        (None, None),
        (Some(Value::String("invalid".into())), None),
    ] {
        let mut fields = vec![
            ("model", Value::String("AirloomTestReceiver".into())),
            ("features", integer((1u64 << 38) | (1u64 << 48))),
        ];
        if let Some(volume) = reported {
            fields.push(("initialVolume", volume));
        }
        let receiver = start_receiver_with_info(
            "127.0.0.1".parse().unwrap(),
            PASSWORD,
            false,
            1,
            Some(plist(fields)),
        )
        .await;
        let mut sender = Sender::connect(receiver.device, options(PASSWORD))
            .await
            .unwrap();
        assert_eq!(sender.volume(), expected);
        receiver.offer.await.unwrap().event_ack.await.unwrap();

        // Missing or malformed optional volume must still allow real media.
        sender.send_pcm(&[1234; 704]).await.unwrap();
        let mut packet = [0; 2048];
        let (size, _) = timeout(Duration::from_secs(1), receiver.data.recv_from(&mut packet))
            .await
            .unwrap()
            .unwrap();
        assert!(size > 12);
        sender.stop().await.unwrap();
        let report = receiver.task.await.unwrap();
        assert!(report.volumes.is_empty());
    }
}

#[tokio::test]
async fn explicit_connect_volume_is_the_only_initial_write() {
    let receiver = start_receiver(PASSWORD, false, 1).await;
    let mut connect = options(PASSWORD);
    connect.initial_volume = Some(0.5);
    let mut sender = Sender::connect(receiver.device, connect).await.unwrap();
    assert_eq!(sender.volume(), Some(0.5));
    receiver.offer.await.unwrap().event_ack.await.unwrap();
    sender.stop().await.unwrap();
    assert_eq!(
        receiver.task.await.unwrap().volumes,
        ["volume: -15.000000\r\n"]
    );
}

fn decode_uncompressed_stereo_frame(payload: &[u8]) -> Vec<i16> {
    // Read the published ALAC element layout independently of the sender's
    // bit writer. The optional FFmpeg test also decodes its encrypted RTP path.
    let mut position = 0usize;
    let mut read = |width: usize| {
        let mut value = 0u32;
        for _ in 0..width {
            value = (value << 1) | ((payload[position / 8] >> (7 - position % 8)) & 1) as u32;
            position += 1;
        }
        value
    };
    assert_eq!(read(3), 1); // stereo channel-pair element
    assert_eq!(read(4), 0);
    assert_eq!(read(12), 0);
    assert_eq!(read(1), 0); // negotiated 352-frame size
    assert_eq!(read(2), 0);
    assert_eq!(read(1), 1); // uncompressed
    let samples = (0..704).map(|_| read(16) as u16 as i16).collect();
    assert_eq!(read(3), 7); // end element
    samples
}

#[tokio::test]
async fn ptp_encrypted_nonzero_audio_has_a_valid_receiver_playback_anchor() {
    use std::sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    };
    let remote: IpAddr = "127.0.0.20".parse().unwrap();
    let event = UdpSocket::bind(SocketAddr::new(remote, 319)).await.unwrap();
    let general = UdpSocket::bind(SocketAddr::new(remote, 320)).await.unwrap();
    let observed = Arc::new(AtomicU64::new(0));
    let observed_clock = observed.clone();
    let observed_time = Arc::new(Mutex::new(None));
    let clock_time = observed_time.clone();
    let clock_receiver = tokio::spawn(async move {
        let mut event_buffer = [0; 2048];
        let mut general_buffer = [0; 2048];
        let mut pending_sync = BTreeMap::new();
        let mut pending_followup = BTreeMap::new();
        let mut announced_clock = None;
        loop {
            tokio::select! {
                received = event.recv_from(&mut event_buffer) => {
                    let (size, address) = received.unwrap();
                    if event_buffer[0] & 15 != 0 { continue; }
                    assert_eq!(size, 44);
                    assert_eq!(&event_buffer[..2], &[0x10, 2]);
                    assert_eq!(&event_buffer[6..8], &[0x06, 0x08]);
                    assert_eq!(&event_buffer[28..30], &[0x80, 0x05]);
                    assert_eq!(event_buffer[32], 0);
                    let identity = u64::from_be_bytes(event_buffer[20..28].try_into().unwrap());
                    observed_clock.store(identity, Ordering::Release);
                    pending_sync.insert(u16::from_be_bytes(event_buffer[30..32].try_into().unwrap()), (address, identity));
                }
                received = general.recv_from(&mut general_buffer) => {
                    let (size, _) = received.unwrap();
                    assert!(size >= 34);
                    assert_eq!(u16::from_be_bytes([general_buffer[2], general_buffer[3]]) as usize, size);
                    match general_buffer[0] & 15 {
                        11 => {
                            // IEEE1588 PATH_TRACE carries the active grandmaster;
                            // Apple-profile senders include it in their Announce.
                            assert_eq!(size, 76);
                            assert_eq!(&general_buffer[..2], &[0x1b, 2]);
                            assert_eq!(&general_buffer[6..8], &[0x04, 0x08]);
                            assert_eq!(&general_buffer[28..30], &[0x80, 0x05]);
                            assert_eq!(&general_buffer[64..68], &[0, 8, 0, 8]);
                            assert_eq!(&general_buffer[68..76], &general_buffer[53..61]);
                            announced_clock = Some(u64::from_be_bytes(general_buffer[53..61].try_into().unwrap()));
                        }
                        8 => {
                            // Independent receive contract from nqptp's IEEE802.1AS
                            // Table11-11 parser and Apple clock-ID offset86. Primary:
                            // music-assistant/airplay-cli@1701bc25e9d303b42532569f624b64c0d0afb1f6
                            // src/ap2_ptp.c; mikebrady/nqptp/nqptp-message-handlers.c.
                            assert_eq!(size, 96, "bare IEEE1588 FollowUp is insufficient for this profile");
                            assert_eq!(&general_buffer[..2], &[0x18, 2]);
                            assert_eq!(&general_buffer[6..8], &[0x04, 0x08]);
                            assert_eq!(&general_buffer[28..30], &[0x80, 0x05]);
                            assert_eq!(&general_buffer[44..54], &[0, 3, 0, 28, 0, 0x80, 0xc2, 0, 0, 1]);
                            assert_eq!(&general_buffer[76..86], &[0, 3, 0, 16, 0, 0x0d, 0x93, 0, 0, 4]);
                            assert_eq!(&general_buffer[86..94], &general_buffer[20..28]);
                            assert_eq!(&general_buffer[94..96], &[0, 0]);
                            let seconds = general_buffer[34..40].iter().fold(0u64, |n, b| (n << 8) | *b as u64);
                            let nanos = u32::from_be_bytes(general_buffer[40..44].try_into().unwrap());
                            assert!(nanos < 1_000_000_000);
                            let sequence = u16::from_be_bytes(general_buffer[30..32].try_into().unwrap());
                            let identity = u64::from_be_bytes(general_buffer[20..28].try_into().unwrap());
                            pending_followup.insert(sequence, (seconds * 1_000_000_000 + nanos as u64, std::time::Instant::now(), identity));
                        }
                        _ => {}
                    }
                }
            }
            // UDP319 and UDP320 are independent receive queues. Pair by sequence
            // instead of assuming select! observes those queues in wire order.
            let matched: Vec<_> = pending_sync
                .keys()
                .filter(|sequence| pending_followup.contains_key(sequence))
                .copied()
                .collect();
            for sequence in matched {
                let (address, identity) = pending_sync.remove(&sequence).unwrap();
                let (nanos, received_at, follow_identity) =
                    pending_followup.remove(&sequence).unwrap();
                assert_eq!(identity, follow_identity);
                if announced_clock != Some(identity) {
                    continue;
                }
                *clock_time.lock().unwrap() = Some((nanos, received_at));
                // Only accept the clock after complete matching messages.
                let mut delay = [0u8; 44];
                delay[0] = 0x11;
                delay[1] = 2;
                delay[2..4].copy_from_slice(&44u16.to_be_bytes());
                delay[6..8].copy_from_slice(&0x0408u16.to_be_bytes());
                delay[20..28].copy_from_slice(&0x8899aabbccddeeffu64.to_be_bytes());
                delay[28..30].copy_from_slice(&0x8005u16.to_be_bytes());
                delay[30..32].copy_from_slice(&sequence.to_be_bytes());
                event.send_to(&delay, address).await.unwrap();
            }
            while pending_sync.len() > 16 {
                pending_sync.pop_first();
            }
            while pending_followup.len() > 16 {
                pending_followup.pop_first();
            }
        }
    });
    let receiver = start_receiver_on(remote, PASSWORD, false, 1).await;
    let mut config = options(PASSWORD);
    config.timing = TimingMode::Ptp;
    let mut sender = Sender::connect(receiver.device, config).await.unwrap();
    let offer = receiver.offer.await.unwrap();
    offer.event_ack.await.unwrap();
    let clock_id = offer.ptp_clock_id.expect("PTP SETUP required");
    assert_eq!(
        clock_id,
        observed.load(Ordering::Acquire),
        "SETUP must advertise the active timing authority"
    );
    let pcm: Vec<i16> = (0..704 * 3)
        .map(|i| (i * 197 % 60001 - 30000) as i16)
        .collect();
    sender.send_pcm(&pcm).await.unwrap();
    let mut buffer = [0u8; 2048];
    let mut first_timestamp: Option<u32> = None;
    for frame in 0..3 {
        let (size, _) = timeout(Duration::from_secs(1), receiver.data.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(buffer[1] & 127, 96);
        assert_eq!(&buffer[8..12], &[0; 4]); // PTP realtime SSRC
        let timestamp = u32::from_be_bytes(buffer[4..8].try_into().unwrap());
        if let Some(first) = first_timestamp {
            assert_eq!(timestamp, first.wrapping_add(frame as u32 * 352));
        } else {
            first_timestamp = Some(timestamp);
        }
        let suffix: [u8; 8] = buffer[size - 8..size].try_into().unwrap();
        let plain = crypto::open(
            &offer.key,
            &crypto::named_nonce(&suffix),
            &buffer[4..12],
            &buffer[12..size - 8],
        )
        .unwrap();
        let decoded = decode_uncompressed_stereo_frame(&plain);
        assert_eq!(decoded, pcm[frame * 704..(frame + 1) * 704]);
        assert!(decoded.iter().any(|sample| sample.unsigned_abs() > 10000));
    }
    let (size, _) = timeout(
        Duration::from_secs(1),
        receiver.control.recv_from(&mut buffer),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(size, 28);
    assert_eq!(&buffer[..4], &[0x90, 0xd7, 0, 6]);
    let anchor_ns = u64::from_be_bytes(buffer[8..16].try_into().unwrap());
    let (clock_ns, observed_at) = observed_time.lock().unwrap().unwrap();
    let expected_ns = clock_ns + observed_at.elapsed().as_nanos() as u64;
    assert!(
        anchor_ns.abs_diff(expected_ns) < 500_000_000,
        "audio anchor must use the clock actually accepted by the receiver"
    );
    assert_eq!(
        u64::from_be_bytes(buffer[20..28].try_into().unwrap()),
        clock_id
    );
    let frame_1 = u32::from_be_bytes(buffer[4..8].try_into().unwrap());
    let frame_2 = u32::from_be_bytes(buffer[16..20].try_into().unwrap());
    assert_eq!(frame_2.wrapping_sub(frame_1), 77175);
    let rendering = frame_1.wrapping_sub(11025);
    let lead = first_timestamp.unwrap().wrapping_sub(rendering);
    assert!(
        (32723..=33075).contains(&lead),
        "first PCM frame must be scheduled about 750ms after the anchor, got {lead}"
    );
    assert_eq!(sender.metrics().packets_sent, 3);
    assert!(sender.metrics().timing_exchanges >= 3);
    sender.stop().await.unwrap();
    let report = receiver.task.await.unwrap();
    assert!(report.event_ack);
    clock_receiver.abort();
    let _ = clock_receiver.await;
    // Keep both real sessions on the same runtime, as the application's shared
    // timing hub does; separate #[tokio::test] runtimes would share127.0.0.1
    // until the earlier runtime destroyed its sockets underneath the other.
    followed_receiver_starts_its_clock_only_after_session_and_peers_are_established().await;
    native_stereo_members_share_full_pcm_and_one_render_timeline().await;
}

async fn native_stereo_members_share_full_pcm_and_one_render_timeline() {
    async fn member(ip: &str) -> (Receiver, JoinHandle<()>) {
        let ip: IpAddr = ip.parse().unwrap();
        let event = UdpSocket::bind(SocketAddr::new(ip, 319)).await.unwrap();
        let general = UdpSocket::bind(SocketAddr::new(ip, 320)).await.unwrap();
        let timing_task = tokio::spawn(async move {
            let mut event_buffer = [0u8; 2048];
            let mut general_buffer = [0u8; 2048];
            loop {
                tokio::select! {
                    received = event.recv_from(&mut event_buffer) => {
                        let (size, source) = received.unwrap();
                        if size < 44 || event_buffer[0] & 15 != 0 { continue; }
                        // The full PTP wire profile is checked by the enclosing
                        // test. This peer replies to each Sync with Delay_Req.
                        let mut delay = [0u8; 44];
                        delay[0] = 0x11;
                        delay[1] = 2;
                        delay[2..4].copy_from_slice(&44u16.to_be_bytes());
                        delay[6..8].copy_from_slice(&0x0408u16.to_be_bytes());
                        delay[20..28].copy_from_slice(&0x0102_0304_0506_0708u64.to_be_bytes());
                        delay[28..30].copy_from_slice(&0x8005u16.to_be_bytes());
                        delay[30..32].copy_from_slice(&event_buffer[30..32]);
                        event.send_to(&delay, source).await.unwrap();
                    }
                    received = general.recv_from(&mut general_buffer) => { received.unwrap(); }
                }
            }
        });
        let mut receiver = start_receiver_with_info(
            ip,
            PASSWORD,
            false,
            1,
            Some(plist([
                ("model", Value::String("AudioAccessory5,1".into())),
                ("features", integer((1u64 << 41) | (1u64 << 48))),
            ])),
        )
        .await;
        receiver
            .device
            .txt
            .insert("tsid".into(), "EAFA36AA-9785-54B2-A537-D9EE2A55CF1C".into());
        (receiver, timing_task)
    }
    let (one, timing_one) = member("127.0.0.24").await;
    let (two, timing_two) = member("127.0.0.25").await;
    let mut config_one = options(PASSWORD);
    config_one.timing = TimingMode::Ptp;
    config_one.latency = Duration::from_millis(500);
    let mut config_two = options(PASSWORD);
    config_two.timing = TimingMode::Ptp;
    config_two.latency = Duration::from_millis(900);
    let (sender_one, sender_two) = tokio::join!(
        Sender::connect(one.device, config_one),
        Sender::connect(two.device, config_two),
    );
    let mut sender_one = sender_one.unwrap();
    let mut sender_two = sender_two.unwrap();
    let offer_one = one.offer.await.unwrap();
    let offer_two = two.offer.await.unwrap();
    offer_one.event_ack.await.unwrap();
    offer_two.event_ack.await.unwrap();
    Sender::prepare_native_group(&mut [&mut sender_one, &mut sender_two]).unwrap();
    assert_eq!(sender_one.metrics().negotiated_latency_ms, 900);
    assert_eq!(sender_two.metrics().negotiated_latency_ms, 900);
    assert!(Sender::prepare_native_group(&mut [&mut sender_one, &mut sender_two]).is_err());
    let pcm: Vec<i16> = (0..FRAMES_PER_PACKET * 2)
        .flat_map(|frame| [frame as i16 * 17, -(frame as i16) * 31])
        .collect();
    let (sent_one, sent_two) = tokio::join!(sender_one.send_pcm(&pcm), async {
        // Unequal task scheduling must not move the second member's first frame.
        tokio::time::sleep(Duration::from_millis(20)).await;
        sender_two.send_pcm(&pcm).await
    });
    sent_one.unwrap();
    sent_two.unwrap();
    async fn received_audio(data: &UdpSocket, key: &[u8; 32], expected: &[i16]) -> u32 {
        let mut buffer = [0u8; 2048];
        let mut first = None;
        for (index, chunk) in expected.chunks(FRAMES_PER_PACKET * CHANNELS).enumerate() {
            let (size, _) = timeout(Duration::from_secs(1), data.recv_from(&mut buffer))
                .await
                .unwrap()
                .unwrap();
            let timestamp = u32::from_be_bytes(buffer[4..8].try_into().unwrap());
            let origin = *first.get_or_insert(timestamp);
            assert_eq!(
                timestamp,
                origin.wrapping_add(index as u32 * FRAMES_PER_PACKET as u32)
            );
            let nonce: [u8; 8] = buffer[size - 8..size].try_into().unwrap();
            let payload = crypto::open(
                key,
                &crypto::named_nonce(&nonce),
                &buffer[4..12],
                &buffer[12..size - 8],
            )
            .unwrap();
            assert_eq!(
                decode_uncompressed_stereo_frame(&payload),
                chunk,
                "each member receives both original channels"
            );
        }
        first.unwrap()
    }
    let first_one = received_audio(&one.data, &offer_one.key, &pcm).await;
    let first_two = received_audio(&two.data, &offer_two.key, &pcm).await;
    assert_eq!(
        first_one, first_two,
        "the two first PCM frames share an RTP position"
    );
    async fn first_render_time(control: &UdpSocket, timestamp: u32) -> (u64, u64) {
        let mut buffer = [0u8; 2048];
        let (size, _) = timeout(Duration::from_secs(1), control.recv_from(&mut buffer))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(size, 28);
        let rendering = u32::from_be_bytes(buffer[4..8].try_into().unwrap()).wrapping_sub(11025);
        let lead = timestamp.wrapping_sub(rendering) as u64;
        let master_ns = u64::from_be_bytes(buffer[8..16].try_into().unwrap());
        (
            master_ns + lead * 1_000_000_000 / SAMPLE_RATE as u64,
            u64::from_be_bytes(buffer[20..28].try_into().unwrap()),
        )
    }
    let (render_one, clock_one) = first_render_time(&one.control, first_one).await;
    let (render_two, clock_two) = first_render_time(&two.control, first_two).await;
    assert_eq!(clock_one, clock_two);
    assert!(
        render_one.abs_diff(render_two) <= 50_000,
        "wire mappings must agree within sample rounding"
    );
    tokio::time::sleep(Duration::from_millis(250)).await;
    assert!(matches!(
        sender_one.send_pcm(&pcm).await,
        Err(Error::SessionLost(_))
    ));
    assert!(matches!(
        sender_two.send_pcm(&pcm).await,
        Err(Error::SessionLost(_))
    ));
    assert_eq!(
        sender_one.metrics().reanchors,
        0,
        "a group member must not independently drop/reanchor audio"
    );
    sender_one.stop().await.unwrap();
    sender_two.stop().await.unwrap();
    assert!(one.task.await.unwrap().event_ack);
    assert!(two.task.await.unwrap().event_ack);
    timing_one.abort();
    timing_two.abort();
    let _ = tokio::join!(timing_one, timing_two);
}

async fn followed_receiver_starts_its_clock_only_after_session_and_peers_are_established() {
    // MA's hardware report (DESIGN.md at commit
    // 1701bc25e9d303b42532569f624b64c0d0afb1f6, "Followed receivers")
    // explicitly permits SETUP naming our clock before the HomePod announces.
    // The receiver below refuses to emit any clock until SETPEERS has arrived.
    const RECEIVER_CLOCK: u64 = 0x8877_6655_4433_2211;
    const RECEIVER_EPOCH_NS: u64 = 10_000_000_000_000;
    let remote: IpAddr = "127.0.0.23".parse().unwrap();
    let event = UdpSocket::bind(SocketAddr::new(remote, 319)).await.unwrap();
    let general = UdpSocket::bind(SocketAddr::new(remote, 320)).await.unwrap();
    let Receiver {
        mut device,
        data,
        control,
        offer,
        task,
    } = start_receiver_with_info(
        remote,
        PASSWORD,
        false,
        1,
        Some(plist([
            ("model", Value::String("AudioAccessory5,1".into())),
            ("osVersion", Value::String("27.0".into())),
            ("sourceVersion", Value::String("980.2".into())),
            ("features", integer((1u64 << 41) | (1u64 << 48))),
        ])),
    )
    .await;
    // The discovered record intentionally omits model and OS version. The
    // runtime /info response must complete clock-role selection.
    device.txt.insert("igl".into(), "1".into());
    let (accepted, negotiated) = oneshot::channel();
    let clock_start = std::time::Instant::now();
    let clock_receiver = tokio::spawn(async move {
        let offer = offer.await.unwrap();
        offer.peers_ready.await.unwrap();
        offer.event_ack.await.unwrap();
        accepted
            .send((offer.key, offer.ptp_clock_id.unwrap()))
            .unwrap();
        let destination = offer.timing.ip();
        let mut ticks = tokio::time::interval(Duration::from_millis(125));
        for sequence in 1..=u16::MAX {
            ticks.tick().await;
            let header = |kind: u8, size: usize, flags: u16| {
                let mut packet = vec![0; size];
                packet[0] = 0x10 | kind;
                packet[1] = 2;
                packet[2..4].copy_from_slice(&(size as u16).to_be_bytes());
                packet[6..8].copy_from_slice(&flags.to_be_bytes());
                packet[20..28].copy_from_slice(&RECEIVER_CLOCK.to_be_bytes());
                packet[28..30].copy_from_slice(&0x8005u16.to_be_bytes());
                packet[30..32].copy_from_slice(&sequence.to_be_bytes());
                packet[33] = (-3i8) as u8;
                packet
            };
            if sequence % 8 == 1 {
                let mut announce = header(11, 76, 0x0408);
                announce[33] = 0;
                announce[47] = 248;
                announce[48] = 248;
                announce[49] = 0xfe;
                announce[50..52].copy_from_slice(&0xffffu16.to_be_bytes());
                announce[52] = 248;
                announce[53..61].copy_from_slice(&RECEIVER_CLOCK.to_be_bytes());
                announce[63] = 0xa0;
                announce[64..68].copy_from_slice(&[0, 8, 0, 8]);
                announce[68..76].copy_from_slice(&RECEIVER_CLOCK.to_be_bytes());
                general
                    .send_to(&announce, SocketAddr::new(destination, 320))
                    .await
                    .unwrap();
            }
            let sync = header(0, 44, 0x0608);
            event
                .send_to(&sync, SocketAddr::new(destination, 319))
                .await
                .unwrap();
            let mut follow = header(8, 96, 0x0408);
            let now = RECEIVER_EPOCH_NS + clock_start.elapsed().as_nanos() as u64;
            follow[34..40].copy_from_slice(&(now / 1_000_000_000).to_be_bytes()[2..]);
            follow[40..44].copy_from_slice(&((now % 1_000_000_000) as u32).to_be_bytes());
            follow[44..54].copy_from_slice(&[0, 3, 0, 28, 0, 0x80, 0xc2, 0, 0, 1]);
            follow[76..86].copy_from_slice(&[0, 3, 0, 16, 0, 0x0d, 0x93, 0, 0, 4]);
            follow[86..94].copy_from_slice(&RECEIVER_CLOCK.to_be_bytes());
            general
                .send_to(&follow, SocketAddr::new(destination, 320))
                .await
                .unwrap();
        }
    });
    let mut config = options(PASSWORD);
    config.timing = TimingMode::Auto;
    let mut sender = timeout(Duration::from_secs(6), Sender::connect(device, config))
        .await
        .unwrap()
        .unwrap();
    let (key, setup_clock) = negotiated.await.unwrap();
    assert_ne!(
        setup_clock, RECEIVER_CLOCK,
        "receiver has not announced during SETUP"
    );
    let pcm = vec![12345; FRAMES_PER_PACKET * CHANNELS];
    sender.send_pcm(&pcm).await.unwrap();
    let mut buffer = [0u8; 2048];
    let (size, _) = timeout(Duration::from_secs(1), data.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    let nonce: [u8; 8] = buffer[size - 8..size].try_into().unwrap();
    let plain = crypto::open(
        &key,
        &crypto::named_nonce(&nonce),
        &buffer[4..12],
        &buffer[12..size - 8],
    )
    .unwrap();
    assert_eq!(decode_uncompressed_stereo_frame(&plain), pcm);
    let (size, _) = timeout(Duration::from_secs(1), control.recv_from(&mut buffer))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(size, 28);
    assert_eq!(
        u64::from_be_bytes(buffer[20..28].try_into().unwrap()),
        RECEIVER_CLOCK,
        "first audio announce must use the subsequently learned receiver clock"
    );
    let anchor_ns = u64::from_be_bytes(buffer[8..16].try_into().unwrap());
    let expected_ns = RECEIVER_EPOCH_NS + clock_start.elapsed().as_nanos() as u64;
    assert!(anchor_ns.abs_diff(expected_ns) < 100_000_000);
    let diagnostic = sender.diagnostics();
    assert_eq!(diagnostic.timing.role, "receiver");
    assert!(diagnostic.timing.ready);
    assert!(diagnostic.timing.incoming_sync >= 3);
    assert_eq!(diagnostic.receiver.os_version.as_deref(), Some("27.0"));
    sender.stop().await.unwrap();
    assert!(task.await.unwrap().event_ack);
    clock_receiver.abort();
    let _ = clock_receiver.await;
}

#[tokio::test]
async fn wrong_password_fails_both_bounded_attempts_without_audio_setup() {
    let receiver = start_receiver(PASSWORD, false, 2).await;
    let result = Sender::connect(receiver.device, options("incorrect")).await;
    assert!(matches!(
        result,
        Err(Error::AuthenticationRejected {
            kind: AuthenticationKind::Password
        })
    ));
    assert_eq!(receiver.task.await.unwrap().auth_failures, 2);
    assert!(receiver.offer.await.is_err());
}

const RECEIVER_ID: &[u8] = b"00:11:22:33:44:55";
struct PairedClient {
    id: Vec<u8>,
    public: [u8; 32],
}

async fn persistent_setup(wire: &mut Wire, tamper_signature: bool) -> PairedClient {
    let start = request(wire).await;
    assert!(start.0.starts_with("POST /pair-pin-start "));
    reply(wire, &start, 200, &[], "").await;
    let shared = server_srp(wire, "1234", false).await.unwrap();
    let encryption = crypto::derive(
        &shared,
        b"Pair-Setup-Encrypt-Salt",
        b"Pair-Setup-Encrypt-Info",
    );
    let fifth = request(wire).await;
    let outer = tlv::decode(&fifth.2).unwrap();
    assert_eq!(outer[&tlv::STATE], [5]);
    let payload = crypto::open(
        &encryption,
        &crypto::named_nonce(b"PS-Msg05"),
        &[],
        &outer[&tlv::ENCRYPTED],
    )
    .unwrap();
    let fields = tlv::decode(&payload).unwrap();
    let id = fields[&tlv::IDENTIFIER].clone();
    let public: [u8; 32] = fields[&tlv::PUBLIC_KEY].as_slice().try_into().unwrap();
    assert_eq!(fifth.1["dacp-id"].as_bytes(), id);
    let mut info = crypto::derive(
        &shared,
        b"Pair-Setup-Controller-Sign-Salt",
        b"Pair-Setup-Controller-Sign-Info",
    )
    .to_vec();
    info.extend_from_slice(&id);
    info.extend_from_slice(&public);
    VerifyingKey::from_bytes(&public)
        .unwrap()
        .verify_strict(
            &info,
            &Signature::from_slice(&fields[&tlv::SIGNATURE]).unwrap(),
        )
        .unwrap();
    let signing = SigningKey::from_bytes(&[0x88; 32]);
    let receiver_public = signing.verifying_key().to_bytes();
    let mut info = crypto::derive(
        &shared,
        b"Pair-Setup-Accessory-Sign-Salt",
        b"Pair-Setup-Accessory-Sign-Info",
    )
    .to_vec();
    info.extend_from_slice(RECEIVER_ID);
    info.extend_from_slice(&receiver_public);
    let mut signature = signing.sign(&info).to_bytes();
    if tamper_signature {
        signature[0] ^= 1;
    }
    let inner = tlv::encode(&[
        (tlv::IDENTIFIER, RECEIVER_ID),
        (tlv::PUBLIC_KEY, &receiver_public),
        (tlv::SIGNATURE, &signature),
    ]);
    let ciphertext =
        crypto::seal(&encryption, &crypto::named_nonce(b"PS-Msg06"), &[], &inner).unwrap();
    reply(
        wire,
        &fifth,
        200,
        &tlv::encode(&[(tlv::STATE, &[6]), (tlv::ENCRYPTED, &ciphertext)]),
        "",
    )
    .await;
    PairedClient { id, public }
}

async fn persistent_verify(wire: &mut Wire, client: &PairedClient) -> Vec<u8> {
    let first = request(wire).await;
    assert!(first.0.starts_with("POST /pair-verify "));
    assert_eq!(first.1["dacp-id"].as_bytes(), client.id);
    let fields = tlv::decode(&first.2).unwrap();
    assert_eq!(fields[&tlv::STATE], [1]);
    let remote: [u8; 32] = fields[&tlv::PUBLIC_KEY].as_slice().try_into().unwrap();
    let private = StaticSecret::from([0x66; 32]);
    let public = PublicKey::from(&private).to_bytes();
    let shared = private.diffie_hellman(&PublicKey::from(remote));
    let key = crypto::derive(
        shared.as_bytes(),
        b"Pair-Verify-Encrypt-Salt",
        b"Pair-Verify-Encrypt-Info",
    );
    let signing = SigningKey::from_bytes(&[0x88; 32]);
    let mut info = public.to_vec();
    info.extend_from_slice(RECEIVER_ID);
    info.extend_from_slice(&remote);
    let signature = signing.sign(&info).to_bytes();
    let body = tlv::encode(&[(tlv::IDENTIFIER, RECEIVER_ID), (tlv::SIGNATURE, &signature)]);
    let ciphertext = crypto::seal(&key, &crypto::named_nonce(b"PV-Msg02"), &[], &body).unwrap();
    reply(
        wire,
        &first,
        200,
        &tlv::encode(&[
            (tlv::STATE, &[2]),
            (tlv::PUBLIC_KEY, &public),
            (tlv::ENCRYPTED, &ciphertext),
        ]),
        "",
    )
    .await;
    let third = request(wire).await;
    let fields = tlv::decode(&third.2).unwrap();
    assert_eq!(fields[&tlv::STATE], [3]);
    let clear = crypto::open(
        &key,
        &crypto::named_nonce(b"PV-Msg03"),
        &[],
        &fields[&tlv::ENCRYPTED],
    )
    .unwrap();
    let fields = tlv::decode(&clear).unwrap();
    assert_eq!(fields[&tlv::IDENTIFIER], client.id);
    let mut info = remote.to_vec();
    info.extend_from_slice(&client.id);
    info.extend_from_slice(&public);
    VerifyingKey::from_bytes(&client.public)
        .unwrap()
        .verify_strict(
            &info,
            &Signature::from_slice(&fields[&tlv::SIGNATURE]).unwrap(),
        )
        .unwrap();
    reply(wire, &third, 200, &tlv::encode(&[(tlv::STATE, &[4])]), "").await;
    shared.as_bytes().to_vec()
}

#[tokio::test]
async fn displayed_pin_pairing_credentials_survive_serialization_and_verify() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let events = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let data = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let control = UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let (offer_tx, offer) = oneshot::channel();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut wire = Wire::new(stream);
        let client = persistent_setup(&mut wire, false).await;
        drop(wire);
        let (stream, peer) = listener.accept().await.unwrap();
        let mut wire = Wire::new(stream);
        info(&mut wire).await;
        let shared = persistent_verify(&mut wire, &client).await;
        wire.enable_encryption(
            crypto::derive(&shared, b"Control-Salt", b"Control-Write-Encryption-Key"),
            crypto::derive(&shared, b"Control-Salt", b"Control-Read-Encryption-Key"),
        );
        serve_media(
            wire,
            shared,
            events,
            peer,
            data.local_addr().unwrap().port(),
            control.local_addr().unwrap().port(),
            offer_tx,
            false,
            "",
            Report::default(),
        )
        .await
    });
    let mut device = Device::manual(address.ip(), address.port());
    device.requires_pin = true;
    let pairing = PairingSession::start(&device).await.unwrap();
    let credentials = pairing.finish("1234").await.unwrap();
    assert_eq!(format!("{credentials:?}"), "PairingCredentials([redacted])");
    let serialized = serde_json::to_string(&credentials).unwrap();
    let mut options = options("");
    options.password = None;
    options.credentials = Some(serde_json::from_str(&serialized).unwrap());
    let mut sender = Sender::connect(device, options).await.unwrap();
    offer.await.unwrap().event_ack.await.unwrap();
    assert!(sender.credentials().is_some());
    sender.stop().await.unwrap();
    assert!(task.await.unwrap().event_ack);
}

#[tokio::test]
async fn pairing_rejects_a_validly_encrypted_but_forged_accessory_signature() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        persistent_setup(&mut Wire::new(stream), true).await;
    });
    let device = Device::manual(address.ip(), address.port());
    let result = PairingSession::start(&device)
        .await
        .unwrap()
        .finish("1234")
        .await;
    assert!(matches!(
        result,
        Err(Error::AuthenticationRejected {
            kind: AuthenticationKind::Pin
        })
    ));
    task.await.unwrap();
}
