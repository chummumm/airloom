# Native desktop design and interaction

Airloom's desktop shell is **WinUI 3, C++/WinRT and XAML**. Capture, discovery, authentication, credentials, transport and reconnection belong to the Rust core. The shell hosts no browser or WebView UI. [PRODUCT.md](../PRODUCT.md) describes the product's job; [DESIGN.md](../DESIGN.md) defines the visual system.

## Home and settings

The 0.4.0 home retains the compact audio workspace introduced in 0.3.0: a source-and-status strip above a dark instrument display, an adjacent receiver panel when space permits, and a persistent transport dock below. The window header carries the Airloom identity and settings action. Source information now distinguishes the shared mix of a Windows output from a selected application's process tree. Selected-target text summarizes real selections; receiver rows show their individual states. The instrument displays real left/right levels, the peak envelope, sample rate, elapsed transfer time and packet count. A short test tone helps isolate capture problems.

`PageContent` binds its width and minimum height to the workspace scroll viewport. Home and settings reserve scrollbar space. At 1050 effective pixels of actual root content width, the receiver panel occupies a bounded 320-pixel column beside the signal; narrower windows put receivers above the instrument, keeping selection visible on opening. `TransportPanel` is a separate row outside that scroll viewer, so connecting, changing volume and muting the local output remain available while the workspace scrolls. The native window initially opens at 1120 × 780 effective pixels, clamped to the monitor work area, with an 880 × 680 minimum tracking size and a 48-pixel title region.

