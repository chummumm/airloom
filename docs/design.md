# Airloom native desktop design

Airloom is a Windows 11 desktop application for sending existing Windows playback audio to a supported AirPlay receiver. The desktop shell is **WinUI 3, C++/WinRT and XAML**. It does not host a browser, HTML interface, Electron application, or WebView. Audio capture, network discovery, authentication, credentials, transport and reconnection belong to the Rust core.

The first-run screen is deliberately honest: it contains no example speakers, invented signal, successful-connection badge or measured latency. Devices and playback endpoints appear only after the engine reports them. With no engine DLL, the application remains usable as a window and explains the installation problem.

## Visual direction

The interface is a quiet listening surface with a small original three-bar Airloom mark. A lavender accent marks selection and actions; a mint signal color identifies actual audio. Rounded cards, fine borders and generous spacing sit over the Windows Mica backdrop. Light, dark and Windows high-contrast resources are defined in XAML. The default theme follows Windows; a user can choose light or dark in settings.

| Element | Design |
| --- | --- |
| Window | 1140 × 810 effective pixels initially, centered and clamped to the current monitor work area |
| Minimum tracking size | 880 × 680 effective pixels, adjusted for DPI |
| Title bar | Original vector brand, preview badge, native minimize/maximize/close controls and Windows snap behavior |
| Content | Scrollable at smaller sizes; 296 px discovery column and flexible listening column |
| Cards | 20 px corners, 18–24 px internal spacing, subtle 1 px borders |
| Main heading | 32 px, semibold; Chinese product copy |
| Device names | 14 px, semibold, truncation with full accessible name |
| Dark accent / signal | `#BFB1FF` / `#8DE1CF` |
| Light accent / signal | `#6A53CC` / `#127B68` |
| Primary action | 44 px high, 12 px corners, native Fluent button behavior |

The speaker illustration and logo are original vector geometry. The executable's multi-resolution icon derives from the same Airloom mark. This is not an Apple-provided product illustration.

### Layout and hierarchy

1. **Header:** brand, short listening-oriented heading, current connection status, settings.
2. **Discovery:** actual network devices, selected device, model, access requirement, refresh and a useful empty state.
3. **Source:** an existing active Windows playback endpoint supplied by the engine. A process picker is not shown until the core actually supplies that capability.
4. **Listening surface:** selected speaker, connection explanation, actual waveform, left/right level, sample rate, elapsed time and sent packet count.
5. **Control strip:** connect/disconnect/cancel, debounced receiver volume and an explicit local endpoint mute preference.

The selected device and source are locked while a connection is active or being established. The user disconnects before switching routes. Refresh remains available.

## Data integrity and audio display

The UI polls a nonblocking engine snapshot every 33.333 ms while the window is visible. When only the tray is visible, it reads approximately four snapshots per second so discovery and reconnection remain current without continually redrawing a hidden window. The polling timer runs on the XAML dispatcher. Engine commands must enqueue work; they must not perform socket operations or synchronous pairing on the UI thread.

All meter and waveform data come from the core snapshot. Amplitudes are clamped at the boundary to valid finite ranges. The line renders the supplied waveform samples in their supplied order; there is no synthetic oscillator, randomly moving meter or simulated device. When the stream is silent, the UI explains that it is waiting for audio. When disconnected, meters are zero and the line is hidden.

Displayed dB values are derived from the supplied linear amplitude using `20 × log10(amplitude)`, with silence shown as `−∞ dB`. The meaning of the amplitude measurement itself—such as peak or RMS—is owned by the capture engine. The UI does not rename it as a calibrated acoustic loudness measurement.

The application does **not** display an acoustic end-to-end latency number. `metrics.buffer_ms` can be parsed for future detailed diagnostics, but it must not be presented as a measured HomePod delay. Buffer configuration, sender timing, receiver buffering and audible sound at a microphone are different quantities.

### Motion and accessibility

Connection state changes fade the speaker title over 180 ms. The interface respects Windows client-area animation settings: the decorative transition, spinning connect indicator and dynamic waveform are disabled when animation is disabled. The two numerical and level displays continue to communicate signal. Users can independently turn the waveform off in settings.

