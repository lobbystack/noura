//! A server restored from a backup (seam 4, sync-v2.md §14): a device that
//! sees a new generation stops, compares what it accepted with what the
//! server holds, and either stops with `sync_restore_lost` or rewinds and
//! writes the lost content again from its files. A database put back
//! without a new generation is caught as a rollback.

mod sync_support;

use app_core::sync::workspace::restore::Lost;
use sync_store_sqlite::SqliteBackend;
use sync_support::{MemorySecrets, Pair, Server, approve, pair, pair_of_accounts_on};

const NO_WAIT: std::time::Duration = std::time::Duration::ZERO;

/// Both devices through the proxy, which now reaches `base`.
async fn reconnect(pair: &mut Pair<SqliteBackend>, base: &str) {
    pair.proxy.retarget(base);
    pair.service = pair.laptop.connect_through(&pair.proxy.base).await;
    pair.desktop.service = pair.desktop.connect_through(&pair.proxy.base).await;
    pair.sync_a.set_restore_wait(NO_WAIT);
    pair.sync_b.set_restore_wait(NO_WAIT);
}

async fn rounds(pair: &mut Pair<SqliteBackend>) {
    for _ in 0..3 {
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("laptop");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("desktop");
    }
}

/// A desktop that wrote behind the backup: the laptop writes, both apply.
async fn write_after_backup(pair: &mut Pair<SqliteBackend>) {
    pair.a.write("Lost.md", b"# Lost\n");
    rounds(pair).await;
    assert_eq!(pair.a.files(), pair.b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restore_that_lost_only_content_writes_it_again() {
    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    write_after_backup(&mut pair).await;
    let files = pair.a.files();

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;

    assert_eq!(pair.a.files(), files, "no file changed on the laptop");
    assert_eq!(pair.b.files(), files, "no file changed on the desktop");
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert!(pair.sync_a.restore_alert().is_some(), "the app alerts you");
    assert!(pair.sync_a.status().held == 0 && pair.sync_b.status().held == 0);
    let written = [pair.sync_a.written_again(), pair.sync_b.written_again()].concat();
    assert!(written.iter().any(|path| path == "Lost.md"));

    // The lost file is back on the server: a new device finds it, and the
    // devices keep syncing.
    pair.b.write("After.md", b"# After\n");
    rounds(&mut pair).await;
    assert_eq!(pair.a.files(), pair.b.files());
    assert!(pair.a.files().contains_key("After.md"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_file_copy_of_the_database_is_a_rollback_until_the_generation_changes() {
    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    write_after_backup(&mut pair).await;
    let files = pair.a.files();

    let put_back = pair.server.put_back(&copy, false).await;
    reconnect(&mut pair, &put_back.base).await;
    pair.a.write("Held.md", b"# Held\n");
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        round
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_server_rollback")
    );
    assert_eq!(pair.b.files(), files, "the desktop's files stay");
    let again = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(again.is_err(), "the device stays held");

    // `mark-restored` raises the generation, and the devices compare.
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.rollback(), None);
    assert_eq!(pair.a.files(), pair.b.files());
    assert!(pair.a.files().contains_key("Lost.md"));
    assert!(pair.a.files().contains_key("Held.md"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lower_generation_is_a_rollback() {
    let mut pair = pair().await;
    let first = pair.server.backup("first.sqlite3").await;
    let restored = pair.server.put_back(&first, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert!(pair.sync_a.restore_alert().is_some());

    // The original server, at the generation before.
    let old = pair.server.base.clone();
    reconnect(&mut pair, &old).await;
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        round
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_server_rollback")
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_link_of_your_own_chain_stops_the_workspace() {
    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    // A third device: a link of the account chain the backup lacks.
    let mut third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
    )
    .await;
    let _ = third.service.client();
    rounds(&mut pair).await;
    pair.a.write("Kept.md", b"# Kept\n");
    let before = pair.a.files();

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(round.is_ok(), "{round:?}");
    assert_eq!(pair.sync_a.restore_lost(), Some(&Lost::OwnAccount));
    assert_eq!(
        pair.a.files(),
        before,
        "the stopped workspace keeps its files"
    );
    pair.a.write("Later.md", b"# Later\n");
    let round = pair
        .sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(round.pushed, 0, "it writes nothing");
}

/// A restore that lost a link of a member's chain: the member resets, an
/// owner compares the approval code and accepts, and both resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_resets_and_the_owner_accepts_the_reset() {
    use app_core::sync::approval::CandidateStep;
    use app_core::sync::device::{Joined, join};
    use std::sync::Arc;

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let copy = pair.server.backup("before.sqlite3").await;
    let workspace = pair.sync_a.workspace();
    let account = pair.desktop.credentials.account;
    // A new device of the member: a link the backup lacks, which the owner
    // follows. No workspace records it, so no policy change goes.
    let _third = approve(&pair.server, &mut pair.desktop, &mut []).await;
    let genesis = pair.desktop.credentials.reference_head.genesis;
    pair.service
        .follow_account(account, genesis)
        .await
        .expect("the owner keeps the new head");
    rounds(&mut pair).await;

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("desktop");
    assert_eq!(pair.sync_b.restore_lost(), Some(&Lost::OwnAccount));
    assert_eq!(
        pair.sync_a.restore_lost(),
        Some(&Lost::MemberAccount(account))
    );

    // The member signs in on a new card and resets.
    let signed_in = restored.sign_in(account, &restored.base).await;
    let secrets = Arc::new(MemorySecrets::default());
    let Joined::NeedsApproval { keys, mut client } =
        join(&signed_in, &*secrets).await.expect("enrolled")
    else {
        panic!("the account has a chain");
    };
    let session = sync_store::Session::new(account, keys.device());
    let (code, _) = restored
        .store
        .issue_reset_code(session)
        .await
        .expect("a code");
    let done = app_core::sync::reset::reset(
        &mut client,
        &*secrets,
        account,
        &keys,
        &signed_in.session_token,
        &code,
    )
    .await
    .expect("the reset");
    assert_ne!(done.fingerprint, sync_protocol::types::Digest::ZERO);
    let state = tempfile::tempdir().expect("a folder");
    let mut member =
        sync_support::Desktop::connect(&restored.base, done.credentials.clone(), secrets, state)
            .await;

    // Its device asks the workspace to accept it.
    let mut candidate = member
        .service
        .request_reset_approval(workspace)
        .await
        .expect("the request");
    let requests = pair
        .service
        .reset_requests(workspace)
        .await
        .expect("listed");
    assert_eq!(requests.len(), 1);
    let mut remote = pair
        .sync_a
        .open_for_reset(&mut pair.service)
        .await
        .expect("the owner opens the workspace");
    let mut offer = pair
        .service
        .offer_invited(&requests[0], &mut remote)
        .await
        .expect("offered");
    let CandidateStep::Code(shown) = member
        .service
        .poll_invited(&mut candidate)
        .await
        .expect("polled")
    else {
        panic!("a code");
    };
    assert_eq!(
        pair.service.approval_code(&mut offer).await.expect("read"),
        Some(shown)
    );
    candidate.confirm();
    let revision = pair
        .sync_a
        .accept_reset(&mut pair.service, offer)
        .await
        .expect("accepted");
    assert!(revision > 1);

    // The owner's stop for that chain ends, and it syncs again.
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    assert_eq!(pair.sync_a.restore_lost(), None);
}

/// Set up sync again closes the workspace and starts a new one; a device
/// still on the old one shows that its owner closed it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_up_sync_again_closes_the_old_workspace() {
    let mut pair = pair().await;
    let old = pair.sync_a.workspace();
    let files = pair.a.files();
    let mut fresh = pair
        .sync_a
        .set_up_again(&mut pair.service, &mut pair.a.registry)
        .await
        .expect("set up again");
    assert_ne!(fresh.workspace(), old);
    fresh.round(&mut pair.service, 0).await.expect("a round");
    assert_eq!(pair.a.files(), files);

    let round = pair
        .sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(round.applied, 0);
    assert_eq!(pair.sync_b.restore_lost(), Some(&Lost::Closed));
}

/// One writer goes first for each lost object, whichever device asks.
#[test]
fn two_hundred_writers_rank_one_writer_first_for_each_object() {
    use app_core::sync::workspace::restore::{rank, wait_for};
    use std::collections::BTreeSet;
    use sync_protocol::types::Id;
    let writers: BTreeSet<Id> = (0..200u8).map(|i| Id([i; 16])).collect();
    let rid = Id([250; 16]);
    let ranks: BTreeSet<usize> = writers.iter().map(|w| rank(&rid, w, &writers)).collect();
    assert_eq!(ranks.len(), 200, "each writer holds its own place");
    let first = writers
        .iter()
        .find(|w| rank(&rid, w, &writers) == 0)
        .unwrap();
    let last = writers
        .iter()
        .find(|w| rank(&rid, w, &writers) == 199)
        .unwrap();
    assert!(wait_for(rank(&rid, first, &writers), 200, 60) < 2);
    assert!(wait_for(rank(&rid, last, &writers), 200, 60) >= 59);
}

/// A server that claims a restore shows you an alert, and with nothing
/// lost no file changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_generation_claim_shows_an_alert_and_changes_no_file() {
    let mut pair = pair().await;
    let files = pair.a.files();
    let copy = pair.server.backup("same.sqlite3").await;
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert!(pair.sync_a.restore_alert().is_some());
    assert_eq!(pair.a.files(), files);
    pair.sync_a.dismiss_restore_alert().expect("dismissed");
    assert_eq!(pair.sync_a.restore_alert(), None);
}

/// A blob store that lost objects a live operation lists: a device that
/// holds the file uploads it again under the same ID.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_uploads_a_lost_blob_again_under_its_id() {
    let mut pair = pair().await;
    let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    pair.a.write("photo.png", &bytes);
    rounds(&mut pair).await;
    assert_eq!(pair.b.files().get("photo.png"), Some(&bytes));
    let copy = pair.server.backup("before.sqlite3").await;
    let stored = pair.server.blob_files();
    assert!(!stored.is_empty());
    for file in &stored {
        std::fs::remove_file(file).expect("lost");
    }
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert_eq!(
        pair.server.blob_files().len(),
        stored.len(),
        "the same objects came back"
    );
    assert!(stored.iter().all(|file| file.exists()));
    assert_eq!(pair.a.files(), pair.b.files());
}

/// Every request of the comparison fails once in turn, and the device still
/// ends up compared and rewound, never on its old cursor.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_comparison_cut_short_runs_again_at_every_step() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    write_after_backup(&mut pair).await;
    let files = pair.a.files();
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;

    let mut failed_at = 0;
    loop {
        failed_at += 1;
        assert!(failed_at < 60, "the comparison never ended");
        let seen = Arc::new(AtomicUsize::new(0));
        let target = failed_at;
        pair.proxy.forge(Box::new(move |_, _| {
            (seen.fetch_add(1, Ordering::SeqCst) + 1 == target).then(|| (503, Vec::new()))
        }));
        let round = pair.sync_a.round(&mut pair.service, 0).await;
        pair.proxy.honest();
        if round.is_ok() && pair.sync_a.status().rollback.is_none() {
            break;
        }
    }
    assert!(failed_at > 2, "the comparison made requests to fail");
    assert_eq!(pair.sync_a.restore_lost(), None);
    rounds(&mut pair).await;
    assert_eq!(pair.a.files(), files);
    let written = [pair.sync_a.written_again(), pair.sync_b.written_again()].concat();
    assert!(written.iter().any(|path| path == "Lost.md"));
}

/// A generation that rises during a rewind compares again and ends like one
/// restore, from the journal the first rewind kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_restores_in_a_row_end_like_one() {
    let mut pair = pair().await;
    let older = pair.server.backup("older.sqlite3").await;
    pair.a.write("Middle.md", b"# Middle\n");
    rounds(&mut pair).await;
    let newer = pair.server.backup("newer.sqlite3").await;
    write_after_backup(&mut pair).await;
    let files = pair.a.files();

    // The first restore, with the pull of its rewind cut off.
    let first = pair.server.put_back(&newer, true).await;
    reconnect(&mut pair, &first.base).await;
    pair.proxy.forge(Box::new(|_, path| {
        path.ends_with("/pull").then(|| (503, Vec::new()))
    }));
    assert!(pair.sync_a.round(&mut pair.service, 0).await.is_err());
    pair.proxy.honest();
    assert!(pair.sync_a.status().restored.is_some());

    // The second, from an older backup, reaches the device mid-rewind. The
    // generation takes the clock's seconds.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second = pair.server.put_back(&older, true).await;
    reconnect(&mut pair, &second.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert_eq!(pair.a.files(), files);
    assert_eq!(pair.b.files(), files);
    let written = [pair.sync_a.written_again(), pair.sync_b.written_again()].concat();
    for lost in ["Lost.md", "Middle.md"] {
        assert!(
            written.iter().any(|path| path == lost),
            "{lost}: {written:?}"
        );
    }
}

/// A stopped device applies nothing a frame carries.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_frame_changes_no_file_on_a_stopped_workspace() {
    use sync_protocol::messages::Frame;

    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    let mut third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
    )
    .await;
    let _ = third.service.client();
    rounds(&mut pair).await;
    let before = pair.a.files();

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(pair.sync_a.restore_lost(), Some(&Lost::OwnAccount));

    // The server moves on, and tells the stopped device.
    let domain = pair.sync_a.domain();
    let cursor = pair.sync_a.cursor();
    let outcome = pair
        .sync_a
        .on_frame(
            &mut pair.service,
            &Frame::Notify {
                domain,
                head: cursor + 10,
                checkpoint_due: 0,
            },
        )
        .await
        .expect("handled");
    assert_eq!(outcome.applied, 0);
    assert!(!outcome.pulled, "a stopped device doesn't pull");
    assert_eq!(pair.a.files(), before);
    let (applied, _) = pair
        .sync_a
        .pull(&mut pair.service, 0)
        .await
        .expect("a pull");
    assert_eq!(applied, 0);
    assert_eq!(pair.sync_a.cursor(), cursor);
    assert_eq!(
        pair.sync_a
            .rename(&mut pair.service, "Elsewhere")
            .await
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_restore_lost"),
        "a stopped device writes nothing"
    );
}

