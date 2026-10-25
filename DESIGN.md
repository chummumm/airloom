# Airloom visual system

## Current design: route stage (unreleased)

The home view is one **stage** card and one **speaker rail** on Mica, with a layered dock beneath them. It keeps WinUI 3, Fluent controls, the system accent and every behavior contract below; only composition, visuals and motion change. Windows CI screenshots have not yet been reviewed for this design.

| Region | Rule |
| --- | --- |
| Stage header | Scope (`系统声音` / `指定应用`) and source as native ComboBoxes, an icon refresh action, and a state pill on the right |
| Headline | The chosen speakers (24 px compact, 28 px wide), one subline for what is happening, one caption line for what the capture actually hears |
| Route | This computer (or the chosen app) as an 80 px tile on the left; one curve per chosen speaker (up to four) to a 52 px node on the right |
| Signal | The 64-bin engine envelope above a dotted baseline, L/R meters beside it, transfer figures in one divided row |
| Speaker rail | 340 px beside the stage at 1050 px and wider, above it below that; rows of check, avatar, name, live state and inline next step |
| Dock | 84 px on a layer with a top divider: source summary, test tone, a 164 × 44 accent pill, elapsed time, volume with a fixed 52 px value slot, local mute |

The route is drawn from real state only. A dotted line means chosen but idle; marching dashes mean connecting; a solid accent line means connected; short caution or critical dashes mean the speaker needs the person. Particles travel a connected line only while the capture reports a signal (or the test tone plays), and fade out during silence or a source switch while the line stays.

Motion follows WinUI durations (167 / 250 / 333 ms) with a decelerating curve: a connected route draws in over 500 ms, a joining node pops once (0.86 → 1.06 → 1, 450 ms), settings drills in from 96 % and home returns from 102 %, and the primary button swaps its icon in one fixed slot. Continuous motion — route particles, node halos, the connecting ring, row equalizers, the discovery radar and the live pill — exists only while the state it explains is true and the home view is visible. Hiding or minimizing the window and opening settings stop those animations even if the receiver state has not changed. Silent particles stop after their 150 ms fade. Windows' animation setting stops all of it; high contrast also drops halos and particles. Line styles, state text, levels and figures remain in every case.

When one receiver is playing and another requests credentials, the primary action remains **停止投放** and stops the session. The credential action belongs only to the receiver named by the active authentication request; queued requests cannot open a different receiver's prompt.

Smoke-test contracts are unchanged: all automation IDs, the 44 px primary action height, fixed volume geometry, the non-scrolling dock, wide/compact placement and the compact first-receiver visibility rule.

## Interaction contract: 0.4.3 (still current)

The 0.4.2 visual baseline is recorded in [the 0.4.2 interface specification](docs/design/ui-0.4.2.md) and superseded by the route stage above. The earlier silver-workspace tokens and permanently dark instrument described below are retained as historical design and evidence, not instructions to restore the previous appearance.

The 0.4.3 interaction contract preserves active receiver sessions during discovery refresh and source/scope changes. Windows capture alone is replaced; the established connection can wait with zero audio until an application is explicitly selected. The stop action and independent test tone remain available while connected. A capture transition may have a brief silent interval.

Volume comes from the selected receivers, with no locally saved value and no automatic write at startup or connection. The fixed label slot shows a percentage, `不同` for mixed readings, or `—` when readings are incomplete. Only explicit slider input changes the current targets. The slider is disabled without a selected connected/connecting target; an established target with an unknown reading still permits explicit adjustment. The preview label is removed. See [0.4.3 behavior and verification boundaries](docs/releases/0.4.3.md); historical stop-on-scope-change and preconnection-volume behavior below no longer applies.

## Historical design: 0.3.0–0.4.1

Airloom is a native Windows audio-routing app. Its interface puts the sound route, the receiving speakers and direct playback control in one compact workspace. The 0.3.0 redesign replaces the full-height left receiver rail and disconnected white cards from 0.2.1.

The 0.4.0 work refines the native button hierarchy and adds an explicit choice between system sound and a selected application's sound. It keeps the established composition, instrument and responsive rules. Final source, native visual evidence and verification boundaries are recorded below.

