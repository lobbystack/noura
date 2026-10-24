// The guest's verdict and integration, natively, on `data` as the yrs bytes
// of an update by card 1 and of a snapshot: a record that passes the
// verdict merges into a note holding a first document, the change it
// reports takes the first document's text to the note's on code points, and
// a document booted from the result's state opens. Shared by the guest
// targets.

use collab_sandbox::fuzzing::{snapshot_payload, update_payload};
use collab_sandbox_guest::document::{Note, first_document};
use collab_sandbox_guest::op::{Component, Op};
use collab_sandbox_guest::verdict::{Record, judge};

pub fn record(data: &[u8]) {
    let in_blob = data.first().is_some_and(|byte| byte & 1 == 1);
    for payload in [update_payload(data, in_blob), snapshot_payload(data, in_blob)] {
        let record = Record {
            payload: &payload,
            blob: in_blob.then_some(data),
            card_index: 1,
            card_count: 3,
        };
        if judge(&record).is_err() {
            continue;
        }
        let mut note = Note::new((3 << 29) + 1).expect("a client ID below 2^53");
        note.integrate(&first_document(BASE))
            .expect("a first document integrates");
        if let Ok((op, _)) = note.integrate(data) {
            assert_eq!(apply_on_code_points(BASE, &op), note.text(), "{op:?}");
            let state = note.state();
            let mut booted = Note::new(4 << 29).expect("a client ID below 2^53");
            booted.integrate(&state).expect("a device boots from the state");
            assert_eq!(booted.len() as usize, booted.text().encode_utf16().count());
        }
    }
}

const BASE: &str = "fuzz 😀 base\n";

/// `op` applied to `text`, after checking that each run of deletes and
/// inserts starts and ends on a code point of `text`, as the editor needs.
fn apply_on_code_points(text: &str, op: &Op) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    let on_code_point = |at: usize| at >= units.len() || !(0xDC00..0xE000).contains(&units[at]);
    let mut out = Vec::new();
    let mut at = 0usize;
    let mut change = None;
    for component in op.components() {
        match component {
            Component::Retain(n) => {
                if let Some(start) = change.take() {
                    assert!(on_code_point(start) && on_code_point(at), "{op:?}");
                }
                out.extend_from_slice(&units[at..at + *n as usize]);
                at += *n as usize;
            }
            Component::Insert(inserted) => {
                change.get_or_insert(at);
                out.extend(inserted.encode_utf16());
            }
            Component::Delete(n) => {
                change.get_or_insert(at);
                at += *n as usize;
            }
        }
    }
    if let Some(start) = change {
        assert!(on_code_point(start) && on_code_point(at), "{op:?}");
    }
    out.extend_from_slice(&units[at..]);
    String::from_utf16(&out).expect("valid UTF-16")
}
