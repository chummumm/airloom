# AirPlay sender: implementation and verification

Airloom contains an original Rust sender in `crates/airloom-airplay`. It targets
an AirPlay 2 audio receiver per `Sender`, using the realtime, encrypted type-96 transport. The application fans one capture stream out to independently paced senders.
The implementation is experimental. Its tests use synthetic receivers and an
independent audio decoder. A user confirmed that 0.1.1 connects to a HomePod, captures nonzero audio and sends packets, but reported no audible output. **Audible playback on physical HomePod, HomePod mini, Apple TV, stereo pairs and specific audioOS versions remains unconfirmed.** A successful
RTSP exchange, a PTP exchange, and a UDP send are observable protocol events;
none proves that a speaker produced sound.

## Library boundary

`discover(Duration)` returns a bounded mDNS snapshot. `Device::manual` provides
an explicit IP/port path. `Sender::connect` performs authentication and transport
setup, and fails with a typed error when an exchange fails. `send_pcm` accepts
interleaved **44,100 Hz, signed 16-bit stereo** samples, at most one second per
call. It accumulates 352 frames per packet and owns the media pacing clock.

The caller owns its bounded capture queue. It must supply zero samples when
capture is idle; a period with no `send_pcm` calls is not implicitly silence.
Each receiver worker submits 441-frame blocks and awaits this method. The
application captures once, produces a shared block every 10 ms, and sends it to
independent bounded queues. It never awaits one receiver inside the producer or
splits the capture queue between consumers. A full queue or a block older than
200 ms triggers recovery for that receiver only. A delay exceeding 200 ms discards the pending old
audio, advances the RTP position by the missed duration, and establishes a new
anchor. Sequence numbers and cryptographic nonces continue; the `reanchors`
counter records that event. Audible recovery after a machine suspension remains
a hardware qualification item.

`Sender::volume_control()` returns a cloneable `VolumeControl`. Its async
`set_volume` runs independently of the audio pacer. A control timeout, I/O error,
or cancellation marks the session failed, so subsequent audio calls cannot
silently continue on a desynchronized control stream. The control request
timeout is six seconds. Do not wrap individual requests in a shorter external
timeout and then reuse the same session. `stop` attempts TEARDOWN and releases
workers even if the receiver does not answer. Dropping a sender also releases
its receiver-specific workers and sockets. The shared PTP listener intentionally
retains UDP 319/320 for the application lifetime, independently of individual
receiver sessions; a stopped receiver only removes its own routing entry.

## Discovery and authentication

Discovery merges `_airplay._tcp.local.` and `_raop._tcp.local.` records by device
identity, preferring the AirPlay endpoint. Apple's two-word features string is
parsed **low word first**. Feature bit 41 selects PTP in automatic mode; status
flag **0x8** indicates PIN authentication, and `pw=true` requests a password.
These meanings were checked against the upstream protocol reference [1]. An
AirPlay 1-only endpoint can appear in discovery, but the sender does not implement
legacy RSA/AES RAOP or claim support for it.

The implemented authentication paths are:

- **Transient pairing:** `/pair-setup`, `X-Apple-HKP: 4`, SRP-6a with the
  RFC 5054 3072-bit group, SHA-512, identity `Pair-Setup`, and the supplied
  password or protocol secret `3939`. The receiver's M4 proof is verified.
- **Digest protection:** MD5 or SHA-256 Digest challenges, including `qop=auth`,
  are answered when a password was supplied. A receiver that rejects the
  password as its SRP secret is retried once on a new TCP connection using the
  fixed transient secret, with the password retained for Digest. There is no
  unbounded authentication retry, and a malformed/failed wire exchange is not
  retried on the same connection.
- **Persistent pairing:** `PairingSession::start(&device)` starts the PIN
  exchange and retains the connection while the user reads the code;
  `finish(pin)` completes M1–M6. Both controller and accessory Ed25519 proofs
  are verified. The returned opaque, serializable credentials contain the
  controller's private signing seed and must be treated as a secret.
