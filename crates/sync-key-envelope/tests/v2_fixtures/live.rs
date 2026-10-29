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
        "Live sets (sync-v2.md §13, §17): each case applies operations in log order and lists the class the server gives each and the live set of each object afterward, computed alike by server and client. `floors` applies a checkpoint's rising floor at its covered_seq; `covers_for` asks which covers an edit writes. `journal_less_match` matches local files to objects when a folder syncs without its journal (§15, §18), against the states each live set derives (§16), and `archived` lists what connecting a folder keeps of the old review store.",
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
    // A rewrite after a replacement (§9): the record under the skipped
    // epoch is a1, and the device's own highest live operation on the
    // `rid`, the patch a2, lies above it, so the rewrite covers a2 too and
    // leaves none of the device's operations live.
    let skipped = [op(note, a, 1, 1, &[], false), op(note, a, 2, 2, &[], false)];
    f.case(json!({
        "kind": "covers_for",
        "name": "a_rewrite_covers_the_devices_later_live_operation_too",
        "ops": skipped,
        "chosen": [{"device": device(a).to_hex(), "counter": 1}],
        "own_device": device(a).to_hex(),
        "expect": [{"device": device(a).to_hex(), "counter": 2}],
    }));
    f.case(json!({
        "kind": "live_check",
        "name": "that_rewrite_is_a_state_operation",
        "ops": skipped,
        "next": op(note, a, 3, 3, &[(a, 2)], false),
        "expect": "state",
    }));
    journal_less(&mut f);
    f
}

fn rev(label: &str) -> String {
    sync_protocol::crypto::hash(label.as_bytes()).to_hex()
}

fn state(path: &str, revision: &str) -> Value {
    json!({"path": path, "revision": rev(revision), "delete": false})
}

fn object(rid: &str, object_id: &str, states: &[Value]) -> Value {
    json!({"rid": id(rid).to_hex(), "object_id": object_id, "states": states})
}

fn file(path: &str, revision: &str, object_id: &str) -> Value {
    json!({"path": path, "revision": rev(revision), "object_id": object_id})
}

fn verdict(kind: &str, rid: Option<&str>) -> Value {
    match rid {
        Some(rid) => json!({"verdict": kind, "rid": id(rid).to_hex()}),
        None => json!({"verdict": kind}),
    }
}

/// Matching files when a folder syncs without this device's journal
/// (§9, §15, §18). Each `objects` entry lists the states its live set
/// derives (§16), and `expect` gives one verdict per local file.
fn journal_less(f: &mut Fixture) {
    let plan = "match plan";
    let photo = "match photo";
    let doc = "match doc";
    f.case(json!({
        "kind": "journal_less_match",
        "name": "a_lost_journal_adopts_identical_files_and_holds_differing_ones",
        "objects": [
            object(plan, "note_01j9z3y7q8r2s4t6v8w0x2y4z6", &[state("Plan.md", "plan v1")]),
            object(photo, "", &[state("photo.png", "photo bytes")]),
            object(doc, "", &[state("Doc.md", "doc v1")]),
        ],
        "locals": [
            file("Plan.md", "plan v1", "note_01j9z3y7q8r2s4t6v8w0x2y4z6"),
            file("Pictures/photo.png", "photo bytes", ""),
            file("doc.md", "doc v2", ""),
            file("New.md", "new", ""),
        ],
        // The note by ID, the photo by revision but at another path, the
        // document by its folded path but with other content, and a file
        // nothing matches.
        "expect": [verdict("adopt", Some(plan)), verdict("held", Some(photo)), verdict("held", Some(doc)), verdict("new", None)],
    }));
    f.case(json!({
        "kind": "journal_less_match",
        "name": "a_managed_note_matched_by_id_at_another_path_is_held",
        "objects": [object(plan, "note_01j9z3y7q8r2s4t6v8w0x2y4z6", &[state("Plan.md", "plan v1")])],
        "locals": [file("Archive/Plan.md", "plan v1", "note_01j9z3y7q8r2s4t6v8w0x2y4z6"), file("Plan.md", "plan v1", "")],
        "expect": [verdict("held", Some(plan)), verdict("new", None)],
    }));
    f.case(json!({
        "kind": "journal_less_match",
        "name": "a_text_mode_note_matches_on_its_merged_body",
        // The state operation's own revision field names the body before
        // its live patches; the derived state is the merged body.
        "state_operation_revision": rev("plan before patches"),
        "objects": [object(plan, "note_01j9z3y7q8r2s4t6v8w0x2y4z6", &[state("Plan.md", "plan merged")])],
        "locals": [file("Plan.md", "plan merged", "note_01j9z3y7q8r2s4t6v8w0x2y4z6")],
        "expect": [verdict("adopt", Some(plan))],
    }));
    f.case(json!({
        "kind": "journal_less_match",
        "name": "a_file_matches_any_branch_of_a_conflict",
        "objects": [object(doc, "", &[state("Doc.md", "doc mine"), state("Doc.md", "doc theirs")])],
        "locals": [file("Doc.md", "doc theirs", "")],
        "expect": [verdict("adopt", Some(doc))],
    }));
    f.case(json!({
        "kind": "journal_less_match",
        "name": "a_grant_restored_after_a_reset_matches_the_held_files_first",
        // The files beneath a hold that ends when the domain's own marker
        // returns, matched before the domain's bootstrap writes any.
        "objects": [
            object("match pay sheet", "", &[state("Pay/Sheet.csv", "sheet v1")]),
            object("match pay memo", "", &[state("Pay/Memo.md", "memo v1")]),
        ],
        "locals": [file("Pay/Sheet.csv", "sheet v1", ""), file("Pay/Memo.md", "memo edited offline", ""), file("Pay/Draft.md", "draft", "")],
        "expect": [verdict("adopt", Some("match pay sheet")), verdict("held", Some("match pay memo")), verdict("new", None)],
    }));
    let gone = id("match device that is gone");
    let mine = id("match this device");
    f.case(json!({
        "kind": "archived",
        "name": "set_up_sync_again_keeps_a_branch_whose_author_device_is_gone",
        "objects": [{
            "rid": id(doc).to_hex(),
            "base": [mine.to_hex(), 4],
            "branches": [
                {"device": mine.to_hex(), "counter": 4, "delete": false, "kept": true},
                {"device": gone.to_hex(), "counter": 9, "delete": false, "kept": true},
                {"device": gone.to_hex(), "counter": 10, "delete": true, "kept": false},
            ],
        }],
        "expect": [[id(doc).to_hex(), gone.to_hex(), 9]],
    }));
}
