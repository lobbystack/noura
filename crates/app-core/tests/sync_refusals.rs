//! Refusals and the outbox at seam 4 (sync-v2.md §14, §20, §21): a record
//! the server refuses never stops a round before its pull, never leaves
//! the outbox's head stuck, and never loses the change it carried.

mod sync_support;

use std::time::Duration;

use app_core::sync::invitations::InvitationBook;
use app_core::sync::members::MemberChange;
use app_core::sync::workspace::rebuild::HeldChoice;
use app_core::sync::workspace::text::Entered;
use app_core::sync::workspace::{SkipWhy, WorkspaceSync};
use sync_protocol::codec::{Bytes, Encode, Text};
use sync_protocol::messages::{ErrorBody, PushFailureDetail};
use sync_protocol::records::Role;
use sync_protocol::types::Id;
use sync_store::Backend;
use sync_store_sqlite::SqliteBackend;
use sync_support::{Desktop, Folder, Pair, Proxy, Server, first_desktop, invite_and_join, pair};

const PLAN: &str = "Plan.md";

/// An owner and an editor of another account.
struct Team {
    _server: Server<SqliteBackend>,
    ana: Desktop,
    ana_sync: WorkspaceSync,
    ana_folder: Folder,
    ben: Desktop,
    ben_sync: WorkspaceSync,
    ben_folder: Folder,
}

impl Team {
    async fn start() -> Self {
        let server = Server::start().await;
        let mut ana = first_desktop(&server, "ana@example.com").await;
        let mut ana_folder = Folder::new("Field notes");
        ana_folder.write(PLAN, b"# Plan\n");
        let mut ana_sync = WorkspaceSync::turn_on(
            &mut ana.service,
            ana_folder.engine.clone(),
            "Field notes",
            &mut ana_folder.registry,
        )
        .await
        .expect("sync turns on");
        ana_sync.round(&mut ana.service, 0).await.expect("a round");
        let mut book = InvitationBook::open(ana.state.path(), ana.id()).unwrap();
        let mut ben = first_desktop(&server, "ben@example.com").await;
        let (mut ben_sync, ben_folder) =
            invite_and_join(&mut ana, &mut ana_sync, &mut book, &mut ben, Role::Editor).await;
        ben_sync.set_rotation_wait(Duration::ZERO);
        Self {
            _server: server,
            ana,
            ana_sync,
            ana_folder,
            ben,
            ben_sync,
            ben_folder,
        }
    }

    async fn set_ben(&mut self, role: Role) {
        let account: Id = self.ben.credentials.account;
        self.ana_sync
            .change_member(
                &mut self.ana.service,
                MemberChange::SetRole { account, role },
            )
            .await
            .expect("the role changes");
    }

    async fn ana_round(&mut self) {
        self.ana_sync
            .round(&mut self.ana.service, 0)
            .await
            .expect("a round");
    }

    /// Gives Ben `role` back: the pull of one round takes the change, and
    /// the next signs what waited.
    async fn give_back(&mut self, role: Role) {
        self.set_ben(role).await;
        self.ben_round().await;
        self.ben_round().await;
        self.ana_round().await;
    }

    async fn ben_round(&mut self) {
        self.ben_sync
            .round(&mut self.ben.service, 0)
            .await
            .expect("a refusal doesn't fail the round");
    }
}

/// Answers the next `times` pushes with a refusal of the record at `index`.
fn refuse_push(proxy: &Proxy, code: &'static str, index: u16, times: usize) {
    let left = std::sync::atomic::AtomicUsize::new(times);
    proxy.forge(Box::new(move |method, path| {
        if method != "POST" || !path.ends_with("/ops") {
            return None;
        }
        left.try_update(
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
            |n| n.checked_sub(1),
        )
        .ok()?;
        let detail = PushFailureDetail {
            record_index: index.into(),
            code: Text(code.to_owned()),
            record_detail: None,
        };
        let body = ErrorBody {
            code: Text(code.to_owned()),
            message: Text("refused".to_owned()),
            retry_after_seconds: None,
            detail: Some(Bytes(detail.to_bytes())),
        };
        Some((409, body.to_bytes()))
    }));
}

async fn both_rounds<B: Backend>(pair: &mut Pair<B>) {
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
}

