# Native desktop design and interaction

Airloom's desktop shell is **WinUI 3, C++/WinRT and XAML**. Capture, discovery, authentication, credentials, transport and reconnection belong to the Rust core. The shell hosts no browser or WebView UI. [PRODUCT.md](../PRODUCT.md) describes the product's job; [DESIGN.md](../DESIGN.md) defines the visual system.

## Home and settings

Home has a fixed receiver rail and a flexible working area. Task header, source, signal and transport form a direct operating sequence. Source information explains which Windows output is captured. Selected-target text summarizes real selections; receiver rows show their individual states. The signal panel displays actual left/right levels, waveform, sample rate, elapsed transfer time and packet count. A short test tone helps isolate capture problems.

The right grid binds its width to the scroll viewer's viewport, without a centered maximum-width wrapper. Home and settings reserve vertical scrollbar space. At 1280 effective pixels the transport moves beside the signal; smaller windows keep it below. The native window initially opens at 1140 × 810 effective pixels, clamped to the monitor work area, with an 880 × 680 minimum tracking size.

Settings is a separate native view. It starts with update status and check action, followed by diagnostics, appearance/startup, receiver visibility and application actions. Hiding a selected receiver explicitly deselects it before persisting its visibility. Hidden IDs and display names remain local so offline receivers can be restored. Discovery never silently chooses a first target.

## Volume stability

The reported movement had two interacting risks in the prior implementation: dynamic content measured inside a centered layout, and a snapshot that could overwrite the slider immediately after its debounced command was enqueued. The pending flag was cleared before the engine acknowledged the new value.

The replacement has four protections:

1. `PageContent` owns the right viewport width. A changing label cannot alter its center or the receiver rail's width.
2. The percentage has a fixed 52 effective pixel width and tabular numerals. Status and transport controls also have reserved slots.
3. A local value remains authoritative while a pointer is held, while a command is pending, or while acknowledgement is outstanding. A matching snapshot releases that protection. A three-second acknowledgement timeout reports a recoverable error.
4. Receiver rows are keyed by ID and updated in place. State changes do not clear and rebuild the list. Frequently updated text and updater controls are assigned only when changed.

Slider changes update the percentage immediately. Commands debounce by 180 ms; pointer release flushes the final position. Native routed pointer events are observed with `handledEventsToo` without replacing the slider's own capture or interaction. With an available core, the slider is usable before selecting a receiver so starting volume can be chosen in advance.

## Signal integrity

The visible UI polls a nonblocking snapshot approximately every 33 ms; a hidden tray window polls about four times a second. Commands enqueue core work rather than synchronously performing pairing or socket operations on the UI thread.

Meters and waveform use actual outgoing PCM, including a temporary test tone. Separate capture status describes the real Windows source and is unaffected by the tone. Displayed dB is `20 × log10(amplitude)`, with silence shown as `−∞ dB`. Values are validated and clamped at the boundary. Empty states contain no synthetic waveform or sample receiver.

Capture states distinguish idle, waiting, signal, quiet, silent and error. Silent-input help points to the playback output and local-mute setting. A test tone is a three-second diagnostic action on receivers already streaming; it does not assert that sound was heard. Packet counts and timing exchanges are transport evidence, not acoustic proof or latency measurements.

Windows animation preferences govern decorative fades, the spinner and waveform. Turning off the waveform leaves numeric and meter information. System/light/dark preferences are retained; high contrast uses system resources.

## Multiple receivers and authentication

The core supports eight selected receivers. Selection and state are separate fields. Adding a target while another streams starts only the new target; removing one stops only that target. Retry can address a failed receiver. Changing source preserves selected receivers and reconnects active routes around the replacement capture session.

| Receiver state | Interface behavior |
| --- | --- |
| `idle` | Selected or available, not transmitting |
| `connecting` | Explicit connection progress |
| `auth_required` | Password or pairing-code action for that receiver |
| `streaming` | Active transport, with separately reported capture signal |
| `reconnecting` | Retry state with stop/deselect available |
| `error` | Target failure and retry without resetting healthy receivers |

Global `streaming` can coexist with authentication for another receiver. The UI opens authentication by `auth.request_id`, not only the global state. A native `ContentDialog` names the target, uses a `PasswordBox`, and leaves “remember this device” unchecked by default. Password prompts can request pairing-code mode.

Each challenge is bound to a changing request ID. Obsolete dialogs are dismissed when the active challenge changes. Authenticate, pair and cancel carry the captured ID, which the core also validates. Cancelling one challenge must not disconnect other active receivers. Credential values never appear in snapshots or diagnostics; persistent protection belongs to the core.

## Native/core boundary

The shell loads `airloom_core.dll` from the executable directory with restricted DLL search flags. It validates the complete C ABI before creating an engine. The DLL owns returned allocations, including strings released after parsing failures. C++ exceptions and objects do not cross this ABI.

```c
void* airloom_create(void);
void  airloom_destroy(void* engine);
int   airloom_command(void* engine, const char* json_utf8);
char* airloom_snapshot(void* engine);
void  airloom_free_string(char* json_utf8);
```

The UI accepts snapshot schema version 1, validates types and finite numbers, limits decoded lists, and rejects incompatible or oversized responses. `selected_device_id` remains a legacy first-selection field; `selected_device_ids` contains all selections. Devices expose selection, state, status, optional error and counters. `capture` is separate; `test_tone_active` reports the tone. Definitions live in [model.rs](../crates/airloom-core/src/model.rs) and [Models.cpp](../native/Airloom/Models.cpp).

