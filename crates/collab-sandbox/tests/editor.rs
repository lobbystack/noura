//! Seam 2: `editor-core` with a text-mode note whose document lives in the
//! collaboration sandbox. Keystrokes, input method composition, outside
//! edits and remote yrs records go in, and the tests check the editor's
//! text, selection and change events, and the document's merged changes,
//! which are what the file gets, against yrs outside the sandbox and the
//! Yjs and yrs interop fixtures.

mod support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use collab_sandbox::{
    Event as SandboxEvent, HandleError, LineageStatus, LoadReservation, LocalUpdate,
    MemoryReservation, NoteHandle, Record, RemoteRecord, ReservationStore, SandboxDocument,
    SharedNote, StopReason,
};
use editor_core::MdDoc;
use editor_core::input::{Advance, Event, Increment, Location, NoLayout, Region};
use editor_core::offset_types::Grapheme;
use serde_json::Value;
use support::{
    Lib0, cases, hex, insert, insert_after, insert_between, outside_yrs, remap_clients, sandbox,
    snapshot_record, update_record,
};

fn g(offset: usize) -> Grapheme {
    Grapheme(offset)
}

/// An editor on a text-mode note, the sync client's side of its document,
/// and the note's file body, which follows the document's merged changes.
struct Editor {
    doc: MdDoc,
    note: SharedNote,
    file: String,
}

/// A device whose card has index `card` in the workspace, with load numbers
/// from `reservation`.
fn device(card: u32, reservation: MemoryReservation) -> collab_sandbox::ClientIds {
    LoadReservation::client_ids(
        Arc::new(Mutex::new(LoadReservation::start(reservation).unwrap())),
        card,
    )
}

/// Opens a note from the yrs records its journal holds, on card 3's device.
fn open(journal: Vec<Vec<u8>>) -> Editor {
    open_on(journal, device(3, MemoryReservation::default()))
}

fn open_on(journal: Vec<Vec<u8>>, client_ids: collab_sandbox::ClientIds) -> Editor {
    let (handle, text) = NoteHandle::open(sandbox(), journal, client_ids).unwrap();
    let mut doc = MdDoc::new(&text);
    let document = SandboxDocument::new(handle);
    let note = document.handle();
    doc.enter_text_mode(Box::new(document));
    Editor {
        doc,
        note,
        file: text,
    }
}

impl Editor {
    fn caret(&mut self, offset: usize) {
        self.select(offset, offset);
    }

    fn select(&mut self, start: usize, end: usize) {
        self.doc.apply(
            [Event::Select {
                region: Region::BetweenLocations {
                    start: Location::Grapheme(g(start)),
                    end: Location::Grapheme(g(end)),
                },
            }],
            &NoLayout,
        );
    }

    fn type_text(&mut self, text: &str) {
        for ch in text.chars() {
            self.doc.apply(
                [Event::Replace {
                    region: Region::Selection,
                    text: ch.to_string(),
                    advance_cursor: true,
                }],
                &NoLayout,
            );
        }
    }

    fn backspace(&mut self) {
        self.doc.apply(
            [Event::Delete {
                region: Region::SelectionOrAdvance {
                    advance: Advance::By(Increment::Char),
                    backwards: true,
                },
            }],
            &NoLayout,
        );
    }

    fn undo(&mut self) {
        self.doc.apply([Event::Undo], &NoLayout);
    }

    fn redo(&mut self) {
        self.doc.apply([Event::Redo], &NoLayout);
    }

    /// Hands the sandbox a remote record.
    fn receive(&mut self, record: RemoteRecord) {
        self.note.receive(record).unwrap();
    }

    /// Waits for the sandbox to catch up and applies what it sent, as a
    /// frame does.
    fn settle(&mut self) {
        self.doc.sync_text_mode();
        self.note.settle().unwrap();
        self.doc.sync_text_mode();
        assert_eq!(self.note.take_errors(), []);
    }

    /// The document's text in the sandbox, once the file caught up with
    /// it: the file holds the same text.
    fn merged(&mut self) -> String {
        self.doc.sync_text_mode();
        let merged = self.note.merged_text().unwrap();
        assert_eq!(self.file(), merged, "the file follows the document");
        merged
    }

    /// The file body after the document's merged changes since the last
    /// call, which this takes without waiting for the sandbox.
    fn file(&mut self) -> &str {
        for op in self.note.take_merged_changes() {
            self.file = op.apply(&self.file).expect("a change fits the file");
        }
        &self.file
    }

