//! Recovery kits and recovery proofs (§12).

use super::{AccountHead, ByWorkspace, Card};
use crate::codec::{Bytes, Decode, Encode, Nested, SortSpec, Sorted, Text};
use crate::crypto::{CryptoError, context, derive_key, open, seal};
use crate::tag::Tag;
use crate::types::{Digest, Id, Key, Nonce, SecretKey};

/// The length of a recovery kit's secret.
pub const KIT_SECRET_LEN: usize = 20;

crate::wire_struct! {
    /// `checkpoint_anchor`: a domain's latest accepted checkpoint.
    pub struct CheckpointAnchor {
        /// The domain.
        pub domain: Id,
        /// The checkpoint's `covered_seq`.
        pub covered_seq: u64,
        /// The checkpoint's digest.
        pub checkpoint: Digest,
    }
}

/// Sorts checkpoint anchors by domain.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct ByDomain;

impl SortSpec<CheckpointAnchor> for ByDomain {
    fn key(item: &CheckpointAnchor) -> Vec<u8> {
        item.domain.0.to_vec()
    }
}

crate::wire_struct! {
    /// `kit_workspace`: what a kit anchors in one workspace.
    pub struct KitWorkspace {
        /// The workspace.
        pub workspace: Id,
        /// The digest of the workspace's revision-1 policy change.
        pub genesis: Digest,
        /// The accepted policy head revision.
        pub revision: u64,
        /// The accepted policy head digest.
        pub head: Digest,
        /// The restore generation of the anchors.
        pub generation: u64,
        /// The latest accepted checkpoint per domain, sorted by domain.
        pub anchors: Sorted<CheckpointAnchor, ByDomain>,
    }
}

impl SortSpec<KitWorkspace> for ByWorkspace {
    fn key(item: &KitWorkspace) -> Vec<u8> {
        item.workspace.0.to_vec()
    }
}

crate::wire_struct! {
    /// The recovery kit's plaintext.
    pub struct KitPayload: tag RecoveryKitPayload {
        /// The server's origin.
        pub origin: Text,
        /// The account.
        pub account: Id,
        /// The account chain head when the kit was written, at which this
        /// recovery identity is active.
        pub account_head: AccountHead,
        /// The recovery identity's card.
        pub card: Nested<Card>,
        /// The Ed25519 seed.
        pub signing_seed: SecretKey,
        /// The 32-byte X-Wing seed.
        pub hpke_secret: SecretKey,
        /// The workspaces, sorted by workspace.
        pub workspaces: Sorted<KitWorkspace, ByWorkspace>,
    }
}

crate::wire_struct! {
    /// A recovery kit file.
    pub struct RecoveryKit: tag RecoveryKit {
        /// 16 random bytes.
        pub salt: Id,
        /// The nonce.
        pub nonce: Nonce,
        /// The payload under the kit key, with its tag.
        pub ciphertext: Bytes,
    }
}

/// Why a recovery kit failed to open: a wrong secret or a tampered file,
/// `sync_recovery_kit_invalid` either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum KitError {
    /// The secret's text form doesn't parse.
    #[error("the secret isn't 32 base32 characters")]
    Secret,
    /// Wrong secret or tampered file.
    #[error("{0}")]
    Decrypt(#[from] CryptoError),
    /// The plaintext doesn't parse.
    #[error("{0}")]
    Decode(#[from] crate::codec::DecodeError),
}

fn kit_key(secret: &[u8; KIT_SECRET_LEN], salt: &Id) -> SecretKey {
    let mut material = [0u8; KIT_SECRET_LEN + 16];
    material[..KIT_SECRET_LEN].copy_from_slice(secret);
    material[KIT_SECRET_LEN..].copy_from_slice(&salt.0);
    let key = derive_key(context::RECOVERY_KIT, &material);
    zeroize::Zeroize::zeroize(&mut material);
    key
}

impl RecoveryKit {
    fn aad(&self) -> Vec<u8> {
        let mut w = crate::codec::Writer::new();
        w.tag(Tag::RecoveryKit);
        w.put(&self.salt);
        w.into_bytes()
    }

    /// Encrypts `payload` under the kit key of `secret` and `salt`.
    pub fn seal(
        payload: &KitPayload,
        secret: &[u8; KIT_SECRET_LEN],
        salt: Id,
        nonce: Nonce,
    ) -> Self {
        let mut kit = Self {
            salt,
            nonce,
            ciphertext: Bytes::default(),
        };
        let mut plaintext = payload.to_bytes();
        kit.ciphertext = Bytes(seal(
            &kit_key(secret, &salt),
            &nonce,
            &kit.aad(),
            &plaintext,
        ));
        zeroize::Zeroize::zeroize(&mut plaintext);
        kit
    }

    /// Decrypts the kit with `secret`.
    pub fn open(&self, secret: &[u8; KIT_SECRET_LEN]) -> Result<KitPayload, KitError> {
        let mut plaintext = open(
            &kit_key(secret, &self.salt),
            &self.nonce,
            &self.aad(),
            &self.ciphertext,
        )?;
        let payload = KitPayload::from_bytes(&plaintext);
        zeroize::Zeroize::zeroize(&mut plaintext);
        Ok(payload?)
    }
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// The secret as you write it down: 32 characters of RFC 4648 base32 in 8
/// groups of 4.
pub fn kit_secret_text(secret: &[u8; KIT_SECRET_LEN]) -> String {
    let mut chars = Vec::with_capacity(32);
    for block in secret.chunks(5) {
        let value = block.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
        for i in (0..8).rev() {
            chars.push(BASE32[((value >> (i * 5)) & 31) as usize]);
        }
    }
    chars
        .chunks(4)
        .map(|group| std::str::from_utf8(group).expect("base32 is ASCII"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Parses a typed secret, ignoring spaces, hyphens and letter case.
pub fn parse_kit_secret(text: &str) -> Result<[u8; KIT_SECRET_LEN], KitError> {
    let symbols: Vec<u8> = text
        .chars()
        .filter(|c| *c != ' ' && *c != '-')
        .map(|c| {
            let upper = c.to_ascii_uppercase();
            BASE32
                .iter()
                .position(|&b| char::from(b) == upper)
                .map(|p| p as u8)
                .ok_or(KitError::Secret)
        })
        .collect::<Result<_, _>>()?;
    if symbols.len() != 32 {
        return Err(KitError::Secret);
    }
    let mut out = [0u8; KIT_SECRET_LEN];
    for (block, chunk) in symbols.chunks(8).enumerate() {
        let value = chunk.iter().fold(0u64, |acc, &s| (acc << 5) | u64::from(s));
        out[block * 5..block * 5 + 5].copy_from_slice(&value.to_be_bytes()[3..]);
    }
    Ok(out)
}

crate::wire_struct! {
    /// `recovery_proof`, signed with the recovery identity's key.
    pub struct RecoveryProof: tag RecoveryProof {
        /// The server's origin, as in the enrollment proof.
        pub origin: Text,
        /// The recovery identity's device ID.
        pub recovery_device: Id,
        /// `BLAKE3` of the new device's enrolled card.
        pub new_card_digest: Digest,
        /// A fresh enrollment challenge.
        pub challenge: Key,
    }
}
