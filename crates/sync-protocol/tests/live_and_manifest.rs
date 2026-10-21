//! Live sets (§13) and the manifest and checkpoint chain (§17).

use std::collections::{BTreeMap, BTreeSet};

use proptest::prelude::*;
use sync_protocol::crypto::{SigningKey, hash_parts};
use sync_protocol::live::{LiveError, LiveOp, LiveSet, covers_for};
use sync_protocol::manifest::{
    BUCKETS, CheckpointChain, CheckpointError, Manifest, bucket_hash, bucket_of, descent_path,
    heads_digest, leaf_hash, skip_number, verify_bucket, verify_buckets, verify_descent,
    verify_manifest,
};
use sync_protocol::records::{Checkpoint, Cover, OpClass, Ref, Signed};
use sync_protocol::tag::Tag;
use sync_protocol::types::{Digest, Id, SecretKey};

const A: Id = Id([0xa0; 16]);
const B: Id = Id([0xb0; 16]);
const C: Id = Id([0xc0; 16]);
const RID: Id = Id([0x10; 16]);

fn op(device: Id, counter: u64, seq: u64, covers: &[(Id, u64)], delete: bool) -> LiveOp {
    LiveOp {
        rid: RID,
        device,
        counter,
        seq,
        digest: hash_parts(&[&device.0, &counter.to_be_bytes()]),
        delete,
        covers: covers
            .iter()
            .map(|&(device, counter)| Cover { device, counter })
            .collect(),
        weight: 300,
    }
}

fn live_keys(set: &LiveSet) -> Vec<(Id, u64)> {
    set.live(&RID)
        .iter()
        .map(|(o, _)| (o.device, o.counter))
        .collect()
}

#[test]
fn an_empty_supersedes_on_an_existing_rid_is_a_patch() {
    let mut set = LiveSet::new();
    assert_eq!(set.apply(op(A, 1, 1, &[], false)), OpClass::State);
    assert_eq!(set.apply(op(B, 1, 2, &[], false)), OpClass::Patch);
    assert_eq!(
        set.apply(op(A, 2, 3, &[(A, 1), (B, 1)], false)),
        OpClass::State
    );
    assert_eq!(live_keys(&set), vec![(A, 2)]);
    // A delete that covers nothing on an existing rid counts as a patch.
    assert_eq!(set.apply(op(C, 1, 4, &[], true)), OpClass::Patch);
}

#[test]
fn a_cover_supersedes_every_earlier_operation_of_its_device() {
    let mut set = LiveSet::new();
    set.apply(op(A, 1, 1, &[], false));
    set.apply(op(A, 2, 2, &[], false));
    set.apply(op(B, 1, 3, &[(A, 2)], false));
    assert_eq!(live_keys(&set), vec![(B, 1)]);
}

#[test]
fn an_edit_on_a_conflicted_rid_covers_only_its_branch_and_identical_copies() {
    let mut set = LiveSet::new();
    set.apply(op(A, 1, 1, &[], false));
    set.apply(op(A, 2, 2, &[(A, 1)], false));
    set.apply(op(B, 1, 3, &[(A, 1)], false));
    // Two branches: A's state at 2 and B's at 1.
    assert_eq!(live_keys(&set), vec![(A, 2), (B, 1)]);
    // C's file matched A's branch.
    let live = set.live(&RID);
    let refs: Vec<&LiveOp> = live.iter().map(|(o, _)| *o).collect();
    let chosen: BTreeSet<(Id, u64)> = [(A, 2)].into_iter().collect();
    let covers = covers_for(&refs, &chosen, C);
    assert_eq!(
        covers,
        vec![Cover {
            device: A,
            counter: 2
        }]
    );
    set.apply(op(C, 1, 4, &[(A, 2)], false));
    assert_eq!(live_keys(&set), vec![(B, 1), (C, 1)]);

    // Two identical live branches are covered together.
    let live = set.live(&RID);
    let refs: Vec<&LiveOp> = live.iter().map(|(o, _)| *o).collect();
    let both: BTreeSet<(Id, u64)> = [(B, 1), (C, 1)].into_iter().collect();
    assert_eq!(
        covers_for(&refs, &both, A),
        vec![
            Cover {
                device: B,
                counter: 1
            },
            Cover {
                device: C,
                counter: 1
            }
        ]
    );
}

#[test]
fn a_state_operation_must_cover_its_own_devices_earlier_state() {
    let mut set = LiveSet::new();
    set.apply(op(A, 1, 1, &[], false));
    set.apply(op(B, 1, 2, &[(A, 1)], false));
    // A writes again covering only B: its own state at 1 is already covered
    // by B, so this passes.
    assert!(set.check(&op(A, 2, 3, &[(B, 1)], false)).is_ok());
    let mut set = LiveSet::new();
    set.apply(op(A, 1, 1, &[], false));
    set.apply(op(B, 1, 2, &[], false));
    set.apply(op(B, 2, 3, &[(B, 1)], false));
    // B's state at 2 leaves... A's state at 1 live; A now writes a state
    // covering B only: its own state at 1 stays live.
    assert_eq!(
        set.check(&op(A, 2, 4, &[(B, 2)], false)),
        Err(LiveError::SupersedesIncomplete)
    );
    assert_eq!(
        set.check(&op(A, 2, 4, &[(B, 7)], false)),
        Err(LiveError::SupersedesUnknown)
    );
}

