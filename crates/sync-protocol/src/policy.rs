//! Policy verification (§8), from the policy chain and the account chains
//! its changes record.
//!
//! A workspace's policy is a hash chain of signed changes. It names member
//! accounts and their roles, never device cards: a member's devices are the
//! active cards of its account chain at the head the policy last recorded
//! for it. [`PolicyChain`] checks each change's links, account heads,
//! signature and authority, and keeps the state at every revision, so
//! server and client judge records by the policy at any revision.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::account::AccountChain;
use crate::records::{AccountHead, Card, Level, PolicyChange, PolicyKind, Role, Signed};
use crate::types::{Digest, Id};

/// The most readers a domain may have (§8).
pub const MAX_READERS: usize = 512;
/// The most card indices a workspace assigns over its life (§8).
pub const MAX_CARD_INDEX: u32 = (1 << 24) - 1;
/// The most domains a workspace holds (§9).
pub const MAX_DOMAINS: usize = 256;

/// The verified account chains a policy's changes record, keyed by account
/// and genesis digest.
#[derive(Clone, Debug, Default)]
pub struct AccountChains {
    chains: BTreeMap<(Id, Digest), AccountChain>,
}

impl AccountChains {
    /// No chains.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces a verified chain.
    pub fn insert(&mut self, chain: AccountChain) {
        self.chains
            .insert((chain.account(), chain.genesis()), chain);
    }

    /// The chain of `account` under `genesis`.
    pub fn get(&self, account: &Id, genesis: &Digest) -> Option<&AccountChain> {
        self.chains.get(&(*account, *genesis))
    }

    /// The chain a head lies on, if it's verified and reaches the head.
    pub fn chain_of(&self, head: &AccountHead) -> Option<&AccountChain> {
        self.get(&head.account, &head.genesis)
            .filter(|c| c.contains(head))
    }
}

/// A member account's place in the policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Member {
    /// Its role.
    pub role: Role,
    /// The head of its account chain the workspace records.
    pub head: AccountHead,
    /// Whether a change that a device of this account signed recorded
    /// `head` (§8, Account heads).
    pub recorded_by_own: bool,
}

/// A key domain's place in the policy.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Domain {
    /// Whether this is a restricted domain; the workspace domain isn't.
    pub restricted: bool,
    /// A restricted domain's marker `rid`.
    pub marker_rid: Option<Id>,
    /// A restricted domain's grants.
    pub access: BTreeMap<Id, Level>,
    /// The revision that created it.
    pub created_at: u64,
    /// The device that signed the change that created it.
    pub creator: Id,
}

/// A device of the workspace at some revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceDevice {
    /// The account whose chain lists it.
    pub account: Id,
    /// Its card, byte for byte from the account chain.
    pub card: Card,
    /// Whether it is a recovery identity.
    pub recovery: bool,
}

/// The policy's state after one revision.
#[derive(Clone, Debug)]
pub struct Snapshot {
    /// The revision.
    pub revision: u64,
    /// The digest of the change at this revision.
    pub digest: Digest,
    /// The change's signer.
    pub signer: Id,
    /// The workspace domain.
    pub workspace_domain: Id,
    /// Member accounts.
    pub members: BTreeMap<Id, Member>,
    /// The last head recorded for each account that left.
    pub former: BTreeMap<Id, AccountHead>,
    /// Live domains, the workspace domain included.
    pub domains: BTreeMap<Id, Domain>,
    /// Every domain ID ever used.
    pub used_domains: BTreeSet<Id>,
    /// The workspace's devices.
    pub devices: BTreeMap<Id, WorkspaceDevice>,
    /// Card indices assigned so far, for the workspace's life.
    pub card_indices: BTreeMap<Id, u32>,
    /// Domains this change makes due for rotation.
    pub rotations: BTreeSet<Id>,
    /// Devices this change brings into each domain's readers.
    pub new_readers: BTreeMap<Id, BTreeSet<Id>>,
}

impl Snapshot {
    /// The owner account.
    pub fn owner(&self) -> Option<Id> {
        self.members
            .iter()
            .find(|(_, m)| m.role == Role::Owner)
            .map(|(a, _)| *a)
    }

    /// The role of `account`.
    pub fn role(&self, account: &Id) -> Option<Role> {
        self.members.get(account).map(|m| m.role)
    }

    /// The account of a device of the workspace.
    pub fn account_of(&self, device: &Id) -> Option<Id> {
        self.devices.get(device).map(|d| d.account)
    }

