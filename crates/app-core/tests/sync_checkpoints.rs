//! Checkpoints in the desktop sync client against the real server (seam 4,
//! sync-v2.md §17): a writer signs a checkpoint once the server publishes
//! a due point, each device checks the checkpoints its log carries, every
//! operation names the latest checkpoint its writer reached, and a client
//! fetches by digest a checkpoint its log left out.

mod sync_support;

use std::time::Duration;

use app_core::sync::SyncError;
use sync_protocol::codec::Decode;
use sync_protocol::messages::{EntryRecord, PullResponse, PushRequest, RecordRef};
use sync_store::Backend;
use sync_support::{Pair, Server, pair_on};

/// Writes a note on the laptop and runs a round on each side.
async fn write_and_sync<B: Backend>(pair: &mut Pair<B>, name: &str) {
    pair.a
        .write(&format!("{name}.md"), format!("# {name}\n").as_bytes());
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round on the laptop");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
}

/// Writes notes until both devices accepted a checkpoint numbered
/// `number`.
async fn until_checkpoint<B: Backend>(pair: &mut Pair<B>, number: u64, prefix: &str) {
    for i in 0..40 {
        let number_of = |sync: &app_core::sync::workspace::WorkspaceSync| {
            sync.latest_checkpoint().map(|c| c.body.number)
        };
        if number_of(&pair.sync_a) == Some(number) && number_of(&pair.sync_b) == Some(number) {
            return;
        }
        write_and_sync(pair, &format!("{prefix}{i}")).await;
    }
    panic!("no checkpoint numbered {number} reached both devices");
}