- **Remembered credentials:** `/pair-verify` uses fresh X25519 session keys and
  verifies the saved accessory identity and Ed25519 signature before establishing
  an encrypted channel. The controller identity persists across reconnects.

The library does not write credentials to disk. The application stores them
only through its explicit remember-device flow. Passwords and signing seeds are
redacted from Debug/error output; owned secret buffers are cleared on drop where
their types permit it. This is not a claim that every temporary copy or
third-party arithmetic allocation is erased, or that the implementation has
received a security audit. In particular, the selected big-integer SRP backend
does not provide a project-level constant-time guarantee.

Errors distinguish password/PIN/credentials required or rejected, a receiver's
403 access denial, malformed responses, and transport failures. A 470 response
requests PIN pairing. A 403 is not mislabeled as an incorrect password.

**Password compatibility is not universal.** Reference receivers differ on
whether a password replaces the transient SRP secret, protects RTSP with Digest,
or disables transient pairing. Reports also describe HomePods returning 401 at
session SETUP after a successful transient handshake [1]. The implemented
routes are tested against synthetic peers. This project has not demonstrated
that every HomePod “Require Password” configuration accepts them. It does not
remove or bypass the receiver's access restrictions.

## Control and audio transport

After plaintext `/info` and pairing, control messages use authenticated HAP
frames: a two-byte little-endian plaintext length, ChaCha20-Poly1305 ciphertext,
and a 16-byte tag. Frames contain at most 1,024 plaintext bytes. Read and write
directions have distinct HKDF-SHA512 keys and monotonic 64-bit counters. Event
keys use the full pairing secret, including all 64 bytes for transient SRP;
only the audio `shk` is its 32-byte prefix [1,2].

The session performs encrypted binary-plist session SETUP, opens the receiver's
event socket, sends RECORD, and performs audio SETUP. PTP sessions then send
SETPEERS. Stream ports and latency bounds are parsed by dictionary key. The
event worker authenticates incoming messages and acknowledges them. It does
not currently translate Siri/touch remote commands into Windows playback
actions or implement MediaRemote now-playing metadata.

### Session SETUP field compatibility

Physical HomePod testing reported status 400 at the first session SETUP. Review
against the current OwnTone and Music Assistant sender implementations found
incorrect fields in Airloom's request [5,6]. The corrected request preserves
the eight-octet `deviceID` and 64-bit DACP/PTP identity, but sends a separate
six-octet `macAddress`. PTP `ClockID` uses a signed 64-bit plist integer with the
same underlying bits: for example, `0xf842885f71750008` is serialized as
`-557733460333756408`. Passing that value as an unsigned integer to `plist`
would instead produce a 16-byte integer, incompatible with the field's shape.

PTP requests include `ID` (a UUID), `DeviceType`, `ClockID`,
`SupportsClockPortMatchingOverride`, and `Addresses` in `timingPeerInfo`.
`timingPeerList` is also a **request** field in both reference implementations;
it contains the same peer dictionary, including the same UUID. `timingPort`
belongs only to the NTP request. NTP requests omit both PTP peer fields.

Regression tests were run against the old construction first and failed on
the malformed MAC, 128-bit clock integer, and PTP/NTP field mixture. They now
check the full serialized plist, its integer marker (`0x13`, eight bytes),
identity round trips, and mutually exclusive timing fields. The loopback
receiver additionally checks the identity lengths in the actual encrypted
request. These checks establish the corrected protocol representation;
successful playback on the reported HomePod still requires a hardware retest.

Audio SETUP requests type 96, ALAC format `0x40000`, two 16-bit channels at
44.1 kHz, and 352 frames per packet. The small original ALAC encoder emits the
format's uncompressed lossless frames. It does not vendor or link Apple's ALAC
encoder. Its output has been decoded byte-for-byte by FFmpeg in the optional
independent-decoder test [3].

Packets contain a 12-byte RTP header, encrypted ALAC, a 16-byte Poly1305 tag,
and the trailing eight nonce bytes. Header bytes 4–11 are authenticated. A full
64-bit nonce counter is initialized from the RTP sequence and continues when
the 16-bit RTP sequence wraps; it is never reset within the key's lifetime.
The control worker handles bounded retransmission requests from the selected
receiver using a bounded cache of 512 original packets.