    /// Whether `device` is a device of the workspace at this revision.
    pub fn is_device(&self, device: &Id) -> bool {
        self.devices.contains_key(device)
    }

    fn account_level(&self, account: &Id, domain: &Id) -> Option<Level> {
        let d = self.domains.get(domain)?;
        if d.restricted {
            d.access.get(account).copied()
        } else {
            match self.role(account)? {
                Role::Owner | Role::Admin | Role::Editor => Some(Level::Write),
                Role::Viewer => Some(Level::Read),
            }
        }
    }

    /// The level at which `device` reaches `domain`: a restricted domain's
    /// access list alone decides, whatever the workspace role.
    pub fn level(&self, device: &Id, domain: &Id) -> Option<Level> {
        self.account_level(&self.account_of(device)?, domain)
    }

    /// Whether `device` could read `domain` at this revision.
    pub fn can_read(&self, device: &Id, domain: &Id) -> bool {
        self.level(device, domain).is_some()
    }

    /// Whether `device` could write `domain` at this revision.
    pub fn can_write(&self, device: &Id, domain: &Id) -> bool {
        self.level(device, domain)
            .is_some_and(|l| l >= Level::Write)
    }

    /// Whether `account` manages restricted domain `domain`.
    pub fn manages(&self, account: &Id, domain: &Id) -> bool {
        self.domains
            .get(domain)
            .is_some_and(|d| d.restricted && d.access.get(account) == Some(&Level::Manage))
    }

    /// The devices that read `domain`, sorted: an epoch's recipients.
    pub fn readers(&self, domain: &Id) -> BTreeSet<Id> {
        self.devices
            .keys()
            .filter(|device| self.can_read(device, domain))
            .copied()
            .collect()
    }
}

/// A change that breaks a rule of §8.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum PolicyError {
    /// The change names another workspace.
    #[error("revision {revision}: wrong workspace")]
    Workspace {
        /// The failing change's revision.
        revision: u64,
    },
    /// The revision isn't one above the head's.
    #[error("revision {revision}: wrong revision")]
    Revision {
        /// The failing change's revision.
        revision: u64,
    },
    /// `previous` isn't the head's digest.
    #[error("revision {revision}: previous isn't the head")]
    Previous {
        /// The failing change's revision.
        revision: u64,
    },
    /// A genesis away from revision 1, or another kind at revision 1.
    #[error("revision {revision}: genesis out of place")]
    GenesisPosition {
        /// The failing change's revision.
        revision: u64,
    },
    /// A recorded head lies on no account chain the verifier holds.
    #[error("revision {revision}: a head lies on no verified chain")]
    UnknownHead {
        /// The failing change's revision.
        revision: u64,
    },
    /// A member's head lies behind the recorded one, or on another chain
    /// where only `add_member` and `accept_reset` may move it.
    #[error("revision {revision}: a head is behind or off the recorded chain")]
    HeadBehind {
        /// The failing change's revision.
        revision: u64,
    },
    /// `heads` holds no head of the signer's account at which the signer is
    /// active.
    #[error("revision {revision}: the signer isn't active at a recorded head")]
    SignerNotInHeads {
        /// The failing change's revision.
        revision: u64,
    },
    /// `heads` records an account outside `add_member`, `accept_reset` and
    /// a member account in `account_heads`, or lacks the head the kind
    /// needs.
    #[error("revision {revision}: heads record the wrong accounts")]
    ForeignHead {
        /// The failing change's revision.
        revision: u64,
    },
    /// The signature doesn't verify under the signer's card.
    #[error("revision {revision}: bad signature")]
    Signature {
        /// The failing change's revision.
        revision: u64,
    },
    /// A recovery identity signed a policy change.
    #[error("revision {revision}: a recovery identity signs no policy change")]
    RecoverySigner {
        /// The failing change's revision.
        revision: u64,
    },
    /// The signer's account lacks the authority for this kind.
    #[error("revision {revision}: not authorized")]
    Unauthorized {
        /// The failing change's revision.
        revision: u64,
    },
    /// An `account_heads` change that records no later head and isn't the
    /// one allowed repeat.
    #[error("revision {revision}: account_heads records nothing new")]
    NothingNew {
        /// The failing change's revision.
        revision: u64,
    },
    /// The change names an account, domain or role it can't apply to.
    #[error("revision {revision}: invalid target")]
    InvalidTarget {
        /// The failing change's revision.
        revision: u64,
    },
    /// The heads would make one device ID active under two accounts.
    #[error("revision {revision}: a device ID under two accounts")]
    DeviceUnderTwoAccounts {
        /// The failing change's revision.
        revision: u64,
    },
    /// A domain would have more than 512 readers, or the workspace more
    /// than 16,777,215 card indices.
    #[error("revision {revision}: recipient limit")]
    RecipientLimit {
        /// The failing change's revision.
        revision: u64,
    },
    /// The workspace would hold more than 256 domains.
    #[error("revision {revision}: domain limit")]
    DomainLimit {
        /// The failing change's revision.
        revision: u64,
    },
}

