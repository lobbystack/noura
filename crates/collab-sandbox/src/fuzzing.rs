//! What the fuzz suite feeds the sandbox: the proptest generators under
//! `cargo test` and the cargo-fuzz targets in `crates/collab-sandbox/fuzz`
//! both call these, and a plain test replays every crash the scheduled job
//! kept (`crates/collab-sandbox/fuzz/regressions`).
//!
//! Each entry point takes arbitrary bytes, runs them through the sandbox,
//! and panics only when the sandbox breaks a rule: the host must never
//! panic, a verdict must stay within its budget, and a note's text must
//! stay consistent whatever a record does to it.

use std::time::Instant;

use crate::note::{LineageStatus, Note, RemoteRecord};
use crate::op::Op;
use crate::verdict::{Record, verdict_budget};
use crate::{Event, Sandbox};

#[path = "../fuzz/inputs.rs"]
mod inputs;

pub use inputs::{snapshot_payload, update_payload, writer_yrs};

/// Judges `data` as a whole record: the first 8 bytes give the signer's card
/// index and the workspace's card count, a ninth byte picks whether the rest
/// splits into a payload and a blob, and the rest is the payload. The
/// verdict must stay within its budget whatever the bytes hold.
pub fn verdict(sandbox: &Sandbox, data: &[u8]) {
    let Some((numbers, rest)) = data.split_first_chunk::<8>() else {
        return;
    };
    let card_index = u32::from_le_bytes(numbers[..4].try_into().expect("four bytes"));
    let card_count = u32::from_le_bytes(numbers[4..].try_into().expect("four bytes"));
    let (payload, blob) = match rest.split_first() {
        Some((split, rest)) if split & 1 == 1 && !rest.is_empty() => {
            let at = usize::from(*split) % rest.len();
            let (payload, blob) = rest.split_at(at);
            (payload, Some(blob))
        }
        Some((_, rest)) => (rest, None),
        None => (rest, None),
    };
    let record = Record {
        payload,
        blob,
        card_index,
        card_count,
    };
    let judged = sandbox.verdict(&record);
    let bytes = payload.len() + blob.map_or(0, <[u8]>::len);
    assert!(
        judged.fuel <= verdict_budget(bytes as u64),
        "the verdict used {} fuel, past its budget",
        judged.fuel
    );
}

/// Merges `data` as the yrs bytes of a record into a note that holds a
/// first document, as an update of card 1 and as a snapshot, and checks
/// that the note stays consistent (see `check`). Returns how many of the
/// two records merged, so a generator can show it reaches integration.
pub fn record(sandbox: &Sandbox, data: &[u8]) -> usize {
    let in_blob = data.first().is_some_and(|byte| byte & 1 == 1);
    let first = sandbox.first_document(BASE).expect("a first document");
    let mut merged = 0;
    for (id, payload) in [
        (1, update_payload(data, in_blob)),
        (2, snapshot_payload(data, in_blob)),
    ] {
        let mut load = 0;
        let Ok(mut note) = Note::open(
            sandbox,
            vec![first.clone()],
            Box::new(move || {
                load += 1;
                Some((3 << 29) + load)
            }),
        ) else {
            panic!("a note holding a first document opens");
        };
        let effects = note.receive(
            RemoteRecord {
                id,
                writer: 1,
                card_count: 3,
                payload,
                blob: in_blob.then(|| data.to_vec()),
                place: Default::default(),
            },
            Instant::now(),
        );
        check(sandbox, &mut note, &effects.events, &effects.changes);
        merged += effects
            .events
            .iter()
            .filter(|event| matches!(event, Event::Merged { .. } | Event::Refused { .. }))
            .count();
    }
    merged
}

/// The text of the first document every record merges into: an emoji, so
/// records can split its surrogate pair.
const BASE: &str = "fuzz 😀 base\n";

/// A note's invariants after a record: the text matches the length the
/// note keeps, the changes the record sent the editor take its text from
/// [`BASE`] to the note's on code points alone (see `apply_on_code_points`),
/// a stopped note kept its last good state, and a device that boots from
/// the note's state shows the same text: integration refuses an item that
/// would land elsewhere there (§22).
fn check(sandbox: &Sandbox, note: &mut Note, events: &[Event], changes: &[Op]) {
    let text = note.text().expect("the note's text reads");
    assert_eq!(
        text.encode_utf16().count(),
        note.len() as usize,
        "the note's length matches its text: {events:?}"
    );
    assert_eq!(
        apply_on_code_points(BASE, changes),
        text,
        "the editor reaches the note's text: {changes:?}"
    );
    if let LineageStatus::Stopped(_) = note.status() {
        assert_eq!(text, BASE, "a stopped note keeps its last good state");
        return;
    }
    let state = note.state().expect("the note's state encodes");
    let mut booted = match Note::open(sandbox, vec![state], Box::new(|| Some(4 << 29))) {
        Ok(booted) => booted,
        Err(error) => panic!("a device boots from the note's state: {error}"),
    };
    let booted_text = booted.text().expect("the booted text reads");
    assert_eq!(booted_text.encode_utf16().count(), booted.len() as usize);
    assert_eq!(
        booted_text, text,
        "a booted device shows the note: {events:?}"
    );
}

/// `changes` applied to `text` as the editor applies them, splice by
/// splice, after checking that each splice starts and ends on a code point
/// of the text it applies to: the editor works on code points, so a splice
/// inside a surrogate pair would leave it with another text than the note.
fn apply_on_code_points(text: &str, changes: &[Op]) -> String {
    let on_code_point =
        |units: &[u16], at: usize| at >= units.len() || !(0xDC00..0xE000).contains(&units[at]);
    let mut units: Vec<u16> = text.encode_utf16().collect();
    for op in changes {
        for splice in op.splices() {
            let start = splice.start as usize;
            let end = start + splice.removed as usize;
            assert!(
                end <= units.len() && on_code_point(&units, start) && on_code_point(&units, end),
                "the splice {splice:?} of {op:?} splits a code point of {:?}",
                String::from_utf16_lossy(&units)
            );
            units.splice(start..end, splice.inserted.encode_utf16());
        }
    }
    String::from_utf16(&units).expect("the editor's text is valid UTF-16")
}

/// The same as [`record`] on [`writer_yrs`] of `data`.
pub fn writer_record(sandbox: &Sandbox, data: &[u8]) -> usize {
    record(sandbox, &writer_yrs(data))
}