The requested receiver lead defaults to 750 ms and is clamped to any returned
latency window. `negotiated_latency_ms` describes that configured RTP lead; it
is **not a microphone measurement of end-to-end delay**, and receivers can
apply additional buffering. Packet, byte, retransmission, timing-exchange, and
queue metrics count actual local events.

## Timing

NTP mode binds an ephemeral UDP port, answers timing requests, and sends the
20-byte RTP/NTP sync format. PTP mode binds the selected local interface on UDP
319 and 320, sends unicast gPTP Sync/Follow_Up and Announce messages, responds to
E2E and peer-delay requests, and handles bounded unicast grant requests. If
those ports cannot be bound, the error is explicit; Airloom does not silently
switch a PTP session to NTP.

An isolated HomePod advertising `AudioAccessory*`, `igl=1`, audioOS 27 or newer,
and no `pgid`/`tsid` is followed as the clock master, matching the documented
upstream observation [1]. Other sessions advertise the sender's clock. This is
a deliberately narrow heuristic, not a general IEEE 1588 clock-management
implementation. A followed clock applies measured two-step offsets and E2E
path-delay estimates. Version detection accepts `osvers/ov` and `srcvers/vs`
aliases, with an explicit OS version taking precedence. Validated GET `/info`
versions fill missing discovery metadata. Receiver-owned clocks can first
announce after session SETUP/SETPEERS: waiting for them before SETUP creates a
startup dependency deadlock. After SETPEERS, the sender learns that receiver's
identity/timebase and uses it in playback announcements. Before the first PCM
packet, PTP connect waits for three consecutive exchanges, a 250 ms settling
period, and a valid offset when following, with a five-second bound. A gap over
three seconds resets the readiness streak; Announce/grant messages alone do
not establish readiness. This is the documented Apple fast-start gate, not a
claim that every receiver's servo converges within that period [1, 6].
A later five-second timing stall is also surfaced.

### PTP playback announcement correction in 0.2.0

A real-device report showed valid capture levels and increasing send counters,
but no sound. Two old-code regressions were reproduced against an independent
wire capture and receiver calculation. A 28-byte type-215 announcement must
encode its RTCP length as `0x0006`, which was previously zero [7]. The PTP RTP
positions must not reuse the NTP layout: the independent receiver anchors
`frame_1 - 11025` at the announced PTP nanoseconds [8]. The default 88200-frame
receive window gives the captured 77175-frame difference between the two
positions. Playback lead and receive window are separate; a longer requested
lead also expands the advertised maximum receive window. Arithmetic wraps at
the 32-bit RTP boundary.

The regression compares all 28 bytes with OwnTone's published Spotify iOS
capture, computes sample playback time using Shairport Sync's receiver rule,
and checks actual decrypted nonzero PCM across a full synthetic PTP session.
This confirms those protocol defects are corrected; it does not establish an
audible result on the user's HomePod.

### gPTP profile correction in 0.2.1

The user reported that 0.2.0 still lit the HomePod without audible playback.
The timing sender was emitting minimal PTP messages: 44-byte Follow_Up and
64-byte Announce, without the gPTP extension records used by the receiver
profile. Tests accepted those shortened messages, so a passing loopback test
did not establish real-device clock compatibility.

The original Rust builders now emit the timescale flag, the profile's control
and interval fields, a 76-byte Announce with Path Trace, a 96-byte Follow_Up
with IEEE 802.1AS Follow_Up Information and Apple's clock-identity TLV, and a
106-byte periodic Apple Signaling message [9, 10]. Delay and peer-delay replies
use the same source port identity/profile, and unicast grants explicitly invite
renewal. Messages are unicast only to the selected peer; a followed receiver
does not receive a competing sender Announce/Sync.

