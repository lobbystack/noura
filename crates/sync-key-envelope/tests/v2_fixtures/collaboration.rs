//! `sync-v2-collaboration.json`: the formats of text mode (§22): the text
//! lineage ID and yrs client IDs, and the collaboration sandbox: the guest
//! module's hash, the verdict budget and the Wasmtime operator costs it
//! assumes, the raw-check and verdict cases, the surrogate-pair cases, the
//! round trip of client IDs above 2^32, and base states for the writer
//! allowance. `crates/collab-sandbox` runs the sandbox cases; ticket 55 adds
//! the merge cases.

use serde_json::{Value, json};
use sync_protocol::codec::{Bytes, Encode, Sorted, Text};
use sync_protocol::crypto::hash;
use sync_protocol::ids::{text_lineage_id, yrs_client_id};
use sync_protocol::records::{BlobRef, Content, Meta, Payload, PayloadKind};
use sync_protocol::tag::Tag;
use sync_protocol::types::{Id, to_hex};

use super::common::{Fixture, id, secret};

/// The guest module the app runs, as `crates/collab-sandbox` embeds it.
const MODULE: &str = "crates/collab-sandbox/guest/collab_sandbox_guest.wasm";

/// The operators that cost no fuel; every other costs one unit.
const FREE_OPERATORS: [&str; 8] = [
    "Block",
    "Drop",
    "Else",
    "End",
    "Loop",
    "Nop",
    "Return",
    "Unreachable",
];

/// Wasmtime's costs for work that grows with an operand, per unit of work.
const VARIABLE_COSTS: [(&str, u8); 16] = [
    ("array_copy_per_element", 1),
    ("array_fill_per_element", 1),
    ("array_init_data_per_element", 1),
    ("array_init_elem_per_element", 1),
    ("array_new_data_per_element", 1),
    ("array_new_default_per_element", 1),
    ("array_new_elem_per_element", 1),
    ("array_new_per_element", 1),
    ("memory_copy_per_byte", 1),
    ("memory_fill_per_byte", 1),
    ("memory_grow_per_page", 0),
    ("memory_init_per_byte", 1),
    ("table_copy_per_element", 1),
    ("table_fill_per_element", 1),
    ("table_grow_per_element", 1),
    ("table_init_per_element", 1),
];

/// Card 1's first load: the client ID most cases write under.
const ME: u64 = (1 << 29) + 1;
/// Card 2's first load.
const OTHER: u64 = (2 << 29) + 1;
/// 2^32 − 1, the most a first clock plus lengths may reach.
const MAX_CLOCK: u64 = (1 << 32) - 1;

const GC: u8 = 0;
const DELETED: u8 = 1;
const STRING: u8 = 4;
const SKIP: u8 = 10;
const HAS_ORIGIN: u8 = 0x80;
const HAS_RIGHT_ORIGIN: u8 = 0x40;
const HAS_PARENT_SUB: u8 = 0x20;

/// A lib0 v1 writer, for yrs bytes that honest and hostile writers make.
#[derive(Default)]
struct Lib0(Vec<u8>);

impl Lib0 {
    fn var(mut self, mut value: u64) -> Self {
        loop {
            let byte = (value & 0x7f) as u8;
            value >>= 7;
            if value == 0 {
                self.0.push(byte);
                return self;
            }
            self.0.push(byte | 0x80);
        }
    }

    fn raw(mut self, bytes: &[u8]) -> Self {
        self.0.extend_from_slice(bytes);
        self
    }

    fn string(self, text: &str) -> Self {
        self.var(text.len() as u64).raw(text.as_bytes())
    }

    /// A struct section entry header: struct count, client, first clock.
    fn run(self, structs: u64, client: u64, clock: u64) -> Self {
        self.var(structs).var(client).var(clock)
    }

    /// An item with no origins whose parent is the root `content`.
    fn root_item(self, content: u8) -> Self {
        self.raw(&[content]).var(1).string("content")
    }

    fn done(self) -> Vec<u8> {
        self.0
    }
}

/// One struct section run of `client` inserting `text` at the start of
/// `content`, and an empty delete set.
fn insert(client: u64, clock: u64, text: &str) -> Vec<u8> {
    Lib0::default()
        .var(1)
        .run(1, client, clock)
        .root_item(STRING)
        .string(text)
        .var(0)
        .done()
}

/// An update holding only a delete set: `ranges` of `client`.
fn deletes(client: u64, ranges: &[(u64, u64)]) -> Vec<u8> {
    let mut out = Lib0::default()
        .var(0)
        .var(1)
        .var(client)
        .var(ranges.len() as u64);
    for (clock, len) in ranges {
        out = out.var(*clock).var(*len);
    }
    out.done()
}