/// A records frame that arrives between the generation change and the
/// rewind leaves the cursor and the files alone: the device compares first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_records_frame_before_the_rewind_applies_nothing() {
    use sync_protocol::messages::Frame;

    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    write_after_backup(&mut pair).await;
    let files = pair.a.files();
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    let generation = pair.service.client().restore_generation();

    // The frame says the generation rose, while a later record's frame
    // comes in behind it, as the channel would deliver them.
    let domain = pair.sync_a.domain();
    let cursor = pair.sync_a.cursor();
    pair.sync_a
        .on_frame(
            &mut pair.service,
            &Frame::Records {
                domain,
                previous_seq: cursor,
                checkpoint_due: 0,
                entries: sync_protocol::codec::Sorted::new(Vec::new()).expect("sorted"),
            },
        )
        .await
        .expect("handled");
    assert_eq!(pair.a.files(), files);
    pair.sync_a
        .on_frame(
            &mut pair.service,
            &Frame::Restored {
                restore_generation: generation,
            },
        )
        .await
        .expect("the frame starts the comparison");
    assert_eq!(pair.a.files(), files, "no file changed");
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert!(pair.sync_a.restore_alert().is_some());
}

/// A rollback holds the device until the generation changes, and the held
/// device finds that out by itself.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_held_device_learns_of_the_new_generation() {
    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    write_after_backup(&mut pair).await;

    let put_back = pair.server.put_back(&copy, false).await;
    reconnect(&mut pair, &put_back.base).await;
    assert!(pair.sync_a.round(&mut pair.service, 0).await.is_err());
    assert!(pair.sync_a.rollback().is_some());

    // The same service, on the same connection details: only the proxy
    // moves to the marked server.
    let marked = pair.server.put_back(&copy, true).await;
    pair.proxy.retarget(&marked.base);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the device compares");
    assert_eq!(pair.sync_a.rollback(), None);
    assert!(pair.sync_a.restore_alert().is_some());
}

