//! Builders for hand-made test scenarios. Keys derive from names, so every
//! run builds the same records. The X-Wing keys are filler: nothing in this
//! crate reads them.

#![allow(dead_code)]

use sync_protocol::codec::{Nested, Sorted};
use sync_protocol::crypto::{SigningKey, derive_key};
use sync_protocol::records::{AccountLink, Card, LinkKind, Signed};
use sync_protocol::types::{Digest, Id};

/// A test device: a signing key and its card.
#[derive(Clone, Debug)]
pub struct Device {
    pub key: SigningKey,
    pub card: Card,
}

impl Device {
    pub fn id(&self) -> Id {
        self.card.device
    }
}

/// A test account ID from a name.
pub fn account(name: &str) -> Id {
    let mut id = [0u8; 16];
    id.copy_from_slice(&derive_key("noura test account", name.as_bytes()).0[..16]);
    Id(id)
}

/// A test device of `account`, keyed by `name`.
pub fn device(account: Id, name: &str) -> Device {
    let key = SigningKey::from_seed(&derive_key("noura test device", name.as_bytes()));
    let card = Card::new(account, key.public(), vec![0x5a; 1_216]).unwrap();
    Device { key, card }
}

/// Builds an account chain link by link.
pub struct ChainBuilder {
    pub account: Id,
    pub links: Vec<Signed<AccountLink>>,
}

impl ChainBuilder {
    /// A chain whose genesis `first` signs.
    pub fn new(first: &Device) -> Self {
        let account = first.card.account;
        let genesis = Signed::sign(
            AccountLink {
                account,
                seqno: 1,
                previous: Digest::ZERO,
                signer: first.id(),
                kind: LinkKind::Genesis {
                    card: Nested(first.card.clone()),
                },
            },
            &first.key,
        );
        Self {
            account,
            links: vec![genesis],
        }
    }

    /// The next link, signed by `signer`, without appending it.
    pub fn next(&self, signer: &Device, kind: LinkKind) -> Signed<AccountLink> {
        let last = self.links.last().unwrap();
        Signed::sign(
            AccountLink {
                account: self.account,
                seqno: last.body.seqno + 1,
                previous: last.digest(),
                signer: signer.id(),
                kind,
            },
            &signer.key,
        )
    }

    /// Appends the next link.
    pub fn push(&mut self, signer: &Device, kind: LinkKind) -> &mut Self {
        let link = self.next(signer, kind);
        self.links.push(link);
        self
    }

    pub fn add_device(&mut self, signer: &Device, new: &Device) -> &mut Self {
        self.push(
            signer,
            LinkKind::AddDevice {
                card: Nested(new.card.clone()),
                offer: Digest([0x0f; 32]),
            },
        )
    }

    pub fn add_recovery(&mut self, signer: &Device, identity: &Device) -> &mut Self {
        self.push(
            signer,
            LinkKind::AddRecovery {
                card: Nested(identity.card.clone()),
            },
        )
    }

    pub fn recover(&mut self, identity: &Device, new: &Device) -> &mut Self {
        self.push(
            identity,
            LinkKind::RecoverDevice {
                card: Nested(new.card.clone()),
            },
        )
    }

    pub fn remove(&mut self, signer: &Device, target: &Device, keep: &[&Device]) -> &mut Self {
        self.push(signer, remove_kind(target, keep))
    }
}

pub fn remove_kind(target: &Device, keep: &[&Device]) -> LinkKind {
    LinkKind::Remove {
        device: target.id(),
        keep: Sorted::new(keep.iter().map(|d| d.id()).collect()).unwrap(),
    }
}

impl ChainBuilder {
    /// The head at the last link.
    pub fn head(&self) -> sync_protocol::records::AccountHead {
        self.head_at(self.links.len() as u64)
    }

    /// The head at `seqno`.
    pub fn head_at(&self, seqno: u64) -> sync_protocol::records::AccountHead {
        sync_protocol::records::AccountHead {
            account: self.account,
            genesis: self.links[0].digest(),
            seqno,
            head: self.links[seqno as usize - 1].digest(),
        }
    }

    /// The verified chain.
    pub fn verified(&self) -> sync_protocol::account::AccountChain {
        sync_protocol::account::AccountChain::verify(&self.links, &Default::default()).unwrap()
    }
}

/// Builds a policy chain change by change.
pub struct PolicyBuilder {
    pub workspace: Id,
    pub changes: Vec<Signed<sync_protocol::records::PolicyChange>>,
}

impl PolicyBuilder {
    pub fn new(workspace: Id) -> Self {
        Self {
            workspace,
            changes: Vec::new(),
        }
    }

    /// The next change, signed by `signer`, without appending it.
    pub fn next(
        &self,
        signer: &Device,
        heads: Vec<sync_protocol::records::AccountHead>,
        kind: sync_protocol::records::PolicyKind,
    ) -> Signed<sync_protocol::records::PolicyChange> {
        let (revision, previous) = match self.changes.last() {
            Some(last) => (last.body.revision + 1, last.digest()),
            None => (1, Digest::ZERO),
        };
        Signed::sign(
            sync_protocol::records::PolicyChange {
                workspace: self.workspace,
                revision,
                previous,
                signer: signer.id(),
                heads: Sorted::new(heads).unwrap(),
                kind,
            },
            &signer.key,
        )
    }

    /// Appends the next change.
    pub fn push(
        &mut self,
        signer: &Device,
        heads: Vec<sync_protocol::records::AccountHead>,
        kind: sync_protocol::records::PolicyKind,
    ) -> &mut Self {
        let change = self.next(signer, heads, kind);
        self.changes.push(change);
        self
    }
}

/// The verified chains of every builder.
pub fn chains(builders: &[&ChainBuilder]) -> sync_protocol::policy::AccountChains {
    let mut chains = sync_protocol::policy::AccountChains::new();
    for builder in builders {
        chains.insert(builder.verified());
    }
    chains
}

/// A test account with its chain and devices.
pub struct Person {
    pub chain: ChainBuilder,
    pub devices: Vec<Device>,
}

impl Person {
    pub fn new(name: &str) -> Self {
        let first = device(account(name), &format!("{name} laptop"));
        Self {
            chain: ChainBuilder::new(&first),
            devices: vec![first],
        }
    }

    pub fn account(&self) -> Id {
        self.chain.account
    }

    pub fn first(&self) -> &Device {
        &self.devices[0]
    }

    pub fn add(&mut self, name: &str) -> Device {
        let new = device(self.account(), name);
        let signer = self.devices[0].clone();
        self.chain.add_device(&signer, &new);
        self.devices.push(new.clone());
        new
    }
}
