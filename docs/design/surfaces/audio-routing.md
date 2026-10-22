# Sound-routing surface brief

**Mode: Operate.** This brief covers the native home and settings views.

This file preserves the 0.3.0/0.4.0 design brief and its attributed screenshots. Current visual authority is the [0.4.2 native redesign](../ui-0.4.2.md). The [0.4.3 source and volume contract](../../releases/0.4.3.md) supersedes the historical stop-on-scope-change behavior below: source changes and rediscovery retain active receiver sessions, capture can wait silently for an explicit app choice, and volume is read from devices with writes only after user input. Historical acceptance rows are not evidence of these new behaviors.

## Job and constraints

Home answers: which speakers receive this computer's sound, and is the outgoing signal real? Its primary action is to connect or stop the selected route; volume and individual receiver selection remain directly available. Settings supports appearance, device visibility, updates and diagnosis without hiding the state of a running session.

Keep WinUI 3/C++ native controls and Rust ownership of audio behavior. Preserve per-device authentication, multi-receiver independence, hidden-device recovery, the volume acknowledgement fix and orderly tray/update exit. The user reported that 0.2.1 produces audible HomePod output and asked to replace the entire unattractive interface. That listening result remains the attributed hardware baseline for the 0.3.0 visual replacement.

For 0.4.0, the user rejected the button treatment and requested application-specific streaming and native HomePod stereo behavior. The current work refines buttons within the established visual direction and adds explicit process capture alongside endpoint capture. Native HomePod grouping is not implemented or represented as an available control; existing pair compatibility remains unverified. See [the stereo support boundary](../../stereo.md).

## Original 0.2.1 critique

Actual 0.2.1 screenshots at 880/1100/1600 pixels and the settings view were reviewed. The full-height receiver rail, two disconnected white panels and broad source form look like a generic diagnostic dashboard. At wide size, content ends near the middle of the window and leaves a large unused lower area. Source, target and connection state are repeated in several captions. Receivers have little presence while secondary parameters stretch across the page. Settings uses tall panels with explanatory labels far from their controls.

The earlier layout and acknowledgement corrections remain valid, but geometric stability alone did not create a considered product. Recoloring the same arrangement would retain the cause of the user's rejection.

## Data and jobs

- Airloom is a Windows desktop tool for routing a system output mix or an explicitly selected application's sound to real local-network receivers.
- Receivers have an ID, visible name/model, online/selected state, authentication state and per-device transfer status.
- The core supports up to eight selected targets; one waiting for a password must not stop the others.
- A source is an actual Windows playback endpoint or an explicitly chosen process tree. Neither mode installs a virtual sound card.
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

## 0.4.0 component and source refinement

The incumbent direction and composition stay in place; no new taste draw or shell redraw was performed for the button work. Buttons now have one deliberate hierarchy: 44-pixel primary actions with a 10-pixel radius, 40-pixel secondary actions, and 36-pixel compact, quiet and icon actions with an 8-pixel radius. Labels are 14-pixel semibold with 16-pixel glyphs and an 8-pixel gap. Quiet controls use WinUI's native subtle template; primary and secondary controls keep their corresponding native templates. Pointer, press, disabled and keyboard-focus behavior remain native. There is no copied control template, glow, gradient or new decorative motion.

The main route button is 152 × 44 pixels within the reserved 88-pixel transport action row. Refresh has a fixed 104-pixel width so its working label stays stable. The source strip adds a 134-pixel **系统声音 / 指定应用** selector before the flexible source name and the separate status column. Native theme resources, content foreground inheritance and compact packing have received static and actual Windows review in the final UI run below.

The source contract must be understandable from this strip:

- **系统声音** captures the shared mix on the selected Windows output. **指定应用** captures the chosen process and its children, with no virtual audio driver.
- In the historical 0.4.0 contract, switching scope stopped the current route, retained receiver choices and cleared the source selection. The current 0.4.3 contract retains active receiver sessions and waits silently for explicit app choice.
- The empty app list explains that the app may need to play once, followed by **刷新列表**. Paused audio sessions can remain available.
- Exiting an app cannot select another app or the system mix automatically. Relaunched instances require a new selection.
- Browser tabs in one instance may share the selected process tree; this is not a per-tab picker.
- **电脑静音** is disabled in application mode, and the explanation states that local playback continues. Endpoint-wide mute would affect other apps.

Independent multi-device streaming still sends complete stereo audio to each receiver. It is not native Apple left/right grouping and does not establish synchronized playback. UI fixtures can demonstrate the above states, but cannot establish actual process isolation, native pair support or acoustic output.

