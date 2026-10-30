//! Conformance cases for checkpoints (§17, §26): due points by volume, at
//! a removal and at a replacement, published in pull pages; the server's
//! checks and recomputation of a pushed checkpoint; and the `checkpoint`,
//! `checkpoints/{digest}`, `checkpoint-chain`, `live` and `manifest`
//! routes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use serde_json::Value;

use sync_protocol::codec::{Decode, Nested, Sorted};
use sync_protocol::manifest::{
    Manifest, bucket_of, node_hash, verify_bucket, verify_buckets, verify_descent,
};
use sync_protocol::messages::{CursorCompactedDetail, EntryRecord, Frame, LiveCursor, RecordRef};
use sync_protocol::records::{
    Card, Checkpoint, CheckpointBundle, Epoch, EpochBundle, LinkKind, PolicyChange, PolicyKind,
    Ref, Role, Signed,
};
use sync_protocol::types::{Digest, Id, SecretKey};

use super::{World, push_failure, refusal};
use crate::{Backend, CheckpointInterval, ReadTxn, Session, Store, WriteTxn};

/// What a test wrote: each device's chain, the objects it made and the
/// checkpoints the server accepted.
#[derive(Default)]
struct Track {
    heads: BTreeMap<Id, Ref>,
    rids: Vec<Id>,
    accepted: Vec<Signed<Checkpoint>>,
    epoch: Option<Signed<Epoch>>,
}

impl Track {
    async fn new<B: Backend>(world: &World<B>) -> Self {
        Self {
            epoch: Some(world.first_epoch().await),
            ..Self::default()
        }
    }

    /// Pushes an operation of `device` on a new object and returns its
    /// sequence.
    async fn write<B: Backend>(&mut self, world: &World<B>, device: usize) -> u64 {
        self.write_with(world, device, |_| {}).await
    }

    /// [`Track::write`] after `edit` changes the operation.
    async fn write_with<B: Backend>(
        &mut self,
        world: &World<B>,
        device: usize,
        edit: impl FnOnce(&mut sync_protocol::records::Operation),
    ) -> u64 {
        let id = world.devices[device].id();
        let (counter, previous) = self
            .heads
            .get(&id)
            .map_or((1, Digest::ZERO), |h| (h.counter + 1, h.op_digest));
        let epoch = self.epoch.as_ref().map_or(1, |e| e.body.epoch);
        let op = world.op(device, |op| {
            op.counter = counter;
            op.previous = previous;
            op.epoch = epoch;
            edit(op);
        });
        let receipt = world
            .push(device, std::slice::from_ref(&op))
            .await
            .expect("written");
        let seq = receipt.receipts[0].seq;
        self.heads.insert(
            id,
            Ref {
                device: id,
                counter,
                seq,
                op_digest: op.digest(),
            },
        );
        self.rids.push(op.body.rid);
        seq
    }

    /// The manifest the store's live sets give at `at`.
    async fn manifest<B: Backend>(&self, world: &World<B>, at: u64) -> Manifest {
        let mut leaves = BTreeMap::new();
        for rid in &self.rids {
            let refs: Vec<Ref> = world
                .store
                .live_at(world.workspace, *rid, at)
                .await
                .expect("live")
                .into_iter()
                .map(|(op, _)| Ref {
                    device: op.device,
                    counter: op.counter,
                    seq: op.seq,
                    op_digest: op.digest,
                })
                .collect();
            leaves.insert(*rid, refs);
        }
        Manifest::new(leaves)
    }

    /// An honest checkpoint at `covered_seq`, signed by `signer`, with the
    /// heads at or below it, after `edit` changes it.
    async fn checkpoint<B: Backend>(
        &self,
        world: &World<B>,
        signer: usize,
        covered_seq: u64,
        leave: &[Id],
        edit: impl FnOnce(&mut Checkpoint, &mut Vec<Ref>),
    ) -> CheckpointBundle {
        let manifest = self.manifest(world, covered_seq).await;
        let mut heads: Vec<Ref> = self
            .heads
            .values()
            .filter(|h| h.seq <= covered_seq && !leave.contains(&h.device))
            .cloned()
            .collect();
        let number = self.accepted.len() as u64 + 1;
        let skip = sync_protocol::manifest::skip_number(number);
        let epoch = self.epoch.as_ref().expect("an epoch");
        let mut body = Checkpoint {
            workspace: world.workspace,
            domain: world.domain,
            previous: self.accepted.last().map_or(Digest::ZERO, Signed::digest),
            number,
            skip: if skip == 0 {
                Digest::ZERO
            } else {
                self.accepted[skip as usize - 1].digest()
            },
            covered_seq,
            floor: 0,
            policy_revision: world.head.0,
            policy_head: world.head.1,
            live_objects: manifest.live_objects(),
            manifest_root: manifest.root(),
            heads_digest: Digest::ZERO,
            epoch: epoch.body.epoch,
            epoch_record: epoch.digest(),
            signer: world.devices[signer].id(),
        };
        edit(&mut body, &mut heads);
        if body.heads_digest == Digest::ZERO {
            body.heads_digest = sync_protocol::manifest::heads_digest(&heads);
        }
        CheckpointBundle {
            checkpoint: Nested(Signed::sign(body, world.devices[signer].keys.signing())),
            heads: Sorted::new(heads).expect("sorted heads"),
        }
    }
}

async fn push_checkpoint<B: Backend>(
    world: &World<B>,
    device: usize,
    bundle: &CheckpointBundle,
) -> crate::Result<sync_protocol::messages::PushResponse> {
    world
        .push_records(device, vec![RecordRef::CheckpointBundle(bundle.clone())])
        .await
}

fn interval(entries: u64) -> CheckpointInterval {
    CheckpointInterval {
        entries,
        weight: u64::MAX,
    }
}