/// A string item inserted after `origin` and before `right`.
fn insert_between(
    client: u64,
    clock: u64,
    origin: Option<(u64, u64)>,
    right: Option<(u64, u64)>,
    text: &str,
) -> Vec<u8> {
    let mut info = STRING;
    if origin.is_some() {
        info |= HAS_ORIGIN;
    }
    if right.is_some() {
        info |= HAS_RIGHT_ORIGIN;
    }
    let mut out = Lib0::default().var(1).run(1, client, clock).raw(&[info]);
    for (client, clock) in [origin, right].into_iter().flatten() {
        out = out.var(client).var(clock);
    }
    if origin.is_none() && right.is_none() {
        out = out.var(1).string("content");
    }
    out.string(text).var(0).done()
}

fn meta(key: &str, seq: u64, value: Option<&str>) -> Meta {
    Meta {
        key: Text::new(key).unwrap(),
        seq,
        value: value.map(|value| Bytes(value.as_bytes().to_vec())),
    }
}

fn meta_list(entries: Vec<Meta>) -> Sorted<Meta, sync_protocol::records::ByKey> {
    Sorted::new(entries).unwrap()
}

fn lineage() -> Id {
    id("collaboration lineage")
}

fn update_payload(object_id: &str, load: u32, meta: Vec<Meta>, content: Content) -> Vec<u8> {
    Payload {
        rid: id("note rid"),
        kind: PayloadKind::TextUpdate {
            object_id: Text::new(object_id).unwrap(),
            text_lineage: lineage(),
            load,
            meta: meta_list(meta),
            content,
        },
    }
    .to_bytes()
}

fn snapshot_payload(object_id: &str, meta: Vec<Meta>, content: Content) -> Vec<u8> {
    Payload {
        rid: id("note rid"),
        kind: PayloadKind::TextSnapshot {
            object_id: Text::new(object_id).unwrap(),
            revision: hash(b"snapshot"),
            text_lineage: lineage(),
            meta: meta_list(meta),
            content,
            origin: None,
        },
    }
    .to_bytes()
}

fn inline(bytes: Vec<u8>) -> Content {
    Content::Inline {
        bytes: Bytes(bytes),
    }
}

fn blob_ref(file: &[u8], plaintext_length: u64) -> Content {
    Content::Blob(BlobRef {
        blob: id("collaboration blob"),
        ciphertext_length: file.len() as u64 + 16,
        ciphertext_digest: hash(b"collaboration blob ciphertext"),
        blob_key: secret("collaboration blob key"),
        plaintext_length,
    })
}

/// A verdict case on an unmanaged note's update by card 1's first load.
fn update_case(name: &str, yrs: Vec<u8>, expect: Option<&str>) -> Value {
    verdict_case(
        name,
        "text_update",
        update_payload(
            "",
            1,
            vec![meta("path", 0, Some("Notes/Plan.md"))],
            inline(yrs.clone()),
        ),
        None,
        (1, 2),
        Some(&yrs),
        expect,
    )
}

fn verdict_case(
    name: &str,
    record: &str,
    payload: Vec<u8>,
    blob: Option<&[u8]>,
    (card_index, card_count): (u32, u32),
    yrs: Option<&[u8]>,
    expect: Option<&str>,
) -> Value {
    json!({
        "kind": "verdict",
        "name": name,
        "record": record,
        "card_index": card_index,
        "card_count": card_count,
        "yrs": yrs.map(to_hex),
        "payload": to_hex(&payload),
        "blob": blob.map(to_hex),
        "expect": expect,
    })
}

