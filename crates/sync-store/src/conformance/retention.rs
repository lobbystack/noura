//! Conformance cases for retention, pruning, reclaiming, the blob collector
//! and the quota (§19, §20): the same schedules on both stores.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use sync_protocol::codec::{Decode, Encode, Sorted};
use sync_protocol::crypto::{SigningKey, hash};
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::{CursorCompactedDetail, EntryRecord, PageStatus, RecordRef};
use sync_protocol::records::{Card, Cover, Operation, PolicyKind, Signed};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};

use super::blobs::{add_domain, blob_for, upload_all};
use super::checkpoints::{Track, interval, push_checkpoint};
use super::domains::member_device;
use super::{Device, World, head_of, manager, push_failure, refusal};
use crate::{
    Backend, BlobToCollect, CheckpointInterval, CheckpointRow, ReadTxn, Recorded, Retention, Store,
    UPLOAD_TTL, WriteTxn,
};

/// The encoded length of a 272-byte operation with no covers or blobs.
fn operation_length() -> u64 {
    let key = SigningKey::random().expect("random");
    let op = Signed::sign(
        Operation {
            workspace: Id([1; 16]),
            domain: Id([2; 16]),
            device: Id([3; 16]),
            counter: 1,
            previous: Digest::ZERO,
            epoch: 1,
            policy_revision: 1,
            policy_head: Digest::ZERO,
            previous_attempt: Digest::ZERO,
            generation: 1,
            checkpoint_seq: 0,
            checkpoint: Digest::ZERO,
            rid: Id([4; 16]),
            delete: false,
            supersedes: Sorted::default(),
            blobs: Sorted::default(),
            nonce: Nonce::ZERO,
            ciphertext: sync_protocol::codec::Bytes(vec![0; 272]),
        },
        &key,
    );
    op.to_bytes().len() as u64
}

fn retention(retained_bytes: u64) -> Retention {
    Retention {
        retained_bytes,
        ..Retention::default()
    }
}

/// The checkpoint due now, at the head, with `floor` in its `floor` field.
async fn commit_checkpoint<B: Backend>(
    track: &mut Track,
    world: &World<B>,
    signer: usize,
    floor: u64,
) -> u64 {
    let page = world.pull(signer, world.store_floor().await).await;
    let covered = page.head;
    let bundle = track
        .checkpoint(world, signer, covered, &[], |c, _| c.floor = floor)
        .await;
    push_checkpoint(world, signer, &bundle)
        .await
        .expect("the checkpoint commits");
    track.accepted.push(bundle.checkpoint.0.clone());
    covered
}

impl<B: Backend> World<B> {
    /// The workspace domain's published floor.
    pub async fn store_floor(&self) -> u64 {
        self.domain_row(self.domain).await.retention_floor
    }

    /// A domain's head row.
    pub async fn domain_row(&self, domain: Id) -> crate::DomainRow {
        let workspace = self.workspace;
        self.store
            .backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.domain(workspace, domain).await })
            }))
            .await
            .expect("read")
            .expect("a head row")
    }

    /// The workspace's usage and history as `GET /v2/workspaces` reports.
    pub async fn usage(&self) -> (u64, u64, Option<u64>) {
        let response = self
            .store
            .workspaces(self.devices[0].session)
            .await
            .expect("workspaces");
        let info = response
            .workspaces
            .iter()
            .find(|w| w.workspace == self.workspace)
            .expect("listed");
        (info.used_bytes, info.history_bytes, info.quota_bytes)
    }

    /// The sequences of the domain's retained entries.
    pub async fn retained(&self, domain: Id) -> Vec<(u64, u8)> {
        let workspace = self.workspace;
        self.store
            .backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let raw = txn.entries_after(workspace, domain, 0, 100_000).await?;
                    Ok(raw.into_iter().map(|e| (e.seq, e.record_type)).collect())
                })
            }))
            .await
            .expect("read")
    }
}

/// The retention floor (§19): the `covered_seq` of the newest checkpoint
/// with `retained_bytes` of weight after its entry, 0 while none qualifies,
/// and never lower than a floor the server published. `GET /v2/limits`
/// reports the setting.
pub async fn retention_floor_follows_the_weight_after_a_checkpoint<B: Backend>(store: Store<B>) {
    let length = operation_length();
    let retained = 3 * length;
    let store = store
        .with_checkpoint_interval(interval(4))
        .with_retention(retention(retained));
    assert_eq!(store.limits().retained_bytes, retained);
    let world = World::new(store, 1).await;
    let mut track = Track::new(&world).await;
    let (w, d) = (world.workspace, world.domain);
    track.write(&world, 0).await;
    track.write(&world, 0).await;
    assert_eq!(world.store.raise_floor(w, d).await.expect("floor"), 0);
    let covered = commit_checkpoint(&mut track, &world, 0, 0).await;
    assert_eq!(covered, 4);
    track.write(&world, 0).await;
    track.write(&world, 0).await;
    assert_eq!(
        world.store.raise_floor(w, d).await.expect("floor"),
        0,
        "two operations weigh less than retained_bytes"
    );
    track.write(&world, 0).await;
    assert_eq!(
        world.store.raise_floor(w, d).await.expect("floor"),
        covered,
        "three operations after its entry qualify the checkpoint"
    );
    assert_eq!(world.pull(0, covered).await.retention_floor, covered);

    // A floor the server published never falls, whatever the rule says.
    let higher = covered + 1;
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move { txn.set_retention_floor(w, d, higher).await })
        }))
        .await
        .expect("set");
    assert_eq!(world.store.raise_floor(w, d).await.expect("floor"), higher);
}

