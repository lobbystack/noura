//! Reset devices, `accept_reset` and closing a workspace (§7, §8, §10,
//! §14), on both stores.

use std::time::Duration;

use sync_key_envelope::v2::DeviceKeys;
use sync_protocol::codec::{Encode, Nested, Text};
use sync_protocol::messages::{ApprovalRequest, RecordRef, ResetRequest};
use sync_protocol::records::{
    AccountHead, AccountLink, LinkKind, PolicyKind, Role, Signed, approval_commit,
};
use sync_protocol::types::{Id, Key};

use super::invitations::Invitee;
use super::{World, genesis_link, head_of, push_failure, random_token, refusal};
use crate::{Backend, ReadTxn, Session, Store, WriteTxn};

/// Gives an account an address, as signing up does.
async fn sign_up<B: Backend>(store: &Store<B>, account: Id) {
    let email = format!("{}@example.com", sync_protocol::types::to_hex(&account.0));
    store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move { txn.insert_account(account, &email).await })
        }))
        .await
        .expect("the account row");
}

/// A new card of `account`, enrolled, with a session.
async fn new_card<B: Backend>(store: &Store<B>, account: Id) -> (DeviceKeys, Session) {
    let keys = DeviceKeys::generate().expect("random");
    store
        .enroll_card(keys.card(account))
        .await
        .expect("enrolled");
    let session = Session::new(account, keys.device());
    store
        .create_session(random_token(), session)
        .await
        .expect("a session");
    (keys, session)
}

fn reset(code: &str, genesis: Signed<AccountLink>) -> ResetRequest {
    ResetRequest {
        code: Text(code.to_owned()),
        genesis: Nested(genesis),
    }
}

/// A reset of `account`: a new card and its genesis, with a code issued.
async fn reset_account<B: Backend>(
    store: &Store<B>,
    account: Id,
) -> (DeviceKeys, Session, Signed<AccountLink>) {
    let (keys, session) = new_card(store, account).await;
    let (code, _) = store.issue_reset_code(session).await.expect("a code");
    let genesis = genesis_link(&keys, account);
    store
        .reset_account(session, reset(&code, genesis.clone()))
        .await
        .expect("the reset");
    (keys, session, genesis)
}

/// The approval request a reset device posts to `workspace`.
fn reset_request(
    keys: &DeviceKeys,
    account: Id,
    head: &AccountHead,
    workspace: Id,
) -> ApprovalRequest {
    let nc = Key(sync_protocol::crypto::random_array().expect("random"));
    ApprovalRequest {
        commit: approval_commit(&keys.card(account).fingerprint(), Some(head), &nc),
        account: Some(head.clone()),
        invitation: None,
        invite_proof: None,
        workspace: Some(workspace),
    }
}

/// A reset code works once for 10 minutes; a wrong, used or expired one
/// gets `reset_code_invalid`. The reset retires the chain in one
/// transaction: it ends the sessions of every card on it, serves it as
/// retired and never lets one of its cards enroll again (§7).
pub async fn reset_codes_retire_the_chain<B: Backend>(store: Store<B>) {
    let store = std::sync::Arc::new(store);
    let (member, others) = Invitee::with_cards(&store, 2).await;
    let account = member.account;
    sign_up(&store, account).await;
    let (keys, session) = new_card(&store, account).await;
    let genesis = genesis_link(&keys, account);

    let (code, email) = store.issue_reset_code(session).await.expect("a code");
    assert_eq!(code.len(), 8);
    assert!(email.ends_with("@example.com"));
    assert!(
        code.bytes()
            .all(|b| crate::devices::USER_CODE_ALPHABET.contains(&b))
    );

    let wrong = if code == "BBBBBBBB" {
        "CCCCCCCC"
    } else {
        "BBBBBBBB"
    };
    let failed = store
        .reset_account(session, reset(wrong, genesis.clone()))
        .await;
    assert_eq!(refusal(failed).code, "reset_code_invalid");

    // The code expires after 10 minutes.
    store.clock().advance(Duration::from_secs(601));
    let expired = store
        .reset_account(session, reset(&code, genesis.clone()))
        .await;
    assert_eq!(refusal(expired).code, "reset_code_invalid");

    let (code, _) = store.issue_reset_code(session).await.expect("a code");
    // A genesis of a card the old chain lists is refused.
    let old = genesis_link(&member.keys, account);
    let failed = store.reset_account(session, reset(&code, old)).await;
    assert_eq!(refusal(failed).code, "forbidden");

    store
        .reset_account(session, reset(&code, genesis.clone()))
        .await
        .expect("the reset");
    let used = store
        .reset_account(session, reset(&code, genesis.clone()))
        .await;
    assert_eq!(refusal(used).code, "reset_code_invalid");

    // The old cards lost their sessions, and can't come back.
    assert_eq!(
        refusal(store.enroll_card(others[0].card(account)).await).code,
        "id_conflict"
    );
    assert_eq!(
        refusal(store.enroll_card(member.keys.card(account)).await).code,
        "id_conflict"
    );
}