impl PolicyError {
    /// The server's answer to a pushed change failing this way.
    pub fn server_code(&self) -> &'static str {
        match self {
            PolicyError::Revision { .. } | PolicyError::Previous { .. } => "policy_conflict",
            PolicyError::UnknownHead { .. } | PolicyError::HeadBehind { .. } => "account_conflict",
            PolicyError::Signature { .. } => "signature_invalid",
            PolicyError::DeviceUnderTwoAccounts { .. } => "id_conflict",
            PolicyError::RecipientLimit { .. } => "recipient_limit",
            PolicyError::DomainLimit { .. } => "domain_limit",
            _ => "forbidden",
        }
    }

    /// The client's report on a served change failing this way.
    pub fn client_code(&self) -> &'static str {
        match self {
            PolicyError::Revision { .. } | PolicyError::Previous { .. } => "sync_policy_forked",
            PolicyError::UnknownHead { .. } | PolicyError::HeadBehind { .. } => {
                "sync_account_forked"
            }
            _ => "sync_policy_unauthorized",
        }
    }
}

/// A verified policy chain with its state at every revision.
#[derive(Clone, Debug)]
pub struct PolicyChain {
    workspace: Id,
    changes: Vec<Signed<PolicyChange>>,
    snapshots: Vec<Snapshot>,
}

impl PolicyChain {
    /// Verifies a chain from its genesis.
    pub fn verify(
        changes: &[Signed<PolicyChange>],
        chains: &AccountChains,
    ) -> Result<Self, PolicyError> {
        let (genesis, rest) = changes
            .split_first()
            .ok_or(PolicyError::Revision { revision: 0 })?;
        let mut chain = Self::from_genesis(genesis, chains)?;
        for change in rest {
            chain.append(change, chains)?;
        }
        Ok(chain)
    }

    /// Starts a chain from its genesis change.
    pub fn from_genesis(
        change: &Signed<PolicyChange>,
        chains: &AccountChains,
    ) -> Result<Self, PolicyError> {
        let body = &change.body;
        let revision = body.revision;
        if revision != 1 {
            return Err(PolicyError::Revision { revision });
        }
        if !body.previous.is_zero() {
            return Err(PolicyError::Previous { revision });
        }
        let PolicyKind::Genesis { workspace_domain } = body.kind else {
            return Err(PolicyError::GenesisPosition { revision });
        };
        let [head] = body.heads.as_ref() else {
            return Err(PolicyError::ForeignHead { revision });
        };
        let account_chain = chains
            .chain_of(head)
            .ok_or(PolicyError::UnknownHead { revision })?;
        check_signer(change, account_chain, head)?;
        let mut members = BTreeMap::new();
        members.insert(
            head.account,
            Member {
                role: Role::Owner,
                head: head.clone(),
                recorded_by_own: true,
            },
        );
        let mut domains = BTreeMap::new();
        domains.insert(
            workspace_domain,
            Domain {
                restricted: false,
                marker_rid: None,
                access: BTreeMap::new(),
                created_at: 1,
                creator: body.signer,
            },
        );
        let mut snapshot = Snapshot {
            revision: 1,
            digest: change.digest(),
            signer: body.signer,
            workspace_domain,
            members,
            former: BTreeMap::new(),
            domains,
            used_domains: BTreeSet::from([workspace_domain]),
            devices: BTreeMap::new(),
            card_indices: BTreeMap::new(),
            rotations: BTreeSet::new(),
            new_readers: BTreeMap::new(),
        };
        let empty = snapshot.clone();
        finish_snapshot(&mut snapshot, &empty, chains, revision)?;
        Ok(Self {
            workspace: body.workspace,
            changes: vec![change.clone()],
            snapshots: vec![snapshot],
        })
    }

    /// Takes the next change, applying every rule of §8.
    pub fn append(
        &mut self,
        change: &Signed<PolicyChange>,
        chains: &AccountChains,
    ) -> Result<&Snapshot, PolicyError> {
        let snapshot = self.check(change, chains)?;
        self.changes.push(change.clone());
        self.snapshots.push(snapshot);
        Ok(self.snapshots.last().expect("just pushed"))
    }

