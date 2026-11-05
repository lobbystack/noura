//! A note's lineage at seam 2 (sync-v2.md §22): the canonical order, the
//! snapshot check, the rule on where an item may land, and what a note keeps
//! of deferred records and debts across a reopen.

mod support;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use collab_sandbox::{
    DEFERRED_MEMORY_BYTES, Event, JournalRecord, LineageStatus, Note, Opened, Place, REFILL_PERIOD,
    RemoteRecord,
};
use serde_json::Value;
use support::{
    Lib0, cases, client, client_ids, hex, insert, insert_after, note, sandbox, snapshot_payload,
    update_payload,
};
use sync_protocol::crypto::hash;
use sync_protocol::ids::text_lineage_id;
use sync_protocol::records::Cover;
use sync_protocol::types::{Digest, Id};
use workspace_format::text_note::{NoteParts, compose_note, note_parts};

const WRITER: u32 = 2;

fn device(card: u32) -> Id {
    Id([card as u8; 16])
}

fn place(card: u32, counter: u64) -> Place {
    Place {
        device: device(card),
        counter,
        covers: Vec::new(),
    }
}

fn update(id: u64, card: u32, counter: u64, yrs: &[u8]) -> RemoteRecord {
    RemoteRecord {
        id,
        writer: card,
        card_count: 4,
        payload: update_payload(1, yrs),
        blob: None,
        place: place(card, counter),
    }
}

fn snapshot(id: u64, card: u32, counter: u64, covers: &[(u32, u64)], yrs: &[u8]) -> RemoteRecord {
    RemoteRecord {
        id,
        writer: card,
        card_count: 4,
        payload: snapshot_payload(yrs),
        blob: None,
        place: Place {
            covers: covers
                .iter()
                .map(|(card, counter)| Cover {
                    device: device(*card),
                    counter: *counter,
                })
                .collect(),
            ..place(card, counter)
        },
    }
}

fn first(text: &str) -> Vec<u8> {
    sandbox().first_document(text).expect("a first document")
}

fn events(note: &mut Note, record: RemoteRecord) -> Vec<Event> {
    note.receive(record, Instant::now()).events
}

/// A note reached through the journal as a device that restarted holds it.
fn reopened(records: Vec<JournalRecord>) -> Note {
    Note::open_lineage(
        sandbox(),
        Opened {
            records,
            ..Opened::default()
        },
        client_ids(),
    )
    .expect("the note opens")
}

fn merged(events: &[Event]) -> bool {
    events.iter().any(|e| matches!(e, Event::Merged { .. }))
}

/// A delete of `len` elements of `target` at `clock`, by nobody's struct.
fn delete(target: u64, clock: u64, len: u64) -> Vec<u8> {
    Lib0::default()
        .var(0)
        .var(1)
        .var(target)
        .var(1)
        .var(clock)
        .var(len)
        .0
}

#[test]
fn updates_of_one_signer_keep_their_chain_order_while_one_is_pending() {
    let mut note = note(vec![first("x")]);
    let writer = client(WRITER.into(), 1);
    // The update at clocks 0 to 2 never arrived, so this one waits in yrs.
    let waits = insert_after(writer, 3, (writer, 2), "d");
    note.receive(update(10, WRITER, 2, &waits), Instant::now());
    assert_eq!(note.status(), LineageStatus::Pending);

    // A later update of the signer at the same clocks waits behind it.
    let later = insert_after(writer, 3, (writer, 2), "z");
    let reported = events(&mut note, update(11, WRITER, 3, &later));
    assert_eq!(reported, [Event::Deferred { id: 11 }]);

    // The rewrite supplies what the first depends on, and then the later
    // update runs and shows it reuses the clock.
    let base = insert_after(writer, 0, (0, 0), "abc");
    let rewrite = snapshot(12, 3, 1, &[], &base);
    let reported = events(&mut note, rewrite);
    assert!(merged(&reported), "{reported:?}");
    assert_eq!(note.status(), LineageStatus::Merging);
    assert_eq!(note.text().unwrap(), "xabcd");
    assert!(
        reported.contains(&Event::Disagrees {
            id: 11,
            writer: WRITER
        }),
        "{reported:?}"
    );
}