/// A forged `accept_reset` the server only claims erases no evidence: the
/// policy chain doesn't verify, so the workspace stays stopped and keeps
/// its heads.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_forged_accept_reset_leaves_the_stop_in_place() {
    use sync_protocol::codec::{Decode, Encode, List, Nested};
    use sync_protocol::messages::PolicyResponse;
    use sync_protocol::records::{PolicyChange, PolicyKind, Signed};

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let copy = pair.server.backup("before.sqlite3").await;
    let account = pair.desktop.credentials.account;
    let _third = approve(&pair.server, &mut pair.desktop, &mut []).await;
    let genesis = pair.desktop.credentials.reference_head.genesis;
    pair.service
        .follow_account(account, genesis)
        .await
        .expect("kept");
    rounds(&mut pair).await;
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(
        pair.sync_a.restore_lost(),
        Some(&Lost::MemberAccount(account))
    );
    let head = pair.sync_a.policy_revision();

    // The server appends a change it made up, signed by nobody it can name.
    let forger =
        sync_protocol::crypto::SigningKey::from_seed(&sync_protocol::types::SecretKey([9; 32]));
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.contains("/policy") {
            return body;
        }
        let mut response = PolicyResponse::from_bytes(&body).expect("a policy response");
        let last: Option<Signed<PolicyChange>> = response.changes.0.last().map(|c| c.0.clone());
        let Some(last) = last else {
            return body;
        };
        let mut change = last.body.clone();
        change.revision = head + 1;
        change.previous = last.digest();
        change.kind = PolicyKind::AcceptReset { account };
        let forged = Signed::sign(change, &forger);
        let mut changes = response.changes.0.clone();
        changes.push(Nested(forged));
        response.changes = List(changes);
        response.head_revision = head + 1;
        response.to_bytes()
    }));
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    pair.proxy.honest();
    assert_eq!(
        pair.sync_a.restore_lost(),
        Some(&Lost::MemberAccount(account)),
        "the forged change ended nothing"
    );
    assert!(
        !pair.service.kept_heads().kept_of(&account).is_empty(),
        "the head stays kept"
    );
}

