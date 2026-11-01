//! Retention, compaction and quota in the desktop client against the real
//! server (seam 4, sync-v2.md §15, §18, §19, §20, §30).

mod sync_support;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use app_core::sync::SyncError;
use app_core::sync::recovery::{Recovery, open_kit};
use app_core::sync::workspace::WorkspaceSync;
use app_core::sync::workspace::rebuild::HeldChoice;
use app_core::sync::workspace::text::Entered;
use sync_protocol::codec::{Bytes, Decode, Encode, Sorted, Text};
use sync_protocol::messages::{CursorCompactedDetail, ErrorBody, Page, PageStatus, PullResponse};
use sync_store::{Backend, Retention};
use sync_support::{Folder, MemorySecrets, Pair, Proxy, Server, approve, cut_off, pair_on};

const ENTRIES: u64 = 4;

fn keep_little() -> Retention {
    Retention {
        retained_bytes: 1,
        ..Retention::default()
    }
}

fn quota(bytes: u64) -> Retention {
    Retention {
        quota_bytes: Some(bytes),
        ..Retention::default()
    }
}

/// `len` bytes that don't compress, rounded to 32.
fn noise(len: u32) -> Vec<u8> {
    (0..len / 32)
        .flat_map(|i| sync_protocol::crypto::hash(&i.to_be_bytes()).0)
        .collect()
}

async fn ready<B: Backend>(server: Server<B>) -> Pair<B> {
    let mut pair = pair_on(server).await;
    pair.sync_a.set_checkpoint_wait(Duration::ZERO);
    pair.sync_b.set_checkpoint_wait(Duration::ZERO);
    pair
}

