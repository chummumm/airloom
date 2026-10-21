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
- True waveform samples and left/right levels are available, including a short diagnostic test tone.
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

| Requirement | Evidence needed |
| --- | --- |
| A complete replacement composition | Actual first-screen screenshots visibly distinct from the rejected left-rail/white-card arrangement |
| Audio behavior preserved | Audio/protocol regression checks; user's 0.2.1 audible baseline remains explicitly attributed |
| Stable direct volume control | Slot/label/panel bounds at 0/9/10/99/100 and actual pointer drag with acknowledgement retention |
| Responsive workspace | 880 × 680, 1100 × 800 and 1600 × 1000 native screenshots; receiver panel placement and dock availability |
| Product identity | Airloom mark/name, real Windows source, genuine receiver states and instrument display |
| Representative states | Empty, selected/playing, multiple devices, authentication and receiver/source error; UI-only fixtures clearly identified |
| Theme quality | Actual light/dark captures; high contrast retains system resources |
| Settings usability | Native settings/back, update check, diagnostic clipboard and device visibility interactions |
| Exit differs from hiding | Existing real-process/tray and Restart Manager checks |

**New-design status: verification pending.** The previous release's successful CI and screenshots are baseline evidence only. New screenshots must come from the rebuilt native executable; a styled HTML image cannot replace this gate. Resource calculations and XML validity are useful prechecks, but do not certify rendering or interaction. HomePod output was confirmed by the user for 0.2.1; other models, firmware and stereo/multiroom behavior retain their separate hardware verification limits.