Buttons, source selection, password entry and sliders are native controls with keyboard interaction. Device cards have accessible names including selection and access requirement. The status badge uses a polite accessibility live region. Artwork is excluded from the accessibility tree, while status remains readable in text. High-contrast theme resources replace the ordinary color palette with Windows system colors.

## Connection and authentication

The UI recognizes these engine states:

| State | Visible behavior |
| --- | --- |
| `idle` | Select a speaker and source; connect only when both are present |
| `discovering` | Discovery status and progress indicator; actual results remain visible |
| `connecting` | A cancellable connection action with progress indication |
| `auth_required` | Access-password or pairing-code dialog, plus a manual reopen action |
| `streaming` | Actual signal, elapsed transfer duration and disconnect action |
| `reconnecting` | Explicit reconnection status with an action to stop retrying |
| `error` | Engine error explanation and retry, without claiming the connection succeeded |

The first authentication request opens a native `ContentDialog` associated with the window's `XamlRoot`. It uses a `PasswordBox`, a cancel action and an unchecked-by-default “remember this device” checkbox. Password requests also offer a **use pairing code** action, which asks the engine to initiate pairing. The dialog never turns an arbitrary transport error into an assertion that the user's password was wrong.

Each challenge includes a monotonically changing `request_id`. The UI captures it when the dialog opens, closes the dialog when the engine changes challenges or leaves authentication, and checks it again before handling the result. Authenticate, pair and dialog-cancel commands include that identifier. The core must also reject a stale identifier: a 30 fps UI cannot itself eliminate the interval between its last snapshot and a button click. This prevents an old password dialog from cancelling or authenticating a newer connection.

Credential values are absent from snapshots and UI logs. The UTF-8 command copy is cleared after calling the C ABI, and the password control is cleared after the dialog completes. The UI does not promise complete erasure of every framework-managed string allocation. Persistent credential protection and storage belong to the core; “forget credentials” calls that core operation for the selected device.

## Desktop behavior

- **One instance:** a per-session named mutex prevents a second capture engine from starting. Reopening Airloom activates the existing window through its registered window identity.
- **Tray:** `Shell_NotifyIcon` provides a native notification-area icon. Single click or keyboard selection opens the window. The context menu contains open and exit.
- **Close:** the window hides only if a tray icon was successfully registered. A first-close notification explains how to reopen or exit. If the tray is unavailable, closing retains normal window-close behavior.
- **Explorer restart:** the app handles `TaskbarCreated` and recreates its tray icon.
- **Exit:** polling stops, authentication UI closes, disconnect is requested and the owned engine is destroyed before the process exits.
- **Login startup:** an optional current-user Windows Run entry points to the quoted executable path and `--background`. It needs no elevation. It starts in the tray and does not automatically connect, capture or mute.
- **Preferences:** theme and waveform visibility live under `HKCU\Software\Airloom`. No passwords are stored in these preferences.

Local mute refers to the **selected Windows output endpoint**. It can affect other applications using that endpoint. The control and its tooltip explain that scope; Airloom does not advertise that every application can be privately silenced without affecting capture. Restoration and recovery are responsibilities of the core's capture/mute owner. This version does not create a Windows virtual playback endpoint.

## Native/core boundary

The shell loads `airloom_core.dll` from the executable directory with restricted DLL search flags. It never searches a working directory for an arbitrary same-named engine. All symbols must be present before an engine instance is created.

```c
void* airloom_create(void);
void  airloom_destroy(void* engine);
int   airloom_command(void* engine, const char* json_utf8);
char* airloom_snapshot(void* engine);
void  airloom_free_string(char* json_utf8);
```

The DLL owns the engine and snapshot allocations. A returned snapshot is always released through `airloom_free_string`, including JSON parsing failures. C++ objects and exceptions must not cross the ABI. The UI considers command return code `0` accepted; other results are surfaced as an unaccepted operation rather than shown as success. Volume changes are debounced by 180 ms before being submitted.

The core snapshot is versioned. The UI accepts schema version `1`, validates types and finite numbers, limits decoded device/source lists, and rejects oversized or incompatible responses. Optional or null objects do not crash rendering.

