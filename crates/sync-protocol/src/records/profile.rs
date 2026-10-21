//! The workspace profile (§25).

use crate::codec::{Bytes, Decode, DecodeError, Encode, Reader, Text, Writer};
use crate::crypto::{CryptoError, context, derive_key, open, seal};
use crate::padding::{PaddingError, pad, unpad};
use crate::tag::Tag;
use crate::types::{Digest, Id, Nonce, SecretKey};

crate::wire_struct! {
    /// The profile's padded plaintext.
    pub struct ProfilePayload: tag ProfilePayload {
        /// The workspace's display name.
        pub name: Text,
    }
}

/// The signed body of a workspace profile.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct Profile {
    /// The workspace.
    pub workspace: Id,
    /// 1 at creation, then +1.
    pub profile_revision: u64,
    /// The workspace domain's epoch whose profile subkey encrypts it.
    pub epoch: u64,
    /// The policy revision the signer saw.
    pub policy_revision: u64,
    /// The digest of the change at `policy_revision`.
    pub policy_head: Digest,
    /// The signing device.
    pub signer: Id,
    /// The nonce.
    pub nonce: Nonce,
    /// The padded payload and its tag.
    pub ciphertext: Bytes,
}

/// Why a profile failed to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProfileError {
    /// Decryption failed.
    #[error("{0}")]
    Decrypt(#[from] CryptoError),
    /// Malformed padding.
    #[error("{0}")]
    Padding(#[from] PaddingError),
    /// The payload doesn't parse.
    #[error("{0}")]
    Decode(#[from] DecodeError),
}

impl Profile {
    /// The AEAD associated data: the prefix through `signer`, then the
    /// epoch record's digest.
    pub fn aad(&self, epoch_record: &Digest) -> Vec<u8> {
        let mut w = Writer::new();
        self.encode_prefix(&mut w);
        w.put(epoch_record);
        w.into_bytes()
    }

    fn encode_prefix(&self, w: &mut Writer) {
        w.tag(Tag::Profile);
        w.put(&self.workspace);
        w.put(&self.profile_revision);
        w.put(&self.epoch);
        w.put(&self.policy_revision);
        w.put(&self.policy_head);
        w.put(&self.signer);
    }

    /// Encrypts `payload` under the profile subkey of `epoch_key`.
    pub fn seal_payload(
        &mut self,
        payload: &ProfilePayload,
        epoch_key: &SecretKey,
        epoch_record: &Digest,
        nonce: Nonce,
    ) {
        let key = derive_key(context::PROFILE_KEY, epoch_key.expose());
        let padded = pad(&payload.to_bytes()).expect("a profile fits a u32 length");
        self.nonce = nonce;
        self.ciphertext = Bytes(seal(&key, &nonce, &self.aad(epoch_record), &padded));
    }

    /// Decrypts the payload.
    pub fn open_payload(
        &self,
        epoch_key: &SecretKey,
        epoch_record: &Digest,
    ) -> Result<ProfilePayload, ProfileError> {
        let key = derive_key(context::PROFILE_KEY, epoch_key.expose());
        let padded = open(&key, &self.nonce, &self.aad(epoch_record), &self.ciphertext)?;
        Ok(ProfilePayload::from_bytes(unpad(&padded)?)?)
    }
}

impl Encode for Profile {
    fn encode(&self, w: &mut Writer) {
        self.encode_prefix(w);
        w.put(&self.nonce);
        w.put(&self.ciphertext);
    }
}

impl Decode for Profile {
    const MIN_SIZE: usize = Tag::Profile.encoded_len() + 16 + 8 * 3 + 32 + 16 + 24 + 4;
    fn decode(r: &mut Reader<'_>) -> Result<Self, DecodeError> {
        r.tag(Tag::Profile)?;
        Ok(Self {
            workspace: r.get()?,
            profile_revision: r.get()?,
            epoch: r.get()?,
            policy_revision: r.get()?,
            policy_head: r.get()?,
            signer: r.get()?,
            nonce: r.get()?,
            ciphertext: r.get()?,
        })
    }
}