/// The lib0 rules: varints, ranges, duplicates, zero lengths, UTF-8, and
/// what an item may hold.
fn raw_checks(f: &mut Fixture) {
    let readable = |name: &str, yrs: Vec<u8>| update_case(name, yrs, None);
    let unreadable = |name: &str, yrs: Vec<u8>, code: &str| update_case(name, yrs, Some(code));

    f.case(readable("readable_insert", insert(ME, 0, "hello")));

    // A struct count of 1 padded to two bytes.
    let padded = Lib0::default()
        .var(1)
        .raw(&[0x81, 0x00])
        .var(ME)
        .var(0)
        .root_item(STRING)
        .string("hi")
        .var(0)
        .done();
    f.case(unreadable(
        "varint_not_shortest",
        padded,
        "varint_not_shortest",
    ));
    let mut client_bytes = Lib0::default().var(ME).done();
    *client_bytes.last_mut().unwrap() |= 0x80;
    client_bytes.push(0x00);
    let padded_client = Lib0::default()
        .var(1)
        .var(1)
        .raw(&client_bytes)
        .var(0)
        .root_item(STRING)
        .string("hi")
        .var(0)
        .done();
    f.case(unreadable(
        "client_id_not_shortest",
        padded_client,
        "varint_not_shortest",
    ));

    f.case(unreadable(
        "client_id_2_pow_53",
        insert(1 << 53, 0, "hi"),
        "varint_range",
    ));
    let origin_2_pow_53 = insert_between(ME, 0, Some((1 << 53, 0)), None, "hi");
    f.case(unreadable(
        "origin_client_2_pow_53",
        origin_2_pow_53,
        "varint_range",
    ));
    f.case(unreadable(
        "delete_set_client_2_pow_53",
        deletes(1 << 53, &[(0, 1)]),
        "varint_range",
    ));
    let clients_2_pow_32 = Lib0::default().var(1 << 32).done();
    f.case(unreadable(
        "count_2_pow_32",
        clients_2_pow_32,
        "varint_range",
    ));
    f.case(unreadable(
        "clock_2_pow_32",
        insert(ME, 1 << 32, "hi"),
        "varint_range",
    ));
    let length_2_pow_32 = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .root_item(DELETED)
        .var(1 << 32)
        .var(0)
        .done();
    f.case(unreadable(
        "length_2_pow_32",
        length_2_pow_32,
        "varint_range",
    ));
    let marker_2_pow_32 = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .raw(&[STRING])
        .var(1 << 32)
        .string("content")
        .string("hi")
        .var(0)
        .done();
    f.case(unreadable(
        "parent_marker_2_pow_32",
        marker_2_pow_32,
        "varint_range",
    ));
    // Eleven bytes of continuation: past 64 bits.
    let mut endless = vec![0xff; 10];
    endless.push(0x01);
    f.case(unreadable("varint_past_64_bits", endless, "varint_range"));

    // "hi" takes clocks c and c + 1, so the clock after it is c + 2.
    f.case(readable(
        "clock_reaches_2_pow_32_minus_1",
        insert(ME, MAX_CLOCK - 2, "hi"),
    ));
    f.case(unreadable(
        "clock_passes_2_pow_32_minus_1",
        insert(ME, MAX_CLOCK - 1, "hi"),
        "clock_overflow",
    ));
    f.case(readable(
        "delete_range_reaches_2_pow_32_minus_1",
        deletes(0, &[(MAX_CLOCK - 2, 2)]),
    ));
    f.case(unreadable(
        "delete_range_passes_2_pow_32_minus_1",
        deletes(0, &[(MAX_CLOCK - 1, 2)]),
        "delete_range_overflow",
    ));

    let twice = Lib0::default()
        .var(2)
        .run(1, ME, 0)
        .root_item(STRING)
        .string("a")
        .run(1, ME, 1)
        .root_item(STRING)
        .string("b")
        .var(0)
        .done();
    f.case(unreadable(
        "client_twice_in_structs",
        twice,
        "duplicate_client",
    ));
    let twice_deleted = Lib0::default()
        .var(0)
        .var(2)
        .var(0)
        .var(1)
        .var(0)
        .var(1)
        .var(0)
        .var(1)
        .var(1)
        .var(1)
        .done();
    f.case(unreadable(
        "client_twice_in_delete_set",
        twice_deleted,
        "duplicate_client",
    ));

    let skip_zero = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .raw(&[SKIP])
        .var(0)
        .var(0)
        .done();
    f.case(unreadable("skip_of_length_0", skip_zero, "zero_length"));
    f.case(unreadable(
        "string_of_length_0",
        insert(ME, 0, ""),
        "zero_length",
    ));
    let deleted_zero = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .root_item(DELETED)
        .var(0)
        .var(0)
        .done();
    f.case(unreadable(
        "deletion_of_length_0",
        deleted_zero,
        "zero_length",
    ));
    f.case(unreadable(
        "delete_range_of_length_0",
        deletes(0, &[(0, 0)]),
        "zero_length",
    ));

    let bad_utf8 = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .root_item(STRING)
        .var(2)
        .raw(&[0xc3, 0x28])
        .var(0)
        .done();
    f.case(unreadable("string_not_utf8", bad_utf8, "invalid_utf8"));
    let root_not_utf8 = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .raw(&[STRING])
        .var(1)
        .var(1)
        .raw(&[0xff])
        .string("hi")
        .var(0)
        .done();
    f.case(unreadable(
        "root_name_not_utf8",
        root_not_utf8,
        "invalid_utf8",
    ));

    let mut truncated = insert(ME, 0, "hello");
    truncated.truncate(truncated.len() - 3);
    f.case(unreadable("truncated", truncated, "yrs_parse"));
    let mut trailing = insert(ME, 0, "hello");
    trailing.push(0);
    f.case(unreadable("trailing_bytes", trailing, "yrs_parse"));

    let gc = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .raw(&[GC])
        .var(3)
        .var(0)
        .done();
    f.case(unreadable("gc_struct", gc, "gc_struct"));
    for (name, content) in [
        ("json_content", 2u8),
        ("binary_content", 3),
        ("embed_content", 5),
        ("format_content", 6),
        ("type_content", 7),
        ("any_content", 8),
        ("doc_content", 9),
        ("move_content", 11),
    ] {
        let yrs = Lib0::default()
            .var(1)
            .run(1, ME, 0)
            .root_item(content)
            .var(1)
            .var(0)
            .done();
        f.case(unreadable(name, yrs, "content_kind"));
    }
    let other_root = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .raw(&[STRING])
        .var(1)
        .string("other")
        .string("hi")
        .var(0)
        .done();
    f.case(unreadable("parent_other_root", other_root, "parent"));
    let nested = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .raw(&[STRING])
        .var(0)
        .var(ME)
        .var(7)
        .string("hi")
        .var(0)
        .done();
    f.case(unreadable("parent_nested_type", nested, "parent"));
    let map_key = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .root_item(STRING | HAS_PARENT_SUB)
        .string("key")
        .string("hi")
        .var(0)
        .done();
    f.case(unreadable("map_key", map_key, "map_key"));
    let map_key_with_origin = Lib0::default()
        .var(1)
        .run(1, ME, 0)
        .raw(&[STRING | HAS_ORIGIN | HAS_PARENT_SUB])
        .var(0)
        .var(0)
        .string("hi")
        .var(0)
        .done();
    f.case(unreadable(
        "map_key_flag_with_origin",
        map_key_with_origin,
        "map_key",
    ));
}

