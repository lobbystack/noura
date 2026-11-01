//! A round on a device that follows the engine's change events (seam 4,
//! sync-v2.md §28): an edit reaches the other device without either side
//! walking the workspace again, and anything the events can't vouch for
//! falls back to a walk.

mod sync_support;

use std::time::Duration;

use app_core::sync::workspace::WorkspaceSync;
use sync_store::Backend;
use sync_support::{Folder, Pair, pair};

/// Tells `folder`'s engine that `paths` changed, as the native watcher does.
fn heard(folder: &Folder, paths: &[&str]) {
    let root = folder.engine.root().to_path_buf();
    folder
        .engine
        .inject_watch_event(paths.iter().map(|path| root.join(path)).collect());
    folder
        .engine
        .poll_external_changes(Duration::from_millis(1))
        .expect("the watcher reports");
}

/// Both devices follow their engines, and each took its first walk.
async fn following<B: Backend>(pair: &mut Pair<B>) {
    pair.sync_a.follow_changes();
    pair.sync_b.follow_changes();
    both(pair).await;
}

async fn both<B: Backend>(pair: &mut Pair<B>) {
    for _ in 0..2 {
        pair.sync_a.round(&mut pair.service, 0).await.expect("a");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("b");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_reaches_the_other_device_without_a_walk() {
    let mut pair = pair().await;
    following(&mut pair).await;
    let walks = (pair.sync_a.walks(), pair.sync_b.walks());
    pair.a.write("Plan.md", b"# Plan, edited\n");
    heard(&pair.a, &["Plan.md"]);
    both(&mut pair).await;
    assert_eq!(pair.b.files()["Plan.md"], b"# Plan, edited\n");
    assert_eq!((pair.sync_a.walks(), pair.sync_b.walks()), walks);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_round_with_no_news_walks_nothing() {
    let mut pair = pair().await;
    following(&mut pair).await;
    let walks = pair.sync_a.walks();
    both(&mut pair).await;
    assert_eq!(pair.sync_a.walks(), walks);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_deleted_file_needs_no_walk_and_a_new_one_takes_one() {
    let mut pair = pair().await;
    pair.a.write("Other.md", b"# Other\n");
    following(&mut pair).await;
    let walks = pair.sync_a.walks();
    std::fs::remove_file(pair.a.path("Other.md")).unwrap();
    heard(&pair.a, &["Other.md"]);
    both(&mut pair).await;
    assert!(!pair.b.files().contains_key("Other.md"));
    assert_eq!(pair.sync_a.walks(), walks);

    // The ignore rules decide on a new file, and only a walk applies them.
    pair.a.write("New.md", b"# New\n");
    heard(&pair.a, &["New.md"]);
    both(&mut pair).await;
    assert_eq!(pair.b.files()["New.md"], b"# New\n");
    assert_eq!(pair.sync_a.walks(), walks + 1);
}

/// A new file that waited for a page takes a walk once the page ends, with
/// no event left to name it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_that_waited_for_a_page_goes_out_without_the_periodic_walk() {
    let mut pair = pair().await;
    following(&mut pair).await;
    pair.a.write("New.md", b"# New\n");
    heard(&pair.a, &["New.md"]);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_b.kill_after_page();
    let killed = pair.sync_b.pull(&mut pair.desktop.service, 0).await;
    assert!(killed.is_err(), "the kill: {killed:?}");

    pair.sync_b = WorkspaceSync::open(
        pair.b.engine.clone(),
        pair.desktop.credentials.clone(),
        pair.desktop.secrets(),
        &pair.b.registry,
    )
    .expect("the journal opens");
    pair.sync_b.follow_changes();
    pair.sync_b
        .set_walk_every(Duration::from_secs(u64::MAX >> 8));
    pair.b.write("Mine.md", b"# Mine\n");
    pair.sync_b.capture().expect("a capture");
    both(&mut pair).await;
    both(&mut pair).await;
    both(&mut pair).await;
    assert_eq!(pair.a.files()["Mine.md"], b"# Mine\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_folder_takes_a_walk() {
    let mut pair = pair().await;
    pair.a.write("Trip/One.md", b"# One\n");
    pair.a.write("Trip/Two.md", b"# Two\n");
    following(&mut pair).await;
    assert!(pair.b.files().contains_key("Trip/One.md"));
    std::fs::remove_dir_all(pair.a.path("Trip")).unwrap();
    heard(&pair.a, &["Trip"]);
    both(&mut pair).await;
    assert!(!pair.b.files().contains_key("Trip/One.md"));
    assert!(!pair.b.files().contains_key("Trip/Two.md"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_no_event_named_arrives_with_the_periodic_walk() {
    let mut pair = pair().await;
    following(&mut pair).await;
    pair.a.write("Plan.md", b"# Plan, unheard\n");
    both(&mut pair).await;
    assert_ne!(pair.b.files()["Plan.md"], b"# Plan, unheard\n");

    pair.sync_a.set_walk_every(Duration::ZERO);
    both(&mut pair).await;
    assert_eq!(pair.b.files()["Plan.md"], b"# Plan, unheard\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn events_lost_to_a_full_queue_take_a_walk() {
    let mut pair = pair().await;
    following(&mut pair).await;
    let walks = pair.sync_a.walks();
    pair.a.write("Plan.md", b"# Plan, in a burst\n");
    // More reports than the engine's event queue holds.
    for _ in 0..300 {
        heard(&pair.a, &["Plan.md"]);
    }
    both(&mut pair).await;
    assert_eq!(pair.b.files()["Plan.md"], b"# Plan, in a burst\n");
    assert_eq!(pair.sync_a.walks(), walks + 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_that_doesnt_follow_walks_each_round() {
    let mut pair = pair().await;
    let walks = pair.sync_a.walks();
    both(&mut pair).await;
    assert_eq!(pair.sync_a.walks(), walks + 2);
}
