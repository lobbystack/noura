//! A pull answered with status 4 or 5 (seam 4, sync-v2.md §15): the
//! client fetches the policy, and once a change it accepted took its access
//! it stops pulling the domain. A device whose account `remove_member`
//! removed shows by which device and account, and its files stay as they
//! are.

mod sync_support;

use app_core::folder_sync::FolderSyncScreen;
use app_core::sync::workspace::WorkspaceSync;
use sync_key_envelope::v2::{EnvelopeHeader, unwrap, wrap};
use sync_protocol::codec::{Encode, Sorted};
use sync_protocol::messages::KeyRecord;
use sync_protocol::records::{AccountHead, Envelope, PolicyChange, PolicyKind, Role, Signed};
use sync_protocol::types::Id;
use sync_store::{Backend, WriteTxn};
use sync_support::{Desktop, Folder, Server, approve, first_desktop, pair};

/// The next policy change after the server's head, signed by `signer`.
async fn next_change(
    signer: &mut Desktop,
    workspace: Id,
    heads: Vec<AccountHead>,
    kind: PolicyKind,
) -> Signed<PolicyChange> {
    let changes = signer.service.client().policy(workspace, 0).await.unwrap();
    let last = &changes.changes.0.last().unwrap().0;
    Signed::sign(
        PolicyChange {
            workspace,
            revision: last.body.revision + 1,
            previous: last.digest(),
            signer: signer.credentials.keys.device(),
            heads: Sorted::new(heads).unwrap(),
            kind,
        },
        signer.credentials.keys.signing(),
    )
}

/// Wraps the workspace domain's epoch 1 key from `from`'s envelope for
/// `to`.
async fn envelope_for(
    from: &mut Desktop,
    to: &Desktop,
    workspace: Id,
    domain: Id,
) -> Signed<Envelope> {
    let wrapper = from.id();
    let keys = from
        .service
        .client()
        .keys(workspace, 0, wrapper)
        .await
        .unwrap();
    let own = keys
        .entries
        .0
        .iter()
        .find_map(|e| match &e.record {
            KeyRecord::Envelope(env) => Some(env.clone()),
            _ => None,
        })
        .expect("the wrapper's envelope");
    let from_keys = &from.credentials.keys;
    let key = unwrap(
        &own.body,
        from_keys,
        &from_keys.card(from.credentials.account),
    )
    .unwrap();
    wrap(
        &EnvelopeHeader {
            workspace,
            domain,
            epoch: 1,
            wrapping_device: wrapper,
        },
        &key,
        &to.credentials.keys.card(to.credentials.account),
        from_keys,
    )
    .unwrap()
}

/// Commits `change` in the server's store, as its push will once the
/// invitation flow (ticket 50) sends it.
async fn commit(server: &Server, domain: Id, change: Signed<PolicyChange>) {
    sync_store::conformance::inject_change(&server.store, change, vec![domain], None)
        .await
        .expect("the change commits");
}

/// Stores `envelope` in the key stream, as the push of the change that
/// brings its recipient in will.
async fn store_envelope(server: &Server, workspace: Id, domain: Id, envelope: Signed<Envelope>) {
    server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                txn.append_key_entry(
                    workspace,
                    4,
                    domain,
                    1,
                    Some(envelope.body.recipient),
                    &envelope.to_bytes(),
                )
                .await
                .map(|_| ())
            })
        }))
        .await
        .expect("the envelope is stored");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_removed_member_stops_and_shows_who_removed_it() {
    let mut pair = pair().await;
    let workspace = pair.sync_a.workspace();
    let domain = pair.sync_a.remote_mut().unwrap().domain;
    let owner_head = pair.laptop.service.own_chain().await.unwrap().head();

    // Ben's account joins the workspace as an editor.
    let mut ben = first_desktop(&pair.server, "ben@example.com").await;
    let ben_head = ben.service.own_chain().await.unwrap().head();
    let add = next_change(
        &mut pair.laptop,
        workspace,
        vec![owner_head.clone(), ben_head.clone()],
        PolicyKind::AddMember {
            account: ben.credentials.account,
            role: Role::Editor,
            invitation: Id::random().unwrap(),
        },
    )
    .await;
    commit(&pair.server, domain, add).await;
    let account = ben.credentials.account;
    pair.server
        .store
        .backend()
        .write(Box::new(move |txn: &mut sync_store_sqlite::SqliteTxn| {
            Box::pin(async move {
                txn.upsert_member(workspace, account, Role::Editor.code())
                    .await
            })
        }))
        .await
        .expect("the membership is stored");
    let envelope = envelope_for(&mut pair.laptop, &ben, workspace, domain).await;
    store_envelope(&pair.server, workspace, domain, envelope).await;
    // Ben approves another computer, and his device records the new head
    // there, which anchors its trust in the workspace (§8).
    let _second = approve(&pair.server, &mut ben, &mut []).await;
    let ben_head = ben.service.own_chain().await.unwrap().head();
    let heads = next_change(
        &mut ben,
        workspace,
        vec![owner_head.clone(), ben_head],
        PolicyKind::AccountHeads,
    )
    .await;
    commit(&pair.server, domain, heads).await;
    let mut c = Folder::new("Ben's copy");
    let mut sync_c = WorkspaceSync::download(
        &mut ben.service,
        c.engine.clone(),
        workspace,
        &mut c.registry,
    )
    .await
    .expect("Ben downloads the workspace");
    sync_c.round(&mut ben.service, 0).await.expect("a round");
    assert_eq!(c.files()["Plan.md"], b"# Plan\n");

    // The laptop removes Ben's account.
    let remove = next_change(
        &mut pair.laptop,
        workspace,
        vec![owner_head],
        PolicyKind::RemoveMember {
            account: ben.credentials.account,
        },
    )
    .await;
    commit(&pair.server, domain, remove).await;
    c.write("Notes.md", b"# Written after the removal\n");
    sync_c
        .round(&mut ben.service, 0)
        .await
        .expect("the removal isn't an error");
    let ended = sync_c.status().ended.expect("sync ended");
    assert_eq!(ended.by_device, pair.laptop.id());
    assert_eq!(ended.by_account, Some(pair.laptop.credentials.account));
    // Its files stay as they are, and it sends and fetches nothing more.
    assert_eq!(c.files()["Plan.md"], b"# Plan\n");
    assert_eq!(c.files()["Notes.md"], b"# Written after the removal\n");
    let round = sync_c.round(&mut ben.service, 0).await.expect("a round");
    assert_eq!((round.captured, round.pushed, round.applied), (0, 0, 0));
    let screen = FolderSyncScreen::of(&sync_c.status(), sync_c.paused());
    assert!(
        screen.ended.as_ref().is_some_and(
            |line| line.contains("removed your account") && line.contains("Your files stay")
        ),
        "{:?}",
        screen.ended
    );

    // Across a relaunch too.
    drop(sync_c);
    let sync_c = WorkspaceSync::open(
        c.engine.clone(),
        ben.credentials.clone(),
        ben.secrets(),
        &c.registry,
    )
    .expect("the journal opens");
    assert!(sync_c.status().ended.is_some());
}

