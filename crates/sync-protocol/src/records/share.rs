//! Share link snapshots (§24).

use crate::codec::{Bytes, Decode, DecodeError, Encode, Text, Writer};
use crate::crypto::{CryptoError, SigningKey, context, derive_key, hash_parts, open, seal};
use crate::padding::{PaddingError, pad, unpad};
use crate::tag::Tag;
use crate::types::{Digest, Key, Nonce, SecretKey, TextFormError, from_base64url, to_base64url};

/// The most bytes of a padded share snapshot.
pub const MAX_SHARE_PLAINTEXT: usize = 983_040;

crate::wire_struct! {
    /// The share snapshot's padded plaintext.
    pub struct SharePayload: tag SharePayload {
        /// The note's title.
        pub title: Text,
        /// The note's Markdown.
        pub markdown: Bytes,
    }
}

crate::wire_struct! {
    /// The signed body of a share snapshot, signed with the link's key.
    pub struct Share: tag Share {
        /// `BLAKE3(tag(noura.sync.share.token) || token)`.
        pub token_hash: Digest,
        /// 1, then +1 on each Update link.
        pub snapshot_revision: u64,
        /// The nonce.
        pub nonce: Nonce,
        /// The padded payload under the share key, with its tag.
        pub ciphertext: Bytes<{ MAX_SHARE_PLAINTEXT + 16 }>,
    }
}

/// `BLAKE3(tag(noura.sync.share.token) || token)`.
pub fn share_token_hash(token: &[u8; 16]) -> Digest {
    hash_parts(&[&Tag::ShareToken.bytes(), token])
}

/// The URL fragment: base64url of the link key and the link's verify key.
pub fn share_fragment(link_key: &SecretKey, verify_key: &Key) -> String {
    let mut bytes = [0u8; 64];
    bytes[..32].copy_from_slice(link_key.expose());
    bytes[32..].copy_from_slice(&verify_key.0);
    to_base64url(&bytes)
}

/// Parses a share URL fragment into the link key and verify key.
pub fn parse_share_fragment(fragment: &str) -> Result<(SecretKey, Key), TextFormError> {
    let bytes: [u8; 64] = from_base64url(fragment)?;
    let mut link_key = [0u8; 32];
    link_key.copy_from_slice(&bytes[..32]);
    let mut verify_key = [0u8; 32];
    verify_key.copy_from_slice(&bytes[32..]);
    Ok((SecretKey(link_key), Key(verify_key)))
}

/// Why a share snapshot failed to verify or open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ShareError {
    /// A bad signature, tampered ciphertext or wrong key.
    #[error("{0}")]
    Crypto(#[from] CryptoError),
    /// Malformed padding.
    #[error("{0}")]
    Padding(#[from] PaddingError),
    /// The record or payload doesn't parse.
    #[error("{0}")]
    Decode(#[from] DecodeError),
    /// The padded snapshot exceeds 960 KiB.
    #[error("share snapshot exceeds 960 KiB")]
    TooLarge,
}

impl Share {
    fn aad(&self) -> Vec<u8> {
        let mut w = Writer::new();
        w.tag(Tag::Share);
        w.put(&self.token_hash);
        w.put(&self.snapshot_revision);
        w.into_bytes()
    }

    /// Encrypts and signs a snapshot.
    pub fn seal(
        token_hash: Digest,
        snapshot_revision: u64,
        payload: &SharePayload,
        link_key: &SecretKey,
        signing_key: &SigningKey,
        nonce: Nonce,
    ) -> Result<super::Signed<Share>, ShareError> {
        let padded = pad(&payload.to_bytes())?;
        if padded.len() > MAX_SHARE_PLAINTEXT {
            return Err(ShareError::TooLarge);
        }
        let mut share = Share {
            token_hash,
            snapshot_revision,
            nonce,
            ciphertext: Bytes::default(),
        };
        let key = derive_key(context::SHARE_KEY, link_key.expose());
        share.ciphertext = Bytes(seal(&key, &nonce, &share.aad(), &padded));
        Ok(super::Signed::sign(share, signing_key))
    }
}

/// Verifies a snapshot's signature with the verify key from the fragment,
/// then decrypts it with the link key, as the viewer in `account-web` does.
pub fn open_share(
    record: &[u8],
    link_key: &SecretKey,
    verify_key: &Key,
) -> Result<(super::Signed<Share>, SharePayload), ShareError> {
    let share = super::Signed::<Share>::from_bytes(record)?;
    share.verify(verify_key)?;
    let key = derive_key(context::SHARE_KEY, link_key.expose());
    let padded = open(
        &key,
        &share.body.nonce,
        &share.body.aad(),
        &share.body.ciphertext,
    )?;
    let payload = SharePayload::from_bytes(unpad(&padded)?)?;
    Ok((share, payload))
}
