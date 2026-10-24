//! Seam 2: the fuzz suite under `cargo test`. Random bytes, structured yrs
//! updates with hostile fields, and mutations of honest updates go through
//! the sandbox's verdict and integration. None may crash the host, pass its
//! budget or leave a note inconsistent, and the process's memory returns to
//! within 10% of where it started after each run. The scheduled job runs
//! the same entry points under cargo-fuzz (`crates/collab-sandbox/fuzz`),
//! and every crash it keeps replays here.
//!
//! One test runs every generator in turn, so no other test's allocations
//! land in its measurements.

mod support;

#[cfg(unix)]
#[path = "support/direct.rs"]
mod direct;

#[cfg(unix)]
#[global_allocator]
static ALLOCATOR: direct::Direct = direct::Direct;

use std::cell::Cell;
use std::path::PathBuf;

use collab_sandbox::fuzzing;
use collab_sandbox::op::{Op, Splice};
use plugin_host::{process_memory, release_free_memory};
use proptest::prelude::*;
use proptest::test_runner::{Config, TestRunner};
use support::{Lib0, client, sandbox};

/// Client IDs a hostile writer might name: the signer's own, others, client
/// 0, and the edges of the ranges the verdict checks.
fn client_id() -> impl Strategy<Value = u64> {
    prop_oneof![
        Just(client(1, 1)),
        Just(client(1, 1)),
        Just(0u64),
        Just(client(2, 1)),
        Just((1u64 << 53) - 1),
        Just(1u64 << 53),
        Just(u64::MAX),
        any::<u64>(),
    ]
}

/// Clocks and lengths: small ones, and the edges of 2^32.
fn number() -> impl Strategy<Value = u64> {
    prop_oneof![
        4 => 0u64..16,
        1 => Just((1u64 << 32) - 2),
        1 => Just((1u64 << 32) - 1),
        1 => Just(1u64 << 32),
        1 => any::<u32>().prop_map(u64::from),
        1 => any::<u64>(),
    ]
}

/// One struct of a struct section, as lib0 bytes: any info byte, origins,
/// a parent, a map key and content in any combination.
fn structure() -> impl Strategy<Value = Vec<u8>> {
    (
        prop_oneof![
            Just(0x84u8),
            Just(0x04),
            Just(0x81),
            Just(0x44),
            Just(0xc4),
            Just(0xc1),
            Just(0x0a),
            Just(0x00),
            Just(0x24),
            any::<u8>(),
        ],
        (client_id(), number()),
        (client_id(), number()),
        prop_oneof![Just("content".to_owned()), ".{0,8}"],
        prop_oneof!["[a-z😀中\u{301}]{0,12}", ".{0,4}"],
        number(),
    )
        .prop_map(|(info, origin, right, parent, text, length)| {
            let mut w = Lib0::default().raw(&[info]);
            match info & 0x1f {
                0 | 10 => return w.var(length).0,
                _ => {}
            }
            if info & 0x80 != 0 {
                w = w.var(origin.0).var(origin.1);
            }
            if info & 0x40 != 0 {
                w = w.var(right.0).var(right.1);
            }
            if info & 0xc0 == 0 {
                w = w.var(1).string(&parent);
                if info & 0x20 != 0 {
                    w = w.string("key");
                }
            }
            match info & 0x1f {
                1 => w.var(length).0,
                _ => w.string(&text).0,
            }
        })
}

/// A whole update: runs of structs under hostile clients and clocks, then a
/// delete set, sometimes with a few random bytes appended.
fn update() -> impl Strategy<Value = Vec<u8>> {
    (
        prop::collection::vec(
            (
                client_id(),
                number(),
                prop::collection::vec(structure(), 0..6),
            ),
            0..3,
        ),
        prop::collection::vec(
            (client_id(), prop::collection::vec((number(), number()), 0..4)),
            0..3,
        ),
        prop::collection::vec(any::<u8>(), 0..3),
    )
        .prop_map(|(runs, deletes, tail)| {
            let mut w = Lib0::default().var(runs.len() as u64);
            for (client, clock, structs) in runs {
                w = w.var(structs.len() as u64).var(client).var(clock);
                for bytes in structs {
                    w = w.raw(&bytes);
                }
            }
            w = w.var(deletes.len() as u64);
            for (client, ranges) in deletes {
                w = w.var(client).var(ranges.len() as u64);
                for (clock, len) in ranges {
                    w = w.var(clock).var(len);
                }
            }
            w.raw(&tail).0
        })
}

