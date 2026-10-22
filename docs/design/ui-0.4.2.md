# Airloom 0.4.2 native interface

## Adopted design

This release integrates the four WinUI files supplied by the user after Claude's redesign. That supplied design is the visual authority. It replaces the older silver workspace and permanently dark instrument with Windows 11 Mica, a Fluent layer sheet, theme-aware cards, native button templates and the user's system accent. No new taste draw or web implementation is introduced.

The home screen still answers three operational questions: which Windows source is selected, which speakers receive it, and whether the outgoing audio contains a signal. The source settings card leads, the casting and speaker cards share the wide workspace, and the transport dock remains outside the content scroller. At the existing 1050 effective-pixel breakpoint the speaker card moves above the signal card. The existing 880 effective-pixel minimum and stable volume value slot remain.

Use the supplied native typography, Fluent resources and per-theme C++ brush aliases. Mica is the window backdrop; working content uses the layer/card surface resources. Settings use Windows-style rows. Native button templates retain their own pointer, pressed, disabled and keyboard-focus behavior. User-supplied Windows styling takes precedence over the web-oriented skill rules about materials and accent colors.

The state pulse, page transitions, device selection and waveform are actual native composition/XAML behavior. They obey Windows animation preferences, and the waveform remains an engine-data visualization. Layout slots and the slider acknowledgement protection must remain stable during changes.

## Behavioral contracts retained

- Source items are reused by ID. An open source dropdown defers structural changes, then applies the latest list after closing; a disappeared application never becomes an implicit system capture.
- Device and visibility rows retain identity. Hiding a native group preserves its member association; restoring an offline historical group clears the corresponding member hidden records.
- Password challenges belong to the current receiver and request ID. A cancelled challenge does not stop unrelated outputs.
- Volume remains immediate under the pointer and protected from stale snapshots. The active primary button says “停止投放” and retains the same stop-all command.
- The native stereo card's accessible name says “原生立体声组合”. The compatibility/awaiting-validation labels are removed from the product UI and core status text. The existing left-then-right test tone is retained.
- Ordinary close continues in the tray; explicit exit and installer requests terminate the engine and detach the tray icon.

## Verification

The existing Windows CI exercises the shipped native executable at 880, 1100 and 1600 pixels, both themes, source selection/events, volume dragging, settings, passwords, native group states and update exit/cleanup. Its stereo accessible-name assertion follows the new product wording; behavior and geometry gates are retained.

Actual release screenshots and run provenance are linked from the README after verification. Populated fixture screenshots are synthetic UI evidence; the source notification API lifecycle, protocol tests and physical HomePod listening results remain separate evidence. A successful header-only C++ check does not substitute for XAML compilation, packaged app startup or rendered UI review.

The Impeccable launcher and referenced playbooks are absent from the available installation in this session. PRODUCT.md, the supplied files and the available Impeccable principles/Taste app-screen guidance provide the review context. Browser CSS inspectors are not evidence for a native WinUI executable; native CI and its screenshots are used instead.

## Verified release evidence

[CI 39701717193](https://github.com/chummumm/airloom/actions/runs/39701717193) passed on source commit `481d3ee17a06b54c0cdc4ce483716aea9cbd8fa6`. Both Linux and Windows jobs succeeded, including actual WinUI/XAML compilation, packaged-app launch, native UI automation and updater lifecycle/cleanup. The downloaded Windows artifact matched its API SHA-256 digest, `afc3bf5cdcfffd31bb7f22c9ed30e55d59e1b43b3f24b841e8d422ce1355cc23`.

Rendered light/dark home and settings views, the source dropdown, main button states, native group views and compact layouts were reviewed. The UIA password-dialog and cancellation checks passed, but `fixture-auth-dark.png` captures the home screen and is not visual evidence of the password dialog. UIA focus is likewise a logical focus check, not proof of the visible keyboard focus ring. No physical HomePod was connected to CI.

[Release v0.4.2](https://github.com/chummumm/airloom/releases/tag/v0.4.2) was published as Latest by [publish run 39703079372](https://github.com/chummumm/airloom/actions/runs/39703079372). The installer and portable ZIP release asset digests match the verified CI files. [Published release screenshot](../evidence/github-release-0.4.2.jpg).
