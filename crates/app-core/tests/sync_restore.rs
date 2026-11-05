//! A server restored from a backup (seam 4, sync-v2.md §14): a device that
//! sees a new generation stops, compares what it accepted with what the
//! server holds, and either stops with `sync_restore_lost` or rewinds and
//! writes the lost content again from its files. A database put back
//! without a new generation is caught as a rollback.

mod sync_support;

use app_core::sync::workspace::restore::Lost;
use sync_protocol::records::{Access, Level, Role};
use sync_support::{
    Desktop, LinkToRevoke, MemorySecrets, Pair, Restores, Server, approve, approve_new,
    new_desktop_in, pair, pair_of_accounts_on, pair_of_accounts_with, pair_on, reconnect,
    reset_devices, rounds,
};

const NO_WAIT: std::time::Duration = std::time::Duration::ZERO;

/// Changes a backup with `sql`, for a restore that loses exactly one kind
/// of signed state.
fn edit_backup(copy: &std::path::Path, sql: &str) {
    let connection = rusqlite::Connection::open(copy).expect("the copy opens");
    let changed = connection.execute(sql, []).expect("the change");
    assert!(changed > 0, "{sql}");
}

/// A desktop that wrote behind the backup: the laptop writes, both apply.
async fn write_after_backup<B: sync_store::Backend>(pair: &mut Pair<B>) {
    pair.a.write("Lost.md", b"# Lost\n");
    rounds(pair).await;
    assert_eq!(pair.a.files(), pair.b.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restore_that_lost_only_content_writes_it_again() {
    let mut pair = pair().await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    link.revoke(&mut pair).await;
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
    link.revoked_again(&mut pair).await;
}

/// A page a kill cut short named a sequence the restored log never reaches
/// again. Its note ends with the rewind, so new files go out in a quiet
/// workspace, and the files that page left upload as the only copies.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rewind_ends_the_note_of_a_page_above_the_restored_head() {
    let mut pair = pair().await;
    let workspace = pair.sync_a.workspace();
    let mut third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut remote = third
        .service
        .open_workspace(workspace)
        .await
        .expect("opens");
    third
        .service
        .push_file(&mut remote, "Third.md", b"# Third\n")
        .await
        .expect("written");
    rounds(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;

    for i in 0..4 {
        third
            .service
            .push_file(&mut remote, &format!("Gone {i}.md"), b"# Gone\n")
            .await
            .expect("written");
    }
    pair.sync_b.kill_after_page();
    let killed = pair.sync_b.pull(&mut pair.desktop.service, 0).await;
    assert!(killed.is_err(), "the kill: {killed:?}");
    pair.sync_b = app_core::sync::workspace::WorkspaceSync::open(
        pair.b.engine.clone(),
        pair.desktop.credentials.clone(),
        pair.desktop.secrets(),
        &pair.b.registry,
    )
    .expect("the journal opens");

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    pair.b.write("Mine.md", b"# Mine\n");
    rounds(&mut pair).await;
    rounds(&mut pair).await;

    assert_eq!(pair.a.files()["Mine.md"], b"# Mine\n");
    assert_eq!(pair.a.files()["Gone 3.md"], b"# Gone\n");
    assert_eq!(pair.a.files(), pair.b.files());
}

/// The backup keeps a removal but not the checkpoint that followed it. The
/// removal makes a checkpoint due again after the restore, and a rewound
/// writer signs it, at the removed device's final head (§14, §17).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rewound_writer_signs_the_checkpoint_a_surviving_removal_made_due() {
    rewound_writer_signs(Server::start_with_interval(6).await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_rewound_writer_signs_the_due_checkpoint_on_postgres() {
    rewound_writer_signs(Server::start_postgres_alone_with_interval(6).await).await;
}

async fn rewound_writer_signs<B: sync_store::Backend>(server: Server<B>)
where
    Server<B>: Restores,
{
    let never = std::time::Duration::from_secs(1 << 40);
    let mut pair = pair_on(server).await;
    pair.sync_a.set_checkpoint_wait(NO_WAIT);
    pair.sync_b.set_checkpoint_wait(NO_WAIT);
    let (workspace, domain) = (pair.sync_a.workspace(), pair.sync_a.domain());

    // A third device writes, and everyone reaches a first checkpoint.
    let mut third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut remote = third
        .service
        .open_workspace(workspace)
        .await
        .expect("opens");
    third
        .service
        .push_file(&mut remote, "Third.md", b"# Third\n")
        .await
        .expect("written");
    for i in 0..8 {
        pair.a.write(&format!("First {i}.md"), b"# First\n");
        rounds(&mut pair).await;
    }
    let first = pair
        .sync_a
        .latest_checkpoint()
        .expect("a checkpoint")
        .digest();
    assert_eq!(
        pair.sync_b.latest_checkpoint().map(|c| c.digest()),
        Some(first)
    );

    // The removal lands and rotates; the backup holds no checkpoint after it.
    pair.sync_a.set_checkpoint_wait(never);
    pair.sync_b.set_checkpoint_wait(never);
    pair.laptop
        .service
        .remove_device(third.id(), &[], &mut [])
        .await
        .expect("removed");
    rounds(&mut pair).await;
    pair.desktop
        .service
        .record_heads(pair.sync_b.remote_mut().expect("connected"), &[])
        .await
        .expect("recorded");
    rounds(&mut pair).await;
    assert!(!pair.sync_a.removal_hold() && !pair.sync_b.removal_hold());
    let copy = pair.server.snapshot("before.sqlite3").await;

    // The checkpoint after the removal, then content, both lost.
    pair.sync_a.set_checkpoint_wait(NO_WAIT);
    pair.sync_b.set_checkpoint_wait(NO_WAIT);
    rounds(&mut pair).await;
    let signed = pair.sync_a.latest_checkpoint().expect("signed").clone();
    assert!(
        signed.digest() != first,
        "a checkpoint followed the removal"
    );
    let head_of_third = |heads: &[sync_protocol::records::Ref]| {
        heads
            .iter()
            .find(|h| h.device == third.id())
            .map(|h| (h.counter, h.op_digest))
    };
    let lost = pair
        .service
        .client()
        .checkpoint(workspace, domain)
        .await
        .expect("answered")
        .expect("the server holds it");
    write_after_backup(&mut pair).await;

    let restored = pair.server.restored(&copy, None).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a.set_checkpoint_wait(NO_WAIT);
    pair.sync_b.set_checkpoint_wait(NO_WAIT);
    for _ in 0..3 {
        rounds(&mut pair).await;
    }

    let again = pair
        .sync_a
        .latest_checkpoint()
        .expect("signed again")
        .clone();
    assert_eq!(again.body.number, signed.body.number);
    assert!(again.digest() != signed.digest());
    assert_eq!(
        pair.sync_b.latest_checkpoint().map(|c| c.digest()),
        Some(again.digest())
    );
    assert!(pair.sync_a.checkpoint_failures().is_empty());
    assert!(pair.sync_b.checkpoint_failures().is_empty());
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert_eq!(pair.a.files(), pair.b.files());
    let served = pair
        .service
        .client()
        .checkpoint(workspace, domain)
        .await
        .expect("answered")
        .expect("the server holds it");
    // The first checkpoint after a removal still lists the device's final
    // head (§17), which the lost one listed too.
    assert!(head_of_third(&served.bundle.heads).is_some());
    assert_eq!(
        head_of_third(&served.bundle.heads),
        head_of_third(&lost.bundle.heads)
    );
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
    let mut link = LinkToRevoke::make(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    link.revoke(&mut pair).await;
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
    link.revoked_again(&mut pair).await;
}

/// The approval that commits a reset (§7, §10): `member`'s device asks,
/// the owner opens the workspace and offers, they compare the code, and
/// the owner pushes the `accept_reset`.
async fn accept_the_reset<B: sync_store::Backend>(
    pair: &mut Pair<B>,
    member: &mut Desktop,
    workspace: sync_protocol::types::Id,
) -> (app_core::sync::joining::InvitedCandidate, u64) {
    use app_core::sync::approval::CandidateStep;

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
    (candidate, revision)
}

/// A restore cut the member's chain short of the head its device keeps. On
/// that computer the reset keeps the old device's secrets aside and drops
/// what it kept of the retired chain, so the old head stops nothing, and
/// the new device goes on to resume (§7, §14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reset_drops_the_kept_head_of_its_own_retired_chain() {
    use app_core::sync::KeptHeads;
    use app_core::sync::device::{StoredDevice, retired_item};

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let copy = pair.server.backup("before.sqlite3").await;
    let workspace = pair.sync_a.workspace();
    let account = pair.desktop.credentials.account;
    let _third = approve(&pair.server, &mut pair.desktop, &mut []).await;
    let genesis = pair.desktop.credentials.reference_head.genesis;
    pair.service
        .follow_account(account, genesis)
        .await
        .expect("the owner keeps the new head");
    rounds(&mut pair).await;
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("desktop");
    assert_eq!(pair.sync_b.restore_lost(), Some(&Lost::OwnAccount));
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");

    // This computer holds the card, under the restored server's address.
    let old = pair.desktop.id();
    let secrets = pair.desktop.secrets.clone();
    assert_eq!(
        pair.desktop.service.kept_heads().kept_of(&account)[0].seqno,
        2
    );
    StoredDevice::new(
        &restored.base,
        account,
        &pair.desktop.credentials.keys,
        &pair.desktop.credentials.session_token,
    )
    .save(&*secrets)
    .expect("stored");
    let (mut member, done) = reset_devices(&restored, account, secrets.clone()).await;
    assert_eq!(done.retired, Some(old));
    assert!(
        local_core::sync::SyncCredentials::read_optional(
            &*secrets,
            &retired_item(&restored.base, &old)
        )
        .expect("read")
        .is_some(),
        "the old device's secrets stay aside"
    );

    let dir = pair.desktop.state.path();
    KeptHeads::forget_retired(dir, &old, &*secrets).expect("dropped");
    let kept = KeptHeads::open(dir, account, old, &*secrets).expect("no rollback");
    assert!(kept.kept_of(&account).is_empty());

    let (candidate, _) = accept_the_reset(&mut pair, &mut member, workspace).await;
    assert!(
        member
            .service
            .join_invited(&candidate)
            .await
            .expect("the change verifies")
    );
    assert_eq!(member.service.kept_heads().stopped([account]), None);
    let files = pair.b.files();
    let rebuilt = app_core::sync::workspace::WorkspaceSync::rebuild(
        &mut member.service,
        pair.b.engine.clone(),
        &mut pair.b.registry,
    )
    .await
    .expect("the folder rebuilds");
    assert_eq!(rebuilt.restore_lost(), None);
    assert_eq!(pair.b.files(), files);
}

/// A device you still hold, on the chain your reset retired, learns of the
/// reset, signs in as a new device, and a device on the new chain approves
/// it (§7 step 3). It trusts the workspace once the reset device records
/// the account's head there again (`sync-v2-approval.json`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_device_of_the_retired_chain_comes_back_through_the_new_chain() {
    use app_core::sync::SyncError;
    use std::sync::Arc;

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    let workspace = pair.sync_a.workspace();
    let account = pair.desktop.credentials.account;
    let secrets = Arc::new(MemorySecrets::default());
    let (mut member, done) = reset_devices(&pair.server, account, secrets).await;

    // The old device answers `device_removed`, and no link says why.
    let Err(SyncError::Refused { code, detail, .. }) =
        pair.desktop.service.client().workspaces().await
    else {
        panic!("the session ended");
    };
    assert_eq!(code, "device_removed");
    let new = pair
        .desktop
        .service
        .retired_by_reset(detail.as_deref())
        .await
        .expect("read")
        .expect("a reset");
    assert_eq!(new, done.fingerprint);

    // Signing in moves the old card aside, and the new chain approves.
    let candidate = new_desktop_in(
        &pair.server,
        account,
        &pair.server.base,
        pair.desktop.secrets.clone(),
    )
    .await;
    assert_ne!(
        candidate.candidate.fingerprint(),
        pair.desktop.credentials.card().fingerprint()
    );
    let mut sibling = approve_new(&pair.server, &mut member, candidate, &mut []).await;
    assert_ne!(sibling.id(), pair.desktop.id());
    assert_eq!(
        sibling.service.own_chain().await.expect("own").head_seqno(),
        2
    );
    assert!(
        sibling.service.open_workspace(workspace).await.is_err(),
        "nothing accepted the reset yet"
    );

    let (accepted, _) = accept_the_reset(&mut pair, &mut member, workspace).await;
    assert!(
        member
            .service
            .join_invited(&accepted)
            .await
            .expect("the change verifies")
    );
    let mut rebuilt = app_core::sync::workspace::WorkspaceSync::rebuild(
        &mut member.service,
        pair.b.engine.clone(),
        &mut pair.b.registry,
    )
    .await
    .expect("the folder rebuilds");
    let mut remote = member
        .service
        .open_workspace(workspace)
        .await
        .expect("verified");
    member
        .service
        .record_heads(&mut remote, &[])
        .await
        .expect("the account's head goes in");
    rebuilt
        .round(&mut member.service, 0)
        .await
        .expect("a round");

    let mut folder = sync_support::Folder::new("Sibling");
    let mut downloaded = app_core::sync::workspace::WorkspaceSync::download(
        &mut sibling.service,
        folder.engine.clone(),
        workspace,
        &mut folder.registry,
    )
    .await
    .expect("the sibling trusts the workspace");
    downloaded
        .round(&mut sibling.service, 0)
        .await
        .expect("a round");
    assert_eq!(folder.files(), pair.b.files());
}

/// An owner with no other owner or admin resets and reaches Set up sync
/// again from the new card (§14). The folder's old journal gives the new
/// workspace its name, and the members find the old one closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reset_owner_sets_up_sync_again_from_the_new_card() {
    use app_core::sync::members::MemberChange;
    use app_core::sync::workspace::WorkspaceSync;

    let Pair {
        server,
        mut service,
        laptop,
        a: mut folder,
        mut sync_a,
        mut desktop,
        b,
        mut sync_b,
        proxy: _proxy,
    } = pair_of_accounts_on(Server::start().await).await;
    sync_a
        .change_member(
            &mut service,
            MemberChange::SetRole {
                account: desktop.credentials.account,
                role: sync_protocol::records::Role::Editor,
            },
        )
        .await
        .expect("no other admin");
    sync_a.round(&mut service, 0).await.expect("laptop");
    sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("desktop");
    let old = sync_a.workspace();
    let old_device = laptop.id();
    let files = folder.files();
    drop(sync_a);

    let owner = laptop.credentials.account;
    let (mut card, done) = reset_devices(&server, owner, laptop.secrets.clone()).await;
    assert_eq!(done.retired, Some(old_device));
    let mut fresh = WorkspaceSync::set_up_after_reset(
        &mut card.service,
        folder.engine.clone(),
        &mut folder.registry,
        done.retired,
    )
    .await
    .expect("set up again");
    assert_ne!(fresh.workspace(), old);
    assert_eq!(fresh.name(), "Field notes");
    fresh.round(&mut card.service, 0).await.expect("a round");
    assert_eq!(folder.files(), files);

    let round = sync_b
        .round(&mut desktop.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(round.applied, 0);
    assert_eq!(sync_b.restore_lost(), Some(&Lost::Closed));
    assert_eq!(b.files(), files, "the member keeps its files");
}

/// With an admin left who can accept the reset, the server refuses the new
/// card's close and the folder stays as it was (§14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reset_owner_with_an_admin_keeps_the_accept_reset_path() {
    use app_core::sync::SyncError;
    use app_core::sync::workspace::WorkspaceSync;

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let old = pair.sync_a.workspace();
    let owner = pair.laptop.credentials.account;
    let (mut card, done) = reset_devices(&pair.server, owner, pair.laptop.secrets.clone()).await;
    let refused = WorkspaceSync::set_up_after_reset(
        &mut card.service,
        pair.a.engine.clone(),
        &mut pair.a.registry,
        done.retired,
    )
    .await;
    assert!(matches!(
        refused,
        Err(SyncError::Refused { ref code, .. }) if code == "forbidden"
    ));
    assert_eq!(
        pair.a
            .registry
            .of_folder(pair.a.engine.root())
            .expect("still mapped")
            .workspace,
        old
    );
    assert!(
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .is_ok()
    );
    assert_eq!(pair.sync_b.restore_lost(), None);
}

/// An `accept_reset` a server serves again ends no stop: a device looks
/// only above the policy head it accepted, and the old change neither
/// resumes the workspace nor leaves the device free to read the retired
/// chain short (§14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_old_accept_reset_leaves_the_stop_in_place() {
    use std::sync::Arc;
    use sync_protocol::codec::{Decode, Encode, List, Nested};
    use sync_protocol::messages::PolicyResponse;

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let workspace = pair.sync_a.workspace();
    let account = pair.desktop.credentials.account;
    let secrets = Arc::new(MemorySecrets::default());
    let (mut member, _) = reset_devices(&pair.server, account, secrets).await;
    let (candidate, revision) = accept_the_reset(&mut pair, &mut member, workspace).await;
    assert!(
        member
            .service
            .join_invited(&candidate)
            .await
            .expect("the change verifies")
    );
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    let old = pair
        .service
        .client()
        .policy(workspace, revision - 1)
        .await
        .expect("read")
        .changes
        .0[0]
        .clone();

    // A link of the new chain that the restore loses.
    let copy = pair.server.backup("before.sqlite3").await;
    let _sibling = approve(&pair.server, &mut member, &mut []).await;
    let genesis = member.credentials.reference_head.genesis;
    pair.service
        .follow_account(account, genesis)
        .await
        .expect("kept");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    let restored = pair.server.put_back(&copy, true).await;
    pair.proxy.retarget(&restored.base);
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(
        pair.sync_a.restore_lost(),
        Some(&Lost::MemberAccount(account))
    );

    let digest = old.0.digest();
    pair.proxy.tamper(Box::new(move |path, body| {
        if !path.contains("/policy") {
            return body;
        }
        let mut response = PolicyResponse::from_bytes(&body).expect("a policy response");
        if response.changes.0.iter().any(|c| c.0.digest() == digest) {
            return body;
        }
        let mut changes = vec![Nested(old.0.clone())];
        changes.extend(response.changes.0.clone());
        response.changes = List(changes);
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
        "the old change ended nothing"
    );
    assert!(!pair.service.kept_heads().excused(&account));
}

/// A restore that lost a link of a member's chain: the member resets, an
/// owner compares the approval code and accepts, and both resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_resets_and_the_owner_accepts_the_reset() {
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
    let floors_before = floors(&restored, account);
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
    let secrets = Arc::new(MemorySecrets::default());
    let (mut member, done) = reset_devices(&restored, account, secrets).await;
    assert_ne!(done.fingerprint, sync_protocol::types::Digest::ZERO);

    // Before any request, the owner's device shows the new fingerprint.
    assert_eq!(
        pair.service.changed_chains().await,
        vec![(account, done.fingerprint)]
    );

    // Its device asks the workspace to accept it.
    let (candidate, revision) = accept_the_reset(&mut pair, &mut member, workspace).await;
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
    assert!(
        pair.service.changed_chains().await.is_empty(),
        "the alert ends with the acceptance"
    );

    // The accepted account keeps its history floor.
    assert_eq!(floors(&restored, account), floors_before);
    assert!(!floors_before.is_empty(), "the member joined with a floor");

    // The reset device verifies the change, and its folder bootstraps and
    // matches its files as after a lost journal.
    assert!(
        member
            .service
            .join_invited(&candidate)
            .await
            .expect("the change verifies"),
        "the workspace accepted the reset"
    );
    let files = pair.b.files();
    let rebuilt = app_core::sync::workspace::WorkspaceSync::rebuild(
        &mut member.service,
        pair.b.engine.clone(),
        &mut pair.b.registry,
    )
    .await
    .expect("the folder rebuilds");
    assert_eq!(rebuilt.status().held, 0);
    assert_eq!(pair.b.files(), files);
}

/// A refusal of the member's chain, however the device reached it, leaves
/// the way back open: the owner accepts the reset, and the stop ends for
/// good, in the credential store too (§14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_refused_chain_still_lets_the_owner_accept_the_reset() {
    use std::sync::Arc;

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let copy = pair.server.backup("before.sqlite3").await;
    let workspace = pair.sync_a.workspace();
    let account = pair.desktop.credentials.account;
    let _third = approve(&pair.server, &mut pair.desktop, &mut []).await;
    let genesis = pair.desktop.credentials.reference_head.genesis;
    pair.service
        .follow_account(account, genesis)
        .await
        .expect("the owner keeps the new head");
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    assert_eq!(
        pair.sync_a.restore_lost(),
        Some(&Lost::MemberAccount(account))
    );
    assert!(pair.service.workspace_held(workspace), "the stop shows");
    assert_eq!(
        pair.service
            .open_workspace(workspace)
            .await
            .err()
            .and_then(|e| e.code().map(str::to_owned)),
        Some("sync_account_forked".to_owned()),
        "a read that went around the stop"
    );

    let (mut member, _) =
        reset_devices(&restored, account, Arc::new(MemorySecrets::default())).await;
    accept_the_reset(&mut pair, &mut member, workspace).await;
    for _ in 0..2 {
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("laptop");
    }
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert!(!pair.service.workspace_held(workspace), "the stop ended");
    let heads = app_core::sync::KeptHeads::open(
        pair.laptop.state.path(),
        pair.laptop.credentials.account,
        pair.laptop.id(),
        &*pair.laptop.secrets,
    )
    .expect("the heads open");
    assert_eq!(heads.stopped([account]), None, "nothing brings it back");
}