/// Each workspace accepts a reset as the commit of an approval (§7, §8):
/// the server takes the request only from a member whose current chain
/// isn't the one the workspace records, an owner or admin of another
/// account answers, and it refuses an `accept_reset` without that request,
/// from the reset account itself, or for a chain that isn't current. The
/// old cards leave, the new ones come in with their envelopes, and the
/// account keeps its role.
pub async fn accept_reset_commits_an_approval<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let member = super::invitations::member(&mut world, Role::Editor).await;
    let editor = super::invitations::member(&mut world, Role::Editor).await;
    let account = member.account;
    sign_up(&world.store, account).await;

    let (keys, session, genesis) = reset_account(&world.store, account).await;
    let new_head = head_of(std::slice::from_ref(&genesis));

    // The server takes a request only for a chain the workspace doesn't
    // record: another member, whose chain it does, gets `forbidden`.
    let other = reset_request(
        &editor.keys,
        editor.account,
        &editor.head(),
        world.workspace,
    );
    assert_eq!(
        refusal(
            world
                .store
                .post_approval_request(editor.session, other.to_bytes())
                .await
        )
        .code,
        "forbidden"
    );

    // No request yet: the change is refused.
    let accept = |world: &World<B>| {
        world.change(
            0,
            vec![head_of(&world.links), new_head.clone()],
            PolicyKind::AcceptReset { account },
        )
    };
    let first = accept(&world);
    let refused = world.push_records(0, vec![RecordRef::Policy(first)]).await;
    assert_eq!(push_failure(refused), ("forbidden", 0));

    let request = reset_request(&keys, account, &new_head, world.workspace);
    world
        .store
        .post_approval_request(session, request.to_bytes())
        .await
        .expect("the request");
    let pending = world
        .store
        .pending_devices(world.devices[0].session, world.workspace)
        .await
        .expect("listed");
    assert_eq!(pending.pending.0.len(), 1);
    assert_eq!(pending.pending.0[0].workspace, Some(world.workspace));

    // A head off the account's current chain is refused.
    let stale = world.change(
        0,
        vec![head_of(&world.links), member.head()],
        PolicyKind::AcceptReset { account },
    );
    let refused = world.push_records(0, vec![RecordRef::Policy(stale)]).await;
    assert_ne!(push_failure(refused).0, "");

    // The envelope for the new card comes with it.
    let change = accept(&world);
    let envelope = world.envelope(0, &keys.card(account), 1, &world.epoch_key);
    let pushed = world
        .push_records(
            0,
            vec![
                RecordRef::Policy(change.clone()),
                RecordRef::Envelope(envelope),
            ],
        )
        .await
        .expect("the change commits");
    assert!(pushed.policy_revision > 1);
    let policy = world
        .store
        .policy(session, world.workspace, 0)
        .await
        .expect("the new card reads the policy");
    assert!(!policy.changes.0.is_empty());
}