async fn pair_with_checkpoints(entries: u64) -> Pair<sync_store_sqlite::SqliteBackend> {
    let mut pair = pair_on(Server::start_with_interval(entries).await).await;
    pair.sync_a.set_checkpoint_wait(Duration::ZERO);
    pair.sync_b.set_checkpoint_wait(Duration::ZERO);
    pair
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writers_sign_due_checkpoints_and_both_devices_accept_them() {
    let mut pair = pair_with_checkpoints(6).await;
    until_checkpoint(&mut pair, 1, "first").await;
    let first = pair.sync_a.latest_checkpoint().expect("accepted").clone();
    assert_eq!(
        pair.sync_b.latest_checkpoint().map(|c| c.digest()),
        Some(first.digest()),
        "both devices accept the same checkpoint"
    );
    assert!(pair.sync_a.checkpoint_failures().is_empty());
    assert!(pair.sync_b.checkpoint_failures().is_empty());

    // An operation names the latest checkpoint its writer reached (§13).
    let before = pair.proxy.requests().len();
    write_and_sync(&mut pair, "named").await;
    let named: Vec<(u64, sync_protocol::types::Digest)> = pair.proxy.requests()[before..]
        .iter()
        .filter(|r| r.path.ends_with("/ops"))
        .flat_map(|r| PushRequest::from_bytes(&r.body).expect("a push").records.0)
        .filter_map(|record| match record {
            RecordRef::Operation(op) => Some((op.body.checkpoint_seq, op.body.checkpoint)),
            _ => None,
        })
        .collect();
    assert!(!named.is_empty(), "the laptop pushed the note");
    assert!(
        named
            .iter()
            .all(|n| *n == (first.body.covered_seq, first.digest())),
        "{named:?}"
    );

    // The chain goes on: the next checkpoint names the first as previous.
    until_checkpoint(&mut pair, 2, "second").await;
    let second = pair.sync_b.latest_checkpoint().expect("accepted");
    assert_eq!(second.body.previous, first.digest());
    assert!(second.body.covered_seq > first.body.covered_seq);
    assert!(pair.sync_a.checkpoint_failures().is_empty());
    assert!(pair.sync_b.checkpoint_failures().is_empty());
    assert_eq!(pair.a.files(), pair.b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkpoint_left_out_of_one_log_is_fetched_by_digest() {
    let mut pair = pair_with_checkpoints(6).await;
    // The laptop's log never carries a checkpoint entry.
    pair.proxy.tamper(Box::new(|path, body| {
        if !path.ends_with("/pull") {
            return body;
        }
        let mut response = PullResponse::from_bytes(&body).expect("a pull response");
        for page in response.pages.0.iter_mut() {
            let kept: Vec<_> = page
                .entries
                .iter()
                .filter(|e| !matches!(e.record, EntryRecord::Checkpoint(_)))
                .cloned()
                .collect();
            page.entries = sync_protocol::codec::Sorted::new(kept).expect("sorted");
        }
        sync_protocol::codec::Encode::to_bytes(&response)
    }));
    // Only a record whose writer reached the checkpoint shows the laptop
    // that its log left one out.
    for i in 0..40 {
        if pair.sync_a.latest_checkpoint().is_some() {
            break;
        }
        pair.b.write(&format!("hidden{i}.md"), b"# Hidden\n");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round on the desktop");
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("a round on the laptop");
    }
    assert_eq!(
        pair.sync_a.latest_checkpoint().map(|c| c.digest()),
        pair.sync_b.latest_checkpoint().map(|c| c.digest()),
    );
    assert!(pair.sync_a.latest_checkpoint().is_some());
    assert!(
        pair.proxy
            .requests()
            .iter()
            .any(|r| r.path.contains("/checkpoints/")),
        "the laptop fetched the checkpoint by digest"
    );
    assert!(pair.sync_a.checkpoint_failures().is_empty());

    // A server that can't serve the checkpoint a record names leaves a
    // gap.
    pair.proxy.forge(Box::new(|_, path| {
        path.contains("/checkpoints/")
            .then(|| (404, sync_support::not_found_body()))
    }));
    let mut gap = false;
    for i in 0..40 {
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round on the desktop");
        pair.b.write(&format!("gap{i}.md"), b"# Gap\n");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round on the desktop");
        match pair.sync_a.round(&mut pair.service, 0).await {
            Err(SyncError::Check("sync_chain_gap")) => {
                gap = true;
                break;
            }
            Err(error) => panic!("unexpected {error:?}"),
            Ok(_) => {}
        }
    }
    assert!(gap, "the laptop reports sync_chain_gap");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkpoint_that_fails_is_reported_and_holds_nothing() {
    let mut pair = pair_with_checkpoints(6).await;
    // The laptop's log carries the first checkpoint without its heads.
    pair.proxy.tamper(Box::new(|path, body| {
        if !path.ends_with("/pull") {
            return body;
        }
        let mut response = PullResponse::from_bytes(&body).expect("a pull response");
        for page in response.pages.0.iter_mut() {
            let entries: Vec<_> = page
                .entries
                .iter()
                .cloned()
                .map(|mut e| {
                    if let EntryRecord::Checkpoint(bundle) = &mut e.record
                        && bundle.checkpoint.0.body.number == 1
                    {
                        bundle.heads = sync_protocol::codec::Sorted::default();
                    }
                    e
                })
                .collect();
            page.entries = sync_protocol::codec::Sorted::new(entries).expect("sorted");
        }
        sync_protocol::codec::Encode::to_bytes(&response)
    }));
    for i in 0..40 {
        if !pair.sync_a.checkpoint_failures().is_empty() {
            break;
        }
        write_and_sync(&mut pair, &format!("bad{i}")).await;
    }
    let failures = pair.sync_a.checkpoint_failures().to_vec();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].number, 1);
    assert_eq!(failures[0].code, "sync_manifest_mismatch");
    assert!(
        pair.sync_a.latest_checkpoint().is_none(),
        "a failed checkpoint never becomes the latest accepted one"
    );
    // It doesn't hold the cursor: the desktop's notes still arrive, and the
    // next checkpoint, which names it as previous, passes.
    pair.b.write("Later.md", b"# Later\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round on the laptop");
    assert_eq!(pair.a.files(), pair.b.files());
    until_checkpoint(&mut pair, 2, "next").await;
    assert_eq!(pair.sync_a.checkpoint_failures().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_root_mismatch_walks_the_tree_to_the_differing_objects() {
    use std::sync::{Arc, Mutex};
    use sync_protocol::codec::{Encode, List, Nested, Sorted};
    use sync_protocol::manifest::{bucket_hash, leaf_hash, root_of_buckets};
    use sync_protocol::messages::{LeafRef, ManifestBucketsResponse, ManifestLeavesResponse};
    use sync_protocol::records::{CheckpointBundle, Ref, Signed};
    use sync_protocol::types::{Digest, Id};

    let mut pair = pair_with_checkpoints(6).await;
    // Only the desktop signs, and the laptop pulls nothing meanwhile.
    pair.sync_a
        .set_checkpoint_wait(Duration::from_secs(1 << 40));
    for i in 0..40 {
        if pair.sync_b.latest_checkpoint().is_some() {
            break;
        }
        pair.b.write(&format!("d{i}.md"), b"# Desktop\n");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round on the desktop");
    }
    let real = pair.sync_b.latest_checkpoint().expect("accepted").clone();
    let (workspace, domain) = (pair.sync_b.workspace(), pair.sync_b.domain());
    let buckets = pair
        .desktop
        .service
        .client()
        .manifest_buckets(workspace, domain, real.body.covered_seq)
        .await
        .expect("the buckets")
        .buckets
        .0;
    // A server that claims an object the log never carried, in a bucket
    // that is empty in truth.
    let (index, rid) = (0u16..4096)
        .find_map(|i| {
            (buckets[usize::from(i)] == bucket_hash(i, &[])).then(|| {
                (
                    i,
                    Id([
                        (i >> 4) as u8,
                        ((i & 15) << 4) as u8,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        0,
                        9,
                    ]),
                )
            })
        })
        .expect("an empty bucket");
    let fake = Ref {
        device: pair.desktop.credentials.keys.device(),
        counter: 999,
        seq: 1,
        op_digest: Digest([7; 32]),
    };
    let mut fake_buckets = buckets.clone();
    fake_buckets[usize::from(index)] = bucket_hash(index, &[leaf_hash(&rid, &[fake.clone()])]);
    let mut body = real.body.clone();
    body.manifest_root = root_of_buckets(&fake_buckets);
    body.live_objects += 1;
    let forged = Signed::sign(body, pair.desktop.credentials.keys.signing());
    let forged_digest = forged.digest();
    let slot = Arc::new(Mutex::new(Some(forged)));
    let leaf = LeafRef {
        rid,
        op: fake.clone(),
    };
    pair.proxy.tamper(Box::new(move |path, body| {
        if path.ends_with("/pull") {
            let mut response = PullResponse::from_bytes(&body).expect("a pull response");
            for page in response.pages.0.iter_mut() {
                let entries: Vec<_> = page
                    .entries
                    .iter()
                    .cloned()
                    .map(|mut e| {
                        if let EntryRecord::Checkpoint(bundle) = &mut e.record
                            && let Some(forged) = slot.lock().unwrap().clone()
                        {
                            *bundle = CheckpointBundle {
                                checkpoint: Nested(forged),
                                heads: bundle.heads.clone(),
                            };
                        }
                        e
                    })
                    .collect();
                page.entries = Sorted::new(entries).expect("sorted");
            }
            return response.to_bytes();
        }
        // The proxy sees no query: the laptop asks for every bucket hash,
        // then for the leaves of the one bucket that differs.
        if path.ends_with("/manifest") && ManifestBucketsResponse::from_bytes(&body).is_ok() {
            return ManifestBucketsResponse {
                buckets: List(fake_buckets.clone()),
            }
            .to_bytes();
        }
        if path.ends_with("/manifest") {
            return ManifestLeavesResponse {
                leaves: Sorted::new(vec![leaf.clone()]).expect("sorted"),
                next: None,
            }
            .to_bytes();
        }
        body
    }));
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round on the laptop");
    let failures = pair.sync_a.checkpoint_failures().to_vec();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].digest, forged_digest);
    assert_eq!(failures[0].code, "sync_manifest_mismatch");
    assert_eq!(failures[0].differing, vec![rid]);
    assert!(pair.sync_a.latest_checkpoint().is_none());
}
