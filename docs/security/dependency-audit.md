# Dependency audit exceptions

Last reviewed: 2026-09-07

`cargo deny` blocks vulnerable, yanked, unlicensed, and unknown-source dependencies. The repository has narrow exceptions for unmaintained transitive crates that currently have no safe replacement within the locked Tauri 2 and `genai` stack.

## Tauri Linux WebView dependencies

Tauri 2 currently reaches the archived gtk-rs GTK3 bindings through its supported Linux WebView runtime. The RustSec notices `RUSTSEC-2024-0411` through `RUSTSEC-2024-0420` report that the bindings are unmaintained; they do not describe an exploitable vulnerability. Replacing them independently would fork Tauri's platform layer, so Noura accepts these notices until Tauri provides a supported replacement.

The same dependency path includes `proc-macro-error` (`RUSTSEC-2024-0370`) and the `rust-unic` identifier crates (`RUSTSEC-2025-0075`, `RUSTSEC-2025-0080`, `RUSTSEC-2025-0081`, `RUSTSEC-2025-0098`, and `RUSTSEC-2025-0100`). These are build-time or parsing dependencies with no safe upgrade in the current graph.

## AI provider dependency

`genai` currently depends on the archived `paste` macro crate (`RUSTSEC-2024-0436`). The notice reports maintenance status, not a known vulnerability. Noura contains no direct `paste` usage and will remove the exception when `genai` replaces it or a compatible provider layer is available.

## Collaborative-text decoder dependency

The pinned `yrs 0.26.0` interoperability stack reaches unmaintained `smallstr 0.3.1` through the required `small-client` feature (`RUSTSEC-2026-0215`). The advisory reports maintenance status and has no patched release. The inspected `yrs 0.27.4` still uses the dependency and requires Rust language features newer than Noura's pinned Rust 1.91 toolchain. Noura accepts this single advisory while tracking upstream removal in [`docs/provenance/yrs.md`](../provenance/yrs.md). The exception does not cover a future vulnerability advisory and does not waive isolated, bounded decoding or the malicious-input acceptance suite.

Collaboration activation additionally requires an immutable owner-signed workspace capability. Relay feature discovery alone cannot authorize a transition.

## PostgreSQL transaction driver

The pinned `postgres 3.4.9` release has an open upstream transaction-reservation defect: at a pipeline boundary, `BEGIN` can reach PostgreSQL before the driver marks that connection reserved. Noura does not rely on the affected `sql.begin` path. `SyncStore.transaction` first obtains an exclusive physical connection, then issues `BEGIN`, `COMMIT`, or `ROLLBACK` only through that reserved handle; all relay transaction sites use this wrapper. The collaboration load gate keeps the concurrency level above the reproduced boundary. Track [porsager/postgres#1189](https://github.com/porsager/postgres/issues/1189) and remove the workaround only after an upstream release has a targeted regression test and Noura's concurrent transaction tests and soak still pass.

## License exception

Rust TLS uses `webpki-root-certs`, which distributes Mozilla's public root-certificate data under `CDLA-Permissive-2.0`. This permissive data license is explicitly allowed. It does not authorize copying unrelated source code.

Review these exceptions whenever Tauri or `genai` changes, and before each release. An unmaintained-code exception does not cover a newly reported vulnerability.
