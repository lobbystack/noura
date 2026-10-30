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
    let index = (0u16..4096)
        .find(|i| buckets[usize::from(*i)] == bucket_hash(*i, &[]))
        .expect("an empty bucket");
    let mut bytes = [0u8; 16];
    bytes[0] = (index >> 4) as u8;
    bytes[1] = ((index & 15) << 4) as u8;
    bytes[15] = 9;
    let rid = Id(bytes);
    let fake = Ref {
        device: pair.desktop.credentials.keys.device(),
        counter: 999,
        seq: 1,
        op_digest: Digest([7; 32]),
    };
    let mut fake_buckets = buckets.clone();
    fake_buckets[usize::from(index)] =
        bucket_hash(index, &[leaf_hash(&rid, std::slice::from_ref(&fake))]);
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

/// A laptop that synced a folder until its log holds a checkpoint, with
/// one more note after it, and a second desktop of the account that
/// connects through a proxy, ready to download the workspace.
struct Bootstrap {
    _server: Server,
    laptop: sync_support::Desktop,
    a: sync_support::Folder,
    sync_a: app_core::sync::workspace::WorkspaceSync,
    checkpoint: sync_protocol::records::Signed<sync_protocol::records::Checkpoint>,
    proxy: sync_support::Proxy,
    desktop: sync_support::Desktop,
    service: app_core::sync::SyncService,
    b: sync_support::Folder,
}