/// Which client IDs an update and a snapshot may hold structs under.
fn client_checks(f: &mut Fixture) {
    f.case(update_case(
        "update_under_another_load",
        insert((1 << 29) + 2, 0, "hi"),
        Some("foreign_client"),
    ));
    f.case(update_case(
        "update_under_another_card",
        insert(OTHER, 0, "hi"),
        Some("foreign_client"),
    ));
    let skip_elsewhere = Lib0::default()
        .var(1)
        .run(1, OTHER, 0)
        .raw(&[SKIP])
        .var(2)
        .var(0)
        .done();
    f.case(update_case(
        "update_skip_under_another_card",
        skip_elsewhere,
        Some("foreign_client"),
    ));
    let own_skip = Lib0::default()
        .var(1)
        .run(2, ME, 0)
        .raw(&[SKIP])
        .var(2)
        .root_item(STRING)
        .string("hi")
        .var(0)
        .done();
    f.case(update_case("update_with_own_skip", own_skip, None));
    f.case(update_case(
        "update_deleting_others_text",
        deletes(OTHER, &[(0, 3)]),
        None,
    ));

    // An item can only name elements that existed when its writer made it:
    // under its own client ID, clocks below its own. One that names its own
    // clock or a later one integrates in one place as it arrives and in
    // another from a snapshot of the result, and a device booted from that
    // snapshot traps.
    let two_items = |origin: (u64, u64), right: Option<(u64, u64)>| {
        let mut info = STRING | HAS_ORIGIN;
        if right.is_some() {
            info |= HAS_RIGHT_ORIGIN;
        }
        let mut out = Lib0::default()
            .var(1)
            .run(2, ME, 0)
            .root_item(STRING)
            .string("ab")
            .raw(&[info])
            .var(origin.0)
            .var(origin.1);
        if let Some((client, clock)) = right {
            out = out.var(client).var(clock);
        }
        out.string("c").var(0).done()
    };
    f.case(update_case(
        "item_between_its_writers_earlier_elements",
        two_items((ME, 0), Some((ME, 1))),
        None,
    ));
    f.case(update_case(
        "item_after_its_own_clock",
        two_items((ME, 2), None),
        Some("forward_origin"),
    ));
    f.case(update_case(
        "item_before_a_later_clock_of_its_writer",
        two_items((ME, 0), Some((ME, 3))),
        Some("forward_origin"),
    ));
    let self_right = insert_between(ME, 0, Some((0, 0)), Some((ME, 1)), "ab");
    f.case(update_case(
        "item_whose_right_origin_lies_inside_itself",
        self_right,
        Some("forward_origin"),
    ));

    let snapshot = |name: &str, client: u64, card_count: u32, expect: Option<&str>| {
        let yrs = insert(client, 0, "hi");
        verdict_case(
            name,
            "text_snapshot",
            snapshot_payload(
                "",
                vec![meta("path", 4, Some("Notes/Plan.md"))],
                inline(yrs.clone()),
            ),
            None,
            (1, card_count),
            Some(&yrs),
            expect,
        )
    };
    f.case(snapshot("snapshot_under_client_0", 0, 1, None));
    f.case(snapshot("snapshot_under_an_indexed_card", OTHER, 2, None));
    f.case(snapshot(
        "snapshot_under_a_card_not_yet_indexed",
        OTHER,
        1,
        Some("snapshot_client"),
    ));
    f.case(snapshot(
        "snapshot_under_load_0",
        2 << 29,
        2,
        Some("snapshot_client"),
    ));
    f.case(snapshot(
        "snapshot_under_card_0",
        5,
        2,
        Some("snapshot_client"),
    ));
}