    fn updates(&mut self) -> Vec<LocalUpdate> {
        self.doc.sync_text_mode();
        self.note.take_updates().unwrap()
    }

    fn events(&mut self) -> Vec<SandboxEvent> {
        self.note.take_events()
    }

    fn status(&mut self) -> LineageStatus {
        self.note.status()
    }
}

fn verdict_ok(card: u32, card_count: u32, update: &LocalUpdate) -> Vec<u8> {
    let payload = support::update_payload(update.load, &update.bytes);
    let verdict = sandbox().verdict(&Record {
        payload: &payload,
        blob: None,
        card_index: card,
        card_count,
    });
    assert_eq!(
        verdict.outcome,
        Ok(()),
        "the device's own update is readable"
    );
    payload
}

fn fixture_file(name: &str) -> Value {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/workspace-format/fixtures")
        .join(name);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn base64(value: &Value) -> Vec<u8> {
    STANDARD.decode(value.as_str().unwrap()).unwrap()
}

/// Card 1's load 17: where the Yjs fixture's client 17 goes, so its
/// updates are a signer's own.
fn yjs_client(client: u64) -> u64 {
    (1 << 29) + client
}

#[test]
fn yjs_updates_change_the_buffer_as_yrs_applies_them_outside_the_sandbox() {
    let fixture = fixture_file("yjs-text-v1.json");
    let base = base64(&fixture["base"]);
    let updates: Vec<Vec<u8>> = fixture["updates"]
        .as_array()
        .unwrap()
        .iter()
        .map(base64)
        .collect();
    let mut outside = vec![base.clone()];
    outside.extend(updates.iter().cloned());
    let expected = outside_yrs(&outside);

    // The note starts empty; the base arrives as a snapshot of card 1.
    let mut editor = open(vec![]);
    editor.receive(snapshot_record(1, 1, 1, &remap_clients(&base, yjs_client)));
    editor.settle();
    assert_eq!(editor.doc.text(), expected[0]);
    // The caret sits at the end of "remove", which no update touches.
    let caret = expected[0].chars().count() - 3;
    editor.caret(caret);

    for (index, update) in updates.iter().enumerate() {
        editor.receive(update_record(
            index as u64 + 2,
            1,
            1,
            17,
            &remap_clients(update, yjs_client),
        ));
        editor.settle();
        let state = fixture["states"][index].as_str().unwrap();
        assert_eq!(editor.doc.text(), expected[index + 1], "update {index}");
        assert_eq!(editor.doc.text(), state, "update {index}");
        // Each update replaced one character before the caret with another,
        // or added text after it, so the caret keeps its place.
        assert_eq!(
            editor.doc.selection(),
            (g(caret), g(caret)),
            "update {index}"
        );
    }
    assert_eq!(editor.doc.text(), fixture["text"].as_str().unwrap());
    assert_eq!(editor.merged(), editor.doc.text());

    // Every change the editor took from the sandbox is a collaborator's: it
    // isn't local, and none of it went back as this device's edit.
    let changes = editor.doc.take_changes();
    assert!(!changes.is_empty());
    assert!(changes.iter().all(|change| !change.local));
    assert_eq!(editor.updates(), []);
}

#[test]
fn a_yrs_update_changes_the_buffer_as_yrs_applies_it_outside_the_sandbox() {
    let fixture = fixture_file("yrs-text-v1.json");
    let base = base64(&fixture["base"]);
    let delta = base64(&fixture["delta"]);
    let expected = outside_yrs(&[base.clone(), delta.clone()]);
    assert_eq!(expected[0], fixture["baseText"].as_str().unwrap());
    assert_eq!(expected[1], fixture["text"].as_str().unwrap());

    // The fixture's client IDs already sit under cards 3 and 7.
    let mut editor = open(vec![]);
    editor.receive(snapshot_record(1, 3, 7, &base));
    editor.settle();
    assert_eq!(editor.doc.text(), expected[0]);
    editor.caret(2);
    editor.receive(update_record(2, 7, 7, 256_501_284, &delta));
    editor.settle();
    assert_eq!(editor.doc.text(), expected[1]);
    assert_eq!(editor.doc.selection(), (g(2), g(2)));
    assert_eq!(editor.updates(), []);
}

#[test]
fn local_edits_reach_the_document_in_utf16_offsets_as_this_devices_own() {
    let text = "A😀 e\u{301} 中文\r\nend";
    let first = sandbox().first_document(text).unwrap();
    let mut editor = open(vec![first.clone()]);
    assert_eq!(editor.doc.text(), text);

    // After the emoji, after the combining mark, inside the CJK pair and
    // at the end: byte offsets that each sit at another UTF-16 offset.
    editor.caret(2);
    editor.type_text("x");
    editor.caret(6);
    editor.type_text("🦀");
    editor.caret(9);
    editor.type_text("字");
    let end = editor.doc.text().chars().count();
    editor.caret(end);
    editor.backspace();
    editor.type_text("D");

    let expected = "A😀x e\u{301} 🦀中文字\r\nenD";
    assert_eq!(editor.doc.text(), expected);
    assert_eq!(editor.merged(), expected);

    // The edits go out as one update under card 3's first load, which
    // passes the verdict and gives yrs outside the sandbox the same text.
    let updates = editor.updates();
    assert_eq!(updates.len(), 1);
    assert_eq!(updates[0].load, 1);
    verdict_ok(3, 3, &updates[0]);
    let outside = outside_yrs(&[first, updates[0].bytes.clone()]);
    assert_eq!(outside[1], expected);
}

#[test]
fn composed_text_reaches_the_document_only_once_committed() {
    let mut editor = open(vec![sandbox().first_document("note").unwrap()]);
    editor.caret(4);
    for step in ["n", "に", "にh", "にほ"] {
        editor.doc.apply(
            [Event::Compose {
                text: step.into(),
                selected: None,
            }],
            &NoLayout,
        );
        assert_eq!(
            editor.merged(),
            "note",
            "{step:?} stays out of the document"
        );
        assert_eq!(editor.updates(), []);
    }
    editor.doc.apply(
        [Event::Commit {
            text: "日本".into(),
        }],
        &NoLayout,
    );
    assert_eq!(editor.doc.text(), "note日本");
    assert_eq!(editor.merged(), "note日本");
    assert_eq!(editor.updates().len(), 1);
}

#[test]
fn an_outside_edit_reaches_the_document_but_undo_skips_it() {
    let mut editor = open(vec![sandbox().first_document("one\n").unwrap()]);
    editor.caret(3);
    editor.type_text(" two");
    // Another app appends a line to the file.
    editor
        .doc
        .buffer
        .saved(editor.doc.buffer.current.seq, "one two\n".into());
    editor.doc.buffer.reload("one two\nthree\n".into());
    editor.doc.buffer.update();
    assert_eq!(editor.merged(), "one two\nthree\n");

    editor.undo();
    editor.settle();
    assert_eq!(editor.doc.text(), "one\nthree\n");
    assert_eq!(editor.merged(), "one\nthree\n");
}

#[test]
fn undo_and_redo_take_back_only_your_own_edits() {
    let mut editor = open(vec![insert(0, 0, "base")]);
    assert!(!editor.doc.can_undo() && !editor.doc.can_redo());
    editor.caret(4);
    editor.type_text(" mine");
    editor.settle();
    assert!(editor.doc.can_undo() && !editor.doc.can_redo());

    // A collaborator writes after the "b" while you type.
    let theirs = insert_between(
        support::client(2, 1),
        0,
        Some((0, 0)),
        Some((0, 1)),
        "theirs ",
    );
    editor.receive(update_record(1, 2, 3, 1, &theirs));
    editor.settle();
    assert_eq!(editor.doc.text(), "btheirs ase mine");

    editor.undo();
    editor.settle();
    assert_eq!(editor.doc.text(), "btheirs ase");
    assert_eq!(editor.merged(), "btheirs ase");
    // The editor's toolbar asks the document, whose undo holds only yours.
    assert!(!editor.doc.can_undo() && editor.doc.can_redo());

    editor.redo();
    editor.settle();
    assert_eq!(editor.doc.text(), "btheirs ase mine");
    assert!(editor.doc.can_undo() && !editor.doc.can_redo());

    // Undo with nothing of yours left to undo changes nothing.
    editor.undo();
    editor.undo();
    editor.settle();
    assert_eq!(editor.doc.text(), "btheirs ase");
    assert!(!editor.doc.can_undo());
}

#[test]
fn remote_text_never_goes_back_to_the_sandbox_as_a_local_edit() {
    let mut editor = open(vec![insert(0, 0, "x")]);
    editor.caret(1);
    editor.type_text("y");
    let mine = editor.updates();
    assert_eq!(mine.len(), 1);
    for id in 0..5u64 {
        let yrs = insert_after(
            support::client(2, 1),
            id,
            if id == 0 {
                (0, 0)
            } else {
                (support::client(2, 1), id - 1)
            },
            "r",
        );
        editor.receive(update_record(id, 2, 3, 1, &yrs));
        editor.settle();
    }
    assert_eq!(editor.doc.text(), "xrrrrry");
    // Nothing of the collaborator's goes out under this device's load.
    assert_eq!(editor.updates(), []);
    editor.type_text("z");
    let after = editor.updates();
    assert_eq!(after.len(), 1);
    let outside = outside_yrs(&[after[0].bytes.clone()]);
    // The update holds only the "z" and depends on text it doesn't hold.
    assert_eq!(outside[0], "");
}

#[test]
fn each_load_takes_a_new_load_number_and_its_updates_name_it() {
    let reservation = MemoryReservation::default();
    let shared = Arc::new(Mutex::new(
        LoadReservation::start(reservation.clone()).unwrap(),
    ));
    let first = sandbox().first_document("a").unwrap();
    let mut one = open_on(
        vec![first.clone()],
        LoadReservation::client_ids(shared.clone(), 5),
    );
    let mut two = open_on(vec![first], LoadReservation::client_ids(shared, 5));
    one.caret(1);
    one.type_text("b");
    two.caret(1);
    two.type_text("c");
    let one_update = one.updates();
    let two_update = two.updates();
    assert_eq!(one_update[0].load, 1);
    assert_eq!(two_update[0].load, 2);
    verdict_ok(5, 5, &one_update[0]);
    verdict_ok(5, 5, &two_update[0]);
    assert_eq!(reservation.mark(), 1_024);
}

#[test]
fn client_ids_past_32_bits_round_trip_through_the_editor() {
    // Card 8's first load is 2^32 + 1; card 2^24 − 1's last load is the
    // largest client ID, 2^53 − 1.
    let mut last = MemoryReservation::default();
    last.reserve((1 << 29) - 2).unwrap();
    for (card, reservation, client) in [
        (8u32, MemoryReservation::default(), 4_294_967_297u64),
        (16_777_215, last, (1 << 53) - 1),
    ] {
        let mut writer = open_on(vec![], device(card, reservation));
        writer.type_text("wide");
        let update = writer.updates().remove(0);
        assert_eq!(u64::from(update.load), client & ((1 << 29) - 1));
        let payload = verdict_ok(card, card, &update);

        let mut reader = open(vec![]);
        reader.receive(RemoteRecord {
            id: 1,
            writer: card,
            card_count: card,
            payload,
            blob: None,
            place: Default::default(),
        });
        reader.settle();
        assert_eq!(reader.doc.text(), "wide");
        // A device booted from the reader's document holds it too.
        let state = reader.note.state().unwrap();
        let booted = open(vec![state]);
        assert_eq!(booted.doc.text(), "wide");
    }

    // And the fixture's records of both client IDs.
    for case in cases("client_round_trip") {
        let card = case["card_index"].as_u64().unwrap() as u32;
        let mut reader = open(vec![]);
        reader.receive(RemoteRecord {
            id: 1,
            writer: card,
            card_count: card,
            payload: hex(&case["payload"]),
            blob: None,
            place: Default::default(),
        });
        reader.settle();
        assert_eq!(reader.doc.text(), case["text"]);
    }
}

#[test]
fn relative_positions_encode_and_decode_in_the_sandbox() {
    let mut editor = open(vec![insert(0, 0, "hello world")]);
    let position = editor.note.encode_position(6, true).unwrap();
    // "say " right before the "h".
    let theirs = Lib0::default()
        .var(1)
        .var(1)
        .var(support::client(2, 1))
        .var(0)
        .raw(&[0x44])
        .var(0)
        .var(0)
        .string("say ")
        .var(0)
        .0;
    editor.receive(update_record(1, 2, 3, 1, &theirs));
    editor.settle();
    assert_eq!(editor.doc.text(), "say hello world");
    assert_eq!(editor.note.decode_position(position).unwrap(), 10);
    assert!(editor.note.decode_position(vec![0xff, 0xff]).is_err());
}

#[test]
fn an_unreadable_record_leaves_text_selection_and_file_as_they_were() {
    let mut editor = open(vec![insert(0, 0, "keep me")]);
    editor.select(1, 4);
    let _ = editor.doc.take_changes();
    let unreadable: Vec<Value> = cases("verdict")
        .into_iter()
        .filter(|case| !case["expect"].is_null() && case["record"] == "text_update")
        .collect();
    assert!(unreadable.len() > 50);
    for (id, case) in unreadable.iter().enumerate() {
        let blob = (!case["blob"].is_null()).then(|| hex(&case["blob"]));
        editor.receive(RemoteRecord {
            id: id as u64,
            writer: case["card_index"].as_u64().unwrap() as u32,
            card_count: case["card_count"].as_u64().unwrap() as u32,
            payload: hex(&case["payload"]),
            blob,
            place: Default::default(),
        });
    }
    editor.settle();
    assert_eq!(editor.doc.text(), "keep me");
    assert_eq!(editor.doc.selection(), (g(1), g(4)));
    // No change event, so nothing writes the file.
    assert_eq!(editor.doc.take_changes(), []);
    assert_eq!(editor.merged(), "keep me");
    let verdicts = editor
        .events()
        .into_iter()
        .filter(|event| matches!(event, SandboxEvent::Verdict { verdict, .. } if verdict.outcome.is_err()))
        .count();
    assert_eq!(verdicts, unreadable.len());
    assert_eq!(editor.status(), LineageStatus::Merging);

    // The next valid record still applies.
    let theirs = insert_after(support::client(2, 1), 0, (0, 6), "!");
    editor.receive(update_record(999, 2, 3, 1, &theirs));
    editor.settle();
    assert_eq!(editor.doc.text(), "keep me!");
}

#[test]
fn an_update_that_depends_on_an_unreadable_record_waits_for_a_rewrite() {
    let pending = cases("pending");
    assert!(!pending.is_empty());
    for case in pending {
        let name = &case["name"];
        let mut editor = open(vec![hex(&case["base"])]);
        for step in case["steps"].as_array().unwrap() {
            let receive = &step["receive"];
            editor.receive(RemoteRecord {
                id: receive["id"].as_u64().unwrap(),
                writer: receive["card_index"].as_u64().unwrap() as u32,
                card_count: receive["card_count"].as_u64().unwrap() as u32,
                payload: hex(&receive["payload"]),
                blob: None,
                place: Default::default(),
            });
            editor.settle();
            assert_eq!(editor.doc.text(), step["text"], "{name}");
            let status = match editor.status() {
                LineageStatus::Merging => "merging",
                LineageStatus::Pending => "pending",
                LineageStatus::Stopped(_) => "stopped",
            };
            assert_eq!(status, step["status"], "{name}");
        }
    }
}

#[test]
fn a_trap_during_integration_stops_the_lineage_at_its_last_good_state() {
    for case in cases("trap") {
        let mut editor = open(vec![hex(&case["base"])]);
        editor.caret(1);
        editor.type_text("!");
        let _ = editor.updates();
        for step in case["steps"].as_array().unwrap() {
            let receive = &step["receive"];
            if step["trap"] == true {
                editor.note.trap_next_integration().unwrap();
            }
            editor.receive(RemoteRecord {
                id: receive["id"].as_u64().unwrap(),
                writer: receive["card_index"].as_u64().unwrap() as u32,
                card_count: receive["card_count"].as_u64().unwrap() as u32,
                payload: hex(&receive["payload"]),
                blob: None,
                place: Default::default(),
            });
            editor.settle();
        }
        assert!(
            editor.events().contains(&SandboxEvent::Rebuilt),
            "the device rebuilt the instance from the journal"
        );
        assert_eq!(
            editor.status(),
            LineageStatus::Stopped(StopReason::Trap),
            "{}",
            case["name"]
        );
        // The fixture's text with your "!", which follows the base's "x"
        // like the collaborator's "a" and sorts after it by client ID: the
        // last good state, which the file keeps.
        let expected = format!("{}!", case["text"].as_str().unwrap());
        assert_eq!(editor.merged(), expected);
        assert_eq!(editor.doc.text(), expected);
        // The device writes nothing more for the lineage, and the file
        // keeps the last good state while the note waits for your choice.
        editor.type_text("?");
        assert_eq!(editor.updates(), []);
        editor.settle();
        assert_eq!(editor.file(), expected);
    }
}

#[test]
fn an_overdrawn_writer_waits_while_others_keep_merging() {
    for case in cases("allowance") {
        let name = &case["name"];
        let mut editor = open(vec![hex(&case["base"])]);
        for (writer, balance) in case["balances"].as_object().unwrap() {
            editor
                .note
                .set_balance(writer.parse().unwrap(), balance.as_i64().unwrap())
                .unwrap();
        }
        let mut elapsed = 0;
        for step in case["steps"].as_array().unwrap() {
            if let Some(receive) = step.get("receive") {
                let id = receive["id"].as_u64().unwrap();
                editor.receive(RemoteRecord {
                    id,
                    writer: receive["card_index"].as_u64().unwrap() as u32,
                    card_count: receive["card_count"].as_u64().unwrap() as u32,
                    payload: hex(&receive["payload"]),
                    blob: None,
                    place: Default::default(),
                });
                editor.settle();
                let events = editor.events();
                let merged = events
                    .iter()
                    .any(|event| matches!(event, SandboxEvent::Merged { id: merged, .. } if *merged == id));
                let deferred = events.contains(&SandboxEvent::Deferred { id });
                let outcome = match (merged, deferred) {
                    (true, _) => "merged",
                    (_, true) => "deferred",
                    _ => "other",
                };
                assert_eq!(outcome, step["expect"], "{name} record {id}");
            } else {
                let at = step["after_seconds"].as_u64().unwrap();
                editor
                    .note
                    .advance_clock(Duration::from_secs(at - elapsed))
                    .unwrap();
                elapsed = at;
                editor.settle();
                let merged: Vec<u64> = editor
                    .events()
                    .iter()
                    .filter_map(|event| match event {
                        SandboxEvent::Merged { id, .. } => Some(*id),
                        _ => None,
                    })
                    .collect();
                assert_eq!(serde_json::json!(merged), step["expect_merged"], "{name}");
            }
        }
        assert_eq!(editor.doc.text(), case["text"], "{name}");
    }
}

#[test]
fn the_note_stays_editable_while_a_slow_record_decodes() {
    let mut editor = open(vec![insert(0, 0, "ab")]);
    editor.caret(2);
    editor.type_text("!");
    editor.backspace();
    assert_eq!(editor.merged(), "ab");
    let _ = editor.doc.take_changes();
    // The sandbox's thread stands still, as it does while a large record
    // decodes.
    editor.note.hold(true);
    let release = watchdog(&editor.note, Duration::from_secs(10));
    let theirs = insert_between(support::client(2, 1), 0, Some((0, 0)), Some((0, 1)), "R");
    editor.receive(update_record(1, 2, 3, 1, &theirs));
    editor.type_text("cd");
    editor.doc.sync_text_mode();
    // You keep typing on the last merged state, and nothing else changes.
    assert_eq!(editor.doc.text(), "abcd");
    assert_eq!(editor.doc.selection(), (g(4), g(4)));
    editor.backspace();
    editor.doc.sync_text_mode();
    assert_eq!(editor.doc.text(), "abc");
    // The editor's change events carry your queued edits, but the file
    // keeps its last merged state until they merge.
    let queued: Vec<String> = editor
        .doc
        .take_changes()
        .into_iter()
        .map(|c| c.text)
        .collect();
    assert_eq!(queued, ["c", "d", ""]);
    std::thread::sleep(Duration::from_millis(100));
    assert_eq!(editor.file(), "ab");

    // Once the record decodes, your edits apply after it, and the file
    // catches up in the same order.
    let _ = release.send(());
    editor.note.hold(false);
    editor.settle();
    assert_eq!(editor.doc.text(), "aRbc");
    assert_eq!(editor.merged(), "aRbc");
    assert_eq!(editor.file(), "aRbc");
    assert_eq!(editor.doc.selection(), (g(4), g(4)));
}

#[test]
fn a_note_opens_without_waiting_for_its_journal() {
    // The journal loads on the note's thread, which here waits for the test
    // to hand it its load number, as a costly journal keeps it busy.
    let (release, released) = std::sync::mpsc::channel::<()>();
    let released = Mutex::new(released);
    let mut ids = device(3, MemoryReservation::default());
    let client_ids: collab_sandbox::ClientIds = Box::new(move || {
        let _ = released
            .lock()
            .unwrap()
            .recv_timeout(Duration::from_secs(10));
        ids()
    });
    let started = Instant::now();
    let mut opening = NoteHandle::start(sandbox(), vec![insert(0, 0, "ab")], client_ids);
    assert!(started.elapsed() < Duration::from_secs(5));
    assert!(opening.poll().is_none(), "the journal still loads");
    release.send(()).unwrap();
    let (handle, text) = opening.wait().unwrap();
    assert_eq!(text, "ab");
    assert_eq!(handle.len(), 2);

    // A failed load, here for want of a load number, reports through poll,
    // once.
    let mut opening = NoteHandle::start(sandbox(), vec![], Box::new(|| None));
    let failed = loop {
        if let Some(result) = opening.poll() {
            break result;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert!(matches!(failed, Err(HandleError::Document(_))));
    assert!(matches!(opening.poll(), Some(Err(HandleError::Closed))));
}

#[test]
fn a_hostile_update_meets_the_editor_only_after_the_verdict() {
    // A record that inserts into a map under `content`: unreadable, so the
    // editor never sees it.
    let map = Lib0::default()
        .var(1)
        .var(1)
        .var(support::client(2, 1))
        .var(0)
        .raw(&[0x24])
        .var(1)
        .string("content")
        .string("key")
        .string("value")
        .var(0)
        .0;
    let mut editor = open(vec![insert(0, 0, "x")]);
    editor.receive(update_record(1, 2, 3, 1, &map));
    editor.settle();
    assert_eq!(editor.doc.text(), "x");
    assert!(editor.events().iter().any(|event| matches!(
        event,
        SandboxEvent::Verdict { verdict, .. } if verdict.outcome == Err(collab_sandbox::Unreadable::MapKey)
    )));
}

#[test]
fn surrogate_splits_reach_the_editor_as_they_reach_the_document() {
    let surrogates = cases("surrogate");
    assert_eq!(surrogates.len(), 5);
    for case in surrogates {
        let name = &case["name"];
        let expected = case["text"].as_str().unwrap();
        let mut editor = open(vec![hex(&case["base"])]);
        assert_eq!(editor.doc.text(), "a😀b", "{name}");
        // The caret after the "a", before the pair the record splits.
        editor.caret(1);
        editor.receive(RemoteRecord {
            id: 1,
            writer: case["card_index"].as_u64().unwrap() as u32,
            card_count: case["card_count"].as_u64().unwrap() as u32,
            payload: hex(&case["payload"]),
            blob: None,
            place: Default::default(),
        });
        editor.settle();
        assert_eq!(editor.doc.text(), expected, "{name}");
        assert_eq!(editor.merged(), expected, "{name}");
        assert_eq!(editor.doc.selection(), (g(1), g(1)), "{name}");

        // Keystrokes after the split land where the document puts them.
        editor.type_text("Y");
        let end = editor.doc.text().chars().count();
        editor.caret(end);
        editor.type_text("Z");
        editor.settle();
        let typed = format!("aY{}Z", &expected[1..]);
        assert_eq!(editor.doc.text(), typed, "{name}");
        assert_eq!(editor.merged(), typed, "{name}");
        editor.undo();
        editor.settle();
        assert_eq!(editor.doc.text(), expected, "{name}");
        assert_eq!(editor.merged(), expected, "{name}");
    }
}

/// Releases the note's thread after `after` unless told not to first, so a
/// test that would wait forever on a held thread fails instead.
fn watchdog(note: &SharedNote, after: Duration) -> std::sync::mpsc::Sender<()> {
    let (cancel, cancelled) = std::sync::mpsc::channel::<()>();
    let note = note.clone();
    std::thread::spawn(move || {
        if cancelled.recv_timeout(after).is_err() {
            note.hold(false);
        }
    });
    cancel
}

#[test]
fn the_sync_client_waits_for_a_slow_record_without_holding_up_the_editor() {
    let mut editor = open(vec![insert(0, 0, "ab")]);
    editor.caret(2);
    editor.type_text("!");
    editor.settle();
    // The sandbox's thread stands still, as it does while a large record
    // decodes, with a record ahead of everything else.
    editor.note.hold(true);
    let release = watchdog(&editor.note, Duration::from_secs(10));
    let theirs = insert_between(support::client(2, 1), 0, Some((0, 0)), Some((0, 1)), "R");
    editor.receive(update_record(1, 2, 3, 1, &theirs));

    // The sync client asks for your updates, a position for presence and
    // the merged text, each of which waits for the thread.
    let done = Arc::new(AtomicBool::new(false));
    let asking = {
        let note = editor.note.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            let updates = note.take_updates();
            let position = note.encode_position(0, true);
            let text = note.merged_text();
            done.store(true, Ordering::SeqCst);
            (updates, position, text)
        })
    };
    std::thread::sleep(Duration::from_millis(200));

    // Meanwhile the editor takes keystrokes and frames without waiting.
    let started = Instant::now();
    editor.type_text("cd");
    editor.doc.sync_text_mode();
    editor.backspace();
    editor.doc.sync_text_mode();
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the editor waited {:?} for the sandbox",
        started.elapsed()
    );
    assert!(
        !done.load(Ordering::SeqCst),
        "the sync client still waits for the record"
    );
    assert_eq!(editor.doc.text(), "ab!c");

    let _ = release.send(());
    editor.note.hold(false);
    let (updates, position, text) = asking.join().unwrap();
    assert_eq!(updates.unwrap().len(), 1);
    assert!(!position.unwrap().is_empty());
    assert!(text.unwrap().starts_with("aRb!"));
    editor.settle();
    assert_eq!(editor.doc.text(), "aRb!c");
    assert_eq!(editor.merged(), "aRb!c");
}