/// Due points by volume (§17): a checkpoint falls due at the entry that
/// brings the log to the interval's entries since the latest checkpoint
/// entry, the pull page publishes it, and a checkpoint commits only while
/// one is due and its `covered_seq` reaches the due point. The first one
/// in commits; another naming the same `previous` gets `checkpoint_stale`,
/// and a retry gets the original receipt.
pub async fn checkpoints_fall_due_by_volume<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_checkpoint_interval(interval(6)), 2).await;
    let mut track = Track::new(&world).await;
    // The genesis and epoch 1 are entries 1 and 2.
    for device in [0, 1, 0] {
        track.write(&world, device).await;
    }
    let page = world.pull(0, 0).await;
    assert_eq!((page.checkpoint_due, page.checkpoint_seq), (0, 0));
    let early = track.checkpoint(&world, 0, 5, &[], |_, _| {}).await;
    assert_eq!(
        push_failure(push_checkpoint(&world, 0, &early).await),
        ("checkpoint_not_due", 0),
        "no checkpoint is due yet"
    );
    let session = world.devices[1].session;
    let mut watch = world
        .store
        .realtime_watch(session, world.workspace)
        .await
        .expect("a watch");
    let due = track.write(&world, 1).await;
    assert_eq!(due, 6);
    assert_eq!(world.pull(1, 0).await.checkpoint_due, 6);
    // The realtime channel publishes the due point with the entry (§23).
    let frames = world
        .store
        .realtime_frames(session, world.workspace, &mut watch)
        .await
        .expect("frames");
    assert!(
        frames.iter().any(|f| matches!(
            f,
            Frame::Records {
                checkpoint_due: 6,
                ..
            } | Frame::Notify {
                checkpoint_due: 6,
                ..
            }
        )),
        "a records or notify frame carries the due point: {frames:?}"
    );
    assert_eq!(
        push_failure(push_checkpoint(&world, 0, &early).await),
        ("checkpoint_not_due", 0),
        "covered_seq falls short of the due point"
    );

    let first = track.checkpoint(&world, 0, 6, &[], |_, _| {}).await;
    let receipt = push_checkpoint(&world, 0, &first)
        .await
        .expect("the checkpoint commits");
    assert_eq!(receipt.receipts.len(), 1);
    assert_eq!(receipt.receipts[0].seq, 7);
    assert_eq!(
        receipt.receipts[0].record_digest,
        first.checkpoint.0.digest()
    );
    let retried = push_checkpoint(&world, 0, &first)
        .await
        .expect("a retry returns the original receipt");
    assert_eq!(retried.receipts, receipt.receipts);
    track.accepted.push(first.checkpoint.0.clone());
    let page = world.pull(1, 6).await;
    assert_eq!((page.checkpoint_due, page.checkpoint_seq), (0, 6));
    assert!(matches!(
        &page.entries[0].record,
        EntryRecord::Checkpoint(b) if b == &first
    ));

    // A checkpoint that covers the previous one's entry, while none is due.
    let quiet = track.checkpoint(&world, 1, 7, &[], |_, _| {}).await;
    assert_eq!(
        push_failure(push_checkpoint(&world, 1, &quiet).await),
        ("checkpoint_not_due", 0)
    );
    // An operation names the checkpoint its writer reached.
    let named = world.op(0, |op| {
        op.counter = track.heads[&world.devices[0].id()].counter + 1;
        op.previous = track.heads[&world.devices[0].id()].op_digest;
        op.checkpoint_seq = 6;
        op.checkpoint = Digest([9; 32]);
    });
    assert_eq!(
        push_failure(world.push(0, &[named]).await),
        ("checkpoint_unknown", 0)
    );

    // Two writers race the next due point.
    for device in [0, 1, 0, 1, 0, 1] {
        track.write(&world, device).await;
    }
    let due = world.pull(0, 0).await.checkpoint_due;
    assert_eq!(due, 13, "six entries after the checkpoint entry at 7");
    let one = track.checkpoint(&world, 0, 13, &[], |_, _| {}).await;
    let other = track.checkpoint(&world, 1, 13, &[], |_, _| {}).await;
    push_checkpoint(&world, 0, &one)
        .await
        .expect("the first in commits");
    assert_eq!(
        push_failure(push_checkpoint(&world, 1, &other).await),
        ("checkpoint_stale", 0),
        "the server keeps the first and answers the rest"
    );
    track.accepted.push(one.checkpoint.0.clone());
    let page = world.pull(0, 0).await;
    assert_eq!((page.checkpoint_due, page.checkpoint_seq), (0, 13));
}

/// Weight counts toward the due point as entries do (§17, §19).
pub async fn checkpoints_fall_due_by_weight<B: Backend>(store: Store<B>) {
    let store = store.with_checkpoint_interval(CheckpointInterval {
        entries: u64::MAX,
        weight: 2_000,
    });
    let world = World::new(store, 1).await;
    let mut track = Track::new(&world).await;
    let mut seq = 0;
    while world.pull(0, 0).await.checkpoint_due == 0 {
        seq = track.write(&world, 0).await;
        assert!(seq < 10, "a few 272-byte operations pass 2,000 bytes");
    }
    assert_eq!(world.pull(0, 0).await.checkpoint_due, seq);
}

