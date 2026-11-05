//! Invitations (§11).

use super::Role;
use crate::codec::{Bytes, Decode, Encode, Text};
use crate::crypto::{CryptoError, context, derive_key, hash_parts, keyed_hash, open, seal};
use crate::padding::{PaddingError, pad, unpad};
use crate::tag::Tag;
use crate::types::{Digest, Id, Nonce, SecretKey};

crate::wire_struct! {
    /// `invite_context`: the invitation's details, encrypted under a subkey
    /// of the secret in the link's fragment.
    pub struct InviteContext: tag InviteContext {
        /// The workspace.
        pub workspace: Id,
        /// The invitation ID.
        pub invitation: Id,
        /// The role the invitation grants.
        pub role: Role,
        /// The nonce.
        pub nonce: Nonce,
        /// The padded payload and its tag.
        pub ciphertext: Bytes,
    }
}

crate::wire_struct! {
    /// The invitation context's padded plaintext.
    pub struct InvitePayload: tag InvitePayload {
        /// The workspace's name.
        pub workspace_name: Text,
        /// The inviter's name.
        pub inviter_name: Text,
        /// The fingerprint of the inviting device's card.
        pub inviter_fingerprint: Digest,
    }
}

/// `BLAKE3(tag(noura.sync.invite.token) || token)`, which the server stores.
pub fn invite_token_hash(token: &[u8; 16]) -> Digest {
    hash_parts(&[&Tag::InviteToken.bytes(), token])
}

/// `invite_proof = keyed_hash(derive_key("noura.sync.v2 invitation proof",
/// secret), card)`, which the inviting device checks before it offers.
pub fn invite_proof(secret: &SecretKey, card: &[u8]) -> Digest {
    keyed_hash(
        &derive_key(context::INVITATION_PROOF, secret.expose()),
        card,
    )
}

/// Why an invitation context failed to open.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum InviteError {
    /// Wrong secret or tampered context.
    #[error("{0}")]
    Decrypt(#[from] CryptoError),
    /// Malformed padding.
    #[error("{0}")]
    Padding(#[from] PaddingError),
    /// The payload doesn't parse.
    #[error("{0}")]
    Decode(#[from] crate::codec::DecodeError),
}

impl InviteContext {
    fn aad(&self) -> Vec<u8> {
        let mut w = crate::codec::Writer::new();
        w.tag(Tag::InviteContext);
        w.put(&self.workspace);
        w.put(&self.invitation);
        w.put(&self.role);
        w.into_bytes()
    }

    /// Encrypts `payload` for the holders of `secret`.
    pub fn seal(
        workspace: Id,
        invitation: Id,
        role: Role,
        payload: &InvitePayload,
        secret: &SecretKey,
        nonce: Nonce,
    ) -> Self {
        let mut context = Self {
            workspace,
            invitation,
            role,
            nonce,
            ciphertext: Bytes::default(),
        };
        let key = derive_key(context::INVITATION_KEY, secret.expose());
        let padded = pad(&payload.to_bytes()).expect("an invitation payload fits a u32 length");
        context.ciphertext = Bytes(seal(&key, &nonce, &context.aad(), &padded));
        context
    }

    /// Decrypts the payload with `secret`.
    pub fn open(&self, secret: &SecretKey) -> Result<InvitePayload, InviteError> {
        let key = derive_key(context::INVITATION_KEY, secret.expose());
        let padded = open(&key, &self.nonce, &self.aad(), &self.ciphertext)?;
        Ok(InvitePayload::from_bytes(unpad(&padded)?)?)
    }
}
