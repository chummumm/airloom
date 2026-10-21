# Contributing

Airloom is an independent implementation under MIT. Please keep changes focused and explain the user-visible behavior and the evidence used to verify it.

## Build and checks

Use the toolchain in `rust-toolchain.toml`. Run `cargo fmt --all -- --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`. The capture API intentionally returns an unsupported-platform error outside Windows; pure audio conversion and protocol tests still run on Linux.

Windows UI build instructions are in the main README and `scripts/build-windows.ps1`. Native UI changes require a Windows build. Audio or authentication changes should include a public protocol reference or fixture, a meaningful test, and explicit hardware verification notes.

## Design rules

- Keep platform and protocol complexity in the module that owns it.
- Preserve bounded queues and deterministic cancellation; do not accumulate stale audio.
- Do not log passwords, pairing keys, raw authentication bodies, or private device identifiers.
- Do not present simulated devices, made-up latency, or a successful handshake as proof of acoustic output.
- Use standard cryptographic implementations. Do not copy GPL application code into this MIT project.
- Respect reduced motion, high contrast, keyboard navigation, and clear error states.

## Reporting compatibility

Include the Windows version, output-device type, HomePod model/firmware, access-policy category, whether password protection is enabled, and sanitized error code. Never include the password or pairing credentials. Different devices and firmware are separate compatibility results.