/// The bytes from ticket 54's Comments: an item whose right origin lies left
/// of its origin, then an insert with no origins into a text that isn't
/// empty.
const HOSTILE: &[u8] = &[
    0x01, 0x02, 0x81, 0x80, 0x80, 0x80, 0x02, 0x00, 0xc1, 0x00, 0x07, 0x00, 0x01, 0x01, 0x04, 0x01,
    0x07, 0x63, 0x6f, 0x6e, 0x74, 0x65, 0x6e, 0x74, 0x03, 0xe4, 0xb8, 0xad, 0x00,
];

#[test]
fn an_item_that_would_land_elsewhere_on_a_booted_device_is_refused() {
    let mut note = note(vec![first("fuzz 😀 base\n")]);
    let reported = events(&mut note, update(1, 1, 1, HOSTILE));
    assert!(reported.contains(&Event::Refused { id: 1 }), "{reported:?}");
    assert_eq!(note.text().unwrap(), "fuzz 😀 base\n");
    assert_eq!(note.status(), LineageStatus::Merging);
    // The next valid record merges.
    let honest = insert_after(client(2, 1), 0, (0, 12), "!");
    assert!(merged(&events(&mut note, update(2, 2, 1, &honest))));
    assert_eq!(note.text().unwrap(), "fuzz 😀 base\n!");
}

#[test]
fn a_snapshot_that_holds_a_refused_item_is_refused_too() {
    let mut note = note(vec![first("base")]);
    let hostile = Lib0::default()
        .var(1)
        .var(1)
        .var(client(1, 1))
        .var(0)
        .raw(&[0xc4])
        .var(0)
        .var(3)
        .var(0)
        .var(1)
        .string("q")
        .var(0)
        .0;
    let reported = events(&mut note, snapshot(1, 1, 1, &[], &hostile));
    assert!(reported.contains(&Event::Refused { id: 1 }), "{reported:?}");
    assert_eq!(note.text().unwrap(), "base");
}

#[test]
fn two_first_inserts_into_an_empty_lineage_both_merge() {
    let mut note = note(vec![first("")]);
    let a = insert(client(1, 1), 0, "a");
    let b = insert(client(2, 1), 0, "b");
    assert!(merged(&events(&mut note, update(1, 1, 1, &a))));
    assert!(merged(&events(&mut note, update(2, 2, 1, &b))));
    assert_eq!(note.text().unwrap().len(), 2);
    // A writer's second item with no origins can't be honest: its first
    // item was there to see.
    let again = insert(client(1, 1), 1, "c");
    let reported = events(&mut note, update(3, 1, 2, &again));
    assert!(reported.contains(&Event::Refused { id: 3 }), "{reported:?}");
}

/// A note with `base`, an edit `"xy"` after it by card 1, and that edit's
/// delete of two base elements by card 2, with the journal's records.
fn note_with_a_delete() -> (Note, Vec<JournalRecord>, Vec<u8>, Vec<u8>) {
    let base = first("abcd");
    let edit = insert_after(client(1, 1), 0, (0, 3), "xy");
    let removal = delete(0, 1, 2);
    let mut note = note(vec![base.clone()]);
    note.receive(update(5, 1, 1, &edit), Instant::now());
    note.receive(update(6, 2, 1, &removal), Instant::now());
    assert_eq!(note.text().unwrap(), "adxy");
    let records = vec![
        JournalRecord {
            id: 0,
            snapshot: true,
            place: Place::default(),
            bytes: base.clone(),
        },
        JournalRecord {
            id: 5,
            snapshot: false,
            place: place(1, 1),
            bytes: edit.clone(),
        },
        JournalRecord {
            id: 6,
            snapshot: false,
            place: place(2, 1),
            bytes: removal,
        },
    ];
    (note, records, base, edit)
}

#[test]
fn an_honest_snapshot_agrees_and_merges() {
    let (mut holder, _, _, _) = note_with_a_delete();
    let state = holder.state().unwrap();
    let (mut other, _, _, _) = note_with_a_delete();
    let reported = events(&mut other, snapshot(9, 3, 1, &[(1, 1), (2, 1)], &state));
    assert!(merged(&reported), "{reported:?}");
    assert!(
        !reported
            .iter()
            .any(|event| matches!(event, Event::Disagrees { .. })),
        "{reported:?}"
    );
    assert_eq!(other.text().unwrap(), "adxy");
}