/// `len` bytes that fill an attachment.
fn attachment(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|i| (i as u32).wrapping_mul(31).wrapping_add(seed.into()) as u8)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_edit_refused_as_forbidden_stays_in_its_file_and_goes_out_with_access() {
    let mut team = Team::start().await;
    team.ben_folder.write(PLAN, b"# Ben's plan\n");
    assert_eq!(team.ben_sync.capture().expect("captured"), 1);
    team.set_ben(Role::Viewer).await;
    team.ben_round().await;
    assert_eq!(team.ben_sync.status().pending, 0, "the record left");
    assert_eq!(team.ben_folder.files()[PLAN], b"# Ben's plan\n");

    // Another device's edit doesn't take the file while it holds the
    // refused change, and the device tells you the edit stays here.
    team.ana_folder.write(PLAN, b"# Ana's plan\n");
    team.ana_round().await;
    team.ben_round().await;
    assert_eq!(team.ben_folder.files()[PLAN], b"# Ben's plan\n");
    let skipped = team.ben_sync.status().skipped;
    assert!(
        skipped
            .iter()
            .any(|s| s.path == PLAN && s.why == SkipWhy::CantWrite),
        "{skipped:?}"
    );
    assert_eq!(team.ben_sync.status().pending, 0, "it signs nothing");

    // With access back, the edit goes out as a branch of its own.
    team.give_back(Role::Editor).await;
    team.ben_round().await;
    assert_eq!(team.ana_sync.status().conflicts, 1);
    assert_eq!(team.ben_sync.status().conflicts, 1);
    assert_eq!(team.ben_folder.files()[PLAN], b"# Ben's plan\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_new_file_and_a_delete_wait_on_a_device_that_cant_write() {
    let mut team = Team::start().await;
    team.ana_folder.write("Keep.md", b"# Keep\n");
    team.ana_round().await;
    team.ben_round().await;
    team.set_ben(Role::Viewer).await;
    team.ben_round().await;

    team.ben_folder.write("New.md", b"# New\n");
    std::fs::remove_file(team.ben_folder.path("Keep.md")).expect("removed");
    assert_eq!(team.ben_sync.capture().expect("captured"), 0);
    team.ben_round().await;
    assert_eq!(team.ben_sync.status().skipped.len(), 2);

    // An edit that arrives doesn't bring the file back while its delete
    // waits for you.
    team.ana_folder.write("Keep.md", b"# Keep, changed\n");
    team.ana_round().await;
    team.ben_round().await;
    assert!(!team.ben_folder.path("Keep.md").exists());

    team.give_back(Role::Editor).await;
    assert_eq!(team.ana_folder.files()["New.md"], b"# New\n");
    assert_eq!(team.ana_sync.status().conflicts, 1, "edit against delete");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn undoing_an_edit_on_a_device_that_cant_write_lets_the_file_follow_again() {
    let mut team = Team::start().await;
    team.set_ben(Role::Viewer).await;
    team.ben_round().await;
    team.ben_folder.write(PLAN, b"# Ben's plan\n");
    team.ben_round().await;
    assert_eq!(team.ben_sync.status().skipped.len(), 1);

    team.ben_folder.write(PLAN, b"# Plan\n");
    team.ben_round().await;
    assert!(team.ben_sync.status().skipped.is_empty());
    team.ana_folder.write(PLAN, b"# Ana's plan\n");
    team.ana_round().await;
    team.ben_round().await;
    assert_eq!(team.ben_folder.files()[PLAN], b"# Ana's plan\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_text_mode_edit_on_a_device_that_cant_write_survives_until_access_returns() {
    let mut team = Team::start().await;
    let (entered, document) = team.ana_sync.open_text(PLAN).expect("the note opens");
    assert_eq!(entered, Entered::Yes);
    drop(document);
    team.ana_sync.close_text(PLAN).expect("closed");
    team.ana_round().await;
    team.ben_round().await;
    team.set_ben(Role::Viewer).await;
    team.ben_round().await;

    team.ben_folder.write(PLAN, b"# Plan\n\nBen's line.\n");
    team.ben_round().await;
    team.ben_round().await;
    assert_eq!(team.ben_folder.files()[PLAN], b"# Plan\n\nBen's line.\n");

    team.give_back(Role::Editor).await;
    let text = String::from_utf8(team.ana_folder.files()[PLAN].clone()).expect("text");
    assert!(text.contains("Ben's line."), "{text}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn forbidden_on_an_upload_leaves_the_pull_running() {
    let mut team = Team::start().await;
    let pdf = attachment(600_000, 1);
    team.ben_folder.write("Report.pdf", &pdf);
    assert_eq!(team.ben_sync.capture().expect("captured"), 1);
    team.set_ben(Role::Viewer).await;
    team.ana_folder.write("Later.md", b"# Later\n");
    team.ana_round().await;

    team.ben_round().await;
    assert_eq!(team.ben_sync.status().pending, 0);
    assert_eq!(team.ben_folder.files()["Later.md"], b"# Later\n");
    assert!(team.ben_folder.files()["Report.pdf"] == pdf);

    team.give_back(Role::Editor).await;
    assert!(team.ana_folder.files()["Report.pdf"] == pdf);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_blob_collected_before_its_push_uploads_again() {
    let mut pair = pair().await;
    let pdf = attachment(1_500_000, 2);
    pair.a.write("Report.pdf", &pdf);
    pair.proxy.forge(Box::new(|method, path| {
        (method == "POST" && path.ends_with("/ops")).then(|| (503, Vec::new()))
    }));
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect_err("the push doesn't get through");
    assert_eq!(pair.sync_a.status().pending, 1);
    assert!(!pair.server.blob_files().is_empty(), "the upload completed");

    // The collector takes the completed upload no push listed.
    pair.server
        .store
        .clock()
        .advance(Duration::from_secs(25 * 3600));
    assert_eq!(pair.server.collect_blobs().await, 1);
    assert!(pair.server.blob_files().is_empty());

    // The file changes meanwhile: the record still carries what it signed.
    let newer = attachment(1_500_000, 3);
    pair.a.write("Report.pdf", &newer);
    pair.proxy.honest();
    both_rounds(&mut pair).await;
    both_rounds(&mut pair).await;
    assert_eq!(pair.sync_a.status().pending, 0);
    assert!(pair.b.files()["Report.pdf"] == newer);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lent_blob_gone_from_the_server_seals_again_from_the_file() {
    let mut pair = pair().await;
    let pdf = attachment(1_500_000, 4);
    pair.a.write("Report.pdf", &pdf);
    both_rounds(&mut pair).await;
    // The server says the blob isn't there, though it holds it: the device
    // reads the offset, finds the server contradicts it, and keeps the
    // record for later, while the pull still runs.
    std::fs::rename(pair.a.path("Report.pdf"), pair.a.path("Moved.pdf")).expect("moved");
    refuse_push(&pair.proxy, "blob_incomplete", 0, 1);
    pair.b.write("Later.md", b"# Later\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(round.is_err(), "the refusal is reported");
    assert!(pair.a.path("Later.md").exists(), "the pull ran");
    assert_eq!(pair.sync_a.status().pending, 1);
    both_rounds(&mut pair).await;
    assert_eq!(pair.sync_a.status().pending, 0);
    assert!(pair.b.path("Moved.pdf").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lone_refused_record_stays_local_and_the_pull_runs() {
    for code in ["conflict_limit", "snapshot_required"] {
        let mut pair = pair().await;
        pair.b.write("Later.md", b"# Later\n");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("a round on the desktop");
        pair.a.write(PLAN, b"# Mine\n");
        refuse_push(&pair.proxy, code, 0, 1);
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("a refused record doesn't fail the round");
        assert_eq!(pair.a.files()["Later.md"], b"# Later\n", "{code}");
        assert_eq!(pair.a.files()[PLAN], b"# Mine\n");
        both_rounds(&mut pair).await;
        assert_eq!(pair.b.files()[PLAN], b"# Mine\n", "{code}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refusal_that_waits_still_lets_the_round_pull() {
    let mut pair = pair().await;
    pair.b.write("Later.md", b"# Later\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
    pair.a.write(PLAN, b"# Mine\n");
    // Nothing moved, so the device retries the same bytes later.
    refuse_push(&pair.proxy, "policy_stale", 0, 1);
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(round.is_err(), "the refusal is reported");
    assert_eq!(pair.a.files()["Later.md"], b"# Later\n");
    assert_eq!(pair.sync_a.status().pending, 1);
    both_rounds(&mut pair).await;
    assert_eq!(pair.b.files()[PLAN], b"# Mine\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_pull_keeps_a_file_whose_edit_waits_in_the_outbox() {
    let mut pair = pair().await;
    pair.a.write(PLAN, b"# Mine\n");
    assert_eq!(pair.sync_a.capture().expect("captured"), 1);
    pair.b.write(PLAN, b"# Theirs\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");

    pair.sync_a
        .pull(&mut pair.service, 0)
        .await
        .expect("pulled");
    assert_eq!(pair.a.files()[PLAN], b"# Mine\n");

    both_rounds(&mut pair).await;
    both_rounds(&mut pair).await;
    assert_eq!(pair.sync_a.status().conflicts, 1);
    assert_eq!(pair.sync_b.status().conflicts, 1);
    assert_eq!(pair.a.files()[PLAN], b"# Mine\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_delete_goes_out_again() {
    let mut pair = pair().await;
    pair.a.write("Other.md", b"# Other\n");
    both_rounds(&mut pair).await;

    std::fs::remove_file(pair.a.path(PLAN)).expect("removed");
    refuse_push(&pair.proxy, "conflict_limit", 0, 1);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a refused delete doesn't fail the round");
    assert!(!pair.a.path(PLAN).exists());
    assert_eq!(pair.sync_a.status().pending, 0);

    // A change that arrives meanwhile doesn't bring the file back on its
    // own: the delete goes out again and the two stand as a conflict.
    pair.b.write(PLAN, b"# Theirs\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
    pair.sync_a
        .pull(&mut pair.service, 0)
        .await
        .expect("pulled");
    assert!(!pair.a.path(PLAN).exists());
    both_rounds(&mut pair).await;
    both_rounds(&mut pair).await;
    assert_eq!(pair.sync_a.status().pending, 0);
    assert_eq!(pair.sync_b.status().conflicts, 0, "cover_all resolves it");
    assert!(!pair.b.path(PLAN).exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_delete_that_lost_every_cover_waits_for_your_choice() {
    let mut pair = pair().await;
    let rid = pair.sync_a.rid_at(PLAN).expect("mapped");
    std::fs::remove_file(pair.a.path(PLAN)).expect("removed");
    assert_eq!(pair.sync_a.capture().expect("captured"), 1);
    pair.b.write(PLAN, b"# Theirs\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a round on the desktop");
    pair.sync_a
        .pull(&mut pair.service, 0)
        .await
        .expect("pulled");

    // The pruned cover is gone from the chain, so no cover is left. The
    // refusal's check finds the cover at its position only on an honest
    // server, so this forges what a pruned one answers.
    let pruned = std::sync::atomic::AtomicBool::new(false);
    pair.proxy.forge(Box::new(move |method, path| {
        if method == "GET" && path.contains("/chain/") {
            let body = ErrorBody {
                code: Text("pruned".to_owned()),
                message: Text("pruned".to_owned()),
                retry_after_seconds: None,
                detail: None,
            };
            return Some((410, body.to_bytes()));
        }
        if method == "POST"
            && path.ends_with("/ops")
            && !pruned.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            let detail = PushFailureDetail {
                record_index: 0,
                code: Text("supersedes_unknown".to_owned()),
                record_detail: None,
            };
            let body = ErrorBody {
                code: Text("supersedes_unknown".to_owned()),
                message: Text("refused".to_owned()),
                retry_after_seconds: None,
                detail: Some(Bytes(detail.to_bytes())),
            };
            return Some((409, body.to_bytes()));
        }
        None
    }));
    pair.sync_a
        .push(&mut pair.service)
        .await
        .expect("the record leaves");
    pair.proxy.honest();
    assert_eq!(pair.sync_a.status().pending, 0);
    assert_eq!(pair.sync_a.status().held, 1, "it waits for you");
    assert!(!pair.a.path(PLAN).exists());

    pair.sync_a
        .resolve_held(rid, HeldChoice::KeepMine)
        .expect("kept");
    both_rounds(&mut pair).await;
    both_rounds(&mut pair).await;
    assert_eq!(pair.sync_b.status().conflicts, 0);
    assert!(!pair.b.path(PLAN).exists());
}