/// A viewer device writes nothing: a blob the restore lost waits for a
/// writer, and the viewer's rewind doesn't stop on it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_rewinds_without_writing_a_lost_blob() {
    use app_core::sync::invitations::InvitationBook;
    use sync_protocol::records::Role;

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    pair.a.write("photo.png", &bytes);
    rounds(&mut pair).await;

    let dir = tempfile::tempdir().expect("a folder");
    let mut book = InvitationBook::open(dir.path(), pair.laptop.id()).expect("a book");
    let mut viewer = sync_support::first_desktop(&pair.server, "vera@example.com").await;
    let (mut sync_v, folder) = sync_support::invite_and_join(
        &mut pair.laptop,
        &mut pair.sync_a,
        &mut book,
        &mut viewer,
        Role::Viewer,
    )
    .await;
    sync_v.round(&mut viewer.service, 0).await.expect("joined");
    assert_eq!(folder.files().get("photo.png"), Some(&bytes));

    let copy = pair.server.backup("before.sqlite3").await;
    for file in pair.server.blob_files() {
        std::fs::remove_file(file).expect("lost");
    }
    let restored = pair.server.put_back(&copy, true).await;
    let mut viewer_service = viewer.connect_through(&restored.base).await;
    sync_v.set_restore_wait(NO_WAIT);
    let round = sync_v.round(&mut viewer_service, 0).await;
    assert!(round.is_ok(), "{round:?}");
    assert_eq!(sync_v.restore_lost(), None);
    assert_eq!(folder.files().get("photo.png"), Some(&bytes));
    assert!(
        pair.server.blob_files().is_empty(),
        "the viewer uploaded nothing"
    );
}

