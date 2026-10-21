//! The records of version 2: their layouts, encodings and signatures.
//!
//! A signed record is its body followed by `sig`, an Ed25519 signature over
//! the body bytes, and its digest is `BLAKE3(body || sig)` (§4). Encoding is
//! canonical, so the body bytes a verifier checks are the bytes it received.

mod approval;
mod card;
mod checkpoint;
mod device;
mod epoch;
mod invitation;
mod link;
mod operation;
mod policy;
mod presence;
mod profile;
mod recovery;
mod share;

pub use approval::{Anchor, Offer, approval_code, approval_code_text, approval_commit};
pub use card::{Card, HPKE_PUBLIC_LEN, HpkePublic, fingerprint_text};
pub use checkpoint::{ByDeviceCounter, Checkpoint, CheckpointBundle, Heads, Ref};
pub use device::Enroll;
pub use epoch::{
    ENCAPSULATED_KEY_LEN, ENVELOPE_CIPHERTEXT_LEN, Envelope, Epoch, EpochBundle, LINEAGE_LEN,
    Lineage, MAX_RECIPIENTS,
};
pub use invitation::{InviteContext, InviteError, InvitePayload, invite_proof, invite_token_hash};
pub use link::{AccountHead, AccountLink, ByAccount, LinkKind, MAX_ACTIVE_CARDS, SignedLink};
pub use operation::{
    BlobRef, ByDevice, ByKey, Content, Cover, DeleteReason, MAX_BLOBS, MAX_COVERS, MAX_LOAD,
    MAX_OPERATION_CIPHERTEXT, MAX_PATH_LEN, MAX_PAYLOAD_LEN, Meta, MetaList, OpClass, OpenError,
    Operation, Origin, Payload, PayloadKind, SealError, SyncPath, operation_ciphertext_len,
    validate_path,
};
pub use policy::{
    Access, AccessList, ByWorkspace, Level, MAX_ACCESS, PolicyChange, PolicyKind, Role,
};
pub use presence::{
    MAX_POSITION_LEN, MAX_PRESENCE_PLAINTEXT, MAX_SELECTIONS, Presence, PresenceError,
    PresencePayload, PresenceState, Selection, presence_channel,
};
pub use profile::{Profile, ProfileError, ProfilePayload};
pub use recovery::{
    ByDomain, CheckpointAnchor, KIT_SECRET_LEN, KitError, KitPayload, KitWorkspace, RecoveryKit,
    RecoveryProof, kit_secret_text, parse_kit_secret,
};
pub use share::{
    MAX_SHARE_PLAINTEXT, Share, ShareError, SharePayload, open_share, parse_share_fragment,
    share_fragment, share_token_hash,
};

use serde::{Deserialize, Serialize};

use crate::codec::{Decode, DecodeError, Encode, Reader, Writer};
use crate::crypto::{CryptoError, SigningKey, hash, verify};
use crate::types::{Digest, Key, Signature};

/// A signed record: a body and an Ed25519 signature over its encoding.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Signed<B> {
    /// The signed fields.
    pub body: B,
    /// The signature over the body's encoding.
    pub sig: Signature,
}

impl<B: Encode> Signed<B> {
    /// Signs `body` with `key`.
    pub fn sign(body: B, key: &SigningKey) -> Self {
        let sig = key.sign(&body.to_bytes());
        Self { body, sig }
    }

    /// The bytes the signature covers.
    pub fn body_bytes(&self) -> Vec<u8> {
        self.body.to_bytes()
    }

    /// The record's digest, `BLAKE3(body || sig)`.
    pub fn digest(&self) -> Digest {
        hash(&self.to_bytes())
    }

    /// Checks the signature under `public` with `verify_strict`.
    pub fn verify(&self, public: &Key) -> Result<(), CryptoError> {
        verify(public, &self.body_bytes(), &self.sig)
    }
}

impl<B: Encode> Encode for Signed<B> {
    fn encode(&self, w: &mut Writer) {
        self.body.encode(w);
        self.sig.encode(w);
    }
}

impl<B: Decode> Decode for Signed<B> {
    const MIN_SIZE: usize = B::MIN_SIZE + Signature::LEN;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        Ok(Self {
            body: r.get()?,
            sig: r.get()?,
        })
    }
}

/// The reader error for an enum, kind or type byte the spec doesn't define.
pub(crate) fn unknown_value(r: &Reader<'_>, offset: usize) -> DecodeError {
    r.error_at(crate::codec::DecodeErrorKind::UnknownValue, offset)
}