/// After a relaunch the device holds no verified policy yet, and still
/// compares the chain of a member the last change doesn't record (§14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_relaunch_compares_a_member_the_last_change_left_out() {
    let mut pair = pair_of_accounts_on(Server::start().await).await;
    let workspace = pair.sync_a.workspace();
    let account = pair.desktop.credentials.account;
    pair.sync_a
        .restrict_folder(&mut pair.service, "Pay", &[])
        .await
        .expect("a change that records the owner's head alone");
    rounds(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    let _third = approve(&pair.server, &mut pair.desktop, &mut []).await;
    let genesis = pair.desktop.credentials.reference_head.genesis;
    pair.service
        .follow_account(account, genesis)
        .await
        .expect("the owner keeps the new head");

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a.lose_saves();
    pair.sync_a = app_core::sync::workspace::WorkspaceSync::open(
        pair.a.engine.clone(),
        pair.laptop.credentials.clone(),
        pair.laptop.secrets(),
        &pair.a.registry,
    )
    .expect("the folder opens");
    assert!(
        pair.service.workspace_held(workspace),
        "it waits for the comparison"
    );
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("the comparison ends");
    assert_eq!(
        pair.sync_a.restore_lost(),
        Some(&Lost::MemberAccount(account))
    );
}

/// A member's history floors, from the server's database.
fn floors(server: &Server, account: sync_protocol::types::Id) -> Vec<(Vec<u8>, i64)> {
    let database = server.dir.path().join("data").join(sync_support::DATABASE);
    let connection = rusqlite::Connection::open(database).expect("the database");
    let mut statement = connection
        .prepare("SELECT domain, floor FROM history_floors WHERE account = ?1 ORDER BY domain")
        .expect("the query");
    statement
        .query_map([&account.0[..]], |row| Ok((row.get(0)?, row.get(1)?)))
        .expect("the rows")
        .map(|row| row.expect("a row"))
        .collect()
}

