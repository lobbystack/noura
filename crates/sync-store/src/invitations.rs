//! Invitations (§11): the rows behind `POST`, `GET` and `DELETE
//! .../invitations`, the invitee's `GET /v2/invitations/{token}` and `POST
//! .../accept`, and `GET .../pending-devices`. While an invitation is open
//! the server stores its token's hash, the workspace, the role, the inviting
//! account, the accepting account once known, the expiry and the padded
//! context, and never the token or the secret.

use std::sync::Arc;

use sync_protocol::codec::{Decode, Encode, List, Nested};
use sync_protocol::messages::{
    InvitationInfo, InvitationResponse, InvitationStatus, InvitationsResponse, Pending,
    PendingResponse,
};
use sync_protocol::padding::is_padded_ciphertext_length;
use sync_protocol::policy::{PolicyChain, Snapshot};
use sync_protocol::records::{Card, InviteContext, Role, invite_token_hash};
use sync_protocol::types::{Digest, Id};

use crate::{
    Backend, Error, InvitationRow, LockPlan, ReadTxn, Refusal, Result, Session, Store, WriteTxn,
    policy, refuse,
};

/// The most open invitations a workspace holds (§27).
pub const MAX_INVITATIONS: usize = 100;

/// How long an invitation stays open, in seconds: seven days (§11).
pub const INVITATION_TTL: u64 = 7 * 86_400;

/// The most pending approval requests that name one workspace, through
/// invitations and resets together (§27).
pub const MAX_WORKSPACE_REQUESTS: usize = 100;

/// What `POST .../invitations` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvitationCreation {
    /// The invitation ID the inviting device chose.
    pub invitation: Id,
    /// The token's hash.
    pub token_hash: Digest,
    /// The role it grants: admin, editor or viewer.
    pub role: Role,
    /// The encrypted context.
    pub invite_context: InviteContext,
}

fn invalid() -> Error {
    refuse(
        "invitation_invalid",
        "the invitation is revoked, expired, used or unknown",
    )
}

/// The session's role in the workspace, after checking that its device is
/// active there.
fn role_of(snapshot: &Snapshot, session: Session) -> Result<Role> {
    if !snapshot.is_device(&session.device) {
        return Err(refuse(
            "forbidden",
            "the device isn't active in this workspace",
        ));
    }
    snapshot
        .account_of(&session.device)
        .and_then(|account| snapshot.role(&account))
        .ok_or_else(|| refuse("forbidden", "the device's account isn't a member"))
}

/// The policy, after checking that the session's device is active and its
/// account an owner or admin of the workspace (§8).
pub(crate) async fn manager(
    txn: &mut impl ReadTxn,
    cache: &policy::PolicyCache,
    session: Session,
    workspace: Id,
) -> Result<(Arc<PolicyChain>, Role)> {
    let (_, policy) = policy::load(txn, cache, workspace)
        .await?
        .ok_or_else(|| refuse("not_found", "no such workspace"))?;
    let role = role_of(policy.head_snapshot(), session)?;
    if !matches!(role, Role::Owner | Role::Admin) {
        return Err(refuse(
            "forbidden",
            "only owners and admins manage invitations",
        ));
    }
    Ok((policy, role))
}

/// The lock plan of an invitation write: the shared policy lock, then the
/// workspace domain's head row, which every invitation write in the
/// workspace takes, so the count of open invitations stays exact.
fn plan(workspace: Id, workspace_domain: Id) -> LockPlan {
    LockPlan {
        workspace: Some(workspace),
        policy_exclusive: false,
        key_sequence: false,
        domains: vec![workspace_domain],
        blobs: Vec::new(),
    }
}

/// An open invitation whose token hashes to `token_hash`, or
/// `invitation_invalid`.
async fn open_row(txn: &mut impl ReadTxn, token_hash: Digest, now: u64) -> Result<InvitationRow> {
    txn.invitation_by_token(token_hash)
        .await?
        .filter(|row| row.expires_at > now)
        .ok_or_else(invalid)
}

fn context(row: &InvitationRow) -> Result<InviteContext> {
    InviteContext::from_bytes(&row.invite_context)
        .map_err(|_| Error::internal("a stored invitation context doesn't decode"))
}