/// Review item M10: a checkpoint's entry weighs its 128 KiB of bucket
/// hashes, so `retained_bytes` bounds how many checkpoints stay servable.
pub async fn bucket_hashes_count_in_weight<B: Backend>(store: Store<B>) {
    let store = store
        .with_checkpoint_interval(interval(1))
        .with_retention(retention(200 * 1024));
    let world = World::new(store, 1).await;
    let mut track = Track::new(&world).await;
    let (w, d) = (world.workspace, world.domain);
    let mut covered = Vec::new();
    for _ in 0..4 {
        track.write(&world, 0).await;
        covered.push(commit_checkpoint(&mut track, &world, 0, 0).await);
    }
    // Two later checkpoint entries weigh 256 KiB and more; one weighs less
    // than 200 KiB's worth only when no bucket hashes count.
    assert_eq!(
        world.store.raise_floor(w, d).await.expect("floor"),
        covered[1],
        "the second checkpoint has two checkpoint entries after it"
    );
    world.store.maintain().await.expect("pruned");
    let cleared: Vec<bool> = {
        let store = &world.store;
        let mut cleared = Vec::new();
        for number in 1..=4 {
            let hashes = store
                .backend()
                .read(Box::new(move |txn: &mut B::Read| {
                    Box::pin(async move { txn.checkpoint_buckets(w, d, number).await })
                }))
                .await
                .expect("read");
            cleared.push(hashes.is_none());
        }
        cleared
    };
    assert_eq!(cleared, [true, false, false, false]);
    // Every record stays, with no heads once the floor passed it.
    let old = refusal(
        world
            .store
            .manifest_buckets(world.devices[0].session, w, d, covered[0])
            .await,
    );
    assert_eq!(old.code, "cursor_compacted");
    let entry = world
        .store
        .checkpoint_by_digest(world.devices[0].session, w, d, track.accepted[0].digest())
        .await
        .expect("the record stays");
    let EntryRecord::Checkpoint(bundle) = entry.record else {
        panic!("a checkpoint entry");
    };
    assert_eq!(bundle.checkpoint.0, track.accepted[0]);
    assert!(bundle.heads.is_empty());
}

/// A device's next operation on `rid`, covering `counter` of its own.
async fn edit<B: Backend>(
    track: &mut Track,
    world: &World<B>,
    device: usize,
    rid: Id,
    covered: u64,
    delete: bool,
) -> u64 {
    let id = world.devices[device].id();
    track
        .write_with(world, device, |op| {
            op.rid = rid;
            op.delete = delete;
            op.supersedes = Sorted::new(vec![Cover {
                device: id,
                counter: covered,
            }])
            .expect("sorted");
        })
        .await
}

/// Pruning (§19): after each batch of a run that stops at any point, the
/// live set at every servable checkpoint stays complete; a later pass
/// finishes. Covered operations and dropped lone deletes go, the newest
/// epoch and policy entries at or below the floor stay, and a fetch of a
/// pruned position answers `pruned`.
pub async fn pruning_keeps_servable_checkpoints_through_a_crash<B: Backend>(store: Store<B>) {
    let store = store
        .with_checkpoint_interval(interval(1))
        .with_retention(Retention {
            retained_bytes: 1,
            quota_bytes: None,
            prune_batch: 1,
        });
    let world = World::new(store, 2).await;
    let mut track = Track::new(&world).await;
    let (w, d) = (world.workspace, world.domain);
    let session = world.devices[1].session;
    let device = world.devices[0].id();

    // A kept object edited twice, a deleted one and an untouched one.
    track.write(&world, 0).await;
    let kept = track.rids[0];
    edit(&mut track, &world, 0, kept, 1, false).await;
    track.write(&world, 0).await;
    let gone = track.rids[2];
    let delete_seq = edit(&mut track, &world, 0, gone, 3, true).await;
    track.write(&world, 1).await;
    let first = commit_checkpoint(&mut track, &world, 0, 0).await;
    let last_edit = edit(&mut track, &world, 0, kept, 2, false).await;
    world.store.raise_floor(w, d).await.expect("floor");
    assert_eq!(world.store_floor().await, first);

    // The next checkpoint's floor field drops the lone delete.
    track.dropped.insert(gone);
    let second = commit_checkpoint(&mut track, &world, 0, first).await;
    edit(&mut track, &world, 0, kept, 5, false).await;
    world.store.raise_floor(w, d).await.expect("floor");
    let floor = world.store_floor().await;
    assert_eq!(floor, second);
    let third = commit_checkpoint(&mut track, &world, 0, floor).await;
    let head_entries = world.retained(d).await;

    // One batch at a time, as if each could be the last before a crash.
    let servable = [second, third];
    let live_of = async |at: u64| {
        let mut ops = Vec::new();
        let mut after = None;
        loop {
            let page = world
                .store
                .live(session, w, d, at, None, after.clone())
                .await
                .expect("servable");
            ops.extend(page.entries.0.iter().map(|e| e.seq));
            match page.next {
                Some(next) => after = Some(next),
                None => return ops,
            }
        }
    };
    let before = [live_of(servable[0]).await, live_of(servable[1]).await];
    let (mut after, mut batches) = (0, 0);
    loop {
        let (_, next) = world.store.prune_batch(w, d, after).await.expect("a batch");
        batches += 1;
        assert_eq!(
            live_of(servable[0]).await,
            before[0],
            "after batch {batches}"
        );
        assert_eq!(
            live_of(servable[1]).await,
            before[1],
            "after batch {batches}"
        );
        match next {
            // The crash: nothing remembers where the run stood.
            Some(_) if batches == 2 => break,
            Some(next) => after = next,
            None => panic!("two batches can't finish"),
        }
    }
    world.store.maintain().await.expect("finishes");
    assert_eq!(world.domain_row(d).await.pruned_floor, floor);
    assert_eq!(live_of(servable[0]).await, before[0]);
    assert_eq!(live_of(servable[1]).await, before[1]);

    let retained = world.retained(d).await;
    assert!(retained.len() < head_entries.len());
    let seqs: BTreeSet<u64> = retained.iter().map(|(seq, _)| *seq).collect();
    assert!(!seqs.contains(&delete_seq), "the dropped delete went");
    assert!(seqs.contains(&last_edit), "an edit above the floor stays");
    assert!(
        retained.iter().any(|(_, t)| *t == 9),
        "the newest policy entry at the floor stays"
    );
    assert!(
        retained.iter().any(|(_, t)| *t == 8),
        "the newest epoch entry at the floor stays"
    );

    // A pruned position, and a writer whose cover names it.
    assert_eq!(
        refusal(world.store.chain_record(session, w, d, device, 1).await).code,
        "pruned"
    );
    let head = track.heads[&device].clone();
    let stale = world.op(0, |op| {
        op.counter = head.counter + 1;
        op.previous = head.op_digest;
        op.rid = gone;
        op.supersedes = Sorted::new(vec![Cover { device, counter: 4 }]).expect("sorted");
    });
    assert_eq!(
        push_failure(world.push(0, &[stale]).await),
        ("supersedes_unknown", 0)
    );
    // The chain head outlives its records.
    assert_eq!(
        world
            .store
            .chain_head(session, w, d, device)
            .await
            .expect("a head")
            .counter,
        head.counter
    );
    // Later checkpoints still commit over the pruned log.
    track.write(&world, 0).await;
    commit_checkpoint(&mut track, &world, 0, floor).await;
}