## Context and skill record

The requested impeccable and taste skills informed this work. The taste app-screen run was used and its actual draw saved. Its browser inspector does not validate a native WinUI app and is not presented as native evidence.

The impeccable context launcher was attempted and unavailable; direct context reading was announced. Existing PRODUCT.md, DESIGN.md, source and current Windows screenshots were inspected. The installed bundle exposes its main guidance, but exact reads of `reference/new-work.md`, `reference/craft-floor.md` and the later component-refinement `reference/polish.md` were unavailable. Those unavailable references were not treated as completed checks. Available redesign, product-UI, native interaction, accessibility and rendered-review guidance was applied.

## Acceptance evidence

The final 0.4.0 source is `7c5133e006ec0be13017ec21084680518c95082f`. [CI 39583891772](https://github.com/chummumm/airloom/actions/runs/39583891772) passed both jobs and all gates, including the real Windows process-loopback API test, native build/package, complete UI smoke and independent updater lifecycle. The visual review uses actual WinUI captures from that run's final artifact.

| Requirement | Observed evidence and boundary |
| --- | --- |
| Refined native buttons | Enabled/disabled controls fit in both themes; dark hover and pressed fills are distinct and retain stable 44-pixel primary bounds under physical pointer input |
| Focus | UIA `SetFocus` and `HasKeyboardFocus` verify logical focus; no visible keyboard-focus-ring screenshot check is claimed |
| Minimum-size source strip | Actual 880 × 680 controls fit; the first two receivers and fixed dock are visible; the management footer can reach the ordinary outer scroll boundary |
| Explicit app scope | Native fixture verifies unselected app mode, explicit selection, stop on scope change, refresh retaining selection and disabled local mute |
| No automatic app substitution | Core preserves scope and requires explicit application choice; the native fixture starts app mode with no selected source |
| Stable volume and useful signal | Native geometry/value retention at three sizes, actual mouse drag, populated retention and the rendered 64-bar envelope passed |
| Themes and settings | Actual light/dark application home/settings frames are readable; settings/back, read-only update check and restricted diagnostic copy passed |
| Authentication independence | UIA found the native password dialog and verified cancelling only the challenged receiver; the final auth-named frame shows home and is not used as modal visual evidence |
| Exit and update cleanup | Independent updater/process lifecycle gate passed, separately from UI smoke |
| Actual application capture | Real Windows activation, identity, exact-zero received silent samples, mute refusal, bounded cancellation, restart and target-exit checks passed |
| Stereo scope stated accurately | Independent complete-stereo streams are distinguished from unimplemented native HomePod grouping and unverified existing-pair playback |

**Visual verdict:** no new blocking clipping, obscured action or unreadable state was found in the reviewed light/dark application, settings, compact and button frames. The waveform stays inside its instrument. Dark normal/hover/pressed button feedback is visible without geometry movement. Logical focus is confirmed by UIA; its screenshot does not show a visible focus-ring change. The awaiting-selection frame contains three available synthetic apps. Empty/ended-app screens and the complete receiver error/reconnect matrix were not visually reviewed.

All populated `fixture-*` images use the same native executable with a CI-only replacement core DLL in a separate temporary copy. Application choices, receiver identities, levels and authentication challenges are synthetic. The shipped core hash is checked unchanged, and the replacement is not distributed. The final `fixture-auth-dark.png` is preserved as captured: it does not show the modal and provides no modal visual proof. Authentication success in this run is supported by interaction assertions.

The real process API test uses an owned process with no audio stream. It does not establish isolation between two audibly playing apps or a listening result on HomePod. No physical HomePod is connected to CI. Browser process behavior, native stereo pairs, synchronization and other hardware/firmware combinations remain separate real-device checks. High-contrast resources exist, but high-contrast rendering has not been visually verified. Resource calculations and XML validity remain prechecks rather than rendering proof.

The 0.3.0 composition passed [CI 39573924973](https://github.com/chummumm/airloom/actions/runs/39573924973), source `e80e6f9af8707216277fa8396dd351eaf33fad73`, and remains a historical regression baseline. Separately retained reduced-motion [light](../../evidence/fixture-reduced-motion-home-light.png) and [dark](../../evidence/fixture-reduced-motion-home-dark.png) captures come from [run 39572112414](https://github.com/chummumm/airloom/actions/runs/39572112414), source `f2a914feec150f56e98e0f461b5088d3e54616a3`; they keep their original provenance. The user's 0.2.1 listening result remains an attributed hardware observation, not a current CI result.