impl<B: Backend> Store<B> {
    /// The workspace domain, which never changes, so it may come from the
    /// policy before the locks.
    async fn workspace_domain(&self, workspace: Id) -> Result<Id> {
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let (_, policy) = policy::load(txn, &cache, workspace)
                        .await?
                        .ok_or_else(|| refuse("not_found", "no such workspace"))?;
                    Ok(policy.head_snapshot().workspace_domain)
                })
            }))
            .await
    }

    /// `POST .../invitations`: a new invitation from an owner or admin
    /// device active in the workspace, for an admin only from an owner,
    /// open for seven days. Its context must name the workspace, the
    /// invitation and the role, and hold a padded ciphertext. A retry with
    /// identical bytes returns the original expiry; other bytes under the
    /// same ID or token hash get `id_conflict`. A workspace holds at most
    /// 100 open invitations (`invitation_limit`).
    pub async fn create_invitation(
        &self,
        session: Session,
        workspace: Id,
        creation: InvitationCreation,
    ) -> Result<u64> {
        if !matches!(creation.role, Role::Admin | Role::Editor | Role::Viewer) {
            return Err(refuse(
                "bad_request",
                "an invitation grants the admin, editor or viewer role",
            ));
        }
        let context = &creation.invite_context;
        if context.workspace != workspace
            || context.invitation != creation.invitation
            || context.role != creation.role
        {
            return Err(refuse(
                "bad_request",
                "the context names another workspace, invitation or role",
            ));
        }
        if !is_padded_ciphertext_length(context.ciphertext.0.len() as u64) {
            return Err(refuse(
                "padding_invalid",
                "the context's ciphertext isn't a padded length",
            ));
        }
        let now = self.clock().now();
        let cache = self.policies.clone();
        let workspace_domain = self.workspace_domain(workspace).await?;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&plan(workspace, workspace_domain)).await?;
                    let (_, role) = manager(txn, &cache, session, workspace).await?;
                    if creation.role == Role::Admin && role != Role::Owner {
                        return Err(refuse("forbidden", "only the owner invites admins"));
                    }
                    let bytes = creation.invite_context.to_bytes();
                    let expires_at = now + INVITATION_TTL;
                    let held = match txn.invitation(creation.invitation).await? {
                        Some(row) => Some(row),
                        None => txn.invitation_by_token(creation.token_hash).await?,
                    };
                    if let Some(row) = held {
                        // A retry within a day of the creation names the same
                        // expiry; the store keeps no creation time to compare.
                        let same = row.invitation == creation.invitation
                            && row.token_hash == creation.token_hash
                            && row.workspace == workspace
                            && row.role == creation.role
                            && row.inviter == session.account
                            && row.invite_context == bytes
                            && expires_at >= row.expires_at
                            && expires_at - row.expires_at < 86_400;
                        if same {
                            return Ok(row.expires_at);
                        }
                        return Err(refuse(
                            "id_conflict",
                            "another invitation has this ID or token hash",
                        ));
                    }
                    if txn.invitations(workspace, now).await?.len() >= MAX_INVITATIONS {
                        return Err(refuse(
                            "invitation_limit",
                            "the workspace already has 100 open invitations",
                        ));
                    }
                    txn.insert_invitation(&InvitationRow {
                        invitation: creation.invitation,
                        token_hash: creation.token_hash,
                        workspace,
                        role: creation.role,
                        inviter: session.account,
                        accepted_by: None,
                        expires_at,
                        invite_context: bytes,
                    })
                    .await?;
                    Ok(expires_at)
                })
            }))
            .await
    }

    /// `GET .../invitations`: the workspace's open invitations, by ID, each
    /// pending or accepted, for an owner or admin device active in it.
    pub async fn invitations(
        &self,
        session: Session,
        workspace: Id,
    ) -> Result<InvitationsResponse> {
        let now = self.clock().now();
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    manager(txn, &cache, session, workspace).await?;
                    let mut invitations = Vec::new();
                    for row in txn.invitations(workspace, now).await? {
                        invitations.push(InvitationInfo {
                            invitation: row.invitation,
                            role: row.role,
                            status: if row.accepted_by.is_some() {
                                InvitationStatus::Accepted
                            } else {
                                InvitationStatus::Pending
                            },
                            accepted_by: row.accepted_by,
                            expires_at: row.expires_at,
                            invite_context: Nested(context(&row)?),
                        });
                    }
                    Ok(InvitationsResponse {
                        invitations: List(invitations),
                    })
                })
            }))
            .await
    }

    /// `DELETE .../invitations/{id}`: revokes an invitation and deletes its
    /// row, for an owner or admin device active in the workspace, so its
    /// link fails from then on. Revoking one that's gone already succeeds.
    pub async fn revoke_invitation(
        &self,
        session: Session,
        workspace: Id,
        invitation: Id,
    ) -> Result<()> {
        let cache = self.policies.clone();
        let workspace_domain = self.workspace_domain(workspace).await?;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&plan(workspace, workspace_domain)).await?;
                    manager(txn, &cache, session, workspace).await?;
                    match txn.invitation(invitation).await? {
                        Some(row) if row.workspace == workspace => {
                            txn.delete_invitation(invitation).await
                        }
                        _ => Ok(()),
                    }
                })
            }))
            .await
    }

    /// `GET /v2/invitations/{token}`: an open invitation's context, for
    /// anyone who holds the token; `invitation_invalid` once it's revoked,
    /// expired or approved, or for an unknown token.
    pub async fn invitation_context(&self, token: [u8; 16]) -> Result<InvitationResponse> {
        let now = self.clock().now();
        let token_hash = invite_token_hash(&token);
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let row = open_row(txn, token_hash, now).await?;
                    Ok(InvitationResponse {
                        workspace: row.workspace,
                        invitation: row.invitation,
                        role: row.role,
                        status: if row.accepted_by.is_some() {
                            InvitationStatus::Accepted
                        } else {
                            InvitationStatus::Pending
                        },
                        invite_context: Nested(context(&row)?),
                    })
                })
            }))
            .await
    }

    /// `POST /v2/invitations/{token}/accept`: binds an open invitation to
    /// the session's account, which grants nothing. Accepting again from
    /// the same account changes nothing; an invitation another account
    /// accepted is `invitation_invalid`.
    pub async fn accept_invitation(&self, session: Session, token: [u8; 16]) -> Result<()> {
        let now = self.clock().now();
        let token_hash = invite_token_hash(&token);
        // The workspace never changes, so it may come before the locks.
        let workspace = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { Ok(open_row(txn, token_hash, now).await?.workspace) })
            }))
            .await?;
        let workspace_domain = self.workspace_domain(workspace).await?;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&plan(workspace, workspace_domain)).await?;
                    let row = open_row(txn, token_hash, now).await?;
                    match row.accepted_by {
                        Some(account) if account == session.account => Ok(()),
                        Some(_) => Err(invalid()),
                        None => txn.accept_invitation(row.invitation, session.account).await,
                    }
                })
            }))
            .await
    }

    /// `GET .../pending-devices`: the unexpired requests of invited accounts
    /// and of reset member accounts that name this workspace, by commitment,
    /// for an owner or admin device active in it, which approves them.
    pub async fn pending_devices(
        &self,
        session: Session,
        workspace: Id,
    ) -> Result<PendingResponse> {
        let now = self.clock().now();
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    manager(txn, &cache, session, workspace).await?;
                    let mut pending = Vec::new();
                    for row in txn.workspace_approval_requests(workspace).await? {
                        if row.expires_at <= now || row.candidate_head.is_none() {
                            continue;
                        }
                        pending.push(Pending {
                            card: Nested(Card::from_bytes(&row.card).map_err(Error::internal)?),
                            commit: row.commit,
                            account: row.candidate_head,
                            invitation: row.invitation,
                            invite_proof: row.invite_proof,
                            workspace: row.workspace,
                        });
                    }
                    Ok(PendingResponse {
                        pending: List(pending),
                    })
                })
            }))
            .await
    }
}