    /// Checks the next change without taking it, returning the state it
    /// would produce.
    pub fn check(
        &self,
        change: &Signed<PolicyChange>,
        chains: &AccountChains,
    ) -> Result<Snapshot, PolicyError> {
        let prev = self.head_snapshot();
        let body = &change.body;
        let revision = body.revision;
        if body.workspace != self.workspace {
            return Err(PolicyError::Workspace { revision });
        }
        if revision != prev.revision + 1 {
            return Err(PolicyError::Revision { revision });
        }
        if body.previous != prev.digest {
            return Err(PolicyError::Previous { revision });
        }
        if matches!(body.kind, PolicyKind::Genesis { .. }) {
            return Err(PolicyError::GenesisPosition { revision });
        }

        // Every head lies on a verified chain.
        for head in body.heads.iter() {
            if chains.chain_of(head).is_none() {
                return Err(PolicyError::UnknownHead { revision });
            }
        }
        // The signer is active at a head of its own account.
        let signer_head = body
            .heads
            .iter()
            .find(|h| {
                chains
                    .chain_of(h)
                    .is_some_and(|c| c.is_active_at(&body.signer, h.seqno))
            })
            .ok_or(PolicyError::SignerNotInHeads { revision })?;
        let signer_chain = chains.chain_of(signer_head).expect("checked above");
        check_signer(change, signer_chain, signer_head)?;
        let signer_account = signer_head.account;
        let signer_role = prev
            .role(&signer_account)
            .ok_or(PolicyError::Unauthorized { revision })?;

        // Which accounts may appear in heads.
        let named_new = match &body.kind {
            PolicyKind::AddMember { account, .. } | PolicyKind::AcceptReset { account } => {
                Some(*account)
            }
            _ => None,
        };
        for head in body.heads.iter() {
            let allowed = head.account == signer_account
                || Some(head.account) == named_new
                || (matches!(body.kind, PolicyKind::AccountHeads)
                    && prev.members.contains_key(&head.account));
            if !allowed {
                return Err(PolicyError::ForeignHead { revision });
            }
        }
        if let Some(account) = named_new {
            if account == signer_account {
                return Err(PolicyError::Unauthorized { revision });
            }
            if !body.heads.iter().any(|h| h.account == account) {
                return Err(PolicyError::ForeignHead { revision });
            }
        }

        // Heads of member accounts move forward on the recorded chain.
        for head in body.heads.iter() {
            let reset =
                matches!(body.kind, PolicyKind::AcceptReset { account } if account == head.account);
            let behind = match (
                prev.members.get(&head.account),
                prev.former.get(&head.account),
            ) {
                // A member's head moves forward on its recorded chain; only
                // an accept_reset moves it to another chain.
                (Some(member), _) if head.genesis == member.head.genesis => {
                    reset || head.seqno < member.head.seqno
                }
                (Some(_), _) => !reset,
                // A returning member's head doesn't fall behind its last
                // recorded head on the same chain.
                (None, Some(former)) => head.genesis == former.genesis && head.seqno < former.seqno,
                (None, None) => false,
            };
            if behind {
                return Err(PolicyError::HeadBehind { revision });
            }
        }

        let mut next = prev.clone();
        next.revision = revision;
        next.digest = change.digest();
        next.signer = body.signer;
        next.rotations.clear();
        next.new_readers.clear();

        let is_admin = matches!(signer_role, Role::Owner | Role::Admin);
        let unauthorized = PolicyError::Unauthorized { revision };
        let invalid = PolicyError::InvalidTarget { revision };
        match &body.kind {
            PolicyKind::Genesis { .. } => unreachable!("refused above"),
            PolicyKind::AccountHeads => {
                let later = body.heads.iter().any(|h| {
                    prev.members
                        .get(&h.account)
                        .is_some_and(|m| h.genesis == m.head.genesis && h.seqno > m.head.seqno)
                });
                if !later {
                    let own = &prev.members[&signer_account];
                    let repeat =
                        body.heads.len() == 1 && *signer_head == own.head && !own.recorded_by_own;
                    if !repeat {
                        return Err(PolicyError::NothingNew { revision });
                    }
                }
            }
            PolicyKind::AddMember { account, role, .. } => {
                if prev.members.contains_key(account) {
                    return Err(invalid);
                }
                let allowed = match signer_role {
                    Role::Owner => *role != Role::Owner,
                    Role::Admin => matches!(role, Role::Editor | Role::Viewer),
                    _ => false,
                };
                if !allowed {
                    return Err(unauthorized);
                }
                let head = body
                    .heads
                    .iter()
                    .find(|h| h.account == *account)
                    .expect("checked");
                next.former.remove(account);
                next.members.insert(
                    *account,
                    Member {
                        role: *role,
                        head: head.clone(),
                        recorded_by_own: false,
                    },
                );
            }
            PolicyKind::SetRole { account, role } => {
                let current = prev.role(account).ok_or(invalid)?;
                if current == Role::Owner || *role == Role::Owner {
                    return Err(invalid);
                }
                let allowed = match signer_role {
                    Role::Owner => true,
                    Role::Admin => {
                        matches!(current, Role::Editor | Role::Viewer)
                            && matches!(role, Role::Editor | Role::Viewer)
                    }
                    _ => false,
                };
                if !allowed {
                    return Err(unauthorized);
                }
                next.members.get_mut(account).expect("member").role = *role;
            }
            PolicyKind::RemoveMember { account } => {
                let current = prev.role(account).ok_or(invalid)?;
                if current == Role::Owner {
                    return Err(invalid);
                }
                let allowed = *account == signer_account
                    || signer_role == Role::Owner
                    || (signer_role == Role::Admin
                        && matches!(current, Role::Editor | Role::Viewer));
                if !allowed {
                    return Err(unauthorized);
                }
                let member = next.members.remove(account).expect("member");
                next.former.insert(*account, member.head);
                drop_grants(&mut next, account);
            }
            PolicyKind::CreateDomain {
                domain,
                marker_rid: marker,
                access,
            } => {
                if !is_admin {
                    return Err(unauthorized);
                }
                if next.used_domains.contains(domain) {
                    return Err(invalid);
                }
                let access: BTreeMap<Id, Level> =
                    access.iter().map(|a| (a.account, a.level)).collect();
                if access.get(&signer_account) != Some(&Level::Manage)
                    || !access.keys().all(|a| prev.members.contains_key(a))
                {
                    return Err(invalid);
                }
                if next.domains.len() >= MAX_DOMAINS {
                    return Err(PolicyError::DomainLimit { revision });
                }
                next.used_domains.insert(*domain);
                next.domains.insert(
                    *domain,
                    Domain {
                        restricted: true,
                        marker_rid: Some(*marker),
                        access,
                        created_at: revision,
                        creator: body.signer,
                    },
                );
            }
            PolicyKind::SetAccess { domain, access } => {
                let current = prev
                    .domains
                    .get(domain)
                    .filter(|d| d.restricted)
                    .ok_or(invalid)?;
                let access: BTreeMap<Id, Level> =
                    access.iter().map(|a| (a.account, a.level)).collect();
                if !access.keys().all(|a| prev.members.contains_key(a)) {
                    return Err(invalid);
                }
                let only_removes = access
                    .iter()
                    .all(|(account, level)| current.access.get(account) == Some(level));
                let allowed = prev.manages(&signer_account, domain) || (is_admin && only_removes);
                if !allowed {
                    return Err(unauthorized);
                }
                next.domains.get_mut(domain).expect("domain").access = access;
            }
            PolicyKind::DeleteDomain { domain } => {
                if !prev.domains.get(domain).is_some_and(|d| d.restricted) {
                    return Err(invalid);
                }
                if !is_admin && !prev.manages(&signer_account, domain) {
                    return Err(unauthorized);
                }
                next.domains.remove(domain);
            }
            PolicyKind::TransferOwner { account } => {
                if signer_role != Role::Owner {
                    return Err(unauthorized);
                }
                if *account == signer_account || !prev.members.contains_key(account) {
                    return Err(invalid);
                }
                next.members.get_mut(&signer_account).expect("member").role = Role::Admin;
                next.members.get_mut(account).expect("member").role = Role::Owner;
            }
            PolicyKind::AcceptReset { account } => {
                if !is_admin {
                    return Err(unauthorized);
                }
                if !prev.members.contains_key(account) {
                    return Err(invalid);
                }
                drop_grants(&mut next, account);
            }
        }

        // Record the heads.
        for head in body.heads.iter() {
            if let Some(member) = next.members.get_mut(&head.account) {
                if member.head != *head {
                    member.head = head.clone();
                    member.recorded_by_own = false;
                }
                if head.account == signer_account {
                    member.recorded_by_own = true;
                }
            }
        }

        finish_snapshot(&mut next, prev, chains, revision)?;
        Ok(next)
    }