/// The `server_check` cases of `sync-v2-manifest.json` (§17): the signer,
/// the policy and epoch fields, `previous`, `number` and `skip`, the bounds
/// of `covered_seq` and `floor`, and the recomputed root, `live_objects`
/// and heads. Each case runs on a fresh workspace: an owner's two writer
/// devices and a viewer's device, then one operation from each writer.
pub async fn checkpoint_checks_match_the_fixture<B: Backend>(store: Store<B>) {
    let doc = super::fixture("sync-v2-manifest.json");
    // The viewer's injected policy entry doesn't count toward the interval,
    // so the genesis, epoch 1 and the two operations make four entries.
    let store = Arc::new(store.with_checkpoint_interval(interval(4)));
    let mut ran = 0;
    for case in doc["cases"].as_array().expect("cases") {
        if case["kind"] != "server_check" {
            continue;
        }
        ran += 1;
        let label = case["name"].as_str().expect("a name");
        let mut world = World::on(store.clone(), 2).await;
        add_viewer(&mut world).await;
        let mut track = Track::new(&world).await;
        track.write(&world, 0).await;
        let mut head = track.write(&world, 1).await;
        let mut due = world.pull(0, 0).await.checkpoint_due;
        assert_eq!(due, head, "{label}: due at the second writer's operation");
        let mut previous_entry = 0;
        if case["after_one_checkpoint"].as_bool().expect("a flag") {
            let first = track.checkpoint(&world, 0, due, &[], |_, _| {}).await;
            let receipt = push_checkpoint(&world, 0, &first)
                .await
                .expect("the first checkpoint commits");
            previous_entry = receipt.receipts[0].seq;
            track.accepted.push(first.checkpoint.0.clone());
            due = 0;
            let mut writer = 0;
            while due == 0 {
                head = track.write(&world, writer).await;
                writer = 1 - writer;
                due = world.pull(0, 0).await.checkpoint_due;
            }
        }
        // A bad epoch 2, then a replacement epoch 3 that skips it, which
        // makes a checkpoint due at its entry.
        let mut skipped: Option<Signed<Epoch>> = None;
        if case["after_replacement"].as_bool().expect("a flag") {
            let first = track.epoch.clone().expect("epoch 1");
            let readers: BTreeSet<Id> = world.devices.iter().map(|d| d.id()).collect();
            let bad = SecretKey::random().expect("random");
            let second = world.epoch_after(1, 2, &first, &world.epoch_key, &bad, &readers);
            world.inject_epoch(second.clone(), readers.clone()).await;
            let key = SecretKey::random().expect("random");
            let replacement = world.epoch_after(0, 3, &first, &world.epoch_key, &key, &readers);
            let mut records = vec![RecordRef::EpochBundle(EpochBundle {
                epoch: Nested(replacement.clone()),
                recipients: Sorted::new(readers.iter().copied().collect()).expect("sorted"),
            })];
            for device in &world.devices {
                records.push(RecordRef::Envelope(world.envelope(
                    0,
                    &device.keys.card(device.session.account),
                    3,
                    &key,
                )));
            }
            let receipt = world
                .push_records(0, records)
                .await
                .expect("the replacement commits");
            head = receipt.receipts[0].seq;
            due = world.pull(0, 0).await.checkpoint_due;
            assert_eq!(due, head, "{label}: due at the replacement's entry");
            track.epoch = Some(replacement);
            skipped = Some(second);
        }
        let covered = match case["covered_seq"].as_str().expect("a point") {
            "due" => due,
            "due - 1" => due - 1,
            "head + 1" => head + 1,
            "previous entry - 1" => previous_entry - 1,
            other => panic!("{label}: unknown point {other}"),
        };
        let device = |role: &Value| match role.as_str().expect("a role") {
            "writer" => 0,
            "other writer" => 1,
            "viewer" => 2,
            other => panic!("{label}: unknown device {other}"),
        };
        let signer = device(&case["signer"]);
        let pusher = device(&case["pushed_by"]);
        let edit = case["edit"].as_object().expect("an edit").clone();
        let drop_head = case["drop_head"].as_bool().expect("a flag");
        let bundle = track
            .checkpoint(&world, signer, covered, &[], move |c, heads| {
                for (field, value) in &edit {
                    let digest = || Digest::from_hex(value.as_str().expect("hex")).expect("hex");
                    let add = || value.as_u64().expect("a number");
                    match field.as_str() {
                        "manifest_root" => c.manifest_root = digest(),
                        "heads_digest" => c.heads_digest = digest(),
                        "skip" => c.skip = digest(),
                        "previous" => c.previous = digest(),
                        "policy_head" => c.policy_head = digest(),
                        "epoch_record" => c.epoch_record = digest(),
                        "live_objects" => c.live_objects += add(),
                        "number" => c.number += add(),
                        "policy_revision" => c.policy_revision += add(),
                        "epoch" => c.epoch += add(),
                        "floor" => c.floor += add(),
                        "skipped_epoch" => {
                            let skipped = skipped.as_ref().expect("a skipped epoch");
                            c.epoch = skipped.body.epoch;
                            c.epoch_record = skipped.digest();
                        }
                        other => panic!("unknown field {other}"),
                    }
                }
                if drop_head {
                    heads.pop();
                }
            })
            .await;
        let result = push_checkpoint(&world, pusher, &bundle).await;
        match case["expect"].as_str().expect("a code") {
            "ok" => {
                let receipt = result.unwrap_or_else(|e| panic!("{label}: {e:?}"));
                assert_eq!(
                    receipt.receipts[0].record_digest,
                    bundle.checkpoint.0.digest()
                );
            }
            code => assert_eq!(push_failure(result), (code, 0), "{label}"),
        }
        if signer == 2 {
            // The viewer is active and signs its own push, so the refusal
            // comes from the signer's write access.
            let again = refusal(push_checkpoint(&world, pusher, &bundle).await);
            assert!(
                again.message.contains("can't write the domain"),
                "{label}: {}",
                again.message
            );
        }
    }
    assert!(ran >= 22, "the fixture's server checks ran");
}

/// Adds an account with one device as a viewer of the workspace, signed in
/// as `world.devices[2]`.
async fn add_viewer<B: Backend>(world: &mut World<B>) {
    let (viewer, keys, links) = super::account(1);
    world
        .store
        .append_account_links(links.clone())
        .await
        .expect("the viewer's chain");
    let add = world.change(
        0,
        vec![super::head_of(&world.links), super::head_of(&links)],
        PolicyKind::AddMember {
            account: viewer,
            role: Role::Viewer,
            invitation: Id::random().expect("random"),
        },
    );
    let domain = world.domain;
    world.inject_policy(add, vec![domain], None).await;
    let keys = keys.into_iter().next().expect("one device");
    let session = Session::new(viewer, keys.device());
    let token = sync_protocol::crypto::random_array().expect("random");
    world
        .store
        .create_session(token, session)
        .await
        .expect("a session");
    world.devices.push(super::Device {
        keys,
        session,
        token,
    });
}

