//! `sync-v2-manifest.json`: the manifest tree and checkpoints (§17).

use std::collections::BTreeMap;

use serde_json::{Value, json};
use sync_protocol::codec::{Encode, Sorted};
use sync_protocol::manifest::{
    Manifest, bucket_hash, bucket_of, descent_path, heads_digest, leaf_hash, node_hash, skip_number,
};
use sync_protocol::records::{Checkpoint, Ref, Signed};
use sync_protocol::types::{Digest, Id, to_hex};

use super::common::{Actor, Fixture, Shape, id};
use super::world::world;

fn refs(n: u8) -> Vec<Ref> {
    vec![Ref {
        device: id("manifest device"),
        counter: u64::from(n),
        seq: u64::from(n) + 10,
        op_digest: Digest([n; 32]),
    }]
}

fn leaves_json(leaves: &BTreeMap<Id, Vec<Ref>>) -> Value {
    json!(leaves
        .iter()
        .map(|(rid, refs)| json!({"rid": rid.to_hex(), "refs": serde_json::to_value(refs).unwrap()}))
        .collect::<Vec<_>>())
}

pub fn checkpoints(signer: &Actor, count: u64) -> Vec<Signed<Checkpoint>> {
    let mut records: Vec<Signed<Checkpoint>> = Vec::new();
    for number in 1..=count {
        let previous = records.last().map_or(Digest::ZERO, |r| r.digest());
        let skip = match skip_number(number) {
            0 => Digest::ZERO,
            n => records[n as usize - 1].digest(),
        };
        records.push(Signed::sign(
            Checkpoint {
                workspace: id("manifest workspace"),
                domain: id("manifest domain"),
                previous,
                number,
                skip,
                covered_seq: number * 10,
                floor: 0,
                policy_revision: 1,
                policy_head: Digest([4; 32]),
                live_objects: 0,
                manifest_root: Manifest::default().root(),
                heads_digest: heads_digest(&[]),
                epoch: 1,
                epoch_record: Digest([5; 32]),
                signer: signer.device(),
            },
            signer.keys.signing(),
        ));
    }
    records
}