/// The checks on an approval request that names an invitation (§10, §11):
/// the invitation is open and accepted by the session's account, which
/// isn't a member of its workspace, and the request's account head names
/// that account. Returns the invitation's workspace.
pub(crate) async fn invited_request(
    txn: &mut impl ReadTxn,
    cache: &policy::PolicyCache,
    session: Session,
    invitation: Id,
    candidate: Option<&sync_protocol::records::AccountHead>,
    now: u64,
) -> Result<Id> {
    let row = txn
        .invitation(invitation)
        .await?
        .filter(|row| row.expires_at > now && row.accepted_by == Some(session.account))
        .ok_or_else(invalid)?;
    if candidate.is_none_or(|head| head.account != session.account) {
        return Err(refuse(
            "forbidden",
            "the request's account head names another account",
        ));
    }
    if let Some((_, policy)) = policy::load(txn, cache, row.workspace).await?
        && policy
            .head_snapshot()
            .members
            .contains_key(&session.account)
    {
        return Err(refuse(
            "forbidden",
            "the account is already a member of the workspace",
        ));
    }
    workspace_request_room(txn, row.workspace, now).await?;
    Ok(row.workspace)
}

/// The server's checks on an `add_member` beyond the policy's own (§8,
/// §11): the invitation is open in this workspace, the signer's account
/// sent it, the new member's account accepted it and posted an unexpired
/// approval request that names it, and the change grants the invitation's
/// role. Anything else is `forbidden`.
pub(crate) async fn check_add_member(
    txn: &mut impl ReadTxn,
    workspace: Id,
    signer: Id,
    account: Id,
    role: Role,
    invitation: Id,
    now: u64,
) -> Result<()> {
    let row = txn
        .invitation(invitation)
        .await?
        .filter(|row| row.expires_at > now && row.workspace == workspace)
        .ok_or_else(|| {
            refuse(
                "forbidden",
                "add_member names no open invitation of this workspace",
            )
        })?;
    if row.inviter != signer || row.accepted_by != Some(account) || row.role != role {
        return Err(refuse(
            "forbidden",
            "add_member doesn't match the invitation's inviter, accepting account or role",
        ));
    }
    let requested = txn
        .approval_requests(account)
        .await?
        .iter()
        .any(|r| r.expires_at > now && r.invitation == Some(invitation));
    if !requested {
        return Err(refuse(
            "forbidden",
            "the new member's account posted no approval request that names the invitation",
        ));
    }
    Ok(())
}

