//! `sync-v2-live.json`: live sets and supersession (§13).

use serde_json::{Value, json};
use sync_protocol::crypto::hash_parts;
use sync_protocol::types::Id;

use super::common::{Fixture, id};

fn device(label: &str) -> Id {
    id(label)
}

fn op(
    rid: &str,
    device_label: &str,
    counter: u64,
    seq: u64,
    covers: &[(&str, u64)],
    delete: bool,
) -> Value {
    json!({
        "rid": id(rid).to_hex(),
        "device": device(device_label).to_hex(),
        "counter": counter,
        "seq": seq,
        "digest": hash_parts(&[rid.as_bytes(), device_label.as_bytes(), &counter.to_be_bytes()]).to_hex(),
        "delete": delete,
        "covers": covers.iter().map(|(d, c)| json!({"device": device(d).to_hex(), "counter": c})).collect::<Vec<_>>(),
        "weight": 300,
    })
}

fn live(entries: &[(&str, u64)]) -> Value {
    let mut keys: Vec<(Id, u64)> = entries.iter().map(|(d, c)| (device(d), *c)).collect();
    keys.sort();
    json!(
        keys.iter()
            .map(|(d, c)| json!({"device": d.to_hex(), "counter": c}))
            .collect::<Vec<_>>()
    )
}

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-live.json",
        "Live sets (sync-v2.md §13, §17): each case applies operations in log order and lists the class the server gives each and the live set of each object afterward, computed alike by server and client. `floors` applies a checkpoint's rising floor at its covered_seq; `covers_for` asks which covers an edit writes.",
    );
    let a = "live device a";
    let b = "live device b";
    let c = "live device c";
    let note = "live note";

    f.case(json!({
        "kind": "live_set",
        "name": "classes_and_covers_of_covered_operations",
        "ops": [op(note, a, 1, 1, &[], false), op(note, b, 1, 2, &[], false), op(note, a, 2, 3, &[(a, 1), (b, 1)], false)],
        "classes": ["state", "patch", "state"],
        "floors": [],
        "expect": {id(note).to_hex(): live(&[(a, 2)])},
    }));
    f.case(json!({
        "kind": "live_set",
        "name": "a_cover_supersedes_its_devices_earlier_operations",
        "ops": [op(note, a, 1, 1, &[], false), op(note, a, 2, 2, &[], false), op(note, b, 1, 3, &[(a, 2)], false)],
        "classes": ["state", "patch", "state"],
        "floors": [],
        "expect": {id(note).to_hex(): live(&[(b, 1)])},
    }));
    f.case(json!({
        "kind": "live_set",
        "name": "conflict_and_an_edit_that_covers_only_its_branch",
        "ops": [
            op(note, a, 1, 1, &[], false),
            op(note, a, 2, 2, &[(a, 1)], false),
            op(note, b, 1, 3, &[(a, 1)], false),
            op(note, c, 1, 4, &[(a, 2)], false),
        ],
        "classes": ["state", "state", "state", "state"],
        "floors": [],
        "expect": {id(note).to_hex(): live(&[(b, 1), (c, 1)])},
    }));
    f.case(json!({
        "kind": "live_set",
        "name": "a_delete_that_covers_nothing_counts_as_a_patch",
        "ops": [op(note, a, 1, 1, &[], false), op(note, b, 1, 2, &[], true)],
        "classes": ["state", "patch"],
        "floors": [],
        "expect": {id(note).to_hex(): live(&[(a, 1), (b, 1)])},
    }));
    f.case(json!({
        "kind": "live_set",
        "name": "an_edit_against_a_delete_keeps_both",
        "ops": [op(note, a, 1, 1, &[], false), op(note, a, 2, 2, &[(a, 1)], true), op(note, b, 1, 3, &[(a, 1)], false)],
        "classes": ["state", "state", "state"],
        "floors": [{"previous_floor": 0, "floor": 5, "covered_seq": 5, "dropped": []}],
        "expect": {id(note).to_hex(): live(&[(a, 2), (b, 1)])},
    }));
    f.case(json!({
        "kind": "live_set",
        "name": "a_lone_delete_that_a_rising_floor_passes_leaves_for_good",
        "ops": [
            op(note, a, 1, 1, &[], false),
            op(note, a, 2, 2, &[(a, 1)], true),
            op(note, b, 1, 11, &[], false),
        ],
        "classes": ["state", "state", "patch"],
        "floors": [{"after_op": 2, "previous_floor": 0, "floor": 5, "covered_seq": 10, "dropped": [{"rid": id(note).to_hex(), "device": device(a).to_hex(), "counter": 2}]}],
        "expect": {id(note).to_hex(): live(&[(b, 1)])},
    }));
    f.case(json!({
        "kind": "live_set",
        "name": "a_lone_delete_above_the_floor_stays",
        "ops": [op(note, a, 1, 1, &[], false), op(note, a, 2, 8, &[(a, 1)], true)],
        "classes": ["state", "state"],
        "floors": [{"previous_floor": 0, "floor": 5, "covered_seq": 10, "dropped": []}],
        "expect": {id(note).to_hex(): live(&[(a, 2)])},
    }));
    f.case(json!({
        "kind": "live_check",
        "name": "a_state_operation_that_leaves_its_own_earlier_state_live",
        "ops": [op(note, a, 1, 1, &[], false), op(note, b, 1, 2, &[], false), op(note, b, 2, 3, &[(b, 1)], false)],
        "next": op(note, a, 2, 4, &[(b, 2)], false),
        "expect": "supersedes_incomplete",
    }));
    f.case(json!({
        "kind": "live_check",
        "name": "a_cover_naming_no_operation",
        "ops": [op(note, a, 1, 1, &[], false)],
        "next": op(note, b, 1, 2, &[(a, 7)], false),
        "expect": "supersedes_unknown",
    }));
    f.case(json!({
        "kind": "live_check",
        "name": "a_cover_of_every_live_operation",
        "ops": [op(note, a, 1, 1, &[], false), op(note, b, 1, 2, &[], false)],
        "next": op(note, c, 1, 3, &[(a, 1), (b, 1)], false),
        "expect": "state",
    }));
    f.case(json!({
        "kind": "covers_for",
        "name": "an_edit_on_one_branch_of_a_conflict",
        "ops": [op(note, a, 1, 1, &[], false), op(note, a, 2, 2, &[(a, 1)], false), op(note, b, 1, 3, &[(a, 1)], false)],
        "chosen": [{"device": device(a).to_hex(), "counter": 2}],
        "own_device": device(c).to_hex(),
        "expect": [{"device": device(a).to_hex(), "counter": 2}],
    }));
    let mut covers = [(device(b), 1u64), (device(c), 1u64)];
    covers.sort();
    let covers: Vec<Value> = covers
        .iter()
        .map(|(d, n)| json!({"device": d.to_hex(), "counter": n}))
        .collect();
    f.case(json!({
        "kind": "covers_for",
        "name": "one_edit_covers_two_identical_branches",
        "ops": [op(note, a, 1, 1, &[], false), op(note, b, 1, 2, &[(a, 1)], false), op(note, c, 1, 3, &[(a, 1)], false)],
        "chosen": [{"device": device(b).to_hex(), "counter": 1}, {"device": device(c).to_hex(), "counter": 1}],
        "own_device": device(a).to_hex(),
        "expect": covers,
    }));
    f
}