    /// The workspace.
    pub fn workspace(&self) -> Id {
        self.workspace
    }

    /// The accepted head: its revision and digest.
    pub fn head(&self) -> (u64, Digest) {
        let s = self.head_snapshot();
        (s.revision, s.digest)
    }

    /// The state after the latest change.
    pub fn head_snapshot(&self) -> &Snapshot {
        self.snapshots.last().expect("a policy has a genesis")
    }

    /// The state after revision `revision`.
    pub fn at(&self, revision: u64) -> Option<&Snapshot> {
        let index = usize::try_from(revision.checked_sub(1)?).ok()?;
        self.snapshots.get(index)
    }

    /// The change at `revision`.
    pub fn change(&self, revision: u64) -> Option<&Signed<PolicyChange>> {
        let index = usize::try_from(revision.checked_sub(1)?).ok()?;
        self.changes.get(index)
    }

    /// Whether `device` may leave a checkpoint's heads whose previous
    /// checkpoint has `policy_revision` `revision` (§17): its card can't
    /// come back, because a change at or below `revision` recorded a head
    /// of its account at which a link had removed the card, or a head on a
    /// chain other than the card's. The account is still a member there,
    /// yet its recorded head no longer lists the card. A device whose
    /// account `remove_member` removed, or that only lost access, stays.
    pub fn card_gone(&self, device: &Id, revision: u64) -> bool {
        let Some(at) = self.at(revision) else {
            return false;
        };
        if at.is_device(device) {
            return false;
        }
        (1..revision)
            .rev()
            .find_map(|r| self.at(r).and_then(|s| s.account_of(device)))
            .is_some_and(|account| at.members.contains_key(&account))
    }

