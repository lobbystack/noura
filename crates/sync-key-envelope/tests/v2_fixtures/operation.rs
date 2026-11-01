//! `sync-v2-operation.json`: operation records and payloads (§13).

use serde_json::{Value, json};
use sync_protocol::codec::{Bytes, Encode, Sorted, Text};
use sync_protocol::crypto::{context, derive_key, hash};
use sync_protocol::records::{
    BlobRef, Content, Cover, DeleteReason, MAX_LOAD, Meta, Operation, Origin, Payload, PayloadKind,
    Signed, SyncPath,
};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey, to_hex};

use super::common::{Actor, Fixture, Shape, id, nonce, secret};
use super::world::{World, world};

fn meta(key: &str, seq: u64, value: Option<&str>) -> Meta {
    Meta {
        key: Text::new(key).unwrap(),
        seq,
        value: value.map(|v| Bytes(v.as_bytes().to_vec())),
    }
}

pub fn payloads(w: &World) -> Vec<(&'static str, Payload)> {
    let lineage = id("text lineage");
    vec![
        ("payload_file", w.payload.clone()),
        (
            "payload_file_blob_moved_in",
            Payload {
                rid: id("moved rid"),
                kind: PayloadKind::File {
                    object_id: Text::default(),
                    path: SyncPath::new("Photos/garden.jpg").unwrap(),
                    revision: hash(b"jpeg bytes"),
                    content: Content::Blob(BlobRef {
                        blob: id("blob"),
                        ciphertext_length: 2_163_269,
                        ciphertext_digest: Digest([0x0d; 32]),
                        blob_key: secret("blob key"),
                        plaintext_length: 2_097_152,
                    }),
                    origin: Some(Origin {
                        domain: id("other domain"),
                        rid: id("rid in the other domain"),
                    }),
                },
            },
        ),
        (
            "payload_delete_moved",
            Payload {
                rid: w.rid,
                kind: PayloadKind::Delete {
                    object_id: Text::new("note_01").unwrap(),
                    revision: hash(b"# Plan\n"),
                    reason: DeleteReason::Moved,
                    target: Some(Origin {
                        domain: id("other domain"),
                        rid: id("rid in the other domain"),
                    }),
                },
            },
        ),
        (
            "payload_text_snapshot",
            Payload {
                rid: w.rid,
                kind: PayloadKind::TextSnapshot {
                    object_id: Text::new("note_01").unwrap(),
                    revision: hash(b"---\nid: note_01\ntype: note\n---\n# Plan\n"),
                    text_lineage: lineage,
                    meta: Sorted::new(vec![
                        meta("p:id", 6, Some("\"note_01\"")),
                        meta("p:type", 6, Some("\"note\"")),
                        meta("path", 6, Some("Notes/Plan.md")),
                    ])
                    .unwrap(),
                    content: Content::Inline {
                        bytes: Bytes(vec![0x01, 0x01, 0x00, 0x00]),
                    },
                    origin: None,
                },
            },
        ),
        (
            "payload_text_update",
            Payload {
                rid: w.rid,
                kind: PayloadKind::TextUpdate {
                    object_id: Text::new("note_01").unwrap(),
                    text_lineage: lineage,
                    load: 1,
                    meta: Sorted::new(vec![
                        meta("p:status", 0, None),
                        meta("path", 0, Some("Notes/Plans.md")),
                    ])
                    .unwrap(),
                    content: Content::Inline {
                        bytes: Bytes(vec![0x01, 0x01, 0x80, 0x80, 0x80, 0x20, 0x00]),
                    },
                },
            },
        ),
    ]
}

fn sealed(w: &World, mut body: Operation, payload: &Payload, label: &str) -> Signed<Operation> {
    body.seal_payload(payload, &w.ws_key2, &w.ws_epoch2.digest(), nonce(label))
        .unwrap();
    Signed::sign(body, w.alice_laptop.keys.signing())
}

