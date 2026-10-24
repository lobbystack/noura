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

Interop is tested against Yjs 13.6.32 and y-codemirror.next 0.3.6. Noura uses UTF-16 offsets explicitly; Yrs defaults to byte offsets.

The 2026-09-06 dependency audit reports `RUSTSEC-2026-0215`: transitive `smallstr 0.3.1` is unmaintained with no patched release. The approved narrow exception is recorded in `deny.toml`. Upgrading to the inspected Yrs 0.27.4 does not remove that dependency and also requires language features newer than the repository's Rust 1.91 CI toolchain. Recheck the upstream dependency and remove the exception on every Yrs upgrade and before each release. A future vulnerability advisory is not covered by this maintenance-only exception.

## Surrogate-pair patch for the collaboration sandbox

The sync v2 collaboration sandbox's guest module builds its own copy of yrs with one patch ([sync-v2.md §3](../workspace-format/sync-v2.md#3-primitives)). Ticket 43 records the patch here before ticket 54 lands it.

- Repository: https://github.com/y-crdt/y-crdt
- Version: yrs 0.26.0; Cargo package VCS commit `5238955513138fb10db67fe6547745fc9df557a5`, package path `yrs`.
- Source: `src/block.rs`, the `ItemContent::String` arm of `ItemContent::splice`, which calls `split_str` with the UTF-16 offset.
- License: MIT, Bartosz Sypytkowski and Kevin Jahns (2020), the same repository-root `LICENSE` as above.
- Destination: the guest module's crate, which the workspace's `exclude` list holds, in its copy of yrs (below). It declares `yrs = { version = "=0.26.0", default-features = false }`, so neither `small-client` nor `sync` is on.
- Modifications: when the split offset falls between the two UTF-16 units of a surrogate pair, the left part ends with U+FFFD and the right part starts with U+FFFD, one unit each. The parts keep the lengths that the item's clocks give them. Unpatched yrs 0.26.0 rounds the split to the next code point while the item keeps the offset as its length. yrs carries the intended code as a commented-out block in the same arm; the patch enables that behavior for UTF-16 offsets.
- Behavior reference: Yjs `ContentString.splice` (MIT, Kevin Jahns), which replaces both halves of a split pair with U+FFFD. The patch copies no Yjs source.
- Notices: the yrs MIT notice in `THIRD_PARTY_NOTICES.md` covers the patched copy. The patch adds no notice of its own.
- Reviewer: Claude, ticket 43 (author review). Ticket 54's review checks the landed patch against this record, and the guest module's pinned hash covers it.
- Date: 2026-10-21.

## The guest's copy of yrs

Ticket 54 landed the surrogate-pair patch in a full copy of the yrs 0.26.0 package under `crates/collab-sandbox-guest/vendor/yrs`, which the guest crate's `[patch.crates-io]` table puts in place of the crates.io package. The guest module's pinned hash covers the copy and every patch below.

- Repository: https://github.com/y-crdt/y-crdt
- Version: yrs 0.26.0; Cargo package VCS commit `5238955513138fb10db67fe6547745fc9df557a5`, package path `yrs`, as `.cargo_vcs_info.json` in the copy records.
- Source: the crates.io package `yrs-0.26.0.crate`, unpacked whole: `src/`, `benches/`, `Cargo.toml`, `Cargo.toml.orig`, `Cargo.lock` and `README.md`. The package ships no license file, so the copy adds `LICENSE` with the repository-root MIT text.
- License: MIT, Bartosz Sypytkowski and Kevin Jahns (2020).
- Destination: `crates/collab-sandbox-guest/vendor/yrs`, built only into the guest module `crates/collab-sandbox/guest/collab_sandbox_guest.wasm`. `crates/local-core` keeps the crates.io package.
- Modifications:
  - `src/block.rs`: the surrogate-pair split above, in `ItemContent::splice` and a new helper `split_surrogate_pair`.
  - `src/block.rs`, `src/transaction.rs` and `src/types/text.rs`: the text delta of a transaction reports the units a split inside a surrogate pair turned into U+FFFD. `ItemPtr::splice` records the two elements of each such split in a thread-local set, which `TransactionMut::new` empties. When the delta reaches a retained string item whose first or last unit is a recorded element and holds U+FFFD, it deletes that unit and inserts U+FFFD instead of retaining it. Without this, the delta retains those units, so an editor that applies it keeps the emoji while the document holds two U+FFFD. The patch changes no encoding and no document state, only the deltas that observers read. Yjs 13.6.32's `YTextEvent.delta` retains those units too, so the patch follows no Yjs code: its reference is the document's text after the transaction, which the guest's tests and the editor's seam tests compare against.
  - `src/id_set.rs`: `IdRange::contains_clock` finds the one range that can hold a clock by binary search instead of scanning every range. `IdRange` keeps its ranges sorted and apart, as its documentation states, so both forms give the same answer. The scan made each lookup cost as much as the number of ranges, so integrating a record that deletes many scattered ranges cost fuel quadratic in their number.
  - `Cargo.toml` and `Cargo.toml.orig`: `fastrand` loses its `js` feature, which would pull wasm-bindgen imports into a module that may import nothing.
- Notices: `THIRD_PARTY_NOTICES.md` carries the yrs MIT notice, which covers the copy.
- Reviewer: Claude, ticket 54 (author review).
- Date: 2026-10-24.
