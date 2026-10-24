//! Removing devices, `device_removed`, and signing out
//! (docs/workspace-format/sync-v2.md §7).
//!
//! Removing a device is account-wide: one `remove` link takes down the card
//! and every active card it added, directly or in turn, unless you keep
//! one, never a recovery identity. The device then records the new head in
//! every workspace of your account, which takes the removed cards out and
//! makes the domains they read due for rotation, and rotates each domain it
//! reads. Workspace changes the removed cards signed stay in force, so the
//! app lists them with the undo your role allows. Signing out is a
//! `remove` link that names the device itself.

use std::collections::BTreeSet;

use local_core::sync::SyncCredentials;
use sync_key_envelope::v2::next_link;
use sync_protocol::account::AccountChain;
use sync_protocol::codec::{Decode, Sorted};
use sync_protocol::messages::DeviceRemovedDetail;
use sync_protocol::records::{Card, LinkKind, PolicyKind, Role};
use sync_protocol::types::{Digest, Id};

use super::device::StoredDevice;
use super::{RemoteWorkspace, SyncError, SyncService};

/// What removing a card would take down (§7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovalPreview {
    /// The card you picked.
    pub target: Card,
    /// The other active cards the link would take down with it: the cards
    /// it added, directly or in turn.
    pub with_it: Vec<Card>,
    /// The ones among them you may keep: every device, never a recovery
    /// identity.
    pub keepable: Vec<Id>,
}

/// A workspace change that a removed card signed, which stays in force
/// (§7, §30), with the undo the viewer's role allows; ticket 50 builds the
/// undos.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovedCardChange {
    /// The workspace.
    pub workspace: Id,
    /// The change's revision.
    pub revision: u64,
    /// The removed device that signed it.
    pub signer: Id,
    /// The change's kind, as §8 names it.
    pub kind: &'static str,
    /// The undo your role allows, as §8 names the change, if any.
    pub undo: Option<&'static str>,
}

/// What a removal did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Removal {
    /// The cards the `remove` link took down.
    pub removed: Vec<Id>,
    /// The policy revision each workspace reached, in the order given.
    pub revisions: Vec<u64>,
    /// The workspace changes the removed cards signed.
    pub changes: Vec<RemovedCardChange>,
}

/// Who removed this device, as it verified from its account chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovedBy {
    /// The device that signed the removing link; this device itself for a
    /// sign-out.
    pub signer: Id,
    /// That device's fingerprint.
    pub signer_fingerprint: Digest,
    /// The removing link's seqno.
    pub seqno: u64,
}

/// A policy change's kind, as §8 names it.
pub fn kind_name(kind: &PolicyKind) -> &'static str {
    match kind {
        PolicyKind::Genesis { .. } => "genesis",
        PolicyKind::AccountHeads => "account_heads",
        PolicyKind::AddMember { .. } => "add_member",
        PolicyKind::SetRole { .. } => "set_role",
        PolicyKind::RemoveMember { .. } => "remove_member",
        PolicyKind::CreateDomain { .. } => "create_domain",
        PolicyKind::SetAccess { .. } => "set_access",
        PolicyKind::DeleteDomain { .. } => "delete_domain",
        PolicyKind::TransferOwner { .. } => "transfer_owner",
        PolicyKind::AcceptReset { .. } => "accept_reset",
    }
}

