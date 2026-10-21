# AirPlay sender: implementation and verification

Airloom contains an original Rust sender in `crates/airloom-airplay`. It targets
one AirPlay 2 audio receiver using the realtime, encrypted type-96 transport.
The implementation is experimental. Its tests use synthetic receivers and an
independent audio decoder; **no physical HomePod, HomePod mini, Apple TV, stereo
pair, or audioOS version has been validated by this project yet**. A successful
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
The engine may submit 441 frames at a time and await this method without adding
a second periodic scheduler. A delay exceeding 200 ms discards the pending old
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
its workers and sockets.

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
path-delay estimates. PTP connect waits for three actual exchanges, with a
five-second bound; missing clock exchanges are errors, not synthetic success.
A later five-second timing stall is also surfaced.

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
One active sender owns PTP ports; there is no shared multi-session clock daemon.

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