/// An update that reads as the writer's own: one run of card 1's first
/// load from clock 0, whose structs name elements of the base document or
/// of the run, and a delete set over both. Most reach integration, where
/// their origins, overlaps and deletes stress yrs.
fn plausible_update() -> impl Strategy<Value = Vec<u8>> {
    let element = prop_oneof![
        (Just(0u64), 0u64..14),
        (Just(client(1, 1)), 0u64..12),
    ];
    let item = (
        prop_oneof![
            Just(0x84u8),
            Just(0x44),
            Just(0xc4),
            Just(0x81),
            Just(0xc1),
            Just(0x04)
        ],
        element.clone(),
        element.clone(),
        "[a-z😀中\u{301}]{1,6}",
        1u64..4,
    )
        .prop_map(|(info, origin, right, text, length)| {
            let mut w = Lib0::default().raw(&[info]);
            if info & 0x80 != 0 {
                w = w.var(origin.0).var(origin.1);
            }
            if info & 0x40 != 0 {
                w = w.var(right.0).var(right.1);
            }
            if info & 0xc0 == 0 {
                w = w.var(1).string("content");
            }
            match info & 0x1f {
                1 => w.var(length).0,
                _ => w.string(&text).0,
            }
        });
    (
        prop::collection::vec(item, 1..6),
        prop::collection::vec(
            (
                prop_oneof![Just(0u64), Just(client(1, 1))],
                prop::collection::vec((0u64..14, 1u64..4), 1..4),
            ),
            0..2,
        ),
    )
        .prop_map(|(structs, deletes)| {
            let mut w = Lib0::default()
                .var(1)
                .var(structs.len() as u64)
                .var(client(1, 1))
                .var(0);
            for bytes in structs {
                w = w.raw(&bytes);
            }
            let mut deletes = deletes;
            deletes.dedup_by_key(|(client, _)| *client);
            w = w.var(deletes.len() as u64);
            for (client, ranges) in deletes {
                w = w.var(client).var(ranges.len() as u64);
                for (clock, len) in ranges {
                    w = w.var(clock).var(len);
                }
            }
            w.0
        })
}

/// An honest update from a note's own edits, then bytes flipped, inserted,
/// removed or cut off.
fn mutated() -> impl Strategy<Value = Vec<u8>> {
    (
        prop::collection::vec(("[a-z😀 ]{1,6}", 0u32..40, 0u32..3), 1..6),
        prop::collection::vec((any::<prop::sample::Index>(), any::<u8>(), 0u8..4), 0..4),
    )
        .prop_map(|(edits, mutations)| {
            let mut note = support::note(vec![support::insert(0, 0, "honest base")]);
            for (text, at, removed) in edits {
                let len = note.len();
                let start = at.min(len);
                let removed = removed.min(len - start);
                let op = Op::from_splices(
                    len,
                    &[Splice {
                        start,
                        removed,
                        inserted: text,
                    }],
                )
                .unwrap();
                // A start inside a surrogate pair doesn't fit: skip it.
                let _ = note.local(&op, true, 0);
            }
            let mut bytes = note
                .take_updates()
                .unwrap()
                .into_iter()
                .next()
                .map(|update| update.bytes)
                .unwrap_or_default();
            for (index, byte, kind) in mutations {
                if bytes.is_empty() {
                    break;
                }
                let at = index.index(bytes.len());
                match kind {
                    0 => bytes[at] ^= byte | 1,
                    1 => bytes.insert(at, byte),
                    2 => {
                        bytes.remove(at);
                    }
                    _ => bytes.truncate(at),
                }
            }
            bytes
        })
}

/// Meta values nested deep, or in non-canonical forms, beside an honest
/// insert.
fn nested_meta() -> impl Strategy<Value = Vec<u8>> {
    (0usize..20_000, "[\\[\\]{}\":,0-9a-e. ]{0,24}").prop_map(|(depth, noise)| {
        use sync_protocol::codec::{Bytes, Encode, Sorted, Text};
        use sync_protocol::records::{Content, Meta, Payload, PayloadKind};
        use sync_protocol::types::Id;
        let value = format!("{}{noise}{}", "[".repeat(depth), "]".repeat(depth));
        let payload = Payload {
            rid: Id([7; 16]),
            kind: PayloadKind::TextUpdate {
                object_id: Text::new("note_01").unwrap(),
                text_lineage: Id([9; 16]),
                load: 1,
                meta: Sorted::new(vec![Meta {
                    key: Text::new("p:deep").unwrap(),
                    seq: 0,
                    value: Some(Bytes(value.into_bytes())),
                }])
                .unwrap(),
                content: Content::Inline {
                    bytes: Bytes(support::insert(client(1, 1), 0, "deep")),
                },
            },
        }
        .to_bytes();
        // Card 1 of 1, then no blob split.
        let mut data = vec![1, 0, 0, 0, 1, 0, 0, 0, 0];
        data.extend_from_slice(&payload);
        data
    })
}