#[test]
fn closing_the_note_waits_for_no_record() {
    let editor = open(vec![insert(0, 0, "ab")]);
    let note = editor.note.clone();
    note.hold(true);
    let release = watchdog(&note, Duration::from_secs(10));
    let theirs = insert_after(support::client(2, 1), 0, (0, 1), "R");
    note.receive(update_record(1, 2, 3, 1, &theirs)).unwrap();

    let started = Instant::now();
    drop(editor);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "closing waited {:?} for the sandbox",
        started.elapsed()
    );

    // Once the record is done, the thread ends.
    let _ = release.send(());
    note.hold(false);
    assert_eq!(note.settle(), Err(HandleError::Closed));
    assert_eq!(note.take_updates(), Err(HandleError::Closed));
}

#[test]
fn a_collaborators_cursor_stays_on_its_character_as_text_changes_around_it() {
    let mut editor = open(vec![support::insert(0, 0, "a😀b")]);
    // The UTF-16 offset after the emoji, as a relative position.
    let position = editor.note.encode_position(3, false).unwrap();
    assert_eq!(editor.doc.offset_of_utf16(3).0, 2, "two graphemes in");

    // Text arrives ahead of the cursor, and the cursor moves with its
    // character.
    let theirs = insert_between(support::client(2, 1), 0, None, Some((0, 0)), "xy");
    editor.receive(update_record(1, 2, 3, 1, &theirs));
    editor.settle();
    assert_eq!(editor.doc.text(), "xya😀b");
    let index = editor.note.decode_position(position).unwrap();
    assert_eq!(index, 5);
    assert_eq!(editor.doc.offset_of_utf16(index as usize).0, 4);

    // Your own selection reads the same way.
    editor.caret(4);
    assert_eq!(editor.doc.selection_utf16(), (5, 5));
}

#[test]
fn an_outside_edit_applies_on_top_of_what_the_document_merged_since_its_base() {
    let mut editor = open(vec![sandbox().first_document("base\n").unwrap()]);
    // You type in the editor while the file holds an older body.
    editor.caret(0);
    editor.type_text("typed ");
    editor.settle();
    editor
        .note
        .merge_text("base\n".into(), "base\nfile\n".into(), 0)
        .unwrap();
    editor.settle();
    assert_eq!(editor.merged(), "typed base\nfile\n");
    assert_eq!(editor.doc.text(), "typed base\nfile\n");
}

#[test]
fn a_document_outlives_its_editor_for_the_sync_client() {
    let (mut handle, text) = NoteHandle::open(
        sandbox(),
        vec![sandbox().first_document("base\n").unwrap()],
        device(3, MemoryReservation::default()),
    )
    .unwrap();
    assert_eq!(text, "base\n");
    handle.outlive();
    let note = handle.shared();
    let mut doc = MdDoc::new(&text);
    doc.enter_text_mode(Box::new(SandboxDocument::new(handle)));
    drop(doc);
    assert_eq!(note.merged_text().unwrap(), "base\n", "the thread stays");
}