/// The undo of `kind` that `role` allows in a workspace (§8, Roles and
/// authority), with `manages` telling whether the viewer manages the
/// domain a `set_access` or `create_domain` names.
fn undo_for(
    kind: &PolicyKind,
    role: Option<Role>,
    target_role: Option<Role>,
    manages: bool,
) -> Option<&'static str> {
    let admin = matches!(role, Some(Role::Owner | Role::Admin));
    match kind {
        PolicyKind::AddMember { .. } | PolicyKind::AcceptReset { .. } => {
            let removable = match role {
                Some(Role::Owner) => target_role != Some(Role::Owner),
                Some(Role::Admin) => matches!(target_role, Some(Role::Editor | Role::Viewer)),
                _ => false,
            };
            removable.then_some("remove_member")
        }
        PolicyKind::SetRole { .. } => {
            let allowed = match role {
                Some(Role::Owner) => target_role != Some(Role::Owner),
                Some(Role::Admin) => matches!(target_role, Some(Role::Editor | Role::Viewer)),
                _ => false,
            };
            allowed.then_some("set_role")
        }
        PolicyKind::SetAccess { .. } => manages.then_some("set_access"),
        PolicyKind::CreateDomain { .. } => (admin || manages).then_some("delete_domain"),
        _ => None,
    }
}

impl SyncService {
    /// What removing `target` from the account would take down (§7).
    pub async fn removal_preview(&mut self, target: Id) -> Result<RemovalPreview, SyncError> {
        let chain = self.own_chain().await?;
        let removed = chain
            .removal(self.credentials.keys.device(), target, &[])
            .map_err(|e| SyncError::Invalid(format!("this card can't be removed: {e}")))?;
        let card = |id: &Id| chain.card(id).map(|s| s.card.clone());
        let target_card = card(&target).ok_or_else(|| SyncError::Invalid("no such card".into()))?;
        let with_it: Vec<Card> = removed
            .iter()
            .filter(|id| **id != target)
            .filter_map(card)
            .collect();
        let keepable = removed
            .iter()
            .filter(|id| **id != target)
            .filter(|id| chain.card(id).is_some_and(|s| !s.recovery))
            .copied()
            .collect();
        Ok(RemovalPreview {
            target: target_card,
            with_it,
            keepable,
        })
    }

    /// Removes `target` from the account, keeping `keep` among the cards it
    /// would take down (§7): appends the `remove` link, then records the new
    /// head in each of `workspaces`, which rotates every domain the device
    /// reads there, and lists the workspace changes the removed cards
    /// signed.
    pub async fn remove_device(
        &mut self,
        target: Id,
        keep: &[Id],
        workspaces: &mut [RemoteWorkspace],
    ) -> Result<Removal, SyncError> {
        let chain = self.own_chain().await?;
        let device = self.credentials.keys.device();
        let removed = chain
            .removal(device, target, keep)
            .map_err(|e| SyncError::Invalid(format!("this card can't be removed: {e}")))?;
        let link = next_link(
            &self.credentials.keys,
            &chain.head(),
            LinkKind::Remove {
                device: target,
                keep: Sorted::new(keep.to_vec())
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
            },
        );
        let digest = link.digest();
        self.append_links(vec![link]).await?;
        self.heads.witness(digest)?;
        let mut revisions = Vec::with_capacity(workspaces.len());
        let mut changes = Vec::new();
        for remote in workspaces.iter_mut() {
            self.record_heads(remote, &[]).await?;
            revisions.push(remote.policy.head().0);
            changes.extend(self.changes_signed_by(remote, &removed));
        }
        Ok(Removal {
            removed: removed.into_iter().collect(),
            revisions,
            changes,
        })
    }

    /// The changes of `remote` other than `account_heads` that a card in
    /// `cards` signed, with the undo the device's account's role allows.
    pub fn changes_signed_by(
        &self,
        remote: &RemoteWorkspace,
        cards: &BTreeSet<Id>,
    ) -> Vec<RemovedCardChange> {
        let snapshot = remote.policy.head_snapshot();
        let role = snapshot.role(&self.credentials.account);
        let account = self.credentials.account;
        (1..=remote.policy.head().0)
            .filter_map(|revision| remote.policy.change(revision))
            .filter(|change| cards.contains(&change.body.signer))
            .filter(|change| !matches!(change.body.kind, PolicyKind::AccountHeads))
            .map(|change| {
                let kind = &change.body.kind;
                let target_role = match kind {
                    PolicyKind::AddMember { account, .. }
                    | PolicyKind::SetRole { account, .. }
                    | PolicyKind::AcceptReset { account } => snapshot.role(account),
                    _ => None,
                };
                let manages = match kind {
                    PolicyKind::SetAccess { domain, .. }
                    | PolicyKind::CreateDomain { domain, .. } => snapshot.manages(&account, domain),
                    _ => false,
                };
                RemovedCardChange {
                    workspace: remote.workspace,
                    revision: change.body.revision,
                    signer: change.body.signer,
                    kind: kind_name(kind),
                    undo: undo_for(kind, role, target_role, manages),
                }
            })
            .collect()
    }

