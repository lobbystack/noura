//! The commit of an invited member's approval (sync-v2.md §10 step 5): one
//! `add_member` recording the head the candidate's request committed to,
//! a current-epoch envelope of the workspace domain for each card active
//! at that head, and a fresh checkpoint of the workspace domain, in one
//! push.

use std::collections::BTreeMap;

use sync_protocol::codec::Sorted;
use sync_protocol::messages::RecordRef;
use sync_protocol::records::{PolicyChange, PolicyKind, Signed};

use super::super::approval::OutstandingOffer;
use super::super::{SyncError, SyncService, verify_served};
use super::{WorkspaceSync, policy_changes};

/// How many rounds the approving device takes at most to catch up before
/// it signs the fresh checkpoint.
const CATCH_UP_ROUNDS: usize = 64;

impl WorkspaceSync {
    /// Commits the invited member's approval you confirmed (§10 step 5).
    /// The device catches up on the workspace domain, then pushes one
    /// `add_member` that records its own account at the head the workspace
    /// records (§8) and the candidate's account at the head its request
    /// committed to, with the invitation's role, a current-epoch envelope
    /// for every card active at that head, and a checkpoint over every
    /// record it applied, so the member's history floor has a checkpoint to
    /// bootstrap from. Returns the new policy revision.
    pub async fn approve_member(
        &mut self,
        service: &mut SyncService,
        mut offer: OutstandingOffer,
    ) -> Result<u64, SyncError> {
        let Some((invitation, role)) = offer.invited else {
            return Err(SyncError::Invalid(
                "a device of your own account joins through your account chain".into(),
            ));
        };
        let candidate = offer
            .offer
            .body
            .candidate_head
            .clone()
            .ok_or(SyncError::Check("sync_approval_mismatch"))?;
        // Confirming spends the offer whatever happens next.
        let code = service.approval_code(&mut offer).await;
        service.outstanding = None;
        if code?.is_none() {
            return Err(SyncError::Invalid(
                "the new member's device hasn't shown its code yet".into(),
            ));
        }
        for _ in 0..CATCH_UP_ROUNDS {
            self.round(service, 0).await?;
            if self.state.log.cursor >= self.state.log.head {
                break;
            }
        }
        if self.state.log.cursor < self.state.log.head {
            return Err(SyncError::Network(
                "the workspace kept changing while the approval caught up; try again".into(),
            ));
        }
        let mut remote = self.remote.take().ok_or(SyncError::KeyMissing)?;
        let pushed = self
            .push_add_member(service, &mut remote, candidate, Some((invitation, role)))
            .await;
        self.state.policy = policy_changes(&remote.policy);
        self.remote = Some(remote);
        let revision = pushed?;
        self.save()?;
        Ok(revision)
    }

    /// Commits a reset you confirmed after comparing the approval code
    /// (§7, §8): one `accept_reset` recording the account's new head, with
    /// the new cards' workspace-domain envelopes. An owner or admin device
    /// may sign it while the workspace is stopped for that account's chain.
    /// Returns the new policy revision.
    pub async fn accept_reset(
        &mut self,
        service: &mut SyncService,
        mut offer: OutstandingOffer,
    ) -> Result<u64, SyncError> {
        let candidate = offer
            .offer
            .body
            .candidate_head
            .clone()
            .ok_or(SyncError::Check("sync_approval_mismatch"))?;
        let code = service.approval_code(&mut offer).await;
        service.outstanding = None;
        if code?.is_none() {
            return Err(SyncError::Invalid(
                "the other device hasn't shown its code yet".into(),
            ));
        }
        if let Some(super::restore::Lost::MemberAccount(account)) = self.state.restore.lost {
            service.kept_heads_mut().excuse_short(account);
        }
        let mut remote = match self.remote.take() {
            Some(remote) => remote,
            None => self.open_for_reset(service).await?,
        };
        let pushed = self
            .push_add_member(service, &mut remote, candidate, None)
            .await;
        // A stopped device accepts the change only by verifying it from the
        // server, which is what ends the stop (§14).
        if !self.halted() {
            self.state.policy = policy_changes(&remote.policy);
            self.remote = Some(remote);
        }
        let revision = pushed?;
        self.save()?;
        Ok(revision)
    }

    async fn push_add_member(
        &mut self,
        service: &mut SyncService,
        remote: &mut super::super::RemoteWorkspace,
        candidate: sync_protocol::records::AccountHead,
        invited: Option<(sync_protocol::types::Id, sync_protocol::records::Role)>,
    ) -> Result<u64, SyncError> {
        service.refresh_policy(remote).await?;
        let mut chains = service.chains_of(remote).await?;
        let (links, genesis) = service
            .client
            .whole_chain(candidate.account, Some(candidate.genesis))
            .await?;
        let chain = verify_served(
            candidate.account,
            genesis,
            &links,
            &std::collections::BTreeSet::new(),
        )?;
        if !chain.contains(&candidate) {
            return Err(SyncError::Check("sync_account_forked"));
        }
        chains.insert(chain);
        let own = service.credentials.account;
        let before = remote.policy.head_snapshot().clone();
        let own_head = before
            .members
            .get(&own)
            .map(|m| m.head.clone())
            .ok_or_else(|| SyncError::Invalid("this account isn't a member here".into()))?;
        let mut heads = BTreeMap::new();
        heads.insert(own, own_head);
        heads.insert(candidate.account, candidate.clone());
        let keys = &service.credentials.keys;
        let (revision, previous) = remote.policy.head();
        let change = Signed::sign(
            PolicyChange {
                workspace: remote.workspace,
                revision: revision + 1,
                previous,
                signer: keys.device(),
                heads: Sorted::new(heads.into_values().collect())
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                kind: match invited {
                    Some((invitation, role)) => PolicyKind::AddMember {
                        account: candidate.account,
                        role,
                        invitation,
                    },
                    None => PolicyKind::AcceptReset {
                        account: candidate.account,
                    },
                },
            },
            keys.signing(),
        );
        let next = remote
            .policy
            .check(&change, &chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        let envelopes = service.envelopes_for(remote, &before, &next)?;
        let mut records = vec![RecordRef::Policy(change.clone())];
        records.extend(envelopes.into_iter().map(RecordRef::Envelope));
        // A new member's history floor needs a checkpoint to bootstrap
        // from; an accepted reset keeps the floor it has.
        if invited.is_some() {
            let bundle = self
                .state
                .log
                .checkpoints
                .sign(
                    self.state.workspace,
                    self.state.domain,
                    self.state.log.cursor,
                    self.state.log.retention_floor,
                    &self.state.log.live,
                    &remote.policy,
                    keys.device(),
                    keys.signing(),
                )
                .ok_or_else(|| {
                    SyncError::Invalid("the workspace domain has nothing to checkpoint yet".into())
                })?;
            records.push(RecordRef::CheckpointBundle(bundle));
        }
        service.client.push(remote.workspace, records).await?;
        remote
            .policy
            .append(&change, &chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        Ok(change.body.revision)
    }
}