/// Writes on the desktop alone for `rounds` rounds, with the server's
/// background pass after each, so the floor walks away from the laptop.
async fn walk_the_floor<B: Backend>(pair: &mut Pair<B>, prefix: &str, rounds: usize) {
    for i in 0..rounds {
        pair.b
            .write(&format!("{prefix}{i}.md"), format!("# {i}\n").as_bytes());
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round on the desktop");
        pair.server.store.maintain().await.expect("a pass");
    }
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

async fn floor_of<B: Backend>(pair: &Pair<B>) -> u64 {
    let (workspace, domain) = (pair.sync_b.workspace(), pair.sync_b.domain());
    pair.server
        .store
        .backend()
        .read(Box::new(move |txn: &mut B::Read| {
            Box::pin(async move {
                use sync_store::ReadTxn;
                txn.domain(workspace, domain).await
            })
        }))
        .await
        .expect("read")
        .expect("a head row")
        .retention_floor
}

/// A connection that drops in the middle of a returning device's bootstrap
/// leaves buckets it hasn't reached. The next attempt finds them through
/// their hashes, so edits in them still arrive, and only then does the
/// cursor move.
async fn a_returning_bootstrap_resumes_past_a_drop<B: Backend>(server: Server<B>) {
    let mut pair = ready(server).await;
    const NOTES: usize = 12;
    for i in 0..NOTES {
        pair.a.write(&format!("Note{i}.md"), b"# Before\n");
    }
    both_rounds(&mut pair).await;

    for i in 0..NOTES {
        pair.b.write(&format!("Note{i}.md"), b"# Edited away\n");
    }
    walk_the_floor(&mut pair, "away", 16).await;
    assert!(floor_of(&pair).await > 0, "the floor moved");

    // The third object fetched never comes.
    let fetched = Arc::new(AtomicUsize::new(0));
    let counted = fetched.clone();
    pair.proxy.forge(Box::new(move |_, path| {
        (path.ends_with("/live") && counted.fetch_add(1, Ordering::SeqCst) >= 2)
            .then(|| (503, Vec::new()))
    }));
    assert!(
        pair.sync_a.round(&mut pair.service, 0).await.is_err(),
        "the connection dropped"
    );
    let edited = |pair: &Pair<B>| {
        let files = pair.a.files();
        (0..NOTES)
            .filter(|i| files[&format!("Note{i}.md")] == b"# Edited away\n")
            .count()
    };
    let before = edited(&pair);
    assert!(before < NOTES, "some buckets wait: {before} arrived");
    pair.proxy.honest();

    for _ in 0..3 {
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("the laptop resumes");
    }
    assert_eq!(edited(&pair), NOTES, "the edits in the other buckets came");
    assert_eq!(pair.a.files(), pair.b.files());
    assert!(pair.sync_a.checkpoint_failures().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_returning_bootstrap_resumes_past_a_dropped_connection() {
    a_returning_bootstrap_resumes_past_a_drop(
        Server::start_with_retention(ENTRIES, keep_little()).await,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_returning_bootstrap_resumes_past_a_dropped_connection_on_postgres() {
    a_returning_bootstrap_resumes_past_a_drop(
        Server::start_postgres_with_retention(ENTRIES, keep_little()).await,
    )
    .await;
}

/// A device whose cursor fell below the floor loads what it lacks and
/// nothing more: each bucket that differs comes down through its leaf refs
/// and `live` with `rid`, and no `live` page lists the whole domain (§18).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_returning_device_downloads_only_what_it_lacks() {
    let mut pair = ready(Server::start_with_retention(ENTRIES, keep_little()).await).await;
    pair.a.write("Keep.md", b"# Keep\n");
    pair.a.write("Edit.md", b"# Edit\n");
    both_rounds(&mut pair).await;

    pair.b.write("Edit.md", b"# Edited away\n");
    walk_the_floor(&mut pair, "away", 16).await;
    let laptop_cursor_floor = floor_of(&pair).await;
    assert!(laptop_cursor_floor > 0, "the floor moved");

    let seen = pair.proxy.requests().len();
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop returns");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("and settles");
    assert_eq!(pair.a.files(), pair.b.files());
    assert!(pair.sync_a.checkpoint_failures().is_empty());
    let paths: Vec<String> = pair.proxy.requests()[seen..]
        .iter()
        .map(|r| r.path.clone())
        .collect();
    assert!(
        paths.iter().any(|p| p.contains("level=3")),
        "it read the leaf refs of the buckets that differ: {paths:?}"
    );
    assert!(
        paths
            .iter()
            .filter(|p| p.contains("/live?"))
            .all(|p| p.contains("rid=")),
        "it fetched objects by rid: {paths:?}"
    );
    let fetched = paths.iter().filter(|p| p.contains("/live?")).count();
    assert!(
        fetched <= 18,
        "it fetched what the desktop wrote, not the unchanged notes: {fetched}"
    );
}

/// A returning device never deletes a file because the manifest lacks its
/// object: it lists the file once and deletes it only when you confirm, and
/// a changed copy uploads under a new `rid` (§18).
/// The checkpoint a returning device lands on names a policy revision the
/// device hasn't seen (§18), found by the soak.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_returning_device_catches_up_on_the_policy_its_checkpoint_names() {
    let mut pair = ready(Server::start_with_retention(ENTRIES, keep_little()).await).await;
    let _third = approve(
        &pair.server,
        &mut pair.desktop,
        std::slice::from_mut(pair.sync_b.remote_mut().expect("connected")),
    )
    .await;
    walk_the_floor(&mut pair, "Away ", 12).await;
    let before = pair.sync_a.policy_revision();
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop returns");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop settles");
    assert!(pair.sync_a.policy_revision() > before);
    assert_eq!(pair.a.files(), pair.b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_deleted_while_away_waits_for_you() {
    let mut pair = ready(Server::start_with_retention(ENTRIES, keep_little()).await).await;
    pair.a.write("Gone.md", b"# Gone\n");
    pair.a.write("Changed.md", b"# Changed\n");
    both_rounds(&mut pair).await;

    // Deleted on the desktop, and the delete leaves the manifest for good
    // once a checkpoint's floor passes it.
    std::fs::remove_file(pair.b.path("Gone.md")).expect("removed");
    std::fs::remove_file(pair.b.path("Changed.md")).expect("removed");
    walk_the_floor(&mut pair, "away", 30).await;
    // The laptop changes one of them meanwhile, offline.
    pair.a.write("Changed.md", b"# Changed here\n");

    for _ in 0..3 {
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("the laptop returns");
    }
    assert!(pair.a.path("Gone.md").exists(), "the file stays");
    assert_eq!(pair.sync_a.deleted_away(), ["Gone.md"]);
    assert_eq!(pair.sync_a.status().deleted_away, 1);

    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
    assert!(
        pair.b.path("Changed.md").exists(),
        "a changed copy uploads as a new object"
    );
    assert!(
        !pair.b.path("Gone.md").exists(),
        "a file that waits for you stays off the other devices"
    );

    pair.sync_a
        .confirm_deleted_away("Gone.md")
        .expect("confirmed");
    assert!(!pair.a.path("Gone.md").exists());
    assert!(pair.sync_a.deleted_away().is_empty());
    both_rounds(&mut pair).await;
    assert!(!pair.b.path("Gone.md").exists());
}

/// A bootstrapping device downloads a blob only when it holds no file at
/// the payload's revision (§18): a move on another device costs a returning
/// device no download.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_returning_device_downloads_no_blob_it_holds() {
    let mut pair = ready(Server::start_with_retention(ENTRIES, keep_little()).await).await;
    let attachment: Vec<u8> = (0..400 * 1024u32 / 32)
        .flat_map(|i| sync_protocol::crypto::hash(&i.to_be_bytes()).0)
        .collect();
    pair.a.write("Photo.bin", &attachment);
    both_rounds(&mut pair).await;
    assert_eq!(pair.b.files()["Photo.bin"], attachment);

    std::fs::create_dir_all(pair.b.path("Moved")).expect("a folder");
    std::fs::rename(pair.b.path("Photo.bin"), pair.b.path("Moved/Photo.bin")).expect("moved");
    walk_the_floor(&mut pair, "away", 16).await;
    let seen = pair.proxy.requests().len();
    for _ in 0..2 {
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("the laptop returns");
    }
    assert_eq!(pair.a.files()["Moved/Photo.bin"], attachment);
    let downloads = pair.proxy.requests()[seen..]
        .iter()
        .filter(|r| r.method == "GET" && r.path.contains("/blobs/"))
        .count();
    assert_eq!(downloads, 0, "the file was already here");
}

/// Rewrites every pull page through `proxy`.
fn tamper_pulls(proxy: &Proxy, change: impl Fn(&mut Page) + Send + Sync + 'static) {
    proxy.tamper(Box::new(move |path, body| {
        if !path.ends_with("/pull") {
            return body;
        }
        let mut response = PullResponse::from_bytes(&body).expect("a pull response");
        for page in response.pages.0.iter_mut() {
            change(page);
        }
        response.to_bytes()
    }));
}

fn compacted(page: &mut Page, floor: u64) {
    page.status = PageStatus::Compacted;
    page.complete_through = page.after;
    page.head = 0;
    page.retention_floor = floor;
    page.checkpoint_due = 0;
    page.entries = Sorted::default();
}

async fn expect_rollback<B: Backend>(pair: &mut Pair<B>, what: &str) {
    let files = pair.a.files();
    let error = pair
        .sync_a
        .round(&mut pair.service, 0)
        .await
        .expect_err(what);
    assert!(
        matches!(error, SyncError::Check("sync_manifest_rollback")),
        "{error:?}"
    );
    assert_eq!(pair.a.files(), files, "no file changed");
    pair.proxy.honest();
}

/// The tampering proxy (§30): a floor below one reported, a claimed
/// compaction of a cursor at or above the floor, and a bootstrap forced
/// from a checkpoint below the cursor all report
/// `sync_manifest_rollback`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_server_that_lowers_the_floor_or_claims_compaction_is_caught() {
    let mut pair = ready(Server::start_with_retention(ENTRIES, keep_little()).await).await;
    walk_the_floor(&mut pair, "first", 10).await;
    both_rounds(&mut pair).await;
    walk_the_floor(&mut pair, "second", 4).await;
    both_rounds(&mut pair).await;
    assert!(floor_of(&pair).await > 0);

    tamper_pulls(&pair.proxy, |page| page.retention_floor = 0);
    expect_rollback(&mut pair, "a lower floor").await;

    // The cursor isn't below the floor the page names.
    tamper_pulls(&pair.proxy, |page| {
        let floor = page.after;
        compacted(page, floor);
    });
    expect_rollback(&mut pair, "a claim at the floor").await;

    // The page claims a floor past the cursor, so the device must bootstrap
    // from a checkpoint that lies below its cursor.
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the laptop passes the latest checkpoint");
    tamper_pulls(&pair.proxy, |page| {
        let floor = page.after + 1;
        compacted(page, floor);
    });
    expect_rollback(&mut pair, "a forced bootstrap").await;

    both_rounds(&mut pair).await;
    assert_eq!(pair.a.files(), pair.b.files());
}

/// The body of a `cursor_compacted` that names `floor` and `latest`.
fn compacted_body(floor: u64, latest: u64) -> Vec<u8> {
    ErrorBody {
        code: Text::new("cursor_compacted").unwrap(),
        message: Text::new("compacted").unwrap(),
        retry_after_seconds: None,
        detail: Some(Bytes(
            CursorCompactedDetail {
                retention_floor: floor,
                checkpoint_covered_seq: latest,
            }
            .to_bytes(),
        )),
    }
    .to_bytes()
}

/// `cursor_compacted` from `live` and `manifest` passes the checks a pull
/// page does (§26, §30): a floor below one reported, a claim about a
/// checkpoint the server still serves, a latest checkpoint below the one
/// the device is installing, and a floor past the real latest checkpoint
/// all report `sync_manifest_rollback`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_false_compaction_from_live_or_manifest_is_caught() {
    a_false_compaction_is_caught(Server::start_with_retention(ENTRIES, keep_little()).await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_false_compaction_from_live_or_manifest_is_caught_on_postgres() {
    a_false_compaction_is_caught(
        Server::start_postgres_with_retention(ENTRIES, keep_little()).await,
    )
    .await;
}

async fn a_false_compaction_is_caught<B: Backend>(server: Server<B>) {
    let mut pair = ready(server).await;
    pair.a.write("Note.md", b"# Before\n");
    both_rounds(&mut pair).await;
    walk_the_floor(&mut pair, "first", 10).await;
    both_rounds(&mut pair).await;
    let reported = floor_of(&pair).await;
    assert!(reported > 0);
    pair.b.write("Note.md", b"# Edited away\n");
    walk_the_floor(&mut pair, "second", 16).await;
    assert!(floor_of(&pair).await > reported, "the laptop fell behind");

    let (workspace, domain) = (pair.sync_b.workspace(), pair.sync_b.domain());
    let latest = pair
        .desktop
        .service
        .client()
        .checkpoint(workspace, domain)
        .await
        .expect("read")
        .expect("a checkpoint")
        .bundle
        .checkpoint
        .0
        .body
        .covered_seq;
    let forge = |pair: &Pair<B>, route: &'static str, floor, claimed| {
        pair.proxy.forge(Box::new(move |_, path| {
            path.ends_with(route)
                .then(|| (410, compacted_body(floor, claimed)))
        }));
    };
    for route in ["/manifest", "/live"] {
        forge(&pair, route, reported - 1, latest);
        expect_rollback(&mut pair, "a lower floor").await;
        forge(&pair, route, latest, latest);
        expect_rollback(&mut pair, "a checkpoint it still serves").await;
        forge(&pair, route, latest + 1, latest - 1);
        expect_rollback(&mut pair, "a latest below the one in use").await;
    }
    // The device restarts on this floor, and the real latest checkpoint
    // then falls below it.
    forge(&pair, "/live", latest + 1, latest + 1);
    expect_rollback(&mut pair, "a floor past the real latest").await;

    // Once the server answers honestly, the device catches up.
    both_rounds(&mut pair).await;
    both_rounds(&mut pair).await;
    assert_eq!(pair.a.files(), pair.b.files());
}

/// A writer whose every cover names a pruned operation can't sign an edit
/// that leaves the version it never saw live (§14, §19), so its edit waits
/// for your choice and overwrites nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_cover_of_a_pruned_operation_goes_on_supersedes_unknown() {
    let mut pair = ready(Server::start_with_retention(ENTRIES, keep_little()).await).await;
    pair.a.write("Note.md", b"# Note\n");
    both_rounds(&mut pair).await;

    // Offline, the laptop signs an edit that covers what it holds.
    pair.a.write("Note.md", b"# Note from the laptop\n");
    cut_off(&pair.proxy);
    assert!(pair.sync_a.round(&mut pair.service, 0).await.is_err());
    assert_eq!(pair.sync_a.status().pending, 1);
    pair.proxy.honest();

    // The desktop edits the note again and again until pruning takes the
    // operation the laptop covered.
    for i in 0..24 {
        pair.b
            .write("Note.md", format!("# Desktop {i}\n").as_bytes());
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round on the desktop");
        pair.server.store.maintain().await.expect("a pass");
    }
    for _ in 0..3 {
        both_rounds(&mut pair).await;
    }
    assert_eq!(pair.sync_a.status().pending, 0, "nothing waits to go out");
    assert_eq!(pair.a.files()["Note.md"], b"# Note from the laptop\n");
    assert_eq!(pair.b.files()["Note.md"], b"# Desktop 23\n");
    let (rid, path) = pair.sync_a.held().pop().expect("the edit waits for you");
    assert_eq!(path, "Note.md");
    let store = pair
        .a
        .path(".noura/sync")
        .join(pair.laptop.id().to_hex())
        .join("held")
        .join(rid.to_hex());
    let kept = sync_protocol::crypto::hash(b"# Note from the laptop\n").to_hex();
    assert_eq!(
        std::fs::read(store.join(&kept)).unwrap(),
        b"# Note from the laptop\n",
        "the review store keeps a copy"
    );

    pair.sync_a
        .resolve_held(rid, HeldChoice::KeepMine)
        .expect("you keep this computer's version");
    both_rounds(&mut pair).await;
    assert_eq!(pair.b.files()["Note.md"], b"# Note from the laptop\n");
    assert_eq!(pair.sync_a.status().held, 0);
}

/// An edit the server keeps refusing for space keeps the version it started
/// from. A version that arrived meanwhile stays a conflict, however many
/// times the edit goes back, and neither replaces the other (§16, §20).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_refused_again_and_again_overwrites_no_other_version() {
    let mut pair = ready(Server::start_with_retention(1_000_000, quota(64 * 1024)).await).await;
    let big = noise(40 * 1024);
    pair.a.write("Filler.md", &big);
    pair.a.write("Plan.md", b"# Plan\n");
    both_rounds(&mut pair).await;

    pair.a.write("Plan.md", &big);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a refused record doesn't fail the round");
    pair.b.write("Plan.md", b"# Plan from the desktop\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
    for _ in 0..3 {
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("a refused record doesn't fail the round");
    }
    assert_eq!(pair.sync_a.status().waiting_for_space, 1);

    std::fs::remove_file(pair.a.path("Filler.md")).expect("removed");
    for _ in 0..3 {
        both_rounds(&mut pair).await;
    }
    assert_eq!(pair.a.files()["Plan.md"], big);
    assert_eq!(pair.b.files()["Plan.md"], b"# Plan from the desktop\n");
    assert_eq!(pair.sync_a.status().conflicts, 1);
    assert_eq!(pair.sync_b.status().conflicts, 1);
}

/// A new device that bootstraps past a conflict between a text snapshot and
/// a file holds neither side, and the snapshot's record alone holds no file
/// bytes. It shows a version and the bucket finishes (§16, §18). Eight notes
/// make it near certain that some snapshot has the lowest digest.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrap_shows_a_conflict_of_a_snapshot_and_a_file() {
    let mut pair = ready(Server::start_with_retention(ENTRIES, keep_little()).await).await;
    let notes: Vec<String> = (0..8).map(|i| format!("Note{i}.md")).collect();
    for note in &notes {
        pair.a.write(note, format!("# {note}\n").as_bytes());
    }
    both_rounds(&mut pair).await;
    for note in &notes {
        let (entered, document) = pair.sync_a.open_text(note).expect("the note opens");
        assert_eq!(entered, Entered::Yes);
        drop(document);
        pair.b
            .write(note, format!("# {note} from the desktop\n").as_bytes());
    }
    both_rounds(&mut pair).await;
    assert_eq!(pair.sync_b.status().conflicts, notes.len());
    walk_the_floor(&mut pair, "after", 16).await;
    assert!(floor_of(&pair).await > 0, "the floor moved");

    let mut third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut c = Folder::new("Third");
    let mut sync_c = WorkspaceSync::download(
        &mut third.service,
        c.engine.clone(),
        pair.sync_a.workspace(),
        &mut c.registry,
    )
    .await
    .expect("the download starts");
    for _ in 0..2 {
        sync_c
            .round(&mut third.service, 0)
            .await
            .expect("the conflict doesn't stop the device");
    }
    assert_eq!(sync_c.status().conflicts, notes.len());
    let files = c.files();
    for note in &notes {
        assert!(files.contains_key(note), "{note} shows a version");
    }
}