fn memory_in_use() -> Option<u64> {
    release_free_memory();
    process_memory()
}

fn run<S: Strategy>(
    name: &str,
    baseline: Option<u64>,
    cases: u32,
    strategy: S,
    test: impl Fn(S::Value),
) where
    S::Value: std::fmt::Debug,
{
    // The scheduled job's longer runs set NOURA_FUZZ_SCALE.
    let scale: u32 = std::env::var("NOURA_FUZZ_SCALE")
        .ok()
        .and_then(|scale| scale.parse().ok())
        .unwrap_or(1);
    let mut runner = TestRunner::new(Config {
        cases: cases.saturating_mul(scale),
        failure_persistence: None,
        ..Config::default()
    });
    let result = runner.run(&strategy, |value| {
        test(value);
        Ok(())
    });
    if let Err(error) = result {
        panic!("{name}: {error}");
    }
    let (Some(baseline), Some(end)) = (baseline, memory_in_use()) else {
        return;
    };
    eprintln!(
        "{name}: {} MiB after, baseline {} MiB",
        end >> 20,
        baseline >> 20
    );
    assert!(
        end <= baseline + baseline / 10,
        "{name}: memory {end} passed the baseline {baseline} by more than 10%"
    );
}

/// Every input the scheduled job kept, for each target.
fn regressions(target: &str) -> Vec<(PathBuf, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fuzz/regressions")
        .join(target);
    let mut inputs: Vec<(PathBuf, Vec<u8>)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && path.extension().is_none_or(|ext| ext != "md"))
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    inputs.sort();
    inputs
}

#[test]
fn fuzzed_records_hit_the_limits_without_crashing_and_memory_returns() {
    let sandbox = sandbox();
    // Compile the module and run each stage once before the baseline.
    fuzzing::record(sandbox, &support::insert(client(1, 1), 0, "warm"));
    let baseline = memory_in_use();

    for target in ["verdict", "record", "writer_record"] {
        let inputs = regressions(target);
        for (path, bytes) in &inputs {
            eprintln!("replaying {}", path.display());
            match target {
                "verdict" => fuzzing::verdict(sandbox, bytes),
                "record" => {
                    fuzzing::record(sandbox, bytes);
                }
                _ => {
                    fuzzing::writer_record(sandbox, bytes);
                }
            }
        }
    }

    run(
        "random bytes judged",
        baseline,
        512,
        prop::collection::vec(any::<u8>(), 0..512),
        |bytes| fuzzing::verdict(sandbox, &bytes),
    );
    run(
        "random bytes merged",
        baseline,
        128,
        prop::collection::vec(any::<u8>(), 0..256),
        |bytes| {
            fuzzing::record(sandbox, &bytes);
        },
    );
    // The generators that build yrs structure must reach integration, or
    // they test only the verdict.
    let merged = Cell::new(0);
    run(
        "random structs under the writer",
        baseline,
        256,
        prop::collection::vec(any::<u8>(), 0..256),
        |bytes| merged.set(merged.get() + fuzzing::writer_record(sandbox, &bytes)),
    );
    run("structured updates", baseline, 256, update(), |bytes| {
        merged.set(merged.get() + fuzzing::record(sandbox, &bytes))
    });
    run(
        "updates that read as the writer's own",
        baseline,
        256,
        plausible_update(),
        |bytes| merged.set(merged.get() + fuzzing::record(sandbox, &bytes)),
    );
    eprintln!("{} hostile records merged", merged.get());
    assert!(merged.get() > 100, "{} merged", merged.get());
    let merged = Cell::new(0);
    run("mutated honest updates", baseline, 128, mutated(), |bytes| {
        merged.set(merged.get() + fuzzing::record(sandbox, &bytes))
    });
    eprintln!("{} mutated records merged", merged.get());
    assert!(merged.get() > 10, "{} merged", merged.get());
    run("nested meta values", baseline, 64, nested_meta(), |data| {
        fuzzing::verdict(sandbox, &data)
    });
}
