# Airloom product context

Airloom is an open-source native Windows app for sending sound from an existing Windows playback output to compatible AirPlay receivers, including HomePod. Its desktop shell is C++/WinRT and WinUI 3; a Rust core owns discovery, WASAPI capture, authentication and transport. The supported desktop baseline is Windows 11. The app does not install an audio driver or create a virtual playback endpoint.

## Main job

Choose the Windows output that currently carries the sound, select the receiving speakers, and control the session without interrupting another selected receiver. A person should be able to answer three questions from the home screen: what is the source, where is it being sent, and is there an actual audio signal?

The device list contains discovered devices and their current states. It has no sample speakers. Selection is explicit, with a core limit of eight simultaneous targets. A receiver may require an access password or pairing code; its prompt belongs to that receiver and its current authentication request. One receiver waiting for a password must not obscure another receiver's running session.

## Main actions

- Select an existing Windows playback output and one or more receivers.
- Start a session, add or remove a target, retry a failed target, or stop all targets.
- Adjust shared speaker volume. Newly connected receivers inherit the current value.
- Inspect real signal levels and send a short test sound to help distinguish capture problems from transport problems.
- Optionally mute the selected local Windows output, understanding that this affects other apps using that output.
- Hide unneeded receivers and restore them in settings, including when they are offline.
- Check for a public release, install it, and clean up the downloaded installer after the installation attempt.
- Copy a restricted diagnostic report when troubleshooting a silent receiver.

## Desktop expectations

Ordinary window close continues in the notification area when a tray icon is available. The tray menu and settings offer explicit exit. Installer requests and confirmed Windows session shutdown exit the process rather than hiding the window. Starting with Windows is opt-in and does not automatically connect or capture.

The interface uses native controls, keyboard behavior and accessibility semantics. Existing system/light/dark theme preferences remain meaningful. Windows high contrast and reduced-motion preferences take precedence over decorative presentation.

## Evidence and limits

Audio meters and waveform samples come from the engine. Packet counts, clock exchanges and successful protocol responses do not prove that a physical speaker emitted sound. The UI must not label them as acoustic verification or measured end-to-end latency. A test tone uses the same active transport and is a diagnostic action, not an automatic success test.

HomePod compatibility and audible output require real Windows audio hardware and receivers. Native CI can establish launch, layout, interaction and process-exit behavior; protocol tests establish the wire behavior they explicitly exercise. Neither substitutes for a listening test.

The app is currently a preview. Compatibility limitations and reproducible errors should be explained in the relevant state, without invented performance metrics.

## Sources of truth

- [Native screen and interaction design](docs/design.md)
- [Visual system](DESIGN.md)
- [Core snapshot and diagnostics](crates/airloom-core/src/model.rs)
- [Session orchestration](crates/airloom-core/src/engine.rs)
- [Protocol notes](docs/protocol.md)
- [Update lifecycle](docs/updates.md)