The visual world is **a silver audio workspace with an inset instrument display**. A low-chroma silver ground, white working surfaces and ink controls form the shell. One dark neutral display contains the actual signal. Blue belongs to real audio and connection state, not navigation or decoration. The display is the product's signature element: an instrument showing this computer's sound, never invented album art or a simulated playing state.

## Composition

| Region | Rule |
| --- | --- |
| Native title bar | 48 effective pixels; the Airloom mark and name; native caption controls |
| Sound route | Explicit system/app scope, source selection, route context and session status above the instrument |
| Main workspace | Instrument first; a bounded receiver panel to its right when space permits |
| Compact workspace | Receiver panel precedes the instrument so selection is visible on opening |
| Transport dock | Persistent bottom position, with connection action, stable volume control and local-output mute |
| Settings | Compact groups of aligned native rows; labels stay close to their controls |
| Ordinary panels | White/neutral fill, one subtle boundary, 8 effective pixel radius |
| Instrument | Distinct dark display, 16 effective pixel radius, 24 effective pixel inset |
| Transport dock | 12 effective pixel radius, 20 × 16 effective pixel inset |
| Main controls | Native buttons: 44-pixel primary actions with 10-pixel radius; 40-pixel secondary and 36-pixel compact/quiet actions with 8-pixel radius |

The normal opening size is 1120 × 780 effective pixels, clamped to the monitor work area. The minimum remains 880 × 680. At 1050 effective pixels of actual root content width, the receiver panel can sit beside the instrument; smaller widths put it above the instrument, keeping speaker selection in the first viewport. The dock remains available while workspace content scrolls. A native `UserControl` owns visual states; HWND physical pixels are not used as the layout breakpoint.

There is no permanently empty navigation rail. Home and settings are the two working views, with explicit actions between them. The window can grow, but essential controls should not spread across an unbounded row or leave the lower half of the window without a purpose. Receiver rows have their own scrolling region when necessary. The compact receiver panel reserves 330 effective pixels. The transport action row reserves 88, and the source detail row reserves 36; loading, unavailable-source and authentication controls do not collapse these working slots.

The volume number retains a fixed 52 effective pixel tabular slot. Dynamic state text and receiver names cannot recenter the workspace. The slider's native thumb outset stays inside its stable slot. Layout and acknowledgement protection remain part of the design, not optional polish.

The source scope selector reserves 134 effective pixels, and the source name takes the remaining flexible column. Source status keeps a separate 154-pixel column. The refresh action reserves 104 pixels so its loading label cannot move its left edge. The primary transport action occupies 152 × 44 effective pixels, with the existing secondary authentication action inside the reserved 88-pixel dock row.

## Type and object identity

Use **Segoe UI Variable Text**, with Windows' native language fallback. Native type was explicitly retained in the taste draw. Body text is 14 effective pixels; secondary explanations are 12; section titles are 16; the principal source/receiver identity is 24–28. Use regular and semibold weights, with tabular numerals for volume, time, sample rate and dB.

Keep the original Airloom vector mark. Use one native outline icon family for controls. A device's actual name, selection and individual connection state carry its identity. A hardware outline may identify a receiver, but do not turn every row into a colored icon tile. System audio has no reliable cover art, track name or artist in the current model, so none may be fabricated.

## Color resources

Application-owned colors live in [App.xaml](native/Airloom/App.xaml). The default preference remains **follow Windows**; existing light/dark choices are preserved. Dark surfaces are neutral, not navy. High contrast maps application resources to Windows system colors and retains the platform's native control states.