/// The floor in the pull and bootstrap routes (§15, §18): a cursor below it
/// gets a status 2 page, `live` and `manifest` below it answer
/// `cursor_compacted` with the floor and the latest checkpoint.
pub async fn the_floor_compacts_cursors_and_routes<B: Backend>(store: Store<B>) {
    let store = store
        .with_checkpoint_interval(interval(1))
        .with_retention(retention(1));
    let world = World::new(store, 1).await;
    let mut track = Track::new(&world).await;
    let (w, d) = (world.workspace, world.domain);
    let session = world.devices[0].session;
    track.write(&world, 0).await;
    let first = commit_checkpoint(&mut track, &world, 0, 0).await;
    track.write(&world, 0).await;
    let second = commit_checkpoint(&mut track, &world, 0, 0).await;
    track.write(&world, 0).await;
    world.store.raise_floor(w, d).await.expect("floor");
    assert_eq!(world.store_floor().await, second);

    let page = world.pull(0, first).await;
    assert_eq!(page.status, PageStatus::Compacted);
    assert_eq!(page.retention_floor, second);
    assert_eq!(page.checkpoint_seq, second);
    assert_eq!(page.complete_through, first);
    assert!(page.entries.is_empty());
    page.check_shape().expect("a well-formed page");

    let page = world.pull(0, second).await;
    assert_eq!(page.status, PageStatus::Ok);
    assert_eq!(page.retention_floor, second);

    for refused in [
        refusal(world.store.live(session, w, d, first, None, None).await),
        refusal(world.store.manifest_buckets(session, w, d, first).await),
    ] {
        assert_eq!(refused.code, "cursor_compacted");
        let detail = CursorCompactedDetail::from_bytes(&refused.detail.expect("a detail"))
            .expect("a detail");
        assert_eq!(detail.retention_floor, second);
        assert_eq!(detail.checkpoint_covered_seq, second);
    }
    world
        .store
        .live(session, w, d, second, None, None)
        .await
        .expect("the floor's checkpoint serves");
}

/// `delete_domain` frees usage at once, and the server reclaims the log,
/// checkpoints and blobs afterwards, in batches (§19).
pub async fn deleted_domains_are_reclaimed_in_batches<B: Backend>(store: Store<B>) {
    let mut world = World::new(store.with_retention(retention(1 << 30)), 1).await;
    let restricted = add_domain(&mut world).await;
    let (w, session) = (world.workspace, world.devices[0].session);
    let (ciphertext, creation) = blob_for(1_500_000, restricted);
    let blob = creation.blob;
    world
        .store
        .create_blob(session, w, creation)
        .await
        .expect("created");
    upload_all(&world, 0, blob, &ciphertext).await;
    let row = world.domain_row(restricted).await;
    assert!(row.usage_bytes >= creation.length);

    let delete = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::DeleteDomain { domain: restricted },
    );
    let before = world.usage().await.0;
    world
        .push_records(0, vec![RecordRef::Policy(delete.clone())])
        .await
        .expect("the delete commits");
    world.head = (delete.body.revision, delete.digest());
    assert!(world.domain_row(restricted).await.deleted);
    assert!(
        world.usage().await.0 < before,
        "the domain's usage stops counting when the delete commits"
    );

    world.store.maintain().await.expect("reclaims");
    assert!(
        world.retained(restricted).await.is_empty(),
        "the log went in batches"
    );
    let due = world.store.collectable_blobs().await.expect("due");
    assert_eq!(due.len(), 1, "the domain's blob waits for the collector");
    let segments = world
        .store
        .mark_blob(due[0])
        .await
        .expect("marked")
        .expect("no listing");
    assert_eq!(segments.len(), 2);
    world.store.finish_blob(due[0]).await.expect("finished");
    world.store.maintain().await.expect("finishes the domain");
    let row = world
        .store
        .backend()
        .read(Box::new(move |txn: &mut B::Read| {
            Box::pin(async move { txn.domain(w, restricted).await })
        }))
        .await
        .expect("read");
    assert_eq!(row, None, "the head row goes last");
}

