//! The verdict budget against legitimate records (sync-v2.md §22): the
//! budget of 1,000,000 fuel units plus 1,000 per byte must sit far above
//! what any record an honest device writes needs, so that no machine and no
//! release ever judges one unreadable.
//!
//! Each record here is one an honest device writes: typing, pastes in
//! several scripts, snapshots of heavily edited and many-writer notes,
//! scattered deletions, a managed note's large frontmatter, and snapshots
//! at the 16 MiB rebase threshold and just under the 32 MiB stop. Fuel and memory count the same on every
//! machine, so `sync-v2-collaboration.json` records them exactly, beside
//! the time each took on the reference machine, and this test fails when
//! the module, the operator costs or the records change until the fixture
//! changes with them.
//!
//! Run with `--nocapture` to print the table the fixture records.

mod support;

use std::time::Instant;

use collab_sandbox::op::{Op, Splice};
use collab_sandbox::{Note, Record, RemoteRecord, VERDICT_MEMORY_BYTES, verdict_budget};
use serde_json::Value;
use support::{Lib0, client, fixture, sandbox};
use sync_protocol::codec::{Bytes, Encode, Sorted, Text};
use sync_protocol::records::{BlobRef, Content, Meta, Payload, PayloadKind};
use sync_protocol::types::{Digest, Id, SecretKey};

/// A legitimate record and the card numbers the verdict takes with it.
struct Sample {
    name: &'static str,
    payload: Vec<u8>,
    blob: Option<Vec<u8>>,
    card_count: u32,
}

fn meta(key: &str, value: &str) -> Meta {
    Meta {
        key: Text::new(key).unwrap(),
        seq: 0,
        value: Some(Bytes(value.as_bytes().to_vec())),
    }
}

fn content(yrs: Vec<u8>, in_blob: bool) -> (Content, Option<Vec<u8>>) {
    if in_blob {
        let blob = Content::Blob(BlobRef {
            blob: Id([3; 16]),
            ciphertext_length: yrs.len() as u64 + 16,
            ciphertext_digest: Digest([0; 32]),
            blob_key: SecretKey([1; 32]),
            plaintext_length: yrs.len() as u64,
        });
        (blob, Some(yrs))
    } else {
        (
            Content::Inline {
                bytes: Bytes(yrs.clone()),
            },
            None,
        )
    }
}

fn update(name: &'static str, load: u32, yrs: Vec<u8>) -> Sample {
    let (content, blob) = content(yrs, false);
    Sample {
        name,
        payload: Payload {
            rid: Id([7; 16]),
            kind: PayloadKind::TextUpdate {
                object_id: Text::new("").unwrap(),
                text_lineage: Id([9; 16]),
                load,
                meta: Sorted::new(vec![]).unwrap(),
                content,
            },
        }
        .to_bytes(),
        blob,
        card_count: 64,
    }
}

fn snapshot(name: &'static str, object_id: &str, meta: Vec<Meta>, yrs: Vec<u8>) -> Sample {
    let in_blob = yrs.len() > 200_000;
    let (content, blob) = content(yrs, in_blob);
    Sample {
        name,
        payload: Payload {
            rid: Id([7; 16]),
            kind: PayloadKind::TextSnapshot {
                object_id: Text::new(object_id).unwrap(),
                revision: Digest([0; 32]),
                text_lineage: Id([9; 16]),
                meta: Sorted::new(meta).unwrap(),
                content,
                origin: None,
            },
        }
        .to_bytes(),
        blob,
        card_count: 64,
    }
}

/// A note of card `card`'s first load, holding `journal`.
fn note(card: u64, journal: Vec<Vec<u8>>) -> Note {
    let mut load = 0;
    Note::open(
        sandbox(),
        journal,
        Box::new(move || {
            load += 1;
            Some(client(card, load))
        }),
    )
    .unwrap()
}

fn splice(note: &mut Note, start: u32, removed: u32, inserted: &str) {
    let op = Op::from_splices(
        note.len(),
        &[Splice {
            start,
            removed,
            inserted: inserted.into(),
        }],
    )
    .unwrap();
    note.local(&op, true, 0).unwrap();
}

/// One update holding `text` pasted into an empty note.
fn paste(name: &'static str, text: &str) -> Sample {
    let mut writer = note(1, vec![]);
    splice(&mut writer, 0, 0, text);
    update(name, 1, writer.take_updates().unwrap().remove(0).bytes)
}

