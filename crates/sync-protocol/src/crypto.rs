//! The primitives of §3 that this crate uses: BLAKE3 hashing, keyed hashing
//! and key derivation, Ed25519 signatures verified with `verify_strict`,
//! XChaCha20-Poly1305 and the operating system's CSPRNG. X-Wing HPKE lives
//! in `sync-key-envelope`.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use ed25519_dalek::Signer as _;
use zeroize::Zeroizing;

use crate::types::{Digest, Key, Nonce, SecretKey, Signature};

/// The BLAKE3 `derive_key` context strings of §3 and §32.
pub mod context {
    /// Operation payloads, from an epoch key.
    pub const OPERATION_KEY: &str = "noura.sync.v2 operation key";
    /// An epoch record's lineage field, from an epoch key.
    pub const LINEAGE_KEY: &str = "noura.sync.v2 lineage key";
    /// An epoch record's `key_check`, from an epoch key.
    pub const KEY_CHECK: &str = "noura.sync.v2 key check";
    /// Presence payloads and channels, from an epoch key.
    pub const PRESENCE_KEY: &str = "noura.sync.v2 presence key";
    /// The workspace profile, from an epoch key.
    pub const PROFILE_KEY: &str = "noura.sync.v2 profile key";
    /// A share snapshot, from a share link's key.
    pub const SHARE_KEY: &str = "noura.sync.v2 share key";
    /// An invitation context, from an invitation secret.
    pub const INVITATION_KEY: &str = "noura.sync.v2 invitation key";
    /// An invitation proof, from an invitation secret.
    pub const INVITATION_PROOF: &str = "noura.sync.v2 invitation proof";
    /// A recovery kit, from its secret and salt.
    pub const RECOVERY_KIT: &str = "noura.sync.v2 recovery kit";
    /// A blob's nonce prefix, from its key and ID.
    pub const BLOB_NONCE: &str = "noura.sync.v2 blob nonce";
    /// The account ID key, from the server's secret.
    pub const ACCOUNT_ID: &str = "noura.sync.v2 account id";
    /// The server secret's key check.
    pub const SECRET_CHECK: &str = "noura.sync.v2 secret check";
    /// The passkey user handle key, from the server's secret.
    pub const PASSKEY_USER: &str = "noura.sync.v2 passkey user";
    /// The device page's confirmation key, from the server's secret.
    pub const DEVICE_CONFIRMATION: &str = "noura.sync.v2 device confirmation";

    /// Every context, in the order of §32.
    pub const ALL: [&str; 14] = [
        OPERATION_KEY,
        LINEAGE_KEY,
        KEY_CHECK,
        PRESENCE_KEY,
        PROFILE_KEY,
        SHARE_KEY,
        INVITATION_KEY,
        INVITATION_PROOF,
        RECOVERY_KIT,
        BLOB_NONCE,
        ACCOUNT_ID,
        SECRET_CHECK,
        PASSKEY_USER,
        DEVICE_CONFIRMATION,
    ];
}

/// `BLAKE3(data)`.
pub fn hash(data: &[u8]) -> Digest {
    Digest(*blake3::hash(data).as_bytes())
}

/// `BLAKE3` of the concatenation of `parts`.
pub fn hash_parts(parts: &[&[u8]]) -> Digest {
    let mut hasher = blake3::Hasher::new();
    for part in parts {
        hasher.update(part);
    }
    Digest(*hasher.finalize().as_bytes())
}

/// `BLAKE3.keyed_hash(key, data)`.
pub fn keyed_hash(key: &SecretKey, data: &[u8]) -> Digest {
    Digest(*blake3::keyed_hash(key.expose(), data).as_bytes())
}

/// `BLAKE3.derive_key(context, material)`.
pub fn derive_key(context: &str, material: &[u8]) -> SecretKey {
    SecretKey(blake3::derive_key(context, material))
}

/// The operating system's random source failed.
pub use getrandom::Error as RandomError;

/// Fills `dst` from the operating system's CSPRNG.
pub fn fill_random(dst: &mut [u8]) -> Result<(), RandomError> {
    getrandom::fill(dst)
}

/// Fills an array from the operating system's CSPRNG.
pub fn random_array<const N: usize>() -> Result<[u8; N], RandomError> {
    let mut out = [0u8; N];
    fill_random(&mut out)?;
    Ok(out)
}

/// A random 24-byte nonce.
pub fn random_nonce() -> Result<Nonce, getrandom::Error> {
    Ok(Nonce(random_array()?))
}

/// An Ed25519 signing key, held as its 32-byte seed.
#[derive(Clone)]
pub struct SigningKey(ed25519_dalek::SigningKey);

impl SigningKey {
    /// The key whose seed is `seed`.
    pub fn from_seed(seed: &SecretKey) -> Self {
        Self(ed25519_dalek::SigningKey::from_bytes(seed.expose()))
    }

    /// A fresh key from the operating system's CSPRNG.
    pub fn random() -> Result<Self, getrandom::Error> {
        let seed = Zeroizing::new(SecretKey(random_array()?));
        Ok(Self::from_seed(&seed))
    }

    /// The 32-byte seed.
    pub fn seed(&self) -> SecretKey {
        SecretKey(self.0.to_bytes())
    }

    /// The public key.
    pub fn public(&self) -> Key {
        Key(self.0.verifying_key().to_bytes())
    }

    /// Signs `message`.
    pub fn sign(&self, message: &[u8]) -> Signature {
        Signature(self.0.sign(message).to_bytes())
    }
}

impl std::fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "SigningKey({:?})", self.public())
    }
}

/// A failed cryptographic check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum CryptoError {
    /// The signature doesn't verify under the key with `verify_strict`.
    #[error("signature doesn't verify")]
    Signature,
    /// The public key isn't a valid Ed25519 point.
    #[error("invalid public key")]
    PublicKey,
    /// Authenticated decryption failed.
    #[error("decryption failed")]
    Decrypt,
}

/// Verifies an Ed25519 signature with `verify_strict` (§3), which refuses
/// small-order keys and signature points and non-canonical encodings.
pub fn verify(public: &Key, message: &[u8], signature: &Signature) -> Result<(), CryptoError> {
    let key =
        ed25519_dalek::VerifyingKey::from_bytes(&public.0).map_err(|_| CryptoError::PublicKey)?;
    let signature = ed25519_dalek::Signature::from_bytes(&signature.0);
    key.verify_strict(message, &signature)
        .map_err(|_| CryptoError::Signature)
}

/// XChaCha20-Poly1305 encryption; the output ends with the 16-byte tag.
pub fn seal(key: &SecretKey, nonce: &Nonce, aad: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let cipher = XChaCha20Poly1305::new(key.expose().into());
    cipher
        .encrypt(
            &XNonce::from(nonce.0),
            Payload {
                msg: plaintext,
                aad,
            },
        )
        .expect("XChaCha20-Poly1305 encrypts any length this protocol uses")
}

/// XChaCha20-Poly1305 decryption.
pub fn open(
    key: &SecretKey,
    nonce: &Nonce,
    aad: &[u8],
    ciphertext: &[u8],
) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new(key.expose().into());
    cipher
        .decrypt(
            &XNonce::from(nonce.0),
            Payload {
                msg: ciphertext,
                aad,
            },
        )
        .map_err(|_| CryptoError::Decrypt)
}

/// The length of the AEAD tag that every ciphertext carries.
pub const AEAD_TAG_LEN: usize = 16;