The fixed Follow_Up regression is assembled from externally documented wire
fields, not a packet capture. The full-session receiver separately validates
the required profile and TLVs before replying to timing probes. It therefore
rejects the old 44/64-byte form. A separate follower session withholds its
clock until SETPEERS to cover startup dependency order. Independent FFmpeg
decoding still checks decrypted, nonzero ALAC output byte-for-byte.

Event replies preserve the incoming HTTP/RTSP protocol version and include an
empty-body length and audio-latency header. Receiver latency bounds remain
optional: a minimum without a maximum no longer invents a contradictory
maximum from the requested lead. These are concrete interoperability fixes,
but their tests do not establish that the reported HomePod is now audible.

Settings can copy a separate allowlisted diagnostic object containing model
and firmware tokens, format/latency, clock role/readiness, real transport and
receiver-response counters, and capture levels. It excludes names, addresses,
clock/device identities, credentials, raw protocol bodies and free-text errors.
Packets sent and successful feedback are observations, not playback receipts.

### Multiple receiver clocks

One shared hub binds UDP 319/320 per local interface and supplies a consistent
local clock identity/timebase. It routes bounded packets by remote IP. Each
receiver independently tracks its chosen master, offset and exchanges. Stopping
one sender leaves the other route running. The fixed listeners remain bound
until process exit, or until a later connection replaces a stale hub after its
owning runtime has been destroyed. Regression tests exercise both simultaneous
receivers and full runtime destruction/recreation on the same interface.

Sync announcements use one RTP-to-monotonic-clock anchor, rather than moving
the receiver's timeline whenever a packet is scheduled late. PTP announcements
carry the chosen master identity and timebase. The synthetic tests cover
unicast master/follower messages and identity/timebase propagation. Clock
convergence, long-running drift, Wi-Fi jitter, reconnection, and acoustic timing
on real Apple hardware remain to be measured.

## Verification

The ordinary Rust tests include:

- SRP A/M1/M2/session-key comparison with an independently generated fixture
  from MIT-licensed `srptools==1.0.1`; the fixture generator is checked in and
  contains only fixed, public test secrets.
- HAP frame authentication, size boundaries, fragmented TLV data, invalid SRP
  public values, and nonce continuity across RTP sequence wrap.
- A real loopback TCP/UDP receiver exercising password SRP, Digest, encrypted
  SETUP/RECORD, authenticated event ACKs, ALAC packet decryption, NTP timing,
  retransmission, volume, and TEARDOWN.
- PIN pairing, serialized credentials, a fresh authenticated pair-verify
  connection, and rejection of a forged accessory signature.
- Rejection of an incorrect password before audio setup, plus synthetic gPTP
  master and receiver-following exchanges.
- Reference-derived session SETUP constraints: eight-octet device identity,
  six-octet MAC, signed 64-bit clock encoding even when its high bit is set,
  identical PTP peer descriptions, and separate PTP/NTP field sets.

Run `cargo test -p airloom-airplay`. Tests open loopback sockets; restricted
sandboxes must allow those sockets. The separate test
`independent_ffmpeg_decodes_alac_to_bit_exact_pcm` is normally ignored because
it requires FFmpeg on PATH. Run it with
`cargo test -p airloom-airplay independent_ffmpeg -- --ignored`.
FFmpeg is a test tool only and is not included in the application.

## Remaining compatibility work

The first hardware qualification should use a single HomePod on an IPv4 LAN,
then a password-protected speaker and remembered pairing. Record the exact
model, audioOS version, access setting, timing exchanges, packet counters, and
audible result. Firmware-specific failure responses should extend fixtures
before any compatibility claim is broadened.

Native HomePod stereo groups, multiroom scheduling, buffered type-103 streams,
legacy AirPlay 1, remote playback controls, and automatic multi-address/interface
fallback are outside this initial implementation. IPv4 is preferred; scoped
IPv6 link-local addresses are not represented by the current discovery API.
The shared listener supports multiple independent receivers. Native group
formation and acoustically calibrated, sample-accurate synchronization remain
outside the current compatibility claim.

## Sources and licensing

