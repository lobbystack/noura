//! Reset devices and Set up sync again on the server (§7, §14): reset
//! codes, retiring an account's chain, what a push of `accept_reset` must
//! prove, and closing a workspace.

use std::collections::BTreeSet;

use sync_protocol::account::AccountChain;
use sync_protocol::codec::Encode;
use sync_protocol::messages::ResetRequest;
use sync_protocol::policy::{PolicyChain, Snapshot};
use sync_protocol::records::{AccountLink, LinkKind, PolicyChange, PolicyKind, Role, Signed};
use sync_protocol::types::{Digest, Id};

use crate::chains::SIGN_IN_TTL;
use crate::devices::{normalized_code, reset_code_hash, reset_code_new};
use crate::{
    Backend, Error, LinkRow, LockPlan, ReadTxn, RemovedBy, Result, Session, Store, WriteTxn,
    policy, refuse,
};

fn invalid_code() -> Error {
    refuse(
        "reset_code_invalid",
        "the reset code is wrong, used or expired",
    )
}

impl<B: Backend> Store<B> {
    /// `POST /v2/account/reset-code`: a new reset code for the session's
    /// account, usable once for 10 minutes, with the address to email it
    /// to. A new code replaces the last one.
    pub async fn issue_reset_code(&self, session: Session) -> Result<(String, String)> {
        let code = reset_code_new()?;
        let hash = reset_code_hash(&code);
        let expires_at = self.clock().now() + SIGN_IN_TTL;
        let email = self
            .backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    let email = txn
                        .account_email(session.account)
                        .await?
                        .ok_or_else(|| refuse("forbidden", "the account has no address"))?;
                    txn.put_reset_code(session.account, hash, expires_at)
                        .await?;
                    Ok(email)
                })
            }))
            .await?;
        Ok((code, email))
    }

    /// Takes back a code whose email failed to send.
    pub async fn revoke_reset_code(&self, session: Session) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.take_reset_code(session.account).await.map(|_| ()) })
            }))
            .await
    }

    /// `POST /v2/account/reset`: in one transaction, uses the code, retires
    /// the account's chain, ends the sessions of every card on it and
    /// starts the new chain from the session's enrolled card.
    pub async fn reset_account(&self, session: Session, request: ResetRequest) -> Result<()> {
        let now = self.clock().now();
        let code = normalized_code(request.code.as_str()).ok_or_else(invalid_code)?;
        let hash = reset_code_hash(&code);
        let genesis = request.genesis.0;
        let account = session.account;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(account).await?;
                    retire_chain(txn, session, hash, now, &genesis).await
                })
            }))
            .await?;
        self.wake_recording(account).await
    }

    /// `DELETE /v2/workspaces/{w}`: closes a workspace, for an active
    /// device of its owner account, or for an active card of that account's
    /// new chain when it reset past the head the workspace records and
    /// nobody else can accept the reset (§14). Every route answers
    /// `not_found` from then on, and maintenance reclaims its domains in
    /// batches.
    pub async fn close_workspace(&self, session: Session, workspace: Id) -> Result<()> {
        let cache = self.policies.clone();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&LockPlan {
                        workspace: Some(workspace),
                        policy_exclusive: true,
                        key_sequence: false,
                        domains: Vec::new(),
                        blobs: Vec::new(),
                    })
                    .await?;
                    let (_, policy) = policy::load(txn, &cache, workspace)
                        .await?
                        .ok_or_else(|| refuse("not_found", "no such workspace"))?;
                    let snapshot = policy.head_snapshot();
                    if !may_close(txn, snapshot, session).await? {
                        return Err(refuse(
                            "forbidden",
                            "only an active device of the owner account closes a workspace",
                        ));
                    }
                    let domains: BTreeSet<Id> = snapshot.domains.keys().copied().collect();
                    for domain in domains {
                        txn.mark_domain_deleted(workspace, domain).await?;
                    }
                    txn.close_workspace(workspace).await
                })
            }))
            .await
    }
}

/// Whether `session` may close the workspace at `snapshot` (§14). An owner
/// account on the chain the workspace records closes it from a card the
/// workspace counts. One whose chain a reset moved past that head closes
/// it from an active card of the new chain, while no other owner or admin
/// is active there, because nobody could accept its reset. Whether the
/// reset is the owner's own rests on the email check of §7.
async fn may_close(txn: &mut impl ReadTxn, snapshot: &Snapshot, session: Session) -> Result<bool> {
    let Some(member) = snapshot.members.get(&session.account) else {
        return Ok(false);
    };
    if member.role != Role::Owner {
        return Ok(false);
    }
    let Some(current) = txn.current_genesis(session.account).await? else {
        return Ok(false);
    };
    if current == member.head.genesis {
        return Ok(snapshot
            .devices
            .get(&session.device)
            .is_some_and(|device| !device.recovery));
    }
    let chains = policy::chains_of(txn, session.account).await?;
    let on_new_chain = chains
        .get(&current)
        .is_some_and(|chain| chain.is_active_at(&session.device, chain.head_seqno()));
    Ok(on_new_chain && !another_manager_is_active(txn, snapshot, session.account).await?)
}