#[test]
fn a_snapshot_that_shows_a_covered_delete_makes_the_device_rebuild() {
    let (mut other, records, _, edit) = note_with_a_delete();
    // The snapshot holds the first document and the edit, without the
    // delete it covers.
    let mut carrier = note(vec![first("abcd")]);
    carrier.receive(update(5, 1, 1, &edit), Instant::now());
    let state = carrier.state().unwrap();
    let hostile = snapshot(9, 3, 1, &[(1, 1), (2, 1)], &state);

    let reported = events(&mut other, hostile.clone());
    assert!(
        reported.contains(&Event::Disagrees { id: 9, writer: 3 }),
        "{reported:?}"
    );
    // Every record the snapshot covers left the live set, so canonical
    // order holds the snapshot alone.
    assert_eq!(other.text().unwrap(), "abcdxy");

    // A restart between applying the snapshot and checking it gives the
    // same: the journal kept the covered records until the check.
    let mut restarted = reopened(records);
    let reported = events(&mut restarted, hostile);
    assert!(
        reported.contains(&Event::Disagrees { id: 9, writer: 3 }),
        "{reported:?}"
    );
    assert_eq!(restarted.text().unwrap(), "abcdxy");
}

#[test]
fn a_snapshot_that_lacks_a_covered_element_makes_the_device_rebuild() {
    let (mut other, _, _, _) = note_with_a_delete();
    let hostile = snapshot(9, 3, 1, &[(1, 1)], &first("abcd"));
    let reported = events(&mut other, hostile);
    assert!(
        reported.contains(&Event::Disagrees { id: 9, writer: 3 }),
        "{reported:?}"
    );
}

#[test]
fn a_snapshot_that_swaps_a_pair_for_replacements_disagrees() {
    let writer = client(1, 1);
    let pair = insert_after(writer, 0, (0, 0), "😀");
    let mut holder = note(vec![first("a")]);
    holder.receive(update(5, 1, 1, &pair), Instant::now());
    let substituted = insert_after(writer, 0, (0, 0), "\u{fffd}\u{fffd}");
    let base = first("a");
    // A snapshot of the first document and the substituted update.
    let mut carrier = note(vec![base]);
    carrier.receive(update(5, 1, 1, &substituted), Instant::now());
    let state = carrier.state().unwrap();
    let reported = events(&mut holder, snapshot(9, 3, 1, &[(1, 1)], &state));
    assert!(
        reported.contains(&Event::Disagrees { id: 9, writer: 3 }),
        "{reported:?}"
    );
    assert_eq!(holder.text().unwrap(), "a\u{fffd}\u{fffd}");
}

#[test]
fn a_real_split_lets_a_replacement_match() {
    let writer = client(1, 1);
    let pair = insert_after(writer, 0, (0, 0), "😀");
    let inside = insert_after(client(2, 1), 0, (writer, 0), "q");
    let mut holder = note(vec![first("a")]);
    holder.receive(update(5, 1, 1, &pair), Instant::now());
    holder.receive(update(6, 2, 1, &inside), Instant::now());
    let state = holder.state().unwrap();
    let (mut other, reported) = {
        let mut other = note(vec![first("a")]);
        other.receive(update(5, 1, 1, &pair), Instant::now());
        other.receive(update(6, 2, 1, &inside), Instant::now());
        let reported = events(&mut other, snapshot(9, 3, 1, &[(1, 1), (2, 1)], &state));
        (other, reported)
    };
    assert!(merged(&reported), "{reported:?}");
    assert_eq!(other.text().unwrap(), holder.text().unwrap());
}

