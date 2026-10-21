# Airloom visual system

Airloom is a native sound-routing tool. Visual emphasis belongs to the selected source, the receiving speakers and the actual audio signal. The interface should be quiet enough to leave open during listening and precise enough to diagnose an unsuccessful connection.

The direction is a flat neutral console: a fixed receiver rail, a ruled task header and a flexible working area. Surface lightness defines grouping. The primary action is an inversion of the current theme; the single green data color belongs to audio and connection state. This replaces the earlier centered lavender composition, large speaker illustration and Mica backdrop.

## Structure

| Element | Rule |
| --- | --- |
| Native title bar | 44 effective pixels, original Airloom mark, native caption controls |
| Task header | 64 effective pixels, task name, reserved status area and settings action |
| Device rail | 248 effective pixels, independent scrolling and persistent device rows |
| Working content | Owns the right viewport width; 22 effective pixels of horizontal padding |
| Section rhythm | 14–22 effective pixels; controls stay adjacent to the signal |
| Panels | 20 effective pixel radius, opaque fill, no shadows or decorative outline |
| Device rows | 72 effective pixels, 8 effective pixel radius, fixed icon and selection slots |
| Primary control | Native 36 effective pixel high button, 18 effective pixel radius, inverted fill |
| Main / section / target type | 20 / 16 / 22 effective pixels, semibold |
| Body / secondary type | 14 / 12 effective pixels |

At compact and normal desktop widths, source, signal and transport stack vertically. At 1280 effective pixels or greater, a 332 effective pixel transport column sits beside the signal. The receiver rail remains fixed. The minimum tracking size is 880 × 680 effective pixels; vertical overflow scrolls. A reserved vertical scrollbar avoids moving content when overflow changes.

Large windows use their available working area. Do not recenter a narrow maximum-width card group as status text, digits or receiver names change. The volume percentage owns a fixed 52 effective pixel slot; changing 9% to 100% must not move the slider or neighboring panels.

## Type and icons

Use Segoe UI Variable Text with native Windows fallback. This preserves platform language coverage and conventions. Use regular and semibold weights, with tabular numerals for volume, time, sample rate and dB values. Headings describe tasks or settings.

Use the existing Airloom vector mark and native Fluent glyphs. A receiver row needs a small speaker symbol, its real name and plain-language state. It does not need a large product illustration, colored icon tile or ornamental status pill.

## Theme resources

These are the application-owned opaque colors in [App.xaml](native/Airloom/App.xaml). Native control templates retain hover, pressed, disabled and focus states.

| Resource | Dark | Light | Purpose |
| --- | --- | --- | --- |
| `AppCanvasBrush` | `#000000` | `#F5F5F5` | Window ground |
| `PanelBrush` | `#111111` | `#FFFFFF` | Working surface |
| `InsetBrush` | `#2A2A2A` | `#EEEEEE` | Track and inset surface |
| `HairlineBrush` | `#242424` | `#E4E4E4` | Structural rules |
| `InkBrush` | `#F7F7F7` | `#191919` | Main text and neutral icons |
| `MutedBrush` | `#B4B4B4` | `#5B5B5B` | Supporting text |
| `BrandBrush` | `#F7F7F7` | `#191919` | Neutral selection emphasis |
| `BrandSoftBrush` | `#242424` | `#E8E8E8` | Selected receiver surface |
| `AudioBrush` | `#7AD9B4` | `#176C4D` | Actual signal and active state |
| `AudioSoftBrush` | `#172A23` | `#E5F1EA` | Subtle audio surface |
| `WarningBrush` | `#E8BE76` | `#80520A` | Attention required |
| `DangerBrush` | `#FF9696` | `#B3261E` | Failed operation |
| `PrimaryActionBrush` | `#F7F7F7` | `#202020` | Primary button fill |
| `PrimaryActionTextBrush` | `#111111` | `#FFFFFF` | Primary button text |
| `ControlOutlineBrush` | `#555555` | `#B7B7B7` | Control boundary |

Do not apply the audio hue to navigation, primary buttons or decorative backgrounds. Do not reintroduce purple, translucent Mica, large glows or gradients. Error and warning colors carry meaning and accompany readable text.

The default remains **follow Windows**. Existing light/dark choices are retained. High-contrast resources use Windows system colors.

## Screen hierarchy

### Sound routing

The source picker and capture explanation come first. Selected receivers and real signal occupy the main surface. Volume, connection and local-mute controls are adjacent. Discovery remains visible in the left rail, with selection and per-device state represented separately.

No fake receivers, demo waveform, invented elapsed time, estimated acoustic latency or decorative metrics may appear in production. A silent or missing source has an explicit explanation. A running transport without acoustic confirmation remains a running transport, not a verified speaker.

### Settings

Settings is a full native view with a clear back action. Order: application update, connection diagnostics, appearance/startup, device visibility, application information/exit. Update status and its action are immediately visible. Hidden receivers remain recoverable while offline. Groups use short explanatory labels beside their controls.

## Interaction and motion

Existing receiver buttons update in place, so periodic snapshots do not recreate keyboard focus or accessibility peers. Dynamic text is assigned only when it changes.

Volume follows the user's hand immediately. Commands are debounced; an older snapshot cannot replace a local drag value before the core acknowledges it. Pointer release flushes the final value. Motion must never interfere with this direct manipulation.

The native slider thumb's two-pixel endpoint outset is reserved inside its track slot, not clipped or replaced. Thumb and track normal, hover, pressed and disabled brushes are scoped directly to the control; the high-contrast variants retain system colors. Explicit connection-button text uses a readable muted foreground when disabled instead of overriding the native disabled state with white text.

Navigation can use a short opacity transition when Windows animations are enabled. Audio movement comes from real PCM. There is no continuous ornamental motion. Disabling animation or the waveform leaves readable states and actual numeric/level information.

## Accessibility and review

Use native buttons, sliders, toggles, combo boxes and password dialogs. Preserve keyboard behavior, focus indication, accessible names, heading levels and status announcements. Color alone must not distinguish selected, authenticating, streaming or failed receivers.

[The saved contrast calculation](docs/design/contrast-check.json) checks main, secondary, signal, warning and error text colors on the three app-owned opaque surfaces. Its minimum calculated ratio is 5.5:1. This is a resource-level calculation, not a rendered accessibility audit or certification of every native control state.

Inspect the actual WinUI window at compact, normal and wide sizes. HTML previews and web inspectors cannot establish XAML layout or native behavior. See [implementation notes](docs/design.md) for the verification procedure.

## Direction provenance

The app-screen draw selected **true-black-flat**, seed **323664579**, on 2026-10-21. Native type was deliberately retained. One shell redraw replaced the centered no-sidebar shell with **ruled-t-frame**, because the former reproduced the reported centered layout and wasted space at maximized size. The data hue was adapted to real audio signal; controls and navigation remain neutral.

The exact draw is retained in [app-direction.json](docs/design/app-direction.json). Surface goals, incumbent critique and verification boundaries are in [the surface brief](docs/design/surfaces/audio-routing.md).