/// A server that lost its secret: the old client meets a fresh server at
/// the same address and gets `unauthorized`. Signing up again gives a new
/// account ID, so this computer reports `sync_restore_lost` for the folder,
/// makes new keys and turns the folder on as a new workspace (§14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_secret_starts_over_under_a_new_account() {
    use app_core::sync::device::{Joined, StoredDevice, join};
    use app_core::sync::workspace::WorkspaceSync;

    let mut pair = pair().await;
    let old_account = pair.laptop.credentials.account;
    let old_device = pair.laptop.id();
    let base = pair.proxy.base.clone();
    let secrets = pair.laptop.secrets.clone();
    let mut stored = StoredDevice::new(
        &base,
        old_account,
        &pair.laptop.credentials.keys,
        "a session the new server never issued",
    );
    stored.reference_head = Some(pair.laptop.credentials.reference_head.clone());
    stored.save(&*secrets).expect("kept");

    let fresh = Server::fresh_behind(&pair.proxy).await;
    pair.a.write("Offline.md", b"# Offline\n");
    let files = pair.a.files();
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(
        matches!(&round, Err(app_core::sync::SyncError::Refused { code, .. }) if code == "unauthorized"),
        "{round:?}"
    );
    assert_eq!(pair.a.files(), files, "no file changed");
    drop(pair.sync_a);

    let account = fresh.person("ana@example.com").await;
    assert_ne!(account, old_account, "a new secret, a new ID");
    let signed_in = fresh.sign_in(account, &base).await;
    let Joined::First(credentials) = join(&signed_in, &*secrets).await.expect("joined") else {
        panic!("a new chain");
    };
    assert_ne!(credentials.keys.device(), old_device, "new device keys");
    let lost = StoredDevice::load(&*secrets, &base)
        .expect("readable")
        .expect("stored")
        .lost_device(&old_account);
    assert_eq!(lost, Some(old_device));

    let opened = WorkspaceSync::open(
        pair.a.engine.clone(),
        credentials.clone(),
        secrets.clone(),
        &pair.a.registry,
    );
    assert_eq!(
        opened.err().and_then(|e| e.code().map(str::to_owned)),
        Some("sync_restore_lost".to_owned())
    );

    let state = tempfile::tempdir().expect("a folder");
    let mut desktop = Desktop::connect(&base, credentials, secrets, state).await;
    let mut sync = WorkspaceSync::set_up_after_reset(
        &mut desktop.service,
        pair.a.engine.clone(),
        &mut pair.a.registry,
        lost,
    )
    .await
    .expect("a new workspace");
    for _ in 0..2 {
        sync.round(&mut desktop.service, 0).await.expect("a round");
    }
    assert_eq!(pair.a.files(), files);
    assert_eq!(sync.status().pending, 0, "the files went up");
    assert_eq!(sync.status().held, 0);
    assert!(sync.rid_at("Offline.md").is_some());
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
    let waits: Vec<u64> = writers
        .iter()
        .map(|w| wait_for(rank(&rid, w, &writers), 200, 60))
        .collect();
    assert!(waits.iter().all(|wait| *wait < 60_000));
    let mut sorted = waits.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(sorted.len(), 200, "no two ranks share a moment");
    assert!(wait_for(rank(&rid, first, &writers), 200, 60) < 300);
    assert!(wait_for(rank(&rid, last, &writers), 200, 60) >= 59_700);
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
    let mut link = LinkToRevoke::make(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    link.revoke(&mut pair).await;
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
    link.revoked_again(&mut pair).await;
}