| Resource | Light | Dark | Role |
| --- | --- | --- | --- |
| `AppCanvasBrush` | `#F2F4F7` | `#101010` | Silver or neutral window ground |
| `PanelBrush` | `#FFFFFF` | `#1C1C1C` | Receiver, settings and transport surfaces |
| `InsetBrush` | `#E9EDF2` | `#303030` | Quiet inset and track |
| `HairlineBrush` | `#DEE2E7` | `#333333` | Surface boundaries |
| `InkBrush` / `BrandBrush` | `#20242A` | `#F5F5F5` | Text and monochrome selection emphasis |
| `MutedBrush` | `#626A73` | `#B8B8B8` | Supporting text |
| `BrandSoftBrush` | `#E5E9EF` | `#303030` | Neutral receiver selection |
| `AudioBrush` | `#1D5FBF` | `#8BB6FB` | Actual signal and active connection state |
| `WarningBrush` | `#80520A` | `#E8BE76` | Attention required |
| `DangerBrush` | `#B3261E` | `#FF9696` | Failed operation |
| `PrimaryActionBrush` | `#20242A` | `#F5F5F5` | Main control |
| `PrimaryActionTextBrush` | `#FFFFFF` | `#171717` | Main control text |
| `ControlOutlineBrush` | `#89939E` | `#777777` | Stronger control boundary |
| `StageBrush` | `#151515` | `#080808` | Instrument ground |
| `StageInkBrush` | `#F7F7F7` | `#F7F7F7` | Main instrument text |
| `StageMutedBrush` | `#B4B4B4` | `#B4B4B4` | Instrument labels |
| `StageRuleBrush` | `#333333` | `#333333` | Instrument grid and separation |
| `StageAudioBrush` | `#8BB6FB` | `#8BB6FB` | Actual waveform and levels |
| `StageInsetBrush` | `#292929` | `#292929` | Instrument track |

Do not apply the audio hue to navigation, a whole sidebar, every button or a card ground. The monochrome primary button inherits the native accent-button template with explicit normal, pointer, pressed and disabled resources. Native toggles also use monochrome on-state resources. This preserves recognizable platform behavior with monochrome working controls. Native focus indicators retain their platform focus color.

No gradients, glass, glows or large resting shadows. Contrast comes from the instrument against the silver workspace, typography and careful alignment. The dark instrument is an intentional audio component, not a dark sidebar on a light app.

## Button hierarchy (0.4.0)

Buttons are native controls with a consistent hierarchy, not independent visual treatments for each page. The existing WinUI templates provide pointer, pressed, disabled and focus behavior. Application resources supply neutral fills and boundaries in light/dark themes; high contrast uses Windows system colors. No replacement control template or ornamental animation is introduced.

| Style | Native base and dimensions | Use |
| --- | --- | --- |
| `PrimaryButtonStyle` | `AccentButtonStyle`; minimum height 44, radius 10, padding 18 × 10 | Start/stop the sound route and install an available update |
| `SecondaryButtonStyle` | `DefaultButtonStyle`; minimum height 40, radius 8, padding 14 × 8 | Test tone, check updates, copy diagnostics and remove saved credentials |
| `CompactButtonStyle` | Secondary; minimum height 36, padding 12 × 6 | Refresh sound sources and hide/restore individual receivers |
| `QuietButtonStyle` | Native `SubtleButtonStyle`; minimum height 36, radius 8, padding 12 × 6 | Cancel, manage devices, rediscover and exit |
| `IconButtonStyle` | Quiet; 36 × 36, no padding | Back and receiver refresh, with an accessible name and tooltip |
| `ToolbarButtonStyle` | Quiet; minimum width 76, padding 10 × 6 | Settings in the native title region |

Text uses 14-pixel semibold Segoe; adjacent glyphs use 16 pixels with an 8-pixel gap. `ButtonLabelStyle` and `ButtonGlyphStyle` leave foreground inheritance intact so nested content can follow the native disabled state. Quiet actions have no resting outline; secondary actions have a restrained surface and boundary; the route action carries the strongest contrast. Disabled controls remain present where their disappearance would move the dock or source strip.

Native system focus visuals remain enabled. Pointer/press feedback uses the native template's transitions and the existing Windows animation preference. Actual 0.4.0 light/dark, disabled and compact presentation was reviewed in the UI run identified below; dark-theme hover and pressed frames show distinct, stable feedback. UIA `SetFocus` verifies logical focus only. The focus capture does not establish a visible keyboard focus ring.

## Sound-source scope (0.4.0)

