# Sound-routing surface brief

**Mode: Operate.** This brief covers the native home and settings views.

## Job and constraints

Home answers: which speakers receive this computer's sound, and is the outgoing signal real? Its primary action is to connect or stop the selected route; volume and individual receiver selection remain directly available. Settings supports appearance, device visibility, updates and diagnosis without hiding the state of a running session.

Keep WinUI 3/C++ native controls and the existing Rust audio core. Preserve per-device authentication, multi-receiver independence, hidden-device recovery, the volume acknowledgement fix and orderly tray/update exit. The user reported that 0.2.1 now produces audible HomePod output and asked to replace the entire unattractive interface. The working audio path is the regression baseline; this is a visual-world replacement, not another protocol project.

## Incumbent critique

Actual 0.2.1 screenshots at 880/1100/1600 pixels and the settings view were reviewed. The full-height receiver rail, two disconnected white panels and broad source form look like a generic diagnostic dashboard. At wide size, content ends near the middle of the window and leaves a large unused lower area. Source, target and connection state are repeated in several captions. Receivers have little presence while secondary parameters stretch across the page. Settings uses tall panels with explanatory labels far from their controls.

The earlier layout and acknowledgement corrections remain valid, but geometric stability alone did not create a considered product. Recoloring the same arrangement would retain the cause of the user's rejection.

## Data and jobs

- Airloom is a Windows desktop tool for routing existing system audio to real local-network receivers.
- Receivers have an ID, visible name/model, online/selected state, authentication state and per-device transfer status.
- The core supports up to eight selected targets; one waiting for a password must not stop the others.
- The source is an actual Windows playback endpoint, not a virtual sound card.
- The most frequent actions are connect/disconnect, choose receivers and adjust shared volume.
- A real 64-bin peak envelope and left/right levels are available, including a short diagnostic test tone.
- Sample rate, elapsed session time and packet count support diagnosis, but do not prove acoustic output or latency.
- Device names may be long; state transitions may occur while the person is dragging the slider.
- Appearance and Windows startup are local preferences; reduced motion and high contrast remain meaningful.
- Updates and diagnostics are maintenance actions with their own loading, success and failure states.
- No track metadata or cover art is available; the UI must not invent a music library or now-playing song.

## Replacement direction

The taste app-screen draw selected `achromatic-ledger`, seed **192244604**. The face is a low-chroma silver meter ground (`#F2F4F7`), white working surfaces, ink controls and one blue real-signal hue (`#1D5FBF`). Dark mode uses neutral black/gray surfaces. The principal signature is a dark instrument display containing the real waveform and levels, paired with a compact source-to-receiver route.

One shell redraw selected `no-sidebar`: the original `open-plane` navigation sidebar had no actual destinations here and reproduced the rejected full-height column. Native Segoe typography was explicitly retained. The generic greeting/count-tile pattern was replaced with the actual audio job. No extra shell redraws or fabricated draws were used. See [app-direction.json](../app-direction.json).

Composition: compact source route above the instrument; receiver panel beside it at 1050 effective pixels and above, or above it when narrower so receiver selection remains visible on opening; a unified transport dock remains at the bottom. Settings uses compact aligned native groups. The title bar is 48 effective pixels; the default window is 1120 × 780, with an 880 × 680 minimum. [DESIGN.md](../../../DESIGN.md) owns the resources, component dimensions and interaction rules.

## Context and skill record

The requested impeccable and taste skills informed this work. The taste app-screen run was used and its actual draw saved. Its browser inspector does not validate a native WinUI app and is not presented as native evidence.

The impeccable context launcher was attempted and unavailable; direct context reading was announced. Existing PRODUCT.md, DESIGN.md, source and current Windows screenshots were inspected. The installed bundle exposes its main guidance but does not contain `reference/new-work.md` or `reference/craft-floor.md`; exact resource reads also failed. Those unavailable references were not treated as completed checks. Available redesign, product-UI, native interaction, accessibility and rendered-review guidance was applied.

## Acceptance evidence

The actual WinUI captures from [CI run 39573924973](https://github.com/chummumm/airloom/actions/runs/39573924973), source `e80e6f9af8707216277fa8396dd351eaf33fad73`, were visually reviewed. That run passed both CI jobs and every release gate, including all native UI smoke stages and the separate updater lifecycle checks.

| Requirement | Observed evidence and boundary |
| --- | --- |
| A complete replacement composition | Native size captures show the source strip, dark instrument, right-side or compact-first receivers and persistent dock; the rejected left navigation-like rail is gone |
| Audio behavior preserved | The user's audible 0.2.1 baseline remains explicitly attributed; the CI desktop has no physical HomePod and the screenshots are not an audio test |
| Stable direct volume control | Native smoke passed slot/label/panel bounds and value retention at 0/9/10/99/100 across three sizes, actual `SendInput` drag and populated-fixture volume changes |
| Responsive workspace | 880 × 680, 1100 × 800 and 1600 × 1000 native captures; compact receiver actions are visible before scroll and the transport stays available |
| Product identity | Airloom mark/name, native source selector, per-receiver status and distinct instrument; populated images label their synthetic source data |
| Representative states | Real unavailable-source/empty-discovery state plus synthetic three-device, two-streaming and password-dialog states; receiver error/reconnect states are not part of this screenshot set |
| Theme quality | Actual light/dark home and settings captures; enabled switches and primary controls are monochrome and readable; high-contrast rendering was not captured |
| Settings usability | Native settings/back, read-only update check and diagnostic clipboard passed; populated visibility rows fit, without claiming a new offline hide/restore interaction test |
| Motion and signal display | Final native light/dark captures show the animation-enabled 64-bar envelope without clipping or overlap; separately retained reduced-motion captures keep numeric levels and meters visible |
| Password interaction | Target name, password input, unchecked remember option, pairing-code action and cancel fit in the native dialog; fixture cancellation preserved the other two selected receivers |
| Exit differs from hiding | Final CI passed the independent updater/process lifecycle gate after a bounded test-only window-caption readiness correction; prior release remains separately attributed baseline evidence |

The `fixture-*` images come from the same native executable with a CI-only replacement core DLL in a separate temporary archive copy. Receiver identities, signal levels, counts and authentication challenges are synthetic. The smoke checks the shipped core hash remains unchanged; fixture data and the replacement DLL are not shipped. This establishes native presentation and interaction, not real discovery, audio transport, hearing a test tone or multiroom synchronization.

**Visual verdict:** the final light/dark, compact, populated-settings and authentication frames pass the reviewed presentation checks. No blocking clipping, unreadable state, inaccessible initial receiver action or obscured dialog action was found. The monochrome resource-order correction is visibly effective. The 64 blue envelope bars stay within the instrument guides and leave its caption, meters and title clear. Native proof records `active_waveform_mode_verified=true`; the CI-only normal-animation setting is restored after capture. Both jobs and the independent lifecycle checks passed in the final build.

Reduced-motion presentation remains documented by [the light capture](../../evidence/fixture-reduced-motion-home-light.png) and [the dark capture](../../evidence/fixture-reduced-motion-home-dark.png) from [run 39572112414](https://github.com/chummumm/airloom/actions/runs/39572112414), source `f2a914feec150f56e98e0f461b5088d3e54616a3`. That run passed UI smoke, while its independent updater test failed during window-caption readiness and was subsequently corrected in the test harness. It is retained for reduced-motion evidence rather than used as the final release gate.

Resource calculations and XML validity remain prechecks rather than rendering or interaction proof. HomePod output was confirmed by the user for 0.2.1; other models, firmware and stereo/multiroom behavior retain their separate hardware verification limits.
