//! What a failed write or a killed app leaves behind on a client
//! (sync-v2.md §15): a page counts only once its file changes reached disk,
//! and a restart picks up from what the journal and the files say.

mod sync_support;

use std::os::unix::fs::PermissionsExt;

use app_core::sync::workspace::WorkspaceSync;
use sync_store::Backend;
use sync_support::{Folder, Pair, Proxy, Server, approve, cut_off, first_desktop, pair, rounds};

fn set_read_only(path: &std::path::Path, read_only: bool) {
    let mode = if read_only { 0o555 } else { 0o755 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// A write that fails in the middle of a page leaves the page to come back
/// whole: the file that failed still arrives, and so do the changes after
/// it, once the disk takes them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_write_brings_the_whole_page_back() {
    let mut pair = pair().await;
    pair.a.write("Locked/Old.md", b"# Old\n");
    rounds(&mut pair).await;

    set_read_only(&pair.b.path("Locked"), true);
    pair.a.write("Another.md", b"# Another\n");
    pair.a.write("Locked/New.md", b"# New\n");
    pair.a.write("Plan.md", b"# Plan, edited\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    let failed = pair.sync_b.round(&mut pair.desktop.service, 0).await;
    assert!(failed.is_err(), "the page can't finish: {failed:?}");

    // Another try fails the same way, and costs nothing.
    let failed = pair.sync_b.round(&mut pair.desktop.service, 0).await;
    assert!(failed.is_err(), "the page still can't finish: {failed:?}");

    set_read_only(&pair.b.path("Locked"), false);
    rounds(&mut pair).await;
    assert_eq!(pair.b.files(), pair.a.files());
    assert_eq!(pair.b.files()["Locked/New.md"], b"# New\n");
    assert_eq!(pair.b.files()["Plan.md"], b"# Plan, edited\n");
    assert_eq!(pair.sync_b.status().conflicts, 0);
    assert_eq!(pair.sync_a.status().conflicts, 0);
}

/// The same failure, with a kill before the page comes back: the journal
/// the next start reads names no record it can't finish applying.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failed_write_and_a_restart_bring_the_whole_page_back() {
    let mut pair = pair().await;
    pair.a.write("Locked/Old.md", b"# Old\n");
    rounds(&mut pair).await;

    set_read_only(&pair.b.path("Locked"), true);
    pair.a.write("Locked/New.md", b"# New\n");
    pair.a.write("Plan.md", b"# Plan, edited\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert!(
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .is_err()
    );
    // The next round's capture saves what the failed page left in memory.
    assert!(
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .is_err()
    );
    set_read_only(&pair.b.path("Locked"), false);

    pair.sync_b.lose_saves();
    pair.sync_b = reopened(&pair);
    rounds(&mut pair).await;
    assert_eq!(pair.b.files(), pair.a.files());
    assert_eq!(pair.sync_b.status().conflicts, 0);
}

/// A kill after a page's files reached disk and before the journal saved
/// them leaves files no object maps. The first round after the restart
/// signs none of them as a new object: the page comes back and maps them.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kill_in_a_page_uploads_no_applied_file_as_new() {
    let mut pair = pair().await;
    pair.a.write("New.md", b"# New\n");
    pair.a.write("Another.md", b"# Another\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");

    pair.sync_b.kill_after_page();
    let killed = pair.sync_b.pull(&mut pair.desktop.service, 0).await;
    assert!(killed.is_err(), "the kill: {killed:?}");
    assert_eq!(pair.b.files()["New.md"], b"# New\n");

    pair.sync_b = reopened(&pair);
    // A local file that arrived meanwhile waits no longer than the page.
    pair.b.write("Mine.md", b"# Mine\n");
    rounds(&mut pair).await;
    let names: Vec<String> = pair.a.files().keys().cloned().collect();
    assert_eq!(names, ["Another.md", "Mine.md", "New.md", "Plan.md"]);
    assert_eq!(pair.b.files(), pair.a.files());
    assert_eq!(pair.sync_a.status().conflicts, 0);
    assert_eq!(pair.sync_b.status().conflicts, 0);
    assert_eq!(pair.sync_a.status().pending, 0);
}

/// A download that hasn't reached the server signs nothing for the files
/// the folder holds: each waits to be matched with the workspace's objects.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_download_that_stays_offline_uploads_no_local_file() {
    let server = Server::start().await;
    let proxy = Proxy::start(&server.base).await;
    let mut laptop = first_desktop(&server, "ana@example.com").await;
    let mut service = laptop.connect_through(&proxy.base).await;
    let mut a = Folder::new("Field notes");
    a.write("Plan.md", b"# Plan\n");
    let mut sync_a = WorkspaceSync::turn_on(
        &mut service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.round(&mut service, 0).await.expect("a round");
    let desktop = approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut desktop_service = desktop.connect_through(&proxy.base).await;

    let mut b = Folder::new("Copy");
    let mut sync_b = WorkspaceSync::download(
        &mut desktop_service,
        b.engine.clone(),
        sync_a.workspace(),
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    b.write("Plan.md", b"# Plan\n");
    cut_off(&proxy);
    assert!(sync_b.round(&mut desktop_service, 0).await.is_err());
    assert_eq!(sync_b.status().pending, 0, "nothing waits to go out");
    proxy.honest();

    for _ in 0..2 {
        sync_b
            .round(&mut desktop_service, 0)
            .await
            .expect("a round");
        sync_a.round(&mut service, 0).await.expect("a round");
    }
    let names: Vec<String> = a.files().keys().cloned().collect();
    assert_eq!(names, ["Plan.md"]);
    assert_eq!(b.files(), a.files());
}

fn reopened<B: Backend>(pair: &Pair<B>) -> WorkspaceSync {
    WorkspaceSync::open(
        pair.b.engine.clone(),
        pair.desktop.credentials.clone(),
        pair.desktop.secrets(),
        &pair.b.registry,
    )
    .expect("the journal opens")
}