#[test]
fn a_device_that_integrates_as_records_arrive_matches_one_that_bootstraps() {
    let base = first("hello");
    let a = insert_after(client(1, 1), 0, (0, 4), " world");
    let b = insert_after(client(2, 1), 0, (0, 4), " there");
    let c = insert_after(client(1, 1), 6, (client(1, 1), 5), "!");
    let mut arrivals = note(vec![base.clone()]);
    for record in [
        update(2, 1, 1, &a),
        update(3, 2, 1, &b),
        update(4, 1, 2, &c),
    ] {
        arrivals.receive(record, Instant::now());
    }
    let mut journal = reopened(vec![
        JournalRecord {
            id: 1,
            snapshot: true,
            place: Place::default(),
            bytes: base,
        },
        JournalRecord {
            id: 2,
            snapshot: false,
            place: place(1, 1),
            bytes: a,
        },
        JournalRecord {
            id: 3,
            snapshot: false,
            place: place(2, 1),
            bytes: b,
        },
        JournalRecord {
            id: 4,
            snapshot: false,
            place: place(1, 2),
            bytes: c,
        },
    ]);
    assert_eq!(arrivals.text().unwrap(), journal.text().unwrap());
}

#[test]
fn deferred_records_stay_within_their_memory_and_come_back_from_the_journal() {
    let journal: Arc<Mutex<BTreeMap<u64, RemoteRecord>>> = Arc::default();
    let source = journal.clone();
    let reloads = Arc::new(AtomicUsize::new(0));
    let counter = reloads.clone();
    let mut note = Note::open_lineage(
        sandbox(),
        Opened {
            records: vec![JournalRecord {
                id: 0,
                snapshot: true,
                place: Place::default(),
                bytes: first("x"),
            }],
            balances: vec![(WRITER, -1)],
            loader: Some(Box::new(move |id| {
                counter.fetch_add(1, Ordering::Relaxed);
                source.lock().unwrap().get(&id).cloned()
            })),
        },
        client_ids(),
    )
    .unwrap();
    let start = Instant::now();
    assert_eq!(note.balance(WRITER, start), -1);

    // A writer in debt keeps sending near-limit records.
    let writer = client(WRITER.into(), 1);
    let chunk = "y".repeat(200_000);
    let mut origin = (0, 0);
    let mut clock = 0;
    for id in 1..=100 {
        let yrs = insert_after(writer, clock, origin, &chunk);
        let record = update(id, WRITER, id, &yrs);
        journal.lock().unwrap().insert(id, record.clone());
        note.receive(record, start);
        origin = (writer, clock + 199_999);
        clock += 200_000;
    }
    assert_eq!(note.deferred(), 100);
    assert!(
        note.deferred_bytes() <= DEFERRED_MEMORY_BYTES,
        "{}",
        note.deferred_bytes()
    );

    // The refill runs them all, the ones only the journal held included.
    let mut later = start;
    while note.deferred() > 0 {
        later += REFILL_PERIOD + Duration::from_secs(1);
        note.wake(later);
    }
    assert_eq!(note.deferred_bytes(), 0);
    assert!(reloads.load(Ordering::Relaxed) > 0, "the journal held some");
    // The records past 8 MiB of text stop the lineage, as any record that
    // jumps past it does.
    assert_eq!(note.len(), 1 + 41 * 200_000);
    assert!(note.is_stopped());
}

#[test]
fn reopening_a_note_keeps_each_writers_debt() {
    let mut note = note(vec![first("x")]);
    let start = Instant::now();
    note.set_balance(WRITER, -5, start);
    let balances = note.balances(start);
    assert_eq!(balances, [(WRITER, -5)]);
    let mut again = Note::open_lineage(
        sandbox(),
        Opened {
            records: vec![JournalRecord {
                id: 0,
                snapshot: true,
                place: Place::default(),
                bytes: first("x"),
            }],
            balances,
            loader: None,
        },
        client_ids(),
    )
    .unwrap();
    let writer = client(WRITER.into(), 1);
    let yrs = insert_after(writer, 0, (0, 0), "z");
    let reported = events(&mut again, update(1, WRITER, 1, &yrs));
    assert_eq!(reported, [Event::Deferred { id: 1 }]);
}

fn record_of(value: &Value) -> RemoteRecord {
    update(
        value["id"].as_u64().unwrap(),
        value["card_index"].as_u64().unwrap() as u32,
        value["counter"].as_u64().unwrap(),
        &hex(&value["yrs"]),
    )
}

