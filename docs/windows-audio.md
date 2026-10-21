# Windows audio capture

Airloom offers two explicit sound-source scopes: the shared playback mix of an
existing Windows output device, or a selected application's process tree.
Both use Windows' built-in WASAPI loopback interfaces. It installs no audio driver and creates no output device
in Windows Sound settings. The implementation is original Rust code in
`crates/airloom-capture`; Microsoft's Core Audio interfaces are accessed through
the `windows` crate.

## Supported capture path

`enumerate_sources()` returns active render endpoints with their Windows endpoint
IDs, display names and default-device marker. `CaptureStream::start` accepts one
of those IDs or the special ID `default`. The default is resolved once, when the
stream starts. Selecting another endpoint, or following a system default-device
change, requires restarting capture.

Capture is shared-mode, event-driven endpoint loopback. It includes applications
rendering through the selected endpoint's shared audio engine. It does not
capture microphones, intercept an application's exclusive-mode stream, bypass
protected-content restrictions, or redirect an application that is rendering to
another endpoint. Selecting **指定应用** uses the separate process path below.

The event-driven loopback behavior used here is available on Windows 10 version
1703 and later. Airloom's native UI and installer can impose a newer Windows
minimum. See Microsoft's [loopback recording documentation][loopback] and
[application loopback example][process-loopback] for the distinction between
endpoint capture and process capture.

## Application capture (0.4.0)

`enumerate_applications()` lists nonexpired Core Audio sessions, including paused
sessions. An app may need to play once before it has a selectable audio session;
the UI provides a dedicated **刷新列表** action. Windows system sounds, Airloom
itself, inaccessible processes and shared system hosts are excluded. Processes
are grouped only through older ancestors with the same full executable path, so
a browser audio subprocess can resolve to its browser root without ascending
into Explorer or a launcher. Each choice includes its process ID to distinguish
separate running instances.

Application IDs contain both PID and process creation time. Selection is explicit:
switching source scope stops the existing route, retains receiver selection, and
clears the selected source. Application enumeration never automatically chooses
the first app. A disappearing app does not select a different app or the system
mix. Restarted instances need to be selected again.

The worker opens the exact process lifetime and keeps its handle. It activates
`VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK` through `ActivateAudioInterfaceAsync`
with `PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE`. Capture therefore
requests the chosen process and its children, independent of their physical
output endpoint. Microsoft documents this path for build 20348 and newer;
Airloom already requires Windows 11 build 22000 or newer. See the official
[application loopback example][process-loopback] and [activation contract][activation].

The completion callback is agile and owns the immutable activation payload until
Windows releases it, including after the five-second activation deadline. The
MTA worker receives a signal and retrieves the audio client. It requests stereo
16-bit PCM at the transport sample rate with Windows automatic format conversion.
The existing bounded capture and sender queues are shared with endpoint mode.

Async activation cannot be cancelled at the Windows API boundary. Before
registering its callback, capture pins the module containing that callback until
process exit. The native host also pins the validated engine module before
creating an instance: blocking device enumeration may outlive the runtime's
bounded shutdown wait. These pins preserve executable code, not active capture
or a running process. The updater waits for complete process exit before replacing
the installation, so module residency does not hold an update open.

If the app has no rendering streams, the API produces silence. The worker checks
process lifetime at each event-loop iteration (at most a 20 ms wait), prioritizes
terminal failures over queued samples, and stops on exit. This is one process
tree, not a browser-tab selector: tabs sharing that browser instance may all be
captured. Apps delegating audio to an unrelated service or inaccessible process,
protected media and uncommon application models require separate validation.

The **电脑静音** control is disabled in app mode. Endpoint mute would also affect
other applications, so the capture API rejects that operation. Local app playback
continues; the app's original output routing is not changed.

## Audio contract

Every `AudioChunk` contains finite, interleaved stereo `f32` samples in the range
`[-1, 1]`, a sample rate, a monotonically advancing sequence number, per-channel
RMS and peak values, and the cumulative number of output frames evicted from the
capture queue. The default output rate is 44,100 Hz. The engine converts these
samples to the transport's integer PCM format.

The capture worker reads the endpoint's native mix format and handles:

- Unsigned 8-bit and signed 16-, 24- and 32-bit integer PCM.
- 32- and 64-bit IEEE floating-point PCM.
- Mono, stereo and up to eight channels with a valid Windows speaker mask.
- `WAVEFORMATEXTENSIBLE` PCM, including integer samples whose valid bits are
  left-aligned in a wider container.
- WASAPI silent packets, including a null data pointer, and discontinuity flags.

Mono is duplicated to both channels. Stereo preserves unity gain. Multichannel
audio is mixed to stereo using speaker positions: front left/right retain their
side, center and surround channels contribute at reduced gain, and the LFE
channel is omitted. Each output's coefficients are normalized to avoid clipping
from correlated surround channels. This is a conservative playback downmix, not
a configurable studio matrix. A multichannel format without an interpretable
speaker mask is rejected. Invalid/nonfinite floating-point values become zero.

Rate conversion uses a stateful windowed-sinc resampler (`rubato`) with a
256-frame input block and anti-alias filtering. Equal-rate audio bypasses the
resampler. Resampler history is reset on a WASAPI discontinuity; incomplete
input blocks and filter tail are discarded when capture stops. This is a live
stream, not an archival recording API. The capture resampler performs fixed-rate
format conversion; receiver-clock synchronization and long-term network timing
belong to the transport engine.

