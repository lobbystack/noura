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
    // A new device of the member: a link the backup lacks, which the
    // workspace records and both accounts accept.
    let _third = approve(
        &pair.server,
        &mut pair.desktop,
        std::slice::from_mut(pair.sync_b.remote_mut().expect("connected")),
    )
    .await;
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
    assert!(
        matches!(
            pair.sync_a.restore_lost(),
            Some(Lost::Policy | Lost::MemberAccount(_))
        ),
        "{:?}",
        pair.sync_a.restore_lost()
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
    let sync = pair.sync_a;
    let files = pair.a.files();
    let mut fresh = sync
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