/// A checkpoint goes in a push of its own, or last in the push of an
/// approval (§9, §10): there the fresh checkpoint of the workspace domain
/// needs no due point, and its entry lands after the `add_member`'s. After
/// any other record, or before the `add_member`, the push fails with
/// `bad_request`.
pub async fn a_checkpoint_goes_alone_or_last_in_an_approval<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_checkpoint_interval(interval(1_000)), 2).await;
    let mut track = Track::new(&world).await;
    track.write(&world, 0).await;
    let head = track.write(&world, 1).await;
    let fresh = track.checkpoint(&world, 0, head, &[], |_, _| {}).await;
    assert_eq!(
        push_failure(push_checkpoint(&world, 0, &fresh).await),
        ("checkpoint_not_due", 0),
        "alone, a checkpoint waits for its due point"
    );
    let op = world.op(0, |op| {
        op.counter = 2;
        op.previous = track.heads[&world.devices[0].id()].op_digest;
    });
    assert_eq!(
        push_failure(
            world
                .push_records(
                    0,
                    vec![
                        RecordRef::Operation(op),
                        RecordRef::CheckpointBundle(fresh.clone())
                    ]
                )
                .await
        ),
        ("bad_request", 1),
        "a checkpoint after an operation"
    );

    let (invite, invitee) = super::invitations::requested(&world, Role::Editor).await;
    let add = world.change(
        0,
        vec![super::head_of(&world.links), invitee.head()],
        PolicyKind::AddMember {
            account: invitee.account,
            role: Role::Editor,
            invitation: invite.invitation,
        },
    );
    let envelope = world.envelope(0, &invitee.keys.card(invitee.account), 1, &world.epoch_key);
    assert_eq!(
        push_failure(
            world
                .push_records(
                    0,
                    vec![
                        RecordRef::CheckpointBundle(fresh.clone()),
                        RecordRef::Policy(add.clone()),
                        RecordRef::Envelope(envelope.clone()),
                    ]
                )
                .await
        ),
        ("bad_request", 0),
        "a checkpoint before the add_member"
    );
    let pushed = world
        .push_records(
            0,
            vec![
                RecordRef::Policy(add),
                RecordRef::Envelope(envelope),
                RecordRef::CheckpointBundle(fresh.clone()),
            ],
        )
        .await
        .expect("the approval commits with its fresh checkpoint");
    let entry = pushed
        .receipts
        .iter()
        .find(|r| r.record_digest == fresh.checkpoint.0.digest())
        .expect("the checkpoint's receipt");
    assert_eq!(entry.seq, head + 2, "after the add_member's entry");
    let latest = world
        .store
        .checkpoint(world.devices[0].session, world.workspace, world.domain)
        .await
        .expect("the latest checkpoint");
    assert_eq!(latest.bundle.checkpoint.0, fresh.checkpoint.0);
    assert_eq!(latest.seq, head + 2);
}

/// History floors (§9): an approval sets the new member's floor in the
/// workspace domain to the `covered_seq` of the fresh checkpoint its push
/// carries. The member's pulls below it get status 3, a chain fetch of a
/// record at or below it and `live` or `manifest` at an earlier checkpoint
/// get `before_history_floor`, and from the floor on everything serves.
pub async fn history_floors_follow_section_9<B: Backend>(store: Store<B>) {
    use sync_protocol::messages::{PageStatus, PullCursor, PullRequest};

    let mut world = World::new(store.with_checkpoint_interval(interval(2)), 1).await;
    let mut track = Track::new(&world).await;
    track.write(&world, 0).await;
    let early = track.write(&world, 0).await;
    let first = track.checkpoint(&world, 0, early, &[], |_, _| {}).await;
    push_checkpoint(&world, 0, &first)
        .await
        .expect("the first checkpoint");
    track.accepted.push(first.checkpoint.0.clone());
    // One blob whose listing a later state supersedes before the floor,
    // and one whose listing stays live there.
    let listed = |blob: Id| {
        move |op: &mut sync_protocol::records::Operation| {
            op.blobs = Sorted::new(vec![blob]).expect("sorted");
        }
    };
    let (gone_bytes, gone) = super::blobs::blob_for(1_000, world.domain);
    let (kept_bytes, kept) = super::blobs::blob_for(1_000, world.domain);
    let owner_session = world.devices[0].session;
    for (bytes, creation) in [(&gone_bytes, &gone), (&kept_bytes, &kept)] {
        world
            .store
            .create_blob(owner_session, world.workspace, *creation)
            .await
            .expect("created");
        super::blobs::upload_all(&world, 0, creation.blob, bytes).await;
    }
    track.write_with(&world, 0, listed(gone.blob)).await;
    let listing = track.heads[&world.devices[0].id()].clone();
    let rid = *track.rids.last().expect("the object");
    track
        .write_with(&world, 0, |op| {
            op.rid = rid;
            op.supersedes = Sorted::new(vec![sync_protocol::records::Cover {
                device: listing.device,
                counter: listing.counter,
            }])
            .expect("sorted");
        })
        .await;
    track.write_with(&world, 0, listed(kept.blob)).await;
    let head = track.write(&world, 0).await;
    let fresh = track.checkpoint(&world, 0, head, &[], |_, _| {}).await;

    let (invite, invitee) = super::invitations::requested(&world, Role::Viewer).await;
    let add = world.change(
        0,
        vec![super::head_of(&world.links), invitee.head()],
        PolicyKind::AddMember {
            account: invitee.account,
            role: Role::Viewer,
            invitation: invite.invitation,
        },
    );
    let envelope = world.envelope(0, &invitee.keys.card(invitee.account), 1, &world.epoch_key);
    let pushed = world
        .push_records(
            0,
            vec![
                RecordRef::Policy(add.clone()),
                RecordRef::Envelope(envelope),
                RecordRef::CheckpointBundle(fresh.clone()),
            ],
        )
        .await
        .expect("the approval commits");
    world.head = (pushed.policy_revision, add.digest());
    let floor = head;
    let member = invitee.session;
    let pull = |after| PullRequest {
        cursors: sync_protocol::codec::List(vec![PullCursor {
            domain: world.domain,
            after,
        }]),
        wait_seconds: 0,
    };
    for after in [0, early, floor - 1] {
        let page = &world
            .store
            .pull(member, world.workspace, pull(after))
            .await
            .expect("pulled")
            .pages
            .0[0];
        assert_eq!(page.status, PageStatus::BeforeHistoryFloor, "after {after}");
        assert!(page.entries.is_empty());
    }
    let page = &world
        .store
        .pull(member, world.workspace, pull(floor))
        .await
        .expect("pulled")
        .pages
        .0[0];
    assert_eq!(page.status, PageStatus::Ok);
    assert!(
        page.entries.iter().all(|e| e.seq > floor),
        "only what came after the floor"
    );
    assert!(
        !page.entries.is_empty(),
        "the add_member's entry and the checkpoint"
    );
    // The owner reads its history as before.
    let page = &world
        .store
        .pull(world.devices[0].session, world.workspace, pull(0))
        .await
        .expect("pulled")
        .pages
        .0[0];
    assert_eq!(page.status, PageStatus::Ok);

    let owner = world.devices[0].id();
    assert_eq!(
        refusal(
            world
                .store
                .chain_record(member, world.workspace, world.domain, owner, 1)
                .await
        )
        .code,
        "before_history_floor"
    );
    world
        .store
        .chain_record(
            world.devices[0].session,
            world.workspace,
            world.domain,
            owner,
            1,
        )
        .await
        .expect("the owner fetches its own chain");
    assert_eq!(
        refusal(
            world
                .store
                .live(member, world.workspace, world.domain, early, None, None)
                .await
        )
        .code,
        "before_history_floor",
        "live at an earlier checkpoint"
    );
    assert_eq!(
        refusal(
            world
                .store
                .manifest_buckets(member, world.workspace, world.domain, early)
                .await
        )
        .code,
        "before_history_floor"
    );
    let live = world
        .store
        .live(member, world.workspace, world.domain, floor, None, None)
        .await
        .expect("live at the fresh checkpoint");
    assert_eq!(
        live.entries.len(),
        5,
        "every operation live there, whatever its sequence"
    );
    world
        .store
        .manifest_buckets(member, world.workspace, world.domain, floor)
        .await
        .expect("the manifest at the fresh checkpoint");

    // A blob serves while a listing sits above the floor or is live at a
    // checkpoint the device reads.
    assert_eq!(
        refusal(
            world
                .store
                .blob_download(member, world.workspace, gone.blob)
                .await
        )
        .code,
        "before_history_floor"
    );
    world
        .store
        .blob_download(owner_session, world.workspace, gone.blob)
        .await
        .expect("the owner has no floor");
    world
        .store
        .blob_download(member, world.workspace, kept.blob)
        .await
        .expect("its listing is live at the checkpoint");
    let (new_bytes, new_blob) = super::blobs::blob_for(1_000, world.domain);
    world
        .store
        .create_blob(owner_session, world.workspace, new_blob)
        .await
        .expect("created");
    super::blobs::upload_all(&world, 0, new_blob.blob, &new_bytes).await;
    track.write_with(&world, 0, listed(new_blob.blob)).await;
    world
        .store
        .blob_download(member, world.workspace, new_blob.blob)
        .await
        .expect("listed above the floor");

    // A stale realtime watch forwards nothing at or below the floor.
    let mut watch = world
        .store
        .realtime_watch(member, world.workspace)
        .await
        .expect("a watch");
    watch.heads.insert(world.domain, 0);
    let frames = world
        .store
        .realtime_frames(member, world.workspace, &mut watch)
        .await
        .expect("frames");
    let mut seen = 0;
    for frame in frames {
        if let Frame::Records {
            previous_seq,
            entries,
            ..
        } = frame
        {
            assert_eq!(previous_seq, floor);
            assert!(entries.iter().all(|e| e.seq > floor));
            seen += entries.len();
        }
    }
    assert!(seen > 0, "what came after the floor still arrives");
}

