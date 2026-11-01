//! Conformance cases for blob rows (§21): tus creation, segments, digest
//! checks, the push checks for listed blobs and who may download.

use std::collections::BTreeSet;
use std::sync::Arc;

use sync_protocol::blob::{SEGMENT_LEN, encrypt_blob};
use sync_protocol::crypto::hash;
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::records::{PolicyKind, Signed};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};

use super::{World, head_of, manager, push_failure, refusal};
use crate::{Backend, BlobCreation, BlobRow, Recorded, Store};

/// A blob's ciphertext and the creation that names it, for a file of
/// `file_len` bytes.
pub fn blob_for(file_len: usize, domain: Id) -> (Vec<u8>, BlobCreation) {
    let file: Vec<u8> = (0..file_len).map(|i| (i % 251) as u8).collect();
    let blob = Id::random().expect("random");
    let (ciphertext, blob_ref) =
        encrypt_blob(&file, blob, SecretKey::random().expect("random")).expect("encrypts");
    let creation = BlobCreation {
        blob,
        length: blob_ref.ciphertext_length,
        digest: blob_ref.ciphertext_digest,
        domain,
    };
    (ciphertext, creation)
}

/// Uploads every segment of `ciphertext` as `device`.
pub async fn upload_all<B: Backend>(
    world: &World<B>,
    device: usize,
    blob: Id,
    ciphertext: &[u8],
) -> Recorded {
    let mut last = None;
    for (index, segment) in ciphertext.chunks(SEGMENT_LEN).enumerate() {
        last = Some(
            world
                .store
                .record_segment(
                    world.devices[device].session,
                    world.workspace,
                    blob,
                    (index * SEGMENT_LEN) as u64,
                    segment.len() as u64,
                    hash(segment),
                )
                .await
                .expect("segment recorded"),
        );
    }
    last.expect("one segment at least")
}

/// Adds a domain that only the world's account manages, through an
/// injected policy change, and returns it.
pub async fn add_domain<B: Backend>(world: &mut World<B>) -> Id {
    let restricted = Id::random().expect("random");
    let change = world.change(
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
                workspace: world.workspace,
                domain: restricted,
                epoch: 1,
                previous: Digest::ZERO,
                policy_revision: world.head.0 + 1,
                policy_head: change.digest(),
                rotating_device: creator.id(),
                recipients: BTreeSet::from([creator.id()]),
            },
            &SecretKey::random().expect("random"),
            None,
            Nonce::ZERO,
        ),
        creator.keys.signing(),
    );
    let domain = world.domain;
    world
        .inject_policy(change, vec![domain], Some((restricted, epoch)))
        .await;
    restricted
}