/// Client IDs that need more than 32 bits: an update by card 8 and one by
/// the largest card's last load, 2^53 − 1. A build with `small-client`
/// fails these.
fn wide_clients(f: &mut Fixture) {
    for (card, load, text) in [(8u32, 1u32, "wide"), (16_777_215, 536_870_911, "widest")] {
        let client = yrs_client_id(card, load).unwrap();
        let yrs = insert(client, 0, text);
        let mut case = verdict_case(
            &format!("update_by_client_{client}"),
            "text_update",
            update_payload("", load, vec![], inline(yrs.clone())),
            None,
            (card, card),
            Some(&yrs),
            None,
        );
        case["kind"] = json!("client_round_trip");
        case["client"] = json!(client);
        case["text"] = json!(text);
        f.case(case);
    }
}

/// The `meta` rules, the payload kind and the blob.
fn meta_checks(f: &mut Fixture) {
    let yrs = insert(ME, 0, "hi");
    let update = |name: &str, object_id: &str, entries: Vec<Meta>, expect: Option<&str>| {
        verdict_case(
            name,
            "text_update",
            update_payload(object_id, 1, entries, inline(yrs.clone())),
            None,
            (1, 2),
            None,
            expect,
        )
    };
    f.case(update("update_without_meta", "", vec![], None));
    f.case(update(
        "update_path_names_sequence_5",
        "",
        vec![meta("path", 5, Some("Notes/Plan.md"))],
        Some("meta_seq"),
    ));
    f.case(update(
        "update_key_neither_path_nor_property",
        "",
        vec![meta("title", 0, Some("\"Plan\""))],
        Some("meta_key"),
    ));
    f.case(update(
        "update_property_name_empty",
        "note_01",
        vec![meta("p:", 0, Some("1"))],
        Some("meta_key"),
    ));
    let long_name = format!("p:{}", "a".repeat(257));
    f.case(update(
        "update_property_name_257_bytes",
        "note_01",
        vec![meta(&long_name, 0, Some("1"))],
        Some("meta_key"),
    ));
    let name_256 = format!("p:{}", "a".repeat(256));
    f.case(update(
        "update_property_name_256_bytes",
        "note_01",
        vec![meta(&name_256, 0, Some("1"))],
        None,
    ));
    f.case(update(
        "update_property_name_with_control",
        "note_01",
        vec![meta("p:a\u{7}", 0, Some("1"))],
        Some("meta_key"),
    ));
    f.case(update(
        "update_property_on_unmanaged_note",
        "",
        vec![meta("p:status", 0, Some("\"done\""))],
        Some("meta_unmanaged"),
    ));
    f.case(update(
        "update_property_canonical",
        "note_01",
        vec![meta(
            "p:tags",
            0,
            Some("{\"a\":[1,2.5,\"x\"],\"b\":null,\"é\":true}"),
        )],
        None,
    ));
    f.case(update(
        "update_property_removed",
        "note_01",
        vec![meta("p:status", 0, None)],
        None,
    ));
    for (name, value) in [
        ("update_property_whitespace", "{\"a\": 1}"),
        ("update_property_keys_unsorted", "{\"b\":1,\"a\":2}"),
        ("update_property_number_not_shortest", "1.50"),
        ("update_property_escaped_letter", "\"\\u0041\""),
        ("update_property_not_json", "done"),
        ("update_property_not_utf8", "\"\u{fffd}\""),
    ] {
        let mut entry = meta("p:status", 0, Some(value));
        if name == "update_property_not_utf8" {
            entry.value = Some(Bytes(vec![b'"', 0xff, b'"']));
        }
        f.case(update(name, "note_01", vec![entry], Some("meta_value")));
    }
    f.case(update(
        "update_id_matches_object_id",
        "note_01",
        vec![meta("p:id", 0, Some("\"note_01\""))],
        None,
    ));
    f.case(update(
        "update_id_differs_from_object_id",
        "note_01",
        vec![meta("p:id", 0, Some("\"note_02\""))],
        Some("meta_id"),
    ));
    f.case(update(
        "update_id_removed",
        "note_01",
        vec![meta("p:id", 0, None)],
        Some("meta_id"),
    ));
    f.case(update(
        "update_type_removed",
        "note_01",
        vec![meta("p:type", 0, None)],
        Some("meta_type"),
    ));
    f.case(update(
        "update_path_removed",
        "",
        vec![meta("path", 0, None)],
        Some("meta_path"),
    ));
    for (name, path) in [
        ("update_path_in_git", ".git/config"),
        ("update_path_with_dot_dot", "Notes/../Plan.md"),
        ("update_path_absolute", "/Notes/Plan.md"),
        ("update_path_reserved_name", "Notes/CON.md"),
    ] {
        f.case(update(
            name,
            "",
            vec![meta("path", 0, Some(path))],
            Some("meta_path"),
        ));
    }

    let snapshot_yrs = insert(0, 0, "hi");
    let snapshot = |name: &str, object_id: &str, entries: Vec<Meta>, expect: Option<&str>| {
        verdict_case(
            name,
            "text_snapshot",
            snapshot_payload(object_id, entries, inline(snapshot_yrs.clone())),
            None,
            (1, 2),
            None,
            expect,
        )
    };
    f.case(snapshot(
        "managed_snapshot",
        "note_01",
        vec![
            meta("p:id", 3, Some("\"note_01\"")),
            meta("p:type", 3, Some("\"note\"")),
            meta("path", 3, Some("Notes/Plan.md")),
        ],
        None,
    ));
    f.case(snapshot(
        "snapshot_names_effective_sequences",
        "",
        vec![meta("path", 9, Some("Notes/Plan.md"))],
        None,
    ));
    f.case(snapshot(
        "snapshot_without_path",
        "",
        vec![],
        Some("meta_path"),
    ));
    f.case(snapshot(
        "managed_snapshot_without_id",
        "note_01",
        vec![
            meta("p:type", 3, Some("\"note\"")),
            meta("path", 3, Some("Notes/Plan.md")),
        ],
        Some("meta_id"),
    ));
    f.case(snapshot(
        "managed_snapshot_without_type",
        "note_01",
        vec![
            meta("p:id", 3, Some("\"note_01\"")),
            meta("path", 3, Some("Notes/Plan.md")),
        ],
        Some("meta_type"),
    ));

    let file_payload = Payload {
        rid: id("note rid"),
        kind: PayloadKind::File {
            object_id: Text::new("").unwrap(),
            path: sync_protocol::records::SyncPath::new("Notes/Plan.md").unwrap(),
            revision: hash(b"hi"),
            content: inline(b"hi".to_vec()),
            origin: None,
        },
    }
    .to_bytes();
    f.case(verdict_case(
        "file_payload",
        "file",
        file_payload,
        None,
        (1, 2),
        None,
        Some("payload"),
    ));
    f.case(verdict_case(
        "payload_not_decodable",
        "text_update",
        vec![0x01, 0x02, 0x03],
        None,
        (1, 2),
        None,
        Some("payload"),
    ));

    let file = insert(ME, 0, "from a blob");
    let path = || vec![meta("path", 0, Some("Notes/Plan.md"))];
    f.case(verdict_case(
        "update_in_a_blob",
        "text_update",
        update_payload("", 1, path(), blob_ref(&file, file.len() as u64)),
        Some(&file),
        (1, 2),
        Some(&file),
        None,
    ));
    f.case(verdict_case(
        "blob_missing",
        "text_update",
        update_payload("", 1, path(), blob_ref(&file, file.len() as u64)),
        None,
        (1, 2),
        None,
        Some("blob"),
    ));
    f.case(verdict_case(
        "blob_length_differs",
        "text_update",
        update_payload("", 1, path(), blob_ref(&file, file.len() as u64 + 1)),
        Some(&file),
        (1, 2),
        None,
        Some("blob"),
    ));
    f.case(verdict_case(
        "inline_content_with_a_blob",
        "text_update",
        update_payload("", 1, path(), inline(file.clone())),
        Some(&file),
        (1, 2),
        None,
        Some("blob"),
    ));
}