/// Racing checkpoints (§17): writers that each sign the due checkpoint and
/// push at once leave exactly one committed, and every other one gets
/// `checkpoint_stale`, never `manifest_mismatch`, since the server checks
/// `previous` again under the domain's lock before it compares the rest.
pub async fn racing_checkpoints_keep_the_first<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_checkpoint_interval(interval(6)), 4).await;
    let mut track = Track::new(&world).await;
    for device in 0..4 {
        track.write(&world, device).await;
    }
    let due = world.pull(0, 0).await.checkpoint_due;
    assert_eq!(due, 6);
    let mut bundles = Vec::new();
    for device in 0..4 {
        bundles.push(track.checkpoint(&world, device, due, &[], |_, _| {}).await);
    }
    let (a, b, c, d) = tokio::join!(
        push_checkpoint(&world, 0, &bundles[0]),
        push_checkpoint(&world, 1, &bundles[1]),
        push_checkpoint(&world, 2, &bundles[2]),
        push_checkpoint(&world, 3, &bundles[3]),
    );
    let mut committed = 0;
    for result in [a, b, c, d] {
        match result {
            Ok(_) => committed += 1,
            failed => assert_eq!(push_failure(failed), ("checkpoint_stale", 0)),
        }
    }
    assert_eq!(committed, 1, "the server keeps the first");
    let page = world.pull(0, 0).await;
    assert_eq!((page.checkpoint_due, page.checkpoint_seq), (0, 6));
}

/// Due points and heads around a removal (§17): the policy entry that
/// ends a writer's access makes a checkpoint due, the first checkpoint
/// that covers it still lists the removed device's final head, and the
/// next one leaves it out, since a link removed its card at or below the
/// previous checkpoint's `policy_revision`.
pub async fn removals_make_checkpoints_due_and_leave_the_heads<B: Backend>(store: Store<B>) {
    let mut world = World::new(store.with_checkpoint_interval(interval(4)), 3).await;
    let mut track = Track::new(&world).await;
    track.write(&world, 2).await;
    track.write(&world, 0).await;
    let first = track.checkpoint(&world, 0, 4, &[], |_, _| {}).await;
    push_checkpoint(&world, 0, &first)
        .await
        .expect("checkpoint 1 commits");
    track.accepted.push(first.checkpoint.0.clone());

    let removed = world.devices[2].id();
    let head = world
        .append_link(
            0,
            LinkKind::Remove {
                device: removed,
                keep: Sorted::default(),
            },
        )
        .await;
    let change = world.change(0, vec![head], PolicyKind::AccountHeads);
    let receipt = world
        .push_records(0, vec![RecordRef::Policy(change.clone())])
        .await
        .expect("the removal's head");
    world.head = (2, change.digest());
    let removal_seq = receipt.receipts[0].seq;
    assert_eq!(removal_seq, 6);
    assert_eq!(
        world.pull(0, 0).await.checkpoint_due,
        removal_seq,
        "the removal's entry makes a checkpoint due"
    );

    let second = track.checkpoint(&world, 0, 6, &[], |_, _| {}).await;
    assert_eq!(
        push_failure(push_checkpoint(&world, 0, &second).await),
        ("rotation_pending", 0),
        "the domain takes no record until the rotation commits"
    );

    // The rotation the removal requires.
    let readers: BTreeSet<Id> = [world.devices[0].id(), world.devices[1].id()]
        .into_iter()
        .collect();
    let key = SecretKey::random().expect("random");
    let first_epoch = track.epoch.clone().expect("epoch 1");
    let rotated = world.epoch_after(0, 2, &first_epoch, &world.epoch_key, &key, &readers);
    let cards: Vec<Card> = world.devices[..2]
        .iter()
        .map(|d| d.keys.card(world.account))
        .collect();
    let mut records = vec![RecordRef::EpochBundle(EpochBundle {
        epoch: Nested(rotated.clone()),
        recipients: Sorted::new(readers.iter().copied().collect()).expect("sorted"),
    })];
    for card in &cards {
        records.push(RecordRef::Envelope(world.envelope(0, card, 2, &key)));
    }
    world
        .push_records(0, records)
        .await
        .expect("the rotation commits");
    let without = track.checkpoint(&world, 0, 6, &[removed], |_, _| {}).await;
    assert_eq!(
        push_failure(push_checkpoint(&world, 0, &without).await),
        ("manifest_mismatch", 0),
        "the first checkpoint covering the removal lists its final head"
    );
    push_checkpoint(&world, 0, &second)
        .await
        .expect("checkpoint 2 commits");
    track.accepted.push(second.checkpoint.0.clone());

    // Writes under the new epoch, to the next due point.
    track.epoch = Some(rotated);
    track.write(&world, 0).await;
    track.write(&world, 1).await;
    track.write(&world, 0).await;
    let due = track.write(&world, 1).await;
    assert_eq!(world.pull(0, 0).await.checkpoint_due, due);
    let with = track.checkpoint(&world, 1, due, &[], |_, _| {}).await;
    assert_eq!(
        push_failure(push_checkpoint(&world, 1, &with).await),
        ("manifest_mismatch", 0),
        "a removed card leaves once the previous checkpoint's revision covers it"
    );
    let third = track
        .checkpoint(&world, 1, due, &[removed], |_, _| {})
        .await;
    push_checkpoint(&world, 1, &third)
        .await
        .expect("checkpoint 3 commits without the removed device");
}