This project is independently implemented and licensed MIT. Protocol
descriptions informed the wire behavior; no AirSend, AirFlash, airplay2-rs, or
other GPL application's implementation was forked or incorporated. This is
not an assertion of an independently audited clean-room process or Apple
certification. AirPlay and HomePod are Apple's trademarks.

The runtime depends on permissively licensed generic libraries: RustCrypto's
ChaCha20-Poly1305/HKDF/SHA/SRP and `mdns-sd` (MIT or Apache-2.0), `plist` and Tokio
(MIT), and Dalek's Ed25519/X25519 implementations (BSD-3-Clause). Exact versions
are pinned in the crate manifest and workspace lockfile; third-party license
notices remain applicable.

1. [Music Assistant airplay-cli protocol design](https://github.com/music-assistant/airplay-cli/blob/main/DESIGN.md):
   discovery flags, native session order, password variants, PTP behavior,
   channel key directions, audio formats, and timing considerations.
2. [pyatv HAP protocol implementation](https://github.com/postlund/pyatv/blob/master/pyatv/auth/hap_srp.py)
   and [OpenAirPlay HAP description](https://openairplay.github.io/airplay-spec/pairing/hkp.html):
   pairing transcripts and key derivation. pyatv is MIT-licensed; the Rust code
   is original and uses maintained cryptographic libraries.
3. [Apple's published ALAC format and reference codec](https://github.com/macosforge/alac)
   and [CAF specification](https://developer.apple.com/library/archive/documentation/MusicAudio/Reference/CAFSpec/CAF_spec/CAF_spec.html):
   bitstream/container reference for independent decoder validation.
4. [RFC 5054](https://www.rfc-editor.org/rfc/rfc5054),
   [RFC 7616](https://www.rfc-editor.org/rfc/rfc7616), and
   [RustCrypto SRP](https://github.com/RustCrypto/PAKEs/tree/master/srp):
   SRP group/arithmetic and HTTP Digest. HAP's proof transcript is constructed
   explicitly because the selected SRP crate's legacy convenience proof differs.
5. [OwnTone session SETUP construction](https://github.com/owntone/owntone-server/blob/3aee59740c715d40df8de87e06b7b91badfbe20b/src/outputs/airplay.c):
   `device_id_colon_make`, interface MAC formatting, and
   `payload_make_setup_session_ptp`, including its explicit signed `ClockID`
   example and both peer request fields.
6. [Music Assistant native sender construction](https://github.com/music-assistant/airplay-cli/blob/1701bc25e9d303b42532569f624b64c0d0afb1f6/src/ap2_client.c):
   distinct eight-byte device ID and six-byte MAC, `ap2_make_timing_peer`, and
   separate PTP/NTP session SETUP builders. These wire-format facts informed
   the fix; no upstream implementation code was incorporated.

7. [OwnTone PTP wire format and captured Spotify iOS packet](https://github.com/owntone/owntone-server/blob/3aee59740c715d40df8de87e06b7b91badfbe20b/src/outputs/rtp_common.c):
   28-byte type-215 length, RTP fields and clock identity; the published capture
   supplies a fixed expected packet independent of Airloom's constructor.
8. [Shairport Sync AirPlay 2 control receiver](https://github.com/mikebrady/shairport-sync/blob/cf6998daa1876c5a8f97128a960c8afc0e731841/rtp.c):
   receiver-side PTP sample anchor, 11025-frame offset and receive-window math.
9. [Music Assistant gPTP wire-profile descriptions](https://github.com/music-assistant/airplay-cli/blob/1701bc25e9d303b42532569f624b64c0d0afb1f6/src/ap2_ptp.c):
   profile headers, Path Trace, IEEE/Apple Follow_Up extensions, periodic
   Signaling and unicast negotiation. Only protocol facts inform Airloom's
   independently written message builders; no upstream code is incorporated.
10. [NQPTP receiver's Follow_Up Information parser](https://github.com/mikebrady/nqptp/blob/main/nqptp-message-handlers.c):
    independently documents IEEE 802.1AS Table 11-11, the 28-byte TLV value,
    standard OUI/subtype and Apple clock-identity position.