/// A split inside a surrogate pair turns each half into U+FFFD, alike on a
/// device that integrates records as they arrive and on one that boots
/// from a snapshot.
fn surrogates(f: &mut Fixture) {
    // "a😀b" by client 0: 'a' at clock 0, the pair at 1 and 2, 'b' at 3.
    let base = insert(0, 0, "a\u{1F600}b");
    let split = "a\u{FFFD}X\u{FFFD}b";
    for (name, yrs, text) in [
        // With no right origin, the item goes after every item that shares
        // its origin and has a lower client ID, and the split leaves the low
        // half's part with the high half as its origin.
        (
            "insert_after_the_high_half",
            insert_between(ME, 0, Some((0, 1)), None, "X"),
            "a\u{FFFD}\u{FFFD}bX",
        ),
        (
            "insert_before_the_low_half",
            insert_between(ME, 0, None, Some((0, 2)), "X"),
            split,
        ),
        (
            "insert_between_the_halves",
            insert_between(ME, 0, Some((0, 1)), Some((0, 2)), "X"),
            split,
        ),
        (
            "delete_from_the_low_half",
            deletes(0, &[(2, 2)]),
            "a\u{FFFD}",
        ),
        (
            "delete_to_the_high_half",
            deletes(0, &[(1, 1)]),
            "a\u{FFFD}b",
        ),
    ] {
        f.case(json!({
            "kind": "surrogate",
            "name": name,
            "base": to_hex(&base),
            "card_index": 1,
            "card_count": 1,
            "payload": to_hex(&update_payload("", 1, vec![], inline(yrs.clone()))),
            "yrs": to_hex(&yrs),
            "text": text,
        }));
    }
}

