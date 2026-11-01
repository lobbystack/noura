//! Epoch records, epoch bundles and envelopes (§9).

use serde::{Deserialize, Serialize};

use super::Signed;
use crate::codec::{
    Decode, DecodeError, DecodeErrorKind, Encode, ExactBytes, Nested, Reader, Sorted, Whole, Writer,
};
use crate::tag::Tag;
use crate::types::{Digest, Id, Nonce};

/// The length of a lineage field after epoch 1: a 32-byte key and its tag.
pub const LINEAGE_LEN: usize = 48;
/// The most devices an epoch's key goes to.
pub const MAX_RECIPIENTS: usize = 512;
/// The length of an X-Wing encapsulated key.
pub const ENCAPSULATED_KEY_LEN: usize = 1_120;
/// The length of an envelope's ciphertext: a 32-byte key and its tag.
pub const ENVELOPE_CIPHERTEXT_LEN: usize = 48;

/// An epoch record's lineage field: empty at epoch 1, otherwise the previous
/// epoch key encrypted under this epoch's lineage subkey, 48 bytes.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Lineage(#[serde(with = "crate::types::hex_vec")] pub Vec<u8>);

impl Encode for Lineage {
    fn encode(&self, w: &mut Writer) {
        w.bytes(&self.0);
    }
}

impl Decode for Lineage {
    const MIN_SIZE: usize = 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        let offset = r.offset();
        let bytes = r.bytes(LINEAGE_LEN)?;
        if !bytes.is_empty() && bytes.len() != LINEAGE_LEN {
            return Err(r.error_at(DecodeErrorKind::WrongLength, offset));
        }
        Ok(Self(bytes.to_vec()))
    }
}

/// The signed body of an epoch record (§9).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Epoch {
    /// The workspace.
    pub workspace: Id,
    /// The key domain.
    pub domain: Id,
    /// 1 for a new domain, then +1.
    pub epoch: u64,
    /// The digest of the previous epoch record; zero at epoch 1.
    pub previous: Digest,
    /// The policy revision whose readers receive the key.
    pub policy_revision: u64,
    /// The digest of the change at `policy_revision`.
    pub policy_head: Digest,
    /// The device that rotated.
    pub rotating_device: Id,
    /// `BLAKE3(tag(noura.sync.epoch.recipients) || list<id>)`.
    pub recipients_digest: Digest,
    /// The key check of the epoch key.
    pub key_check: Digest,
    /// The lineage nonce; zero at epoch 1.
    pub lineage_nonce: Nonce,
    /// The previous epoch key, encrypted; empty at epoch 1.
    pub lineage: Lineage,
}

impl Epoch {
    /// The AEAD associated data of the lineage field: the body prefix that
    /// ends before `lineage_nonce`.
    pub fn aad(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode_prefix(&mut w);
        w.into_bytes()
    }

    fn encode_prefix(&self, w: &mut Writer) {
        w.tag(Tag::Epoch);
        w.put(&self.workspace);
        w.put(&self.domain);
        w.put(&self.epoch);
        w.put(&self.previous);
        w.put(&self.policy_revision);
        w.put(&self.policy_head);
        w.put(&self.rotating_device);
        w.put(&self.recipients_digest);
        w.put(&self.key_check);
    }
}

impl Encode for Epoch {
    fn encode(&self, w: &mut Writer) {
        self.encode_prefix(w);
        w.put(&self.lineage_nonce);
        w.put(&self.lineage);
    }
}

impl Decode for Epoch {
    const MIN_SIZE: usize =
        Tag::Epoch.encoded_len() + 16 + 16 + 8 + 32 + 8 + 32 + 16 + 32 + 32 + 24 + 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.tag(Tag::Epoch)?;
        let workspace = r.get()?;
        let domain = r.get()?;
        let epoch_offset = r.offset();
        let epoch: u64 = r.get()?;
        if epoch == 0 {
            return Err(r.error_at(DecodeErrorKind::OutOfRange, epoch_offset));
        }
        Ok(Self {
            workspace,
            domain,
            epoch,
            previous: r.get()?,
            policy_revision: r.get()?,
            policy_head: r.get()?,
            rotating_device: r.get()?,
            recipients_digest: r.get()?,
            key_check: r.get()?,
            lineage_nonce: r.get()?,
            lineage: r.get()?,
        })
    }
}

crate::wire_struct! {
    /// An epoch bundle, `bytes epoch | list<id> recipients`: what a device
    /// pushes and the server keeps whole. Readers get only the record.
    pub struct EpochBundle {
        /// The signed epoch record.
        pub epoch: Nested<Signed<Epoch>>,
        /// The devices that receive the key, sorted.
        pub recipients: Sorted<Id, Whole, MAX_RECIPIENTS>,
    }
}

/// The signed body of an envelope: one epoch key wrapped for one device.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Envelope {
    /// The workspace.
    pub workspace: Id,
    /// The key domain.
    pub domain: Id,
    /// The epoch whose key it wraps.
    pub epoch: u64,
    /// The recipient device.
    pub recipient: Id,
    /// `BLAKE3` of the recipient card's raw X-Wing public key.
    pub recipient_hpke_digest: Digest,
    /// The wrapping device.
    pub wrapping_device: Id,
    /// The HPKE encapsulated key, exactly 1,120 bytes.
    pub enc: ExactBytes<ENCAPSULATED_KEY_LEN>,
    /// The sealed epoch key and its tag, exactly 48 bytes.
    pub ciphertext: ExactBytes<ENVELOPE_CIPHERTEXT_LEN>,
}

impl Envelope {
    /// The HPKE `info`: the envelope prefix from the tag through
    /// `wrapping_device`.
    pub fn info(&self) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode_prefix(&mut w);
        w.into_bytes()
    }

    fn encode_prefix(&self, w: &mut Writer) {
        w.tag(Tag::Envelope);
        w.put(&self.workspace);
        w.put(&self.domain);
        w.put(&self.epoch);
        w.put(&self.recipient);
        w.put(&self.recipient_hpke_digest);
        w.put(&self.wrapping_device);
    }
}

impl Encode for Envelope {
    fn encode(&self, w: &mut Writer) {
        self.encode_prefix(w);
        w.put(&self.enc);
        w.put(&self.ciphertext);
    }
}

impl Decode for Envelope {
    const MIN_SIZE: usize = Tag::Envelope.encoded_len()
        + 16
        + 16
        + 8
        + 16
        + 32
        + 16
        + 4
        + ENCAPSULATED_KEY_LEN
        + 4
        + ENVELOPE_CIPHERTEXT_LEN;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.tag(Tag::Envelope)?;
        Ok(Self {
            workspace: r.get()?,
            domain: r.get()?,
            epoch: r.get()?,
            recipient: r.get()?,
            recipient_hpke_digest: r.get()?,
            wrapping_device: r.get()?,
            enc: r.get()?,
            ciphertext: r.get()?,
        })
    }
}