pub fn fixture() -> Fixture {
    let w = world();
    let mut f = Fixture::new(
        "sync-v2-manifest.json",
        "The manifest and checkpoints (sync-v2.md §17): leaf, bucket, node and heads hashes, roots that change with each leaf, bucket checks that catch an omitted object, the checkpoint record and bundle, number and skip links, descent walks, and the checks a client runs on each checkpoint it reaches.",
    );
    f.actor(&w.alice_laptop);
    let rid = id("manifest rid");
    let r = refs(1);
    f.case(json!({"kind": "leaf_hash", "name": "one_live_operation", "rid": rid.to_hex(), "refs": serde_json::to_value(&r).unwrap(), "expect": leaf_hash(&rid, &r).to_hex()}));
    f.case(json!({"kind": "bucket_of", "name": "first_12_bits", "rid": "abcdef00000000000000000000000001", "expect": 0xabc}));
    f.case(json!({"kind": "bucket_hash", "name": "empty_bucket", "index": 7, "leaf_hashes": [], "expect": bucket_hash(7, &[]).to_hex()}));
    let leaf = leaf_hash(&rid, &r);
    f.case(json!({"kind": "bucket_hash", "name": "one_leaf", "index": bucket_of(&rid), "leaf_hashes": [leaf.to_hex()], "expect": bucket_hash(bucket_of(&rid), &[leaf]).to_hex()}));
    let children: Vec<Digest> = (0..16u8).map(|i| Digest([i; 32])).collect();
    f.case(json!({
        "kind": "node_hash",
        "name": "level_2_node",
        "level": 2,
        "index": 5,
        "children": children.iter().map(Digest::to_hex).collect::<Vec<_>>(),
        "expect": node_hash(2, 5, &children).to_hex(),
    }));
    f.case(json!({"kind": "heads_digest", "name": "two_heads", "heads": serde_json::to_value([refs(1)[0].clone(), Ref { device: id("manifest device 2"), ..refs(2)[0].clone() }]).unwrap(), "expect": heads_digest(&[refs(1)[0].clone(), Ref { device: id("manifest device 2"), ..refs(2)[0].clone() }]).to_hex()}));
    f.case(json!({"kind": "root", "name": "empty_manifest", "leaves": [], "expect": Manifest::default().root().to_hex(), "live_objects": 0}));

    let mut leaves: BTreeMap<Id, Vec<Ref>> = (1..=20u8)
        .map(|n| (id(&format!("manifest rid {n}")), refs(n)))
        .collect();
    let base = Manifest::new(leaves.clone());
    f.case(json!({"kind": "root", "name": "twenty_objects", "leaves": leaves_json(&leaves), "expect": base.root().to_hex(), "live_objects": 20}));
    leaves.insert(id("manifest rid added"), refs(99));
    f.case(json!({"kind": "root", "name": "one_leaf_added", "leaves": leaves_json(&leaves), "expect": Manifest::new(leaves.clone()).root().to_hex(), "live_objects": 21}));
    leaves.remove(&id("manifest rid added"));
    leaves.remove(&id("manifest rid 3"));
    f.case(json!({"kind": "root", "name": "one_leaf_removed", "leaves": leaves_json(&leaves), "expect": Manifest::new(leaves.clone()).root().to_hex(), "live_objects": 19}));
    leaves.insert(id("manifest rid 3"), refs(4));
    f.case(json!({"kind": "root", "name": "one_leaf_changed", "leaves": leaves_json(&leaves), "expect": Manifest::new(leaves.clone()).root().to_hex(), "live_objects": 20}));
    let mut omitted: BTreeMap<Id, Vec<Ref>> = (1..=20u8)
        .map(|n| (id(&format!("manifest rid {n}")), refs(n)))
        .collect();
    omitted.remove(&id("manifest rid 7"));
    f.case(json!({
        "kind": "verify_manifest",
        "name": "a_manifest_that_omits_an_object_the_root_commits_to",
        "leaves": leaves_json(&omitted),
        "root": base.root().to_hex(),
        "live_objects": 20,
        "expect": "sync_manifest_mismatch",
    }));
    let index = bucket_of(&id("manifest rid 7"));
    let short = Manifest::new(omitted.clone());
    f.case(json!({
        "kind": "verify_bucket",
        "name": "a_bucket_that_omits_an_object",
        "index": index,
        "leaves": leaves_json(&short.bucket_leaves(index).into_iter().collect()),
        "hash": base.bucket_hashes()[usize::from(index)].to_hex(),
        "expect": "sync_manifest_mismatch",
    }));
    f.case(json!({
        "kind": "verify_bucket",
        "name": "a_complete_bucket",
        "index": index,
        "leaves": leaves_json(&base.bucket_leaves(index).into_iter().collect()),
        "hash": base.bucket_hashes()[usize::from(index)].to_hex(),
        "expect": "ok",
    }));

    // The checkpoint record and its bundle.
    f.accept_signed(
        "checkpoint",
        "checkpoint",
        &w.checkpoint.checkpoint,
        &w.alice_laptop,
    );
    f.accept(
        "checkpoint_bundle",
        "checkpoint_bundle",
        &w.checkpoint,
        None,
        Shape::Body,
    );
    let mut zero = w.checkpoint.checkpoint.0.to_bytes();
    let at = 24 + 16 + 16 + 32;
    zero[at..at + 8].copy_from_slice(&0u64.to_be_bytes());
    f.reject(
        "checkpoint_number_0",
        "checkpoint",
        &zero,
        json!({"decode": "out_of_range"}),
    );
    let mut unsorted = w.checkpoint.clone();
    let head = unsorted.heads[0].clone();
    unsorted.heads = Sorted::unchecked(vec![head.clone(), head]);
    f.reject(
        "checkpoint_heads_naming_one_device_twice",
        "checkpoint_bundle",
        &unsorted.to_bytes(),
        json!({"decode": "not_sorted"}),
    );

    // Skip and descent.
    for n in [1u64, 2, 3, 6, 12, 1_000_000] {
        f.case(json!({"kind": "skip_number", "name": format!("skip_of_{n}"), "number": n, "expect": skip_number(n)}));
    }
    for (from, to) in [
        (6u64, 1u64),
        (13, 9),
        (40, 3),
        (1_000_000, 1),
        (1_048_575, 1),
    ] {
        let path = descent_path(from, to);
        f.case(json!({"kind": "descent_path", "name": format!("from_{from}_to_{to}"), "from": from, "to": to, "expect": path}));
    }
    let chain = checkpoints(&w.alice_laptop, 40);
    let walk: Vec<String> = descent_path(40, 3)
        .iter()
        .map(|n| to_hex(&chain[*n as usize - 1].to_bytes()))
        .collect();
    f.case(json!({"kind": "verify_descent", "name": "walk_through_skip_to_an_old_anchor", "walk": walk, "anchor": chain[2].digest().to_hex(), "expect": "ok"}));
    f.case(json!({"kind": "verify_descent", "name": "a_checkpoint_that_doesnt_descend_from_the_anchor", "walk": walk, "anchor": chain[3].digest().to_hex(), "expect": "sync_manifest_rollback"}));
    let mut gap = walk.clone();
    gap.remove(1);
    f.case(json!({"kind": "verify_descent", "name": "a_walk_missing_a_record", "walk": gap, "anchor": chain[2].digest().to_hex(), "expect": "sync_manifest_rollback"}));

    // Checks on checkpoints reached in the log.
    let short_chain: Vec<String> = chain[..3].iter().map(|c| to_hex(&c.to_bytes())).collect();
    let mut wrong_number = chain[3].body.clone();
    wrong_number.number = 5;
    let wrong_number = Signed::sign(wrong_number, w.alice_laptop.keys.signing());
    let mut rolled_back = chain[3].body.clone();
    rolled_back.covered_seq = 30;
    let rolled_back = Signed::sign(rolled_back, w.alice_laptop.keys.signing());
    let mut floor_falls = chain[3].body.clone();
    floor_falls.floor = 0;
    let check = |name: &str, next: &Signed<Checkpoint>, expect: &str| {
        json!({
            "kind": "checkpoint_check",
            "name": name,
            "accepted": short_chain,
            "next": to_hex(&next.to_bytes()),
            "heads": [],
            "previous_heads": [],
            "leaves": [],
            "expect": expect,
        })
    };
    f.case(check("the_next_checkpoint", &chain[3], "ok"));
    f.case(check("a_number_that_skips", &wrong_number, "link"));
    f.case(check("a_rolled_back_checkpoint", &rolled_back, "rollback"));
    let mut root_differs = chain[3].body.clone();
    root_differs.manifest_root = Digest([1; 32]);
    f.case(check(
        "a_root_that_differs_from_the_live_set",
        &Signed::sign(root_differs, w.alice_laptop.keys.signing()),
        "mismatch",
    ));
    let _ = floor_falls;
    f
}