/// tus creation checks the length, names its domain, returns the original
/// row to a retry and `id_conflict` to another creation under the ID; each
/// `PATCH` appends exactly the next segment, and `HEAD` reads the offset.
pub async fn blob_uploads_follow_tus<B: Backend>(store: Store<B>) {
    let world = World::new(store, 2).await;
    let session = world.devices[0].session;
    let (ciphertext, creation) = blob_for(1_500_000, world.domain);
    let store = &world.store;
    let w = world.workspace;

    let odd = BlobCreation {
        length: creation.length + 1,
        ..creation
    };
    assert_eq!(
        refusal(store.create_blob(session, w, odd).await).code,
        "padding_invalid"
    );
    let huge = BlobCreation {
        length: 1 << 40,
        ..creation
    };
    assert_eq!(
        refusal(store.create_blob(session, w, huge).await).code,
        "too_large"
    );
    let elsewhere = BlobCreation {
        domain: Id::random().expect("random"),
        ..creation
    };
    assert_eq!(
        refusal(store.create_blob(session, w, elsewhere).await).code,
        "not_found"
    );

    let row = store
        .create_blob(session, w, creation)
        .await
        .expect("created");
    assert_eq!(
        row,
        BlobRow {
            blob: creation.blob,
            domain: world.domain,
            device: world.devices[0].id(),
            length: creation.length,
            digest: creation.digest,
            offset: 0,
            complete: false,
            delete_after: Some(store.clock().now() + crate::UPLOAD_TTL),
            marked: false,
            live_listings: 0,
        }
    );
    assert_eq!(
        store
            .create_blob(session, w, creation)
            .await
            .expect("retry"),
        row,
        "a retried creation returns the original row"
    );
    let other = BlobCreation {
        digest: Digest([7; 32]),
        ..creation
    };
    assert_eq!(
        refusal(store.create_blob(session, w, other).await).code,
        "id_conflict"
    );
    assert_eq!(
        refusal(
            store
                .create_blob(world.devices[1].session, w, creation)
                .await
        )
        .code,
        "id_conflict",
        "blob IDs are unique within a workspace"
    );

    // Another device sees no upload.
    assert_eq!(
        refusal(
            store
                .blob_upload(world.devices[1].session, w, creation.blob)
                .await
        )
        .code,
        "not_found"
    );

    let first = &ciphertext[..SEGMENT_LEN];
    let seg = SEGMENT_LEN as u64;
    // A short segment that isn't the last, and an offset past the stored one.
    assert_eq!(
        refusal(
            store
                .record_segment(session, w, creation.blob, 0, 1000, hash(&first[..1000]))
                .await
        )
        .code,
        "bad_request"
    );
    assert_eq!(
        refusal(
            store
                .record_segment(session, w, creation.blob, seg, seg, hash(first))
                .await
        )
        .code,
        "blob_offset_conflict"
    );
    let recorded = store
        .record_segment(session, w, creation.blob, 0, seg, hash(first))
        .await
        .expect("first segment");
    assert!(matches!(recorded, Recorded::Stored(ref r) if r.offset == seg && !r.complete));
    let upload = store
        .blob_upload(session, w, creation.blob)
        .await
        .expect("head");
    assert_eq!(
        (upload.row.offset, upload.segments),
        (seg, vec![hash(first)])
    );
    // A retried PATCH at the old offset conflicts.
    assert_eq!(
        refusal(
            store
                .record_segment(session, w, creation.blob, 0, seg, hash(first))
                .await
        )
        .code,
        "blob_offset_conflict"
    );
    let rest = &ciphertext[SEGMENT_LEN..];
    let last = store
        .record_segment(
            session,
            w,
            creation.blob,
            seg,
            rest.len() as u64,
            hash(rest),
        )
        .await
        .expect("last segment");
    let Recorded::Stored(done) = last else {
        panic!("the digest matches");
    };
    assert!(done.complete);
    assert_eq!(done.offset, creation.length);
    assert_eq!(
        refusal(
            store
                .record_segment(session, w, creation.blob, done.offset, 0, hash(&[]))
                .await
        )
        .code,
        "blob_offset_conflict",
        "a completed upload takes no more bytes"
    );
    assert_eq!(
        refusal(store.abandon_blob(session, w, creation.blob).await).code,
        "blob_offset_conflict",
        "DELETE abandons only an incomplete upload"
    );
}

/// An upload whose stored segment hashes miss its digest is discarded with
/// every segment, and an abandoned upload frees its ID.
pub async fn blob_digest_mismatch_discards_the_upload<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let session = world.devices[0].session;
    let (mut ciphertext, creation) = blob_for(1_200_000, world.domain);
    let store = &world.store;
    let w = world.workspace;
    store
        .create_blob(session, w, creation)
        .await
        .expect("created");
    let last = ciphertext.len() - 1;
    ciphertext[last] ^= 1;
    let Recorded::Discarded(segments) = upload_all(&world, 0, creation.blob, &ciphertext).await
    else {
        panic!("the digest misses");
    };
    assert_eq!(
        segments,
        ciphertext
            .chunks(SEGMENT_LEN)
            .map(hash)
            .collect::<Vec<Digest>>()
    );
    assert_eq!(
        refusal(store.blob_upload(session, w, creation.blob).await).code,
        "not_found"
    );

    // Abandoning returns the stored segments and frees the ID.
    store
        .create_blob(session, w, creation)
        .await
        .expect("again");
    let first = &ciphertext[..SEGMENT_LEN];
    store
        .record_segment(
            session,
            w,
            creation.blob,
            0,
            SEGMENT_LEN as u64,
            hash(first),
        )
        .await
        .expect("first segment");
    assert_eq!(
        store
            .abandon_blob(session, w, creation.blob)
            .await
            .expect("abandoned"),
        vec![hash(first)]
    );
    let other = BlobCreation {
        digest: Digest([1; 32]),
        ..creation
    };
    store
        .create_blob(session, w, other)
        .await
        .expect("the ID is free again");
}