#[test]
fn a_lone_delete_leaves_the_live_set_when_a_rising_floor_passes_it() {
    let mut set = LiveSet::new();
    set.apply(op(A, 1, 1, &[], false));
    set.apply(op(A, 2, 2, &[(A, 1)], true));
    // The tombstone stays live alone.
    assert_eq!(live_keys(&set), vec![(A, 2)]);
    let before = Manifest::new(set.leaves_at(10));
    assert_eq!(before.live_objects(), 1);
    // A checkpoint whose floor rises from 0 to 5 drops it.
    let dropped = set.apply_floor(0, 5, 10);
    assert_eq!(dropped.len(), 1);
    let after = Manifest::new(set.leaves_at(10));
    assert_eq!(after.live_objects(), 0);
    assert_ne!(before.root(), after.root());
    // An edit on its rid that doesn't cover it revives the object alone.
    set.apply(op(B, 1, 11, &[], false));
    assert_eq!(live_keys(&set), vec![(B, 1)]);
    // A delete above the floor stays.
    let mut set = LiveSet::new();
    set.apply(op(A, 1, 1, &[], false));
    set.apply(op(A, 2, 8, &[(A, 1)], true));
    assert!(set.apply_floor(0, 5, 10).is_empty());
}

#[test]
fn an_edit_against_a_delete_keeps_both() {
    let mut set = LiveSet::new();
    set.apply(op(A, 1, 1, &[], false));
    set.apply(op(A, 2, 2, &[(A, 1)], true));
    set.apply(op(B, 1, 3, &[(A, 1)], false));
    assert_eq!(live_keys(&set), vec![(A, 2), (B, 1)]);
    // A floor never drops a delete that isn't alone.
    assert!(set.apply_floor(0, 5, 5).is_empty());
}

fn refs(n: u8) -> Vec<Ref> {
    vec![Ref {
        device: A,
        counter: u64::from(n),
        seq: u64::from(n),
        op_digest: Digest([n; 32]),
    }]
}

#[test]
fn the_leaf_and_bucket_hashes_follow_their_formulas() {
    let rid = Id([0xab; 16]);
    let r = refs(1);
    let mut leaf_input = Tag::ManifestLeaf.bytes();
    leaf_input.extend_from_slice(&rid.0);
    leaf_input.extend_from_slice(&1u32.to_be_bytes());
    leaf_input.extend_from_slice(&r[0].device.0);
    leaf_input.extend_from_slice(&1u64.to_be_bytes());
    leaf_input.extend_from_slice(&1u64.to_be_bytes());
    leaf_input.extend_from_slice(&[1; 32]);
    assert_eq!(leaf_hash(&rid, &r), hash_parts(&[&leaf_input]));
    // The first 12 bits of the rid pick the bucket.
    assert_eq!(bucket_of(&rid), 0xaba);
    let mut bucket_input = Tag::ManifestBucket.bytes();
    bucket_input.extend_from_slice(&0x0abau16.to_be_bytes());
    bucket_input.extend_from_slice(&0u32.to_be_bytes());
    assert_eq!(bucket_hash(0xaba, &[]), hash_parts(&[&bucket_input]));
}

#[test]
fn adding_removing_or_changing_one_leaf_changes_the_root() {
    let mut leaves: BTreeMap<Id, Vec<Ref>> = (1..=20u8).map(|n| (Id([n; 16]), refs(n))).collect();
    let base = Manifest::new(leaves.clone()).root();
    leaves.insert(Id([99; 16]), refs(99));
    let added = Manifest::new(leaves.clone()).root();
    leaves.remove(&Id([99; 16]));
    assert_eq!(Manifest::new(leaves.clone()).root(), base);
    leaves.remove(&Id([3; 16]));
    let removed = Manifest::new(leaves.clone()).root();
    leaves.insert(Id([3; 16]), refs(4));
    let changed = Manifest::new(leaves).root();
    let roots: BTreeSet<Digest> = [base, added, removed, changed].into_iter().collect();
    assert_eq!(roots.len(), 4);
}

#[test]
fn verification_rejects_a_manifest_that_omits_an_object() {
    let leaves: BTreeMap<Id, Vec<Ref>> = (1..=20u8).map(|n| (Id([n; 16]), refs(n))).collect();
    let full = Manifest::new(leaves.clone());
    let root = full.root();
    assert!(verify_manifest(&full, &root, 20).is_ok());
    let mut short = leaves;
    short.remove(&Id([7; 16]));
    let short = Manifest::new(short);
    assert!(verify_manifest(&short, &root, 20).is_err());
    // Bucket by bucket, as a bootstrap fetches them.
    let buckets = full.bucket_hashes();
    assert_eq!(buckets.len(), BUCKETS);
    assert!(verify_buckets(&buckets, &root).is_ok());
    let index = bucket_of(&Id([7; 16]));
    assert!(
        verify_bucket(
            index,
            &full.bucket_leaves(index),
            &buckets[usize::from(index)]
        )
        .is_ok()
    );
    assert!(
        verify_bucket(
            index,
            &short.bucket_leaves(index),
            &buckets[usize::from(index)]
        )
        .is_err()
    );
}