async fn bootstrap_setup() -> Bootstrap {
    use sync_support::{Folder, Proxy, approve, first_desktop};

    let server = Server::start_with_interval(6).await;
    let mut laptop = first_desktop(&server, "ana@example.com").await;
    let mut a = Folder::new("Field notes");
    a.write("Old.md", b"# Old\n");
    let mut sync_a = app_core::sync::workspace::WorkspaceSync::turn_on(
        &mut laptop.service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.set_checkpoint_wait(Duration::ZERO);
    sync_a.round(&mut laptop.service, 0).await.expect("a round");
    std::fs::remove_file(a.path("Old.md")).unwrap();
    for i in 0..40 {
        if sync_a.latest_checkpoint().is_some() {
            break;
        }
        a.write(&format!("n{i}.md"), format!("# Note {i}\n").as_bytes());
        sync_a.round(&mut laptop.service, 0).await.expect("a round");
    }
    let checkpoint = sync_a.latest_checkpoint().expect("a checkpoint").clone();
    // Records after the checkpoint arrive by pulling on from it.
    a.write("After.md", b"# After\n");
    sync_a.round(&mut laptop.service, 0).await.expect("a round");

    let desktop = approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let proxy = Proxy::start(&server.base).await;
    let service = desktop.connect_through(&proxy.base).await;
    Bootstrap {
        _server: server,
        laptop,
        a,
        sync_a,
        checkpoint,
        proxy,
        desktop,
        service,
        b: Folder::new("Downloaded"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_device_bootstraps_from_the_latest_checkpoint() {
    use sync_protocol::messages::PullRequest;

    let Bootstrap {
        mut laptop,
        a,
        mut sync_a,
        checkpoint,
        proxy,
        mut service,
        mut b,
        ..
    } = bootstrap_setup().await;
    let mut sync_b = app_core::sync::workspace::WorkspaceSync::download(
        &mut service,
        b.engine.clone(),
        sync_a.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    sync_b.round(&mut service, 0).await.expect("the download");
    assert_eq!(b.files(), a.files());
    assert_eq!(
        sync_b.latest_checkpoint().map(|c| c.digest()),
        Some(checkpoint.digest()),
        "the new device starts its checkpoint chain at the bootstrap"
    );
    let requests = proxy.requests();
    // The keys come from keys/current, never from the whole key stream
    // (§18, step 1).
    assert!(requests.iter().any(|r| r.path.contains("/keys/current?")));
    assert!(!requests.iter().any(|r| r.path.contains("/keys?")));
    assert!(requests.iter().any(|r| r.path.contains("/live?")));
    assert!(requests.iter().any(|r| r.path.contains("buckets=all")));
    let pulled_from: Vec<u64> = requests
        .iter()
        .filter(|r| r.path.ends_with("/pull"))
        .map(|r| PullRequest::from_bytes(&r.body).expect("a pull").cursors.0[0].after)
        .collect();
    assert!(
        pulled_from
            .iter()
            .all(|after| *after >= checkpoint.body.covered_seq),
        "the device never pulls history below the checkpoint: {pulled_from:?}"
    );

    // Both devices keep syncing afterwards.
    b.write("FromDesktop.md", b"# From the desktop\n");
    sync_b.round(&mut service, 0).await.expect("a round");
    a.write("FromLaptop.md", b"# From the laptop\n");
    sync_a.round(&mut laptop.service, 0).await.expect("a round");
    sync_b.round(&mut service, 0).await.expect("a round");
    assert_eq!(b.files(), a.files());
    assert!(sync_b.checkpoint_failures().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrap_refuses_a_manifest_that_omits_an_object() {
    use sync_protocol::codec::{Encode, List};
    use sync_protocol::messages::LiveResponse;

    let Bootstrap {
        a,
        sync_a,
        proxy,
        mut service,
        mut b,
        ..
    } = bootstrap_setup().await;
    proxy.tamper(Box::new(|path, body| {
        if !path.ends_with("/live") {
            return body;
        }
        let mut response = LiveResponse::from_bytes(&body).expect("a live page");
        let mut entries = response.entries.0;
        entries.pop();
        response.entries = List(entries);
        response.to_bytes()
    }));
    let mut sync_b = app_core::sync::workspace::WorkspaceSync::download(
        &mut service,
        b.engine.clone(),
        sync_a.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    let error = sync_b.round(&mut service, 0).await.expect_err("refused");
    assert!(
        matches!(error, SyncError::Check("sync_manifest_mismatch")),
        "{error:?}"
    );
    assert!(b.files().is_empty(), "nothing installs");
    assert!(sync_b.latest_checkpoint().is_none());
    // An honest server lets it finish.
    proxy.honest();
    sync_b.round(&mut service, 0).await.expect("the download");
    assert_eq!(b.files(), a.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrap_during_heavy_checkpointing_matches_the_log() {
    heavy_checkpointing(Server::start_with_interval(3).await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrap_during_heavy_checkpointing_matches_the_log_on_postgres() {
    heavy_checkpointing(Server::start_postgres_with_interval(3).await).await;
}

async fn heavy_checkpointing<B: Backend>(server: Server<B>) {
    use sync_support::{Folder, approve};

    let mut pair = pair_on(server).await;
    pair.sync_a.set_checkpoint_wait(Duration::ZERO);
    pair.sync_b.set_checkpoint_wait(Duration::ZERO);
    for i in 0..8 {
        pair.a
            .write(&format!("a{i}.md"), format!("# A {i}\n").as_bytes());
        pair.b
            .write(&format!("b{i}.md"), format!("# B {i}\n").as_bytes());
        if i % 3 == 2 {
            std::fs::remove_file(pair.a.path(&format!("a{}.md", i - 1))).unwrap();
        }
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("a round");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round");
    }
    let number = pair
        .sync_a
        .latest_checkpoint()
        .map(|c| c.body.number)
        .expect("checkpoints");
    assert!(number >= 3, "checkpoints fell due often: {number}");

    let mut third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut c = Folder::new("Third");
    let mut sync_c = app_core::sync::workspace::WorkspaceSync::download(
        &mut third.service,
        c.engine.clone(),
        pair.sync_a.workspace(),
        &mut c.registry,
    )
    .await
    .expect("the download starts");
    sync_c.set_checkpoint_wait(Duration::ZERO);
    for i in 0..4 {
        pair.a.write(&format!("late{i}.md"), b"# Late\n");
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("a round");
        sync_c.round(&mut third.service, 0).await.expect("a round");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round");
    }
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    sync_c.round(&mut third.service, 0).await.expect("a round");
    assert_eq!(c.files(), pair.a.files());
    assert_eq!(pair.b.files(), pair.a.files());
    assert!(sync_c.checkpoint_failures().is_empty());
    assert!(pair.sync_a.checkpoint_failures().is_empty());
    assert!(pair.sync_b.checkpoint_failures().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_approved_device_bootstraps_only_from_its_anchor_or_a_descendant() {
    use sync_protocol::codec::{Encode, List};
    use sync_protocol::messages::{CheckpointChainResponse, CheckpointResponse};
    use sync_support::{Folder, Proxy, approve, first_desktop};

    let server = Server::start_with_interval(6).await;
    let mut laptop = first_desktop(&server, "ana@example.com").await;
    let mut a = Folder::new("Field notes");
    a.write("First.md", b"# First\n");
    let mut sync_a = app_core::sync::workspace::WorkspaceSync::turn_on(
        &mut laptop.service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.set_checkpoint_wait(Duration::ZERO);
    let mut first = None;
    for i in 0..60 {
        let latest = sync_a.latest_checkpoint().cloned();
        if first.is_none() {
            first = latest.clone().filter(|c| c.body.number == 1);
        }
        if latest.is_some_and(|c| c.body.number == 2) {
            break;
        }
        a.write(&format!("n{i}.md"), b"# Note\n");
        sync_a.round(&mut laptop.service, 0).await.expect("a round");
    }
    let first = first.expect("checkpoint 1");
    let anchor = sync_a.latest_checkpoint().expect("checkpoint 2").clone();
    assert_eq!(anchor.body.number, 2);
    let (workspace, domain) = (sync_a.workspace(), sync_a.domain());
    let older = laptop
        .service
        .client()
        .checkpoint_by_digest(workspace, domain, first.digest())
        .await
        .expect("served")
        .expect("checkpoint 1");
    let EntryRecord::Checkpoint(older_bundle) = older.record else {
        panic!("a checkpoint entry");
    };
    // The offer anchors checkpoint 2.
    let desktop = approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let proxy = Proxy::start(&server.base).await;
    let mut service = desktop.connect_through(&proxy.base).await;
    let mut b = Folder::new("Downloaded");
    let mut sync_b = app_core::sync::workspace::WorkspaceSync::download(
        &mut service,
        b.engine.clone(),
        workspace,
        &mut b.registry,
    )
    .await
    .expect("the download starts");

    // A server that rolls the new device back below its anchor.
    let seq = older.seq;
    proxy.tamper(Box::new(move |path, body| {
        if path.ends_with("/checkpoint") {
            return CheckpointResponse {
                seq,
                bundle: older_bundle.clone(),
            }
            .to_bytes();
        }
        body
    }));
    let error = sync_b.round(&mut service, 0).await.expect_err("refused");
    assert!(
        matches!(error, SyncError::Check("sync_manifest_rollback")),
        "{error:?}"
    );
    assert!(b.files().is_empty());

    // The laptop goes on to checkpoint 3, which descends from the anchor.
    proxy.honest();
    for i in 0..60 {
        if sync_a
            .latest_checkpoint()
            .is_some_and(|c| c.body.number == 3)
        {
            break;
        }
        a.write(&format!("m{i}.md"), b"# More\n");
        sync_a.round(&mut laptop.service, 0).await.expect("a round");
    }
    // A walk that doesn't reach the anchor fails.
    proxy.tamper(Box::new(|path, body| {
        if path.ends_with("/checkpoint-chain") {
            return CheckpointChainResponse {
                checkpoints: List(Vec::new()),
            }
            .to_bytes();
        }
        body
    }));
    let error = sync_b.round(&mut service, 0).await.expect_err("refused");
    assert!(
        matches!(error, SyncError::Check("sync_manifest_rollback")),
        "{error:?}"
    );
    proxy.honest();
    sync_b.round(&mut service, 0).await.expect("the download");
    assert_eq!(b.files(), a.files());
    assert!(
        proxy
            .requests()
            .iter()
            .any(|r| r.path.contains("/checkpoint-chain?")),
        "the device checked the descent"
    );
    assert_eq!(
        sync_b.latest_checkpoint().map(|c| c.body.number),
        sync_a.latest_checkpoint().map(|c| c.body.number)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrap_refuses_a_live_operation_at_another_log_position() {
    use sync_protocol::codec::{Encode, List};
    use sync_protocol::messages::LiveResponse;

    let Bootstrap {
        sync_a,
        proxy,
        mut service,
        mut b,
        ..
    } = bootstrap_setup().await;
    // The server places one live operation one sequence earlier for this
    // device than the checkpoint's root committed to.
    proxy.tamper(Box::new(|path, body| {
        if !path.ends_with("/live") {
            return body;
        }
        let mut response = LiveResponse::from_bytes(&body).expect("a live page");
        let mut entries = response.entries.0;
        if let Some(entry) = entries.iter_mut().find(|e| e.seq > 1) {
            entry.seq -= 1;
        }
        response.entries = List(entries);
        response.to_bytes()
    }));
    let mut sync_b = app_core::sync::workspace::WorkspaceSync::download(
        &mut service,
        b.engine.clone(),
        sync_a.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    let error = sync_b.round(&mut service, 0).await.expect_err("refused");
    assert!(
        matches!(error, SyncError::Check("sync_manifest_mismatch")),
        "{error:?}"
    );
    assert!(b.files().is_empty(), "nothing installs");
}

/// The checkpoint bundles that a page's push of `requests` carried.
fn pushed_checkpoints(
    requests: &[sync_support::Captured],
) -> Vec<sync_protocol::records::CheckpointBundle> {
    requests
        .iter()
        .filter(|r| r.method == "POST" && r.path.ends_with("/ops"))
        .flat_map(|r| PushRequest::from_bytes(&r.body).expect("a push").records.0)
        .filter_map(|record| match record {
            RecordRef::CheckpointBundle(bundle) => Some(bundle),
            _ => None,
        })
        .collect()
}

/// A laptop that turned sync on for a folder with one note.
async fn laptop_with_folder<B: Backend>(
    server: &Server<B>,
) -> (
    sync_support::Desktop,
    sync_support::Folder,
    app_core::sync::workspace::WorkspaceSync,
) {
    use sync_support::{Folder, first_desktop};

    let mut laptop = first_desktop(server, "ana@example.com").await;
    let mut a = Folder::new("Field notes");
    a.write("First.md", b"# First\n");
    let mut sync_a = app_core::sync::workspace::WorkspaceSync::turn_on(
        &mut laptop.service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.set_checkpoint_wait(Duration::ZERO);
    (laptop, a, sync_a)
}

/// Writes notes on one device until its latest accepted checkpoint
/// satisfies `done`.
async fn checkpoint_until(
    service: &mut app_core::sync::SyncService,
    folder: &sync_support::Folder,
    sync: &mut app_core::sync::workspace::WorkspaceSync,
    prefix: &str,
    done: impl Fn(u64) -> bool,
) -> sync_protocol::records::Signed<sync_protocol::records::Checkpoint> {
    for i in 0..300 {
        if let Some(latest) = sync.latest_checkpoint()
            && done(latest.body.number)
        {
            return latest.clone();
        }
        folder.write(&format!("{prefix}{i}.md"), format!("# {i}\n").as_bytes());
        sync.round(service, 0).await.expect("a round");
    }
    panic!("no such checkpoint");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrapped_sole_writer_signs_the_next_checkpoint() {
    use sync_support::{Folder, approve};

    let server = Server::start_with_interval(3).await;
    let (mut laptop, a, mut sync_a) = laptop_with_folder(&server).await;
    // A checkpoint whose successor's `skip` names one below it: an odd
    // number one below no power of two (§17, Checkpoint chain).
    let skips_below = |n: u64| n >= 5 && n % 2 == 1 && !(n + 1).is_power_of_two();
    let reached = checkpoint_until(&mut laptop.service, &a, &mut sync_a, "a", skips_below).await;
    let n = reached.body.number;
    let mut desktop = approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut b = Folder::new("Downloaded");
    let mut sync_b = app_core::sync::workspace::WorkspaceSync::download(
        &mut desktop.service,
        b.engine.clone(),
        sync_a.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    sync_b.set_checkpoint_wait(Duration::ZERO);
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("the download");
    assert_eq!(sync_b.latest_checkpoint().map(|c| c.body.number), Some(n));

    // The laptop goes quiet, as a replaced one would; the desktop writes
    // alone and signs the next checkpoint, whose skip lies below the one it
    // bootstrapped from.
    let next = checkpoint_until(&mut desktop.service, &b, &mut sync_b, "b", |m| m > n).await;
    assert_eq!(next.body.number, n + 1);
    assert_eq!(next.body.signer, desktop.id());
    assert!(sync_b.checkpoint_failures().is_empty());
    sync_a.round(&mut laptop.service, 0).await.expect("a round");
    assert_eq!(
        sync_a.latest_checkpoint().map(|c| c.digest()),
        Some(next.digest()),
        "the laptop accepts it"
    );
    assert!(sync_a.checkpoint_failures().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writer_that_raced_another_drops_its_checkpoint_on_stale() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use sync_protocol::codec::{Encode, Sorted};

    let mut pair = pair_with_checkpoints(6).await;
    pair.sync_a
        .set_checkpoint_wait(Duration::from_secs(1 << 40));
    // The laptop's log stops just before the first checkpoint entry, as if
    // it pulled a moment before the desktop's checkpoint committed.
    let hide = Arc::new(AtomicBool::new(true));
    let hiding = hide.clone();
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/pull") || !hiding.load(Ordering::SeqCst) {
            return body;
        }
        let mut response = PullResponse::from_bytes(&body).expect("a pull response");
        for page in response.pages.0.iter_mut() {
            let cut = page
                .entries
                .iter()
                .find(|e| matches!(e.record, EntryRecord::Checkpoint(_)))
                .map(|e| e.seq);
            if let Some(cut) = cut {
                let kept: Vec<_> = page
                    .entries
                    .iter()
                    .filter(|e| e.seq < cut)
                    .cloned()
                    .collect();
                page.entries = Sorted::new(kept).expect("sorted");
                page.complete_through = cut - 1;
                page.head = cut - 1;
                page.checkpoint_seq = 0;
                page.checkpoint_due = cut - 1;
            }
        }
        response.to_bytes()
    }));
    for i in 0..40 {
        if pair.sync_b.latest_checkpoint().is_some() {
            break;
        }
        write_and_sync(&mut pair, &format!("race{i}")).await;
    }
    let first = pair.sync_b.latest_checkpoint().expect("accepted").clone();
    assert!(pair.sync_a.latest_checkpoint().is_none());

    // The laptop signs its own, which the server answers with
    // `checkpoint_stale`; the laptop drops it and syncs on.
    pair.sync_a.set_checkpoint_wait(Duration::ZERO);
    let before = pair.proxy.requests().len();
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stale checkpoint fails no round");
    let pushed = pushed_checkpoints(&pair.proxy.requests()[before..]);
    assert_eq!(pushed.len(), 1, "the laptop signed one");
    assert_eq!(pushed[0].checkpoint.0.body.number, 1);
    assert_ne!(pushed[0].checkpoint.0.digest(), first.digest());
    let (workspace, domain) = (pair.sync_a.workspace(), pair.sync_a.domain());
    let latest = pair
        .laptop
        .service
        .client()
        .checkpoint(workspace, domain)
        .await
        .expect("served")
        .expect("a checkpoint");
    assert_eq!(
        latest.bundle.checkpoint.0.digest(),
        first.digest(),
        "the server kept the first"
    );

    hide.store(false, Ordering::SeqCst);
    let before = pair.proxy.requests().len();
    write_and_sync(&mut pair, "after").await;
    assert_eq!(
        pair.sync_a.latest_checkpoint().map(|c| c.digest()),
        Some(first.digest())
    );
    assert!(
        pushed_checkpoints(&pair.proxy.requests()[before..]).is_empty(),
        "no checkpoint falls due again yet"
    );
    assert!(pair.sync_a.checkpoint_failures().is_empty());
    assert_eq!(pair.a.files(), pair.b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_checkpoint_that_isnt_due_is_dropped_until_the_next_due_point() {
    use sync_protocol::codec::Encode;

    let mut pair = pair_with_checkpoints(40).await;
    pair.sync_b
        .set_checkpoint_wait(Duration::from_secs(1 << 40));
    // A server that publishes a due point where none is due.
    pair.proxy.tamper(Box::new(|path, body| {
        if !path.ends_with("/pull") {
            return body;
        }
        let mut response = PullResponse::from_bytes(&body).expect("a pull response");
        for page in response.pages.0.iter_mut() {
            if page.status == sync_protocol::messages::PageStatus::Ok && page.checkpoint_due == 0 {
                page.checkpoint_due = page.complete_through;
            }
        }
        response.to_bytes()
    }));
    let before = pair.proxy.requests().len();
    write_and_sync(&mut pair, "early").await;
    assert_eq!(
        pushed_checkpoints(&pair.proxy.requests()[before..]).len(),
        1,
        "the laptop signed at the published due point"
    );
    let (workspace, domain) = (pair.sync_a.workspace(), pair.sync_a.domain());
    let served = pair
        .laptop
        .service
        .client()
        .checkpoint(workspace, domain)
        .await
        .expect("answered");
    assert!(served.is_none(), "the server answered checkpoint_not_due");
    assert!(pair.sync_a.latest_checkpoint().is_none());

    // The laptop waits for the next due point the server publishes.
    pair.proxy.honest();
    let before = pair.proxy.requests().len();
    write_and_sync(&mut pair, "later").await;
    assert!(pushed_checkpoints(&pair.proxy.requests()[before..]).is_empty());
    assert_eq!(pair.a.files(), pair.b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_domain_that_a_removal_holds_gets_no_checkpoint() {
    use sync_support::approve;

    let mut pair = pair_with_checkpoints(6).await;
    let never = Duration::from_secs(1 << 40);
    pair.sync_a.set_checkpoint_wait(never);
    pair.sync_b.set_checkpoint_wait(never);
    let third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    for i in 0..8 {
        write_and_sync(&mut pair, &format!("due{i}")).await;
    }
    // The laptop removes the third device and records nothing yet, so the
    // desktop holds the domain (§9).
    pair.laptop
        .service
        .remove_device(third.id(), &[], &mut [])
        .await
        .expect("removed");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    assert!(pair.sync_b.removal_hold());
    pair.sync_b.set_checkpoint_wait(Duration::ZERO);
    for _ in 0..3 {
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round");
    }
    let (workspace, domain) = (pair.sync_b.workspace(), pair.sync_b.domain());
    let served = pair
        .desktop
        .service
        .client()
        .checkpoint(workspace, domain)
        .await
        .expect("answered");
    assert!(served.is_none(), "the held desktop signed no checkpoint");

    // Once the removal is recorded and the domain rotates, it signs.
    pair.desktop
        .service
        .record_heads(pair.sync_b.remote_mut().unwrap(), &[])
        .await
        .expect("recorded");
    let signed = checkpoint_until(
        &mut pair.desktop.service,
        &pair.b,
        &mut pair.sync_b,
        "held",
        |_| true,
    )
    .await;
    assert!(!pair.sync_b.removal_hold());
    assert_eq!(signed.body.signer, pair.desktop.id());
}

/// Rewrites, on the laptop's pulls, the checkpoint entry numbered `number`
/// with `forge`, which gets the real bundle and the bundles seen before.
fn forge_log_checkpoint(
    proxy: &sync_support::Proxy,
    number: u64,
    forge: impl Fn(
        &sync_protocol::records::CheckpointBundle,
        &[sync_protocol::records::CheckpointBundle],
    ) -> sync_protocol::records::CheckpointBundle
    + Send
    + Sync
    + 'static,
) {
    use std::sync::{Arc, Mutex};
    use sync_protocol::codec::{Encode, Sorted};

    let seen: Arc<Mutex<Vec<sync_protocol::records::CheckpointBundle>>> = Arc::default();
    proxy.tamper(Box::new(move |path, body| {
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
                    if let EntryRecord::Checkpoint(bundle) = &mut e.record {
                        if bundle.checkpoint.0.body.number == number {
                            *bundle = forge(bundle, &seen.lock().unwrap());
                        } else {
                            seen.lock().unwrap().push(bundle.clone());
                        }
                    }
                    e
                })
                .collect();
            page.entries = Sorted::new(entries).expect("sorted");
        }
        response.to_bytes()
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_log_checkpoint_its_signer_didnt_sign_fails() {
    use sync_protocol::codec::Nested;
    use sync_protocol::records::{CheckpointBundle, Signed};

    let mut pair = pair_with_checkpoints(6).await;
    pair.sync_a
        .set_checkpoint_wait(Duration::from_secs(1 << 40));
    // The desktop's checkpoint arrives naming the laptop as its signer.
    let key = pair.desktop.credentials.keys.signing().clone();
    let laptop = pair.laptop.id();
    forge_log_checkpoint(&pair.proxy, 1, move |real, _| {
        let mut body = real.checkpoint.0.body.clone();
        body.signer = laptop;
        CheckpointBundle {
            checkpoint: Nested(Signed::sign(body, &key)),
            heads: real.heads.clone(),
        }
    });
    for i in 0..40 {
        if !pair.sync_a.checkpoint_failures().is_empty() {
            break;
        }
        write_and_sync(&mut pair, &format!("s{i}")).await;
    }
    let failures = pair.sync_a.checkpoint_failures().to_vec();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].code, "sync_writer_not_authorized");
    assert!(pair.sync_a.latest_checkpoint().is_none());
    assert!(pair.sync_b.latest_checkpoint().is_some());
    assert_eq!(pair.a.files(), pair.b.files(), "it holds no cursor");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_log_checkpoint_that_rolls_back_fails() {
    use sync_protocol::codec::Nested;
    use sync_protocol::manifest::heads_digest;
    use sync_protocol::records::{CheckpointBundle, Signed};

    let mut pair = pair_with_checkpoints(6).await;
    pair.sync_a
        .set_checkpoint_wait(Duration::from_secs(1 << 40));
    // The second checkpoint arrives covering no more than the first, with
    // the first's heads, signed by its real signer.
    let key = pair.desktop.credentials.keys.signing().clone();
    forge_log_checkpoint(&pair.proxy, 2, move |real, seen| {
        let first = seen
            .iter()
            .find(|b| b.checkpoint.0.body.number == 1)
            .expect("the first passed before");
        let mut body = real.checkpoint.0.body.clone();
        body.covered_seq = first.checkpoint.0.body.covered_seq;
        body.heads_digest = heads_digest(&first.heads);
        CheckpointBundle {
            checkpoint: Nested(Signed::sign(body, &key)),
            heads: first.heads.clone(),
        }
    });
    for i in 0..60 {
        if !pair.sync_a.checkpoint_failures().is_empty() {
            break;
        }
        write_and_sync(&mut pair, &format!("r{i}")).await;
    }
    let failures = pair.sync_a.checkpoint_failures().to_vec();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].number, 2);
    assert_eq!(failures[0].code, "sync_manifest_rollback");
    assert_eq!(
        pair.sync_a.latest_checkpoint().map(|c| c.body.number),
        Some(1),
        "the first stays the latest accepted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_descent_walk_through_skip_reaches_an_old_offer_anchor() {
    descent_through_skip(Server::start_with_interval(3).await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_descent_walk_through_skip_reaches_an_old_offer_anchor_on_postgres() {
    descent_through_skip(Server::start_postgres_with_interval(3).await).await;
}

async fn descent_through_skip<B: Backend>(server: Server<B>) {
    use std::sync::{Arc, Mutex};
    use sync_protocol::messages::CheckpointChainResponse;
    use sync_support::{Folder, Proxy, approve};

    let (mut laptop, a, mut sync_a) = laptop_with_folder(&server).await;
    let anchor = checkpoint_until(&mut laptop.service, &a, &mut sync_a, "a", |n| n >= 2).await;
    let (workspace, domain) = (sync_a.workspace(), sync_a.domain());
    // The app makes offers over the workspaces it opens afresh, not over
    // a running sync (account/live.rs): the anchor comes from the mark.
    let mut opened = laptop
        .service
        .open_workspace(workspace)
        .await
        .expect("opened");
    let desktop = approve(&server, &mut laptop, std::slice::from_mut(&mut opened)).await;
    let proxy = Proxy::start(&server.base).await;
    let mut service = desktop.connect_through(&proxy.base).await;
    let kept = service
        .kept_heads()
        .checkpoint_anchor(&workspace, &domain)
        .expect("an anchor");
    assert_eq!(
        (kept.covered_seq, kept.checkpoint),
        (anchor.body.covered_seq, anchor.digest()),
        "the offer anchors the accepted checkpoint"
    );

    // Many checkpoints later, the desktop downloads.
    let latest = checkpoint_until(&mut laptop.service, &a, &mut sync_a, "b", |n| n >= 13).await;
    let walked: Arc<Mutex<Option<usize>>> = Arc::default();
    let seen = walked.clone();
    proxy.tamper(Box::new(move |path, body| {
        if path.ends_with("/checkpoint-chain") {
            let walk = CheckpointChainResponse::from_bytes(&body).expect("a walk");
            *seen.lock().unwrap() = Some(walk.checkpoints.0.len());
        }
        body
    }));
    // Checkpoints commit, and a note before the checkpoint goes, while the
    // desktop is between the checkpoint and its live operations.
    let (arrived, release) = proxy.hold("/live");
    let mut b = Folder::new("Downloaded");
    let mut sync_b = app_core::sync::workspace::WorkspaceSync::download(
        &mut service,
        b.engine.clone(),
        workspace,
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    let meanwhile = async {
        arrived.notified().await;
        std::fs::remove_file(a.path("First.md")).unwrap();
        let start = latest.body.number;
        checkpoint_until(&mut laptop.service, &a, &mut sync_a, "c", |n| {
            n >= start + 2
        })
        .await;
        release.notify_one();
    };
    let (downloaded, ()) = tokio::join!(sync_b.round(&mut service, 0), meanwhile);
    downloaded.expect("the download");
    assert_eq!(
        sync_b.checkpoint_failures(),
        &[],
        "the bootstrap matched the checkpoint it started from"
    );
    sync_b.round(&mut service, 0).await.expect("a round");
    assert_eq!(b.files(), a.files());
    assert!(!b.files().contains_key("First.md"));
    assert_eq!(
        sync_b.latest_checkpoint().map(|c| c.digest()),
        sync_a.latest_checkpoint().map(|c| c.digest())
    );

    // The walk took skip links: at most k(k + 1) / 2 records for fewer than
    // 2^k checkpoints, and fewer than the numbers between the two.
    let walked = walked.lock().unwrap().expect("the descent was checked");
    let k = u64::BITS - latest.body.number.leading_zeros();
    assert!(walked as u64 <= u64::from(k * (k + 1) / 2), "{walked}");
    assert!((walked as u64) < latest.body.number - anchor.body.number + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recovery_kit_from_the_app_anchors_the_accepted_checkpoint() {
    let server = Server::start_with_interval(3).await;
    let (mut laptop, a, mut sync_a) = laptop_with_folder(&server).await;
    let accepted = checkpoint_until(&mut laptop.service, &a, &mut sync_a, "a", |n| n >= 1).await;
    let workspace = sync_a.workspace();
    let mut opened = laptop
        .service
        .open_workspace(workspace)
        .await
        .expect("opened");
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("Noura recovery kit");
    let export = laptop
        .service
        .export_kit(&path, &[], std::slice::from_mut(&mut opened))
        .await
        .expect("exported");
    let kit =
        app_core::sync::recovery::open_kit(&path, export.secret.as_ref()).expect("the kit opens");
    let listed = kit
        .workspaces
        .iter()
        .find(|w| w.workspace == workspace)
        .expect("listed");
    let anchor = listed
        .anchors
        .iter()
        .find(|c| c.domain == sync_a.domain())
        .expect("the workspace domain");
    assert_eq!(
        (anchor.covered_seq, anchor.checkpoint),
        (accepted.body.covered_seq, accepted.digest())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrap_resumes_after_the_live_pages_it_fetched() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use sync_protocol::codec::{Encode, List};
    use sync_protocol::messages::{LiveCursor, LiveResponse};

    let Bootstrap {
        a,
        sync_a,
        proxy,
        desktop,
        mut service,
        mut b,
        ..
    } = bootstrap_setup().await;
    // The first live page carries one operation, and the connection drops
    // before the second.
    let calls = Arc::new(AtomicUsize::new(0));
    let counted = calls.clone();
    proxy.forge(Box::new(move |_, path| {
        (path.ends_with("/live") && counted.fetch_add(1, Ordering::SeqCst) == 1)
            .then(|| (503, Vec::new()))
    }));
    proxy.tamper(Box::new(|path, body| {
        if !path.ends_with("/live") {
            return body;
        }
        let mut response = LiveResponse::from_bytes(&body).expect("a live page");
        if response.next.is_none() && response.entries.0.len() > 1 {
            let first = response.entries.0[0].clone();
            let EntryRecord::Operation(op) = &first.record else {
                panic!("an operation");
            };
            response.next = Some(LiveCursor {
                rid: op.body.rid,
                device: op.body.device,
                counter: op.body.counter,
            });
            response.entries = List(vec![first]);
        }
        response.to_bytes()
    }));
    let mut sync_b = app_core::sync::workspace::WorkspaceSync::download(
        &mut service,
        b.engine.clone(),
        sync_a.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    sync_b
        .round(&mut service, 0)
        .await
        .expect_err("the connection dropped");
    assert!(b.files().is_empty());

    // After a restart, the device asks for the page after the one it kept.
    drop(sync_b);
    proxy.honest();
    let before = proxy.requests().len();
    let mut sync_b = app_core::sync::workspace::WorkspaceSync::open(
        b.engine.clone(),
        desktop.credentials.clone(),
        desktop.secrets(),
        &b.registry,
    )
    .expect("the journal opens");
    sync_b.round(&mut service, 0).await.expect("the download");
    let live: Vec<String> = proxy.requests()[before..]
        .iter()
        .filter(|r| r.path.contains("/live?"))
        .map(|r| r.path.clone())
        .collect();
    assert_eq!(live.len(), 1, "{live:?}");
    assert!(live[0].contains("after="), "{live:?}");
    assert_eq!(b.files(), a.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrap_refuses_a_checkpoint_a_removed_device_signed() {
    use sync_protocol::codec::Encode;
    use sync_protocol::messages::CheckpointResponse;
    use sync_support::{Folder, Proxy, approve};

    let mut pair = pair_with_checkpoints(6).await;
    pair.sync_a
        .set_checkpoint_wait(Duration::from_secs(1 << 40));
    // A third device, approved before any checkpoint, holds no anchor and
    // relies on the signer and recency checks.
    let third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    until_checkpoint(&mut pair, 1, "d").await;
    let (workspace, domain) = (pair.sync_a.workspace(), pair.sync_a.domain());
    let old = pair
        .laptop
        .service
        .client()
        .checkpoint(workspace, domain)
        .await
        .expect("served")
        .expect("checkpoint 1");
    assert_eq!(old.bundle.checkpoint.0.body.signer, pair.desktop.id());

    // The laptop removes the desktop, which signed it, and signs the due
    // checkpoint that follows.
    pair.laptop
        .service
        .remove_device(
            pair.desktop.id(),
            &[],
            std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
        )
        .await
        .expect("removed and recorded");
    pair.sync_a.set_checkpoint_wait(Duration::ZERO);
    checkpoint_until(
        &mut pair.laptop.service,
        &pair.a,
        &mut pair.sync_a,
        "l",
        |n| n >= 2,
    )
    .await;

    // A server that seeds the third device from the removed device's.
    let proxy = Proxy::start(&pair.server.base).await;
    let mut service = third.connect_through(&proxy.base).await;
    proxy.tamper(Box::new(move |path, body| {
        if path.ends_with("/checkpoint") {
            return CheckpointResponse {
                seq: old.seq,
                bundle: old.bundle.clone(),
            }
            .to_bytes();
        }
        body
    }));
    let mut c = Folder::new("Third");
    let mut sync_c = app_core::sync::workspace::WorkspaceSync::download(
        &mut service,
        c.engine.clone(),
        workspace,
        &mut c.registry,
    )
    .await
    .expect("the download starts");
    let error = sync_c.round(&mut service, 0).await.expect_err("refused");
    assert!(
        matches!(error, SyncError::Check("sync_manifest_rollback")),
        "{error:?}"
    );
    assert!(c.files().is_empty(), "nothing installs");
    proxy.honest();
    sync_c.round(&mut service, 0).await.expect("the download");
    assert_eq!(c.files(), pair.a.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_device_bootstraps_right_after_a_replacement() {
    use sync_protocol::codec::{Encode, ExactBytes};
    use sync_protocol::messages::{KeyRecord, KeysResponse};
    use sync_support::{Folder, approve};

    let mut pair = pair_with_checkpoints(6).await;
    pair.sync_a
        .set_checkpoint_wait(Duration::from_secs(1 << 40));
    // A third device joins and the laptop removes it without recording the
    // removal, so the desktop records it and rotates to epoch 2.
    let third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    pair.sync_a.round(&mut pair.service, 0).await.unwrap();
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .unwrap();
    pair.laptop
        .service
        .remove_device(third.id(), &[], &mut [])
        .await
        .expect("removed");
    pair.desktop
        .service
        .record_heads(pair.sync_b.remote_mut().unwrap(), &[])
        .await
        .expect("the desktop records the head and rotates");
    assert_eq!(pair.sync_b.remote_mut().unwrap().epoch(), 2);
    // The desktop alone signs a checkpoint under epoch 2.
    let skipped = checkpoint_until(
        &mut pair.desktop.service,
        &pair.b,
        &mut pair.sync_b,
        "e2",
        |_| true,
    )
    .await;
    assert_eq!(skipped.body.epoch, 2);

    // The laptop's envelope of epoch 2 fails, so it replaces the epoch,
    // and the replacement's entry makes a checkpoint due (§17).
    let (reader, wrapper) = (pair.laptop.id(), pair.desktop.id());
    pair.proxy.tamper(Box::new(move |path, body| {
        if !(path.ends_with("/keys/current") || path.ends_with("/keys")) {
            return body;
        }
        let mut keys = KeysResponse::from_bytes(&body).unwrap();
        for entry in keys.entries.0.iter_mut() {
            if let KeyRecord::Envelope(envelope) = &mut entry.record
                && envelope.body.recipient == reader
                && envelope.body.wrapping_device == wrapper
            {
                let mut bytes = envelope.body.ciphertext.as_slice().to_vec();
                bytes[0] ^= 1;
                envelope.body.ciphertext = ExactBytes::new(bytes).unwrap();
            }
        }
        keys.to_bytes()
    }));
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop signs a replacement");
    pair.proxy.honest();
    assert_eq!(pair.sync_a.remote_mut().unwrap().epoch(), 3);
    let mut replaced = None;
    for i in 0..10 {
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round");
        replaced = pair
            .sync_b
            .latest_checkpoint()
            .filter(|c| c.body.epoch == 3)
            .cloned();
        if replaced.is_some() {
            break;
        }
        pair.b.write(&format!("e3{i}.md"), b"# After\n");
    }
    let replaced = replaced.expect("a checkpoint under epoch 3 followed the replacement");
    assert!(replaced.body.number > skipped.body.number);

    // A new device bootstraps from it.
    let mut fourth = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().unwrap()),
    )
    .await;
    let mut d = Folder::new("Fourth");
    let mut sync_d = app_core::sync::workspace::WorkspaceSync::download(
        &mut fourth.service,
        d.engine.clone(),
        pair.sync_a.workspace(),
        &mut d.registry,
    )
    .await
    .expect("the download starts");
    sync_d
        .round(&mut fourth.service, 0)
        .await
        .expect("the download");
    assert_eq!(
        sync_d.latest_checkpoint().map(|c| c.body.epoch),
        Some(3),
        "it starts from the checkpoint the replacement made due"
    );
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    assert_eq!(d.files(), pair.b.files());
    assert!(sync_d.checkpoint_failures().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offers_and_kits_over_workspaces_opened_before_a_checkpoint_anchor_the_latest() {
    use sync_support::approve;

    let server = Server::start_with_interval(3).await;
    let (mut laptop, a, mut sync_a) = laptop_with_folder(&server).await;
    let workspace = sync_a.workspace();
    // The app opens its workspaces once, when it connects (account/live.rs),
    // and makes later offers and kits over them.
    let mut opened = laptop
        .service
        .open_workspace(workspace)
        .await
        .expect("opened");
    let accepted = checkpoint_until(&mut laptop.service, &a, &mut sync_a, "a", |n| n >= 2).await;

    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("Noura recovery kit");
    let export = laptop
        .service
        .export_kit(&path, &[], std::slice::from_mut(&mut opened))
        .await
        .expect("exported");
    let kit =
        app_core::sync::recovery::open_kit(&path, export.secret.as_ref()).expect("the kit opens");
    let anchor = kit.workspaces[0].anchors.iter().next().expect("an anchor");
    assert_eq!(
        (anchor.covered_seq, anchor.checkpoint),
        (accepted.body.covered_seq, accepted.digest()),
        "the kit anchors the checkpoint accepted after the workspace opened"
    );

    let desktop = approve(&server, &mut laptop, std::slice::from_mut(&mut opened)).await;
    let kept = desktop
        .service
        .kept_heads()
        .checkpoint_anchor(&workspace, &sync_a.domain())
        .expect("an anchor");
    assert_eq!(
        (kept.covered_seq, kept.checkpoint),
        (accepted.body.covered_seq, accepted.digest()),
        "the offer anchors it too"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recovery_bootstraps_from_its_kits_anchor_and_signs_the_next_checkpoint() {
    a_recovery_bootstraps_from_its_kits_anchor(Server::start_with_interval(3).await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_recovery_bootstraps_from_its_kits_anchor_and_signs_the_next_checkpoint_on_postgres() {
    a_recovery_bootstraps_from_its_kits_anchor(Server::start_postgres_with_interval(3).await).await;
}

/// After losing the laptop, its only device, the account recovers with a
/// kit. The recovery removes the laptop, which signed every checkpoint,
/// so Recency refuses the latest, and no other writer is left to sign the
/// next. The recovered device bootstraps from the kit's anchor, which
/// Recency skips, pulls on through the later checkpoints as log entries,
/// and signs the checkpoint the removal made due.
async fn a_recovery_bootstraps_from_its_kits_anchor<B: Backend>(server: Server<B>) {
    use app_core::sync::recovery::{Recovery, open_kit};
    use std::sync::Arc;
    use sync_support::{Folder, MemorySecrets};

    let (mut laptop, a, mut sync_a) = laptop_with_folder(&server).await;
    let workspace = sync_a.workspace();
    let anchored = checkpoint_until(&mut laptop.service, &a, &mut sync_a, "a", |n| n >= 2).await;
    let mut opened = laptop
        .service
        .open_workspace(workspace)
        .await
        .expect("opened");
    let outside = tempfile::tempdir().unwrap();
    let path = outside.path().join("Noura recovery kit");
    let export = laptop
        .service
        .export_kit(&path, &[], std::slice::from_mut(&mut opened))
        .await
        .expect("exported");
    // The laptop writes on and signs later checkpoints before it's lost.
    let start = anchored.body.number;
    let latest = checkpoint_until(&mut laptop.service, &a, &mut sync_a, "b", |n| {
        n >= start + 3
    })
    .await;
    a.write("Last.md", b"# Last\n");
    sync_a.round(&mut laptop.service, 0).await.expect("a round");

    let kit = open_kit(&path, export.secret.as_ref()).expect("the kit opens");
    let signed_in = server
        .sign_in(laptop.credentials.account, &server.base)
        .await;
    let secrets = Arc::new(MemorySecrets::default());
    let state = tempfile::tempdir().unwrap();
    let recovery = Recovery::begin(&signed_in, kit, secrets.clone(), state.path())
        .await
        .expect("the kit's identity proves");
    let (mut service, _workspaces, _) = recovery.finish(&[], &*secrets).await.expect("recovered");

    let mut b = Folder::new("Recovered");
    let mut sync_b = app_core::sync::workspace::WorkspaceSync::download(
        &mut service,
        b.engine.clone(),
        workspace,
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    sync_b.set_checkpoint_wait(Duration::ZERO);
    sync_b
        .round(&mut service, 0)
        .await
        .expect("the recovered device downloads");
    assert_eq!(b.files(), a.files());
    assert!(sync_b.checkpoint_failures().is_empty());
    assert_eq!(
        sync_b.latest_checkpoint().map(|c| c.digest()),
        Some(latest.digest()),
        "the later checkpoints pass as log entries"
    );

    // The removal made a checkpoint due, and the recovered device, the only
    // writer left, signs it.
    let me = service.credentials().keys.device();
    let next = checkpoint_until(&mut service, &b, &mut sync_b, "c", |n| {
        n > latest.body.number
    })
    .await;
    assert_eq!(next.body.signer, me);
    assert!(sync_b.checkpoint_failures().is_empty());
}

/// The fixture's kit and offer, read through the client's code: the kit
/// opens with its secret and anchors each domain with the checkpoint and
/// its restore generation, and the offer anchors the workspace domain the
/// same way (§10, §12, `sync-v2-recovery.json`, `sync-v2-approval.json`).
#[test]
fn the_fixture_kit_and_offer_carry_checkpoint_anchors_with_their_generation() {
    use sync_protocol::records::{Offer, Signed};
    use sync_protocol::types::{Digest, Id};

    let fixture = |name: &str| -> serde_json::Value {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/workspace-format/fixtures")
            .join(name);
        serde_json::from_slice(&std::fs::read(path).expect("the fixture")).expect("JSON")
    };
    let hex = |v: &serde_json::Value| sync_protocol::types::from_hex(v.as_str().unwrap()).unwrap();
    let record = |doc: &serde_json::Value, name: &str| -> serde_json::Value {
        doc["records"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .expect("the record")
            .clone()
    };

    let recovery = fixture("sync-v2-recovery.json");
    let case = recovery["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "open_with_the_secret")
        .expect("the case")
        .clone();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("Noura recovery kit");
    std::fs::write(&path, hex(&case["kit"])).unwrap();
    let kit = app_core::sync::recovery::open_kit(&path, case["secret_text"].as_str().unwrap())
        .expect("the kit opens");
    let want = &record(&recovery, "kit_payload")["value"]["workspaces"][0];
    let listed = &kit.workspaces[0];
    assert_eq!(listed.generation, want["generation"].as_u64().unwrap());
    let anchor = listed.anchors.iter().next().expect("an anchor");
    let wanted = &want["anchors"][0];
    assert_eq!(anchor.covered_seq, wanted["covered_seq"].as_u64().unwrap());
    assert_eq!(
        anchor.checkpoint,
        Digest::from_hex(wanted["checkpoint"].as_str().unwrap()).unwrap()
    );
    assert_eq!(
        anchor.domain,
        Id::from_hex(wanted["domain"].as_str().unwrap()).unwrap()
    );

    let approval = fixture("sync-v2-approval.json");
    let offer = record(&approval, "offer");
    let decoded = Signed::<Offer>::from_bytes(&hex(&offer["hex"])).expect("the offer decodes");
    let want = &offer["value"]["body"]["anchors"][0];
    let anchor = &decoded.body.anchors[0];
    assert_eq!(anchor.generation, want["generation"].as_u64().unwrap());
    assert_eq!(
        anchor.checkpoint.checkpoint,
        Digest::from_hex(want["checkpoint"]["checkpoint"].as_str().unwrap()).unwrap()
    );
    assert_eq!(
        anchor.checkpoint.covered_seq,
        want["checkpoint"]["covered_seq"].as_u64().unwrap()
    );
    // 1,000 workspaces' anchors stay near 120 KB, within one request.
    let size = approval["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == "offer_for_1000_workspaces")
        .expect("the case");
    let length = size["length"].as_u64().unwrap();
    assert!((115_000..130_000).contains(&length), "{length}");
}