/// A world whose listed blob a restore left missing, with the ciphertext
/// that belongs under its ID: device 0 uploaded it, and device 1 writes too.
async fn missing_blob<B: Backend>(store: Store<B>) -> (World<B>, Vec<u8>, BlobCreation) {
    let world = World::new(store, 2).await;
    let (ciphertext, creation) = blob_for(100_000, world.domain);
    let session = world.devices[0].session;
    world
        .store
        .create_blob(session, world.workspace, creation)
        .await
        .expect("created");
    upload_all(&world, 0, creation.blob, &ciphertext).await;
    let listing = world.op(0, |o| {
        o.blobs = sync_protocol::codec::Sorted::new(vec![creation.blob]).expect("sorted");
    });
    world.push(0, &[listing]).await.expect("listed");
    let segments = vec![hash(&ciphertext)];
    assert_eq!(
        world
            .store
            .lose_blob(world.workspace, world.domain, creation.blob, segments)
            .await
            .expect("lost"),
        Some(true)
    );
    (world, ciphertext, creation)
}

async fn still_missing<B: Backend>(world: &World<B>, blob: Id) {
    let session = world.devices[1].session;
    let missing = world
        .store
        .missing_blobs(session, world.workspace)
        .await
        .expect("listed");
    assert_eq!(missing.blobs.0, vec![blob], "devices still upload it");
    let upload = world
        .store
        .blob_upload(session, world.workspace, blob)
        .await
        .expect("the row stays");
    assert_eq!(upload.row.offset, 0);
    assert!(upload.segments.is_empty());
}

/// A wrong digest on a blob that operations list discards the bytes and
/// keeps the row, so the blob stays missing and another device can upload
/// it (§19, §21).
pub async fn a_listed_upload_returns_to_offset_zero<B: Backend>(store: Store<B>) {
    let (world, ciphertext, creation) = missing_blob(store).await;
    let usage = world.usage().await;
    let mut wrong = ciphertext.clone();
    wrong[0] ^= 1;
    let Recorded::Discarded(segments) = upload_all(&world, 1, creation.blob, &wrong).await else {
        panic!("the digest misses");
    };
    assert_eq!(segments, vec![hash(&wrong)]);
    still_missing(&world, creation.blob).await;
    assert_eq!(world.usage().await, usage, "the listed blob still counts");
    let Recorded::Stored(row) = upload_all(&world, 1, creation.blob, &ciphertext).await else {
        panic!("the right bytes fit");
    };
    assert!(row.complete);
}

/// Only the device that created an upload abandons a blob that operations
/// list, and that returns the row to offset 0 (§21).
pub async fn only_the_uploader_abandons_a_listed_blob<B: Backend>(store: Store<B>) {
    let (world, ciphertext, creation) = missing_blob(store).await;
    let w = world.workspace;
    assert_eq!(
        refusal(
            world
                .store
                .abandon_blob(world.devices[1].session, w, creation.blob)
                .await
        )
        .code,
        "forbidden"
    );
    still_missing(&world, creation.blob).await;
    let upload = world
        .store
        .abandon_blob(world.devices[0].session, w, creation.blob)
        .await
        .expect("the uploader abandons it");
    assert!(upload.is_empty(), "no segment came back yet");
    still_missing(&world, creation.blob).await;
    upload_all(&world, 1, creation.blob, &ciphertext).await;
}

