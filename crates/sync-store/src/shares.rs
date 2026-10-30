//! Share links (§24): the rows behind `POST`, `GET`, `PUT` and `DELETE
//! .../shares` and the viewer's `GET /v2/shares/{token}`. The server stores
//! each link's token hash, workspace, domain, creating device, expiry and
//! signed padded snapshot, and never the link key or the verify key.

use std::sync::Arc;

use sync_protocol::codec::{Decode, Encode, List};
use sync_protocol::messages::{ShareInfo, SharesResponse};
use sync_protocol::padding::is_padded_ciphertext_length;
use sync_protocol::policy::{PolicyChain, Snapshot};
use sync_protocol::records::{Role, Share, Signed, share_token_hash};
use sync_protocol::types::{Digest, Id};

use crate::{
    Backend, LockPlan, ReadTxn, Result, Session, ShareRow, Store, WriteTxn, policy, refuse,
};

/// The most active share links a workspace holds.
pub const MAX_SHARES: usize = 1_000;

/// The longest a share link lives, in days; also the default.
pub const MAX_SHARE_DAYS: u8 = 30;

const DAY: u64 = 86_400;

/// What `POST .../shares` names.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareCreation {
    /// The token's hash.
    pub token_hash: Digest,
    /// The domain of the shared note.
    pub domain: Id,
    /// 1 to 30.
    pub expires_in_days: u8,
    /// The signed snapshot.
    pub share: Signed<Share>,
}

/// The policy, after checking that the session's device is active and its
/// account an owner or admin of the workspace.
async fn manager(
    txn: &mut impl ReadTxn,
    cache: &policy::PolicyCache,
    session: Session,
    workspace: Id,
) -> Result<Arc<PolicyChain>> {
    let (_, policy) = policy::load(txn, cache, workspace)
        .await?
        .ok_or_else(|| refuse("not_found", "no such workspace"))?;
    let snapshot = policy.head_snapshot();
    if !snapshot.is_device(&session.device) {
        return Err(refuse(
            "forbidden",
            "the device isn't active in this workspace",
        ));
    }
    if !manages_links(snapshot, &session.device) {
        return Err(refuse(
            "forbidden",
            "only owners and admins create and revoke share links",
        ));
    }
    Ok(policy)
}

fn manages_links(snapshot: &Snapshot, device: &Id) -> bool {
    snapshot
        .account_of(device)
        .and_then(|account| snapshot.role(&account))
        .is_some_and(|role| matches!(role, Role::Owner | Role::Admin))
}

/// The snapshot's own checks: it names the link's token hash and its
/// ciphertext is padded (§6). The server can't check its signature, since
/// only the link holds the verify key.
fn check_snapshot(share: &Signed<Share>, token_hash: &Digest) -> Result<()> {
    if share.body.token_hash != *token_hash {
        return Err(refuse(
            "bad_request",
            "the snapshot names another link's token hash",
        ));
    }
    if !is_padded_ciphertext_length(share.body.ciphertext.0.len() as u64) {
        return Err(refuse(
            "padding_invalid",
            "the snapshot's ciphertext isn't a padded length",
        ));
    }
    Ok(())
}

/// The lock plan of a share write: the shared policy lock, then the head
/// rows of `domains` in domain-ID order (§15). A share link holds no blob
/// row, so it takes nothing after them.
fn plan(workspace: Id, mut domains: Vec<Id>) -> LockPlan {
    domains.sort();
    domains.dedup();
    LockPlan {
        workspace: Some(workspace),
        policy_exclusive: false,
        key_sequence: false,
        domains,
        blobs: Vec::new(),
    }
}

/// A live link of `workspace` under `token_hash`, or `not_found`.
async fn live_row(
    txn: &mut impl ReadTxn,
    workspace: Id,
    token_hash: Digest,
    now: u64,
) -> Result<ShareRow> {
    txn.share(token_hash)
        .await?
        .filter(|row| row.workspace == workspace && row.expires_at > now)
        .ok_or_else(|| refuse("not_found", "no such share link"))
}

