//! Roles and removal (docs/workspace-format/sync-v2.md §8): an owner or
//! admin device changes a member's role, removes a member, or, from the
//! owner, transfers ownership, and a member leaves. Each is one signed
//! policy change at the head the workspace records for the signer's
//! account, checked against the verified policy before it goes out. After
//! the push that carries a change that takes read access away, the
//! signing device rotates each domain it can still read (§9); the other
//! key holders rotate the rest.

use sync_protocol::codec::Sorted;
use sync_protocol::messages::RecordRef;
use sync_protocol::records::{PolicyChange, PolicyKind, Role, Signed};
use sync_protocol::types::Id;

use super::{RemoteWorkspace, SyncError, SyncService};

/// A change to the workspace's members.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemberChange {
    /// Give a member another role: admin, editor or viewer.
    SetRole {
        /// The member's account.
        account: Id,
        /// The new role.
        role: Role,
    },
    /// Take a member's account, with every one of its devices, out of the
    /// workspace. A member may remove itself, unless it owns the
    /// workspace.
    Remove(Id),
    /// Make another member the owner; you become an admin.
    TransferOwner(Id),
}

impl MemberChange {
    fn kind(self) -> PolicyKind {
        match self {
            MemberChange::SetRole { account, role } => PolicyKind::SetRole { account, role },
            MemberChange::Remove(account) => PolicyKind::RemoveMember { account },
            MemberChange::TransferOwner(account) => PolicyKind::TransferOwner { account },
        }
    }
}

impl SyncService {
    /// Signs and pushes `change` in `remote`, then rotates the workspace
    /// domain when the change makes it due and this device still reads it
    /// (§9). The verified policy refuses a change your role doesn't allow
    /// with `sync_policy_unauthorized` before anything goes out. Returns
    /// the new policy revision.
    pub async fn change_member(
        &mut self,
        remote: &mut RemoteWorkspace,
        change: MemberChange,
    ) -> Result<u64, SyncError> {
        self.refresh_policy(remote).await?;
        let chains = self.chains_of(remote).await?;
        let own = self.credentials.account;
        // The signer's account at the head the workspace records, so the
        // change takes no card out; a later head goes in its own
        // `account_heads` change first (§8).
        let own_head = remote
            .policy
            .head_snapshot()
            .members
            .get(&own)
            .map(|m| m.head.clone())
            .ok_or(SyncError::Check("sync_policy_unauthorized"))?;
        let keys = &self.credentials.keys;
        let (revision, previous) = remote.policy.head();
        let signed = Signed::sign(
            PolicyChange {
                workspace: remote.workspace,
                revision: revision + 1,
                previous,
                signer: keys.device(),
                heads: Sorted::new(vec![own_head])
                    .map_err(|e| SyncError::Invalid(e.name().to_owned()))?,
                kind: change.kind(),
            },
            keys.signing(),
        );
        let next = remote
            .policy
            .check(&signed, &chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        self.client
            .push(remote.workspace, vec![RecordRef::Policy(signed.clone())])
            .await?;
        remote
            .policy
            .append(&signed, &chains)
            .map_err(|e| SyncError::Check(e.client_code()))?;
        if next.rotations.contains(&remote.domain)
            && next.can_read(&self.credentials.keys.device(), &remote.domain)
        {
            self.rotate(remote).await?;
        }
        Ok(signed.body.revision)
    }
}