/// A push lists only completed uploads of its own domain, and an
/// operation lists at most 4 blobs.
pub async fn pushes_list_completed_blobs_of_their_domain<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let session = world.devices[0].session;
    let w = world.workspace;
    let (ciphertext, creation) = blob_for(10_000, world.domain);
    world
        .store
        .create_blob(session, w, creation)
        .await
        .expect("created");

    let unknown = world.op(0, |o| {
        o.blobs =
            sync_protocol::codec::Sorted::new(vec![Id::random().expect("random")]).expect("sorted");
    });
    assert_eq!(
        push_failure(world.push(0, &[unknown]).await),
        ("blob_incomplete", 0)
    );
    let listing = world.op(0, |o| {
        o.blobs = sync_protocol::codec::Sorted::new(vec![creation.blob]).expect("sorted");
    });
    assert_eq!(
        push_failure(world.push(0, std::slice::from_ref(&listing)).await),
        ("blob_incomplete", 0),
        "the upload isn't complete"
    );
    upload_all(&world, 0, creation.blob, &ciphertext).await;
    let restricted = add_domain(&mut world).await;
    let wrong = world.op(0, |o| {
        o.domain = restricted;
        o.blobs = sync_protocol::codec::Sorted::new(vec![creation.blob]).expect("sorted");
    });
    assert_eq!(
        push_failure(world.push(0, &[wrong]).await),
        ("blob_domain_mismatch", 0)
    );
    let listing = world.op(0, |o| {
        o.blobs = sync_protocol::codec::Sorted::new(vec![creation.blob]).expect("sorted");
    });
    world.push(0, &[listing]).await.expect("lists the blob");

    let five: Vec<Id> = (0..5).map(|_| Id::random().expect("random")).collect();
    assert!(
        sync_protocol::codec::Sorted::<Id, sync_protocol::codec::Whole, 4>::new(five).is_err(),
        "an operation lists at most 4 blobs"
    );
}

/// Only the uploader reads a blob before an operation lists it; then any
/// active device that reads its domain does, and no other workspace's
/// device ever does.
pub async fn blob_downloads_follow_the_listing<B: Backend>(store: Store<B>) {
    let world = World::new(store, 2).await;
    let w = world.workspace;
    let (ciphertext, creation) = blob_for(2_500_000, world.domain);
    let uploader = world.devices[0].session;
    let other = world.devices[1].session;
    world
        .store
        .create_blob(uploader, w, creation)
        .await
        .expect("created");
    assert_eq!(
        refusal(world.store.blob_download(uploader, w, creation.blob).await).code,
        "blob_incomplete"
    );
    upload_all(&world, 0, creation.blob, &ciphertext).await;
    let served = world
        .store
        .blob_download(uploader, w, creation.blob)
        .await
        .expect("the uploader reads it before it's listed");
    assert_eq!(served.segments.len(), 3);
    assert_eq!(
        refusal(world.store.blob_download(other, w, creation.blob).await).code,
        "not_found"
    );
    let listing = world.op(0, |o| {
        o.blobs = sync_protocol::codec::Sorted::new(vec![creation.blob]).expect("sorted");
    });
    world.push(0, &[listing]).await.expect("listed");
    let served = world
        .store
        .blob_download(other, w, creation.blob)
        .await
        .expect("a reader of the domain reads it once listed");
    assert_eq!(served.row.digest, creation.digest);

    let outsider = World::on(Arc::clone(&world.store), 1).await;
    assert_eq!(
        refusal(
            world
                .store
                .blob_download(outsider.devices[0].session, w, creation.blob)
                .await
        )
        .code,
        "forbidden"
    );
    assert_eq!(
        refusal(
            world
                .store
                .blob_download(
                    outsider.devices[0].session,
                    outsider.workspace,
                    creation.blob
                )
                .await
        )
        .code,
        "not_found",
        "blob rows belong to their workspace"
    );
}