The route begins with **系统声音** or **指定应用**, followed by the corresponding source selector. System mode captures the shared playback mix of the chosen Windows output. Application mode captures an explicitly selected process and its children through Windows' process loopback API. It creates no virtual sound card and does not change the application's original output routing. The [capture contract](docs/windows-audio.md) owns platform details.

In the historical 0.4.0 behavior, switching scope stopped the current route and preserved receiver selections; a valid source change could restart the route. This is superseded by the 0.4.3 connection-preserving contract above. Application mode still starts with no app chosen; its empty state tells the person to let the app play once and refresh the list. Paused sessions can remain selectable. A process that exits never causes another app or the system mix to be selected automatically; a restarted instance requires a new selection.

The scope explanation stays beside the selector: a browser instance's tabs may be captured together, because this is a process-tree choice rather than an individual-tab selector. The **电脑静音** control is disabled in application mode and its explanation states that local playback continues. An endpoint-wide mute would affect other applications. Source loading, unavailable-source and recovery text reuse the stable source strip rather than opening a second settings flow.

Multiple selected receivers still receive the complete stereo stream independently. This is distinct from an Apple HomePod stereo pair. The UI must not expose unimplemented pair creation, dissolution or left/right assignment, or imply that an existing Apple pair has been hardware-verified. See [stereo support boundaries](docs/stereo.md).

## Home states and signal truth

Home answers: **which speakers receive this computer's sound, and is an actual signal being sent?** Source choice and selected receivers remain real engine state. A receiver can be available, selected, connecting, waiting for authentication, streaming, reconnecting, offline or failed. One receiver awaiting a password cannot conceal another receiver's session.

The instrument is always framed. Idle and silent states have a quiet baseline and a short next step; an active waveform is drawn only from actual PCM. Numeric levels remain available when animation is disabled. Sample rate, session time and packet count are secondary measurements, grouped closely rather than spread across the full window.

Errors appear once near the failed operation. A source failure belongs beside the source selector and rescan action. Receiver failures belong to that receiver or the connection notice. Diagnostics are available from settings. Neither packet counts nor a lit status mark is labeled as an acoustic test or a measured latency.

The user reported audible HomePod playback with 0.2.1 on 2026-10-21. That observation establishes this user's working baseline. It does not certify every receiver model, firmware, password policy, stereo group or multiroom synchronization behavior.

## Settings hierarchy

Settings supports maintenance without disrupting a running route. Keep update status/actions and connection diagnostics easy to find. Theme, waveform, startup, device visibility and explicit exit are native rows with consistent spacing and nearby explanations. A description is shown once and wraps instead of being clipped.

Update downloading/launching/error states may expand their own group. Hidden receivers remain recoverable even when offline. Hiding a selected receiver explicitly stops that target. Credential values and network addresses do not appear in copied diagnostic text. Ordinary close and explicit exit remain separate user actions.

## Interaction and motion

Volume follows the pointer immediately. The local target remains authoritative while dragging, awaiting dispatch or awaiting core acknowledgement. Pointer release flushes the final value. Old snapshots cannot move the slider back. Receiver rows preserve their identity while labels and states update.

Motion gives feedback: a press, a selection, a state transition or opening settings. It does not introduce staggered card entrances or ornamental motion. Continuous sound visualization comes only from engine data. (The route stage above defines which continuous state motion is allowed and when it stops.) Windows reduced-motion settings and the waveform preference are honored; signal values and controls remain useful with motion off.

Focus, keyboard behavior, accessible names and status announcements remain native. Selection and errors must be legible without color. The instrument's text uses its own foreground resources so the light theme never puts dark text onto its dark display.

## Verification