#[test]
fn skip_clears_the_lowest_set_bit() {
    assert_eq!(skip_number(1), 0);
    assert_eq!(skip_number(6), 4);
    assert_eq!(skip_number(12), 8);
    assert_eq!(descent_path(6, 1), vec![6, 4, 3, 2, 1]);
    assert_eq!(descent_path(13, 9), vec![13, 12, 11, 10, 9]);
}

proptest! {
    #[test]
    fn a_descent_walk_takes_at_most_k_k_plus_1_over_2_records(
        k in 1u32..24,
        a in any::<u64>(),
        b in any::<u64>(),
    ) {
        let limit = (1u64 << k) - 1;
        let (x, y) = (a % limit + 1, b % limit + 1);
        let (from, to) = (x.max(y), x.min(y));
        let path = descent_path(from, to);
        prop_assert_eq!(*path.last().unwrap(), to);
        let bound = u64::from(k) * (u64::from(k) + 1) / 2;
        prop_assert!(path.len() as u64 <= bound.max(1), "{} -> {}: {}", from, to, path.len());
    }
}

#[test]
fn a_million_checkpoints_take_at_most_210_records() {
    let worst = (1..=1_000_000u64)
        .step_by(997)
        .map(|to| descent_path(1_000_000, to).len())
        .max()
        .unwrap();
    assert!(worst <= 210, "{worst}");
}

fn checkpoint_chain(count: u64) -> Vec<Signed<Checkpoint>> {
    let key = SigningKey::from_seed(&SecretKey([3; 32]));
    let mut records: Vec<Signed<Checkpoint>> = Vec::new();
    for number in 1..=count {
        let previous = records.last().map_or(Digest::ZERO, |r| r.digest());
        let skip = match skip_number(number) {
            0 => Digest::ZERO,
            n => records[n as usize - 1].digest(),
        };
        let body = Checkpoint {
            workspace: Id([1; 16]),
            domain: Id([2; 16]),
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
            signer: Id([6; 16]),
        };
        records.push(Signed::sign(body, &key));
    }
    records
}

#[test]
fn a_descent_walk_through_skip_reaches_an_old_anchor() {
    let records = checkpoint_chain(40);
    let walk: Vec<Signed<Checkpoint>> = descent_path(40, 3)
        .iter()
        .map(|n| records[*n as usize - 1].clone())
        .collect();
    assert!(walk.len() < 38);
    assert!(verify_descent(&walk, &records[2].digest()).is_ok());
    // A checkpoint that doesn't descend from the anchor.
    assert!(verify_descent(&walk, &records[3].digest()).is_err());
    let mut tampered = walk.clone();
    tampered.remove(1);
    assert!(verify_descent(&tampered, &records[2].digest()).is_err());
}

#[test]
fn log_entry_checkpoints_link_by_previous_number_and_skip() {
    let records = checkpoint_chain(3);
    let mut chain = CheckpointChain::new();
    let empty = Manifest::default();
    for record in &records {
        chain.check(record, &[], &empty, &[], |_| false).unwrap();
        chain.accept(record);
    }
    // A checkpoint that skips a number, or rolls the floor back.
    let mut next = checkpoint_chain(4).pop().unwrap();
    next.body.number = 5;
    assert_eq!(
        chain.check(&next, &[], &empty, &[], |_| false),
        Err(CheckpointError::Link)
    );
    let mut later = checkpoint_chain(4).pop().unwrap();
    later.body.covered_seq = 30;
    assert_eq!(
        chain.check(&later, &[], &empty, &[], |_| false),
        Err(CheckpointError::Rollback)
    );
    // A head that fell back, and one that left without leave to.
    let good = checkpoint_chain(4).pop().unwrap();
    let old = vec![Ref {
        device: A,
        counter: 5,
        seq: 3,
        op_digest: Digest([1; 32]),
    }];
    let mut fallen = good.clone();
    let fell = vec![Ref {
        counter: 4,
        ..old[0]
    }];
    fallen.body.heads_digest = heads_digest(&fell);
    assert_eq!(
        chain.check(&fallen, &fell, &empty, &old, |_| false),
        Err(CheckpointError::Rollback)
    );
    assert_eq!(
        chain.check(&good, &[], &empty, &old, |_| false),
        Err(CheckpointError::Rollback)
    );
    assert!(chain.check(&good, &[], &empty, &old, |_| true).is_ok());
    // A root that differs from the client's live set.
    let mut leaves = BTreeMap::new();
    leaves.insert(RID, refs(1));
    assert_eq!(
        chain.check(&good, &[], &Manifest::new(leaves), &[], |_| false),
        Err(CheckpointError::Mismatch)
    );
}