/// tus creations, segment records and pushes that list blobs, in several
/// domains from two instances, race policy changes without a deadlock:
/// each takes the domain's head row before any blob row (§15).
pub async fn blob_uploads_race_pushes_and_policy_changes<B: Backend>(
    first: Arc<Store<B>>,
    second: Arc<Store<B>>,
) {
    use std::collections::BTreeMap;
    use std::time::Duration;

    use sync_protocol::codec::{Encode, List, Sorted};
    use sync_protocol::messages::{PushRequest, RecordRef};
    use sync_protocol::records::{Operation, PolicyChange};

    let mut world = World::on(first, 3).await;
    let mut domains = vec![world.domain];
    for _ in 0..2 {
        domains.push(add_domain(&mut world).await);
    }
    let written = domains.clone();
    let world = Arc::new(world);
    let head = Arc::new(std::sync::Mutex::new(world.head));
    let stores = [world.store.clone(), second];

    let mut uploaders = Vec::new();
    for device in 0..3 {
        let world = world.clone();
        let head = head.clone();
        let store = stores[device % 2].clone();
        let mut order = written.clone();
        order.rotate_left(device);
        uploaders.push(tokio::spawn(async move {
            let session = world.devices[device].session;
            let mut chains: BTreeMap<Id, (u64, Digest)> = BTreeMap::new();
            let mut listed = 0;
            for _ in 0..6 {
                let mut ops: Vec<Signed<Operation>> = Vec::new();
                for domain in &order {
                    let (ciphertext, creation) = blob_for(1_000, *domain);
                    store
                        .create_blob(session, world.workspace, creation)
                        .await
                        .expect("no creation fails or deadlocks");
                    store
                        .record_segment(
                            session,
                            world.workspace,
                            creation.blob,
                            0,
                            ciphertext.len() as u64,
                            hash(&ciphertext),
                        )
                        .await
                        .expect("no segment fails or deadlocks");
                    let (revision, policy_head) = *head.lock().expect("the head");
                    let (counter, previous) =
                        chains.get(domain).copied().unwrap_or((0, Digest::ZERO));
                    let op = world.op(device, |op| {
                        op.domain = *domain;
                        op.counter = counter + 1;
                        op.previous = previous;
                        op.policy_revision = revision;
                        op.policy_head = policy_head;
                        op.blobs = Sorted::new(vec![creation.blob]).expect("sorted");
                    });
                    chains.insert(*domain, (counter + 1, op.digest()));
                    ops.push(op);
                }
                let body = PushRequest {
                    records: List(ops.iter().cloned().map(RecordRef::Operation).collect()),
                }
                .to_bytes();
                store
                    .push(session, world.workspace, body)
                    .await
                    .expect("no push fails or deadlocks");
                listed += ops.len();
            }
            listed
        }));
    }
    let changer = {
        let world = world.clone();
        let head = head.clone();
        let stores = stores.clone();
        tokio::spawn(async move {
            let mut domains = domains;
            for round in 0..4 {
                let (revision, digest) = *head.lock().expect("the head");
                let domain = Id::random().expect("random");
                let body = PolicyChange {
                    revision: revision + 1,
                    previous: digest,
                    ..world
                        .change(
                            0,
                            vec![head_of(&world.links)],
                            PolicyKind::CreateDomain {
                                domain,
                                marker_rid: Id::random().expect("random"),
                                access: manager(world.account),
                            },
                        )
                        .body
                };
                let change = Signed::sign(body, world.devices[0].keys.signing());
                let creator = &world.devices[0];
                let epoch = Signed::sign(
                    build_epoch(
                        &EpochParams {
                            workspace: world.workspace,
                            domain,
                            epoch: 1,
                            previous: Digest::ZERO,
                            policy_revision: change.body.revision,
                            policy_head: change.digest(),
                            rotating_device: creator.id(),
                            recipients: BTreeSet::from([creator.id()]),
                        },
                        &SecretKey::random().expect("random"),
                        None,
                        Nonce::ZERO,
                    ),
                    creator.keys.signing(),
                );
                let next = (change.body.revision, change.digest());
                super::inject_change(
                    &stores[round % 2],
                    change,
                    domains.clone(),
                    Some((domain, epoch)),
                )
                .await
                .expect("no policy change fails or deadlocks");
                domains.push(domain);
                *head.lock().expect("the head") = next;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };
    let listed = tokio::time::timeout(Duration::from_secs(120), async {
        let mut listed = 0;
        for uploader in uploaders {
            listed += uploader.await.expect("the uploads end");
        }
        changer.await.expect("the changes end");
        listed
    })
    .await
    .expect("nothing deadlocks");
    assert_eq!(listed, 3 * 6 * written.len());
}