impl<B: Backend> Store<B> {
    /// `POST .../shares`: a new link from an owner or admin device active
    /// in the workspace, which expires `expires_in_days` days from now. A
    /// retry with identical bytes returns the original expiry; other bytes
    /// under the same token hash get `id_conflict`.
    pub async fn create_share(
        &self,
        session: Session,
        workspace: Id,
        creation: ShareCreation,
    ) -> Result<u64> {
        if !(1..=MAX_SHARE_DAYS).contains(&creation.expires_in_days) {
            return Err(refuse(
                "bad_request",
                "a share link expires in 1 to 30 days",
            ));
        }
        check_snapshot(&creation.share, &creation.token_hash)?;
        if creation.share.body.snapshot_revision != 1 {
            return Err(refuse("bad_request", "a new link's snapshot_revision is 1"));
        }
        let now = self.clock().now();
        let cache = self.policies.clone();
        // The workspace domain never changes, so it may come from the
        // policy before the locks.
        let workspace_domain = {
            let cache = cache.clone();
            self.backend
                .read(Box::new(move |txn: &mut B::Read| {
                    Box::pin(async move {
                        let (_, policy) = policy::load(txn, &cache, workspace)
                            .await?
                            .ok_or_else(|| refuse("not_found", "no such workspace"))?;
                        Ok(policy.head_snapshot().workspace_domain)
                    })
                }))
                .await?
        };
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    // The link's domain head row, and the workspace domain's
                    // head row, which every creation in the workspace takes,
                    // so the count of active links stays exact.
                    txn.lock(&plan(workspace, vec![creation.domain, workspace_domain]))
                        .await?;
                    let policy = manager(txn, &cache, session, workspace).await?;
                    if !policy
                        .head_snapshot()
                        .domains
                        .contains_key(&creation.domain)
                    {
                        return Err(refuse("not_found", "no such domain"));
                    }
                    let bytes = creation.share.to_bytes();
                    let expires_at = now + u64::from(creation.expires_in_days) * DAY;
                    if let Some(row) = txn.share(creation.token_hash).await? {
                        // A retry within a day of the creation names the same
                        // expiry; the store keeps no creation time to compare.
                        let same = row.workspace == workspace
                            && row.domain == creation.domain
                            && row.device == session.device
                            && row.share == bytes
                            && expires_at >= row.expires_at
                            && expires_at - row.expires_at < DAY;
                        if same {
                            return Ok(row.expires_at);
                        }
                        return Err(refuse(
                            "id_conflict",
                            "another share link has this token hash",
                        ));
                    }
                    if txn.shares(workspace, now).await?.len() >= MAX_SHARES {
                        return Err(refuse(
                            "share_limit",
                            "the workspace already has 1,000 active share links",
                        ));
                    }
                    txn.insert_share(&ShareRow {
                        token_hash: creation.token_hash,
                        workspace,
                        domain: creation.domain,
                        device: session.device,
                        expires_at,
                        share: bytes,
                    })
                    .await?;
                    Ok(expires_at)
                })
            }))
            .await
    }

    /// `GET .../shares`: the workspace's active links, by token hash, for
    /// an owner or admin device active in it.
    pub async fn shares(&self, session: Session, workspace: Id) -> Result<SharesResponse> {
        let now = self.clock().now();
        let cache = self.policies.clone();
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    manager(txn, &cache, session, workspace).await?;
                    let rows = txn.shares(workspace, now).await?;
                    Ok(SharesResponse {
                        shares: List(
                            rows.into_iter()
                                .map(|row| ShareInfo {
                                    token_hash: row.token_hash,
                                    domain: row.domain,
                                    device: row.device,
                                    expires_at: row.expires_at,
                                })
                                .collect(),
                        ),
                    })
                })
            }))
            .await
    }

    /// `PUT .../shares/{token_hash}`: Update link replaces the snapshot,
    /// only from the creating device while that device stays active and its
    /// account an owner or admin, with a higher `snapshot_revision`.
    pub async fn update_share(
        &self,
        session: Session,
        workspace: Id,
        token_hash: Digest,
        share: Signed<Share>,
    ) -> Result<()> {
        check_snapshot(&share, &token_hash)?;
        let now = self.clock().now();
        let domain = self.share_domain(workspace, token_hash, now).await?;
        let cache = self.policies.clone();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&plan(workspace, vec![domain])).await?;
                    manager(txn, &cache, session, workspace).await?;
                    let row = live_row(txn, workspace, token_hash, now).await?;
                    if row.device != session.device {
                        return Err(refuse(
                            "forbidden",
                            "only the device that created a link updates it",
                        ));
                    }
                    let stored = Signed::<Share>::from_bytes(&row.share)
                        .map_err(|_| crate::Error::internal("a stored share doesn't decode"))?;
                    if share.body.snapshot_revision <= stored.body.snapshot_revision {
                        if share.to_bytes() == row.share {
                            return Ok(());
                        }
                        return Err(refuse("bad_request", "an update raises snapshot_revision"));
                    }
                    txn.set_share(token_hash, &share.to_bytes()).await
                })
            }))
            .await
    }

    /// `DELETE .../shares/{token_hash}`: revokes a link and deletes its
    /// snapshot, for an owner or admin device active in the workspace.
    /// Revoking a link that's gone already succeeds, so a device can send
    /// its revocations again after a restore (§14).
    pub async fn revoke_share(
        &self,
        session: Session,
        workspace: Id,
        token_hash: Digest,
    ) -> Result<()> {
        let now = self.clock().now();
        let domain = match self.share_domain(workspace, token_hash, now).await {
            Ok(domain) => Some(domain),
            Err(crate::Error::Refused(refusal)) if refusal.code == "not_found" => None,
            Err(error) => return Err(error),
        };
        let cache = self.policies.clone();
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock(&plan(workspace, domain.into_iter().collect()))
                        .await?;
                    manager(txn, &cache, session, workspace).await?;
                    match txn.share(token_hash).await? {
                        Some(row) if row.workspace == workspace => {
                            txn.delete_share(token_hash).await
                        }
                        _ => Ok(()),
                    }
                })
            }))
            .await
    }

    /// `GET /v2/shares/{token}`: the snapshot of a live link, for anyone
    /// who holds the token.
    pub async fn read_share(&self, token: [u8; 16]) -> Result<Signed<Share>> {
        let now = self.clock().now();
        let token_hash = share_token_hash(&token);
        let row = self
            .backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.share(token_hash).await })
            }))
            .await?
            .filter(|row| row.expires_at > now)
            .ok_or_else(|| refuse("not_found", "no such share link"))?;
        Signed::<Share>::from_bytes(&row.share)
            .map_err(|_| crate::Error::internal("a stored share doesn't decode"))
    }

    /// The domain of a live link of `workspace`, read before the locks:
    /// a link's domain never changes.
    async fn share_domain(&self, workspace: Id, token_hash: Digest, now: u64) -> Result<Id> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { Ok(live_row(txn, workspace, token_hash, now).await?.domain) })
            }))
            .await
    }
}