/// A note of card 1 after `edits` keystrokes at scattered places, with one
/// deletion every `delete_every` of them when it isn't 0.
fn scattered(edits: u32, delete_every: u32) -> Note {
    let mut writer = note(1, vec![]);
    for i in 0..edits {
        let len = writer.len();
        if delete_every > 0 && i % delete_every == delete_every - 1 && len > 2 {
            splice(&mut writer, len / 2, 1, "");
        } else {
            splice(&mut writer, (i * 7_919) % (len + 1), 0, "a");
        }
    }
    writer
}

fn legitimate() -> Vec<Sample> {
    let mut samples = Vec::new();

    let mut typist = note(1, vec![sandbox().first_document("Hello\n").unwrap()]);
    splice(&mut typist, 5, 0, "!");
    samples.push(update(
        "typing one character",
        1,
        typist.take_updates().unwrap().remove(0).bytes,
    ));
    samples.push(paste(
        "pasting 200,000 ASCII characters",
        &"a".repeat(200_000),
    ));
    samples.push(paste("pasting 60,000 CJK characters", &"中".repeat(60_000)));
    samples.push(paste("pasting 50,000 emoji", &"😀".repeat(50_000)));

    let state = scattered(20_000, 0).state().unwrap();
    samples.push(snapshot(
        "a snapshot after 20,000 scattered keystrokes",
        "",
        vec![meta("path", "Notes/Plan.md")],
        state,
    ));
    let state = scattered(20_000, 3).state().unwrap();
    samples.push(snapshot(
        "a snapshot after 20,000 keystrokes and deletions",
        "",
        vec![meta("path", "Notes/Plan.md")],
        state,
    ));

    // 64 writers, each typing at scattered places in the note.
    let first = sandbox().first_document("shared\n").unwrap();
    let mut collector = note(64, vec![first.clone()]);
    for card in 1..64u64 {
        let mut writer = note(card, vec![collector.state().unwrap()]);
        for i in 0..40u32 {
            let len = writer.len();
            splice(&mut writer, (i * 31 + card as u32 * 17) % (len + 1), 0, "w");
        }
        let bytes = writer.take_updates().unwrap().remove(0).bytes;
        collector.receive(
            RemoteRecord {
                id: card,
                writer: card as u32,
                card_count: 64,
                payload: support::update_payload(1, &bytes),
                blob: None,
                place: Default::default(),
            },
            Instant::now(),
        );
    }
    samples.push(snapshot(
        "a snapshot of 64 writers",
        "",
        vec![meta("path", "Notes/Plan.md")],
        collector.state().unwrap(),
    ));

    // Deleting every other character of a pasted text leaves a delete set
    // of one range per deleted character.
    let mut deleter = note(
        1,
        vec![sandbox().first_document(&"ab".repeat(20_000)).unwrap()],
    );
    for i in 0..20_000u32 {
        splice(&mut deleter, i, 1, "");
    }
    samples.push(update(
        "deleting every other character of 40,000",
        1,
        deleter.take_updates().unwrap().remove(0).bytes,
    ));

    // A managed note whose frontmatter holds a long list of tags, numbers
    // and decimals, a long string and a nested mapping.
    let tags = format!(
        "[{}]",
        (0..2_000)
            .map(|i| format!("\"tag-{i}\""))
            .collect::<Vec<_>>()
            .join(",")
    );
    let numbers = format!(
        "[{}]",
        (0..10_000)
            .map(|i| (i * 37 % 1_000).to_string())
            .collect::<Vec<_>>()
            .join(",")
    );
    let decimals = format!(
        "[{}]",
        (1..5_000)
            .map(|i| format!("{}.{}", i % 100, i % 9 + 1))
            .collect::<Vec<_>>()
            .join(",")
    );
    let summary = format!("\"{}\"", "Summary text, with “quotes” and é. ".repeat(800));
    let nested = format!(
        "{{{}}}",
        (0..500)
            .map(|i| format!(
                "\"k{i:04}\":{{\"done\":true,\"due\":\"2026-10-{:02}\"}}",
                i % 28 + 1
            ))
            .collect::<Vec<_>>()
            .join(",")
    );
    let body = sandbox().first_document("# Plan\n\nBody.\n").unwrap();
    samples.push(snapshot(
        "a managed note with a large frontmatter",
        "note_01",
        vec![
            meta("p:decimals", &decimals),
            meta("p:id", "\"note_01\""),
            meta("p:nested", &nested),
            meta("p:numbers", &numbers),
            meta("p:summary", &summary),
            meta("p:tags", &tags),
            meta("p:type", "\"note\""),
            meta("path", "Notes/Plan.md"),
        ],
        body,
    ));

    // A snapshot at the 16 MiB rebase threshold, and one just under the
    // 32 MiB stop, which a note can reach between its rebase threshold and
    // the rebase.
    samples.push(snapshot(
        "a 16 MiB snapshot of scattered typing",
        "",
        vec![meta("path", "Notes/Plan.md")],
        scattered_snapshot(1_300_000),
    ));
    samples.push(snapshot(
        "a 32 MiB snapshot of scattered typing",
        "",
        vec![meta("path", "Notes/Plan.md")],
        scattered_snapshot(2_270_000),
    ));
    samples
}