A native `UserControl` owns the root grid's visual states. `Window` is not a control and cannot itself host Visual State Manager behavior, as documented in [Microsoft's WinUI migration guidance](https://learn.microsoft.com/en-us/windows/apps/windows-app-sdk/migrate-to-windows-app-sdk/guides/winui3#visual-state-manager-and-pageresources). The root's loaded/size-changed handlers drive mutually exclusive compact and wide triggers from its measured effective width. This avoids treating the HWND's outer frame or physical pixels as layout width, and restores the compact state after shrinking.

The silver ground, neutral surfaces, monochrome controls and one blue signal hue follow the `achromatic-ledger` direction, seed 192244604. A single shell redraw removed unnecessary navigation chrome. The dark instrument is a working signal display, not an album-art placeholder. Native Segoe type and controls are retained; [DESIGN.md](../DESIGN.md) defines the theme-specific resources and component rules.

Settings is a separate native view with compact groups. Update status/actions and diagnostics occupy the first row; appearance/startup and receiver visibility follow, with application actions below. Labels stay inside their groups beside the related controls. Hiding a selected receiver explicitly deselects it before persisting its visibility. Hidden IDs and display names remain local so offline receivers can be restored. Discovery never silently chooses a first target.

Unavailable Windows outputs and application-selection guidance are explained once beside the source picker, with the full actionable message and a native refresh button. Long source errors can wrap rather than losing their recovery instruction. They are not repeated in the global connection banner or signal description. Session, authentication and network errors retain the banner; settings shows errors from its local actions rather than repeating a source failure above the update controls.

## Native button hierarchy (0.4.0)

[App.xaml](../native/Airloom/App.xaml) owns the button styles. `PrimaryButtonStyle` derives from WinUI's `AccentButtonStyle` and has a 44-pixel minimum height, 10-pixel radius and 18 × 10 padding. `SecondaryButtonStyle` derives from `DefaultButtonStyle`, with a 40-pixel minimum height, 8-pixel radius and 14 × 8 padding. `CompactButtonStyle` reduces secondary actions to 36 pixels with 12 × 6 padding. `QuietButtonStyle` derives from the framework's own `SubtleButtonStyle`, retaining a transparent resting surface and no normal outline. The 36 × 36 `IconButtonStyle` and padded `ToolbarButtonStyle` derive from Quiet.

The app does not shadow the framework's `SubtleButtonStyle` or copy a control template. Normal, pointer, pressed and disabled brushes use the template's verified resource keys in the existing final merged resource dictionary. App-owned semantic brushes remain available through the root theme dictionaries for C++ lookup. High contrast maps control states to system colors. The native template contract is defined in [WinUI 1.8.1 Button resources](https://github.com/microsoft/microsoft-ui-xaml/blob/winui3/release/1.8.1/src/controls/dev/CommonStyles/Button_themeresources.xaml).

Labels use 14-pixel semibold Segoe and glyphs use 16 pixels with an 8-pixel gap. Their content styles do not set a local foreground, preserving native state inheritance. Route controls retain the existing snapshot-aware foreground update. All button styles retain native system focus visuals, and the template supplies pointer/press feedback without an application-owned animation layer.

The primary dock action is 152 × 44 pixels. Its icon, label and loading indicator use the same reserved content region; the authentication action remains inside the existing 88-pixel action row. Settings, test tone, refresh, hide/restore and diagnostics use the corresponding hierarchy rather than independent padding and heights. The source refresh button is 104 pixels wide so changing from **刷新列表** to **正在刷新…** cannot move its edge. These dimensions were checked in the final 0.4.0 native UI run below.

## Explicit sound-source scope (0.4.0)

`SourceModePicker` selects `endpoint` (**系统声音**) or `application` (**指定应用**). It reserves a 134-pixel column before the flexible `SourcePicker`; status keeps a separate 154-pixel column. Both selectors and the 104-pixel refresh action remain in the source strip at the minimum 880 × 680 window. Source scope and refresh start disabled until a valid core snapshot enables them. Their accessible names and tooltips describe the active scope.

Endpoint mode keeps the existing shared-output loopback path. Application mode uses Windows process loopback for the explicitly selected process and its children. App choices come from eligible nonexpired Core Audio sessions, including paused sessions. An application may need to play once before appearing; the empty-state instruction and **刷新列表** action provide that recovery. Process IDs distinguish instances in the list, while the source identity binds PID to process creation time. The [capture implementation and platform limits](windows-audio.md) are documented separately.

The scope rules are explicit:

- Switching between system and application modes stops the current route, retains receiver selection, clears the source choice and refreshes sources for the new mode. It does not resume a different scope automatically.
- Application enumeration never chooses an app on the person's behalf. Connecting requires an available source of the selected kind. A late scan for the previous scope cannot replace the current list.
- Changing to a valid source within the current scope preserves selected receivers and can restart an already active route around the new capture session.
- An exited or unavailable app never falls back to another app or the system mix. A restarted process has a new identity and must be selected again.
- Process-tree capture is not tab selection: tabs in one browser instance may be captured together. The source explanation makes that boundary visible.
- Application mode sets local mute off and disables **电脑静音**. Local playback continues; the app's output routing is unchanged. The core also rejects endpoint mute in this scope.

Source enumeration runs outside the UI dispatcher. `sources_refreshing` controls the dedicated refresh state and `source_message` supplies the scope-specific guidance. Source rows are rebuilt only when scope, selection, refresh state or source data changes; routine level snapshots do not reset an open picker. No application name or process ID is added to copied diagnostics.

## Volume stability

The reported movement had two interacting risks in the prior implementation: dynamic content measured inside a centered layout, and a snapshot that could overwrite the slider immediately after its debounced command was enqueued. The pending flag was cleared before the engine acknowledged the new value.

The replacement has four protections:

1. `PageContent` owns the workspace viewport width, and the transport dock owns its separate full-width row. A changing label cannot recenter the page or change the receiver panel's column width.
2. The percentage has a fixed 52 effective pixel width and tabular numerals. Status and transport controls also have reserved slots.
3. A local value remains authoritative while a pointer is held, while a command is pending, or while acknowledgement is outstanding. A matching snapshot releases that protection. A three-second acknowledgement timeout reports a recoverable error.
4. Receiver rows are keyed by ID and updated in place. State changes do not clear and rebuild the list. Frequently updated text and updater controls are assigned only when changed.

Slider changes update the percentage immediately. Commands debounce by 180 ms; pointer release flushes the final position. Native routed pointer events are observed with `handledEventsToo` without replacing the slider's own capture or interaction. With an available core, the slider is usable before selecting a receiver so starting volume can be chosen in advance.

Native UI Automation bounds require a specific distinction here. In the [WinUI 1.8.1 slider template](https://github.com/microsoft/microsoft-ui-xaml/blob/winui3/release/1.8.1/src/controls/dev/CommonStyles/Slider_themeresources.xaml), the thumb's outer border has a negative two-pixel margin. At an endpoint it therefore extends outside the track. The inherited [automation peer](https://github.com/microsoft/microsoft-ui-xaml/blob/winui3/release/1.8.1/src/dxaml/xcp/dxaml/lib/FrameworkElementAutomationPeer_partial.cpp) obtains global bounds from an element whose [inner bounds include its children's outer bounds](https://github.com/microsoft/microsoft-ui-xaml/blob/winui3/release/1.8.1/src/dxaml/xcp/core/core/elements/uielement.cpp). A two-pixel endpoint change in the slider peer rectangle is not itself evidence of a layout shift.

`VolumeSliderHost` reserves that two-pixel horizontal outset inside a transparent, stable slot. Geometry assertions measure this slot, the label and surrounding panels; the real slider still receives range-value and pointer input, and its visual bounds are recorded separately. This keeps the native thumb and interaction behavior without treating its moving decoration as page movement or broadly relaxing the layout tolerance.

## Signal integrity

The visible UI polls a nonblocking snapshot approximately every 33 ms; a hidden tray window polls about four times a second. Commands enqueue core work rather than synchronously performing pairing or socket operations on the UI thread.

Meters and the waveform envelope use actual outgoing PCM, including a temporary test tone. The core supplies a 64-bin peak envelope rather than signed time-domain samples. The instrument draws 64 persistent bars on a fixed −60 to 0 dB scale; mirroring around the baseline is a display convention, not another signal or synthetic animation. Bar magnitude is `clamp((20 × log10(peak) + 60) / 60, 0, 1)` and does not apply automatic gain. Bars are hidden below the displayed range. The caption identifies the peak envelope and its scale.

Separate capture status describes the real Windows source and is unaffected by the tone. Displayed level text uses `20 × log10(amplitude)`, with silence shown as `−∞ dB`. Values are validated and clamped at the boundary. Empty production states contain no synthetic waveform or sample receiver. UI-only fixtures, when used for visual verification, are explicitly marked and cannot establish acoustic output.

Capture states distinguish idle, waiting, signal, quiet, silent and error. Silent-input help points to the playback output and local-mute setting. A test tone is a three-second diagnostic action on receivers already streaming; it does not assert that sound was heard. Packet counts and timing exchanges are transport evidence, not acoustic proof or latency measurements.

Windows animation preferences govern decorative fades, the spinner and waveform. Turning off the waveform leaves numeric and meter information. System/light/dark preferences are retained; high contrast uses system resources.

## Multiple receivers and authentication

The core supports eight selected receivers. Selection and state are separate fields. Adding a target while another streams starts only the new target; removing one stops only that target. Retry can address a failed receiver. Changing a source within the same scope preserves selected receivers and reconnects active routes around the replacement capture session; changing scope follows the explicit stop-and-select rules above.

Each independent target receives the complete stereo stream. This does not implement Apple's native HomePod stereo pairing or a software left/right assignment. Existing Apple stereo pairs are not yet verified with real hardware. The interface must not present an unimplemented grouping switch; [stereo.md](stereo.md) records the support boundary and required hardware evidence.

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

The additive 0.4.0 source fields are `source_mode`, `sources_refreshing` and `source_message`; source entries carry their `kind`. The native model defaults absent `source_mode` to `endpoint`. These fields describe scope and readiness without exposing process handles across the ABI.

| Command | Fields beyond `type` |
| --- | --- |
| `refresh` | None |
| `refresh_sources` | None; refresh the current source scope |
| `select_source_mode` | `mode`: `endpoint` or `application` |
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

### 0.4.0 source and release gates

Source `7c5133e006ec0be13017ec21084680518c95082f` passed both jobs and every gate in [CI 39583891772](https://github.com/chummumm/airloom/actions/runs/39583891772). This includes regular tests, the explicitly invoked Windows process-loopback API test, native build and packaging, the complete native UI smoke, and independent updater/process lifecycle checks. The screenshots reviewed and stored for 0.4.0 come from that run's final artifact.

[Windows smoke](../scripts/smoke-windows.ps1) operates the real WinUI executable from the portable archive through .NET UI Automation and checked `SendInput`. It records 15 volume measurements at 0/9/10/99/100 across 880 × 680, 1100 × 800 and 1600 × 1000, acknowledgement retention, actual pointer drag, settings/back navigation, read-only update checking, restricted diagnostic clipboard content and both themes. At 880 × 680 the first two receiver rows and persistent dock are visible before scrolling. The management footer can be partially clipped at the ordinary outer scroll boundary, matching the preceding layout; the dock does not overlap the scroll viewer.

### Native presentation and interaction

Final light/dark application home and settings, native unavailable-output layouts, compact populated home and dark button states were inspected directly. Source names, primary/secondary labels and instrument text fit without a new blocking truncation. The 64 envelope bars stay inside their guides. The primary button retains its 44-pixel height and stable bounds through physical hover, press and release, with visibly distinct dark-theme resting, hover and pressed fills.

The source fixture starts with two streaming receivers in system mode. Switching to app mode stops capture and transmission, clears the source selection and disables playback until an explicit app is chosen. A physical primary-button click starts the selected synthetic app. Refresh preserves that selection and playback; returning to system mode stops the route while preserving receivers. Local mute is cleared and disabled in app mode. These outcomes are checked through the real native command path and fixture diagnostics.

`SetFocus` plus `HasKeyboardFocus` verifies **UIA logical focus**. The corresponding screenshot has no visible ring change and does not establish keyboard-focus-ring rendering. Standard native focus templates remain in place; no additional keyboard test is claimed.

The password and per-receiver cancellation checks passed UIA interaction: the native dialog is found, cancel is invoked, and the other two receivers remain streaming. The final `fixture-auth-dark.png` capture itself shows the ordinary home view, not the modal. It is retained with its actual provenance and is not presented as password-dialog visual proof.

[UiFixture.cpp](../scripts/fixtures/UiFixture.cpp) runs only as a replacement core DLL in an isolated temporary copy of the real portable app. The shipped core hash is checked unchanged. Its three app choices, three receivers, initial two streams, signal levels and password challenge are synthetic and not shipped. The awaiting-selection frame contains available synthetic apps; empty/ended-app screens were not visually verified. Neither the fixture nor packet counts prove real capture isolation, receiver transport or acoustic output.

Source automation IDs exercised by this flow include `SourceModePicker`, `SourcePicker` and `SourceRetryButton`. Existing IDs include `HomeView`, `DeviceRail`, `HomeScrollViewer`, `PageContent`, `SourcePanel`, `SignalPanel`, `TransportPanel`, `VolumeSliderHost`, `VolumeSlider`, `VolumeValue`, `SettingsButton`, `SettingsView`, `SettingsBackButton`, `UpdateStatusText`, `CheckUpdatesButton` and `CopyDiagnosticsButton`.

### Real Windows API and hardware boundaries

`process_loopback_activation_and_lifetime` creates its own silent process and checks activation, stale identity rejection, finite exact-zero received samples, refusal of endpoint mute, bounded cancellation, restarting the client and reporting target exit. The process path uses float32 capture and retains the strict sample assertion. The final run's [process-capture-api.txt](evidence/process-capture-api.txt) records successful completion. [windows-audio.md](windows-audio.md) owns the capture contract.

That test does not play two applications simultaneously or connect a physical HomePod. Audible app isolation, browser child-process behavior, other application models, native HomePod stereo and multiroom synchronization require hardware validation. High-contrast rendering, the complete receiver error/reconnect matrix and new offline hide/restore interactions are outside this screenshot claim. The [resource contrast calculation](design/contrast-check.json) is a calculation for application-owned surfaces, not a rendered native accessibility audit. No web inspector has certified this XAML interface.

### Historical evidence

The 0.3.0 shell passed [CI 39573924973](https://github.com/chummumm/airloom/actions/runs/39573924973), source `e80e6f9af8707216277fa8396dd351eaf33fad73`. Reduced-motion captures are retained separately as [light](evidence/fixture-reduced-motion-home-light.png) and [dark](evidence/fixture-reduced-motion-home-dark.png) from [run 39572112414](https://github.com/chummumm/airloom/actions/runs/39572112414), source `f2a914feec150f56e98e0f461b5088d3e54616a3`. That earlier run passed UI smoke but failed an updater readiness assertion later corrected in the harness; it is historical reduced-motion evidence, not the current release gate.

The user reported audible HomePod playback with 0.2.1 on 2026-10-21 after its [verified build](https://github.com/chummumm/airloom/actions/runs/39549954880), source `852bc217fa0b44e40dc3256564d951f39b1cb2d0`. That remains an attributed listening result for their setup, not a CI listening test or a claim of universal compatibility.

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