/// A replacement makes a checkpoint due at its entry (§17), since the
/// latest checkpoint may name the epoch it skips.
pub async fn replacements_make_checkpoints_due<B: Backend>(store: Store<B>) {
    let world = World::new(store, 2).await;
    let first = world.first_epoch().await;
    let readers: BTreeSet<Id> = world.devices.iter().map(|d| d.id()).collect();
    let bad = SecretKey::random().expect("random");
    let second = world.epoch_after(1, 2, &first, &world.epoch_key, &bad, &readers);
    world.inject_epoch(second, readers.clone()).await;
    assert_eq!(world.pull(0, 0).await.checkpoint_due, 0);
    let key = SecretKey::random().expect("random");
    let replacement = world.epoch_after(0, 3, &first, &world.epoch_key, &key, &readers);
    let mut records = vec![RecordRef::EpochBundle(EpochBundle {
        epoch: Nested(replacement),
        recipients: Sorted::new(readers.iter().copied().collect()).expect("sorted"),
    })];
    for device in &world.devices {
        records.push(RecordRef::Envelope(world.envelope(
            0,
            &device.keys.card(world.account),
            3,
            &key,
        )));
    }
    let receipt = world
        .push_records(0, records)
        .await
        .expect("the replacement commits");
    assert_eq!(
        world.pull(0, 0).await.checkpoint_due,
        receipt.receipts[0].seq
    );
}

/// The checkpoint routes (§17, §26): the latest bundle with its entry's
/// sequence, a checkpoint by digest, the descent walk through `skip` and
/// `previous`, the operations live at a checkpoint and its manifest, all
/// of which check against the checkpoint's root.
pub async fn checkpoint_routes_serve_the_chain_and_manifest<B: Backend>(store: Store<B>) {
    let world = World::new(store.with_checkpoint_interval(interval(3)), 2).await;
    let session = world.devices[1].session;
    let (workspace, domain) = (world.workspace, world.domain);
    assert_eq!(
        refusal(world.store.checkpoint(session, workspace, domain).await).code,
        "not_found"
    );
    let mut track = Track::new(&world).await;
    let mut seqs = Vec::new();
    for _ in 0..6 {
        let mut due = 0;
        while due == 0 {
            track.write(&world, 0).await;
            due = world.pull(0, 0).await.checkpoint_due;
        }
        let bundle = track.checkpoint(&world, 0, due, &[], |_, _| {}).await;
        let receipt = push_checkpoint(&world, 0, &bundle).await.expect("commits");
        seqs.push(receipt.receipts[0].seq);
        track.accepted.push(bundle.checkpoint.0.clone());
    }
    let latest = track.accepted.last().expect("six").clone();
    let response = world
        .store
        .checkpoint(session, workspace, domain)
        .await
        .expect("the latest");
    assert_eq!(response.seq, seqs[5]);
    assert_eq!(response.bundle.checkpoint.0, latest);

    let second = &track.accepted[1];
    let entry = world
        .store
        .checkpoint_by_digest(session, workspace, domain, second.digest())
        .await
        .expect("by digest");
    assert_eq!(entry.seq, seqs[1]);
    assert!(matches!(&entry.record, EntryRecord::Checkpoint(b) if &b.checkpoint.0 == second));
    assert_eq!(
        refusal(
            world
                .store
                .checkpoint_by_digest(session, workspace, domain, Digest([7; 32]))
                .await
        )
        .code,
        "not_found"
    );

    let walk = world
        .store
        .checkpoint_chain(
            session,
            workspace,
            domain,
            latest.digest(),
            track.accepted[0].digest(),
        )
        .await
        .expect("the walk");
    let walk: Vec<Signed<Checkpoint>> = walk.checkpoints.0.into_iter().map(|n| n.0).collect();
    let numbers: Vec<u64> = walk.iter().map(|c| c.body.number).collect();
    assert_eq!(numbers, vec![6, 4, 3, 2, 1]);
    verify_descent(&walk, &track.accepted[0].digest()).expect("descends from the anchor");

    let at = latest.body.covered_seq;
    let live = world
        .store
        .live(session, workspace, domain, at, None, None)
        .await
        .expect("live");
    assert_eq!(live.at, at);
    assert!(live.next.is_none());
    assert_eq!(live.entries.len() as u64, latest.body.live_objects);
    let mut rids: Vec<Id> = live
        .entries
        .iter()
        .map(|e| match &e.record {
            EntryRecord::Operation(op) => op.body.rid,
            other => panic!("an operation, got {other:?}"),
        })
        .collect();
    let mut sorted = rids.clone();
    sorted.sort();
    assert_eq!(rids, sorted, "ordered by rid");
    let one = world
        .store
        .live(session, workspace, domain, at, Some(rids[2]), None)
        .await
        .expect("live by rid");
    assert_eq!(one.entries.len(), 1);
    let cursor = LiveCursor {
        rid: rids[2],
        device: world.devices[0].id(),
        counter: u64::MAX,
    };
    let after = world
        .store
        .live(session, workspace, domain, at, None, Some(cursor))
        .await
        .expect("live after a cursor");
    assert_eq!(after.entries.len(), rids.len() - 3);
    let compacted = refusal(
        world
            .store
            .live(session, workspace, domain, at - 1, None, None)
            .await,
    );
    assert_eq!(compacted.code, "cursor_compacted");
    let detail = CursorCompactedDetail::from_bytes(&compacted.detail.expect("a detail"))
        .expect("its detail");
    assert_eq!(detail.checkpoint_covered_seq, at);

    let buckets = world
        .store
        .manifest_buckets(session, workspace, domain, at)
        .await
        .expect("buckets");
    verify_buckets(&buckets.buckets, &latest.body.manifest_root).expect("the root");
    let top = world
        .store
        .manifest_node(session, workspace, domain, at, 0, 0)
        .await
        .expect("the root's children");
    assert_eq!(node_hash(0, 0, &top.children), latest.body.manifest_root);
    rids.sort();
    let index = bucket_of(&rids[0]);
    let leaves = world
        .store
        .manifest_leaves(session, workspace, domain, at, index, None)
        .await
        .expect("a bucket's leaves");
    assert!(leaves.next.is_none());
    let grouped: Vec<(Id, Vec<Ref>)> = leaves
        .leaves
        .iter()
        .map(|l| (l.rid, vec![l.op.clone()]))
        .collect();
    verify_bucket(index, &grouped, &buckets.buckets[usize::from(index)])
        .expect("the leaves hash to the bucket");
}