fn journal_of(base: Vec<u8>, records: &[Value]) -> Vec<JournalRecord> {
    let mut journal = vec![JournalRecord {
        id: 0,
        snapshot: true,
        place: Place::default(),
        bytes: base,
    }];
    journal.extend(records.iter().map(|value| JournalRecord {
        id: value["id"].as_u64().unwrap(),
        snapshot: false,
        place: place(
            value["card_index"].as_u64().unwrap() as u32,
            value["counter"].as_u64().unwrap(),
        ),
        bytes: hex(&value["yrs"]),
    }));
    journal
}

#[test]
fn first_snapshots_follow_the_fixture() {
    let found = cases("first_snapshot");
    assert!(found.len() >= 11);
    for case in found {
        let name = &case["name"];
        let file = case["file"].as_str().unwrap();
        let parts = note_parts(case["path"].as_str().unwrap(), file.as_bytes());
        let (properties, body) = match (parts, case["parts"].as_str().unwrap()) {
            (NoteParts::Outside, "outside") => continue,
            (NoteParts::Managed { properties, body }, "managed") => (properties, body),
            (NoteParts::Unmanaged { body }, "unmanaged") => (Default::default(), body),
            (other, wanted) => panic!("{name}: {other:?}, not {wanted}"),
        };
        assert_eq!(body, case["body"], "{name}");
        assert_eq!(
            serde_json::to_value(&properties).unwrap(),
            case["properties"],
            "{name}"
        );
        let document = first(&body);
        assert_eq!(document, hex(&case["document"]), "{name}");
        let rid: [u8; 16] = hex(&case["rid"]).try_into().unwrap();
        let base: [u8; 32] = hex(&case["base_digest"]).try_into().unwrap();
        assert_eq!(
            text_lineage_id(&Id(rid), &Digest(base), &document)
                .0
                .to_vec(),
            hex(&case["lineage"]),
            "{name}"
        );
        let composed = compose_note(&properties, &body).unwrap();
        assert_eq!(composed, case["composed"], "{name}");
        assert_eq!(
            hash(composed.as_bytes()).to_hex(),
            case["revision"],
            "{name}"
        );
    }
}

#[test]
fn items_land_as_the_fixture_says() {
    let found = cases("origin_order");
    assert!(found.len() >= 6);
    for case in found {
        let name = &case["name"];
        let mut note = note(vec![first(case["base_text"].as_str().unwrap())]);
        for (index, before) in case["before"].as_array().into_iter().flatten().enumerate() {
            let record = update(index as u64 + 1, 1, index as u64 + 1, &hex(before));
            note.receive(record, Instant::now());
        }
        let card = case["card_index"].as_u64().unwrap() as u32;
        let reported = events(&mut note, update(9, card, 9, &hex(&case["yrs"])));
        let refused = reported.contains(&Event::Refused { id: 9 });
        let expected = case["expect"] == "refused";
        assert_eq!(refused, expected, "{name}: {reported:?}");
        assert_eq!(merged(&reported), !expected, "{name}");
        // A device that boots from the result shows the same text.
        let state = note.state().unwrap();
        let mut booted = support::note(vec![state]);
        assert_eq!(booted.text().unwrap(), note.text().unwrap(), "{name}");
    }
}

#[test]
fn the_snapshot_check_follows_the_fixture() {
    let found = cases("snapshot_check");
    assert!(found.len() >= 6);
    for case in found {
        let name = &case["name"];
        let base = first(case["base_text"].as_str().unwrap());
        let records = case["records"].as_array().unwrap();
        let mut holder = if case["restart"] == true {
            reopened(journal_of(base, records))
        } else {
            let mut holder = note(vec![base]);
            for record in records {
                holder.receive(record_of(record), Instant::now());
            }
            holder
        };
        let covers: Vec<(u32, u64)> = case["covers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|cover| {
                (
                    cover[0].as_u64().unwrap() as u32,
                    cover[1].as_u64().unwrap(),
                )
            })
            .collect();
        let snapshot = snapshot(99, 3, 1, &covers, &hex(&case["snapshot"]));
        let reported = events(&mut holder, snapshot);
        let disagrees = reported.contains(&Event::Disagrees { id: 99, writer: 3 });
        assert_eq!(
            disagrees,
            case["expect"] == "disagrees",
            "{name}: {reported:?}"
        );
        assert_eq!(holder.text().unwrap(), case["text"], "{name}");
    }
}