async fn a_reader_keeps_up_with_old_cursors<B: Backend>(server: Server<B>) {
    let mut pair = ready(server).await;
    for round in 0..5 {
        walk_the_floor(&mut pair, &format!("r{round}"), 6).await;
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("the old cursor stays correct");
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("a second round");
        assert_eq!(pair.a.files(), pair.b.files(), "after walk {round}");
    }
    assert!(pair.sync_a.checkpoint_failures().is_empty());
}

/// A device that keeps pulling with an old cursor while the floor moves
/// stays correct (§32).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_with_an_old_cursor_stays_correct() {
    a_reader_keeps_up_with_old_cursors(Server::start_with_retention(ENTRIES, keep_little()).await)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_with_an_old_cursor_stays_correct_on_postgres() {
    a_reader_keeps_up_with_old_cursors(
        Server::start_postgres_with_retention(ENTRIES, keep_little()).await,
    )
    .await;
}

async fn the_floor_overtakes_a_bootstrap<B: Backend>(server: Server<B>) {
    let mut pair = ready(server).await;
    for i in 0..6 {
        pair.a.write(&format!("Stays{i}.md"), b"# Stays\n");
    }
    both_rounds(&mut pair).await;
    pair.b.write("Plan.md", b"# Plan, changed\n");
    walk_the_floor(&mut pair, "before", 16).await;
    let before = floor_of(&pair).await;

    // The laptop reaches its first leaf or live request, and the floor
    // passes its checkpoint meanwhile.
    let (arrived, release) = pair.proxy.hold("/live");
    let Pair {
        server,
        service,
        sync_a,
        sync_b,
        desktop,
        b,
        proxy,
        ..
    } = &mut pair;
    let held = async {
        arrived.notified().await;
        for i in 0..16 {
            b.write(&format!("during{i}.md"), b"# During\n");
            sync_b
                .round(&mut desktop.service, 0)
                .await
                .expect("a round on the desktop");
            server.store.maintain().await.expect("a pass");
        }
        release.notify_one();
    };
    let (returned, ()) = tokio::join!(sync_a.round(service, 0), held);
    let _ = proxy;
    returned.expect("the bootstrap restarts from the latest checkpoint");
    assert!(floor_of(&pair).await > before);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.a.files(), pair.b.files());
    let answered: Vec<String> = pair
        .proxy
        .requests()
        .iter()
        .filter(|r| r.path.contains("/live?") && !r.path.contains("rid="))
        .map(|r| r.path.clone())
        .collect();
    assert!(
        answered.is_empty(),
        "a restart keeps the buckets whose hashes match: {answered:?}"
    );
    let hashes = pair
        .proxy
        .requests()
        .iter()
        .filter(|r| r.path.contains("buckets=all"))
        .count();
    assert!(hashes >= 2, "it read the hashes of a later checkpoint too");
}

