//! Who reads a restricted folder (docs/workspace-format/sync-v2.md §8, §9):
//! a manager changes a domain's access list with `set_access`, an owner or
//! admin only removes entries, and an owner, admin or manager deletes the
//! domain. A grant carries the new readers' envelopes and, unless the same
//! change takes a reader out, a fresh checkpoint that sets the grantee's
//! history floor. A change that takes a reader out rotates the folder's
//! key: the signer rotates when it still reads the domain, and any other
//! device that holds the key does otherwise.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Instant;

use sync_protocol::codec::Sorted;
use sync_protocol::messages::RecordRef;
use sync_protocol::records::{Access, Level, PolicyKind};
use sync_protocol::types::Id;

use super::super::heads::random_wait;
use super::super::{SyncError, SyncService};
use super::{WorkspaceSync, policy_changes};

impl WorkspaceSync {
    /// Sets `domain`'s access list to `access`. Returns the new policy
    /// revision.
    pub async fn set_access(
        &mut self,
        service: &mut SyncService,
        domain: Id,
        access: &[Access],
    ) -> Result<u64, SyncError> {
        if self.remote.is_none() {
            self.connect(service).await?;
        }
        // The fresh checkpoint covers what the device has applied.
        self.round(service, 0).await?;
        let list: BTreeMap<Id, Level> = access.iter().map(|a| (a.account, a.level)).collect();
        let list = Sorted::new(
            list.into_iter()
                .map(|(account, level)| Access { account, level })
                .collect(),
        )
        .map_err(|e| SyncError::Invalid(e.name().to_owned()))?;
        let reads = self.loaded_domains().contains(&domain);
        if reads {
            self.swap_domain(domain);
        }
        let pushed = self
            .push_policy_change(
                service,
                domain,
                PolicyKind::SetAccess {
                    domain,
                    access: list,
                },
            )
            .await;
        if reads {
            self.swap_domain(domain);
        }
        let revision = pushed?;
        self.follow_change(service).await?;
        Ok(revision)
    }

    /// Hands a domain to `to` in two changes, since the server takes the
    /// new readers' envelopes only from a device that reads the domain
    /// after the change (§8): one that makes `to` a manager, and one that
    /// takes this account off. Another key holder then rotates the folder.
    pub async fn hand_off_domain(
        &mut self,
        service: &mut SyncService,
        domain: Id,
        to: Id,
    ) -> Result<u64, SyncError> {
        let own = service.credentials.account;
        let mut list = self
            .remote
            .as_ref()
            .and_then(|r| r.policy.head_snapshot().domains.get(&domain).cloned())
            .ok_or_else(|| SyncError::Invalid("no such restricted folder".into()))?
            .access;
        list.insert(to, Level::Manage);
        let entries = |list: &BTreeMap<Id, Level>| -> Vec<Access> {
            list.iter()
                .map(|(account, level)| Access {
                    account: *account,
                    level: *level,
                })
                .collect()
        };
        self.set_access(service, domain, &entries(&list)).await?;
        list.remove(&own);
        self.set_access(service, domain, &entries(&list)).await
    }

    /// Deletes a restricted domain (§8). The server answers `not_found`
    /// for it from then on, and the device keeps the files, held for
    /// review.
    pub async fn delete_domain(
        &mut self,
        service: &mut SyncService,
        domain: Id,
    ) -> Result<u64, SyncError> {
        if self.remote.is_none() {
            self.connect(service).await?;
        }
        let revision = self
            .push_policy_change(service, domain, PolicyKind::DeleteDomain { domain })
            .await?;
        self.follow_change(service).await?;
        Ok(revision)
    }

    /// Signs `kind` where the device works, pushes it with what it owes,
    /// and rotates `domain` when the change makes it due and the device
    /// still reads it.
    async fn push_policy_change(
        &mut self,
        service: &mut SyncService,
        domain: Id,
        kind: PolicyKind,
    ) -> Result<u64, SyncError> {
        let mut remote = self.remote.take().ok_or(SyncError::KeyMissing)?;
        let pushed = self.sign_and_push(service, &mut remote, domain, kind).await;
        if let Err(error) = &pushed {
            tracing::warn!(%error, "a policy change didn't go through");
        }
        self.adopt(remote)?;
        pushed
    }