/// A file that changed since its capture isn't the blob's bytes: the device
/// never encrypts it under the blob's key (§21).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_file_is_never_sealed_under_a_lost_blobs_key() {
    let mut pair = pair().await;
    let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    pair.a.write("photo.png", &bytes);
    rounds(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    for file in &pair.server.blob_files() {
        std::fs::remove_file(file).expect("lost");
    }
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;

    pair.a
        .write("photo.png", b"another picture, not captured yet");
    let uploaded = pair
        .sync_a
        .upload_missing_for_testing(&mut pair.service)
        .await
        .expect("the listing");
    assert_eq!(uploaded, 0);
    assert_eq!(pair.sync_a.sealed_again(), 0);
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
    let mut link = LinkToRevoke::make(&mut pair).await;
    let older = pair.server.backup("older.sqlite3").await;
    pair.a.write("Middle.md", b"# Middle\n");
    rounds(&mut pair).await;
    let newer = pair.server.backup("newer.sqlite3").await;
    link.revoke(&mut pair).await;
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
    link.revoked_again(&mut pair).await;

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
    link.revoked_again(&mut pair).await;
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

/// One run per kind of loss: each stops the workspace with what went, and
/// the app says an owner can set it up again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_envelope_stops_the_workspace() {
    let mut pair = pair().await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    link.revoke(&mut pair).await;
    edit_backup(&copy, "DELETE FROM key_entries WHERE recipient IS NOT NULL");
    let files = pair.a.files();
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert!(
        matches!(
            pair.sync_a.restore_lost(),
            Some(Lost::Envelope | Lost::Epoch)
        ),
        "{:?}",
        pair.sync_a.restore_lost()
    );
    assert_eq!(pair.a.files(), files);
    let notice = app_core::folder_sync::restore_notice(&pair.sync_a.status()).expect("a notice");
    assert!(notice.set_up_again);
    link.revoked_again(&mut pair).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_epoch_record_stops_the_workspace() {
    let mut pair = pair().await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    link.revoke(&mut pair).await;
    edit_backup(
        &copy,
        "UPDATE epochs SET digest = zeroblob(32), epoch = epoch + 1",
    );
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(round.is_ok(), "{round:?}");
    assert!(pair.sync_a.restore_lost().is_some());
    link.revoked_again(&mut pair).await;
}

/// A policy change only the owner's device accepted: it stops, and the
/// member's device, which never saw the change, finds nothing missing and
/// keeps syncing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_policy_change_stops_only_the_devices_that_accepted_it() {
    use app_core::sync::members::MemberChange;
    use sync_protocol::records::Role;

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    rounds(&mut pair).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    link.revoke(&mut pair).await;
    let member = pair.desktop.credentials.account;
    pair.sync_a
        .change_member(
            &mut pair.service,
            MemberChange::SetRole {
                account: member,
                role: Role::Admin,
            },
        )
        .await
        .expect("promoted");

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(pair.sync_a.restore_lost(), Some(&Lost::Policy));
    pair.a.write("Stopped.md", b"# Stays here\n");
    let round = pair
        .sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(round.pushed, 0);

    pair.b.write("Member.md", b"# Member\n");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the member keeps syncing");
    assert_eq!(pair.sync_b.restore_lost(), None);
    assert!(
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .is_ok()
    );
    link.revoked_again(&mut pair).await;
}

/// A lost attachment keeps its `blob_ref`: a new operation reuses it and
/// the bytes go back under the same ID.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_attachment_comes_back_under_the_same_blob() {
    let mut pair = pair().await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    link.revoke(&mut pair).await;
    let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 249) as u8).collect();
    pair.a.write("photo.png", &bytes);
    rounds(&mut pair).await;
    let stored = pair.server.blob_files();
    assert!(!stored.is_empty());

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert!(
        stored.iter().all(|file| file.exists()),
        "the same objects, under the same keys"
    );
    assert_eq!(pair.a.files(), pair.b.files());
    // What the database no longer knew sat in quarantine, and goes after
    // 7 days and no sooner.
    let quarantined = pair.server.quarantined().len() as u64;
    assert!(quarantined > 0);
    assert_eq!(pair.server.collect_quarantine(6).await, 0);
    assert_eq!(pair.server.collect_quarantine(8).await, quarantined);
    link.revoked_again(&mut pair).await;
}

/// An edit that reached no other device while a restore took its older
/// state away stays yours: a file that differs from the state the journal
/// last applied, over a server that kept that state, counts as an edit.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_local_edit_survives_the_rewind() {
    let mut pair = pair().await;
    pair.a.write("Note.md", b"# First\n");
    rounds(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    pair.a.write("Note.md", b"# Unsent edit\n");

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.status().held, 0);
    assert_eq!(pair.a.files()["Note.md"], b"# Unsent edit\n");
    assert_eq!(pair.b.files()["Note.md"], b"# Unsent edit\n");
}

