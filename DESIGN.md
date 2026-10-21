# Airloom visual system

Airloom is a native Windows audio-routing app. Its interface puts the sound route, the receiving speakers and direct playback control in one compact workspace. The 0.3.0 redesign replaces the full-height left receiver rail and disconnected white cards from 0.2.1.

The visual world is **a silver audio workspace with an inset instrument display**. A low-chroma silver ground, white working surfaces and ink controls form the shell. One dark neutral display contains the actual signal. Blue belongs to real audio and connection state, not navigation or decoration. The display is the product's signature element: an instrument showing this computer's sound, never invented album art or a simulated playing state.

## Composition

| Region | Rule |
| --- | --- |
| Native title bar | 48 effective pixels; the Airloom mark and name; native caption controls |
| Sound route | Compact source selection, route context and session status above the instrument |
| Main workspace | Instrument first; a bounded receiver panel to its right when space permits |
| Compact workspace | Receiver panel precedes the instrument so selection is visible on opening |
| Transport dock | Persistent bottom position, with connection action, stable volume control and local-output mute |
| Settings | Compact groups of aligned native rows; labels stay close to their controls |
| Ordinary panels | White/neutral fill, one subtle boundary, 8 effective pixel radius |
| Instrument | Distinct dark display, 16 effective pixel radius, 24 effective pixel inset |
| Transport dock | 12 effective pixel radius, 20 × 16 effective pixel inset |
| Main controls | Native buttons, 36–44 effective pixels high, 8 effective pixel radius |

The normal opening size is 1120 × 780 effective pixels, clamped to the monitor work area. The minimum remains 880 × 680. At 1050 effective pixels of actual root content width, the receiver panel can sit beside the instrument; smaller widths put it above the instrument, keeping speaker selection in the first viewport. The dock remains available while workspace content scrolls. A native `UserControl` owns visual states; HWND physical pixels are not used as the layout breakpoint.

There is no permanently empty navigation rail. Home and settings are the two working views, with explicit actions between them. The window can grow, but essential controls should not spread across an unbounded row or leave the lower half of the window without a purpose. Receiver rows have their own scrolling region when necessary. The compact receiver panel reserves 330 effective pixels. The transport action row reserves 88, and the source detail row reserves 36; loading, unavailable-source and authentication controls do not collapse these working slots.

The volume number retains a fixed 52 effective pixel tabular slot. Dynamic state text and receiver names cannot recenter the workspace. The slider's native thumb outset stays inside its stable slot. Layout and acknowledgement protection remain part of the design, not optional polish.

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

Motion gives feedback: a press, a selection, a state transition or opening settings. It does not introduce staggered card entrances or ornamental motion. Continuous sound visualization comes only from engine data. Windows reduced-motion settings and the waveform preference are honored; signal values and controls remain useful with motion off.

Focus, keyboard behavior, accessible names and status announcements remain native. Selection and errors must be legible without color. The instrument's text uses its own foreground resources so the light theme never puts dark text onto its dark display.

## Verification

Native layout, theme and active-waveform visual review passed for source `e80e6f9af8707216277fa8396dd351eaf33fad73` in [CI run 39573924973](https://github.com/chummumm/airloom/actions/runs/39573924973). The reviewed evidence comes from the actual WinUI executable, not a web rendering. The source-error/empty home was checked at 880 × 680, 1100 × 800 and 1600 × 1000; home and settings were checked in light and dark themes. The compact view exposes receivers before scrolling, the dock stays available, text fits its working slots, and the enabled native switches now use the intended monochrome resources.

The run's `native-smoke.json` records successful volume geometry and value retention at 0/9/10/99/100 across all three sizes, actual `SendInput` drag, settings/back, read-only update checking, restricted diagnostic copy and both theme selections. The native fixture checks also passed populated volume retention, the password dialog, cancelling only the challenged receiver and `active_waveform_mode_verified=true`. The animation-enabled 64-bar envelope is visible in both themes, stays inside the instrument guides and leaves the caption, meters and title unobscured.

The `fixture-*` screenshots use the same WinUI executable with a CI-only replacement core DLL in an isolated temporary copy. They show three synthetic receivers, two selected streaming states, a third receiver's password challenge, populated settings and the compact view. Device names, levels, counts and authentication requests in those images are test data. The shipped core is checked for an unchanged hash; the replacement DLL and synthetic data are not distributed. These images establish presentation and interaction, not real discovery, AirPlay transport or acoustic output. No physical HomePod was connected to CI.

**Release verification passed:** both CI jobs and all release gates passed in run 39573924973, including the independent updater lifecycle checks. The earlier window-caption readiness race was corrected in the test harness; no product lifecycle change was needed. The normal-animation fixture setting is applied only to the disposable CI desktop and restored afterward.

Reduced-motion presentation was also visually reviewed in [run 39572112414](https://github.com/chummumm/airloom/actions/runs/39572112414), source `f2a914feec150f56e98e0f461b5088d3e54616a3`. The retained [light](docs/evidence/fixture-reduced-motion-home-light.png) and [dark](docs/evidence/fixture-reduced-motion-home-dark.png) captures show the explanatory baseline with numeric levels and meters retained. That earlier run passed UI smoke but failed the independent updater readiness test; it is reduced-motion evidence, not the final release gate.

No high-contrast screenshot or complete receiver error/reconnect matrix was reviewed in this run. The [saved resource calculation](docs/design/contrast-check.json) checks 54 application-owned text/surface pairs, including the instrument, with a minimum ratio of 4.501:1. These calculations do not certify native rendering or high-contrast behavior. The user's 0.2.1 listening result remains a separate, attributed hardware observation.

## Direction provenance

The taste app-screen draw selected **achromatic-ledger**, seed **192244604**, on 2026-10-21. The selected face is the low-chroma silver meter ground `#F2F4F7`; its main data hue is `#1D5FBF`. The signature is the actual waveform instrument and compact Windows-to-speaker route. Native Segoe type was retained with `--keep type`.

One shell redraw selected **no-sidebar**. The initial open-plane navigation sidebar had no real navigation destinations in this small app and reproduced the rejected full-height column. The drawn greeting/count-tile pattern was adapted to the native audio job: no greeting, vanity counters or invented content. The receiver panel is an operational object list, not navigation. These adaptations preserve the style's monochrome chrome and one data hue.

The exact draw is saved in [app-direction.json](docs/design/app-direction.json). The surface brief records the incumbent critique, available skill guidance and evidence boundaries.
