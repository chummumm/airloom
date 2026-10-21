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
        initial_volume: 0.25,
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
            info(&mut wire).await;
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
        event
            .write(b"POST /event RTSP/1.0\r\nCSeq: 77\r\nContent-Length: 0\r\n\r\n")
            .await
            .unwrap();
        let ack = request(&mut event).await;
        assert!(ack.0.starts_with("RTSP/1.0 200"));
        assert_eq!(ack.1["cseq"], "77");
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
    offer
        .send(AudioOffer {
            key,
            control,
            timing,
            event_ack,
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
        reply(&mut wire, &message, 200, &[], "").await;
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
    let metrics = sender.metrics();
    assert_eq!(metrics.packets_sent, 3);
    assert_eq!(metrics.retransmissions_sent, 1);
    assert_eq!(metrics.timing_exchanges, 1);
    sender.stop().await.unwrap();
    let report = receiver.task.await.unwrap();
    assert!(report.digest_verified && report.event_ack);
    assert_eq!(report.volumes.len(), 2);
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
        Arc,
    };
    let remote: IpAddr = "127.0.0.20".parse().unwrap();
    let event = UdpSocket::bind(SocketAddr::new(remote, 319)).await.unwrap();
    let general = UdpSocket::bind(SocketAddr::new(remote, 320)).await.unwrap();
    let observed = Arc::new(AtomicU64::new(0));
    let observed_clock = observed.clone();
    let clock_receiver = tokio::spawn(async move {
        let mut event_buffer = [0; 2048];
        let mut general_buffer = [0; 2048];
        loop {
            tokio::select! {
                received = event.recv_from(&mut event_buffer) => {
                    let (size, address) = received.unwrap();
                    if event_buffer[0] & 15 != 0 { continue; }
                    assert_eq!(size, 44);
                    assert_eq!(event_buffer[1], 2);
                    observed_clock.store(u64::from_be_bytes(event_buffer[20..28].try_into().unwrap()), Ordering::Release);
                    // A receiver asks for a real delay exchange. Construct this
                    // PTP request without the sender's packet builder.
                    let mut delay = [0u8; 44];
                    delay[0] = 0x11;
                    delay[1] = 2;
                    delay[2..4].copy_from_slice(&44u16.to_be_bytes());
                    delay[6..8].copy_from_slice(&0x0400u16.to_be_bytes());
                    delay[20..28].copy_from_slice(&0x8899aabbccddeeffu64.to_be_bytes());
                    delay[29] = 1;
                    delay[30..32].copy_from_slice(&event_buffer[30..32]);
                    delay[32] = 1;
                    event.send_to(&delay, address).await.unwrap();
                }
                received = general.recv_from(&mut general_buffer) => {
                    let (size, _) = received.unwrap();
                    assert!(size >= 34);
                    assert_eq!(u16::from_be_bytes([general_buffer[2], general_buffer[3]]) as usize, size);
                }
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