/// The yrs bytes of a snapshot of `count` items of the shape scattered
/// typing leaves: one-character items that each name both neighbours, here
/// each typed between the one before it and a final "x".
fn scattered_snapshot(count: u64) -> Vec<u8> {
    let me = client(1, 1);
    let mut w = Lib0::default().var(2).var(1).var(0).var(0).raw(&[4]).var(1);
    w = w.string("content").string("x");
    w = w.var(count).var(me).var(0);
    let mut clock = 0;
    for i in 0..count {
        let text = ["a", "é", "中", "😀"][i as usize % 4];
        w = if i == 0 {
            w.raw(&[0x44]).var(0).var(0)
        } else {
            w.raw(&[0xc4]).var(me).var(clock - 1).var(0).var(0)
        };
        w = w.string(text);
        clock += text.encode_utf16().count() as u64;
    }
    w.var(0).0
}

#[test]
fn the_budget_sits_far_above_every_legitimate_record() {
    // Compile the module before timing.
    let _ = sandbox().first_document("");
    let recorded = &fixture()["sandbox"]["verdict"]["measured"];
    let mut table = Vec::new();
    let mut largest_per_byte = (0.0f64, "");
    for sample in legitimate() {
        let bytes = sample.payload.len() + sample.blob.as_ref().map_or(0, Vec::len);
        let started = Instant::now();
        let verdict = sandbox().verdict(&Record {
            payload: &sample.payload,
            blob: sample.blob.as_deref(),
            card_index: 1,
            card_count: sample.card_count,
        });
        let seconds = started.elapsed().as_secs_f64();
        assert_eq!(verdict.outcome, Ok(()), "{} is readable", sample.name);
        let budget = verdict_budget(bytes as u64);
        // Far above: each record needs at most a fifth of its budget. yrs
        // holds a decoded struct in about 13 times its encoded size, so the
        // 16 MiB snapshot needs close to half the verdict's memory, and the
        // 32 MiB one most of it.
        assert!(
            verdict.fuel * 5 <= budget,
            "{}: {} fuel of a budget of {budget}",
            sample.name,
            verdict.fuel
        );
        let share = if sample.name.contains("32 MiB") { 1 } else { 2 };
        assert!(
            verdict.memory * share <= VERDICT_MEMORY_BYTES as u64,
            "{}: {} bytes of memory",
            sample.name,
            verdict.memory
        );
        let per_byte = verdict.fuel as f64 / bytes as f64;
        if bytes >= 65_536 && per_byte > largest_per_byte.0 {
            largest_per_byte = (per_byte, sample.name);
        }
        eprintln!(
            "{:48} {:>10} bytes {:>12} fuel {:>7.1} per byte {:>5} MiB {:>8.4} s",
            sample.name,
            bytes,
            verdict.fuel,
            per_byte,
            verdict.memory >> 20,
            seconds
        );
        table.push(serde_json::json!({
            "record": sample.name,
            "bytes": bytes,
            "fuel": verdict.fuel,
            "memory_bytes": verdict.memory,
        }));
    }
    eprintln!(
        "largest fuel per byte: {:.1}, {}",
        largest_per_byte.0, largest_per_byte.1
    );
    let records = recorded["records"].as_array().cloned().unwrap_or_default();
    let found: Vec<Value> = records
        .iter()
        .map(|record| {
            serde_json::json!({
                "record": record["record"],
                "bytes": record["bytes"],
                "fuel": record["fuel"],
                "memory_bytes": record["memory_bytes"],
            })
        })
        .collect();
    assert_eq!(
        found,
        table,
        "the measurements changed: update MEASURED in \
         crates/sync-key-envelope/tests/v2_fixtures/collaboration.rs to\n{}",
        serde_json::to_string_pretty(&table).unwrap()
    );
}