```json
{
  "version": 1,
  "state": "idle",
  "status_message": "",
  "devices": [],
  "sources": [],
  "selected_device_id": "",
  "selected_source_id": "",
  "volume": 0.75,
  "mute_local": false,
  "level": { "left": 0.0, "right": 0.0 },
  "waveform": [],
  "elapsed_seconds": 0,
  "error": null,
  "auth": null,
  "metrics": { "packets_sent": 0, "sample_rate": 0, "buffer_ms": null }
}
```

Device objects contain `id`, `name`, `model`, `address`, `password_required` and `online`. Source objects contain `id`, `name` and `kind`. An authentication object contains `kind` (`password` or `pin`), `device_id`, `message` and `request_id`; it never contains the supplied secret. Error objects contain `code` and `message`.

| Command | Payload fields beyond `type` |
| --- | --- |
| `refresh` | None |
| `select_device` | `device_id` |
| `select_source` | `source_id` |
| `connect` | None |
| `disconnect` | Optional `request_id` when cancelling an authentication dialog |
| `set_volume` | `volume`, in `[0, 1]` |
| `set_mute_local` | `enabled` |
| `authenticate` | `value`, `remember`, `request_id` |
| `pair` | `request_id` from the active password challenge |
| `forget_credentials` | `device_id` |

## Build and distribution

The native solution is `native/Airloom/Airloom.sln`; the actual program is an unpackaged x64 WinUI desktop executable. The baseline is Windows 11 build 22000 and Windows SDK 10.0.26100.0. The project selects the v143 C++ toolset with Visual Studio 2022 and v145 with Visual Studio 2026, following the current Microsoft C++ sample. The corresponding desktop and Windows application C++ build components must be installed. `packages.config` locks the Windows App SDK 1.8 dependency set and C++/WinRT generator to the same versions used by Microsoft's 1.8 stable self-contained C++ sample.

From a Visual Studio developer shell at the repository root:

```powershell
nuget restore native\Airloom\Airloom.sln -NonInteractive
msbuild native\Airloom\Airloom.sln /m /p:Configuration=Release /p:Platform=x64 /p:WindowsTargetPlatformVersion=10.0.26100.0
```

The executable is emitted at `native\Airloom\bin\x64\Release\Airloom.exe`. The distribution must include **the whole output directory**, including compiled XAML/resource files and the self-contained Windows App SDK payload. Place the matching x64 `airloom_core.dll` alongside the executable. The C++ runtime must also be included app-locally from the licensed Visual C++ redistributable or installed as a prerequisite; “self-contained Windows App SDK” is not a promise that the CRT is already present on the target machine.

The locked SDK's umbrella dependency set includes the WebView2 SDK as a transitive WinUI dependency. Airloom creates no WebView2 control and implements no browser-based UI. The application code calls native WinUI controls, Windows APIs and the engine ABI only.

### Verification boundaries

XML validity, source review and a successful Windows compile are separate from launching the app, inspecting its render and connecting actual hardware. A hosted CI screenshot can establish that the native interface opens and renders; it cannot establish HomePod compatibility, receiver authentication behavior, audible latency or correct local-mute recovery on every audio device. Those require the engine's protocol/capture tests and testing against real Windows hardware and receivers.

Useful Windows acceptance checks include empty discovery, absent engine, available/unavailable playback endpoints, selection while idle, password and pairing-code retry, stale authentication results, quiet and active audio, connection interruption, user volume drag, user-selected local mute, cancellation, close-to-tray, Explorer restart, second launch, startup opt-in and light/dark/high-contrast/reduced-motion settings.

## Primary platform references

- [Microsoft's Windows App SDK 1.8 stable self-contained C++ WinUI sample](https://github.com/microsoft/WindowsAppSdkResources/tree/main/WindowsAppSDK-Samples/1.8-stable/SelfContainedDeployment/cpp/cpp-winui-unpackaged)
- [Unpackaged WinUI apps](https://learn.microsoft.com/windows/apps/package-and-deploy/unpackage-winui-app)
- [Mica and other system backdrops](https://learn.microsoft.com/windows/apps/develop/ui/system-backdrops)
- [Title-bar customization](https://learn.microsoft.com/windows/apps/develop/title-bar)
- [Shell_NotifyIconW](https://learn.microsoft.com/windows/win32/api/shellapi/nf-shellapi-shell_notifyiconw)
- [WASAPI loopback recording](https://learn.microsoft.com/windows/win32/coreaudio/loopback-recording)
