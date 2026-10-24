//! The account chain heads a device keeps (docs/workspace-format/sync-v2.md
//! §7, §15): for each chain of its own account and of every account it
//! shares a workspace with, the genesis digest, the seqno and the head
//! digest of the latest head it accepted, and the digest of each link up
//! to it that it verified. The heads live in the app's application data,
//! under a BLAKE3 keyed hash with the journal key that the credential
//! store keeps, and the credential store keeps the device's own-account
//! head too, so a rolled-back application data folder can't make the
//! device accept a shorter chain of its own account.
//!
//! A served chain under a kept genesis must extend the kept head; one that
//! doesn't reports `sync_account_forked`, and a later link that breaks a
//! rule reports `sync_account_unauthorized`. Either stops every workspace
//! the device shares with that account, or every workspace when the
//! account is its own. The one exception is a chain that the read bounds
//! of §7 let an honest server stop at a head a workspace records, which
//! [`KeptHeads::accept_short`] takes when its links are the ones the
//! device kept. A chain under a genesis the device doesn't keep enters
//! only through an anchor: a verified workspace log that names it in its
//! genesis, an `add_member` or an `accept_reset`, or, for the device's own
//! account, the head it verified its chain from. Refusals live in the
//! credential store beside the own-account head, so neither a restart nor
//! a rebuild after a rolled-back folder lifts a stop until you act on its
//! alert.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::{Path, PathBuf};

use local_core::sync::SyncCredentials;
use serde::{Deserialize, Serialize};
use sync_protocol::account::{AccountChain, ChainRefusal, KeptHead, Stops};
use sync_protocol::crypto::keyed_hash;
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{AccountHead, AccountLink, Signed};
use sync_protocol::types::{Digest, Id, SecretKey, from_hex, to_hex};

use super::SyncError;

/// The kept heads' file in the device's application data folder.
pub const KEPT_HEADS_FILE: &str = "account-heads.json";

fn journal_key_name(device: &Id) -> String {
    format!("noura.sync.v2.journal-key.{}", device.to_hex())
}

fn own_head_name(device: &Id) -> String {
    format!("noura.sync.v2.own-account-head.{}", device.to_hex())
}

fn refusals_name(device: &Id) -> String {
    format!("noura.sync.v2.account-refusals.{}", device.to_hex())
}

fn credential_error(error: impl std::fmt::Display) -> SyncError {
    SyncError::Invalid(format!("the credential store failed: {error}"))
}

fn rolled_back() -> SyncError {
    SyncError::Check("sync_journal_rollback")
}