/// The fuel a keystroke appended by card 2 costs its writer's allowance on
/// a note whose first document holds `text`.
fn keystroke_fuel(text: &str) -> u64 {
    let first = sandbox().first_document(text).unwrap();
    let mut note = note(3, vec![first]);
    let units = text.encode_utf16().count() as u64;
    let typed = support::insert_between(client(2, 1), 0, Some((0, units - 1)), None, "b");
    let effects = note.receive(
        support::update_record(1, 2, 3, 1, &typed),
        std::time::Instant::now(),
    );
    let fuel = effects
        .events
        .iter()
        .find_map(|event| match event {
            collab_sandbox::Event::Merged { fuel, .. } => Some(*fuel),
            _ => None,
        })
        .expect("the keystroke merges");
    assert_eq!(note.text().unwrap(), format!("{text}b"));
    fuel
}

/// A keystroke on a long note costs its writer's allowance about what it
/// costs on a short one. The integration's check of the text's size copies
/// the text only once a bound on its UTF-8 length passes 8 MiB, which a
/// note of 3,000,000 ASCII characters doesn't reach, so honest typists on
/// a note between 2.8 and 4 MiB aren't deferred.
#[test]
fn a_keystroke_on_a_long_note_costs_its_writer_little() {
    let short = keystroke_fuel("aaa");
    let long = keystroke_fuel(&"a".repeat(3_000_000));
    eprintln!("keystroke fuel: {short} on a short note, {long} on a 3,000,000-character one");
    assert!(
        long < short + 500_000,
        "{long} fuel on the long note against {short} on the short one"
    );
}

/// Ten thousand keystrokes between two takes, as typing offline leaves
/// them, merge into one update that the verdict reads well inside its
/// budget and another device integrates for little fuel, into the document
/// yrs outside the sandbox builds from it. Each keystroke stays its own
/// struct in the update, and yrs joins them as it integrates them.
#[test]
fn ten_thousand_keystrokes_merge_into_one_readable_update() {
    let first = sandbox().first_document("x").unwrap();
    let mut typist = note(1, vec![first.clone()]);
    for i in 0..10_000u32 {
        let typed = Op::from_splices(
            i + 1,
            &[Splice {
                start: i,
                removed: 0,
                inserted: "a".into(),
            }],
        )
        .unwrap();
        typist.local(&typed, true, u64::from(i) * 1_000).unwrap();
    }
    let updates = typist.take_updates().unwrap();
    assert_eq!(updates.len(), 1);
    let update = &updates[0];
    let record = support::update_record(1, 1, 3, update.load, &update.bytes);
    let verdict = sandbox().verdict(&Record {
        payload: &record.payload,
        blob: None,
        card_index: 1,
        card_count: 3,
    });
    assert_eq!(verdict.outcome, Ok(()));
    assert!(verdict.fuel * 5 <= verdict_budget(record.payload.len() as u64));
    let mut reader = note(3, vec![first.clone()]);
    let effects = reader.receive(record, std::time::Instant::now());
    let fuel = effects
        .events
        .iter()
        .find_map(|event| match event {
            collab_sandbox::Event::Merged { fuel, .. } => Some(*fuel),
            _ => None,
        })
        .unwrap_or_else(|| panic!("the update merges: {:?}", effects.events));
    eprintln!("ten thousand keystrokes: {fuel} fuel to integrate");
    // yrs joining the structs one by one from the right would copy the
    // joined text at each step, about 150,000,000 fuel.
    assert!(fuel < 60_000_000, "{fuel} fuel");
    assert_eq!(reader.text().unwrap(), typist.text().unwrap());
    assert_eq!(
        reader.state().unwrap(),
        support::outside_state(&[first, update.bytes.clone()])
    );
}