The 0.4.0 source `7c5133e006ec0be13017ec21084680518c95082f` passed both jobs and all gates in [CI 39583891772](https://github.com/chummumm/airloom/actions/runs/39583891772), including the Windows build/package, native UI smoke, updater lifecycle and real process-loopback API checks. The visual review used actual screenshots from that run's native artifact.

**Visual verdict:** the final light/dark application home and settings, native unavailable-output layout, compact populated view and dark button-state frames have no new blocking clipping, obscured action or unreadable state. Source scope and application names fit their reserved slots. The 64-bar envelope stays inside the instrument and leaves its labels and meters clear. At 880 × 680 the first two receiver rows and fixed dock are visible before scrolling; the management footer can meet the ordinary outer scroll boundary, as in the previous baseline.

The native proof records volume geometry and retention at 0/9/10/99/100 across 880 × 680, 1100 × 800 and 1600 × 1000, actual `SendInput` drag, settings/back, update checking, restricted diagnostic copy and both themes. The application fixture verifies explicit selection, stop on scope change, refresh retaining selection, disabled local mute and the active waveform. The primary button retains its 44-pixel height and position under actual pointer input; dark normal, hover and pressed frames show distinct feedback.

UIA `SetFocus` and `HasKeyboardFocus` verify **logical focus**. The focus frame does not show a visible change around the button and is not evidence of a keyboard focus ring. Native focus templates remain enabled. Password-dialog opening and cancelling only the challenged receiver passed UIA interaction checks. The final `fixture-auth-dark.png` frame shows the home view rather than the modal, so it is not used as visual proof of the password dialog.

Populated `fixture-*` frames use the same WinUI executable with a CI-only replacement core DLL in an isolated temporary copy. Source applications, receiver identities, levels and authentication challenges are synthetic. The smoke checks the shipped core's hash remains unchanged; the replacement and its data are not distributed. The awaiting-selection frame contains three available synthetic applications; empty and ended-app screens were not visually verified.

The separate Windows process-loopback test uses an owned process with no audio stream. It passed activation, stale identity rejection, finite exact-zero received samples, mute refusal, bounded stop, client restart and process-exit handling. This is API/lifecycle evidence. It does not establish isolation between two simultaneously playing applications, browser child-process behavior or audible HomePod playback. No physical HomePod is connected to CI. Native stereo pairs and multiroom synchronization retain their hardware-verification boundaries.

High-contrast rendering and the complete receiver error/reconnect matrix were not visually verified. The [saved resource calculation](docs/design/contrast-check.json) checks 54 application-owned text/surface pairs, including the instrument, with a minimum ratio of 4.501:1; it is not a rendered accessibility audit of all native control states.

### Historical baseline and reduced motion

The 0.3.0 shell passed [CI 39573924973](https://github.com/chummumm/airloom/actions/runs/39573924973), source `e80e6f9af8707216277fa8396dd351eaf33fad73`. It remains a historical regression baseline rather than evidence for the new application scope.

The separately retained reduced-motion [light](docs/evidence/fixture-reduced-motion-home-light.png) and [dark](docs/evidence/fixture-reduced-motion-home-dark.png) captures come from [run 39572112414](https://github.com/chummumm/airloom/actions/runs/39572112414), source `f2a914feec150f56e98e0f461b5088d3e54616a3`. They show the explanatory baseline with numeric levels and meters retained. That earlier run passed UI smoke but failed an independent updater readiness test subsequently corrected in the test harness; these files are reduced-motion evidence, not the 0.4.0 release gate.

## Direction provenance

The taste app-screen draw selected **achromatic-ledger**, seed **192244604**, on 2026-10-21. The selected face is the low-chroma silver meter ground `#F2F4F7`; its main data hue is `#1D5FBF`. The signature is the actual waveform instrument and compact Windows-to-speaker route. Native Segoe type was retained with `--keep type`.

One shell redraw selected **no-sidebar**. The initial open-plane navigation sidebar had no real navigation destinations in this small app and reproduced the rejected full-height column. The drawn greeting/count-tile pattern was adapted to the native audio job: no greeting, vanity counters or invented content. The receiver panel is an operational object list, not navigation. These adaptations preserve the style's monochrome chrome and one data hue.

The exact draw is saved in [app-direction.json](docs/design/app-direction.json). The surface brief records the incumbent critique, available skill guidance and evidence boundaries.

The 0.4.0 button work is a component refinement within this incumbent direction. No new taste draw or additional shell redraw was performed.