    /// Whether some change in (`after`, `through`] makes `domain` due for
    /// rotation, and the highest such revision.
    pub fn rotation_required(&self, domain: &Id, after: u64, through: u64) -> Option<u64> {
        ((after + 1)..=through)
            .rev()
            .find(|r| self.at(*r).is_some_and(|s| s.rotations.contains(domain)))
    }

    /// The first revision at which a device of the policy's own account,
    /// active at its kept head, signed a change recording a head of that
    /// account at or after `reference` on the same chain (§8, Verifying the
    /// chain). `own` is the account chain verified up to the head the device
    /// keeps.
    pub fn anchored_by(&self, own: &AccountChain, reference: &AccountHead) -> Option<u64> {
        let kept = own.head_seqno();
        self.changes.iter().find_map(|change| {
            let body = &change.body;
            let records = body.heads.iter().any(|h| {
                h.account == own.account()
                    && h.genesis == reference.genesis
                    && h.genesis == own.genesis()
                    && h.seqno >= reference.seqno
                    && own.contains(h)
            });
            (records && own.is_active_at(&body.signer, kept)).then_some(body.revision)
        })
    }
}

/// Where a recovery with a kit stands, for the workspaces the kit doesn't
/// list (§12).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RecoveryStage {
    /// Step 3: the device verifies each such workspace against the
    /// confirmed head and shows it to you.
    Review,
    /// From step 4's `recover_device` until step 5's `remove` links commit.
    Removing,
    /// After the flow, or a device that isn't recovering.
    Done,
}

/// A change that a removed card signed, which stays in force (§7, §30),
/// with the undo the viewer's role allows now.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemovedCardChange {
    /// The change's revision.
    pub revision: u64,
    /// The removed device that signed it.
    pub signer: Id,
    /// The change's kind, as §8 names it.
    pub kind: &'static str,
    /// The undo the viewer's role allows, as §8 names that change, if any.
    pub undo: Option<&'static str>,
}

/// The undo of `kind` that `role` allows (§8, Roles and authority):
/// `target_role` is the role a member that `kind` names holds now, and
/// `manages` whether the viewer manages the domain a `set_access` or
/// `create_domain` names.
fn undo_for(
    kind: &PolicyKind,
    role: Option<Role>,
    target_role: Option<Role>,
    manages: bool,
) -> Option<&'static str> {
    let admin = matches!(role, Some(Role::Owner | Role::Admin));
    // An owner changes any member but the owner; an admin, editors and
    // viewers.
    let reaches = match role {
        Some(Role::Owner) => target_role.is_some() && target_role != Some(Role::Owner),
        Some(Role::Admin) => matches!(target_role, Some(Role::Editor | Role::Viewer)),
        _ => false,
    };
    match kind {
        PolicyKind::AddMember { .. } | PolicyKind::AcceptReset { .. } => {
            reaches.then_some("remove_member")
        }
        PolicyKind::SetRole { .. } => reaches.then_some("set_role"),
        PolicyKind::SetAccess { .. } => manages.then_some("set_access"),
        PolicyKind::CreateDomain { .. } => (admin || manages).then_some("delete_domain"),
        _ => None,
    }
}