/// The blob collector (§19): an incomplete upload and a completed unlisted
/// one go after 24 hours, a listed blob stays until its last listing leaves
/// through pruning, and the three steps survive a crash between them.
pub async fn the_collector_follows_a_blobs_three_states<B: Backend>(store: Store<B>) {
    let store = store
        .with_checkpoint_interval(interval(1))
        .with_retention(retention(1));
    let world = World::new(store, 1).await;
    let (w, session) = (world.workspace, world.devices[0].session);
    let store = world.store.clone();
    let day = Duration::from_secs(UPLOAD_TTL + 1);

    // Incomplete, then expired.
    let (_, incomplete) = blob_for(1_500_000, world.domain);
    let used = world.usage().await.0;
    store
        .create_blob(session, w, incomplete)
        .await
        .expect("created");
    assert_eq!(world.usage().await.0, used + incomplete.length);
    assert!(store.collectable_blobs().await.expect("due").is_empty());
    store.clock().advance(day);
    let target = BlobToCollect {
        workspace: w,
        domain: world.domain,
        blob: incomplete.blob,
    };
    assert_eq!(store.collectable_blobs().await.expect("due"), [target]);
    store.mark_blob(target).await.expect("marked").expect("due");
    assert_eq!(world.usage().await.0, used, "marking frees the usage");
    // A crash here leaves a marked row that the next pass finishes.
    assert_eq!(store.collectable_blobs().await.expect("due"), [target]);
    assert_eq!(
        refusal(store.create_blob(session, w, incomplete).await).code,
        "blob_incomplete"
    );
    store
        .mark_blob(target)
        .await
        .expect("again")
        .expect("again");
    assert_eq!(world.usage().await.0, used, "marking twice counts once");
    store.finish_blob(target).await.expect("finished");
    assert!(store.collectable_blobs().await.expect("due").is_empty());

    // Completed, unlisted: 24 hours after completion, not creation.
    let (ciphertext, completed) = blob_for(1_500_000, world.domain);
    store
        .create_blob(session, w, completed)
        .await
        .expect("created");
    store.clock().advance(Duration::from_secs(UPLOAD_TTL / 2));
    let Recorded::Stored(row) = upload_all(&world, 0, completed.blob, &ciphertext).await else {
        panic!("stored");
    };
    assert!(row.complete);
    store
        .clock()
        .advance(Duration::from_secs(UPLOAD_TTL / 2 + 60));
    assert!(store.collectable_blobs().await.expect("due").is_empty());
    let mut track = Track::new(&world).await;
    let listed = completed.blob;
    track
        .write_with(&world, 0, |op| {
            op.blobs = Sorted::new(vec![listed]).expect("sorted");
        })
        .await;
    store.clock().advance(day);
    assert!(
        store.collectable_blobs().await.expect("due").is_empty(),
        "a listed blob has no deletion time"
    );

    // Its last listing leaves through pruning.
    let rid = track.rids[0];
    edit(&mut track, &world, 0, rid, 1, false).await;
    let covered = commit_checkpoint(&mut track, &world, 0, 0).await;
    track.write(&world, 0).await;
    world
        .store
        .raise_floor(w, world.domain)
        .await
        .expect("floor");
    assert_eq!(world.store_floor().await, covered);
    store.maintain().await.expect("pruned");
    let due = store.collectable_blobs().await.expect("due");
    assert_eq!(due.iter().map(|t| t.blob).collect::<Vec<_>>(), [listed]);
    // A push never lists a marked blob.
    store.mark_blob(due[0]).await.expect("marked").expect("due");
    let head = track.heads[&world.devices[0].id()].clone();
    let late = world.op(0, |op| {
        op.counter = head.counter + 1;
        op.previous = head.op_digest;
        op.blobs = Sorted::new(vec![listed]).expect("sorted");
    });
    assert_eq!(
        push_failure(world.push(0, &[late]).await),
        ("blob_incomplete", 0)
    );
}

/// A push racing the collector for one blob (§19): whichever takes the
/// blob's row first wins, and the two never disagree.
pub async fn a_push_racing_the_collector_stays_consistent<B: Backend>(store: Store<B>) {
    let world = Arc::new(World::new(store, 1).await);
    let (w, d) = (world.workspace, world.domain);
    let session = world.devices[0].session;
    let day = Duration::from_secs(UPLOAD_TTL + 1);
    let mut head = (0, Digest::ZERO);
    for _ in 0..8 {
        let (ciphertext, creation) = blob_for(1_500_000, d);
        let blob = creation.blob;
        world
            .store
            .create_blob(session, w, creation)
            .await
            .expect("created");
        upload_all(&world, 0, blob, &ciphertext).await;
        world.store.clock().advance(day);
        let op = world.op(0, |op| {
            op.counter = head.0 + 1;
            op.previous = head.1;
            op.blobs = Sorted::new(vec![blob]).expect("sorted");
        });
        let digest = op.digest();
        let target = BlobToCollect {
            workspace: w,
            domain: d,
            blob,
        };
        let pushing = {
            let world = world.clone();
            tokio::spawn(async move { world.push(0, &[op]).await.map(|_| ()) })
        };
        let collecting = {
            let world = world.clone();
            tokio::spawn(async move { world.store.mark_blob(target).await })
        };
        let pushed = pushing.await.expect("joined");
        let marked = collecting.await.expect("joined").expect("no failure");
        match (pushed, marked) {
            (Ok(()), None) => head = (head.0 + 1, digest),
            (Err(_), Some(_)) => {}
            (pushed, marked) => panic!("they disagree: {pushed:?} and {marked:?}"),
        }
    }
}

