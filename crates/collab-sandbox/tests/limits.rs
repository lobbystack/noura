//! Oversized, deeply nested and decompressing records hit the sandbox's
//! limits without crashing or hanging, and the process's memory returns to
//! within 10% of where it started after each one (sync-v2.md §22).
//!
//! One test runs every scenario in turn, so no other test's allocations
//! land in its measurements.

mod support;

/// This test's allocator: see `support/direct.rs`.
#[cfg(unix)]
#[path = "support/direct.rs"]
mod direct;

#[cfg(unix)]
#[global_allocator]
static ALLOCATOR: direct::Direct = direct::Direct;

use std::time::Instant;

use collab_sandbox::{
    Event, Note, Record, RemoteRecord, StopReason, Unreadable, VERDICT_MEMORY_BYTES,
};
use support::{
    Lib0, client, client_ids, insert, memory_in_use, path_meta, sandbox, update_payload,
};
use sync_protocol::codec::{Bytes, Encode, Sorted, Text};
use sync_protocol::records::{BlobRef, Content, Meta, Payload, PayloadKind};
use sync_protocol::types::{Digest, Id, SecretKey};

const ME: u64 = (1 << 29) + 1;

/// A `text_update` by card 1's first load whose yrs bytes travel in a blob.
fn blob_record(id: u64, yrs: Vec<u8>) -> RemoteRecord {
    let payload = Payload {
        rid: Id([7; 16]),
        kind: PayloadKind::TextUpdate {
            object_id: Text::new("").unwrap(),
            text_lineage: Id([9; 16]),
            load: 1,
            meta: Sorted::new(vec![path_meta("Notes/Plan.md")]).unwrap(),
            content: Content::Blob(BlobRef {
                blob: Id([3; 16]),
                ciphertext_length: yrs.len() as u64 + 16,
                ciphertext_digest: Digest([0; 32]),
                blob_key: SecretKey([1; 32]),
                plaintext_length: yrs.len() as u64,
            }),
        },
    }
    .to_bytes();
    RemoteRecord {
        id,
        writer: 1,
        card_count: 2,
        payload,
        blob: Some(yrs),
        place: Default::default(),
    }
}

fn inline_record(id: u64, yrs: &[u8]) -> RemoteRecord {
    RemoteRecord {
        id,
        writer: 1,
        card_count: 2,
        payload: update_payload(1, yrs),
        blob: None,
        place: Default::default(),
    }
}

/// Card 1's update of `count` structs after `origin`, alternating one-letter
/// strings and one-unit deletions, so yrs can't squash them together.
fn many_structs(clock: u64, origin: (u64, u64), count: u64) -> Vec<u8> {
    let mut w = Lib0::default().var(1).var(count).var(ME).var(clock);
    let mut previous = origin;
    for i in 0..count {
        w = w.raw(&[if i % 2 == 0 { 0x84 } else { 0x81 }]);
        w = w.var(previous.0).var(previous.1);
        w = if i % 2 == 0 { w.string("a") } else { w.var(1) };
        previous = (ME, clock + i);
    }
    w.var(0).0
}

/// A delete set of card 1's update deleting one unit of every `step` of
/// `client`'s clocks, `count` times.
fn scattered_deletes(target: u64, count: u64, step: u64) -> Vec<u8> {
    let mut w = Lib0::default().var(0).var(1).var(target).var(count);
    for i in 0..count {
        w = w.var(i * step + 1).var(1);
    }
    w.0
}

fn merged(events: &[Event], id: u64) -> bool {
    events
        .iter()
        .any(|event| matches!(event, Event::Merged { id: merged, .. } if *merged == id))
}

fn stopped(events: &[Event]) -> Option<StopReason> {
    events.iter().find_map(|event| match event {
        Event::Stopped { reason, .. } => Some(*reason),
        _ => None,
    })
}

/// Runs `scenario` and checks that the process's memory afterward sits
/// within 10% of `baseline`.
fn run(baseline: Option<u64>, name: &str, scenario: fn()) {
    let started = Instant::now();
    scenario();
    let (Some(baseline), Some(end)) = (baseline, memory_in_use()) else {
        return;
    };
    eprintln!(
        "{name}: {} MiB after, baseline {} MiB, {:?}",
        end >> 20,
        baseline >> 20,
        started.elapsed()
    );
    assert!(
        end <= baseline + baseline / 10,
        "{name}: memory {end} passed the baseline {baseline} by more than 10%"
    );
}

/// A verdict that would need more than 512 MiB fails on the memory limit,
/// and the next valid record still merges.
fn verdict_past_its_memory() {
    let mut note = support::note(vec![insert(0, 0, "x")]);
    let huge = blob_record(1, vec![0; VERDICT_MEMORY_BYTES + (1 << 20)]);
    let effects = note.receive(huge, Instant::now());
    assert!(
        effects.events.iter().any(|event| matches!(
            event,
            Event::Verdict { id: 1, verdict } if verdict.outcome == Err(Unreadable::Memory)
        )),
        "{:?}",
        effects.events
    );
    let effects = note.receive(inline_record(2, &insert(ME, 0, "y")), Instant::now());
    assert!(merged(&effects.events, 2));
    assert!(!note.is_stopped());
}