impl PolicyChain {
    /// The changes other than `account_heads` that a card in `cards`
    /// signed, each with the undo that `viewer`'s role allows at the head
    /// (§7, Removing a device).
    pub fn removed_card_changes(
        &self,
        viewer: &Id,
        cards: &BTreeSet<Id>,
    ) -> Vec<RemovedCardChange> {
        let snapshot = self.head_snapshot();
        let role = snapshot.role(viewer);
        self.changes
            .iter()
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
                    | PolicyKind::CreateDomain { domain, .. } => snapshot.manages(viewer, domain),
                    _ => false,
                };
                RemovedCardChange {
                    revision: change.body.revision,
                    signer: change.body.signer,
                    kind: kind.name(),
                    undo: undo_for(kind, role, target_role, manages),
                }
            })
            .collect()
    }

    /// The revision at which a recovering device trusts this log, which
    /// its kit doesn't list (§12): in step 3 and after the flow, as
    /// [`PolicyChain::anchored_by`] with the kit's `account_head` as the
    /// reference and `own` at the confirmed head or the head the device
    /// keeps; between step 4 and the commit of step 5's links, never, so
    /// the device reports `sync_workspace_unverified`.
    pub fn recovery_anchor(
        &self,
        own: &AccountChain,
        kit_head: &AccountHead,
        stage: RecoveryStage,
    ) -> Option<u64> {
        match stage {
            RecoveryStage::Removing => None,
            RecoveryStage::Review | RecoveryStage::Done => self.anchored_by(own, kit_head),
        }
    }

    /// The revisions after `trusted_after` whose changes the device list
    /// alerts on (§7, Device list): every change other than
    /// `account_heads` that a device of `own`'s account other than
    /// `this_device` signed. `trusted_after` is the revision of the
    /// approval anchor or kit the device first trusted the log at, 1 for a
    /// log whose genesis it signed, and 0 for a log it first trusted
    /// through another device's change, whose earlier changes it alerts on
    /// too.
    pub fn alerted_changes(
        &self,
        own: &AccountChain,
        this_device: &Id,
        trusted_after: u64,
    ) -> Vec<u64> {
        self.changes
            .iter()
            .filter(|change| change.body.revision > trusted_after)
            .filter(|change| {
                let signer = &change.body.signer;
                signer != this_device
                    && own.card(signer).is_some()
                    && !matches!(change.body.kind, PolicyKind::AccountHeads)
            })
            .map(|change| change.body.revision)
            .collect()
    }
}

impl PolicyChain {
    /// Whether this verified log names `account`'s chain under `genesis` at
    /// a place where a chain the device doesn't keep may enter (§7): in the
    /// genesis, in an `add_member` that adds the account, or in an
    /// `accept_reset` that accepts it. An `account_heads` change never
    /// brings a new chain in.
    pub fn names_genesis(&self, account: &Id, genesis: &Digest) -> bool {
        self.changes.iter().any(|change| {
            let body = &change.body;
            let anchors = match &body.kind {
                PolicyKind::Genesis { .. } => true,
                PolicyKind::AddMember { account: added, .. }
                | PolicyKind::AcceptReset { account: added } => added == account,
                _ => false,
            };
            anchors
                && body
                    .heads
                    .iter()
                    .any(|h| h.account == *account && h.genesis == *genesis)
        })
    }

    /// Every account head the changes up to `revision` record, in order.
    pub fn recorded_heads(&self, revision: u64) -> impl Iterator<Item = &AccountHead> {
        self.changes
            .iter()
            .take_while(move |c| c.body.revision <= revision)
            .flat_map(|c| c.body.heads.iter())
    }

    /// Whether the sessions of `reader_account` read, in full, the current
    /// chain of an account whose approval request names an invitation to
    /// this workspace (§7): whether that account belongs to the workspace.
    pub fn reads_invitees(&self, reader_account: &Id) -> bool {
        self.head_snapshot().role(reader_account).is_some()
    }