#[test]
fn canonical_order_follows_the_fixture() {
    let found = cases("canonical_order");
    assert!(!found.is_empty());
    for case in found {
        let name = &case["name"];
        let base = first(case["base_text"].as_str().unwrap());
        let records = case["records"].as_array().unwrap();
        let mut arrivals = note(vec![base.clone()]);
        // Records arrive in any order across signers, and in chain order
        // for each one.
        for index in [1, 0, 2] {
            arrivals.receive(record_of(&records[index]), Instant::now());
        }
        let mut booted = reopened(journal_of(base, records));
        assert_eq!(booted.text().unwrap(), case["text"], "{name}");
        assert_eq!(arrivals.text().unwrap(), case["text"], "{name}");
    }
}

/// A snapshot the fuzz suite found: client 0's clocks 0 to 4 deleted, then a
/// string up to clock 13 that names an element of card 1, which nobody holds.
/// Over [`PAIR_NOTE`], whose units sit at clocks 0 to 12, only clock 13 is
/// new, and it holds the low half of an emoji. It also replays through
/// `fuzzing::record`, from `fuzz/regressions/record`.
const WAITING_SNAPSHOT: [u8; 39] = [
    1, 2, 0, 0, 129, 129, 128, 128, 128, 2, 0, 5, 132, 129, 128, 128, 128, 2, 0, 18, 204, 129, 228,
    184, 173, 97, 97, 204, 129, 204, 129, 228, 184, 173, 240, 159, 152, 128, 0,
];
const PAIR_NOTE: &str = "fuzz 😀 base\n";

fn booted_text(state: Vec<u8>) -> String {
    note(vec![state]).text().unwrap()
}

#[test]
fn a_state_holds_nothing_of_a_snapshot_that_waits() {
    let hostile = snapshot(9, 3, 1, &[], &WAITING_SNAPSHOT);
    let mut device = note(vec![first(PAIR_NOTE)]);
    let reported = events(&mut device, hostile.clone());
    assert!(
        reported.contains(&Event::Disagrees { id: 9, writer: 3 }),
        "{reported:?}"
    );
    assert_eq!(device.status(), LineageStatus::Pending);
    assert_eq!(device.text().unwrap(), PAIR_NOTE);

    let state = device.state().unwrap();
    assert_eq!(booted_text(state.clone()), PAIR_NOTE);

    // A second device took the same record, then the snapshot the first
    // writes: both show the same text.
    let mut other = note(vec![first(PAIR_NOTE)]);
    events(&mut other, hostile);
    events(&mut other, snapshot(10, 2, 1, &[], &state));
    assert_eq!(other.text().unwrap(), device.text().unwrap());
}

#[test]
fn a_state_holds_nothing_of_an_update_that_waits() {
    let writer = client(1, 1);
    let held = insert_after(writer, 0, (0, 0), "😀");
    // The same clocks again and one more, after an element nobody holds.
    let waits = insert_after(writer, 0, (client(2, 1), 0), "😀b");
    let shown = "a😀";

    let mut device = note(vec![first("a")]);
    device.receive(update(5, 1, 1, &held), Instant::now());
    device.receive(update(6, 1, 2, &waits), Instant::now());
    assert_eq!(device.status(), LineageStatus::Pending);
    assert_eq!(device.text().unwrap(), shown);
    let state = device.state().unwrap();
    assert_eq!(booted_text(state.clone()), shown);

    // The update stays live, so a device that boots from the state still
    // takes it, and reaches the text of the device that held it all along.
    let mut booted = note(vec![state]);
    booted.receive(update(6, 1, 2, &waits), Instant::now());
    assert_eq!(booted.text().unwrap(), shown);
    let dependency = insert_after(client(2, 1), 0, (0, 0), "q");
    let arrives = update(7, 2, 1, &dependency);
    device.receive(arrives.clone(), Instant::now());
    booted.receive(arrives, Instant::now());
    assert_eq!(booted.text().unwrap(), device.text().unwrap());
}
