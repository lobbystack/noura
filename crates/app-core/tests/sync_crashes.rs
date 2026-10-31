//! What a failed write or a killed app leaves behind on a client
//! (sync-v2.md §15): a page counts only once its file changes reached disk,
//! and a restart picks up from what the journal and the files say.

mod sync_support;

use std::os::unix::fs::PermissionsExt;

use app_core::sync::workspace::WorkspaceSync;
use app_core::sync::workspace::text::Entered;
use editor_core::text_mode::{CollabDocument, Utf16Splice};
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

const MANAGED: &[u8] = b"---\nid: note_01j9z3y7q8r2s4t6v8w0x2y4z6\ntype: note\n---\n# Plan\n";

/// A kill after an outside edit ended a note's text mode and before the
/// journal saved it: the note keeps syncing, and the edit still goes out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kill_as_text_mode_ends_keeps_the_note_syncing() {
    let mut pair = pair().await;
    pair.a.write("Note.md", MANAGED);
    rounds(&mut pair).await;
    let (entered, document) = pair.sync_a.open_text("Note.md").expect("the note opens");
    assert_eq!(entered, Entered::Yes);
    drop(document);
    let rid = pair.sync_a.text_rid("Note.md").expect("the note");
    pair.sync_a.close_text_note(rid).expect("it closes");
    rounds(&mut pair).await;

    // With no frontmatter the file stops fitting the document.
    pair.a.write("Note.md", b"# Plan without frontmatter\n");
    pair.sync_a.lose_saves();
    assert!(pair.sync_a.capture().is_err());

    pair.sync_a = laptop_reopened(&pair);
    rounds(&mut pair).await;
    assert_eq!(pair.b.files()["Note.md"], b"# Plan without frontmatter\n");
    assert!(!pair.sync_a.in_text_mode(&rid));

    pair.a.write("Note.md", b"# Plan, later\n");
    rounds(&mut pair).await;
    assert_eq!(pair.b.files()["Note.md"], b"# Plan, later\n");
    assert_eq!(pair.sync_a.status().conflicts, 0);
}

/// A kill after a text update reached the note's journal file and before
/// the outbox saved it: the update still goes out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kill_after_a_text_update_still_sends_it() {
    let mut pair = pair().await;
    let (entered, document) = pair.sync_a.open_text("Plan.md").expect("the note opens");
    assert_eq!(entered, Entered::Yes);
    let mut document = document.expect("the document");
    rounds(&mut pair).await;
    document.edit(
        vec![Utf16Splice {
            start: 0,
            removed: 0,
            inserted: "Hello ".to_owned(),
        }],
        true,
    );
    let rid = pair.sync_a.text_rid("Plan.md").expect("the note");
    pair.sync_a.send_text_for_testing(rid).expect("signed");
    pair.sync_a.lose_saves();
    drop(document);

    pair.sync_a = laptop_reopened(&pair);
    rounds(&mut pair).await;
    let plan = String::from_utf8(pair.b.files()["Plan.md"].clone()).unwrap();
    assert!(plan.contains("Hello "), "{plan}");
    assert_eq!(pair.sync_a.status().pending, 0);
}

/// A kill after a snapshot reached the note's journal file and before the
/// outbox saved it leaves no snapshot that seems on its way: a `file`
/// operation still ends the note's text mode here.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_kill_after_a_text_snapshot_lets_the_lineage_end() {
    let mut pair = pair().await;
    pair.a.write("Note.md", MANAGED);
    rounds(&mut pair).await;
    let (entered, document) = pair.sync_a.open_text("Note.md").expect("the note opens");
    assert_eq!(entered, Entered::Yes);
    drop(document);
    rounds(&mut pair).await;
    let (entered, document) = pair.sync_b.open_text("Note.md").expect("the note opens");
    assert_eq!(entered, Entered::Yes);
    drop(document);
    let rid = pair.sync_a.text_rid("Note.md").expect("the note");
    pair.sync_b.close_text_note(rid).expect("it closes");
    rounds(&mut pair).await;

    pair.sync_a
        .write_snapshot_for_testing(rid)
        .expect("a snapshot");
    pair.sync_a.lose_saves();
    pair.sync_a = laptop_reopened(&pair);

    pair.b.write("Note.md", b"# Plan without frontmatter\n");
    rounds(&mut pair).await;
    assert!(!pair.sync_a.in_text_mode(&rid));
    assert_eq!(pair.a.files()["Note.md"], b"# Plan without frontmatter\n");
}

fn laptop_reopened<B: Backend>(pair: &Pair<B>) -> WorkspaceSync {
    WorkspaceSync::open(
        pair.a.engine.clone(),
        pair.laptop.credentials.clone(),
        pair.laptop.secrets(),
        &pair.a.registry,
    )
    .expect("the journal opens")
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