/// A file behind the state another device wrote again goes to review
/// rather than over it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stale_file_can_not_cover_a_newer_state() {
    let mut pair = pair().await;
    pair.a.write("Note.md", b"# One\n");
    rounds(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    // The laptop goes on ahead; the desktop stays behind, with its own edit.
    pair.a.write("Note.md", b"# Two\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    pair.b.write("Note.md", b"# Desktop\n");

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    for _ in 0..4 {
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("laptop");
    }
    assert_eq!(pair.a.files()["Note.md"], b"# Two\n");
    for _ in 0..4 {
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("desktop");
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("laptop");
    }
    assert_eq!(pair.a.files()["Note.md"], b"# Two\n", "the rewrite stands");
    assert!(
        pair.sync_b.status().held > 0 || pair.sync_b.status().conflicts > 0,
        "the desktop's version waits for you"
    );
}

/// A conflict the journal held lists its other branch for review when a
/// restore loses both sides.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_conflict_the_journal_held_lists_every_branch() {
    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    pair.a.write("Plan.md", b"# Laptop\n");
    pair.b.write("Plan.md", b"# Desktop\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("desktop");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    assert_eq!(pair.sync_a.status().conflicts, 1);

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert!(
        pair.sync_a
            .status()
            .to_review
            .iter()
            .any(|path| path == "Plan.md"),
        "{:?}",
        pair.sync_a.status().to_review
    );
    assert!(!pair.sync_a.archived().is_empty(), "its bytes stay");
}

/// A change only a viewer applied stays in its files, listed for review.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_change_only_a_viewer_applied_is_listed_for_review() {
    use app_core::sync::invitations::InvitationBook;
    use sync_protocol::records::Role;

    let mut pair = pair_of_accounts_on(Server::start().await).await;
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
    let copy = pair.server.backup("before.sqlite3").await;
    pair.a.write("Lost.md", b"# Lost\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    sync_v.round(&mut viewer.service, 0).await.expect("viewer");
    assert!(folder.files().contains_key("Lost.md"));

    let restored = pair.server.put_back(&copy, true).await;
    let mut service = viewer.connect_through(&restored.base).await;
    sync_v.set_restore_wait(NO_WAIT);
    sync_v.round(&mut service, 0).await.expect("a round");
    assert_eq!(sync_v.status().to_review, vec!["Lost.md".to_owned()]);
    assert!(sync_v.written_again().is_empty(), "a viewer writes nothing");
    assert!(folder.files().contains_key("Lost.md"));
}

/// A file in a restricted domain comes back from its own rewind, with the
/// domain's log and final heads kept across it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_restricted_domain_rewinds_too() {
    let mut pair = pair().await;
    pair.sync_a
        .restrict_folder(&mut pair.service, "Pay", &[])
        .await
        .expect("restricted");
    rounds(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    pair.a.write("Pay/March.md", b"# March\n");
    rounds(&mut pair).await;
    let files = pair.a.files();

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert_eq!(pair.a.files(), files);
    assert_eq!(pair.b.files(), files);
    let written = [pair.sync_a.written_again(), pair.sync_b.written_again()].concat();
    assert!(
        written.iter().any(|path| path == "Pay/March.md"),
        "{written:?}"
    );
}

/// A workspace editor who only reads a restricted domain writes nothing
/// there: the domain's access list decides, so a device that can write it
/// brings the lost file back and the reader lists its copy for review (§14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_reader_of_a_restricted_domain_writes_nothing_again() {
    let mut pair = pair_of_accounts_with(Server::start().await, Role::Editor).await;
    let grant = Access {
        account: pair.desktop.credentials.account,
        level: Level::Read,
    };
    pair.sync_a
        .restrict_folder(&mut pair.service, "Pay", &[grant])
        .await
        .expect("restricted");
    rounds(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    pair.a.write("Pay/March.md", b"# March\n");
    rounds(&mut pair).await;
    assert!(pair.b.files().contains_key("Pay/March.md"));
    let files = pair.a.files();

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    for _ in 0..3 {
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("the reader rewinds");
    }
    assert_eq!(pair.sync_b.restore_lost(), None);
    assert!(
        pair.sync_b.written_again().is_empty(),
        "it can't write here"
    );
    assert_eq!(
        pair.sync_b.status().to_review,
        vec!["Pay/March.md".to_owned()]
    );
    assert_eq!(
        pair.sync_b.status().refused,
        0,
        "the server refused nothing"
    );

    rounds(&mut pair).await;
    assert_eq!(pair.a.files(), files);
    assert_eq!(pair.b.files(), files);
    assert_eq!(pair.sync_a.written_again(), ["Pay/March.md".to_owned()]);
}

/// A workspace viewer with write access to a restricted domain holds the
/// only copy of a lost change there, and writes it back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_viewer_who_writes_a_restricted_domain_writes_it_again() {
    let mut pair = pair_of_accounts_with(Server::start().await, Role::Viewer).await;
    let grant = Access {
        account: pair.desktop.credentials.account,
        level: Level::Write,
    };
    pair.sync_a
        .restrict_folder(&mut pair.service, "Pay", &[grant])
        .await
        .expect("restricted");
    rounds(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    pair.b.write("Pay/April.md", b"# April\n");
    for _ in 0..2 {
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("the viewer writes");
    }
    assert_eq!(pair.sync_b.status().pending, 0, "it went up");

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_b.restore_lost(), None);
    assert_eq!(pair.sync_b.written_again(), ["Pay/April.md".to_owned()]);
    assert_eq!(pair.a.files(), pair.b.files(), "the owner has it now");
    assert!(pair.a.files().contains_key("Pay/April.md"));
}