/// An account accepted after a reset keeps its history floor (ticket 60):
/// from the new card, pulls answer status 3 below it, other routes
/// `before_history_floor`, and `live`, `manifest` and blobs serve only as
/// §9 says, through a real `accept_reset`.
pub async fn an_accepted_account_keeps_its_history_floor<B: Backend>(store: Store<B>) {
    let mut run = super::checkpoints::world_with_a_floor(store).await;
    let account = run.invitee.account;
    sign_up(&run.world.store, account).await;
    let (keys, session, genesis) = reset_account(&run.world.store, account).await;
    let new_head = head_of(std::slice::from_ref(&genesis));
    let request = reset_request(&keys, account, &new_head, run.world.workspace);
    run.world
        .store
        .post_approval_request(session, request.to_bytes())
        .await
        .expect("the request");
    let world = &mut run.world;
    let change = world.change(
        0,
        vec![head_of(&world.links), new_head],
        PolicyKind::AcceptReset { account },
    );
    let envelope = world.envelope(0, &keys.card(account), 1, &world.epoch_key);
    let pushed = world
        .push_records(
            0,
            vec![
                RecordRef::Policy(change.clone()),
                RecordRef::Envelope(envelope),
            ],
        )
        .await
        .expect("the change commits");
    world.head = (pushed.policy_revision, change.digest());
    super::checkpoints::check_the_floor(&run, session).await;
}

/// A device of the owner account closes a workspace; from then on every
/// route answers `not_found`, and the account's list drops it (§14).
pub async fn closing_a_workspace_answers_not_found<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let member = super::invitations::member(&mut world, Role::Admin).await;
    assert_eq!(
        refusal(
            world
                .store
                .close_workspace(member.session, world.workspace)
                .await
        )
        .code,
        "forbidden",
        "an admin can't close it"
    );
    let owner = world.devices[0].session;
    let link = super::shares::Link::new();
    world
        .store
        .create_share(
            owner,
            world.workspace,
            link.creation(world.domain, 30, "# Plan\n"),
        )
        .await
        .expect("a link");
    assert!(world.store.read_share(link.token).await.is_ok());
    world
        .store
        .close_workspace(owner, world.workspace)
        .await
        .expect("closed");
    assert_eq!(
        refusal(world.store.close_workspace(owner, world.workspace).await).code,
        "not_found"
    );
    assert_eq!(
        refusal(world.store.policy(owner, world.workspace, 0).await).code,
        "not_found"
    );
    assert_eq!(
        refusal(
            world
                .push_records(0, vec![RecordRef::Operation(world.op(0, |_| {}))])
                .await
        )
        .code,
        "not_found"
    );
    assert_eq!(
        refusal(world.store.read_share(link.token).await).code,
        "not_found",
        "the link stops with its workspace"
    );
    let blob = Id::random().expect("random");
    assert_eq!(
        refusal(
            world
                .store
                .blob_download(owner, world.workspace, blob)
                .await
        )
        .code,
        "not_found"
    );
    assert_eq!(
        refusal(world.store.missing_blobs(owner, world.workspace).await).code,
        "not_found"
    );
    assert_eq!(
        refusal(world.store.realtime_watch(owner, world.workspace).await).code,
        "not_found"
    );
    let listed = world
        .store
        .workspaces(member.session)
        .await
        .expect("listed");
    assert!(listed.workspaces.0.is_empty());
    // Maintenance reclaims the domains.
    world.store.maintain().await.expect("a pass");
}