/// Room for one more approval request that names `workspace`: at most 100
/// are pending there, and another gets `rate_limited` until the oldest one
/// expires (§27).
pub(crate) async fn workspace_request_room(
    txn: &mut impl ReadTxn,
    workspace: Id,
    now: u64,
) -> Result<()> {
    let pending: Vec<u64> = txn
        .workspace_approval_requests(workspace)
        .await?
        .into_iter()
        .map(|r| r.expires_at)
        .filter(|expires_at| *expires_at > now)
        .collect();
    if pending.len() >= MAX_WORKSPACE_REQUESTS {
        let first = pending.iter().copied().min().unwrap_or(now);
        return Err(Error::Refused(
            Refusal::new(
                "rate_limited",
                "the workspace already holds 100 pending approval requests",
            )
            .retry_after(first.saturating_sub(now).max(1)),
        ));
    }
    Ok(())
}

/// The checks on an offer that answers an invited account's request: the
/// invitation is still open, and the offering device belongs to the
/// inviting account, active in the workspace as an owner or admin (§8,
/// §11). The candidate checks that the card is the inviting device's.
pub(crate) async fn invited_offer(
    txn: &mut impl ReadTxn,
    cache: &policy::PolicyCache,
    session: Session,
    invitation: Id,
    now: u64,
) -> Result<()> {
    let row = txn
        .invitation(invitation)
        .await?
        .filter(|row| row.expires_at > now)
        .ok_or_else(invalid)?;
    manager(txn, cache, session, row.workspace).await?;
    if row.inviter != session.account {
        return Err(refuse(
            "forbidden",
            "only the inviting device approves an invited account",
        ));
    }
    Ok(())
}