/// A backup that lacks the removal of a device that wrote to the domain:
/// the device that accepted the removal stops and writes nothing, and the
/// member who never saw it keeps syncing (§14, §32).
async fn lost_removal_of_a_writer<B: sync_store::Backend>(server: Server<B>)
where
    Server<B>: Restores,
{
    let mut pair = pair_of_accounts_on(server).await;
    rounds(&mut pair).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let workspace = pair.sync_a.workspace();
    let mut third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
    )
    .await;
    let mut remote = third
        .service
        .open_workspace(workspace)
        .await
        .expect("opens");
    third
        .service
        .push_file(&mut remote, "Third.md", b"# Third\n")
        .await
        .expect("written");
    rounds(&mut pair).await;
    assert!(pair.b.files().contains_key("Third.md"));
    let copy = pair.server.snapshot("before.sqlite3").await;

    link.revoke(&mut pair).await;
    pair.laptop
        .service
        .remove_device(third.id(), &[], &mut [])
        .await
        .expect("removed");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    pair.service
        .record_heads(pair.sync_a.remote_mut().expect("connected"), &[])
        .await
        .expect("recorded");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    assert!(!pair.sync_a.removal_hold());

    let restored = pair.server.restored(&copy, None).await;
    reconnect(&mut pair, &restored.base).await;
    let files = pair.a.files();
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(pair.sync_a.restore_lost(), Some(&Lost::OwnAccount));
    assert_eq!(pair.a.files(), files, "the stopped device keeps its files");
    pair.a.write("Stopped.md", b"# Stays here\n");
    let round = pair
        .sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(round.pushed, 0, "it writes nothing");

    pair.b.write("Member.md", b"# Member\n");
    for _ in 0..2 {
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("the member keeps syncing");
    }
    assert_eq!(pair.sync_b.restore_lost(), None);
    assert_eq!(pair.sync_b.status().pending, 0);
    link.revoked_again(&mut pair).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_removal_of_a_writer_stops_the_devices_that_accepted_it() {
    lost_removal_of_a_writer(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_removal_of_a_writer_stops_them_on_postgres() {
    lost_removal_of_a_writer(Server::start_postgres_alone().await).await;
}

/// Two restores in a row, the second from an older backup, end in the same
/// stop: both lost a policy change the laptop accepted, and each raises the
/// generation (§14, §32).
async fn two_restores_end_in_one_stop<B: sync_store::Backend>(server: Server<B>)
where
    Server<B>: Restores,
{
    use app_core::sync::members::MemberChange;

    let mut pair = pair_of_accounts_on(server).await;
    rounds(&mut pair).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let older = pair.server.snapshot("older.sqlite3").await;
    pair.a.write("Middle.md", b"# Middle\n");
    rounds(&mut pair).await;
    let newer = pair.server.snapshot("newer.sqlite3").await;
    link.revoke(&mut pair).await;
    let member = pair.desktop.credentials.account;
    pair.sync_a
        .change_member(
            &mut pair.service,
            MemberChange::SetRole {
                account: member,
                role: Role::Editor,
            },
        )
        .await
        .expect("the role changes");
    let files = pair.a.files();

    let first = pair.server.restored(&newer, None).await;
    reconnect(&mut pair, &first.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(pair.sync_a.restore_lost(), Some(&Lost::Policy));
    let alerted = pair.sync_a.restore_alert().expect("an alert");
    link.revoked_again(&mut pair).await;

    // The generation takes the clock's seconds.
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second = pair.server.restored(&older, None).await;
    reconnect(&mut pair, &second.base).await;
    pair.a.write("Stopped.md", b"# Stays here\n");
    let round = pair
        .sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(round.pushed, 0, "it writes nothing");
    assert_eq!(pair.sync_a.restore_lost(), Some(&Lost::Policy));
    assert!(pair.sync_a.restore_alert().expect("an alert") > alerted);
    let mut expected = files;
    expected.insert("Stopped.md".to_owned(), b"# Stays here\n".to_vec());
    assert_eq!(pair.a.files(), expected);
    link.revoked_again(&mut pair).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_restores_end_in_the_same_stop() {
    two_restores_end_in_one_stop(Server::start().await).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_restores_end_in_the_same_stop_on_postgres() {
    two_restores_end_in_one_stop(Server::start_postgres_alone().await).await;
}

/// An editor device reads no pending requests, and still shows that an
/// account's devices changed, with the new fingerprint, from the chain it
/// may read (§7). It keeps nothing of the new chain.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_editor_device_shows_a_reset_nobody_asked_it_to_accept() {
    let mut pair = pair_of_accounts_with(Server::start().await, Role::Editor).await;
    let workspace = pair.sync_a.workspace();
    let owner = pair.laptop.credentials.account;
    assert!(pair.desktop.service.changed_chains().await.is_empty());

    let secrets = std::sync::Arc::new(MemorySecrets::default());
    let (_new, done) = reset_devices(&pair.server, owner, secrets).await;

    let refused = pair.desktop.service.reset_requests(workspace).await;
    assert!(
        matches!(&refused, Err(app_core::sync::SyncError::Refused { code, .. }) if code == "forbidden"),
        "{refused:?}"
    );
    assert_eq!(
        pair.desktop.service.changed_chains().await,
        vec![(owner, done.fingerprint)]
    );
    assert_eq!(pair.desktop.service.kept_heads().kept_of(&owner).len(), 1);
}

/// A text-mode note: the restore lost its updates, and a writer that holds
/// the document writes a snapshot under the same lineage.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_text_mode_note_comes_back_as_a_snapshot() {
    use editor_core::text_mode::{CollabDocument, Utf16Splice};

    const PLAN: &str = "Plan.md";
    let mut pair = pair().await;
    pair.a.write(PLAN, b"# Plan\n");
    rounds(&mut pair).await;
    let (_, document) = pair.sync_a.open_text(PLAN).expect("the note opens");
    let mut document = document.expect("the document");
    rounds(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    document.edit(
        vec![Utf16Splice {
            start: 7,
            removed: 0,
            inserted: "Lost words.\n".to_owned(),
        }],
        true,
    );
    rounds(&mut pair).await;
    pair.sync_a.close_text(PLAN).expect("closed");
    rounds(&mut pair).await;
    let text = pair.a.files()[PLAN].clone();
    assert!(String::from_utf8_lossy(&text).contains("Lost words."));

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert!(
        String::from_utf8_lossy(&pair.b.files()[PLAN]).contains("Lost words."),
        "{:?}",
        String::from_utf8_lossy(&pair.b.files()[PLAN])
    );
}

/// A device that lost write access keeps a final head; a rewind from
/// sequence 0 fetches its records by chain position, and a server that
/// can't serve them stops the workspace before any file changes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_writers_records_that_disagree_with_its_final_head_stop_the_rewind() {
    use app_core::sync::members::MemberChange;
    use sync_protocol::records::Role;

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    // A checkpoint covering the change would answer the final head without
    // the chain records this test withholds.
    let never = std::time::Duration::from_secs(1 << 40);
    pair.sync_a.set_checkpoint_wait(never);
    pair.sync_b.set_checkpoint_wait(never);
    pair.b.write("Member.md", b"# Member\n");
    rounds(&mut pair).await;
    let member = pair.desktop.credentials.account;
    pair.sync_a
        .change_member(
            &mut pair.service,
            MemberChange::SetRole {
                account: member,
                role: Role::Viewer,
            },
        )
        .await
        .expect("demoted");
    rounds(&mut pair).await;
    let files = pair.a.files();
    let copy = pair.server.backup("before.sqlite3").await;

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.proxy.forge(Box::new(|_, path| {
        path.contains("/chain/")
            .then(|| (404, sync_support::not_found_body()))
    }));
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert!(round.is_ok(), "{round:?}");
    assert!(
        matches!(pair.sync_a.restore_lost(), Some(Lost::FinalHead(Some(_)))),
        "{:?}",
        pair.sync_a.restore_lost()
    );
    assert_eq!(pair.a.files(), files, "no file changed");
}

/// The honest run of the same restore passes: the head and the records
/// form one chain that ends at the final head.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_honest_restore_passes_every_final_head() {
    use app_core::sync::members::MemberChange;
    use sync_protocol::records::Role;

    let mut pair = pair_of_accounts_on(Server::start().await).await;
    pair.b.write("Member.md", b"# Member\n");
    rounds(&mut pair).await;
    let member = pair.desktop.credentials.account;
    pair.sync_a
        .change_member(
            &mut pair.service,
            MemberChange::SetRole {
                account: member,
                role: Role::Viewer,
            },
        )
        .await
        .expect("demoted");
    rounds(&mut pair).await;
    let copy = pair.server.backup("before.sqlite3").await;
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert_eq!(pair.sync_a.restore_lost(), None);
}

/// Two writers race on a rewrite: the one the server took as a patch covers
/// both with a state operation, so nothing stays unreadable.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_writers_that_race_on_a_rewrite_settle() {
    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    write_after_backup(&mut pair).await;
    let files = pair.a.files();
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;

    // Each rewinds and writes with the other's record out of sight: the
    // laptop's push waits until the desktop's went through.
    let (arrived, release) = pair.proxy.hold("/ops");
    let (first, _) = tokio::join!(pair.sync_a.round(&mut pair.service, 0), async {
        tokio::time::timeout(std::time::Duration::from_secs(20), arrived.notified())
            .await
            .expect("the laptop pushes");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("desktop");
        pair.sync_b
            .round(&mut pair.desktop.service, 0)
            .await
            .expect("desktop");
        release.notify_one();
    });
    first.expect("laptop");
    rounds(&mut pair).await;
    assert_eq!(pair.a.files(), files);
    assert_eq!(pair.b.files(), files);
    assert!(
        pair.sync_a.unreadable().is_empty(),
        "nothing stays unreadable"
    );
    assert!(
        pair.sync_b.unreadable().is_empty(),
        "nothing stays unreadable"
    );
}

/// An owner's restore over a review store: the branch whose author is gone
/// survives Set up sync again in the archive, which lists it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_up_sync_again_carries_the_review_store() {
    let mut pair = pair().await;
    pair.a.write("Plan.md", b"# Laptop\n");
    pair.b.write("Plan.md", b"# Desktop\n");
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.status().conflicts, 1);
    let fresh = pair
        .sync_a
        .set_up_again(&mut pair.service, &mut pair.a.registry)
        .await
        .expect("set up");
    assert_eq!(fresh.archived().len(), 1);
    assert_eq!(fresh.archived()[0].path, "Plan.md");
}

/// A root whose marker went while sync was stopped stays held, so its files
/// don't reach the new workspace domain (§14).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_up_sync_again_holds_a_root_whose_marker_went() {
    let mut pair = pair().await;
    pair.sync_a
        .restrict_folder(&mut pair.service, "Pay", &[])
        .await
        .expect("restricted");
    pair.a.write("Pay/March.md", b"# March\n");
    rounds(&mut pair).await;
    std::fs::remove_file(pair.a.path("Pay/.noura-domain")).unwrap();
    let mut fresh = pair
        .sync_a
        .set_up_again(&mut pair.service, &mut pair.a.registry)
        .await
        .expect("set up");
    let held: Vec<String> = fresh.held_roots().into_iter().map(|root| root.0).collect();
    assert_eq!(held, vec!["Pay".to_owned()]);
    fresh.round(&mut pair.service, 0).await.expect("a round");
    assert_eq!(fresh.rid_at("Pay/March.md"), None, "nothing uploaded");
}