    /// The highest seqno of `account`'s chain under `genesis` that a
    /// session reads through this workspace (§7), or none. `reader` holds
    /// the session's device and any recovery identity it proved, of the
    /// account `reader_account`; `head_seqno` is the chain's head.
    ///
    /// The workspace's devices read every chain the policy records in its
    /// heads, up to the latest head recorded on it, and in full the chain
    /// the policy names for a current member, whether it's the account's
    /// current chain or one a reset retired before the workspace accepted
    /// it. A device whose account `remove_member` removed reads the policy
    /// up to that change, and these chains up to the heads recorded by
    /// then.
    pub fn chain_read_bound(
        &self,
        reader: &[Id],
        reader_account: &Id,
        account: &Id,
        genesis: &Digest,
        head_seqno: u64,
    ) -> Option<u64> {
        let head = self.head_snapshot();
        let active = reader.iter().any(|device| head.is_device(device));
        let limit = if active {
            head.revision
        } else {
            (2..=head.revision).rev().find(|&revision| {
                let removes = self.change(revision).is_some_and(|c| {
                    matches!(c.body.kind, PolicyKind::RemoveMember { account: removed } if removed == *reader_account)
                });
                removes
                    && self
                        .at(revision - 1)
                        .is_some_and(|before| reader.iter().any(|d| before.is_device(d)))
            })?
        };
        let recorded = self
            .recorded_heads(limit)
            .filter(|h| h.account == *account && h.genesis == *genesis)
            .map(|h| h.seqno)
            .max()?;
        let named = head
            .members
            .get(account)
            .is_some_and(|member| member.head.genesis == *genesis);
        Some(if active && named {
            head_seqno
        } else {
            recorded
        })
    }
}

fn check_signer(
    change: &Signed<PolicyChange>,
    chain: &AccountChain,
    head: &AccountHead,
) -> Result<(), PolicyError> {
    let revision = change.body.revision;
    let state = chain
        .card(&change.body.signer)
        .filter(|s| s.active_at(head.seqno))
        .ok_or(PolicyError::SignerNotInHeads { revision })?;
    if state.recovery {
        return Err(PolicyError::RecoverySigner { revision });
    }
    change
        .verify(&state.card.signing_public)
        .map_err(|_| PolicyError::Signature { revision })
}

/// The card index after `highest`, or none past 16,777,215 (§8).
pub fn next_card_index(highest: u32) -> Option<u32> {
    (highest < MAX_CARD_INDEX).then_some(highest + 1)
}

fn drop_grants(snapshot: &mut Snapshot, account: &Id) {
    for domain in snapshot.domains.values_mut() {
        domain.access.remove(account);
    }
}

/// Recomputes the devices, card indices, reader limits, rotations and new
/// readers after a change.
fn finish_snapshot(
    next: &mut Snapshot,
    prev: &Snapshot,
    chains: &AccountChains,
    revision: u64,
) -> Result<(), PolicyError> {
    let mut devices: BTreeMap<Id, WorkspaceDevice> = BTreeMap::new();
    // New cards in order of account ID, then of the seqno that added them.
    let mut new_cards: Vec<(Id, u64, Id)> = Vec::new();
    for (account, member) in &next.members {
        let chain = chains
            .chain_of(&member.head)
            .ok_or(PolicyError::UnknownHead { revision })?;
        for state in chain.active_at(member.head.seqno) {
            let device = state.card.device;
            if devices.contains_key(&device) {
                return Err(PolicyError::DeviceUnderTwoAccounts { revision });
            }
            if !next.card_indices.contains_key(&device) {
                new_cards.push((*account, state.added_at, device));
            }
            devices.insert(
                device,
                WorkspaceDevice {
                    account: *account,
                    card: state.card.clone(),
                    recovery: state.recovery,
                },
            );
        }
    }
    new_cards.sort();
    let mut index = next.card_indices.values().copied().max().unwrap_or(0);
    for (_, _, device) in new_cards {
        index = next_card_index(index).ok_or(PolicyError::RecipientLimit { revision })?;
        next.card_indices.insert(device, index);
    }
    next.devices = devices;

    for domain in next.domains.keys() {
        let after = next.readers(domain);
        if after.len() > MAX_READERS {
            return Err(PolicyError::RecipientLimit { revision });
        }
        if prev.domains.contains_key(domain) {
            let before = prev.readers(domain);
            if before.difference(&after).next().is_some() {
                next.rotations.insert(*domain);
            }
            let added: BTreeSet<Id> = after.difference(&before).copied().collect();
            if !added.is_empty() {
                next.new_readers.insert(*domain, added);
            }
        } else {
            next.new_readers.insert(*domain, after);
        }
    }
    Ok(())
}
