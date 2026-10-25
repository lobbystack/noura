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
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{Card, LinkKind, PolicyKind};
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
    kind.name()
}

/// The changes of the verified `policy` other than `account_heads` that a
/// card in `cards` signed, each with the undo that `viewer`'s role there
/// allows now (§7, §8 Roles and authority).
pub fn removed_card_changes(
    policy: &PolicyChain,
    viewer: Id,
    cards: &BTreeSet<Id>,
) -> Vec<RemovedCardChange> {
    policy
        .removed_card_changes(&viewer, cards)
        .into_iter()
        .map(|change| RemovedCardChange {
            workspace: policy.workspace(),
            revision: change.revision,
            signer: change.signer,
            kind: change.kind,
            undo: change.undo,
        })
        .collect()
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
        removed_card_changes(&remote.policy, self.credentials.account, cards)
    }

    /// Verifies a `device_removed` answer (§7): the removed session still
    /// reads its account's chain, whose link at the detail's seqno must
    /// remove this card. Returns the device that signed that link. An
    /// answer with no detail, or one that doesn't decode, names no link the
    /// chain could confirm, so it fails with `sync_account_forked`.
    pub async fn verify_removal(&mut self, detail: Option<&[u8]>) -> Result<RemovedBy, SyncError> {
        let detail = detail
            .and_then(|bytes| DeviceRemovedDetail::from_bytes(bytes).ok())
            .ok_or(SyncError::Check("sync_account_forked"))?;
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
    /// each workspace's domain it reads, then appends a `remove` link that
    /// names itself, which ends its sessions, and deletes its keys, session
    /// token and journal key from `store`. Workspace files stay as they are.
    ///
    /// It skips a workspace whose sync is stopped, whose checks refuse what
    /// the server serves, and a domain whose rotation it can't commit, such
    /// as one where its envelope failed and it holds no key, and leaves
    /// those to the readers that remain. Any other failure, such as a server
    /// it can't reach, stops the sign-out before the link, so you can try
    /// again.
    pub async fn sign_out(
        &mut self,
        workspaces: &mut [RemoteWorkspace],
        store: &dyn SyncCredentials,
    ) -> Result<(), SyncError> {
        for remote in workspaces.iter_mut() {
            match self.rotate_if_pending(remote).await {
                Ok(_) => {}
                Err(error) if skips_sign_out_rotation(&error) => {
                    tracing::info!(kind = ?error.code(), "signing out left a rotation to the remaining readers");
                }
                Err(error) => return Err(error),
            }
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

/// Whether signing out leaves a workspace's rotation to the readers that
/// remain after `error` (§7): a client check that stops the workspace or
/// fails its key, or a refusal that means the device can't commit that
/// rotation.
pub fn skips_sign_out_rotation(error: &SyncError) -> bool {
    match error {
        SyncError::Check(_) => true,
        SyncError::Refused { code, .. } => matches!(
            code.as_str(),
            "forbidden"
                | "not_found"
                | "rotation_not_due"
                | "recipients_invalid"
                | "epoch_conflict"
        ),
        _ => false,
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

#[cfg(test)]
mod tests {
    //! The list of a removed card's policy changes, with the undo each role
    //! of the viewer's account allows (§7, §8 Roles and authority). The
    //! server takes these change kinds through ticket 50, so the policies
    //! here are built and verified locally.

    use std::collections::BTreeSet;

    use sync_key_envelope::v2::{DeviceKeys, genesis_link, next_link};
    use sync_protocol::account::AccountChain;
    use sync_protocol::codec::{Nested, Sorted};
    use sync_protocol::policy::{AccountChains, PolicyChain};
    use sync_protocol::records::{
        Access, AccountHead, AccountLink, Level, LinkKind, PolicyChange, PolicyKind, Role, Signed,
    };
    use sync_protocol::types::{Digest, Id};

    use super::removed_card_changes;

    /// An account with its devices and chain links.
    struct Person {
        account: Id,
        devices: Vec<DeviceKeys>,
        links: Vec<Signed<AccountLink>>,
    }

    impl Person {
        fn new(byte: u8) -> Self {
            let account = Id([byte; 16]);
            let first = DeviceKeys::generate().unwrap();
            let links = vec![genesis_link(&first, account)];
            Self {
                account,
                devices: vec![first],
                links,
            }
        }

        fn head(&self) -> AccountHead {
            self.chain().head()
        }

        fn chain(&self) -> AccountChain {
            AccountChain::verify(&self.links, &BTreeSet::new()).unwrap()
        }

        /// Adds a device through the first device.
        fn add(&mut self) -> usize {
            let keys = DeviceKeys::generate().unwrap();
            let link = next_link(
                &self.devices[0],
                &self.head(),
                LinkKind::AddDevice {
                    card: Nested(keys.card(self.account)),
                    offer: Digest([7; 32]),
                },
            );
            self.links.push(link);
            self.devices.push(keys);
            self.devices.len() - 1
        }

        fn device(&self, index: usize) -> &DeviceKeys {
            &self.devices[index]
        }
    }

    /// A workspace's changes as one device after another signs them.
    struct Log {
        workspace: Id,
        changes: Vec<Signed<PolicyChange>>,
    }

    impl Log {
        fn new(byte: u8, owner: &Person) -> Self {
            let mut log = Self {
                workspace: Id([byte; 16]),
                changes: Vec::new(),
            };
            log.push(
                owner.device(0),
                vec![owner.head()],
                PolicyKind::Genesis {
                    workspace_domain: Id([byte + 1; 16]),
                },
            );
            log
        }

        fn push(&mut self, signer: &DeviceKeys, heads: Vec<AccountHead>, kind: PolicyKind) {
            let previous = self.changes.last().map_or(Digest::ZERO, |c| c.digest());
            let change = PolicyChange {
                workspace: self.workspace,
                revision: self.changes.len() as u64 + 1,
                previous,
                signer: signer.device(),
                heads: Sorted::new(heads).unwrap(),
                kind,
            };
            self.changes.push(Signed::sign(change, signer.signing()));
        }

        fn verify(&self, people: &[&Person]) -> PolicyChain {
            let mut chains = AccountChains::new();
            for person in people {
                chains.insert(person.chain());
            }
            PolicyChain::verify(&self.changes, &chains).unwrap()
        }
    }

    fn access(grants: &[(&Person, Level)]) -> sync_protocol::records::AccessList {
        Sorted::new(
            grants
                .iter()
                .map(|(person, level)| Access {
                    account: person.account,
                    level: *level,
                })
                .collect(),
        )
        .unwrap()
    }

    fn add_member(role: Role, who: &Person) -> PolicyKind {
        PolicyKind::AddMember {
            account: who.account,
            role,
            invitation: Id([0x51; 16]),
        }
    }

    /// (revision, kind, undo) for each listed change.
    fn listed(
        policy: &PolicyChain,
        viewer: Id,
        removed: Id,
    ) -> Vec<(u64, &'static str, Option<&'static str>)> {
        removed_card_changes(policy, viewer, &BTreeSet::from([removed]))
            .into_iter()
            .map(|change| {
                assert_eq!(change.workspace, policy.workspace());
                assert_eq!(change.signer, removed);
                (change.revision, change.kind, change.undo)
            })
            .collect()
    }

    #[test]
    fn a_removed_cards_changes_list_with_the_undo_each_role_allows() {
        let mut alice = Person::new(0xa1);
        let phone = alice.add();
        let mut bob = Person::new(0xb0);
        bob.add();
        let carol = Person::new(0xc0);
        let removed = alice.device(phone).device();

        // Alice owns the workspace; her phone, removed since, added members,
        // changed a role, created a domain and changed its access.
        let mut owned = Log::new(0x10, &alice);
        let domain = Id([0x1d; 16]);
        owned.push(
            alice.device(phone),
            vec![alice.head(), bob.head()],
            add_member(Role::Editor, &bob),
        );
        owned.push(
            alice.device(phone),
            vec![alice.head(), carol.head()],
            add_member(Role::Viewer, &carol),
        );
        owned.push(
            alice.device(phone),
            vec![alice.head()],
            PolicyKind::SetRole {
                account: carol.account,
                role: Role::Editor,
            },
        );
        owned.push(
            alice.device(phone),
            vec![alice.head()],
            PolicyKind::CreateDomain {
                domain,
                marker_rid: Id([0x1e; 16]),
                access: access(&[(&alice, Level::Manage)]),
            },
        );
        owned.push(
            alice.device(phone),
            vec![alice.head()],
            PolicyKind::SetAccess {
                domain,
                access: access(&[(&alice, Level::Manage), (&bob, Level::Read)]),
            },
        );
        // Neither an account_heads change nor a change by a device that
        // stays shows.
        alice.add();
        owned.push(
            alice.device(phone),
            vec![alice.head()],
            PolicyKind::AccountHeads,
        );
        owned.push(
            alice.device(0),
            vec![alice.head()],
            PolicyKind::SetRole {
                account: bob.account,
                role: Role::Viewer,
            },
        );
        let policy = owned.verify(&[&alice, &bob, &carol]);
        assert_eq!(
            listed(&policy, alice.account, removed),
            vec![
                (2, "add_member", Some("remove_member")),
                (3, "add_member", Some("remove_member")),
                (4, "set_role", Some("set_role")),
                (5, "create_domain", Some("delete_domain")),
                (6, "set_access", Some("set_access")),
            ],
            "an owner undoes each of them"
        );

        // Alice is an admin of Bob's workspace. Her phone added Carol and
        // made her a viewer, then Bob made Carol an admin, whom an admin
        // can't remove or demote.
        let mut admin = Log::new(0x20, &bob);
        admin.push(
            bob.device(0),
            vec![bob.head(), alice.head()],
            add_member(Role::Admin, &alice),
        );
        admin.push(
            alice.device(phone),
            vec![alice.head(), carol.head()],
            add_member(Role::Editor, &carol),
        );
        admin.push(
            alice.device(phone),
            vec![alice.head()],
            PolicyKind::SetRole {
                account: carol.account,
                role: Role::Viewer,
            },
        );
        admin.push(
            alice.device(phone),
            vec![alice.head()],
            PolicyKind::CreateDomain {
                domain: Id([0x2d; 16]),
                marker_rid: Id([0x2e; 16]),
                access: access(&[(&alice, Level::Manage)]),
            },
        );
        let policy = admin.verify(&[&alice, &bob, &carol]);
        assert_eq!(
            listed(&policy, alice.account, removed),
            vec![
                (3, "add_member", Some("remove_member")),
                (4, "set_role", Some("set_role")),
                (5, "create_domain", Some("delete_domain")),
            ],
            "an admin undoes changes to editors and viewers"
        );
        admin.push(
            bob.device(0),
            vec![bob.head()],
            PolicyKind::SetRole {
                account: carol.account,
                role: Role::Admin,
            },
        );
        let policy = admin.verify(&[&alice, &bob, &carol]);
        assert_eq!(
            listed(&policy, alice.account, removed),
            vec![
                (3, "add_member", None),
                (4, "set_role", None),
                (5, "create_domain", Some("delete_domain")),
            ],
            "an admin can't undo what reaches another admin"
        );

        // Alice edits Bob's workspace and manages one of its domains, whose
        // access her phone changed.
        let mut editor = Log::new(0x30, &bob);
        let managed = Id([0x3d; 16]);
        editor.push(
            bob.device(0),
            vec![bob.head(), alice.head()],
            add_member(Role::Editor, &alice),
        );
        editor.push(
            bob.device(0),
            vec![bob.head(), carol.head()],
            add_member(Role::Viewer, &carol),
        );
        editor.push(
            bob.device(0),
            vec![bob.head()],
            PolicyKind::CreateDomain {
                domain: managed,
                marker_rid: Id([0x3e; 16]),
                access: access(&[(&alice, Level::Manage), (&bob, Level::Manage)]),
            },
        );
        editor.push(
            alice.device(phone),
            vec![alice.head()],
            PolicyKind::SetAccess {
                domain: managed,
                access: access(&[
                    (&alice, Level::Manage),
                    (&bob, Level::Manage),
                    (&carol, Level::Read),
                ]),
            },
        );
        let policy = editor.verify(&[&alice, &bob, &carol]);
        assert_eq!(
            listed(&policy, alice.account, removed),
            vec![(5, "set_access", Some("set_access"))],
            "an editor who manages the domain undoes its access change"
        );
        editor.push(
            bob.device(0),
            vec![bob.head()],
            PolicyKind::SetAccess {
                domain: managed,
                access: access(&[(&bob, Level::Manage), (&carol, Level::Read)]),
            },
        );
        let policy = editor.verify(&[&alice, &bob, &carol]);
        assert_eq!(
            listed(&policy, alice.account, removed),
            vec![(5, "set_access", None)],
            "an editor who no longer manages it can't"
        );
    }
}

#[cfg(test)]
mod sign_out_tests {
    use super::{SyncError, skips_sign_out_rotation};

    #[test]
    fn signing_out_skips_only_rotations_it_cant_commit() {
        assert!(skips_sign_out_rotation(&SyncError::Check(
            "sync_policy_rollback"
        )));
        assert!(skips_sign_out_rotation(&SyncError::Check(
            "sync_key_mismatch"
        )));
        for code in ["forbidden", "rotation_not_due", "epoch_conflict"] {
            assert!(skips_sign_out_rotation(&SyncError::Refused {
                code: code.into(),
                status: 409,
                detail: None,
            }));
        }
        assert!(!skips_sign_out_rotation(&SyncError::Network(
            "connection refused".into()
        )));
        assert!(!skips_sign_out_rotation(&SyncError::Refused {
            code: "unauthorized".into(),
            status: 401,
            detail: None,
        }));
    }
}