/// An owner who reset their devices reaches Set up sync again from the new
/// card, when nobody else can accept the reset (§14): the account's chain
/// moved past the head the workspace records, and no other owner or admin
/// is active there.
pub async fn a_reset_owner_closes_a_workspace_nobody_else_can_accept<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let editor = super::invitations::member(&mut world, Role::Editor).await;
    let owner = world.account;
    sign_up(&world.store, owner).await;
    let old = world.devices[0].session;
    let (_, session, _) = reset_account(&world.store, owner).await;

    // A card of the account that isn't on the new chain gets nothing.
    let (_, waiting) = new_card(&world.store, owner).await;
    assert_eq!(
        refusal(world.store.close_workspace(waiting, world.workspace).await).code,
        "forbidden"
    );
    // The retired card has no place on the account's current chain.
    assert_eq!(
        refusal(world.store.close_workspace(old, world.workspace).await).code,
        "forbidden"
    );
    assert_eq!(
        refusal(
            world
                .store
                .close_workspace(editor.session, world.workspace)
                .await
        )
        .code,
        "forbidden"
    );
    // Nothing else opened: until a change accepts it, the new card reads
    // nothing in the workspace.
    assert!(
        world
            .store
            .policy(session, world.workspace, 0)
            .await
            .is_err()
    );
    assert!(
        world
            .store
            .keys(session, world.workspace, 0, session.device)
            .await
            .is_err()
    );
    assert!(
        world
            .store
            .missing_blobs(session, world.workspace)
            .await
            .is_err()
    );

    world
        .store
        .close_workspace(session, world.workspace)
        .await
        .expect("closed");
    assert_eq!(
        refusal(world.store.policy(editor.session, world.workspace, 0).await).code,
        "not_found"
    );
}

/// The closing right of a reset owner stops at the rule of §14: another
/// active owner or admin keeps the `accept_reset` path, another reset
/// account doesn't, and neither does an owner whose workspace accepted the
/// reset, whose new card is an ordinary device there.
pub async fn a_reset_owner_with_another_admin_can_not_close<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let admin = super::invitations::member(&mut world, Role::Admin).await;
    let owner = world.account;
    sign_up(&world.store, owner).await;
    let (_, session, _) = reset_account(&world.store, owner).await;

    assert_eq!(
        refusal(world.store.close_workspace(session, world.workspace).await).code,
        "forbidden",
        "the admin can accept the reset"
    );
    // The admin isn't an owner: it never closes, reset or not.
    assert_eq!(
        refusal(
            world
                .store
                .close_workspace(admin.session, world.workspace)
                .await
        )
        .code,
        "forbidden"
    );

    // The admin resets too: nobody is left to accept either reset, and the
    // owner's new card closes the workspace, while the admin's doesn't.
    sign_up(&world.store, admin.account).await;
    let (_, admin_session, _) = reset_account(&world.store, admin.account).await;
    assert_eq!(
        refusal(
            world
                .store
                .close_workspace(admin_session, world.workspace)
                .await
        )
        .code,
        "forbidden",
        "a reset admin holds no closing right"
    );
    world
        .store
        .close_workspace(session, world.workspace)
        .await
        .expect("nobody else is active");

    // An ordinary card of an account that never reset stays refused.
    let mut world = World::on(world.store.clone(), 1).await;
    let (card, unrecorded) = new_card(&world.store, world.account).await;
    let link = super::add_device(
        &world.devices[0].keys,
        &world.links,
        &card.card(world.account),
    );
    world
        .store
        .append_account_links(vec![link.clone()])
        .await
        .expect("the link");
    world.links.push(link);
    assert_eq!(
        refusal(
            world
                .store
                .close_workspace(unrecorded, world.workspace)
                .await
        )
        .code,
        "forbidden",
        "a card the workspace doesn't record, on a chain that never reset"
    );
}