/// A record that would take the text past 8 MiB stops the lineage, and the
/// note keeps its last good state.
fn text_past_8_mib() {
    let mut note = support::note(vec![insert(0, 0, "x")]);
    let text = "a".repeat((8 << 20) + 1);
    let effects = note.receive(blob_record(1, insert(ME, 0, &text)), Instant::now());
    assert_eq!(stopped(&effects.events), Some(StopReason::TextTooLarge));
    assert_eq!(note.text().unwrap(), "x");
}

/// A record whose integration passes 10,000,000,000 fuel units stops the
/// lineage: scattered deletions each split one long string, and each split
/// copies the rest of it.
fn integration_past_its_fuel() {
    let base = insert(0, 0, &"b".repeat(4 << 20));
    let mut note = support::note(vec![base]);
    let effects = note.receive(
        blob_record(1, scattered_deletes(0, 400_000, 8)),
        Instant::now(),
    );
    assert_eq!(stopped(&effects.events), Some(StopReason::Fuel));
    assert_eq!(note.len(), 4 << 20);
}

/// A record that would take the encoded document past 32 MiB stops the
/// lineage.
fn document_past_32_mib() {
    // About ten bytes per struct: eleven records of 300,000 take the
    // document to about 32 MiB, and the text stays far below 8 MiB.
    let per_record = 300_000;
    let mut journal = vec![insert(0, 0, "x")];
    let mut origin = (0, 0);
    for record in 0..11 {
        let clock = record * per_record;
        journal.push(many_structs(clock, origin, per_record));
        origin = (ME, clock + per_record - 1);
    }
    let mut note = Note::open(sandbox(), journal, client_ids()).unwrap();
    let more = many_structs(11 * per_record, origin, 400_000);
    let effects = note.receive(blob_record(1, more), Instant::now());
    assert_eq!(
        stopped(&effects.events),
        Some(StopReason::DocumentTooLarge),
        "{:?}",
        effects.events
    );
}

/// Decompression bombs: a few bytes that claim billions of clocks.
fn decompression_bombs() {
    let mut note = support::note(vec![insert(0, 0, "x")]);
    let deleted = Lib0::default()
        .var(1)
        .var(1)
        .var(ME)
        .var(0)
        .raw(&[0x81])
        .var(0)
        .var(0)
        .var((1 << 32) - 2)
        .var(0)
        .0;
    let effects = note.receive(inline_record(1, &deleted), Instant::now());
    assert!(merged(&effects.events, 1), "{:?}", effects.events);
    let range = Lib0::default()
        .var(0)
        .var(1)
        .var(client(2, 1))
        .var(1)
        .var(0)
        .var((1 << 32) - 1)
        .0;
    let effects = note.receive(inline_record(2, &range), Instant::now());
    assert!(merged(&effects.events, 2));
    assert!(effects.events.contains(&Event::Pending(true)));
    let skip = Lib0::default()
        .var(1)
        .var(2)
        .var(ME)
        .var((1 << 32) - 4)
        .raw(&[10])
        .var(1)
        .raw(&[0x84])
        .var(0)
        .var(0)
        .string("z")
        .var(0)
        .0;
    let effects = note.receive(inline_record(3, &skip), Instant::now());
    assert!(merged(&effects.events, 3) || stopped(&effects.events).is_some());
    assert_eq!(note.text().unwrap(), "x");
}

/// Nesting as deep as a payload allows, in a property value.
fn deep_nesting() {
    let depth = 120_000;
    let value = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
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
                bytes: Bytes(insert(ME, 0, "deep")),
            },
        },
    }
    .to_bytes();
    let judge = |payload: &[u8]| {
        sandbox()
            .verdict(&Record {
                payload,
                blob: None,
                card_index: 1,
                card_count: 1,
            })
            .outcome
    };
    assert_eq!(judge(&payload), Ok(()));
    let mut unbalanced = payload.clone();
    let at = unbalanced.iter().rposition(|byte| *byte == b']').unwrap();
    unbalanced[at] = b'[';
    assert_eq!(judge(&unbalanced), Err(Unreadable::MetaValue));
}

/// Many records in a row leave nothing behind.
fn many_records() {
    let mut note = support::note(vec![insert(0, 0, "x")]);
    for i in 0..200u64 {
        let origin = if i == 0 { (0, 0) } else { (ME, i - 1) };
        let yrs = Lib0::default()
            .var(1)
            .var(1)
            .var(ME)
            .var(i)
            .raw(&[0x84])
            .var(origin.0)
            .var(origin.1)
            .string("n")
            .var(0)
            .0;
        let effects = note.receive(inline_record(i, &yrs), Instant::now());
        assert!(merged(&effects.events, i));
    }
    assert_eq!(note.len(), 201);
}

#[test]
fn records_past_the_limits_stop_cleanly_and_memory_returns() {
    // Compile the module and run each stage once before the baseline.
    many_records();
    let baseline = memory_in_use();
    run(baseline, "verdict past 512 MiB", verdict_past_its_memory);
    run(baseline, "text past 8 MiB", text_past_8_mib);
    run(
        baseline,
        "integration past 10,000,000,000 fuel units",
        integration_past_its_fuel,
    );
    run(baseline, "document past 32 MiB", document_past_32_mib);
    run(baseline, "decompression bombs", decompression_bombs);
    run(baseline, "deep nesting", deep_nesting);
    run(baseline, "many records", many_records);
}