/// A bootstrap whose checkpoint falls below the floor before it ends
/// restarts from the latest checkpoint, keeping the buckets whose hashes
/// match (§18).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrap_the_floor_overtakes_restarts() {
    the_floor_overtakes_a_bootstrap(Server::start_with_retention(ENTRIES, keep_little()).await)
        .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_bootstrap_the_floor_overtakes_restarts_on_postgres() {
    the_floor_overtakes_a_bootstrap(
        Server::start_postgres_with_retention(ENTRIES, keep_little()).await,
    )
    .await;
}

/// A record that gets `quota_exceeded` stays local, the next record takes
/// its position, the change goes out once space frees up, and the status
/// shows usage against the quota (§14, §20).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_over_quota_waits_until_space_frees_up() {
    let mut pair = ready(Server::start_with_retention(1_000_000, quota(64 * 1024)).await).await;
    let big = noise(40 * 1024);
    pair.a.write("Filler.md", &big);
    both_rounds(&mut pair).await;
    let usage = pair.sync_a.status().usage.expect("the server reported it");
    assert_eq!(usage.quota, Some(64 * 1024));
    assert!(usage.used > 40 * 1024);

    // In one round: a note too big for what's left, then a small one.
    pair.a.write("A big note.md", &big);
    pair.a.write("B small note.md", b"# Small\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a refused record doesn't fail the round");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a second round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
    assert!(
        pair.b.path("B small note.md").exists(),
        "the next record went"
    );
    assert!(!pair.b.path("A big note.md").exists(), "the big one waits");
    assert!(pair.a.path("A big note.md").exists(), "and stays local");
    let status = pair.sync_a.status();
    assert_eq!(status.pending, 0, "no record waits in the outbox");
    assert_eq!(status.waiting_for_space, 1, "the status says what waits");

    // A delete commits and frees its usage.
    std::fs::remove_file(pair.a.path("Filler.md")).expect("removed");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the delete commits over quota");
    for _ in 0..2 {
        both_rounds(&mut pair).await;
    }
    assert_eq!(
        pair.b.files().get("A big note.md").map(Vec::len),
        Some(big.len()),
        "the change went out once space freed up"
    );
    assert_eq!(pair.sync_a.status().pending, 0);
    assert_eq!(pair.sync_a.status().waiting_for_space, 0);
}