/// Writes operations from `world.devices[0]` and commits `count` honest
/// checkpoints, each at its due point, for tests at other seams. Returns
/// each checkpoint with its entry's sequence.
pub async fn commit_checkpoints<B: Backend>(
    world: &World<B>,
    count: usize,
) -> Vec<(Signed<Checkpoint>, u64)> {
    let mut track = Track::new(world).await;
    let mut committed = Vec::new();
    for _ in 0..count {
        let mut due = 0;
        while due == 0 {
            track.write(world, 0).await;
            due = world.pull(0, 0).await.checkpoint_due;
        }
        let bundle = track.checkpoint(world, 0, due, &[], |_, _| {}).await;
        let receipt = push_checkpoint(world, 0, &bundle).await.expect("commits");
        track.accepted.push(bundle.checkpoint.0.clone());
        committed.push((bundle.checkpoint.0, receipt.receipts[0].seq));
    }
    committed
}

/// The heads rule on the server (§17), from the `server_heads` cases of
/// `sync-v2-manifest.json`: a demoted device and one whose account
/// `remove_member` removed stay in the heads, while cards that a recorded
/// head retired leave them once the previous checkpoint's
/// `policy_revision` covers that head, as after an `accept_reset`, an
/// `add_member` after a reset, or a removal that records a link removing a
/// card. The first checkpoint covering the events still lists every
/// device.
pub async fn checkpoint_heads_follow_the_fixture<B: Backend>(store: Store<B>) {
    let doc = super::fixture("sync-v2-manifest.json");
    let store = Arc::new(store.with_checkpoint_interval(interval(4)));
    let mut ran = 0;
    for case in doc["cases"].as_array().expect("cases") {
        if case["kind"] != "server_heads" {
            continue;
        }
        ran += 1;
        let label = case["name"].as_str().expect("a name");
        let mut world = World::on(store.clone(), 2).await;
        // The member's laptop and phone, as devices 2 and 3.
        let (member, keys, links) = super::account(2);
        world
            .store
            .append_account_links(links.clone())
            .await
            .expect("the member's chain");
        let add = world.change(
            0,
            vec![super::head_of(&world.links), super::head_of(&links)],
            PolicyKind::AddMember {
                account: member,
                role: Role::Editor,
                invitation: Id::random().expect("random"),
            },
        );
        let domain = world.domain;
        world.inject_policy(add, vec![domain], None).await;
        for keys in keys {
            let session = Session::new(member, keys.device());
            let token = sync_protocol::crypto::random_array().expect("random");
            world
                .store
                .create_session(token, session)
                .await
                .expect("a session");
            world.devices.push(super::Device {
                keys,
                session,
                token,
            });
        }
        let device_of = |name: &Value| match name.as_str().expect("a device") {
            "member laptop" => 2,
            "member phone" => 3,
            other => panic!("{label}: unknown device {other}"),
        };
        let mut track = Track::new(&world).await;
        track.write(&world, 2).await;
        let head = track.write(&world, 3).await;
        let due = world.pull(0, 0).await.checkpoint_due;
        assert_eq!(due, head, "{label}: due at the phone's operation");
        let first = track.checkpoint(&world, 0, due, &[], |_, _| {}).await;
        push_checkpoint(&world, 0, &first)
            .await
            .unwrap_or_else(|e| panic!("{label}: checkpoint 1: {e:?}"));
        track.accepted.push(first.checkpoint.0.clone());

        // The member's current chain and the cards that read the domain.
        let owner_cards: Vec<Card> = world.devices[..2]
            .iter()
            .map(|d| d.keys.card(world.account))
            .collect();
        let mut member_links = links;
        let mut member_keys: Vec<sync_key_envelope::v2::DeviceKeys> =
            world.devices[2..].iter().map(|d| d.keys.clone()).collect();
        let mut readers: Vec<Card> = member_keys.iter().map(|k| k.card(member)).collect();
        let mut reduced = false;
        for event in case["events"].as_array().expect("events") {
            let owner_head = super::head_of(&world.links);
            match event.as_str().expect("an event") {
                "set_role_viewer" => {
                    let change = world.change(
                        0,
                        vec![owner_head],
                        PolicyKind::SetRole {
                            account: member,
                            role: Role::Viewer,
                        },
                    );
                    inject_counted(&mut world, change).await;
                }
                "remove_member" => {
                    let change = world.change(
                        0,
                        vec![owner_head],
                        PolicyKind::RemoveMember { account: member },
                    );
                    inject_counted(&mut world, change).await;
                    readers.clear();
                    reduced = true;
                }
                "reset" => {
                    let keys = sync_key_envelope::v2::DeviceKeys::generate().expect("random");
                    let genesis = sync_key_envelope::v2::genesis_link(&keys, member);
                    let row = crate::LinkRow {
                        genesis: genesis.digest(),
                        seqno: 1,
                        link: sync_protocol::codec::Encode::to_bytes(&genesis),
                    };
                    let new = genesis.digest();
                    world
                        .store
                        .backend()
                        .write(Box::new(move |txn: &mut B::Write| {
                            Box::pin(async move {
                                txn.insert_account_link(member, &row, new).await?;
                                txn.set_current_genesis(member, new).await
                            })
                        }))
                        .await
                        .expect("reset");
                    member_links = vec![genesis];
                    member_keys = vec![keys];
                }
                "accept_reset" => {
                    let change = world.change(
                        0,
                        vec![owner_head, super::head_of(&member_links)],
                        PolicyKind::AcceptReset { account: member },
                    );
                    inject_counted(&mut world, change).await;
                    readers = member_keys.iter().map(|k| k.card(member)).collect();
                    reduced = true;
                }
                "add_member" => {
                    let change = world.change(
                        0,
                        vec![owner_head, super::head_of(&member_links)],
                        PolicyKind::AddMember {
                            account: member,
                            role: Role::Editor,
                            invitation: Id::random().expect("random"),
                        },
                    );
                    inject_counted(&mut world, change).await;
                    readers = member_keys.iter().map(|k| k.card(member)).collect();
                }
                "remove_phone_link" => {
                    let phone = world.devices[3].id();
                    let link = sync_key_envelope::v2::next_link(
                        &world.devices[2].keys,
                        &super::head_of(&member_links),
                        LinkKind::Remove {
                            device: phone,
                            keep: Sorted::default(),
                        },
                    );
                    world
                        .store
                        .append_account_links(vec![link.clone()])
                        .await
                        .expect("the phone's removal");
                    member_links.push(link);
                }
                "remove_own_account" => {
                    let change = world.change(
                        2,
                        vec![super::head_of(&member_links)],
                        PolicyKind::RemoveMember { account: member },
                    );
                    inject_counted(&mut world, change).await;
                    readers.clear();
                    reduced = true;
                }
                other => panic!("{label}: unknown event {other}"),
            }
        }
        if reduced {
            let cards: Vec<Card> = owner_cards.iter().chain(&readers).cloned().collect();
            let recipients: BTreeSet<Id> = cards.iter().map(|c| c.device).collect();
            let key = SecretKey::random().expect("random");
            let previous = track.epoch.clone().expect("epoch 1");
            let rotated = world.epoch_after(0, 2, &previous, &world.epoch_key, &key, &recipients);
            let mut records = vec![RecordRef::EpochBundle(EpochBundle {
                epoch: Nested(rotated.clone()),
                recipients: Sorted::new(recipients.iter().copied().collect()).expect("sorted"),
            })];
            for card in &cards {
                records.push(RecordRef::Envelope(world.envelope(0, card, 2, &key)));
            }
            world
                .push_records(0, records)
                .await
                .unwrap_or_else(|e| panic!("{label}: the rotation: {e:?}"));
            track.epoch = Some(rotated);
        }

        // The first checkpoint covering the events lists every device.
        let page = world.pull(0, 0).await;
        assert_ne!(page.checkpoint_due, 0, "{label}: the events make one due");
        let covered = page.head;
        let laptop = world.devices[2].id();
        let without = track
            .checkpoint(&world, 0, covered, &[laptop], |_, _| {})
            .await;
        assert_eq!(
            push_failure(push_checkpoint(&world, 0, &without).await),
            ("manifest_mismatch", 0),
            "{label}: the first checkpoint covering the events lists every device"
        );
        let second = track.checkpoint(&world, 0, covered, &[], |_, _| {}).await;
        push_checkpoint(&world, 0, &second)
            .await
            .unwrap_or_else(|e| panic!("{label}: checkpoint 2: {e:?}"));
        track.accepted.push(second.checkpoint.0.clone());

        // The owner's devices write to the next due point.
        let mut writer = 0;
        let mut due = 0;
        while due == 0 {
            track.write(&world, writer).await;
            writer = 1 - writer;
            due = world.pull(0, 0).await.checkpoint_due;
        }
        let leave: Vec<Id> = case["leave"]
            .as_array()
            .expect("a list")
            .iter()
            .map(|d| world.devices[device_of(d)].id())
            .collect();
        let wrong: Vec<Id> = if leave.is_empty() {
            vec![laptop]
        } else {
            Vec::new()
        };
        let refused = track.checkpoint(&world, 0, due, &wrong, |_, _| {}).await;
        assert_eq!(
            push_failure(push_checkpoint(&world, 0, &refused).await),
            ("manifest_mismatch", 0),
            "{label}: the heads rule decides who leaves"
        );
        let third = track.checkpoint(&world, 0, due, &leave, |_, _| {}).await;
        push_checkpoint(&world, 0, &third)
            .await
            .unwrap_or_else(|e| panic!("{label}: checkpoint 3: {e:?}"));
    }
    assert!(ran >= 6, "the fixture's heads cases ran");
}

