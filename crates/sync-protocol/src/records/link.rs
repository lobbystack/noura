//! The account chain link (§7) and the account head (§8).

use serde::{Deserialize, Serialize};

use super::{Card, Signed, unknown_value};
use crate::codec::{Decode, DecodeError, Encode, Nested, Reader, SortSpec, Sorted, Whole, Writer};
use crate::tag::Tag;
use crate::types::{Digest, Id};

/// The most cards an account chain holds active at once.
pub const MAX_ACTIVE_CARDS: usize = 256;

crate::wire_struct! {
    /// `account_head`: an account chain's genesis digest, a seqno and the
    /// digest of the link at that seqno (§8).
    pub struct AccountHead {
        /// The account.
        pub account: Id,
        /// The digest of the chain's genesis link.
        pub genesis: Digest,
        /// The seqno of the head link.
        pub seqno: u64,
        /// The digest of the link at `seqno`.
        pub head: Digest,
    }
}

/// Sorts account heads, `access` entries and similar elements by account.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ByAccount;

impl SortSpec<AccountHead> for ByAccount {
    fn key(item: &AccountHead) -> Vec<u8> {
        item.account.0.to_vec()
    }
}

/// The kind-specific body of an account chain link.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LinkKind {
    /// 1: the account's first card, signed by itself.
    Genesis {
        /// The first device's card.
        card: Nested<Card>,
    },
    /// 2: a device that an active device approved.
    AddDevice {
        /// The new card.
        card: Nested<Card>,
        /// The digest of the approval offer whose code was compared (§10).
        offer: Digest,
    },
    /// 3: a recovery identity, added while none is active.
    AddRecovery {
        /// The recovery identity's card.
        card: Nested<Card>,
    },
    /// 4: a device that the active recovery identity recovered, once.
    RecoverDevice {
        /// The recovered device's card.
        card: Nested<Card>,
    },
    /// 5: removes a card and, in turn, every active card whose parent it
    /// removes, except the signer and the devices `keep` names.
    Remove {
        /// The card to remove.
        device: Id,
        /// Devices the removal would otherwise take down, sorted.
        keep: Sorted<Id, Whole, MAX_ACTIVE_CARDS>,
    },
}

impl LinkKind {
    /// The kind byte.
    pub fn code(&self) -> u8 {
        match self {
            LinkKind::Genesis { .. } => 1,
            LinkKind::AddDevice { .. } => 2,
            LinkKind::AddRecovery { .. } => 3,
            LinkKind::RecoverDevice { .. } => 4,
            LinkKind::Remove { .. } => 5,
        }
    }

    /// The card a link of this kind adds, if any.
    pub fn added_card(&self) -> Option<&Card> {
        match self {
            LinkKind::Genesis { card }
            | LinkKind::AddDevice { card, .. }
            | LinkKind::AddRecovery { card }
            | LinkKind::RecoverDevice { card } => Some(card),
            LinkKind::Remove { .. } => None,
        }
    }
}

/// `account_link`, the signed body of one link of an account chain (§7).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AccountLink {
    /// The account whose chain this is.
    pub account: Id,
    /// 1 for the genesis, then +1.
    pub seqno: u64,
    /// The digest of link `seqno − 1`; 32 zero bytes at seqno 1.
    pub previous: Digest,
    /// The device ID of the signing card.
    pub signer: Id,
    /// The kind and its body.
    #[serde(flatten)]
    pub kind: LinkKind,
}

impl Encode for AccountLink {
    fn encode(&self, w: &mut Writer) {
        w.tag(Tag::Account);
        w.put(&self.account);
        w.put(&self.seqno);
        w.put(&self.previous);
        w.put(&self.signer);
        w.u8(self.kind.code());
        match &self.kind {
            LinkKind::Genesis { card }
            | LinkKind::AddRecovery { card }
            | LinkKind::RecoverDevice { card } => w.put(card),
            LinkKind::AddDevice { card, offer } => {
                w.put(card);
                w.put(offer);
            }
            LinkKind::Remove { device, keep } => {
                w.put(device);
                w.put(keep);
            }
        }
    }
}

impl Decode for AccountLink {
    const MIN_SIZE: usize = Tag::Account.encoded_len() + 16 + 8 + 32 + 16 + 1 + 16 + 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.tag(Tag::Account)?;
        let account = r.get()?;
        let seqno = r.get()?;
        let previous = r.get()?;
        let signer = r.get()?;
        let kind_offset = r.offset();
        let kind = match r.u8()? {
            1 => LinkKind::Genesis { card: r.get()? },
            2 => LinkKind::AddDevice {
                card: r.get()?,
                offer: r.get()?,
            },
            3 => LinkKind::AddRecovery { card: r.get()? },
            4 => LinkKind::RecoverDevice { card: r.get()? },
            5 => LinkKind::Remove {
                device: r.get()?,
                keep: r.get()?,
            },
            _ => return Err(unknown_value(r, kind_offset)),
        };
        Ok(Self {
            account,
            seqno,
            previous,
            signer,
            kind,
        })
    }
}

/// A signed account chain link.
pub type SignedLink = Signed<AccountLink>;
