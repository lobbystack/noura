//! Plans in the desktop client against a real server that sells them
//! (seam 4, sync-v2.md §20): a file past the plan's largest file stays on
//! the device and later captures leave it out, and a workspace over plan
//! keeps its changes in the outbox until it's back within the plan. The
//! files on the device never change.

use crate::sync_support;
use std::time::Duration;

use sync_store::{Backend, WriteTxn};
use sync_support::{Pair, Server, pair_on};

/// `len` bytes that don't compress, rounded to 32.
fn noise(len: u32) -> Vec<u8> {
    (0..len / 32)
        .flat_map(|i| sync_protocol::crypto::hash(&i.to_be_bytes()).0)
        .collect()
}

async fn both_rounds<B: Backend>(pair: &mut Pair<B>) {
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round on the laptop");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
}

/// Puts the pair's workspace over plan, or back, as a downgrade or an
/// upgrade would.
async fn set_over_plan<B: Backend>(pair: &Pair<B>, over: bool) {
    let workspace = pair.sync_a.workspace();
    let today = pair.server.store.clock().now() / 86_400;
    pair.server
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move { txn.set_over_plan(workspace, over.then_some(today), 0).await })
        }))
        .await
        .expect("set");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_past_the_plans_largest_file_stays_on_this_device() {
    let mut pair = pair_on(Server::start_with_billing().await).await;
    pair.sync_a.set_checkpoint_wait(Duration::ZERO);
    pair.sync_b.set_checkpoint_wait(Duration::ZERO);
    let film = noise(6_000_000);
    pair.a.write("Film.mov", &film);
    pair.a.write("Note.md", b"# Beside it\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a refused upload doesn't fail the round");
    both_rounds(&mut pair).await;
    let status = pair.sync_a.status();
    assert_eq!(status.max_file, Some(5_000_000), "free's largest file");
    assert!(
        status.skipped.iter().any(|s| s.path == "Film.mov"
            && s.why == app_core::sync::workspace::SkipWhy::AbovePlan),
        "{:?}",
        status.skipped
    );
    assert_eq!(status.pending, 0, "nothing waits in the outbox");
    assert!(pair.b.path("Note.md").exists(), "the note went");
    assert!(!pair.b.path("Film.mov").exists(), "the film didn't");
    assert_eq!(
        pair.a.files().get("Film.mov").map(Vec::len),
        Some(film.len()),
        "the file stays as it was"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_workspace_over_plan_keeps_changes_until_it_is_back() {
    let mut pair = pair_on(Server::start_with_billing().await).await;
    pair.sync_a.set_checkpoint_wait(Duration::ZERO);
    pair.sync_b.set_checkpoint_wait(Duration::ZERO);
    pair.a.write("First.md", b"# First\n");
    both_rounds(&mut pair).await;
    assert!(pair.b.path("First.md").exists());

    set_over_plan(&pair, true).await;
    pair.a.write("Later.md", b"# Later\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("over plan doesn't fail the round");
    let status = pair.sync_a.status();
    assert!(status.over_plan, "the status says so");
    assert_eq!(status.pending, 1, "the change waits in the outbox");
    // A round within the minute sends nothing.
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a quiet round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
    assert!(!pair.b.path("Later.md").exists());
    assert!(pair.a.path("Later.md").exists(), "the file stays");

    // Back within the plan, the change goes out.
    set_over_plan(&pair, false).await;
    pair.sync_a.recheck_plan();
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round that reads the plan");
    both_rounds(&mut pair).await;
    assert!(!pair.sync_a.status().over_plan);
    assert_eq!(pair.sync_a.status().pending, 0);
    assert!(pair.b.path("Later.md").exists(), "it went out");
}
