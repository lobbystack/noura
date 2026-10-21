//! A small, consistent workspace that the fixtures draw records from:
//! Alice owns it from her laptop and phone, Bob edits from his laptop, and a
//! restricted domain grants Alice management and Bob read access.

use std::collections::BTreeSet;

use sync_key_envelope::v2::{EnvelopeHeader, genesis_link, next_link, wrap_with_rng};
use sync_protocol::codec::{Bytes, Nested, Sorted, Text};
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::live::LiveSet;
use sync_protocol::manifest::{Manifest, heads_digest};
use sync_protocol::policy::{AccountChains, PolicyChain};
use sync_protocol::records::{
    Access, AccountHead, AccountLink, Checkpoint, CheckpointBundle, Content, Cover, Envelope,
    Epoch, EpochBundle, Level, LinkKind, Operation, Payload, PayloadKind, PolicyChange, PolicyKind,
    Profile, ProfilePayload, Ref, Role, Signed, SyncPath,
};
use sync_protocol::types::{Digest, Id, SecretKey};

use super::common::{Actor, id, nonce, secret};

/// Seeded randomness for X-Wing encapsulation, so envelopes regenerate
/// byte for byte.
pub struct SeededRng(blake3::OutputReader);

impl SeededRng {
    pub fn new(label: &str) -> Self {
        let mut hasher = blake3::Hasher::new_derive_key("noura.sync.v2 fixture rng");
        hasher.update(label.as_bytes());
        Self(hasher.finalize_xof())
    }
}

impl rand_core::TryRng for SeededRng {
    type Error = std::convert::Infallible;
    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut b = [0u8; 4];
        self.0.fill(&mut b);
        Ok(u32::from_le_bytes(b))
    }
    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut b = [0u8; 8];
        self.0.fill(&mut b);
        Ok(u64::from_le_bytes(b))
    }
    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        self.0.fill(dst);
        Ok(())
    }
}

impl rand_core::TryCryptoRng for SeededRng {}

/// An account chain under construction.
#[derive(Clone)]
pub struct Chain {
    pub links: Vec<Signed<AccountLink>>,
}

impl Chain {
    pub fn genesis(first: &Actor) -> Self {
        Self {
            links: vec![genesis_link(&first.keys, first.account)],
        }
    }

    pub fn head(&self) -> AccountHead {
        self.head_at(self.links.len() as u64)
    }

    pub fn head_at(&self, seqno: u64) -> AccountHead {
        let link = &self.links[seqno as usize - 1];
        AccountHead {
            account: link.body.account,
            genesis: self.links[0].digest(),
            seqno,
            head: link.digest(),
        }
    }

    pub fn push(&mut self, signer: &Actor, kind: LinkKind) -> Signed<AccountLink> {
        let link = next_link(&signer.keys, &self.head(), kind);
        self.links.push(link.clone());
        link
    }

    pub fn add_device(&mut self, signer: &Actor, new: &Actor) -> Signed<AccountLink> {
        self.push(
            signer,
            LinkKind::AddDevice {
                card: Nested(new.card()),
                offer: id("approval offer").into_digest(),
            },
        )
    }
}

/// A 16-byte ID widened to a digest-sized filler, for fields that only need
/// a value.
pub trait IntoDigest {
    fn into_digest(self) -> Digest;
}

impl IntoDigest for Id {
    fn into_digest(self) -> Digest {
        sync_protocol::crypto::hash(&self.0)
    }
}

/// A policy chain under construction.
#[derive(Clone)]
pub struct Policy {
    pub workspace: Id,
    pub changes: Vec<Signed<PolicyChange>>,
}

impl Policy {
    pub fn new(workspace: Id) -> Self {
        Self {
            workspace,
            changes: Vec::new(),
        }
    }

    pub fn next(
        &self,
        signer: &Actor,
        heads: Vec<AccountHead>,
        kind: PolicyKind,
    ) -> Signed<PolicyChange> {
        let (revision, previous) = match self.changes.last() {
            Some(last) => (last.body.revision + 1, last.digest()),
            None => (1, Digest::ZERO),
        };
        Signed::sign(
            PolicyChange {
                workspace: self.workspace,
                revision,
                previous,
                signer: signer.device(),
                heads: Sorted::new(heads).unwrap(),
                kind,
            },
            signer.keys.signing(),
        )
    }

    pub fn push(
        &mut self,
        signer: &Actor,
        heads: Vec<AccountHead>,
        kind: PolicyKind,
    ) -> Signed<PolicyChange> {
        let change = self.next(signer, heads, kind);
        self.changes.push(change.clone());
        change
    }
}

