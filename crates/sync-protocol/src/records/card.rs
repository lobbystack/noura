//! The device card (§7).

use serde::{Deserialize, Serialize};

use crate::codec::{Decode, DecodeError, DecodeErrorKind, Encode, ExactBytes, Reader, Writer};
use crate::crypto::hash;
use crate::ids::device_id;
use crate::tag::Tag;
use crate::types::{Digest, Id, Key};

/// The length of an X-Wing public key.
pub const HPKE_PUBLIC_LEN: usize = 1_216;

/// An X-Wing public key, exactly 1,216 bytes.
pub type HpkePublic = ExactBytes<HPKE_PUBLIC_LEN>;

/// A device card: an unsigned record that binds a device's Ed25519 signing
/// key and X-Wing HPKE key to its account. Decoding refuses a card whose
/// device ID doesn't derive from its signing key.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Card {
    /// The account the card belongs to.
    pub account: Id,
    /// The device ID, derived from `signing_public`.
    pub device: Id,
    /// The Ed25519 public key.
    pub signing_public: Key,
    /// The X-Wing public key.
    pub hpke_public: HpkePublic,
}

impl Card {
    /// A card for `account` whose device ID derives from `signing_public`.
    pub fn new(
        account: Id,
        signing_public: Key,
        hpke_public: Vec<u8>,
    ) -> Result<Self, DecodeErrorKind> {
        Ok(Self {
            account,
            device: device_id(&signing_public),
            signing_public,
            hpke_public: ExactBytes::new(hpke_public)?,
        })
    }

    /// The device fingerprint, `BLAKE3(card)`.
    pub fn fingerprint(&self) -> Digest {
        hash(&self.to_bytes())
    }

    /// `BLAKE3` of the raw X-Wing public key, without its length prefix,
    /// as an envelope's `recipient_hpke_digest` names it (§9).
    pub fn hpke_digest(&self) -> Digest {
        hash(self.hpke_public.as_slice())
    }
}

/// A fingerprint as apps show it: 64 lowercase hexadecimal characters in 16
/// groups of 4 (§7).
pub fn fingerprint_text(fingerprint: &Digest) -> String {
    let hex = fingerprint.to_hex();
    hex.as_bytes()
        .chunks(4)
        .map(|group| std::str::from_utf8(group).expect("hexadecimal is ASCII"))
        .collect::<Vec<_>>()
        .join(" ")
}

impl Encode for Card {
    fn encode(&self, w: &mut Writer) {
        w.tag(Tag::DeviceCard);
        w.put(&self.account);
        w.put(&self.device);
        w.put(&self.signing_public);
        w.put(&self.hpke_public);
    }
}

impl Decode for Card {
    const MIN_SIZE: usize = Tag::DeviceCard.encoded_len() + 16 + 16 + 32 + 4 + HPKE_PUBLIC_LEN;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.tag(Tag::DeviceCard)?;
        let account = r.get()?;
        let device_offset = r.offset();
        let device: Id = r.get()?;
        let signing_public: Key = r.get()?;
        let hpke_public = r.get()?;
        if device_id(&signing_public) != device {
            return Err(r.error_at(DecodeErrorKind::DeviceIdMismatch, device_offset));
        }
        Ok(Self {
            account,
            device,
            signing_public,
            hpke_public,
        })
    }
}
