//! Reset devices, `accept_reset` and closing a workspace (§7, §8, §10,
//! §14), on both stores.

use std::time::Duration;

use sync_key_envelope::v2::DeviceKeys;
use sync_protocol::codec::{Encode, Nested, Text};
use sync_protocol::messages::{ApprovalRequest, RecordRef, ResetRequest};
use sync_protocol::records::{AccountHead, AccountLink, PolicyKind, Role, Signed, approval_commit};
use sync_protocol::types::{Id, Key};

use super::invitations::Invitee;
use super::{World, genesis_link, head_of, push_failure, random_token, refusal};
use crate::{Backend, Session, Store, WriteTxn};

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