/// Set up sync again from a member's device gets a clear refusal, and the
/// section doesn't offer it there; a failed start keeps the old sync.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_up_sync_again_stays_with_the_owner_and_survives_a_failure() {
    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let refused = pair
        .sync_b
        .set_up_again(&mut pair.desktop.service, &mut pair.b.registry)
        .await;
    assert!(refused.is_err(), "a member can't");
    assert!(!pair.sync_b.status().owner);
    let notice = app_core::folder_sync::restore_notice(&app_core::sync::workspace::Status {
        restore_lost: Some(Lost::Policy),
        ..pair.sync_b.status()
    })
    .expect("a notice");
    assert!(!notice.set_up_again);

    // The close goes through, then creating the new workspace fails.
    pair.proxy.forge(Box::new(|method, path| {
        (method == "POST" && path.ends_with("/v2/workspaces")).then(|| (503, Vec::new()))
    }));
    let old = pair.sync_a.workspace();
    assert!(
        pair.sync_a
            .set_up_again(&mut pair.service, &mut pair.a.registry)
            .await
            .is_err()
    );
    pair.proxy.honest();
    assert_eq!(pair.sync_a.restore_lost(), Some(&Lost::Closed));
    let notice = app_core::folder_sync::restore_notice(&pair.sync_a.status()).expect("a notice");
    assert!(notice.set_up_again, "the owner tries again");
    let fresh = pair
        .sync_a
        .set_up_again(&mut pair.service, &mut pair.a.registry)
        .await
        .expect("set up");
    assert_ne!(fresh.workspace(), old);
}