/// Once an admin accepted the owner's reset, the new card is an ordinary
/// device of the workspace and closes it as any owner device does; a later
/// card the workspace doesn't record gets nothing (§14).
pub async fn an_accepted_reset_leaves_the_ordinary_closing_rule<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let admin = super::invitations::member(&mut world, Role::Admin).await;
    let owner = world.account;
    sign_up(&world.store, owner).await;
    let (keys, session, genesis) = reset_account(&world.store, owner).await;
    let new_head = head_of(std::slice::from_ref(&genesis));
    let request = reset_request(&keys, owner, &new_head, world.workspace);
    world
        .store
        .post_approval_request(session, request.to_bytes())
        .await
        .expect("the request");

    world.devices.push(super::Device {
        keys: admin.keys.clone(),
        session: admin.session,
        token: random_token(),
    });
    let mut heads = vec![admin.head(), new_head.clone()];
    heads.sort_by_key(|head| head.account);
    let change = world.change(1, heads, PolicyKind::AcceptReset { account: owner });
    let envelope = world.envelope(1, &keys.card(owner), 1, &world.epoch_key);
    world
        .push_records(
            1,
            vec![RecordRef::Policy(change), RecordRef::Envelope(envelope)],
        )
        .await
        .expect("accepted");

    let (_, waiting) = new_card(&world.store, owner).await;
    assert_eq!(
        refusal(world.store.close_workspace(waiting, world.workspace).await).code,
        "forbidden",
        "a card that isn't on the chain"
    );
    assert_eq!(
        refusal(
            world
                .store
                .close_workspace(admin.session, world.workspace)
                .await
        )
        .code,
        "forbidden"
    );
    world
        .store
        .close_workspace(session, world.workspace)
        .await
        .expect("an owner device closes it");
}

/// The `accept_reset` of `sync-v2-policy.json`, which any client verifies,
/// gets `forbidden` from the server without the account's approval request
/// for this workspace (§8, §10).
pub async fn an_accept_reset_without_a_request_is_forbidden<B: Backend>(store: Store<B>) {
    let (workspace, change, session) =
        super::invitations::forged_policy_case(&store, "accept_reset_by_the_owner").await;
    let body = sync_protocol::messages::PushRequest {
        records: sync_protocol::codec::List(vec![RecordRef::Policy(change)]),
    };
    let refused = store.push(session, workspace, body.to_bytes()).await;
    assert_eq!(refusal(refused).code, "forbidden");
}

/// A card of the account with a session that proved `kit`.
async fn proven_card<B: Backend>(
    store: &Store<B>,
    account: Id,
    kit: &DeviceKeys,
) -> (DeviceKeys, Session, [u8; 32]) {
    let keys = DeviceKeys::generate().expect("random");
    store
        .enroll_card(keys.card(account))
        .await
        .expect("enrolled");
    let token = random_token();
    let session = Session::new(account, keys.device());
    store.create_session(token, session).await.expect("session");
    let origin = "https://sync.example.com";
    let challenge = store
        .challenge(session)
        .await
        .expect("a challenge")
        .challenge;
    let proof = sync_key_envelope::v2::recovery_proof(kit, origin, &keys.card(account), challenge);
    store
        .prove_recovery(session, sync_protocol::crypto::hash(&token), origin, proof)
        .await
        .expect("proven");
    (
        keys,
        Session {
            recovery: Some(kit.device()),
            ..session
        },
        token,
    )
}

/// A workspace whose policy records a recovery card of its one account.
async fn world_with_kit<B: Backend>(store: Store<B>) -> (World<B>, DeviceKeys) {
    let mut world = World::new(store, 1).await;
    sign_up(&world.store, world.account).await;
    let kit = DeviceKeys::generate().expect("random");
    let head = world
        .append_link(
            0,
            LinkKind::AddRecovery {
                card: Nested(kit.card(world.account)),
            },
        )
        .await;
    let change = world.change(0, vec![head], PolicyKind::AccountHeads);
    let domain = world.domain;
    world.inject_policy(change, vec![domain], None).await;
    (world, kit)
}