/// A change undone while it waits for space waits for nothing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_waiting_change_you_take_back_stops_waiting() {
    let mut pair = ready(Server::start_with_retention(1_000_000, quota(64 * 1024)).await).await;
    let big = noise(40 * 1024);
    pair.a.write("Filler.md", &big);
    both_rounds(&mut pair).await;
    pair.a.write("A big note.md", &big);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a refused record doesn't fail the round");
    assert_eq!(pair.sync_a.status().waiting_for_space, 1);

    std::fs::remove_file(pair.a.path("A big note.md")).expect("removed");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.sync_a.status().waiting_for_space, 0);
}

/// An attachment that doesn't fit is refused at its tus creation. The
/// change waits like a refused record, so the records behind it still go
/// out, and a delete among them frees the space.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_attachment_over_quota_leaves_the_outbox_moving() {
    let mut pair = ready(Server::start_with_retention(1_000_000, quota(512 * 1024)).await).await;
    pair.a.write("Filler.bin", &noise(300 * 1024));
    both_rounds(&mut pair).await;

    pair.a.write("Photo.bin", &noise(400 * 1024));
    pair.a.write("Note.md", b"# Behind it\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a refused upload doesn't fail the round");
    assert_eq!(pair.sync_a.status().waiting_for_space, 1);
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
    assert!(pair.b.path("Note.md").exists(), "the next record went");
    assert!(!pair.b.path("Photo.bin").exists());

    std::fs::remove_file(pair.a.path("Filler.bin")).expect("removed");
    for _ in 0..3 {
        both_rounds(&mut pair).await;
    }
    pair.server.store.maintain().await.expect("a pass");
    for _ in 0..2 {
        both_rounds(&mut pair).await;
    }
    assert!(
        pair.b.path("Photo.bin").exists(),
        "it went out once space freed"
    );
    assert_eq!(pair.sync_a.status().waiting_for_space, 0);
}

