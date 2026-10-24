// The guest's verdict and integration, natively, on `data` as the yrs bytes
// of an update by card 1 and of a snapshot: a record that passes the
// verdict merges into a note holding a first document, and a document booted
// from the result's state opens. Shared by the guest targets.

use collab_sandbox::fuzzing::{snapshot_payload, update_payload};
use collab_sandbox_guest::document::{Note, first_document};
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
        note.integrate(&first_document("fuzz 😀 base\n"))
            .expect("a first document integrates");
        if note.integrate(data).is_ok() {
            let state = note.state();
            let mut booted = Note::new(4 << 29).expect("a client ID below 2^53");
            booted.integrate(&state).expect("a device boots from the state");
            assert_eq!(booted.len() as usize, booted.text().encode_utf16().count());
        }
    }
}
