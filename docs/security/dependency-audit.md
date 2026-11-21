# Dependency audit exceptions

Last reviewed: 2026-09-07

`cargo deny` blocks vulnerable, yanked, unlicensed, and unknown-source dependencies. `deny.toml` lists narrow exceptions for transitive crates that have no safe replacement in the locked stack. Each entry there carries its reason, and this page explains the larger ones.

## Linux drag-and-drop dependencies

The desktop crate's `drag` dependency reaches `proc-macro-error` (`RUSTSEC-2024-0370`) through the GTK crates it uses on Linux. The desktop crate uses `drag` on macOS and Windows only, so no build compiles those crates, but `cargo deny` reads every target's dependencies.

## Image decoder dependency

`rav1d`, which app-ui's AVIF decoder uses, depends on the archived `paste` macro crate (`RUSTSEC-2024-0436`). `genai` replaced it with `pastey` in 0.7. The notice reports maintenance status, not a known vulnerability. Noura contains no direct `paste` usage and will remove the exception when `rav1d` replaces it.

## Collaborative-text decoder dependency

The pinned `yrs 0.26.0` interoperability stack reaches unmaintained `smallstr 0.3.1` through the required `small-client` feature (`RUSTSEC-2026-0215`). The advisory reports maintenance status and has no patched release. The inspected `yrs 0.27.4` still uses the dependency and requires Rust language features newer than Noura's pinned Rust 1.91 toolchain. Noura accepts this single advisory while tracking upstream removal in [`docs/provenance/yrs.md`](../provenance/yrs.md). The exception does not cover a future vulnerability advisory and does not waive isolated, bounded decoding or the malicious-input acceptance suite.

Collaboration activation additionally requires an immutable owner-signed workspace capability. Relay feature discovery alone cannot authorize a transition.

## License exception

Rust TLS uses `webpki-root-certs`, which distributes Mozilla's public root-certificate data under `CDLA-Permissive-2.0`. This permissive data license is explicitly allowed. It does not authorize copying unrelated source code.

Review these exceptions whenever `rav1d`, `yrs` or `drag` changes, and before each release. An unmaintained-code exception does not cover a newly reported vulnerability.