| Command | Fields beyond `type` |
| --- | --- |
| `refresh` | None |
| `toggle_device` | `device_id`, explicit `selected` |
| `select_device` | Legacy single-target `device_id` |
| `select_source` | `source_id` |
| `connect` | Optional `device_id` for one target |
| `disconnect` | Optional `device_id` or auth `request_id`; neither means all targets |
| `set_volume` | `volume` in `[0, 1]` |
| `set_mute_local` | `enabled` |
| `authenticate` | `value`, `remember`, `request_id` |
| `pair` | `request_id`, optional `device_id` |
| `test_tone` | Optional `enabled`; false cancels |
| `forget_credentials` | `device_id` |

Return code 0 means accepted for processing, not that a receiver synchronously completed the action.

## Diagnostics and updates

The diagnostic action copies **only the separate `diagnostics` object** through native Windows clipboard APIs. It never serializes the full snapshot. The allowlist includes application/schema versions, session state, volume, mute/test-tone flags, capture measurements and receiver slots with structured protocol diagnostics. It excludes device IDs/names, network addresses, credentials, raw error strings and raw protocol traffic. [model.rs](../crates/airloom-core/src/model.rs) and [diagnostics.rs](../crates/airloom-airplay/src/diagnostics.rs) own the contract.

The update service exposes idle, checking, current, available, downloading, launching and error states. Settings displays the version, result, progress and relevant check/install/cancel controls. Checking reads public release metadata; installation is separate. The native helper owns waiting for the app, running the installer, reporting the outcome and cleanup. See [updates.md](updates.md).

## Desktop and exit behavior

`Shell_NotifyIcon` supplies the tray icon. Ordinary close hides only if that icon registered. Explicit exit stops polling, closes authentication, prepares the updater for exit, stops the engine, removes the tray icon and closes the app. Local mute belongs to the core's capture owner and must be restored during orderly teardown.

Installer shutdown is separate from ordinary close. A window carrying `Airloom.MainWindow.v1` advertises `Airloom.GracefulExit.v1` after its handler is installed. Registered message `Airloom.RequestExit.v1` schedules orderly exit. Once exit begins, later close messages cannot hide the process or recreate its tray icon.

`WM_QUERYENDSESSION` returns readiness without shutdown. Cancelled `WM_ENDSESSION` leaves the app alive. Confirmed `ENDSESSION_CLOSEAPP` schedules exit for Restart Manager. Genuine Windows session termination runs teardown before returning from its confirmed handler. These distinctions follow [Microsoft's Restart Manager guidelines](https://learn.microsoft.com/en-us/windows/win32/rstmgr/guidelines-for-applications).

Theme, waveform and visibility preferences live under `HKCU\Software\Airloom`; passwords do not. Opt-in startup uses a current-user Run entry and `--background`, with no elevation. It does not automatically connect, capture or mute. A per-session mutex prevents a second engine; a second launch activates the existing window.

## Verification

[Windows smoke](../scripts/smoke-windows.ps1) operates the real WinUI executable with .NET UI Automation. Its verification covers native settings/back, read-only update check, volume 0/9/10/99/100, layout geometry at 880 × 680, 1100 × 800 and 1600 × 1000, acknowledgement retention and real mouse drag. The current runner desktop is 1024 × 768, so fully visible native screenshots cover the compact home and settings views. Normal/wide geometry checks do not establish a full visual review at those sizes; that requires a sufficiently large display or a supported full-window capture. Clipboard validation checks the restricted diagnostics. The updater harness separately exercises the actual GUI process and exit messages.

Stable automation IDs include `HomeView`, `DeviceRail`, `HomeScrollViewer`, `PageContent`, `SourcePanel`, `SignalPanel`, `TransportPanel`, `VolumeSlider`, `VolumeValue`, `SettingsButton`, `SettingsView`, `SettingsBackButton`, `UpdateStatusText`, `CheckUpdatesButton` and `CopyDiagnosticsButton`.

XML validity, Windows compile, native screenshots, interaction tests and audible output are distinct evidence. At source handoff this redesign has XML/handler consistency checks and resource contrast calculations. Build, screenshot and interaction results must be read from the associated CI run. No web inspector has certified this XAML interface. Receiver listening, mixed authentication, quiet/active capture and endpoint-mute recovery require their appropriate protocol and hardware checks.

## Build and platform references

`native/Airloom/Airloom.sln` is an unpackaged x64 app using Windows App SDK 1.8 and C++/WinRT. The desktop baseline is Windows 11 build 22000; the build uses Windows SDK 10.0.26100.0. Distribute the complete native output, matching `airloom_core.dll` and required C++ runtime. Self-contained Windows App SDK deployment does not imply an arbitrary machine already has the CRT.

```powershell
nuget restore native\Airloom\Airloom.sln -NonInteractive
msbuild native\Airloom\Airloom.sln /m /p:Configuration=Release /p:Platform=x64 /p:WindowsTargetPlatformVersion=10.0.26100.0
```

- [Microsoft self-contained C++ WinUI sample](https://github.com/microsoft/WindowsAppSdkResources/tree/main/WindowsAppSDK-Samples/1.8-stable/SelfContainedDeployment/cpp/cpp-winui-unpackaged)
- [Unpackaged WinUI apps](https://learn.microsoft.com/windows/apps/package-and-deploy/unpackage-winui-app)
- [Title-bar customization](https://learn.microsoft.com/windows/apps/develop/title-bar)
- [Native routed-event handlers](https://learn.microsoft.com/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.uielement.addhandler)
- [Tabular numeral alignment](https://learn.microsoft.com/windows/windows-app-sdk/api/winrt/microsoft.ui.xaml.documents.typography.numeralalignment)
- [Shell_NotifyIconW](https://learn.microsoft.com/windows/win32/api/shellapi/nf-shellapi-shell_notifyiconw)
- [WASAPI loopback recording](https://learn.microsoft.com/windows/win32/coreaudio/loopback-recording)