/// Writer allowances: a record may overdraw its writer's allowance, the
/// writer's next record waits for a refill, and other writers' records keep
/// merging.
fn allowances(f: &mut Fixture) {
    // "x" by client 0.
    let base = insert(0, 0, "x");
    let record = |id: u64, card: u32, yrs: Vec<u8>| {
        json!({
            "id": id,
            "card_index": card,
            "card_count": 2,
            "payload": to_hex(&update_payload("", 1, vec![], inline(yrs))),
        })
    };
    let a = insert_between(ME, 0, Some((0, 0)), None, "A");
    let b = insert_between(ME, 1, Some((ME, 0)), None, "B");
    let c = insert_between(OTHER, 0, Some((0, 0)), None, "C");
    f.case(json!({
        "kind": "allowance",
        "name": "overdraw_then_defer",
        "base": to_hex(&base),
        "balances": {"1": 1, "2": 2_000_000_000},
        "steps": [
            {"receive": record(1, 1, a), "expect": "merged"},
            {"receive": record(2, 1, b), "expect": "deferred"},
            {"receive": record(3, 2, c), "expect": "merged"},
            {"after_seconds": 59, "expect_merged": []},
            {"after_seconds": 60, "expect_merged": [2]},
        ],
        "text": "xABC",
    }));
    let d = insert_between(ME, 0, Some((0, 0)), None, "D");
    let e = insert_between(ME, 1, Some((ME, 0)), None, "E");
    f.case(json!({
        "kind": "allowance",
        "name": "a_debt_waits_for_enough_refills",
        "base": to_hex(&base),
        "balances": {"1": -3_000_000_000_i64},
        "steps": [
            {"receive": record(1, 1, d), "expect": "deferred"},
            {"receive": record(2, 1, e), "expect": "deferred"},
            {"after_seconds": 60, "expect_merged": []},
            {"after_seconds": 120, "expect_merged": [1, 2]},
        ],
        "text": "xDE",
    }));
}

/// A trap while the sandbox integrates a record that passed the verdict:
/// the pinned module traps on no known record, so the runner makes the
/// instance trap on the record a step marks, as a bug in the guest would.
/// The trap stops the lineage on the device, which rebuilds the document
/// from the journal and merges nothing more of the lineage.
fn traps(f: &mut Fixture) {
    let base = insert(0, 0, "x");
    let record = |id: u64, card: u32, yrs: Vec<u8>| {
        json!({
            "id": id,
            "card_index": card,
            "card_count": 2,
            "payload": to_hex(&update_payload("", 1, vec![], inline(yrs))),
        })
    };
    let first = insert_between(OTHER, 0, Some((0, 0)), None, "a");
    let second = insert_between(OTHER, 1, Some((OTHER, 0)), None, "b");
    let later = insert_between(ME, 0, Some((0, 0)), None, "B");
    f.case(json!({
        "kind": "trap",
        "name": "a_trap_during_integration_stops_the_lineage",
        "base": to_hex(&base),
        "steps": [
            {"receive": record(1, 2, first), "trap": false, "expect": "merged"},
            {"receive": record(2, 2, second), "trap": true, "expect": "stopped"},
            {"receive": record(3, 1, later), "trap": false, "expect": "skipped"},
        ],
        "text": "xa",
    }));
}

/// An update that depends on an unreadable record stays pending in yrs, and
/// the device lists the lineage for review as text, until another record
/// supplies what it depends on: here the signer's rewrite of the record with
/// a valid path, as after an epoch replacement (§9).
fn pending(f: &mut Fixture) {
    let base = insert(0, 0, "x");
    let record = |id: u64, path: &str, yrs: Vec<u8>| {
        json!({
            "id": id,
            "card_index": 2,
            "card_count": 2,
            "payload": to_hex(&update_payload(
                "",
                1,
                vec![meta("path", 0, Some(path))],
                inline(yrs),
            )),
        })
    };
    let abc = insert_between(OTHER, 0, Some((0, 0)), None, "abc");
    let d = insert_between(OTHER, 3, Some((OTHER, 2)), None, "d");
    f.case(json!({
        "kind": "pending",
        "name": "an_update_waits_for_the_rewrite_of_an_unreadable_one",
        "base": to_hex(&base),
        "steps": [
            // Unreadable for its path: neither its body nor its entries
            // reach the note.
            {"receive": record(1, "../Plan.md", abc.clone()), "text": "x", "status": "merging"},
            // Depends on the unreadable record's clocks: yrs holds it.
            {"receive": record(2, "Notes/Plan.md", d), "text": "x", "status": "pending"},
            // The rewrite supplies them, and the held update merges.
            {"receive": record(3, "Notes/Plan.md", abc), "text": "xabcd", "status": "merging"},
        ],
    }));
}