/// After a generation change a stopped workspace still owes its revoked
/// links, whatever you dismissed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_generation_change_keeps_the_revocation_due_past_a_dismissal() {
    let mut pair = pair().await;
    let copy = pair.server.backup("before.sqlite3").await;
    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a round");
    assert!(pair.sync_a.revoke_due());
    pair.sync_a.dismiss_restore_alert().expect("dismissed");
    assert!(pair.sync_a.revoke_due());
    pair.sync_a.revoked_again().expect("recorded");
    assert!(!pair.sync_a.revoke_due());
}

/// A failover to an asynchronous standby that lacks the latest commits:
/// nothing marks it, so devices catch the sequences it hands out again and
/// hold; `mark-restored` on the new primary makes them compare and write the
/// lost content again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_failover_to_an_asynchronous_standby_is_a_rollback_until_it_is_marked() {
    let mut pair = pair_on(Server::start_postgres_alone().await).await;
    pair.a.write("Kept.md", b"# Kept\n");
    rounds(&mut pair).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let standby = pair.server.standby();
    let again = pair.server.standby();
    link.revoke(&mut pair).await;
    write_after_backup(&mut pair).await;
    let files = pair.a.files();

    let promoted = pair.server.fail_over(&standby, false).await;
    reconnect(&mut pair, &promoted.base).await;
    pair.a.write("Held.md", b"# Held\n");
    let round = pair.sync_a.round(&mut pair.service, 0).await;
    assert_eq!(
        round
            .err()
            .and_then(|e| e.code().map(str::to_owned))
            .as_deref(),
        Some("sync_server_rollback")
    );
    assert_eq!(pair.b.files(), files, "no file changed");

    let marked = pair.server.fail_over(&again, true).await;
    reconnect(&mut pair, &marked.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.rollback(), None);
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert_eq!(pair.a.files(), pair.b.files());
    assert!(pair.a.files().contains_key("Lost.md"));
    assert!(pair.a.files().contains_key("Held.md"));
    link.revoked_again(&mut pair).await;
}

/// §32's restore runs on PostgreSQL: only content lost, then a second
/// restore from an older copy, which ends like one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_restores_on_postgres_end_like_one() {
    let mut pair = pair_on(Server::start_postgres_alone().await).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let older = pair.server.standby();
    pair.a.write("Middle.md", b"# Middle\n");
    rounds(&mut pair).await;
    let newer = pair.server.standby();
    link.revoke(&mut pair).await;
    write_after_backup(&mut pair).await;
    let files = pair.a.files();

    let first = pair.server.fail_over(&newer, true).await;
    reconnect(&mut pair, &first.base).await;
    rounds(&mut pair).await;
    link.revoked_again(&mut pair).await;
    assert_eq!(pair.a.files(), files);
    tokio::time::sleep(std::time::Duration::from_millis(1100)).await;
    let second = pair.server.fail_over(&older, true).await;
    reconnect(&mut pair, &second.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert_eq!(pair.a.files(), files);
    assert_eq!(pair.b.files(), files);
    link.revoked_again(&mut pair).await;
}

/// A lost link of your own chain stops the workspace on PostgreSQL too.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_account_link_stops_the_workspace_on_postgres() {
    let mut pair = pair_on(Server::start_postgres_alone().await).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let standby = pair.server.standby();
    link.revoke(&mut pair).await;
    let mut third = approve(
        &pair.server,
        &mut pair.laptop,
        std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
    )
    .await;
    let _ = third.service.client();
    rounds(&mut pair).await;
    let files = pair.a.files();
    let restored = pair.server.fail_over(&standby, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(pair.sync_a.restore_lost(), Some(&Lost::OwnAccount));
    assert_eq!(pair.a.files(), files);
    link.revoked_again(&mut pair).await;
}

/// A restore that kept the attachment's objects and lost its rows, on
/// PostgreSQL. The objects wait in quarantine for 7 days, and the device
/// writes the file again under the same blob.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_that_outlived_its_blob_rows_on_postgres() {
    let mut pair = pair_on(Server::start_postgres_alone().await).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let standby = pair.server.standby();
    link.revoke(&mut pair).await;
    let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 247) as u8).collect();
    pair.a.write("photo.png", &bytes);
    rounds(&mut pair).await;
    let stored = pair.server.blob_files();
    assert!(!stored.is_empty());

    let restored = pair.server.fail_over(&standby, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert!(
        stored.iter().all(|file| file.exists()),
        "the same objects, under the same keys"
    );
    assert_eq!(pair.b.files().get("photo.png"), Some(&bytes));
    let quarantined = pair.server.quarantined().len() as u64;
    assert!(quarantined > 0);
    assert_eq!(pair.server.collect_quarantine(6).await, 0);
    assert_eq!(pair.server.collect_quarantine(8).await, quarantined);
    link.revoked_again(&mut pair).await;
}

/// A blob store that lost objects a live operation lists, on PostgreSQL: a
/// device that holds the file uploads them again under the same IDs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_store_that_lost_listed_objects_on_postgres() {
    let mut pair = pair_on(Server::start_postgres_alone().await).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let bytes: Vec<u8> = (0..3_000_000u32).map(|i| (i % 241) as u8).collect();
    pair.a.write("photo.png", &bytes);
    rounds(&mut pair).await;
    let standby = pair.server.standby();
    link.revoke(&mut pair).await;
    let stored = pair.server.blob_files();
    assert!(!stored.is_empty());
    for file in &stored {
        std::fs::remove_file(file).expect("lost");
    }

    let restored = pair.server.fail_over(&standby, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert!(stored.iter().all(|file| file.exists()));
    assert_eq!(pair.a.files(), pair.b.files());
    link.revoked_again(&mut pair).await;
}

