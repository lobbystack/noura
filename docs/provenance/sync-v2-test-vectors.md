# Sync v2 published test vectors

The sync v2 crates run the published test vectors that [sync-v2.md §3](../workspace-format/sync-v2.md#3-primitives) names. One vector file sits in the repository; the rest arrive through a Cargo dependency or as two typed-in values.

## HPKE: draft-ietf-hpke-pq

- Repository: https://github.com/rozbb/rust-hpke, through the `hpke` 0.14.1 package on crates.io; Cargo package VCS commit `b83b0011030b55ac74f389c112db784274d4d667`.
- Source: `test-vectors/pq-6433c8f.json` in the package (SHA-256 `35c59f4a0132e5631e50ac039d8ca3a72e99f5e92dfd94d45338d6ae243f613c`). The package takes it from the draft-ietf-hpke-pq reference implementation, hpkewg/hpke-pq at commit `6433c8f`.
- License: the `hpke` package is MIT OR Apache-2.0.
- Destination: `crates/sync-key-envelope/tests/vectors/hpke-pq-6433c8f-xwing-hkdfsha256-chacha20poly1305.json`.
- Modifications: the file keeps the one base-mode vector for the suite Noura uses, X-Wing (`0x647a`), HKDF-SHA256 (`0x0001`) and ChaCha20Poly1305 (`0x0003`), with its 10 encryptions and 5 exports. The values are unchanged; the JSON is re-indented with sorted keys.
- Notices: test data only. No build ships it, so releases carry no notice for it.
- Reviewer: Claude, ticket 43 (author review).
- Date: 2026-10-21.

## Wycheproof

`sync-protocol` and `sync-key-envelope` read Wycheproof's XChaCha20-Poly1305, Ed25519 and X25519 sets through the `wycheproof` crate 0.7.0 (Apache-2.0, https://github.com/randombit/wycheproof-rs), a dev-dependency. The repository copies no Wycheproof file.

## BLAKE3

`crates/sync-protocol/tests/published_vectors.rs` holds the published outputs for input lengths 0 and 1 of the BLAKE3 reference vectors (BLAKE3-team/BLAKE3, `test_vectors/test_vectors.json`, CC0-1.0 or Apache-2.0): the first 32 bytes of the hash, keyed hash and derived key at length 0, and of the hash at length 1. These are factual test values with no notice duty. The full file needs the owner's approval to download into the repository, so it isn't here yet.