/// An operation moves its domain's head row whole, whether it starts a
/// `rid`, replaces one's live operation or shares its push with another.
pub async fn an_operation_moves_its_head_row<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let device = world.devices[0].id();
    let start = world.domain_row(world.domain).await;

    let first = world.op(0, |_| {});
    world
        .push(0, std::slice::from_ref(&first))
        .await
        .expect("first");
    let first_length = first.to_bytes().len() as u64;
    let row = world.domain_row(world.domain).await;
    assert_eq!(row.head_seq, start.head_seq + 1);
    assert_eq!(
        row.entries_since_checkpoint,
        start.entries_since_checkpoint + 1
    );
    assert_eq!(
        row.weight_since_checkpoint,
        start.weight_since_checkpoint + first_length
    );
    assert_eq!(row.log_weight, start.log_weight + first_length);
    assert_eq!(row.usage_bytes, start.usage_bytes + first_length);
    assert_eq!(row.history_bytes, start.history_bytes);

    let replacement = world.op(0, |op| {
        op.rid = first.body.rid;
        op.counter = 2;
        op.previous = first.digest();
        op.supersedes = Sorted::new(vec![Cover { device, counter: 1 }]).expect("sorted");
    });
    let next = world.op(0, |op| {
        op.counter = 3;
        op.previous = replacement.digest();
    });
    world
        .push(0, &[replacement.clone(), next.clone()])
        .await
        .expect("replacement and a new rid");
    let added = (replacement.to_bytes().len() + next.to_bytes().len()) as u64;
    let row = world.domain_row(world.domain).await;
    assert_eq!(row.head_seq, start.head_seq + 3);
    assert_eq!(
        row.entries_since_checkpoint,
        start.entries_since_checkpoint + 3
    );
    assert_eq!(row.log_weight, start.log_weight + first_length + added);
    assert_eq!(row.usage_bytes, start.usage_bytes + added);
    assert_eq!(row.history_bytes, start.history_bytes + first_length);

    // A cover names one counter, and takes that device's lower ones too.
    let state = world.op(0, |op| {
        op.counter = 4;
        op.previous = next.digest();
    });
    let first_patch = world.op(0, |op| {
        op.rid = state.body.rid;
        op.counter = 5;
        op.previous = state.digest();
    });
    let second_patch = world.op(0, |op| {
        op.rid = state.body.rid;
        op.counter = 6;
        op.previous = first_patch.digest();
    });
    world
        .push(
            0,
            &[state.clone(), first_patch.clone(), second_patch.clone()],
        )
        .await
        .expect("a state and two patches");
    let before = world.domain_row(world.domain).await;
    let live = world
        .store
        .live_at(world.workspace, state.body.rid, u64::MAX);
    assert_eq!(live.await.expect("live").len(), 3);
    let covering = world.op(0, |op| {
        op.rid = state.body.rid;
        op.counter = 7;
        op.previous = second_patch.digest();
        op.supersedes = Sorted::new(vec![Cover { device, counter: 6 }]).expect("sorted");
    });
    world
        .push(0, std::slice::from_ref(&covering))
        .await
        .expect("the cover");
    let moved = (state.to_bytes().len()
        + first_patch.to_bytes().len()
        + second_patch.to_bytes().len()) as u64;
    let row = world.domain_row(world.domain).await;
    assert_eq!(
        row.usage_bytes,
        before.usage_bytes + covering.to_bytes().len() as u64 - moved,
        "all three leave usage"
    );
    assert_eq!(row.history_bytes, before.history_bytes + moved);
}

/// An operation sets the due point on its head row at the entry that
/// reaches the interval, and leaves it there until a checkpoint.
pub async fn an_operation_sets_the_due_point<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_checkpoint_interval(interval(4)), 1).await;
    let mut track = Track::new(&world).await;
    // The genesis and epoch 1 are entries 1 and 2.
    assert_eq!(track.write(&world, 0).await, 3);
    assert_eq!(world.domain_row(world.domain).await.checkpoint_due, 0);
    assert_eq!(track.write(&world, 0).await, 4);
    assert_eq!(world.domain_row(world.domain).await.checkpoint_due, 4);
    track.write(&world, 0).await;
    assert_eq!(world.domain_row(world.domain).await.checkpoint_due, 4);
}

/// `GET /v2/workspaces` reports usage, history and the quota (§20): live
/// operations and blobs count as usage, what they replaced as history, and
/// a delete frees its object's usage when it commits.
pub async fn usage_counts_what_lives_at_the_head<B: Backend>(store: Store<B>) {
    let store = store.with_retention(Retention {
        quota_bytes: Some(1 << 30),
        ..Retention::default()
    });
    let world = World::new(store, 1).await;
    let mut track = Track::new(&world).await;
    let length = operation_length();
    let id = world.devices[0].id();
    let (base, base_history, quota) = world.usage().await;
    assert_eq!(quota, Some(1 << 30));
    assert!(base > 0, "the genesis and epoch bundle count");

    track.write(&world, 0).await;
    assert_eq!(world.usage().await.0, base + length);
    assert_eq!(world.usage().await.1, base_history);
    let rid = track.rids[0];
    track
        .write_with(&world, 0, |op| {
            op.rid = rid;
            op.supersedes = Sorted::new(vec![Cover {
                device: id,
                counter: 1,
            }])
            .expect("sorted");
        })
        .await;
    let (used, history, _) = world.usage().await;
    assert_eq!(history, base_history + length, "the first edit is history");
    let delete = edit(&mut track, &world, 0, rid, 2, true).await;
    assert!(delete > 0);
    let (after, _, _) = world.usage().await;
    assert!(
        after <= used + 64,
        "a delete replaces its object: {after}, {used}"
    );

    // A blob counts from its creation, and only history holds it once its
    // listing op is covered.
    let (ciphertext, creation) = blob_for(1_500_000, world.domain);
    let blob = creation.blob;
    world
        .store
        .create_blob(world.devices[0].session, world.workspace, creation)
        .await
        .expect("created");
    assert_eq!(world.usage().await.0, after + creation.length);
    upload_all(&world, 0, blob, &ciphertext).await;
    track
        .write_with(&world, 0, |op| {
            op.blobs = Sorted::new(vec![blob]).expect("sorted");
        })
        .await;
    let listed_rid = *track.rids.last().expect("a rid");
    let listing = track.heads[&id].counter;
    let (with_blob, history_before, _) = world.usage().await;
    edit(&mut track, &world, 0, listed_rid, listing, false).await;
    let (without_blob, history_after, _) = world.usage().await;
    assert!(without_blob + creation.length <= with_blob + 64);
    assert!(history_after >= history_before + creation.length);
}