/// §32's runs that lose one kind of signed state, on PostgreSQL: each stops
/// the device, which keeps its files and still ends the revoked link.
async fn stops_on_postgres(sql: &str, expected: impl Fn(&Lost) -> bool) {
    let mut pair = pair_on(Server::start_postgres_alone().await).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let standby = pair.server.standby();
    link.revoke(&mut pair).await;
    let files = pair.a.files();
    let restored = pair
        .server
        .fail_over_edited(&standby, true, Some(sql))
        .await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    let lost = pair.sync_a.restore_lost().expect("stopped");
    assert!(expected(lost), "{lost:?}");
    assert_eq!(pair.a.files(), files);
    link.revoked_again(&mut pair).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_envelope_stops_the_workspace_on_postgres() {
    stops_on_postgres(
        "DELETE FROM key_entries WHERE recipient IS NOT NULL",
        |lost| matches!(lost, Lost::Envelope | Lost::Epoch),
    )
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_epoch_record_stops_the_workspace_on_postgres() {
    stops_on_postgres(
        "UPDATE epochs SET digest = decode(repeat('00', 32), 'hex'), epoch = epoch + 1",
        |_| true,
    )
    .await;
}

/// A policy change only the owner's device accepted, on PostgreSQL.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_policy_change_stops_the_workspace_on_postgres() {
    use app_core::sync::members::MemberChange;
    use sync_protocol::records::Role;

    let mut pair = pair_of_accounts_on(Server::start_postgres_alone().await).await;
    rounds(&mut pair).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let standby = pair.server.standby();
    link.revoke(&mut pair).await;
    let member = pair.desktop.credentials.account;
    pair.sync_a
        .change_member(
            &mut pair.service,
            MemberChange::SetRole {
                account: member,
                role: Role::Admin,
            },
        )
        .await
        .expect("promoted");
    let restored = pair.server.fail_over(&standby, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(pair.sync_a.restore_lost(), Some(&Lost::Policy));
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("the member keeps syncing");
    assert_eq!(pair.sync_b.restore_lost(), None);
    link.revoked_again(&mut pair).await;

    // Set up sync again closes the old workspace for its members too.
    let old = pair.sync_a.workspace();
    let mut fresh = pair
        .sync_a
        .set_up_again(&mut pair.service, &mut pair.a.registry)
        .await
        .expect("set up again");
    assert_ne!(fresh.workspace(), old);
    fresh.round(&mut pair.service, 0).await.expect("a round");
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("a stopped round");
    assert_eq!(pair.sync_b.restore_lost(), Some(&Lost::Closed));
}

/// A restore that lost a link of a member's chain, on PostgreSQL: the
/// member resets, the owner compares the code and accepts, and both resume.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_member_resets_and_the_owner_accepts_on_postgres() {
    use std::sync::Arc;

    let mut pair = pair_of_accounts_on(Server::start_postgres_alone().await).await;
    rounds(&mut pair).await;
    let mut link = LinkToRevoke::make(&mut pair).await;
    let standby = pair.server.standby();
    link.revoke(&mut pair).await;
    let workspace = pair.sync_a.workspace();
    let account = pair.desktop.credentials.account;
    let _third = approve(&pair.server, &mut pair.desktop, &mut []).await;
    let genesis = pair.desktop.credentials.reference_head.genesis;
    pair.service
        .follow_account(account, genesis)
        .await
        .expect("the owner keeps the new head");
    rounds(&mut pair).await;

    let restored = pair.server.fail_over(&standby, true).await;
    reconnect(&mut pair, &restored.base).await;
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    assert_eq!(
        pair.sync_a.restore_lost(),
        Some(&Lost::MemberAccount(account))
    );
    let secrets = Arc::new(MemorySecrets::default());
    let (mut member, _) = reset_devices(&restored, account, secrets).await;
    let (candidate, _) = accept_the_reset(&mut pair, &mut member, workspace).await;
    for _ in 0..2 {
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("laptop");
    }
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert!(
        member
            .service
            .join_invited(&candidate)
            .await
            .expect("the change verifies")
    );
    link.revoked_again(&mut pair).await;
}

/// A snapshot blob a restore lost: no file holds it, so a writer that holds
/// the document writes a snapshot that covers the operation.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_text_blob_is_covered_by_a_new_snapshot() {
    use editor_core::text_mode::{CollabDocument, Utf16Splice};

    const PLAN: &str = "Plan.md";
    let mut pair = pair().await;
    pair.a.write(PLAN, b"# Plan\n");
    rounds(&mut pair).await;
    let (_, document) = pair.sync_a.open_text(PLAN).expect("the note opens");
    let mut document = document.expect("the document");
    rounds(&mut pair).await;
    let long = "A long paragraph that fills a blob. ".repeat(50_000);
    document.edit(
        vec![Utf16Splice {
            start: 7,
            removed: 0,
            inserted: long,
        }],
        true,
    );
    rounds(&mut pair).await;
    let rid = pair.sync_a.rid_at(PLAN).expect("the note");
    pair.sync_a
        .write_snapshot_for_testing(rid)
        .expect("a snapshot");
    rounds(&mut pair).await;
    pair.sync_a.close_text(PLAN).expect("closed");
    rounds(&mut pair).await;
    let kept = pair.sync_a.kept_text_keys_for_testing(&rid);
    assert!(!kept.is_empty(), "a record whose bytes travel in a blob");
    assert!(
        kept.iter().all(|key| key.expose() == &[0; 32]),
        "a kept text record holds no blob key (§15)"
    );
    let copy = pair.server.backup("before.sqlite3").await;
    let lost = pair.server.blob_files();
    assert!(!lost.is_empty(), "the snapshot travels in a blob");
    for file in &lost {
        std::fs::remove_file(file).expect("lost");
    }

    let restored = pair.server.put_back(&copy, true).await;
    reconnect(&mut pair, &restored.base).await;
    rounds(&mut pair).await;
    assert_eq!(pair.sync_a.restore_lost(), None);
    assert!(
        !pair.server.blob_files().is_empty(),
        "a new snapshot's blob stands in for the lost one"
    );
    assert_eq!(pair.a.files(), pair.b.files());
    let written = pair.server.blob_files().len();
    rounds(&mut pair).await;
    assert_eq!(
        pair.server.blob_files().len(),
        written,
        "one snapshot per loss"
    );
}

/// M7's harness: many devices of one account all applied a state the
/// restore lost. Each waits by its rank for that object, so one writes it
/// again and the others find it written, and none gets `conflict_limit`.
/// `NOURA_RESTORE_DEVICES` sets the number of devices; CI runs 200.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn many_devices_pick_one_writer_for_a_lost_object() {
    use app_core::sync::workspace::WorkspaceSync;
    use sync_support::Folder;

    let count: usize = std::env::var("NOURA_RESTORE_DEVICES")
        .ok()
        .and_then(|text| text.parse().ok())
        .unwrap_or(8);
    let wait = std::time::Duration::from_secs((count as u64 / 3).clamp(4, 60));
    let mut pair = pair().await;
    let workspace = pair.sync_a.workspace();
    let mut devices = Vec::new();
    for index in 0..count {
        let mut desktop = approve(
            &pair.server,
            &mut pair.laptop,
            std::slice::from_mut(pair.sync_a.remote_mut().expect("connected")),
        )
        .await;
        pair.sync_a
            .round(&mut pair.service, 0)
            .await
            .expect("laptop");
        let mut folder = Folder::new(&format!("Device {index}"));
        let mut sync = WorkspaceSync::download(
            &mut desktop.service,
            folder.engine.clone(),
            workspace,
            &mut folder.registry,
        )
        .await
        .expect("it downloads");
        sync.round(&mut desktop.service, 0).await.expect("a round");
        devices.push((desktop, folder, sync));
        // The server caps pending requests, and answered ones count until
        // they expire.
        if index % 16 == 15 {
            let database = pair
                .server
                .dir
                .path()
                .join("data")
                .join(sync_support::DATABASE);
            rusqlite::Connection::open(database)
                .expect("the database")
                .execute("DELETE FROM approval_requests", [])
                .expect("cleared");
        }
    }
    pair.sync_b
        .round(&mut pair.desktop.service, 0)
        .await
        .expect("desktop");
    let copy = pair.server.backup("before.sqlite3").await;
    pair.a.write("Lost.md", b"# Lost\n");
    pair.sync_a
        .round(&mut pair.service, 0)
        .await
        .expect("laptop");
    for (desktop, folder, sync) in &mut devices {
        sync.round(&mut desktop.service, 0).await.expect("a round");
        assert!(folder.files().contains_key("Lost.md"));
    }

    // The laptop stays away, so only the devices that applied it decide.
    let restored = pair.server.put_back(&copy, true).await;
    let mut services = Vec::new();
    for (desktop, _, sync) in &mut devices {
        services.push(desktop.connect_through(&restored.base).await);
        sync.set_restore_wait(wait);
    }
    // Each device's wait starts at its first round here, and on a slow
    // machine that one pass can outlast the wait, so the clock starts after
    // it and at least one more pass follows.
    let mut waited: Option<std::time::Instant> = None;
    while waited.is_none_or(|from| from.elapsed() < wait + std::time::Duration::from_secs(3)) {
        let pass = std::time::Instant::now();
        for ((_, _, sync), service) in devices.iter_mut().zip(services.iter_mut()) {
            match sync.round(service, 0).await {
                Ok(_) => {}
                Err(error) => {
                    assert_ne!(error.code(), Some("conflict_limit"));
                    panic!("{error:?}");
                }
            }
        }
        eprintln!("a pass over {count} devices took {:?}", pass.elapsed());
        waited.get_or_insert_with(std::time::Instant::now);
    }
    let writers = devices
        .iter()
        .filter(|(_, _, sync)| sync.written_again().iter().any(|path| path == "Lost.md"))
        .count();
    assert!(writers >= 1, "a writer wrote the object again");
    assert!(
        writers <= count.div_ceil(8) + 1,
        "{writers} of {count} devices wrote it"
    );
    for ((_, folder, sync), service) in devices.iter_mut().zip(services.iter_mut()) {
        // A device idle since the last pass can find its connection closed;
        // its next round reconnects, as it does in the app.
        if let Err(error) = sync.round(service, 0).await {
            assert!(
                matches!(error, app_core::sync::SyncError::Network(_)),
                "{error:?}"
            );
            sync.round(service, 0).await.expect("a round");
        }
        assert_eq!(
            folder.files().get("Lost.md").map(Vec::as_slice),
            Some(&b"# Lost\n"[..])
        );
    }
}