/// Commits a policy change straight into the store, as
/// [`World::inject_policy`] does, with its workspace-domain entry counted
/// toward the due point the way a pushed change's is (§17): due at its
/// entry when it ends a writer's access.
async fn inject_counted<B: Backend>(world: &mut World<B>, change: Signed<PolicyChange>) {
    let workspace = world.workspace;
    let domain = world.domain;
    let head = (change.body.revision, change.digest());
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move {
                txn.lock(&crate::LockPlan {
                    workspace: Some(workspace),
                    policy_exclusive: true,
                    key_sequence: false,
                    domains: vec![domain],
                    blobs: Vec::new(),
                })
                .await?;
                let mut changes = txn
                    .policy_changes(workspace, 0)
                    .await?
                    .iter()
                    .map(|bytes| {
                        Signed::<PolicyChange>::from_bytes(bytes).map_err(crate::Error::internal)
                    })
                    .collect::<crate::Result<Vec<_>>>()?;
                changes.push(change.clone());
                let accounts: BTreeSet<Id> = changes
                    .iter()
                    .flat_map(|c| c.body.heads.iter().map(|h| h.account))
                    .collect();
                let chains = crate::policy::account_chains(txn, accounts).await?;
                let policy = sync_protocol::policy::PolicyChain::verify(&changes, &chains)
                    .map_err(|e| crate::Error::internal(e.to_string()))?;
                let revision = change.body.revision;
                let prev = policy.at(revision - 1).expect("the previous revision");
                let next = policy.at(revision).expect("this revision");
                crate::policy::store_change(txn, workspace, &change).await?;
                txn.set_policy_head(workspace, revision, change.digest())
                    .await?;
                let entry = sync_protocol::messages::PolicyEntry {
                    revision,
                    head: change.digest(),
                };
                crate::checkpoints::append_policy_entry(
                    txn,
                    interval(4),
                    workspace,
                    domain,
                    &entry,
                    Some(prev),
                    next,
                )
                .await?;
                Ok(())
            })
        }))
        .await
        .expect("policy change injected");
    world.head = head;
}