/// A reset ends the session of every card on the chain (§7), and that
/// takes in a session on an unlisted card that proved a card of the chain:
/// its proof names a retired identity. The session that resets keeps going
/// without its proof.
pub async fn a_reset_ends_the_sessions_that_proved_its_recovery_card<B: Backend>(store: Store<B>) {
    let (world, kit) = world_with_kit(store).await;
    let (account, w) = (world.account, world.workspace);
    let store = world.store.clone();
    let (_, thief, thief_token) = proven_card(&store, account, &kit).await;
    assert!(
        store.policy(thief, w, 0).await.is_ok(),
        "the proof reads as the kit until the reset"
    );

    let (keys, resetting, resetting_token) = proven_card(&store, account, &kit).await;
    let (code, _) = store.issue_reset_code(resetting).await.expect("a code");
    store
        .reset_account(resetting, reset(&code, genesis_link(&keys, account)))
        .await
        .expect("the reset");

    assert_eq!(
        refusal(
            store
                .session(&sync_protocol::types::to_base64url(&thief_token))
                .await
        )
        .code,
        "device_removed"
    );
    let kept = store
        .session(&sync_protocol::types::to_base64url(&resetting_token))
        .await
        .expect("the reset's own session stays");
    assert_eq!(kept.recovery, None, "its proof named the retired chain");
}

/// A proof reads a workspace as the recovery card, and closes nothing:
/// only an active device of the owner account closes a workspace (§14).
pub async fn a_proven_session_can_not_close_a_workspace<B: Backend>(store: Store<B>) {
    let (world, kit) = world_with_kit(store).await;
    let (_, proven, _) = proven_card(&world.store, world.account, &kit).await;
    assert_eq!(
        refusal(world.store.close_workspace(proven, world.workspace).await).code,
        "forbidden"
    );
    world
        .store
        .close_workspace(world.devices[0].session, world.workspace)
        .await
        .expect("an active device closes it");
}

/// Closing a workspace ends its invitation links at once, and once its
/// last domain goes, maintenance deletes what names its members: the policy
/// chain, the profile, the recorded accounts and the approval requests.
pub async fn a_closed_workspace_leaves_no_invitation_or_policy_rows<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let w = world.workspace;
    let owner = world.devices[0].session;
    let account = world.account;
    let invite = super::invitations::Invite::new(Role::Editor);
    world
        .store
        .create_invitation(owner, w, invite.creation(w))
        .await
        .expect("an invitation");
    assert!(world.store.invitation_context(invite.token).await.is_ok());
    world.store.close_workspace(owner, w).await.expect("closed");
    assert_eq!(
        refusal(world.store.invitation_context(invite.token).await).code,
        "invitation_invalid",
        "the link ends with the workspace"
    );

    world.store.maintain().await.expect("a pass");
    world.store.maintain().await.expect("another pass");
    let left = world
        .store
        .backend()
        .read(Box::new(move |txn: &mut B::Read| {
            Box::pin(async move {
                Ok((
                    txn.policy_changes(w, 0).await?.len(),
                    txn.workspaces_recording(account).await?.len(),
                    txn.workspace_approval_requests(w).await?.len(),
                    txn.drained_closed_workspaces(16).await?,
                ))
            })
        }))
        .await
        .expect("read");
    assert_eq!(left, (0, 0, 0, Vec::new()));
}

/// Retrying the creation of a closed workspace answers `id_conflict`, as
/// any other creation under an ID in use does (§26).
pub async fn creating_a_closed_workspace_again_is_an_id_conflict<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let (account, session) = (world.account, world.devices[0].session);
    let cards = [world.devices[0].keys.card(account)];
    let (body, genesis, _) = super::creation(&world.devices[0].keys, &world.links, &cards);
    world
        .store
        .create_workspace(session, body.clone())
        .await
        .expect("created");
    world
        .store
        .close_workspace(session, genesis.body.workspace)
        .await
        .expect("closed");
    assert_eq!(
        refusal(world.store.create_workspace(session, body.clone()).await).code,
        "id_conflict"
    );
    world.store.maintain().await.expect("a pass");
    world.store.maintain().await.expect("another pass");
    assert_eq!(
        refusal(world.store.create_workspace(session, body).await).code,
        "id_conflict",
        "the ID stays used after the reclaim"
    );
}