/// Whether an owner or admin account other than `account` can still act in
/// the workspace: it stands on the chain the workspace records, with a card
/// of that head still active. A manager that reset too can't.
async fn another_manager_is_active(
    txn: &mut impl ReadTxn,
    snapshot: &Snapshot,
    account: Id,
) -> Result<bool> {
    for (other, member) in &snapshot.members {
        if *other == account || !matches!(member.role, Role::Owner | Role::Admin) {
            continue;
        }
        if txn.current_genesis(*other).await? != Some(member.head.genesis) {
            continue;
        }
        let chains = policy::chains_of(txn, *other).await?;
        let Some(chain) = chains.get(&member.head.genesis) else {
            continue;
        };
        let active = snapshot.devices.iter().any(|(device, card)| {
            card.account == *other
                && !card.recovery
                && chain.is_active_at(device, chain.head_seqno())
        });
        if active {
            return Ok(true);
        }
    }
    Ok(false)
}

async fn retire_chain(
    txn: &mut impl WriteTxn,
    session: Session,
    hash: Digest,
    now: u64,
    genesis: &Signed<AccountLink>,
) -> Result<()> {
    let account = session.account;
    match txn.reset_code(account).await? {
        Some((stored, expires_at)) if stored == hash && expires_at > now => {}
        _ => return Err(invalid_code()),
    }
    txn.take_reset_code(account).await?;
    let body = &genesis.body;
    let LinkKind::Genesis { card } = &body.kind else {
        return Err(refuse("bad_request", "a reset starts with a genesis link"));
    };
    if body.account != account || body.signer != session.device {
        return Err(refuse(
            "forbidden",
            "a reset's genesis comes from the session's card",
        ));
    }
    if txn.enrolled_card(card.device).await? != Some((account, card.0.to_bytes())) {
        return Err(refuse(
            "forbidden",
            "the card isn't one a session of the account enrolled",
        ));
    }
    let chains = policy::chains_of(txn, account).await?;
    let listed: BTreeSet<Id> = chains
        .values()
        .flat_map(|chain| chain.listed_devices())
        .collect();
    let chain = AccountChain::from_genesis(genesis, &listed)
        .map_err(|e| refuse(e.server_code(), e.to_string()))?;
    if let Some(current) = txn.current_genesis(account).await?
        && let Some(old) = chains.get(&current)
    {
        let cards: Vec<Id> = old.listed_devices().into_iter().collect();
        txn.remove_sessions(
            account,
            &cards,
            RemovedBy {
                genesis: current,
                seqno: old.head_seqno(),
            },
        )
        .await?;
    }
    txn.insert_account_link(
        account,
        &LinkRow {
            genesis: chain.genesis(),
            seqno: 1,
            link: genesis.to_bytes(),
        },
        genesis.digest(),
    )
    .await?;
    txn.set_current_genesis(account, chain.genesis()).await
}

/// The checks only the server can make on an `accept_reset` (§8, §10): it
/// names another account than the signer's, records that account's current
/// chain, and answers the account's unexpired approval request for this
/// workspace.
pub(crate) async fn check_accept_reset(
    txn: &mut impl ReadTxn,
    policy: &PolicyChain,
    workspace: Id,
    change: &Signed<PolicyChange>,
    now: u64,
) -> Result<()> {
    let PolicyKind::AcceptReset { account } = change.body.kind else {
        return Ok(());
    };
    let prev = policy.head_snapshot();
    if prev.account_of(&change.body.signer) == Some(account) {
        return Err(refuse(
            "forbidden",
            "the reset account can't accept its own reset",
        ));
    }
    let head = change
        .body
        .heads
        .iter()
        .find(|head| head.account == account)
        .ok_or_else(|| refuse("forbidden", "the change records no head of the account"))?;
    if txn.current_genesis(account).await? != Some(head.genesis) {
        return Err(refuse(
            "account_conflict",
            "the head isn't on the account's current chain",
        ));
    }
    let requested = txn
        .workspace_approval_requests(workspace)
        .await?
        .into_iter()
        .any(|row| {
            row.expires_at > now
                && row.account == account
                && row.invitation.is_none()
                && row.candidate_head.as_ref() == Some(head)
        });
    if !requested {
        return Err(refuse(
            "forbidden",
            "the account posted no approval request for this workspace",
        ));
    }
    Ok(())
}