/// A trap in the verdict: whatever makes the guest trap, a panic in its yrs
/// included, the record is unreadable and the next valid record still
/// applies. The pinned module traps on no known record, so the case stands
/// in a module whose `verdict` export traps for the records it judges.
fn verdict_traps(f: &mut Fixture) {
    let base = insert(0, 0, "x");
    let record = |id: u64, yrs: Vec<u8>| {
        json!({
            "id": id,
            "card_index": 1,
            "card_count": 1,
            "payload": to_hex(&update_payload("", 1, vec![], inline(yrs))),
        })
    };
    let a = insert_between(ME, 0, Some((0, 0)), None, "A");
    let b = insert_between(ME, 1, Some((ME, 0)), None, "B");
    f.case(json!({
        "kind": "verdict_trap",
        "name": "a_trapping_verdict_makes_one_record_unreadable",
        "module_wat": concat!(
            "(module",
            " (memory (export \"memory\") 1)",
            " (func (export \"alloc\") (param i32) (result i32) i32.const 0)",
            " (func (export \"verdict\")",
            " (param i32 i32 i32 i32 i32 i32 i32) (result i32) unreachable))",
        ),
        "base": to_hex(&base),
        "steps": [
            {"judge": "stand_in", "receive": record(1, a.clone()), "expect": "trap"},
            {"judge": "pinned", "receive": record(2, a), "expect": "merged"},
            {"judge": "pinned", "receive": record(3, b), "expect": "merged"},
        ],
        "text": "xAB",
    }));
}

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-collaboration.json",
        "Text mode (sync-v2.md §22): the text lineage ID derived from the rid, the base operation's digest and the first document's bytes; yrs client IDs from a card index and a load number; and the collaboration sandbox: the guest module's hash, the verdict budget and the Wasmtime operator costs it assumes, the raw lib0 checks and the verdict on each record's yrs bytes and meta entries, surrogate pairs split into U+FFFD, client IDs past 32 bits, and writer allowances.",
    );
    let rid = id("note rid");
    let base = hash(b"base operation");
    let document = vec![
        0x01, 0x01, 0x00, 0x00, 0x04, 0x01, 0x07, 0x63, 0x6f, 0x6e, 0x74, 0x65, 0x6e, 0x74,
    ];
    let mut input = Tag::TextLineage.bytes();
    input.extend_from_slice(&rid.0);
    input.extend_from_slice(&base.0);
    input.extend_from_slice(&document);
    f.case(json!({
        "kind": "text_lineage",
        "name": "lineage_of_a_first_document",
        "rid": rid.to_hex(),
        "base": base.to_hex(),
        "document": to_hex(&document),
        "input": to_hex(&input),
        "expect": text_lineage_id(&rid, &base, &document).to_hex(),
    }));
    let mut other = document.clone();
    other.push(0);
    f.case(json!({
        "kind": "text_lineage",
        "name": "another_first_document_another_lineage",
        "rid": rid.to_hex(),
        "base": base.to_hex(),
        "document": to_hex(&other),
        "input": null,
        "expect": text_lineage_id(&rid, &base, &other).to_hex(),
    }));
    for (index, load, expect) in [
        (1u32, 1u32, Some(536_870_913u64)),
        (2, 7, Some(2 * 536_870_912 + 7)),
        (16_777_215, 536_870_911, Some((1u64 << 53) - 1)),
        (8, 1, Some(4_294_967_297)),
        (0, 1, None),
        (1, 0, None),
        (1, 536_870_912, None),
        (16_777_216, 1, None),
    ] {
        f.case(json!({
            "kind": "client_id",
            "name": format!("card_{index}_load_{load}"),
            "card_index": index,
            "load": load,
            "expect": expect,
        }));
        assert_eq!(yrs_client_id(index, load), expect);
    }

    let module = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../..")
            .join(MODULE),
    )
    .expect("the guest module is built: crates/collab-sandbox-guest/build.sh");
    let variable: serde_json::Map<String, Value> = VARIABLE_COSTS
        .iter()
        .map(|(name, cost)| ((*name).to_owned(), json!(cost)))
        .collect();
    f.set(
        "sandbox",
        json!({
            "module": {
                "path": MODULE,
                "blake3": blake3::hash(&module).to_hex().to_string(),
            },
            "verdict": {
                "base_fuel": 1_000_000,
                "fuel_per_byte": 1_000,
                "counted_bytes": "the decrypted payload without its padding, and the blob's file",
                "memory_bytes": 512u64 << 20,
                "epoch_interruption": false,
            },
            "operator_costs": {
                "default": 1,
                "free": FREE_OPERATORS,
                "variable": variable,
            },
            "integration": {
                "allowance_fuel": 2_000_000_000u64,
                "refill_seconds": 60,
                "record_fuel_limit": 10_000_000_000u64,
                "document_bytes": 32u64 << 20,
                "text_bytes": 8u64 << 20,
            },
        }),
    );

    raw_checks(&mut f);
    client_checks(&mut f);
    wide_clients(&mut f);
    meta_checks(&mut f);
    surrogates(&mut f);
    allowances(&mut f);
    traps(&mut f);
    pending(&mut f);
    verdict_traps(&mut f);
    f
}
