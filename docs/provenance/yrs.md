# Yrs collaboration dependency

- Repository: https://github.com/y-crdt/y-crdt
- Version: yrs 0.26.0; Cargo package VCS commit `5238955513138fb10db67fe6547745fc9df557a5`.
- Source: `yrs` package consumed through Cargo; repository-root `LICENSE`.
- License: MIT, Bartosz Sypytkowski and Kevin Jahns (2020).
- Destination: dependency of `crates/local-core`; no implementation source copied or adapted.
- Modifications: none to upstream; enable `small-client` for Yjs 13 and `sync` for native use.
- Notices: retain the MIT copyright and terms in release dependency notices.
- Reviewer: Codex (author review).
- Date: 2026-09-06.

Interop is tested against Yjs 13.6.32 and y-codemirror.next 0.3.6. Noura uses
UTF-16 offsets explicitly; Yrs defaults to byte offsets.

The 2026-09-06 dependency audit reports `RUSTSEC-2026-0215`: transitive
`smallstr 0.3.1` is unmaintained with no patched release. This is not suppressed
in `deny.toml`; the advisory gate currently fails. Upgrading to the inspected
Yrs 0.27.4 does not remove that dependency and also requires language features
newer than the repository's Rust 1.91 CI toolchain. Resolve this maintenance
decision before production enablement.