async fn over_quota_device_changes_go_through<B: Backend>(server: Server<B>) {
    use sync_support::first_desktop;
    let mut laptop = first_desktop(&server, "ana@example.com").await;
    let mut a = Folder::new("Field notes");
    a.write("Plan.md", &vec![b'x'; 2 * 1024]);
    let mut sync_a = WorkspaceSync::turn_on(
        &mut laptop.service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.set_checkpoint_wait(Duration::ZERO);
    let workspace = sync_a.workspace();
    sync_a.round(&mut laptop.service, 0).await.expect("a round");

    // A kit from before the workspace filled up.
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
    sync_a.round(&mut laptop.service, 0).await.expect("a round");
    for i in 0..40 {
        a.write(&format!("Note{i}.md"), &vec![b'y'; 8 * 1024]);
        sync_a.round(&mut laptop.service, 0).await.expect("a round");
    }
    let usage = laptop.service.client().workspaces().await.expect("listed");
    let info = &usage.workspaces[0];
    let quota = info.quota_bytes.expect("a quota");
    assert!(info.used_bytes + 9 * 1024 > quota, "the workspace is full");

    // A lost device's replacement goes through.
    let replacement = approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut b = Folder::new("Replacement");
    let mut desktop = replacement;
    let mut sync_b = WorkspaceSync::download(
        &mut desktop.service,
        b.engine.clone(),
        workspace,
        &mut b.registry,
    )
    .await
    .expect("the download starts");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("the replacement downloads");
    assert!(b.files().contains_key("Plan.md"));

    // So does a recovery with the kit.
    let kit = open_kit(&path, export.secret.as_ref()).expect("the kit opens");
    let signed_in = server
        .sign_in(laptop.credentials.account, &server.base)
        .await;
    let secrets = Arc::new(MemorySecrets::default());
    let state = tempfile::tempdir().unwrap();
    let recovery = Recovery::begin(&signed_in, kit, secrets.clone(), state.path())
        .await
        .expect("the kit's identity proves");
    let (mut service, _, _) = recovery.finish(&[], &*secrets).await.expect("recovered");
    let mut c = Folder::new("Recovered");
    let mut sync_c =
        WorkspaceSync::download(&mut service, c.engine.clone(), workspace, &mut c.registry)
            .await
            .expect("the download starts");
    sync_c
        .round(&mut service, 0)
        .await
        .expect("the recovered device downloads");
    assert!(c.files().contains_key("Plan.md"));
}

/// A workspace over its quota still takes a lost device's replacement and a
/// recovery with a kit (§20).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_quota_a_replacement_and_a_kit_still_work() {
    over_quota_device_changes_go_through(
        Server::start_with_retention(10_000, quota(120 * 1024)).await,
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn over_quota_a_replacement_and_a_kit_still_work_on_postgres() {
    over_quota_device_changes_go_through(
        Server::start_postgres_with_retention(10_000, quota(120 * 1024)).await,
    )
    .await;
}

async fn usage_alone_passes_the_quota<B: Backend>(server: Server<B>) {
    let mut pair = ready(server).await;
    // Filled through the desktop's own device changes and writes that fit.
    for i in 0..6 {
        pair.a.write(&format!("N{i}.md"), &vec![b'x'; 8 * 1024]);
        both_rounds(&mut pair).await;
    }
    let (workspace, domain) = (pair.sync_a.workspace(), pair.sync_a.domain());
    pair.server
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move {
                use sync_store::WriteTxn;
                txn.add_usage(workspace, domain, 1 << 20, 0).await?;
                txn.set_quota_pressure(workspace, true).await.map(|_| ())
            })
        }))
        .await
        .expect("usage past the quota");
    let floor = floor_of(&pair).await;
    for _ in 0..3 {
        pair.server.store.maintain().await.expect("a pass");
    }
    assert_eq!(floor_of(&pair).await, floor, "floors keep to the rule");
    // A device a checkpoint behind doesn't lose its cursor to the pressure.
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert!(pair.sync_a.checkpoint_failures().is_empty());
}

/// When usage alone passes the quota, history can't help, so the server
/// raises no floor beyond the normal rule (§20).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_alone_over_quota_keeps_the_floors() {
    usage_alone_passes_the_quota(Server::start_with_retention(4, quota(1 << 20)).await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn usage_alone_over_quota_keeps_the_floors_on_postgres() {
    usage_alone_passes_the_quota(Server::start_postgres_with_retention(4, quota(1 << 20)).await)
        .await;
}

/// `retention_floor` in a page carries what the server persisted, and a
/// restart of the client keeps the highest one it saw.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_floor_survives_a_page_without_news() {
    let mut pair = ready(Server::start_with_retention(ENTRIES, keep_little()).await).await;
    walk_the_floor(&mut pair, "walk", 10).await;
    both_rounds(&mut pair).await;
    let floor = floor_of(&pair).await;
    assert!(floor > 0);
    let response = pair
        .service
        .client()
        .pull(
            pair.sync_a.workspace(),
            vec![sync_protocol::messages::PullCursor {
                domain: pair.sync_a.domain(),
                after: floor,
            }],
            0,
        )
        .await
        .expect("a pull");
    assert_eq!(response.pages.0[0].retention_floor, floor);
    let _ = response.to_bytes();
}
