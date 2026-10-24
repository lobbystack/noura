//! Account chain verification (§7), from the links alone.
//!
//! An account chain is a linear hash chain of signed links that holds the
//! account's device cards and its recovery identity. [`AccountChain`]
//! applies every rule of §7 as it takes links in order, and answers which
//! cards are active at any head. The server and every client run the same
//! checks.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::records::{AccountHead, AccountLink, Card, LinkKind, MAX_ACTIVE_CARDS, Signed};
use crate::types::{Digest, Id};

/// A card's history in its account chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CardState {
    /// The card.
    pub card: Card,
    /// The signer of the link that added it; none for the genesis card.
    pub parent: Option<Id>,
    /// The seqno of the link that added it.
    pub added_at: u64,
    /// The seqno of the link that removed it, if any.
    pub removed_at: Option<u64>,
    /// Whether an `add_recovery` added it, making it a recovery identity.
    pub recovery: bool,
    /// For a recovery identity, the seqno of the `recover_device` it signed.
    pub recovered_at: Option<u64>,
}

impl CardState {
    /// Whether the card is active at the head at `seqno`.
    pub fn active_at(&self, seqno: u64) -> bool {
        self.added_at <= seqno && self.removed_at.is_none_or(|removed| removed > seqno)
    }
}

/// A link that breaks a rule of §7. Each names the seqno of the failing
/// link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, thiserror::Error, Serialize, Deserialize)]
#[serde(tag = "error", rename_all = "snake_case")]
pub enum AccountError {
    /// The seqno isn't one above the head's, or a chain starts above 1.
    #[error("link {seqno}: wrong seqno")]
    Seqno {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// `previous` isn't the head's digest, or isn't zero at seqno 1.
    #[error("link {seqno}: previous isn't the head")]
    Previous {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// The link names another account than its chain.
    #[error("link {seqno}: wrong account")]
    Account {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// A genesis away from seqno 1, or another kind at seqno 1.
    #[error("link {seqno}: genesis out of place")]
    GenesisPosition {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// A genesis that its own card didn't sign.
    #[error("link {seqno}: genesis not signed by its card")]
    GenesisSigner {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// The signer isn't a card active at the previous head.
    #[error("link {seqno}: signer isn't active")]
    SignerNotActive {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// The signature doesn't verify under the signer's card.
    #[error("link {seqno}: bad signature")]
    Signature {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// A card that names another account.
    #[error("link {seqno}: card names another account")]
    CardAccount {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// A card whose device ID an earlier link of a chain of the account
    /// listed.
    #[error("link {seqno}: device ID listed before")]
    DeviceListed {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// A recovery identity signed something other than `recover_device`.
    #[error("link {seqno}: a recovery identity signs only recover_device")]
    RecoverySigns {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// A `recover_device` that the active recovery identity didn't sign.
    #[error("link {seqno}: recover_device needs the active recovery identity")]
    RecoveryNotSigner {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// A second `recover_device` from one recovery identity.
    #[error("link {seqno}: the recovery identity already recovered a device")]
    RecoveryUsed {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// An `add_recovery` while a recovery identity is active.
    #[error("link {seqno}: a recovery identity is already active")]
    SecondRecovery {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// The link would leave more than 256 active cards.
    #[error("link {seqno}: more than 256 active cards")]
    TooManyCards {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// A `remove` whose target isn't active.
    #[error("link {seqno}: the removed card isn't active")]
    RemoveTarget {
        /// The failing link's seqno.
        seqno: u64,
    },
    /// A `keep` entry that the link wouldn't otherwise remove, or a recovery
    /// identity.
    #[error("link {seqno}: invalid keep")]
    Keep {
        /// The failing link's seqno.
        seqno: u64,
    },
}

impl AccountError {
    /// The seqno of the failing link.
    pub fn seqno(&self) -> u64 {
        match *self {
            AccountError::Seqno { seqno }
            | AccountError::Previous { seqno }
            | AccountError::Account { seqno }
            | AccountError::GenesisPosition { seqno }
            | AccountError::GenesisSigner { seqno }
            | AccountError::SignerNotActive { seqno }
            | AccountError::Signature { seqno }
            | AccountError::CardAccount { seqno }
            | AccountError::DeviceListed { seqno }
            | AccountError::RecoverySigns { seqno }
            | AccountError::RecoveryNotSigner { seqno }
            | AccountError::RecoveryUsed { seqno }
            | AccountError::SecondRecovery { seqno }
            | AccountError::TooManyCards { seqno }
            | AccountError::RemoveTarget { seqno }
            | AccountError::Keep { seqno } => seqno,
        }
    }

    /// The server's answer to a link failing this way (§7):
    /// `account_conflict` when `previous` isn't the chain's head, else
    /// `forbidden`.
    pub fn server_code(&self) -> &'static str {
        match self {
            AccountError::Seqno { .. } | AccountError::Previous { .. } => "account_conflict",
            AccountError::Signature { .. } => "signature_invalid",
            _ => "forbidden",
        }
    }

    /// The client's report: `sync_account_unauthorized`.
    pub fn client_code(&self) -> &'static str {
        "sync_account_unauthorized"
    }
}

/// A verified account chain.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountChain {
    account: Id,
    digests: Vec<Digest>,
    cards: BTreeMap<Id, CardState>,
}

impl AccountChain {
    /// Verifies a whole chain from its genesis. `listed_elsewhere` holds the
    /// device IDs that other chains of the account listed, such as the
    /// chain a reset retired.
    pub fn verify(
        links: &[Signed<AccountLink>],
        listed_elsewhere: &BTreeSet<Id>,
    ) -> Result<Self, AccountError> {
        let (genesis, rest) = links
            .split_first()
            .ok_or(AccountError::Seqno { seqno: 0 })?;
        let mut chain = Self::from_genesis(genesis, listed_elsewhere)?;
        for link in rest {
            chain.append(link, listed_elsewhere)?;
        }
        Ok(chain)
    }

    /// Starts a chain from its genesis link.
    pub fn from_genesis(
        link: &Signed<AccountLink>,
        listed_elsewhere: &BTreeSet<Id>,
    ) -> Result<Self, AccountError> {
        let body = &link.body;
        let seqno = body.seqno;
        if seqno != 1 {
            return Err(AccountError::Seqno { seqno });
        }
        if !body.previous.is_zero() {
            return Err(AccountError::Previous { seqno });
        }
        let LinkKind::Genesis { card } = &body.kind else {
            return Err(AccountError::GenesisPosition { seqno });
        };
        if card.account != body.account {
            return Err(AccountError::CardAccount { seqno });
        }
        if body.signer != card.device {
            return Err(AccountError::GenesisSigner { seqno });
        }
        link.verify(&card.signing_public)
            .map_err(|_| AccountError::Signature { seqno })?;
        if listed_elsewhere.contains(&card.device) {
            return Err(AccountError::DeviceListed { seqno });
        }
        let mut cards = BTreeMap::new();
        cards.insert(
            card.device,
            CardState {
                card: card.0.clone(),
                parent: None,
                added_at: 1,
                removed_at: None,
                recovery: false,
                recovered_at: None,
            },
        );
        Ok(Self {
            account: body.account,
            digests: vec![link.digest()],
            cards,
        })
    }

    /// Takes the next link, applying every rule of §7.
    pub fn append(
        &mut self,
        link: &Signed<AccountLink>,
        listed_elsewhere: &BTreeSet<Id>,
    ) -> Result<(), AccountError> {
        let body = &link.body;
        let seqno = body.seqno;
        let head = self.head_seqno();
        if seqno != head + 1 {
            return Err(AccountError::Seqno { seqno });
        }
        if body.previous != self.head_digest() {
            return Err(AccountError::Previous { seqno });
        }
        if body.account != self.account {
            return Err(AccountError::Account { seqno });
        }
        if matches!(body.kind, LinkKind::Genesis { .. }) {
            return Err(AccountError::GenesisPosition { seqno });
        }
        let signer = self
            .cards
            .get(&body.signer)
            .filter(|state| state.active_at(head))
            .ok_or(AccountError::SignerNotActive { seqno })?;
        link.verify(&signer.card.signing_public)
            .map_err(|_| AccountError::Signature { seqno })?;
        let signer_is_recovery = signer.recovery;
        let signer_recovered = signer.recovered_at.is_some();
        match (&body.kind, signer_is_recovery) {
            (LinkKind::RecoverDevice { .. }, false) => {
                return Err(AccountError::RecoveryNotSigner { seqno });
            }
            (LinkKind::RecoverDevice { .. }, true) if signer_recovered => {
                return Err(AccountError::RecoveryUsed { seqno });
            }
            (LinkKind::RecoverDevice { .. }, true) => {}
            (_, true) => return Err(AccountError::RecoverySigns { seqno }),
            _ => {}
        }

        if let Some(card) = body.kind.added_card() {
            if card.account != self.account {
                return Err(AccountError::CardAccount { seqno });
            }
            if self.cards.contains_key(&card.device) || listed_elsewhere.contains(&card.device) {
                return Err(AccountError::DeviceListed { seqno });
            }
            let active = self.active_states(head).count();
            if active + 1 > MAX_ACTIVE_CARDS {
                return Err(AccountError::TooManyCards { seqno });
            }
            let recovery = matches!(body.kind, LinkKind::AddRecovery { .. });
            if recovery && self.active_states(head).any(|state| state.recovery) {
                return Err(AccountError::SecondRecovery { seqno });
            }
            if matches!(body.kind, LinkKind::RecoverDevice { .. }) {
                let identity = self.cards.get_mut(&body.signer).expect("signer is a card");
                identity.recovered_at = Some(seqno);
            }
            self.cards.insert(
                card.device,
                CardState {
                    card: card.clone(),
                    parent: Some(body.signer),
                    added_at: seqno,
                    removed_at: None,
                    recovery,
                    recovered_at: None,
                },
            );
        }

        if let LinkKind::Remove { device, keep } = &body.kind {
            let removed = self.removal_set(head, body.signer, *device, keep, seqno)?;
            for id in removed {
                if let Some(state) = self.cards.get_mut(&id) {
                    state.removed_at = Some(seqno);
                }
            }
        }

        self.digests.push(link.digest());
        Ok(())
    }

    /// The cards a `remove` link takes down: the target and, in turn, every
    /// active card whose parent it removes, except the signer and the
    /// devices `keep` names. A link that names its own signer removes that
    /// card alone.
    fn removal_set(
        &self,
        head: u64,
        signer: Id,
        target: Id,
        keep: &[Id],
        seqno: u64,
    ) -> Result<BTreeSet<Id>, AccountError> {
        if !self.cards.get(&target).is_some_and(|s| s.active_at(head)) {
            return Err(AccountError::RemoveTarget { seqno });
        }
        if target == signer {
            if !keep.is_empty() {
                return Err(AccountError::Keep { seqno });
            }
            return Ok(BTreeSet::from([signer]));
        }
        let cascade = |kept: &BTreeSet<Id>| {
            let mut removed = BTreeSet::from([target]);
            loop {
                let more: Vec<Id> = self
                    .active_states(head)
                    .filter(|s| {
                        let id = s.card.device;
                        !removed.contains(&id)
                            && id != signer
                            && !kept.contains(&id)
                            && s.parent.is_some_and(|p| removed.contains(&p))
                    })
                    .map(|s| s.card.device)
                    .collect();
                if more.is_empty() {
                    return removed;
                }
                removed.extend(more);
            }
        };
        let without_keep = cascade(&BTreeSet::new());
        for id in keep {
            let valid = *id != target
                && without_keep.contains(id)
                && self.cards.get(id).is_some_and(|s| !s.recovery);
            if !valid {
                return Err(AccountError::Keep { seqno });
            }
        }
        Ok(cascade(&keep.iter().copied().collect()))
    }

    fn active_states(&self, seqno: u64) -> impl Iterator<Item = &CardState> {
        self.cards
            .values()
            .filter(move |state| state.active_at(seqno))
    }

    /// The account.
    pub fn account(&self) -> Id {
        self.account
    }

    /// The digest of the genesis link, which is also the account's
    /// fingerprint.
    pub fn genesis(&self) -> Digest {
        self.digests[0]
    }

    /// The head's seqno.
    pub fn head_seqno(&self) -> u64 {
        self.digests.len() as u64
    }

    /// The head's digest.
    pub fn head_digest(&self) -> Digest {
        *self.digests.last().expect("a chain has a genesis")
    }

    /// The current head.
    pub fn head(&self) -> AccountHead {
        self.head_at(self.head_seqno()).expect("the head exists")
    }

    /// The head at `seqno`, if the chain reaches it.
    pub fn head_at(&self, seqno: u64) -> Option<AccountHead> {
        let index = usize::try_from(seqno.checked_sub(1)?).ok()?;
        Some(AccountHead {
            account: self.account,
            genesis: self.genesis(),
            seqno,
            head: *self.digests.get(index)?,
        })
    }

    /// Whether `head` lies on this chain.
    pub fn contains(&self, head: &AccountHead) -> bool {
        head.account == self.account && self.head_at(head.seqno).as_ref() == Some(head)
    }

    /// The cards active at the head at `seqno`, ordered by device ID.
    pub fn active_at(&self, seqno: u64) -> Vec<&CardState> {
        self.active_states(seqno).collect()
    }

    /// Whether `device` is active at the head at `seqno`.
    pub fn is_active_at(&self, device: &Id, seqno: u64) -> bool {
        self.cards.get(device).is_some_and(|s| s.active_at(seqno))
    }

    /// A card this chain ever added.
    pub fn card(&self, device: &Id) -> Option<&CardState> {
        self.cards.get(device)
    }

    /// Every card this chain ever added.
    pub fn cards(&self) -> impl Iterator<Item = &CardState> {
        self.cards.values()
    }

    /// Every device ID this chain listed.
    pub fn listed_devices(&self) -> BTreeSet<Id> {
        self.cards.keys().copied().collect()
    }

    /// The chain cut back to the head at `seqno`, as a device that kept that
    /// head saw it.
    pub fn truncated(&self, seqno: u64) -> Option<Self> {
        if seqno == 0 || seqno > self.head_seqno() {
            return None;
        }
        let cards = self
            .cards
            .iter()
            .filter(|(_, s)| s.added_at <= seqno)
            .map(|(id, s)| {
                let mut s = s.clone();
                s.removed_at = s.removed_at.filter(|&r| r <= seqno);
                s.recovered_at = s.recovered_at.filter(|&r| r <= seqno);
                (*id, s)
            })
            .collect();
        Some(Self {
            account: self.account,
            digests: self.digests[..seqno as usize].to_vec(),
            cards,
        })
    }
}

/// The head a device keeps of an account chain (§7): the genesis digest,
/// the seqno and the head digest.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct KeptHead {
    /// The chain's genesis digest.
    pub genesis: Digest,
    /// The kept seqno.
    pub seqno: u64,
    /// The digest of the link at `seqno`.
    pub head: Digest,
}

impl From<AccountHead> for KeptHead {
    fn from(head: AccountHead) -> Self {
        Self {
            genesis: head.genesis,
            seqno: head.seqno,
            head: head.head,
        }
    }
}

/// A chain under a kept genesis that doesn't extend the kept head:
/// `sync_account_forked`, which stops every workspace the device shares
/// with that account.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the account chain doesn't extend the kept head")]
pub struct AccountForked;

impl AccountForked {
    /// The client's report.
    pub fn client_code(&self) -> &'static str {
        "sync_account_forked"
    }
}

impl KeptHead {
    /// Checks that `chain`, under this kept genesis, extends this head. A
    /// chain under another genesis isn't this check's to judge.
    pub fn check(&self, chain: &AccountChain) -> Result<(), AccountForked> {
        if chain.genesis() != self.genesis {
            return Ok(());
        }
        match chain.head_at(self.seqno) {
            Some(head) if head.head == self.head => Ok(()),
            _ => Err(AccountForked),
        }
    }

    /// Checks a head recorded elsewhere, such as in a workspace's policy,
    /// under this kept genesis: it must lie on the kept chain, which `chain`
    /// holds verified.
    pub fn check_recorded(
        &self,
        chain: &AccountChain,
        recorded: &AccountHead,
    ) -> Result<(), AccountForked> {
        self.check(chain)?;
        if recorded.genesis == self.genesis && !chain.contains(recorded) {
            return Err(AccountForked);
        }
        Ok(())
    }

    /// Judges a chain a server serves under this kept genesis, from its
    /// genesis (§7). The links up to the kept seqno must be the ones the
    /// device accepted, ending at the kept head; each later link must have
    /// the seqno one above the head before it and name that head's digest
    /// in `previous`, and follow every rule of §7 under the cards active
    /// there. A chain that stops short of the kept head, or a link whose
    /// `previous` names another digest at any seqno, doesn't extend the
    /// kept head: [`ChainRefusal::Forked`]. A later link that breaks any
    /// other rule: [`ChainRefusal::Unauthorized`]. `listed_elsewhere`
    /// holds the device IDs the account's other chains listed.
    pub fn judge(
        &self,
        served: &[Signed<AccountLink>],
        listed_elsewhere: &BTreeSet<Id>,
    ) -> Result<AccountChain, ChainRefusal> {
        let kept = usize::try_from(self.seqno).map_err(|_| ChainRefusal::Forked)?;
        if kept == 0 || served.len() < kept {
            return Err(ChainRefusal::Forked);
        }
        let (held, later) = served.split_at(kept);
        let chain =
            AccountChain::verify(held, listed_elsewhere).map_err(|_| ChainRefusal::Forked)?;
        if chain.genesis() != self.genesis || chain.head_digest() != self.head {
            return Err(ChainRefusal::Forked);
        }
        self.extend(&chain, later, listed_elsewhere)
    }

    /// Takes the links a server serves after this kept head onto `held`,
    /// the chain the device verified up to it, by the rules of
    /// [`KeptHead::judge`].
    pub fn extend(
        &self,
        held: &AccountChain,
        later: &[Signed<AccountLink>],
        listed_elsewhere: &BTreeSet<Id>,
    ) -> Result<AccountChain, ChainRefusal> {
        if held.genesis() != self.genesis
            || held.head_seqno() != self.seqno
            || held.head_digest() != self.head
        {
            return Err(ChainRefusal::Forked);
        }
        let mut chain = held.clone();
        for link in later {
            if link.body.previous != chain.head_digest() {
                return Err(ChainRefusal::Forked);
            }
            chain
                .append(link, listed_elsewhere)
                .map_err(ChainRefusal::Unauthorized)?;
        }
        Ok(chain)
    }
}

/// Why a device refuses an account chain under a genesis it keeps (§7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ChainRefusal {
    /// The chain doesn't extend the kept head: it stops short of it, holds
    /// other links up to it, or a link names another `previous`.
    #[error("the account chain doesn't extend the kept head")]
    Forked,
    /// A link after the kept head breaks a rule of §7.
    #[error("{0}")]
    Unauthorized(AccountError),
}

impl ChainRefusal {
    /// The client's report: `sync_account_forked` or
    /// `sync_account_unauthorized`.
    pub fn client_code(&self) -> &'static str {
        match self {
            ChainRefusal::Forked => "sync_account_forked",
            ChainRefusal::Unauthorized(error) => error.client_code(),
        }
    }

    /// The workspaces a refusal stops (§7): every workspace when the chain
    /// is the device's own account's, else every workspace the device
    /// shares with that account. Both refusals stop the same ones.
    pub fn stops(&self, own_account: bool) -> Stops {
        if own_account {
            Stops::EveryWorkspace
        } else {
            Stops::SharedWithAccount
        }
    }
}

/// The workspaces a device stops when it refuses an account's chain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stops {
    /// Every workspace the device shares with the account.
    SharedWithAccount,
    /// Every workspace, for the device's own account.
    EveryWorkspace,
}
