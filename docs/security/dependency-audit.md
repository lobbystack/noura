# Dependency audit exceptions

Last reviewed: 2026-08-27

`cargo deny` blocks vulnerable, yanked, unlicensed, and unknown-source dependencies. The repository has narrow exceptions for unmaintained transitive crates that currently have no safe replacement within the locked Tauri 2 and `genai` stack.

## Tauri Linux WebView dependencies

Tauri 2 currently reaches the archived gtk-rs GTK3 bindings through its supported Linux WebView runtime. The RustSec notices `RUSTSEC-2024-0411` through `RUSTSEC-2024-0420` report that the bindings are unmaintained; they do not describe an exploitable vulnerability. Replacing them independently would fork Tauri's platform layer, so Noura accepts these notices until Tauri provides a supported replacement.

The same dependency path includes `proc-macro-error` (`RUSTSEC-2024-0370`) and the `rust-unic` identifier crates (`RUSTSEC-2025-0075`, `RUSTSEC-2025-0080`, `RUSTSEC-2025-0081`, `RUSTSEC-2025-0098`, and `RUSTSEC-2025-0100`). These are build-time or parsing dependencies with no safe upgrade in the current graph.

## AI provider dependency

`genai` currently depends on the archived `paste` macro crate (`RUSTSEC-2024-0436`). The notice reports maintenance status, not a known vulnerability. Noura contains no direct `paste` usage and will remove the exception when `genai` replaces it or a compatible provider layer is available.

## License exception

`webpki-root-certs`, used through Rust TLS, distributes Mozilla's public root-certificate data under `CDLA-Permissive-2.0`. This permissive data license is explicitly allowed. It does not authorize copying unrelated source code.

Review these exceptions whenever Tauri or `genai` changes, and before each release. A newly reported vulnerability is not covered by an unmaintained-code exception.