pub fn verified_chains(chains: &[&Chain]) -> AccountChains {
    let mut out = AccountChains::new();
    for chain in chains {
        out.insert(
            sync_protocol::account::AccountChain::verify(&chain.links, &Default::default())
                .unwrap(),
        );
    }
    out
}

pub struct World {
    pub alice_laptop: Actor,
    pub alice_phone: Actor,
    pub bob_laptop: Actor,
    pub alice: Chain,
    pub bob: Chain,
    pub workspace: Id,
    pub ws_domain: Id,
    pub restricted: Id,
    pub marker_rid: Id,
    pub policy: Policy,
    pub verified: PolicyChain,
    pub ws_epoch1: Signed<Epoch>,
    pub ws_epoch2: Signed<Epoch>,
    pub ws_key1: SecretKey,
    pub ws_key2: SecretKey,
    pub restricted_epoch1: Signed<Epoch>,
    pub restricted_key1: SecretKey,
    pub envelope_bob: Signed<Envelope>,
    pub ws_bundle2: EpochBundle,
    pub rid: Id,
    pub op: Signed<Operation>,
    pub payload: Payload,
    pub checkpoint: CheckpointBundle,
    pub profile: Signed<Profile>,
}

pub fn epoch_record(
    world_policy: &PolicyChain,
    workspace: Id,
    domain: Id,
    epoch: u64,
    previous: Option<&Signed<Epoch>>,
    revision: u64,
    rotator: &Actor,
    key: &SecretKey,
    previous_key: Option<&SecretKey>,
) -> Signed<Epoch> {
    let snapshot = world_policy.at(revision).unwrap();
    let params = EpochParams {
        workspace,
        domain,
        epoch,
        previous: previous.map_or(Digest::ZERO, |p| p.digest()),
        policy_revision: revision,
        policy_head: snapshot.digest,
        rotating_device: rotator.device(),
        recipients: snapshot.readers(&domain),
    };
    Signed::sign(
        build_epoch(
            &params,
            key,
            previous_key,
            nonce(&format!("lineage {epoch}")),
        ),
        rotator.keys.signing(),
    )
}