    async fn sign_and_push(
        &mut self,
        service: &mut SyncService,
        remote: &mut super::super::RemoteWorkspace,
        domain: Id,
        kind: PolicyKind,
    ) -> Result<u64, SyncError> {
        let before = {
            service.refresh_policy(remote).await?;
            remote.policy.head_snapshot().clone()
        };
        let (change, next, chains) = service.sign_change(remote, kind).await?;
        let mut records = vec![RecordRef::Policy(change.clone())];
        if next.new_readers.contains_key(&domain) {
            let envelopes = service.envelopes_for(remote, &before, &next)?;
            records.extend(envelopes.into_iter().map(RecordRef::Envelope));
            // A checkpoint can't commit in a domain due for rotation, so
            // the grant's own entry sets the floor then (§9).
            if !next.rotations.contains(&domain) {
                let keys = &service.credentials.keys;
                let bundle = self.state.log.checkpoints.sign(
                    self.state.workspace,
                    self.state.domain,
                    self.state.log.cursor,
                    self.state.log.retention_floor,
                    &self.state.log.live,
                    &remote.policy,
                    keys.device(),
                    keys.signing(),
                );
                records.extend(bundle.map(RecordRef::CheckpointBundle));
            }
        }
        service.client.push(remote.workspace, records).await?;
        remote
            .policy
            .append(&change, &chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        self.state.policy = policy_changes(&remote.policy);
        let device = service.credentials.keys.device();
        if next.rotations.contains(&domain) && next.can_read(&device, &domain) {
            service.rotate_if_pending(remote).await?;
        }
        Ok(change.body.revision)
    }

    /// Takes the change the device just pushed everywhere it works.
    async fn follow_change(&mut self, service: &mut SyncService) -> Result<(), SyncError> {
        self.refresh(service).await?;
        self.sync_domains(service).await?;
        self.save()
    }

    /// Rotates each restricted domain the device reads whose rotation is
    /// pending (§9): at once for the signer of a change, after a random
    /// wait for every other device that holds the key.
    pub(super) async fn rotate_domains(
        &mut self,
        service: &mut SyncService,
        now: bool,
    ) -> Result<(), SyncError> {
        if self.active.is_some() {
            return Ok(());
        }
        let mut pending = BTreeSet::new();
        for domain in self.loaded_domains() {
            self.swap_domain(domain);
            let rotated = self.rotate_here(service, domain, now, &mut pending).await;
            self.swap_domain(domain);
            rotated?;
        }
        self.rotation_due
            .retain(|domain, _| pending.contains(domain));
        Ok(())
    }

    async fn rotate_here(
        &mut self,
        service: &mut SyncService,
        domain: Id,
        now: bool,
        pending: &mut BTreeSet<Id>,
    ) -> Result<(), SyncError> {
        let Some(remote) = self.remote.as_mut() else {
            return Ok(());
        };
        if !service.rotation_pending(remote) {
            return Ok(());
        }
        pending.insert(domain);
        let wait = std::time::Duration::ZERO..self.rotation_wait;
        let due = *self
            .rotation_due
            .entry(domain)
            .or_insert_with(|| Instant::now() + random_wait(&wait));
        if !now && due > Instant::now() {
            return Ok(());
        }
        let remote = self.remote.as_mut().ok_or(SyncError::KeyMissing)?;
        service.refresh_policy(remote).await?;
        service.refresh_keys(remote).await?;
        service.rotate_if_pending(remote).await?;
        let remote = self.remote.take().ok_or(SyncError::KeyMissing)?;
        self.adopt(remote)
    }

    /// Sets how long this device waits before it rotates a restricted
    /// domain another device's change made due.
    pub fn set_rotation_wait(&mut self, wait: std::time::Duration) {
        self.rotation_wait = wait;
    }

    /// Sets how long this device leaves another device's moves before it
    /// finishes them itself.
    pub fn set_move_wait(&mut self, wait: std::time::Duration) {
        self.move_wait = wait;
    }
}