## Threading, overload and failures

Source enumeration runs on a short-lived COM-initialized thread. A capture
stream has one dedicated MTA COM worker. The worker creates and releases its
Core Audio interfaces on that thread, decodes each native packet, releases the
WASAPI buffer, then resamples and queues the output. The caller never receives a
COM pointer or a borrowed Windows audio buffer.

The audio queue is bounded to 1–64 chunks (eight by default). When the consumer
falls behind, the oldest queued chunk is evicted and `dropped_frames` increases.
Capture does not block on network transmission or let latency grow without
bound. Chunk sizes depend on the endpoint period and resampling ratio, so queue
capacity is a chunk count, not an exact duration. The engine should surface
nonzero drops when diagnosing slow processing or a stalled sender.

`read_timeout` returns `Ok(None)` if no packet arrives before the deadline.
Silence between WASAPI packets is not an invented successful recording.
Windows failures, including an invalidated/disconnected endpoint, terminate
the worker and are returned to the caller. There is no silent fallback to a
different output device. The engine can reconnect after asking the user to
select an available source. Stop is idempotent and joins the worker; its normal
event wait is capped at 20 ms, although a Windows API call itself has no hard
real-time completion guarantee. Initialization and stop should therefore run on
an engine thread rather than the UI dispatcher.

## Optional local mute

Loopback copies audio; it does not transfer ownership of the source application's
playback stream. Local playback normally continues while HomePod receives the
copy. `mute_local` is off by default. Enabling it changes the selected endpoint's
mute state through `IAudioEndpointVolume`; this affects every application using
that local output device.

Pre-mute versus post-mute loopback behavior depends on the endpoint and driver.
Muting the endpoint can also silence its loopback stream. Airloom does not claim
that this control can always make only the HomePod audible. Test it with actual
source audio and the capture meters on the target machine. The Windows driver
documentation for [audio loopback pre/post volume controls][loopback-volume]
describes why a universal pre-mute assumption is unsafe.

The worker remembers the original mute state and tags its changes with a unique
event-context GUID. A Core Audio callback records external volume/mute changes
without blocking. On disable, normal stop or normal destruction, Airloom unmutes
only if it originally changed an unmuted endpoint to muted, the endpoint remains
muted, and no external change has been observed. It never unmutes an endpoint
that was already muted when the option was enabled. After an external change it
leaves the current state alone; the user can set the intended state in Windows.

Windows does not provide an atomic conditional `SetMute`, so an independent
control changing the endpoint at precisely the restoration instant still has a
small race window. Process termination, a crash or power loss cannot guarantee
restoration. This option is a convenience with visible scope, not an audio
routing guarantee.

## Credentials

`credentials::{load, save, delete}` store opaque authentication bytes in the
current user's Windows Credential Manager as generic credentials. Entries are
namespaced by service and key and persist for that user on the current machine.
They are not a machine-wide secret shared with other Windows users. The backend
does not log credential contents or include them in error messages, and it does
not write a plaintext fallback file.

The engine must call `save` only after explicit **Remember on this PC** consent.
It owns password/pairing serialization and should zeroize the plaintext buffers
it creates and the bytes returned by `load` after use. The temporary Windows
allocation returned by `CredReadW` is zeroized before it is freed. The supported
blob limit is Windows' `CRED_MAX_CREDENTIAL_BLOB_SIZE` (2,560 bytes); larger data
is rejected. Deleting an absent entry succeeds. See [CredWriteW][cred-write],
[CredReadW][cred-read] and [CREDENTIALW][credential] for the operating-system
storage contract. There is no credential export or roaming implementation.

## Verification and remaining hardware checks

The library deliberately returns `UnsupportedPlatform` for capture and secret
storage on non-Windows hosts. Portable tests cover integer decoding, stereo
mapping, invalid/silent buffers, resampling rate and channel separation, level
measurement, credential namespace separation, and mute-restoration ownership.
Windows-specific code must also pass an MSVC-target compile check:

```text
cargo test -p airloom-capture
cargo check -p airloom-capture --target x86_64-pc-windows-msvc
```

These checks do not establish Windows hardware behavior. Before a release is
represented as usable, run an end-to-end Windows/HomePod test with actual music:
confirm endpoint enumeration, 44.1/48 kHz output, channel separation, mute behavior
on the chosen driver, external volume changes, device removal, stop/start and
sleep/resume recovery. Verify credential save/load/delete using a disposable
test credential and inspect that no password appears in logs or settings files.
The separate ignored `process_loopback_activation_and_lifetime` integration test
uses the real Windows API and an owned silent process to check activation,
process identity, stop, mute refusal and target exit. It is run explicitly in
Windows CI. It does not prove audible isolation between two simultaneously
playing applications. That check, browser child-process behavior and a real
HomePod listening test remain required hardware validation. Automatic endpoint
migration, automatic app relaunch/reselection and exclusive/protected playback
remain outside this implementation.

[loopback]: https://learn.microsoft.com/en-us/windows/win32/coreaudio/loopback-recording
[process-loopback]: https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/
[loopback-volume]: https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/ksproperty-audioloopback
[cred-write]: https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credwritew
[cred-read]: https://learn.microsoft.com/en-us/windows/win32/api/wincred/nf-wincred-credreadw
[credential]: https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentialw

[activation]: https://learn.microsoft.com/en-us/windows/win32/api/mmdeviceapi/nf-mmdeviceapi-activateaudiointerfaceasync