/// A quota of `quota` bytes on a world with `devices` devices, checkpoints
/// due every `entries` entries.
async fn quota_world<B: Backend>(
    store: Store<B>,
    quota: u64,
    entries: u64,
    devices: usize,
) -> World<B> {
    let store = store
        .with_checkpoint_interval(CheckpointInterval {
            entries,
            weight: u64::MAX,
        })
        .with_retention(Retention {
            retained_bytes: 1 << 40,
            quota_bytes: Some(quota),
            ..Retention::default()
        });
    World::new(store, devices).await
}

/// Over quota (§20): the server refuses what raises usage, `quota_exceeded`
/// with the record's index, and accepts deletes, checkpoints and a change
/// for the signer's own account; a tus creation that wouldn't fit fails.
pub async fn over_quota_the_server_takes_what_frees_space<B: Backend>(store: Store<B>) {
    let world = quota_world(store, 16_000, 10, 2).await;
    let mut track = Track::new(&world).await;
    let (w, session) = (world.workspace, world.devices[0].session);
    let (base, _, _) = world.usage().await;
    let length = operation_length();
    let fit = (16_000 - base) / length;
    // The filling writes leave less than one operation of room.
    for _ in 0..fit {
        track.write(&world, 0).await;
    }
    let head = track.heads[&world.devices[0].id()].clone();
    let refused = world.op(0, |op| {
        op.counter = head.counter + 1;
        op.previous = head.op_digest;
    });
    assert_eq!(
        push_failure(world.push(0, &[refused]).await),
        ("quota_exceeded", 0),
        "the next record would pass the quota"
    );

    let (_, creation) = blob_for(1_500_000, world.domain);
    assert_eq!(
        refusal(world.store.create_blob(session, w, creation).await).code,
        "quota_exceeded"
    );

    // A device of the signer's own account joins over quota, an addition
    // from anyone else doesn't.
    let mut world = world;
    let (added, head) = world.add_device(0).await;
    let card = world.devices[added].keys.card(world.account);
    let own = world.change(0, vec![head], PolicyKind::AccountHeads);
    let envelope = world.envelope(0, &card, 1, &world.epoch_key);
    world
        .push_records(
            0,
            vec![
                RecordRef::Policy(own.clone()),
                RecordRef::Envelope(envelope),
            ],
        )
        .await
        .expect("a device of the signer's own account joins over quota");
    world.head = (own.body.revision, own.digest());
    assert!(world.usage().await.0 > 16_000 - length);

    let restricted = Id::random().expect("random");
    let create = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::CreateDomain {
            domain: restricted,
            marker_rid: Id::random().expect("random"),
            access: manager(world.account),
        },
    );
    let creator = &world.devices[0];
    let epoch = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: w,
                domain: restricted,
                epoch: 1,
                previous: Digest::ZERO,
                policy_revision: world.head.0 + 1,
                policy_head: create.digest(),
                rotating_device: creator.id(),
                recipients: BTreeSet::from([creator.id()]),
            },
            &SecretKey::random().expect("random"),
            None,
            Nonce::ZERO,
        ),
        creator.keys.signing(),
    );
    let bundle = world.bundle(&epoch, &BTreeSet::from([creator.id()]));
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![RecordRef::Policy(create), bundle])
                .await
        ),
        ("quota_exceeded", 0),
        "a change that adds a domain fails"
    );

    // A delete doesn't raise usage, and a smaller replacement frees space.
    let before = world.usage().await.0;
    let rid = track.rids[0];
    edit(&mut track, &world, 0, rid, 1, true).await;
    assert!(
        world.usage().await.0 <= before + 64,
        "a delete replaces its object's usage with its own record"
    );
    // Checkpoints go through.
    let due = world.pull(0, 0).await.checkpoint_due;
    if due > 0 {
        commit_checkpoint(&mut track, &world, 0, 0).await;
    }
}