pub fn world() -> World {
    let alice_laptop = Actor::new("alice", "alice laptop");
    let alice_phone = Actor::new("alice", "alice phone");
    let bob_laptop = Actor::new("bob", "bob laptop");
    let mut alice = Chain::genesis(&alice_laptop);
    alice.add_device(&alice_laptop, &alice_phone);
    let bob = Chain::genesis(&bob_laptop);

    let workspace = id("workspace");
    let ws_domain = id("workspace domain");
    let restricted = id("restricted domain");
    let marker_rid = id("marker rid");
    let mut policy = Policy::new(workspace);
    policy.push(
        &alice_laptop,
        vec![alice.head()],
        PolicyKind::Genesis {
            workspace_domain: ws_domain,
        },
    );
    policy.push(
        &alice_laptop,
        vec![alice.head(), bob.head()],
        PolicyKind::AddMember {
            account: bob_laptop.account,
            role: Role::Editor,
            invitation: id("invitation"),
        },
    );
    policy.push(
        &alice_laptop,
        vec![alice.head()],
        PolicyKind::CreateDomain {
            domain: restricted,
            marker_rid,
            access: Sorted::new(vec![
                Access {
                    account: alice_laptop.account,
                    level: Level::Manage,
                },
                Access {
                    account: bob_laptop.account,
                    level: Level::Read,
                },
            ])
            .unwrap(),
        },
    );
    let chains = verified_chains(&[&alice, &bob]);
    let verified = PolicyChain::verify(&policy.changes, &chains).unwrap();

    let ws_key1 = secret("workspace epoch 1");
    let ws_key2 = secret("workspace epoch 2");
    let restricted_key1 = secret("restricted epoch 1");
    let ws_epoch1 = epoch_record(
        &verified,
        workspace,
        ws_domain,
        1,
        None,
        1,
        &alice_laptop,
        &ws_key1,
        None,
    );
    let ws_epoch2 = epoch_record(
        &verified,
        workspace,
        ws_domain,
        2,
        Some(&ws_epoch1),
        3,
        &alice_laptop,
        &ws_key2,
        Some(&ws_key1),
    );
    let restricted_epoch1 = epoch_record(
        &verified,
        workspace,
        restricted,
        1,
        None,
        3,
        &alice_laptop,
        &restricted_key1,
        None,
    );
    let envelope_bob = wrap_with_rng(
        &EnvelopeHeader {
            workspace,
            domain: ws_domain,
            epoch: 2,
            wrapping_device: alice_laptop.device(),
        },
        &ws_key2,
        &bob_laptop.card(),
        &alice_laptop.keys,
        &mut SeededRng::new("envelope for bob"),
    )
    .unwrap();
    let recipients: BTreeSet<Id> = verified.at(3).unwrap().readers(&ws_domain);
    let ws_bundle2 = EpochBundle {
        epoch: Nested(ws_epoch2.clone()),
        recipients: Sorted::new(recipients.into_iter().collect()).unwrap(),
    };

    let rid = id("note rid");
    let payload = Payload {
        rid,
        kind: PayloadKind::File {
            object_id: Text::new("note_01").unwrap(),
            path: SyncPath::new("Notes/Plan.md").unwrap(),
            revision: sync_protocol::crypto::hash(b"# Plan\n"),
            content: Content::Inline {
                bytes: Bytes(b"# Plan\n".to_vec()),
            },
            origin: None,
        },
    };
    let mut op_body = Operation {
        workspace,
        domain: ws_domain,
        device: alice_laptop.device(),
        counter: 1,
        previous: Digest::ZERO,
        epoch: 2,
        policy_revision: 3,
        policy_head: verified.at(3).unwrap().digest,
        previous_attempt: Digest::ZERO,
        generation: 1,
        checkpoint_seq: 0,
        checkpoint: Digest::ZERO,
        rid,
        delete: false,
        supersedes: Sorted::default(),
        blobs: Sorted::default(),
        nonce: sync_protocol::types::Nonce::ZERO,
        ciphertext: Bytes::default(),
    };
    op_body
        .seal_payload(&payload, &ws_key2, &ws_epoch2.digest(), nonce("operation"))
        .unwrap();
    let op = Signed::sign(op_body, alice_laptop.keys.signing());

    // The log: policy entries 1 to 3 at sequences 1 to 3, epoch 1's entry
    // at 4, epoch 2's at 5, the operation at 6; the checkpoint covers 6.
    let mut live = LiveSet::new();
    live.apply(sync_protocol::live::LiveOp::from_operation(&op, 6));
    let manifest = Manifest::new(live.leaves_at(6));
    let heads = vec![Ref {
        device: alice_laptop.device(),
        counter: 1,
        seq: 6,
        op_digest: op.digest(),
    }];
    let checkpoint = Signed::sign(
        Checkpoint {
            workspace,
            domain: ws_domain,
            previous: Digest::ZERO,
            number: 1,
            skip: Digest::ZERO,
            covered_seq: 6,
            floor: 0,
            policy_revision: 3,
            policy_head: verified.at(3).unwrap().digest,
            live_objects: manifest.live_objects(),
            manifest_root: manifest.root(),
            heads_digest: heads_digest(&heads),
            epoch: 2,
            epoch_record: ws_epoch2.digest(),
            signer: alice_laptop.device(),
        },
        alice_laptop.keys.signing(),
    );
    let checkpoint = CheckpointBundle {
        checkpoint: Nested(checkpoint),
        heads: Sorted::new(heads).unwrap(),
    };

    let mut profile_body = Profile {
        workspace,
        profile_revision: 1,
        epoch: 2,
        policy_revision: 3,
        policy_head: verified.at(3).unwrap().digest,
        signer: alice_laptop.device(),
        nonce: sync_protocol::types::Nonce::ZERO,
        ciphertext: Bytes::default(),
    };
    profile_body.seal_payload(
        &ProfilePayload {
            name: Text::new("Garden plans").unwrap(),
        },
        &ws_key2,
        &ws_epoch2.digest(),
        nonce("profile"),
    );
    let profile = Signed::sign(profile_body, alice_laptop.keys.signing());

    let _ = Cover {
        device: alice_laptop.device(),
        counter: 1,
    };
    World {
        alice_laptop,
        alice_phone,
        bob_laptop,
        alice,
        bob,
        workspace,
        ws_domain,
        restricted,
        marker_rid,
        policy,
        verified,
        ws_epoch1,
        ws_epoch2,
        ws_key1,
        ws_key2,
        restricted_epoch1,
        restricted_key1,
        envelope_bob,
        ws_bundle2,
        rid,
        op,
        payload,
        checkpoint,
        profile,
    }
}