    /// Verifies a `device_removed` answer (§7): the removed session still
    /// reads its account's chain, whose link at the detail's seqno must
    /// remove this card. Returns the device that signed that link.
    pub async fn verify_removal(&mut self, detail: &[u8]) -> Result<RemovedBy, SyncError> {
        let detail = DeviceRemovedDetail::from_bytes(detail)
            .map_err(|_| SyncError::Network("the device_removed detail doesn't decode".into()))?;
        let chain = self.own_chain().await?;
        let device = self.credentials.keys.device();
        let removed_at = chain.card(&device).and_then(|s| s.removed_at);
        if chain.genesis() != detail.genesis || removed_at != Some(detail.seqno) {
            return Err(SyncError::Check("sync_account_forked"));
        }
        let (links, _) = self
            .client
            .whole_chain(chain.account(), Some(chain.genesis()))
            .await?;
        let link = links
            .get(detail.seqno as usize - 1)
            .ok_or(SyncError::Check("sync_account_forked"))?;
        let signer = link.body.signer;
        let signer_fingerprint = chain
            .card(&signer)
            .map(|s| s.card.fingerprint())
            .ok_or(SyncError::Check("sync_account_unauthorized"))?;
        Ok(RemovedBy {
            signer,
            signer_fingerprint,
            seqno: detail.seqno,
        })
    }

    /// Whether this device is the account's last active device, so signing
    /// out leaves you needing a recovery kit or a reset to read synced data
    /// again (§7).
    pub async fn is_last_device(&mut self) -> Result<bool, SyncError> {
        let chain = self.own_chain().await?;
        Ok(last_device(&chain, &self.credentials.keys.device()))
    }

    /// Signs this device out (§7): it first finishes the pending rotation of
    /// each workspace's domain it reads, skipping a workspace whose
    /// rotation it can't commit, then appends a `remove` link that names
    /// itself, which ends its sessions, and deletes its keys, session token
    /// and journal key from `store`. Workspace files stay as they are.
    pub async fn sign_out(
        mut self,
        workspaces: &mut [RemoteWorkspace],
        store: &dyn SyncCredentials,
    ) -> Result<(), SyncError> {
        for remote in workspaces.iter_mut() {
            // A stopped workspace, or a domain whose key the device lost,
            // stays with the readers that remain.
            let _ = self.rotate_if_pending(remote).await;
        }
        let chain = self.own_chain().await?;
        let device = self.credentials.keys.device();
        let link = next_link(
            &self.credentials.keys,
            &chain.head(),
            LinkKind::Remove {
                device,
                keep: Sorted::default(),
            },
        );
        self.client.append_chain(vec![link]).await?;
        StoredDevice::delete(store, self.client.base())?;
        for item in super::account_heads::device_items(&device) {
            store
                .delete(&item)
                .map_err(|e| SyncError::Invalid(format!("the credential store failed: {e}")))?;
        }
        Ok(())
    }
}

/// Whether `device` is the only active device of `chain`, a recovery
/// identity aside.
pub fn last_device(chain: &AccountChain, device: &Id) -> bool {
    let head = chain.head_seqno();
    chain
        .active_at(head)
        .iter()
        .filter(|s| !s.recovery)
        .all(|s| s.card.device == *device)
}