/// The pressure rules (§19, §20): usage and history crossing the quota make
/// a checkpoint due once in each domain with entries, and floors rise one
/// checkpoint at a time until the workspace fits, leaving history that the
/// quota doesn't need.
pub async fn history_yields_to_the_quota_only_as_far_as_it_must<B: Backend>(store: Store<B>) {
    let length = operation_length();
    let base_guess = 8_000;
    let quota = base_guess + 40 * length;
    let world = quota_world(store, quota, 1_000_000, 1).await;
    let mut track = Track::new(&world).await;
    let (base, history, _) = world.usage().await;
    assert!(
        base + history + length * 2 < quota,
        "the fixture leaves room"
    );

    // One object, edited over and over: usage stands still, history grows.
    track.write(&world, 0).await;
    let rid = track.rids[0];
    let mut covered = 1;
    let mut checkpoints = 0;
    loop {
        let seq = edit(&mut track, &world, 0, rid, covered, false).await;
        covered += 1;
        let page = world.pull(0, world.store_floor().await).await;
        let (used, history, _) = world.usage().await;
        if used + history > quota {
            assert_eq!(
                page.checkpoint_due, 0,
                "the crossing itself is no entry yet"
            );
            let at = edit(&mut track, &world, 0, rid, covered, false).await;
            covered += 1;
            assert_eq!(
                world
                    .pull(0, world.store_floor().await)
                    .await
                    .checkpoint_due,
                at,
                "the next entry brings the due point"
            );
            break;
        }
        assert!(seq > 0);
        // Checkpoints every few edits give floors something to climb, and
        // only the crossing makes one due here, so inject the due point.
        if covered % 8 == 0 {
            let (w, d) = (world.workspace, world.domain);
            let head = page.head;
            world
                .store
                .backend()
                .write(Box::new(move |txn: &mut B::Write| {
                    Box::pin(async move {
                        let row = txn.domain(w, d).await?.expect("a row");
                        txn.set_checkpoint_progress(
                            w,
                            d,
                            &crate::CheckpointProgress {
                                entries: row.entries_since_checkpoint,
                                weight: row.weight_since_checkpoint,
                                due: head,
                                log_weight: row.log_weight,
                                crossing_seen: row.crossing_seen,
                            },
                        )
                        .await
                    })
                }))
                .await
                .expect("due");
            commit_checkpoint(&mut track, &world, 0, 0).await;
            checkpoints += 1;
        }
    }
    assert!(
        checkpoints >= 2,
        "the schedule took {checkpoints} checkpoints"
    );
    commit_checkpoint(&mut track, &world, 0, 0).await;
    // The crossing counts once: while the total stays above the quota, an
    // entry makes nothing due.
    edit(&mut track, &world, 0, rid, covered, false).await;
    assert_eq!(
        world
            .pull(0, world.store_floor().await)
            .await
            .checkpoint_due,
        0
    );

    let row = world.domain_row(world.domain).await;
    assert!(row.retention_floor < row.checkpoint_seq);
    world.store.maintain().await.expect("relieves");
    let floor = world.store_floor().await;
    let (used, history, _) = world.usage().await;
    assert!(
        used + history <= quota,
        "pruning brought the workspace back within its quota: {used} + {history} of {quota}"
    );
    assert!(floor > 0, "the floor rose");
}

/// Moves usage and history together to `percent` of `quota`, through the
/// history column.
async fn fill_to<B: Backend>(world: &World<B>, quota: u64, percent: u64) {
    let (used, history, _) = world.usage().await;
    let change = (quota * percent / 100) as i64 - (used + history) as i64;
    let (w, d) = (world.workspace, world.domain);
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move { txn.add_usage(w, d, 0, change).await })
        }))
        .await
        .expect("history moved");
}

/// Two more entries: the first notes the pressure, the second is the next
/// entry a crossing makes a checkpoint due at. Returns the due point.
async fn due_after_two_entries<B: Backend>(track: &mut Track, world: &World<B>) -> u64 {
    track.write(world, 0).await;
    track.write(world, 0).await;
    world
        .pull(0, world.store_floor().await)
        .await
        .checkpoint_due
}

/// The trigger has hysteresis (§20): after a crossing, falling only to
/// between 90% and 100% of the quota and passing it again counts nothing,
/// and falling below 90% arms the next crossing.
pub async fn the_pressure_trigger_rearms_only_below_ninety_percent<B: Backend>(store: Store<B>) {
    let quota = 1_000_000;
    let world = quota_world(store, quota, 1_000_000, 1).await;
    let mut track = Track::new(&world).await;
    let crossings = async |world: &World<B>| world.domain_row(world.domain).await.quota_crossings;

    track.write(&world, 0).await;
    assert_eq!(crossings(&world).await, 0);
    fill_to(&world, quota, 101).await;
    assert!(
        due_after_two_entries(&mut track, &world).await > 0,
        "the first crossing makes a checkpoint due"
    );
    assert_eq!(crossings(&world).await, 1);
    commit_checkpoint(&mut track, &world, 0, 0).await;

    // Into the band and back over: the trigger stays spent.
    fill_to(&world, quota, 95).await;
    track.write(&world, 0).await;
    fill_to(&world, quota, 101).await;
    assert_eq!(
        due_after_two_entries(&mut track, &world).await,
        0,
        "no new crossing without falling below 90%"
    );
    assert_eq!(crossings(&world).await, 1);

    // Below 90%, it re-arms, and the next crossing counts.
    fill_to(&world, quota, 89).await;
    track.write(&world, 0).await;
    fill_to(&world, quota, 101).await;
    assert!(
        due_after_two_entries(&mut track, &world).await > 0,
        "the second crossing makes one checkpoint due"
    );
    assert_eq!(crossings(&world).await, 2);
}

/// When usage alone exceeds the quota, history can't bring it back: floors
/// keep to the `retained_bytes` rule (§20).
pub async fn usage_alone_over_quota_moves_no_floor<B: Backend>(store: Store<B>) {
    let world = quota_world(store, 9_000, 4, 1).await;
    let mut track = Track::new(&world).await;
    let quota = 9_000;
    let (used, _, _) = world.usage().await;
    for _ in 0..(quota - used) / operation_length() {
        track.write(&world, 0).await;
        if world
            .pull(0, world.store_floor().await)
            .await
            .checkpoint_due
            > 0
        {
            commit_checkpoint(&mut track, &world, 0, 0).await;
        }
    }
    // Raise usage past the quota directly: only a change of the setting or
    // a racing push can bring a workspace here.
    let (w, d) = (world.workspace, world.domain);
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move { txn.add_usage(w, d, quota as i64, 0).await })
        }))
        .await
        .expect("usage");
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move { txn.set_quota_pressure(w, true).await })
        }))
        .await
        .expect("pressure");
    let before = world.store_floor().await;
    world.store.maintain().await.expect("a pass");
    assert_eq!(world.store_floor().await, before, "no floor moved");
    assert!(world.usage().await.0 > used);
}