pub fn fixture() -> Fixture {
    let w = world();
    let mut f = Fixture::new(
        "sync-v2-operation.json",
        "Operations (sync-v2.md §13): the signed record with its generation, delete flag and checkpoint reference, its payload of each kind under the operation subkey with the epoch record's digest in its associated data, path rules, the payload size limit, meta entries, and payloads that don't match their class.",
    );
    f.actor(&w.alice_laptop);
    f.accept_signed("operation", "operation", &w.op, &w.alice_laptop);
    for (name, payload) in payloads(&w) {
        f.accept(name, "payload", &payload, None, Shape::Tagged);
    }
    let mut load_zero = payloads(&w).pop().unwrap().1;
    if let PayloadKind::TextUpdate { load, .. } = &mut load_zero.kind {
        *load = 0;
    }
    f.reject(
        "text_update_load_0",
        "payload",
        &load_zero.to_bytes(),
        json!({"decode": "out_of_range"}),
    );
    if let PayloadKind::TextUpdate { load, .. } = &mut load_zero.kind {
        *load = MAX_LOAD + 1;
    }
    f.reject(
        "text_update_load_2_29",
        "payload",
        &load_zero.to_bytes(),
        json!({"decode": "out_of_range"}),
    );

    // A delete with its flag, a skipped counter, a checkpoint reference and
    // a later generation.
    let delete_payload = payloads(&w)[2].1.clone();
    let mut delete = w.op.body.clone();
    delete.counter = 1_025;
    delete.previous = w.op.digest();
    delete.delete = true;
    delete.generation = 1_760_000_100;
    delete.checkpoint_seq = 6;
    delete.checkpoint = w.checkpoint.checkpoint.digest();
    delete.supersedes = Sorted::new(vec![Cover {
        device: w.alice_laptop.device(),
        counter: 1,
    }])
    .unwrap();
    let delete = sealed(&w, delete, &delete_payload, "delete");
    f.accept_signed(
        "operation_delete_after_skipped_counters",
        "operation",
        &delete,
        &w.alice_laptop,
    );
    let mut zero_counter = w.op.to_bytes();
    let at = 16 + 16 * 3;
    zero_counter[at..at + 8].copy_from_slice(&0u64.to_be_bytes());
    f.reject(
        "operation_counter_0",
        "operation",
        &zero_counter,
        json!({"decode": "out_of_range"}),
    );
    let mut flag_2 = w.op.to_bytes();
    let flag_at = 16 + 16 * 3 + 8 + 32 + 8 + 8 + 32 + 32 + 8 + 8 + 32 + 16;
    flag_2[flag_at] = 2;
    f.reject(
        "operation_flags_2",
        "operation",
        &flag_2,
        json!({"decode": "unknown_value"}),
    );

    // Generation and policy revision along a chain.
    let mut regressed = delete.body.clone();
    regressed.generation = 1;
    f.case(json!({
        "kind": "monotonic",
        "name": "generation_decreases_along_a_chain",
        "previous": to_hex(&delete.to_bytes()),
        "next": to_hex(&Signed::sign(regressed, w.alice_laptop.keys.signing()).to_bytes()),
        "expect": "sync_policy_regressed",
    }));
    f.case(json!({
        "kind": "monotonic",
        "name": "generation_rises",
        "previous": to_hex(&w.op.to_bytes()),
        "next": to_hex(&delete.to_bytes()),
        "expect": "ok",
    }));

    // Keys and associated data.
    f.case(json!({
        "kind": "derive_key",
        "name": "operation_key",
        "context": context::OPERATION_KEY,
        "input": to_hex(w.ws_key2.expose()),
        "expect": to_hex(derive_key(context::OPERATION_KEY, w.ws_key2.expose()).expose()),
    }));
    f.case(json!({
        "kind": "operation_aad",
        "name": "prefix_and_epoch_record_digest",
        "operation": to_hex(&w.op.to_bytes()),
        "epoch_record": w.ws_epoch2.digest().to_hex(),
        "expect": to_hex(&w.op.body.aad(&w.ws_epoch2.digest())),
    }));
    let open = |name: &str,
                op: &Signed<Operation>,
                key: &SecretKey,
                epoch_record: Digest,
                class: &str,
                expect: Value| {
        json!({
            "kind": "payload_open",
            "name": name,
            "operation": to_hex(&op.to_bytes()),
            "epoch_key": to_hex(key.expose()),
            "epoch_record": epoch_record.to_hex(),
            "class": class,
            "expect": expect,
        })
    };
    f.case(open(
        "open_a_file",
        &w.op,
        &w.ws_key2,
        w.ws_epoch2.digest(),
        "state",
        json!(to_hex(&w.payload.to_bytes())),
    ));
    f.case(open(
        "another_epoch_record_fails_to_decrypt",
        &w.op,
        &w.ws_key2,
        w.ws_epoch1.digest(),
        "state",
        json!("decrypt"),
    ));
    f.case(open(
        "another_epoch_key_fails_to_decrypt",
        &w.op,
        &w.ws_key1,
        w.ws_epoch2.digest(),
        "state",
        json!("decrypt"),
    ));
    f.case(open(
        "a_file_that_covers_nothing_on_an_existing_rid",
        &w.op,
        &w.ws_key2,
        w.ws_epoch2.digest(),
        "patch",
        json!("class_mismatch"),
    ));
    let update = payloads(&w)[4].1.clone();
    let mut new_rid_update = w.op.body.clone();
    new_rid_update.rid = update.rid;
    let new_rid_update = sealed(&w, new_rid_update, &update, "update on a new rid");
    f.case(open(
        "a_text_update_on_a_new_rid",
        &new_rid_update,
        &w.ws_key2,
        w.ws_epoch2.digest(),
        "state",
        json!("class_mismatch"),
    ));
    let mut lone_delete = w.op.body.clone();
    lone_delete.delete = true;
    let lone_delete = sealed(&w, lone_delete, &delete_payload, "lone delete");
    f.case(open(
        "a_delete_that_covers_nothing",
        &lone_delete,
        &w.ws_key2,
        w.ws_epoch2.digest(),
        "patch",
        json!("class_mismatch"),
    ));
    let mut flag_mismatch = w.op.body.clone();
    flag_mismatch.delete = true;
    let flag_mismatch = sealed(&w, flag_mismatch, &w.payload, "flag mismatch");
    f.case(open(
        "a_delete_flag_that_doesnt_match_the_payload",
        &flag_mismatch,
        &w.ws_key2,
        w.ws_epoch2.digest(),
        "state",
        json!("delete_flag_mismatch"),
    ));
    let mut other_rid = w.op.body.clone();
    other_rid.rid = id("another rid");
    let other_rid = sealed(&w, other_rid, &w.payload, "other rid");
    f.case(open(
        "a_payload_for_another_rid",
        &other_rid,
        &w.ws_key2,
        w.ws_epoch2.digest(),
        "state",
        json!("rid_mismatch"),
    ));

    // The payload limit.
    for (n, expect) in [
        (262_144usize, json!(270_352)),
        (262_145, json!("too_large")),
    ] {
        f.case(json!({
            "kind": "payload_limit",
            "name": format!("encoded_payload_of_{n}_bytes"),
            "encoded_length": n,
            "expect": expect,
        }));
    }

    // Path rules.
    for (path, valid) in [
        ("Notes/Plan.md", true),
        ("a", true),
        ("Notes/Été/Plan.md", true),
        ("Notes/.hidden.md", true),
        (".noura-domain", true),
        ("Projects/.noura-domain", true),
        ("Notes/conference.md", true),
        ("/Notes/Plan.md", false),
        ("Notes//Plan.md", false),
        ("Notes/./Plan.md", false),
        ("Notes/../Plan.md", false),
        ("Notes/Plan.md/", false),
        ("", false),
        ("Notes/Pl\u{0007}an.md", false),
        ("Notes/Plan\u{0085}.md", false),
        ("Notes/CON", false),
        ("Notes/con.txt", false),
        ("Notes/Com1.md", false),
        ("Notes/LPT\u{00b9}", false),
        ("Notes/CONIN$", false),
        ("Notes/a<b.md", false),
        ("Notes/a:b.md", false),
        ("Notes\\Plan.md", false),
        ("Notes/a|b.md", false),
        ("Notes/a?b.md", false),
        ("Notes/a*b.md", false),
        ("Notes/a\"b.md", false),
        ("Notes/Plan.", false),
        ("Notes/Plan ", false),
        (".noura/sync/state", false),
        (".NOURA", false),
        (".git/config", false),
        (".Git/config", false),
        ("Notes/.git/config", true),
    ] {
        f.case(
            json!({"kind": "path", "name": path, "path": to_hex(path.as_bytes()), "expect": valid}),
        );
    }
    f.case(
        json!({"kind": "path", "name": "invalid_utf8", "path": "4e6f7465732fff", "expect": false}),
    );
    f.case(json!({"kind": "path", "name": "4096_bytes", "path": to_hex("a".repeat(4096).as_bytes()), "expect": true}));
    f.case(json!({"kind": "path", "name": "4097_bytes", "path": to_hex("a".repeat(4097).as_bytes()), "expect": false}));
    let _ = (Id::ZERO, Nonce::ZERO, Actor::new("x", "x"));
    f
}