/// What lets a chain under a genesis the device doesn't keep enter (§7).
#[derive(Clone, Copy, Debug)]
pub enum Anchor<'a> {
    /// A verified workspace log that names the chain in its genesis, an
    /// `add_member` or an `accept_reset`.
    Workspace(&'a PolicyChain),
    /// For the device's own account: the head it verified its chain from,
    /// which the chain must reach: the genesis it signed, the approver's
    /// head of the offer that brought it in, or its kit's account head.
    Own(&'a AccountHead),
}

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Stored {
    heads: Vec<AccountHead>,
    /// The digests of the links the device verified on each kept chain.
    #[serde(default)]
    links: Vec<KeptLinks>,
    /// The chain each workspace's policy names for each member account.
    names: Vec<Named>,
    /// The policy heads that an approval offer or a recovery kit anchored
    /// for each workspace (§10, §12).
    #[serde(default)]
    anchors: Vec<WorkspaceAnchor>,
    /// The revision after which the device alerts you to each policy
    /// change another device of your account signed, per workspace (§7).
    #[serde(default)]
    trusted: Vec<Trusted>,
    /// The digests of the own account's links this device signed, or
    /// whose approval it took part in (§7, Device list).
    #[serde(default)]
    witnessed: Vec<Digest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct WorkspaceAnchor {
    workspace: Id,
    revision: u64,
    head: Digest,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Trusted {
    workspace: Id,
    after: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct KeptLinks {
    account: Id,
    genesis: Digest,
    /// Link digests by seqno: every link up to the kept head once the
    /// device verified the chain, or only the heads that workspaces
    /// recorded after a rebuild.
    digests: BTreeMap<u64, Digest>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Refused {
    account: Id,
    code: String,
}

/// The refusal codes a stop keeps, as the static codes errors carry.
fn refusal_code(code: &str) -> Option<&'static str> {
    ["sync_account_forked", "sync_account_unauthorized"]
        .into_iter()
        .find(|known| *known == code)
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
struct Named {
    workspace: Id,
    account: Id,
    genesis: Digest,
}

/// The heads a device keeps, and the accounts whose chains it refused.
pub struct KeptHeads {
    path: PathBuf,
    device: Id,
    own: Id,
    key: SecretKey,
    heads: BTreeMap<(Id, Digest), KeptHead>,
    links: BTreeMap<(Id, Digest), BTreeMap<u64, Digest>>,
    names: BTreeMap<(Id, Id), Digest>,
    refused: BTreeMap<Id, &'static str>,
    anchors: BTreeMap<Id, (u64, Digest)>,
    trusted: BTreeMap<Id, u64>,
    witnessed: BTreeSet<Digest>,
}

impl std::fmt::Debug for KeptHeads {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeptHeads")
            .field("path", &self.path)
            .field("heads", &self.heads)
            .finish_non_exhaustive()
    }
}

fn stored_own_head(
    credentials: &dyn SyncCredentials,
    device: &Id,
) -> Result<Option<AccountHead>, SyncError> {
    let Some(text) = credentials
        .read_optional(&own_head_name(device))
        .map_err(credential_error)?
    else {
        return Ok(None);
    };
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|_| SyncError::Invalid("the stored own-account head doesn't parse".into()))
}

/// The refusals the credential store keeps, as the own-account head beside
/// them: a list that doesn't parse, or that names an unknown code, fails.
fn stored_refusals(
    credentials: &dyn SyncCredentials,
    device: &Id,
) -> Result<BTreeMap<Id, &'static str>, SyncError> {
    let Some(text) = credentials
        .read_optional(&refusals_name(device))
        .map_err(credential_error)?
    else {
        return Ok(BTreeMap::new());
    };
    let unreadable = || SyncError::Invalid("the stored account refusals don't parse".into());
    let list: Vec<Refused> = serde_json::from_str(&text).map_err(|_| unreadable())?;
    list.iter()
        .map(|r| {
            refusal_code(&r.code)
                .map(|code| (r.account, code))
                .ok_or_else(unreadable)
        })
        .collect()
}

fn write_refusals(
    credentials: &dyn SyncCredentials,
    device: &Id,
    refused: &BTreeMap<Id, &'static str>,
) -> Result<(), SyncError> {
    let list: Vec<Refused> = refused
        .iter()
        .map(|(account, code)| Refused {
            account: *account,
            code: (*code).to_owned(),
        })
        .collect();
    let text = serde_json::to_string(&list).map_err(credential_error)?;
    credentials
        .write(&refusals_name(device), &text)
        .map_err(credential_error)
}

fn journal_key(credentials: &dyn SyncCredentials, device: &Id) -> Result<SecretKey, SyncError> {
    let name = journal_key_name(device);
    if let Some(text) = credentials.read_optional(&name).map_err(credential_error)? {
        let bytes: [u8; 32] = from_hex(&text)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| SyncError::Invalid("the journal key doesn't parse".into()))?;
        return Ok(SecretKey(bytes));
    }
    let key = SecretKey::random().map_err(|e| SyncError::Invalid(format!("no randomness: {e}")))?;
    credentials
        .write(&name, &to_hex(key.expose()))
        .map_err(credential_error)?;
    Ok(key)
}

impl KeptHeads {
    /// Opens the heads `device` of account `own` keeps in `dir`. A file
    /// whose keyed hash fails, or whose own-account head lies below the one
    /// the credential store keeps, reports `sync_journal_rollback`; the
    /// device then rebuilds its heads with [`KeptHeads::rebuild`].
    pub fn open(
        dir: &Path,
        own: Id,
        device: Id,
        credentials: &dyn SyncCredentials,
    ) -> Result<Self, SyncError> {
        let key = journal_key(credentials, &device)?;
        let path = dir.join(KEPT_HEADS_FILE);
        let stored = match std::fs::read(&path) {
            Ok(bytes) => parse(&key, &bytes).ok_or_else(rolled_back)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Stored::default(),
            Err(error) => {
                return Err(SyncError::Invalid(format!(
                    "the kept heads can't be read: {error}"
                )));
            }
        };
        let heads = Self {
            path,
            device,
            own,
            key,
            heads: stored
                .heads
                .iter()
                .map(|h| ((h.account, h.genesis), KeptHead::from(h.clone())))
                .collect(),
            links: stored
                .links
                .into_iter()
                .map(|l| ((l.account, l.genesis), l.digests))
                .collect(),
            names: stored
                .names
                .into_iter()
                .map(|n| ((n.workspace, n.account), n.genesis))
                .collect(),
            refused: stored_refusals(credentials, &device)?,
            anchors: stored
                .anchors
                .into_iter()
                .map(|a| (a.workspace, (a.revision, a.head)))
                .collect(),
            trusted: stored
                .trusted
                .into_iter()
                .map(|t| (t.workspace, t.after))
                .collect(),
            witnessed: stored.witnessed.into_iter().collect(),
        };
        if let Some(floor) = stored_own_head(credentials, &device)? {
            let below = heads
                .kept(&floor.account, &floor.genesis)
                .is_none_or(|kept| kept.seqno < floor.seqno);
            if below {
                return Err(rolled_back());
            }
        }
        Ok(heads)
    }

    /// Rebuilds the heads after `sync_journal_rollback` from the verified
    /// logs of the device's workspaces: every head each records, the chain
    /// each names for each member, and the own-account head the credential
    /// store keeps, which the rebuilt heads never fall below.
    pub fn rebuild_from<'a>(
        dir: &Path,
        own: Id,
        device: Id,
        policies: impl IntoIterator<Item = &'a PolicyChain>,
        credentials: &dyn SyncCredentials,
    ) -> Result<Self, SyncError> {
        let mut recorded = Vec::new();
        let mut names = Vec::new();
        for policy in policies {
            recorded.extend(policy.recorded_heads(policy.head().0).cloned());
            for (account, member) in &policy.head_snapshot().members {
                names.push((policy.workspace(), *account, member.head.genesis));
            }
        }
        Self::rebuild(dir, own, device, recorded, names, credentials)
    }

    /// Rebuilds the heads from `recorded` heads and the chain `names` that
    /// workspaces' policies give their members, as (workspace, account,
    /// genesis), and the own-account head the credential store keeps,
    /// which the rebuilt heads never fall below. The device keeps each
    /// recorded head's digest as a link of its chain, and the refusals in
    /// the credential store stay. Two recorded heads at one seqno of one
    /// chain with different digests refuse that account's chain.
    pub fn rebuild(
        dir: &Path,
        own: Id,
        device: Id,
        recorded: impl IntoIterator<Item = AccountHead>,
        names: impl IntoIterator<Item = (Id, Id, Digest)>,
        credentials: &dyn SyncCredentials,
    ) -> Result<Self, SyncError> {
        let key = journal_key(credentials, &device)?;
        let mut heads: BTreeMap<(Id, Digest), KeptHead> = BTreeMap::new();
        let mut links: BTreeMap<(Id, Digest), BTreeMap<u64, Digest>> = BTreeMap::new();
        let mut forked = BTreeSet::new();
        let floor = stored_own_head(credentials, &device)?;
        for head in recorded.into_iter().chain(floor) {
            let chain = (head.account, head.genesis);
            let digests = links.entry(chain).or_default();
            if *digests.entry(head.seqno).or_insert(head.head) != head.head {
                forked.insert(head.account);
            }
            let entry = heads
                .entry(chain)
                .or_insert_with(|| KeptHead::from(head.clone()));
            if head.seqno > entry.seqno {
                *entry = KeptHead::from(head);
            }
        }
        let mut rebuilt = Self {
            path: dir.join(KEPT_HEADS_FILE),
            device,
            own,
            key,
            heads,
            links,
            names: names
                .into_iter()
                .map(|(workspace, account, genesis)| ((workspace, account), genesis))
                .collect(),
            refused: stored_refusals(credentials, &device)?,
            anchors: BTreeMap::new(),
            trusted: BTreeMap::new(),
            witnessed: BTreeSet::new(),
        };
        rebuilt.save()?;
        for account in forked {
            rebuilt.refuse(account, ChainRefusal::Forked, credentials);
        }
        Ok(rebuilt)
    }

    /// The head kept of `account`'s chain under `genesis`.
    pub fn kept(&self, account: &Id, genesis: &Digest) -> Option<KeptHead> {
        self.heads.get(&(*account, *genesis)).copied()
    }

    /// Every head kept of `account`'s chains.
    pub fn kept_of(&self, account: &Id) -> Vec<AccountHead> {
        self.heads
            .iter()
            .filter(|((a, _), _)| a == account)
            .map(|((account, genesis), kept)| AccountHead {
                account: *account,
                genesis: *genesis,
                seqno: kept.seqno,
                head: kept.head,
            })
            .collect()
    }

    /// The device's own account.
    pub fn own_account(&self) -> Id {
        self.own
    }

    /// The policy head that an approval offer or a recovery kit anchored for
    /// `workspace`: its revision and digest (§10, §12).
    pub fn anchor(&self, workspace: &Id) -> Option<(u64, Digest)> {
        self.anchors.get(workspace).copied()
    }

    /// Keeps the policy head that an approval offer or a recovery kit
    /// anchors for `workspace`; the device then trusts the log from its
    /// genesis through that head (§8).
    pub fn set_anchor(
        &mut self,
        workspace: Id,
        revision: u64,
        head: Digest,
    ) -> Result<(), SyncError> {
        self.anchors.insert(workspace, (revision, head));
        self.save()
    }

    /// The revision after which the device alerts you to each policy change
    /// that another device of your account signed in `workspace`, once it
    /// trusts the workspace (§7).
    pub fn trusted_after(&self, workspace: &Id) -> Option<u64> {
        self.trusted.get(workspace).copied()
    }

    /// Notes when the device first trusted `workspace`: after `revision`,
    /// the head of an anchor, a kit or the genesis it created, or 0 when it
    /// first trusted the log through another device's change, so it alerts
    /// you to that log's earlier changes too (§7). A later call changes
    /// nothing.
    pub fn first_trusted(&mut self, workspace: Id, revision: u64) -> Result<(), SyncError> {
        if self.trusted.contains_key(&workspace) {
            return Ok(());
        }
        self.trusted.insert(workspace, revision);
        self.save()
    }

    /// Notes a link of the own account's chain that this device signed, or
    /// the `add_device` of an approval it took part in, so the device list
    /// raises no alert for it (§7).
    pub fn witness(&mut self, link: Digest) -> Result<(), SyncError> {
        if self.witnessed.insert(link) {
            self.save()?;
        }
        Ok(())
    }

    /// Whether this device signed `link` or took part in its approval.
    pub fn witnessed(&self, link: &Digest) -> bool {
        self.witnessed.contains(link)
    }

    /// Judges the chain the server serves for `account` under `genesis`,
    /// which the device keeps, from its genesis, and keeps its new head.
    /// Links of another account or under another genesis don't extend the
    /// kept head. `listed_elsewhere` holds the device IDs that the
    /// account's other chains list. A refusal stops the account's
    /// workspaces, as [`KeptHeads::stopped`] reports.
    pub fn accept(
        &mut self,
        account: Id,
        genesis: Digest,
        served: &[Signed<AccountLink>],
        listed_elsewhere: &BTreeSet<Id>,
        credentials: &dyn SyncCredentials,
    ) -> Result<AccountChain, SyncError> {
        let kept = self
            .kept(&account, &genesis)
            .ok_or(SyncError::Check("sync_account_unauthorized"))?;
        let requested = served
            .first()
            .is_some_and(|first| first.body.account == account && first.digest() == genesis);
        if !requested {
            return Err(self.refuse(account, ChainRefusal::Forked, credentials));
        }
        match kept.judge(served, listed_elsewhere) {
            Ok(chain) if self.agrees(&chain) => {
                self.keep(&chain, credentials)?;
                Ok(chain)
            }
            Ok(_) => Err(self.refuse(account, ChainRefusal::Forked, credentials)),
            Err(refusal) => Err(self.refuse(account, refusal, credentials)),
        }
    }

    /// Takes a chain of another account that the server serves only up to
    /// a head a workspace records, below the head the device keeps of it
    /// (§7). An honest server stops such a chain there when the workspace
    /// no longer names it for a current member or doesn't count the
    /// device among its devices, such as after `remove_member`. The served
    /// links must be the ones the device verified: each link digest it
    /// kept up to there must match, and the last link's digest must be
    /// one it kept. The chain must also reach `reach`, the highest head
    /// the workspace's log records on it. The device keeps its higher head
    /// and judges no link past the served ones. Anything else reports
    /// `sync_account_forked`. Whether the workspace's verified policy lets
    /// the server stop the chain short is the caller's to check, with
    /// [`KeptHeads::refuse_short`] when it doesn't.
    pub fn accept_short(
        &mut self,
        account: Id,
        genesis: Digest,
        served: &[Signed<AccountLink>],
        listed_elsewhere: &BTreeSet<Id>,
        reach: u64,
        credentials: &dyn SyncCredentials,
    ) -> Result<AccountChain, SyncError> {
        let kept = self
            .kept(&account, &genesis)
            .ok_or(SyncError::Check("sync_account_unauthorized"))?;
        let short = served.len() as u64;
        let taken = account != self.own
            && short >= reach.max(1)
            && short < kept.seqno
            && served
                .first()
                .is_some_and(|first| first.body.account == account && first.digest() == genesis);
        let chain = taken
            .then(|| AccountChain::verify(served, listed_elsewhere).ok())
            .flatten()
            .filter(|chain| {
                chain.genesis() == genesis
                    && self
                        .links
                        .get(&(account, genesis))
                        .and_then(|digests| digests.get(&short))
                        == Some(&chain.head_digest())
                    && self.agrees(chain)
            });
        chain.ok_or_else(|| self.refuse(account, ChainRefusal::Forked, credentials))
    }

    /// Refuses `account`'s chain, which [`KeptHeads::accept_short`] took,
    /// once the workspace's verified policy shows the server had to serve
    /// it in full: the policy names it for a current member and counts the
    /// device among the workspace's devices. That chain was cut back below
    /// the kept head: `sync_account_forked`.
    pub fn refuse_short(&mut self, account: Id, credentials: &dyn SyncCredentials) -> SyncError {
        self.refuse(account, ChainRefusal::Forked, credentials)
    }

    /// Whether every link digest the device kept of `chain`'s chain, up to
    /// its head, is that chain's.
    fn agrees(&self, chain: &AccountChain) -> bool {
        let Some(digests) = self.links.get(&(chain.account(), chain.genesis())) else {
            return true;
        };
        let held = chain.link_digests();
        digests
            .range(1..=chain.head_seqno())
            .all(|(seqno, digest)| held.get(*seqno as usize - 1) == Some(digest))
    }

    /// Lets a chain under a genesis the device doesn't keep enter through
    /// `anchor`, and keeps its head. Any other new chain reports
    /// `sync_account_unauthorized`.
    pub fn enter(
        &mut self,
        chain: &AccountChain,
        anchor: Anchor<'_>,
        credentials: &dyn SyncCredentials,
    ) -> Result<(), SyncError> {
        let account = chain.account();
        let anchored = match anchor {
            Anchor::Workspace(policy) => policy.names_genesis(&account, &chain.genesis()),
            Anchor::Own(reference) => account == self.own && chain.contains(reference),
        };
        if !anchored {
            return Err(SyncError::Check("sync_account_unauthorized"));
        }
        if let Some(kept) = self.kept(&account, &chain.genesis()) {
            // Already kept: the chain must extend that head.
            if kept.check(chain).is_err() || !self.agrees(chain) {
                return Err(self.refuse(account, ChainRefusal::Forked, credentials));
            }
        }
        self.keep(chain, credentials)
    }

    /// Checks a head that a workspace records under a genesis the device
    /// keeps: it must lie on the kept chain, which `chain` holds verified,
    /// either through the kept head or, for a chain that
    /// [`KeptHeads::accept_short`] took, with the links the device kept.
    pub fn check_recorded(
        &mut self,
        chain: &AccountChain,
        recorded: &AccountHead,
        credentials: &dyn SyncCredentials,
    ) -> Result<(), SyncError> {
        let Some(kept) = self.kept(&recorded.account, &recorded.genesis) else {
            return Ok(());
        };
        let on_chain = if chain.genesis() == recorded.genesis && chain.head_seqno() < kept.seqno {
            self.agrees(chain) && chain.contains(recorded)
        } else {
            kept.check_recorded(chain, recorded).is_ok()
        };
        if on_chain {
            Ok(())
        } else {
            Err(self.refuse(recorded.account, ChainRefusal::Forked, credentials))
        }
    }

    /// Records the chain a workspace's policy names for a member account.
    pub fn name(&mut self, workspace: Id, account: Id, genesis: Digest) -> Result<(), SyncError> {
        if self.names.insert((workspace, account), genesis) != Some(genesis) {
            self.save()?;
        }
        Ok(())
    }

    /// Records the chains a workspace's verified policy names for its
    /// members, as (account, genesis), in place of what the device noted
    /// there before, so an account the workspace removed raises no alert.
    pub fn name_members(
        &mut self,
        workspace: Id,
        members: impl IntoIterator<Item = (Id, Digest)>,
    ) -> Result<(), SyncError> {
        let before: Vec<((Id, Id), Digest)> = self
            .names
            .range((workspace, Id([0; 16]))..=(workspace, Id([0xff; 16])))
            .map(|(key, genesis)| (*key, *genesis))
            .collect();
        let after: Vec<((Id, Id), Digest)> = members
            .into_iter()
            .map(|(account, genesis)| ((workspace, account), genesis))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .collect();
        if before == after {
            return Ok(());
        }
        for (key, _) in before {
            self.names.remove(&key);
        }
        self.names.extend(after);
        self.save()
    }

    /// Forgets the chains a workspace named, once the device left it.
    pub fn forget_workspace(&mut self, workspace: Id) -> Result<(), SyncError> {
        self.name_members(workspace, [])
    }

    /// The accounts of which the device keeps two current chains, because
    /// two workspaces name different ones; the app alerts you to each.
    pub fn two_current_chains(&self) -> Vec<Id> {
        let mut by_account: BTreeMap<Id, BTreeSet<Digest>> = BTreeMap::new();
        for ((_, account), genesis) in &self.names {
            by_account.entry(*account).or_default().insert(*genesis);
        }
        by_account
            .into_iter()
            .filter(|(_, geneses)| geneses.len() > 1)
            .map(|(account, _)| account)
            .collect()
    }

    /// The code that stops a workspace whose members are `members`: the
    /// refusal of the device's own chain stops every workspace, and that of
    /// a member's chain stops the workspaces shared with that member.
    pub fn stopped(&self, members: impl IntoIterator<Item = Id>) -> Option<&'static str> {
        if let Some(code) = self.refused.get(&self.own) {
            return Some(code);
        }
        members
            .into_iter()
            .find_map(|account| self.refused.get(&account).copied())
    }

    /// Lifts the stop that a refusal of `account`'s chain set, once you
    /// acted on its alert, such as by having the server repaired or the
    /// account removed and invited again. The app's alert calls it.
    pub fn clear_refusal(
        &mut self,
        account: &Id,
        credentials: &dyn SyncCredentials,
    ) -> Result<(), SyncError> {
        if self.refused.remove(account).is_some() {
            write_refusals(credentials, &self.device, &self.refused)?;
        }
        Ok(())
    }

    fn refuse(
        &mut self,
        account: Id,
        refusal: ChainRefusal,
        credentials: &dyn SyncCredentials,
    ) -> SyncError {
        let code = refusal.client_code();
        // Both refusals stop the same workspaces (§7).
        let _stops: Stops = refusal.stops(account == self.own);
        if let std::collections::btree_map::Entry::Vacant(entry) = self.refused.entry(account) {
            entry.insert(code);
            // A refusal that can't be written still stops the running
            // service, and the device judges the same chain again later.
            let _ = write_refusals(credentials, &self.device, &self.refused);
        }
        SyncError::Check(code)
    }

    /// Keeps `chain`'s head and its link digests, which the device
    /// verified, writing the heads durably before the credential store
    /// takes a new own-account head, so a crash between the two leaves the
    /// file ahead of the floor.
    fn keep(
        &mut self,
        chain: &AccountChain,
        credentials: &dyn SyncCredentials,
    ) -> Result<(), SyncError> {
        let head = chain.head();
        let key = (head.account, head.genesis);
        let digests = self.links.entry(key).or_default();
        let known = digests.len();
        digests.extend(
            chain
                .link_digests()
                .iter()
                .enumerate()
                .map(|(index, digest)| (index as u64 + 1, *digest)),
        );
        let more_links = digests.len() > known;
        if self
            .heads
            .get(&key)
            .is_some_and(|kept| kept.seqno >= head.seqno)
        {
            return if more_links { self.save() } else { Ok(()) };
        }
        self.heads.insert(key, KeptHead::from(head.clone()));
        self.save()?;
        if head.account == self.own {
            let text = serde_json::to_string(&head).map_err(credential_error)?;
            credentials
                .write(&own_head_name(&self.device), &text)
                .map_err(credential_error)?;
        }
        Ok(())
    }

    fn save(&self) -> Result<(), SyncError> {
        let stored = Stored {
            heads: self
                .heads
                .iter()
                .map(|((account, genesis), kept)| AccountHead {
                    account: *account,
                    genesis: *genesis,
                    seqno: kept.seqno,
                    head: kept.head,
                })
                .collect(),
            links: self
                .links
                .iter()
                .map(|((account, genesis), digests)| KeptLinks {
                    account: *account,
                    genesis: *genesis,
                    digests: digests.clone(),
                })
                .collect(),
            names: self
                .names
                .iter()
                .map(|((workspace, account), genesis)| Named {
                    workspace: *workspace,
                    account: *account,
                    genesis: *genesis,
                })
                .collect(),
            anchors: self
                .anchors
                .iter()
                .map(|(workspace, (revision, head))| WorkspaceAnchor {
                    workspace: *workspace,
                    revision: *revision,
                    head: *head,
                })
                .collect(),
            trusted: self
                .trusted
                .iter()
                .map(|(workspace, after)| Trusted {
                    workspace: *workspace,
                    after: *after,
                })
                .collect(),
            witnessed: self.witnessed.iter().copied().collect(),
        };
        let json =
            serde_json::to_vec_pretty(&stored).map_err(|e| SyncError::Invalid(e.to_string()))?;
        let mut bytes = keyed_hash(&self.key, &json).to_hex().into_bytes();
        bytes.push(b'\n');
        bytes.extend_from_slice(&json);
        let write = || -> std::io::Result<()> {
            if let Some(parent) = self.path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut file = atomic_write_file::AtomicWriteFile::open(&self.path)?;
            file.write_all(&bytes)?;
            file.commit()
        };
        write().map_err(|e| SyncError::Invalid(format!("the kept heads can't be written: {e}")))
    }
}

/// The heads in a file whose keyed hash verifies under `key`.
fn parse(key: &SecretKey, bytes: &[u8]) -> Option<Stored> {
    let split = bytes.iter().position(|b| *b == b'\n')?;
    let (mac, json) = (&bytes[..split], &bytes[split + 1..]);
    let mac = Digest::from_hex(std::str::from_utf8(mac).ok()?)?;
    if keyed_hash(key, json) != mac {
        return None;
    }
    serde_json::from_slice(json).ok()
}