/// Replacements at will (§9, §30): a viewer can't skip an epoch that was
/// itself a replacement or that its own account rotated, so it can't loop,
/// and bundles of skipped epochs stop counting toward usage.
pub async fn replacements_stop_at_honest_rotations<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 2).await;
    let viewer = member_device(&mut world).await;
    let readers: BTreeSet<Id> = world.devices.iter().map(Device::id).collect();
    let cards: Vec<Card> = world
        .devices
        .iter()
        .map(|d| d.keys.card(d.session.account))
        .collect();
    let replace = |world: &World<B>, by: usize, epoch, after: &Signed<_>, key: &SecretKey| {
        let next = SecretKey::random().expect("random");
        let record = world.epoch_after(by, epoch, after, key, &next, &readers);
        let mut records = vec![world.bundle(&record, &readers)];
        for card in &cards {
            records.push(RecordRef::Envelope(world.envelope(by, card, epoch, &next)));
        }
        (record, next, records)
    };
    let first = world.first_epoch().await;
    let key = world.epoch_key.clone();

    // A rotation by another account, which the viewer replaces.
    let bad = SecretKey::random().expect("random");
    let second = world.epoch_after(0, 2, &first, &key, &bad, &readers);
    world.inject_epoch(second.clone(), readers.clone()).await;
    let before = world.usage().await.0;
    let (third, third_key, records) = replace(&world, viewer, 3, &first, &key);
    let size = records[0].to_bytes().len() as u64;
    world
        .push_records(viewer, records)
        .await
        .expect("the viewer replaces an epoch of another account");
    assert!(
        world.usage().await.0 < before + size,
        "the skipped bundle no longer counts"
    );

    // Its own epoch, also a replacement: refused, with no bundle stored.
    let (_, _, again) = replace(&world, viewer, 4, &first, &key);
    assert_eq!(
        push_failure(world.push_records(viewer, again).await),
        ("epoch_conflict", 0),
        "a replacement can't skip a replacement"
    );

    // An honest rotation lets a replacement through again, once, by
    // another account's device.
    let fourth_key = SecretKey::random().expect("random");
    let fourth = world.epoch_after(viewer, 4, &third, &third_key, &fourth_key, &readers);
    world.inject_epoch(fourth, readers.clone()).await;
    let (_, _, own) = replace(&world, viewer, 5, &third, &third_key);
    assert_eq!(
        push_failure(world.push_records(viewer, own).await),
        ("epoch_conflict", 0),
        "a viewer can't skip what its own account rotated"
    );
}

/// Closing a workspace reclaims each of its domains the way `delete_domain`
/// reclaims one (§19): usage stops counting, then the batches take the rest.
pub async fn a_closed_workspace_is_reclaimed_domain_by_domain<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let restricted = add_domain(&mut world).await;
    let mut track = Track::new(&world).await;
    track.write(&world, 0).await;
    let (w, d) = (world.workspace, world.domain);
    let done = world
        .store
        .reclaim_workspace(w, vec![d, restricted])
        .await
        .expect("reclaimed");
    assert!(done, "no blob holds a domain back");
    for domain in [d, restricted] {
        assert!(world.retained(domain).await.is_empty());
        let row = world
            .store
            .backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.domain(w, domain).await })
            }))
            .await
            .expect("read");
        assert_eq!(row, None);
    }
}

/// An account belongs to at most 1,000 workspaces (§27): one more creation
/// answers `workspace_limit`, and so does one more `add_member`
/// (`an_account_in_a_thousand_workspaces_joins_no_more`).
pub async fn an_account_belongs_to_at_most_a_thousand_workspaces<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let account = world.account;
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move {
                for i in 0..1_000u32 {
                    let mut id = [7; 16];
                    id[..4].copy_from_slice(&i.to_be_bytes());
                    txn.upsert_member(Id(id), account, 2).await?;
                }
                Ok(())
            })
        }))
        .await
        .expect("memberships");
    let (body, _, _) = super::creation(
        &world.devices[0].keys,
        &world.links,
        &[world.devices[0].keys.card(account)],
    );
    assert_eq!(
        refusal(
            world
                .store
                .create_workspace(world.devices[0].session, body)
                .await
        )
        .code,
        "workspace_limit"
    );
}

/// A log of `last` entries, for the pruning cases of the fixture, whose
/// operations carry no record.
pub(super) async fn log_of<B: Backend>(store: &Store<B>, workspace: Id, domain: Id, last: u64) {
    store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move {
                txn.insert_domain(workspace, domain).await?;
                for _ in 0..last {
                    txn.append_entry(workspace, domain, 1, &[0]).await?;
                }
                Ok(())
            })
        }))
        .await
        .expect("a log");
}

/// Prunes a fixture's log at `floor` under a servable checkpoint whose
/// `floor` field is `lowest`, and returns the sequences of the operations
/// left.
pub(super) async fn prune_at<B: Backend>(
    store: &Store<B>,
    workspace: Id,
    domain: Id,
    floor: u64,
    lowest: u64,
) -> BTreeSet<u64> {
    let last = store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move {
                let row = CheckpointRow {
                    number: 1,
                    digest: hash(&floor.to_be_bytes()),
                    seq: floor + 1,
                    covered_seq: floor,
                    floor: lowest,
                    record: Vec::new(),
                    heads: Vec::new(),
                    log_weight: 0,
                };
                txn.insert_checkpoint(workspace, domain, &row, &[]).await?;
                txn.set_retention_floor(workspace, domain, floor).await?;
                Ok(txn
                    .domain(workspace, domain)
                    .await?
                    .expect("a row")
                    .head_seq)
            })
        }))
        .await
        .expect("a floor");
    store.prune_domain(workspace, domain).await.expect("pruned");
    let mut left = BTreeSet::new();
    for seq in 1..=last {
        let found = store
            .backend()
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.entry_length(workspace, domain, seq).await })
            }))
            .await
            .expect("read");
        if found.is_some() {
            left.insert(seq);
        }
    }
    left
}