/// A member invited back resumes in the folder they kept: their chain goes
/// on (§8), the first round after the change finds its access again, and
/// an invitation accepted here doesn't wait for the next look.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_member_invited_back_resumes_in_the_same_folder() {
    use app_core::sync::invitations::InvitationBook;
    use app_core::sync::members::MemberChange;
    use sync_support::{invite, invite_and_join};

    let server = Server::start().await;
    let mut ana = first_desktop(&server, "ana@example.com").await;
    let mut ben = first_desktop(&server, "ben@example.com").await;
    let mut a = Folder::new("Field notes");
    a.write("Plan.md", b"# Plan\n");
    let mut sync_a = WorkspaceSync::turn_on(
        &mut ana.service,
        a.engine.clone(),
        "Field notes",
        &mut a.registry,
    )
    .await
    .expect("sync turns on");
    sync_a.round(&mut ana.service, 0).await.expect("a round");
    let mut book = InvitationBook::open(ana.state.path(), ana.id()).expect("the book");
    let (mut sync_b, b) =
        invite_and_join(&mut ana, &mut sync_a, &mut book, &mut ben, Role::Editor).await;
    assert_eq!(b.files()["Plan.md"], b"# Plan\n");

    let account = ben.credentials.account;
    sync_a
        .change_member(&mut ana.service, MemberChange::Remove(account))
        .await
        .expect("removed");
    sync_b.round(&mut ben.service, 0).await.expect("a round");
    assert!(sync_b.status().ended.is_some());

    // Ana writes while Ben is away, and Ben writes on his own copy.
    a.write("Away.md", b"# Away\n");
    sync_a.round(&mut ana.service, 0).await.expect("a round");
    b.write("Mine.md", b"# Mine\n");

    invite(&mut ana, &mut sync_a, &mut book, &mut ben, Role::Editor).await;
    sync_b.round(&mut ben.service, 0).await.expect("a round");
    assert!(
        sync_b.status().ended.is_none(),
        "the round found the access"
    );
    for _ in 0..3 {
        sync_b.round(&mut ben.service, 0).await.expect("a round");
        sync_a.round(&mut ana.service, 0).await.expect("a round");
    }
    assert_eq!(b.files()["Away.md"], b"# Away\n");
    assert_eq!(a.files()["Mine.md"], b"# Mine\n");

    // Removed again: a round looks once, and stays ended.
    sync_a
        .change_member(&mut ana.service, MemberChange::Remove(account))
        .await
        .expect("removed");
    sync_b.round(&mut ben.service, 0).await.expect("a round");
    sync_b.round(&mut ben.service, 0).await.expect("a round");
    assert!(sync_b.status().ended.is_some());
    invite(&mut ana, &mut sync_a, &mut book, &mut ben, Role::Editor).await;
    // The join doesn't wait for the next look.
    assert!(
        sync_b
            .check_access_again(&mut ben.service)
            .await
            .expect("looked")
    );
    assert!(sync_b.status().ended.is_none());
}
