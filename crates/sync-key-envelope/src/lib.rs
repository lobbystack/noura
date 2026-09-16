//! Platform-independent browser key envelopes for Noura encrypted
//! synchronization.
//!
//! This crate implements the portable `noura.sync.key.web` version 1 envelope:
//! X25519 ECDH (RFC 7748), HKDF-SHA256, AES-256-GCM, and Ed25519. It performs no
//! randomness generation so the same code can run on native clients and inside
//! WebAssembly; callers supply the ephemeral secret, salt, and nonce.
//!
//! Managed synchronization stays end-to-end encrypted. This crate only produces
//! and consumes signed ciphertext envelopes and never logs key material.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use thiserror::Error;
use x25519_dalek::{X25519_BASEPOINT_BYTES, x25519};
use zeroize::Zeroizing;

/// Domain string separating browser key envelopes from other signed objects.
pub const DOMAIN: &str = "noura.sync.key.web";

/// Envelope construction version.
pub const VERSION: u8 = 1;

/// HKDF-Expand `info` parameter binding the derived key-encryption key.
pub const INFO: &[u8] = b"noura.sync.key.web.v1";

/// Domain string binding the wrapped object-key plaintext.
pub const OBJECT_KEY_DOMAIN: &str = "noura.sync.object-key";

/// Prefix identifying a raw 32-byte X25519 browser device recipient.
pub const RECIPIENT_PREFIX: &str = "x25519:";

/// Domain string separating browser device fingerprints from native ones.
pub const FINGERPRINT_DOMAIN: &str = "noura.device.card.web";

/// Domain string binding the browser device enrollment proof.
pub const ENROLLMENT_DOMAIN: &str = "noura.device.enroll.web";

/// Largest accepted positive safe integer epoch (`2^53 - 1`).
pub const MAX_EPOCH: u64 = 9_007_199_254_740_991;

/// Largest accepted wrapped-key ciphertext, bounding allocations from untrusted
/// envelope input.
const MAX_WRAPPED_KEY: usize = 4096;

/// Structured failures for browser key envelope handling.
///
/// Messages never contain key material. Match on [`KeyEnvelopeError::code`] to
/// compare against the shared conformance fixtures.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum KeyEnvelopeError {
    /// The envelope is malformed or its construction parameters are unusable.
    #[error("The browser key envelope is invalid")]
    InvalidEnvelope,
    /// The Ed25519 signature does not verify under the trusted signer key.
    #[error("The browser key envelope signature is invalid")]
    InvalidSignature,
    /// The wrapped key could not be decrypted or parsed.
    #[error("The browser key envelope could not be unwrapped")]
    UnwrapFailed,
    /// The epoch is zero or greater than the largest safe integer.
    #[error("The browser key envelope epoch is invalid")]
    InvalidEpoch,
    /// Canonical serialization of an envelope tuple failed.
    #[error("The browser key envelope could not be serialized")]
    SerializeFailed,
    /// A base64 field is not canonical or has the wrong decoded length.
    #[error("The browser key envelope contains invalid base64")]
    InvalidBase64,
    /// An Ed25519 or X25519 key could not be decoded.
    #[error("The browser key envelope contains an invalid key")]
    InvalidKey,
    /// AES-256-GCM wrapping failed.
    #[error("The browser key could not be wrapped")]
    WrapFailed,
    /// The decrypted plaintext does not describe the envelope's object key.
    #[error("The wrapped object key is invalid")]
    InvalidWrappedKey,
    /// A browser device recipient is not a canonical `x25519:` public key.
    #[error("The browser device recipient is invalid")]
    InvalidRecipient,
}

impl KeyEnvelopeError {
    /// Stable public error code for adapters, diagnostics, and conformance fixtures.
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidEnvelope => "sync_invalid_key_envelope",
            Self::InvalidSignature => "sync_invalid_signature",
            Self::UnwrapFailed => "sync_key_unwrap_failed",
            Self::InvalidEpoch => "sync_invalid_epoch",
            Self::SerializeFailed => "sync_serialize_failed",
            Self::InvalidBase64 => "sync_invalid_base64",
            Self::InvalidKey => "sync_invalid_key",
            Self::WrapFailed => "sync_key_wrap_failed",
            Self::InvalidWrappedKey => "sync_invalid_wrapped_key",
            Self::InvalidRecipient => "sync_invalid_recipient",
        }
    }
}

/// Result alias for browser key envelope operations.
pub type Result<T> = std::result::Result<T, KeyEnvelopeError>;

/// A signed, recipient-wrapped object key for a browser device.
///
/// Field names are snake_case on the wire, matching native envelope storage. All
/// byte fields are standard (padded) base64. Unknown fields are rejected.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WebKeyEnvelope {
    /// Stable workspace identifier the object key belongs to.
    #[serde(alias = "workspaceId")]
    pub workspace_id: String,
    /// Stable object identifier the key decrypts.
    #[serde(alias = "objectId")]
    pub object_id: String,
    /// Positive safe integer key epoch.
    pub epoch: u64,
    /// Device identifier of the signer.
    #[serde(alias = "signingDevice")]
    pub signing_device: String,
    /// Device identifier of the intended recipient.
    #[serde(alias = "deviceId")]
    pub device_id: String,
    /// Base64 X25519 recipient public key (32 bytes).
    #[serde(alias = "recipientPublicKey")]
    pub recipient_public_key: String,
    /// Base64 X25519 ephemeral public key (32 bytes).
    #[serde(alias = "ephemeralPublicKey")]
    pub ephemeral_public_key: String,
    /// Base64 HKDF salt (32 bytes).
    pub salt: String,
    /// Base64 AES-256-GCM nonce (12 bytes).
    pub nonce: String,
    /// Base64 AES-256-GCM ciphertext and 16-byte tag.
    #[serde(alias = "wrappedKey")]
    pub wrapped_key: String,
    /// Base64 Ed25519 signature (64 bytes).
    pub signature: String,
}

impl WebKeyEnvelope {
    /// Canonical JSON additional authenticated data binding routing metadata.
    fn aad(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&(
            DOMAIN,
            VERSION,
            &self.workspace_id,
            &self.object_id,
            self.epoch,
            &self.signing_device,
            &self.device_id,
            &self.recipient_public_key,
            &self.ephemeral_public_key,
            &self.salt,
            &self.nonce,
        ))
        .map_err(|_| KeyEnvelopeError::SerializeFailed)
    }

    /// Canonical JSON signing tuple covering the complete envelope.
    fn signing_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&(
            DOMAIN,
            VERSION,
            &self.workspace_id,
            &self.object_id,
            self.epoch,
            &self.signing_device,
            &self.device_id,
            &self.recipient_public_key,
            &self.ephemeral_public_key,
            &self.salt,
            &self.nonce,
            &self.wrapped_key,
        ))
        .map_err(|_| KeyEnvelopeError::SerializeFailed)
    }
}

/// Wrap an object key to a browser device's X25519 public key and sign it.
///
/// The caller supplies the ephemeral secret, HKDF salt, and AES-GCM nonce so
/// this crate performs no randomness generation and stays WebAssembly-portable.
/// The Ed25519 `signing_secret` signs the envelope; `signing_device` names it.
#[expect(
    clippy::too_many_arguments,
    reason = "The wrap API mirrors every field of the signed envelope tuple and caller-supplied cryptographic inputs"
)]
pub fn wrap_key(
    workspace_id: &str,
    object_id: &str,
    epoch: u64,
    signing_device: &str,
    signing_secret: [u8; 32],
    device_id: &str,
    recipient_public: [u8; 32],
    object_key: [u8; 32],
    ephemeral_secret: [u8; 32],
    salt: [u8; 32],
    nonce: [u8; 12],
) -> Result<WebKeyEnvelope> {
    validate_epoch(epoch)?;
    let signing_secret = Zeroizing::new(signing_secret);
    let object_key = Zeroizing::new(object_key);
    let ephemeral_secret = Zeroizing::new(ephemeral_secret);

    let shared = Zeroizing::new(x25519(*ephemeral_secret, recipient_public));
    if *shared == [0_u8; 32] {
        return Err(KeyEnvelopeError::InvalidEnvelope);
    }
    let key = derive_key(&salt, &shared)?;

    let mut envelope = WebKeyEnvelope {
        workspace_id: workspace_id.into(),
        object_id: object_id.into(),
        epoch,
        signing_device: signing_device.into(),
        device_id: device_id.into(),
        recipient_public_key: STANDARD.encode(recipient_public),
        ephemeral_public_key: STANDARD.encode(x25519(*ephemeral_secret, X25519_BASEPOINT_BYTES)),
        salt: STANDARD.encode(salt),
        nonce: STANDARD.encode(nonce),
        wrapped_key: String::new(),
        signature: String::new(),
    };

    let key_base64 = Zeroizing::new(STANDARD.encode(*object_key));
    let plaintext = Zeroizing::new(
        serde_json::to_vec(&(
            OBJECT_KEY_DOMAIN,
            VERSION,
            workspace_id,
            object_id,
            epoch,
            key_base64.as_str(),
        ))
        .map_err(|_| KeyEnvelopeError::SerializeFailed)?,
    );
    let aad = envelope.aad()?;
    envelope.wrapped_key = STANDARD.encode(
        Aes256Gcm::new((&*key).into())
            .encrypt(
                (&nonce).into(),
                Payload {
                    msg: &plaintext,
                    aad: &aad,
                },
            )
            .map_err(|_| KeyEnvelopeError::WrapFailed)?,
    );

    envelope.signature = STANDARD.encode(
        SigningKey::from_bytes(&signing_secret)
            .sign(&envelope.signing_bytes()?)
            .to_bytes(),
    );
    Ok(envelope)
}

/// Verify an envelope's signature against a previously trusted signer key.
pub fn verify_envelope(envelope: &WebKeyEnvelope, trusted_signer_public: [u8; 32]) -> Result<()> {
    validate_epoch(envelope.epoch)?;
    decode_fixed(&envelope.recipient_public_key, 32)?;
    decode_fixed(&envelope.ephemeral_public_key, 32)?;
    decode_fixed(&envelope.salt, 32)?;
    decode_fixed(&envelope.nonce, 12)?;
    decode_range(&envelope.wrapped_key, 16, MAX_WRAPPED_KEY)?;
    let signature = Signature::from_slice(&decode_fixed(&envelope.signature, 64)?)
        .map_err(|_| KeyEnvelopeError::InvalidSignature)?;
    VerifyingKey::from_bytes(&trusted_signer_public)
        .map_err(|_| KeyEnvelopeError::InvalidKey)?
        .verify_strict(&envelope.signing_bytes()?, &signature)
        .map_err(|_| KeyEnvelopeError::InvalidSignature)
}

/// Verify then unwrap an object key. Verification always precedes decryption.
///
/// The returned key zeroizes on drop. `trusted_signer_public` must come from
/// local trust, never from the server that returned the envelope.
pub fn unwrap_key(
    envelope: &WebKeyEnvelope,
    recipient_secret: [u8; 32],
    trusted_signer_public: [u8; 32],
) -> Result<Zeroizing<[u8; 32]>> {
    verify_envelope(envelope, trusted_signer_public)?;
    let recipient_secret = Zeroizing::new(recipient_secret);

    let ephemeral_public: [u8; 32] = decode_fixed(&envelope.ephemeral_public_key, 32)?
        .try_into()
        .map_err(|_| KeyEnvelopeError::InvalidKey)?;
    let salt: [u8; 32] = decode_fixed(&envelope.salt, 32)?
        .try_into()
        .map_err(|_| KeyEnvelopeError::InvalidKey)?;
    let nonce: [u8; 12] = decode_fixed(&envelope.nonce, 12)?
        .try_into()
        .map_err(|_| KeyEnvelopeError::InvalidKey)?;
    let ciphertext = decode_range(&envelope.wrapped_key, 16, MAX_WRAPPED_KEY)?;

    let shared = Zeroizing::new(x25519(*recipient_secret, ephemeral_public));
    if *shared == [0_u8; 32] {
        return Err(KeyEnvelopeError::UnwrapFailed);
    }
    let key = derive_key(&salt, &shared)?;
    let aad = envelope.aad()?;
    let plaintext = Zeroizing::new(
        Aes256Gcm::new((&*key).into())
            .decrypt(
                (&nonce).into(),
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| KeyEnvelopeError::UnwrapFailed)?,
    );

    let (domain, version, workspace_id, object_id, epoch, encoded): (
        String,
        u8,
        String,
        String,
        u64,
        String,
    ) = serde_json::from_slice(&plaintext).map_err(|_| KeyEnvelopeError::InvalidWrappedKey)?;
    if domain != OBJECT_KEY_DOMAIN
        || version != VERSION
        || workspace_id != envelope.workspace_id
        || object_id != envelope.object_id
        || epoch != envelope.epoch
    {
        return Err(KeyEnvelopeError::InvalidWrappedKey);
    }
    let encoded = Zeroizing::new(encoded);
    let bytes = Zeroizing::new(decode_fixed(&encoded, 32)?);
    let key: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| KeyEnvelopeError::InvalidKey)?;
    Ok(Zeroizing::new(key))
}

/// Encode a raw X25519 public key as a browser device recipient string.
///
/// The encoding is `x25519:` followed by standard (padded) base64 of the
/// 32-byte public key. It is the browser counterpart to an `age` recipient and
/// is bound into the browser device fingerprint.
pub fn encode_recipient(public: [u8; 32]) -> String {
    format!("{RECIPIENT_PREFIX}{}", STANDARD.encode(public))
}

/// Decode a browser device recipient string into its raw X25519 public key.
///
/// Only `x25519:` followed by canonical standard base64 of exactly 32 bytes is
/// accepted. Every other form (a missing prefix, an `age1` recipient, a
/// non-canonical encoding, or the wrong decoded length) is rejected with
/// [`KeyEnvelopeError::InvalidRecipient`].
pub fn decode_recipient(recipient: &str) -> Result<[u8; 32]> {
    let encoded = recipient
        .strip_prefix(RECIPIENT_PREFIX)
        .ok_or(KeyEnvelopeError::InvalidRecipient)?;
    let bytes = decode_fixed(encoded, 32).map_err(|_| KeyEnvelopeError::InvalidRecipient)?;
    bytes
        .try_into()
        .map_err(|_| KeyEnvelopeError::InvalidRecipient)
}

/// Compute the browser device fingerprint for a raw X25519 recipient.
///
/// The fingerprint is lowercase hexadecimal BLAKE3 over the canonical JSON tuple
/// `[FINGERPRINT_DOMAIN, 1, device_id, account_id, base64(signing_public),
/// recipient]`. It mirrors the native `noura.device.card` fingerprint, differing
/// only in the domain string and the raw X25519 recipient representation. The
/// recipient is bound as given; call [`decode_recipient`] first when it must be
/// validated.
pub fn device_fingerprint(
    device_id: &str,
    account_id: &str,
    signing_public: [u8; 32],
    recipient: &str,
) -> String {
    let bytes = canonical_tuple(&(
        FINGERPRINT_DOMAIN,
        1_u8,
        device_id,
        account_id,
        STANDARD.encode(signing_public),
        recipient,
    ));
    blake3::hash(&bytes).to_hex().to_string()
}

/// Sign the browser device enrollment tuple and return the raw signature.
///
/// The signed message is the canonical JSON tuple
/// `[ENROLLMENT_DOMAIN, 1, origin, account_id, device_id,
/// base64(signing_public), recipient, challenge]`. `signing_secret` is the
/// 32-byte Ed25519 seed and `signing_public` is its verifying key, included so
/// the server can bind the public key exactly as native enrollment does. The
/// recipient is bound as given; call [`decode_recipient`] first when it must be
/// validated.
pub fn enrollment_proof(
    signing_secret: [u8; 32],
    origin: &str,
    account_id: &str,
    device_id: &str,
    signing_public: [u8; 32],
    recipient: &str,
    challenge: &str,
) -> Vec<u8> {
    let signing_secret = Zeroizing::new(signing_secret);
    let bytes = canonical_tuple(&(
        ENROLLMENT_DOMAIN,
        1_u8,
        origin,
        account_id,
        device_id,
        STANDARD.encode(signing_public),
        recipient,
        challenge,
    ));
    SigningKey::from_bytes(&signing_secret)
        .sign(&bytes)
        .to_bytes()
        .to_vec()
}

/// Serialize a canonical JSON tuple of strings and small integers.
///
/// Every value in the device tuples is a string or a small integer, which
/// `serde_json` always serializes, so this cannot fail in practice.
fn canonical_tuple<T: Serialize>(values: &T) -> Vec<u8> {
    serde_json::to_vec(values).expect("canonical JSON tuple of strings and integers cannot fail")
}

/// Derive the AES-256-GCM key with HKDF-SHA256 over the ECDH shared secret.
fn derive_key(salt: &[u8; 32], shared: &[u8; 32]) -> Result<Zeroizing<[u8; 32]>> {
    let hkdf = hkdf::Hkdf::<Sha256>::new(Some(salt), shared);
    let mut key = Zeroizing::new([0_u8; 32]);
    hkdf.expand(INFO, &mut *key)
        .map_err(|_| KeyEnvelopeError::InvalidEnvelope)?;
    Ok(key)
}

/// Reject zero and unsafe-integer epochs before any cryptographic work.
fn validate_epoch(epoch: u64) -> Result<()> {
    if epoch == 0 || epoch > MAX_EPOCH {
        return Err(KeyEnvelopeError::InvalidEpoch);
    }
    Ok(())
}

/// Decode a base64 field that must contain exactly `len` bytes.
fn decode_fixed(value: &str, len: usize) -> Result<Vec<u8>> {
    decode_range(value, len, len)
}

/// Decode canonical padded base64 within an inclusive byte-length range.
fn decode_range(value: &str, min: usize, max: usize) -> Result<Vec<u8>> {
    if value.len() > max.div_ceil(3) * 4 {
        return Err(KeyEnvelopeError::InvalidBase64);
    }
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| KeyEnvelopeError::InvalidBase64)?;
    if bytes.len() < min || bytes.len() > max || STANDARD.encode(&bytes) != value {
        return Err(KeyEnvelopeError::InvalidBase64);
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::path::PathBuf;

    const FIXTURE: &str =
        include_str!("../../../docs/workspace-format/fixtures/browser-key-v1.json");

    const DEVICE_FIXTURE: &str =
        include_str!("../../../docs/workspace-format/fixtures/browser-device-v1.json");

    #[derive(Deserialize)]
    struct Fixtures {
        domain: String,
        version: u8,
        info: String,
        object_key_domain: String,
        valid: Vec<ValidVector>,
        invalid: Vec<InvalidVector>,
    }

    #[derive(Deserialize)]
    struct ValidVector {
        name: String,
        inputs: Inputs,
        trusted_signer_public: String,
        recipient_secret: String,
        expected_envelope: WebKeyEnvelope,
    }

    #[derive(Deserialize)]
    struct Inputs {
        workspace_id: String,
        object_id: String,
        epoch: u64,
        signing_device: String,
        device_id: String,
        signing_secret: String,
        recipient_public: String,
        object_key: String,
        ephemeral_secret: String,
        salt: String,
        nonce: String,
    }

    #[derive(Deserialize)]
    struct InvalidVector {
        name: String,
        operation: String,
        #[serde(default)]
        inputs: Option<Inputs>,
        #[serde(default)]
        trusted_signer_public: Option<String>,
        #[serde(default)]
        recipient_secret: Option<String>,
        #[serde(default)]
        envelope: Option<WebKeyEnvelope>,
        expected_error: String,
    }

    fn array<const N: usize>(value: &str) -> [u8; N] {
        STANDARD
            .decode(value)
            .unwrap()
            .try_into()
            .unwrap_or_else(|_| panic!("expected {N} decoded bytes for {value}"))
    }

    fn wrap(inputs: &Inputs) -> Result<WebKeyEnvelope> {
        wrap_key(
            &inputs.workspace_id,
            &inputs.object_id,
            inputs.epoch,
            &inputs.signing_device,
            array::<32>(&inputs.signing_secret),
            &inputs.device_id,
            array::<32>(&inputs.recipient_public),
            array::<32>(&inputs.object_key),
            array::<32>(&inputs.ephemeral_secret),
            array::<32>(&inputs.salt),
            array::<12>(&inputs.nonce),
        )
    }

    #[test]
    fn browser_key_fixtures_match_wrap_verify_and_unwrap() {
        let fixtures: Fixtures = serde_json::from_str(FIXTURE).unwrap();
        assert_eq!(fixtures.domain, DOMAIN);
        assert_eq!(fixtures.version, VERSION);
        assert_eq!(fixtures.info.as_bytes(), INFO);
        assert_eq!(fixtures.object_key_domain, OBJECT_KEY_DOMAIN);
        assert!(!fixtures.valid.is_empty() && !fixtures.invalid.is_empty());

        for vector in &fixtures.valid {
            let envelope = wrap(&vector.inputs).unwrap();
            assert_eq!(envelope, vector.expected_envelope, "{}", vector.name);
            let trusted = array::<32>(&vector.trusted_signer_public);
            verify_envelope(&envelope, trusted).unwrap();
            let unwrapped =
                unwrap_key(&envelope, array::<32>(&vector.recipient_secret), trusted).unwrap();
            assert_eq!(
                unwrapped.as_slice(),
                array::<32>(&vector.inputs.object_key).as_slice(),
                "{}",
                vector.name
            );
        }

        for vector in &fixtures.invalid {
            let error = match vector.operation.as_str() {
                "wrap" => wrap(vector.inputs.as_ref().unwrap()).unwrap_err(),
                "verify" => verify_envelope(
                    vector.envelope.as_ref().unwrap(),
                    array::<32>(vector.trusted_signer_public.as_deref().unwrap()),
                )
                .unwrap_err(),
                "unwrap" => unwrap_key(
                    vector.envelope.as_ref().unwrap(),
                    array::<32>(vector.recipient_secret.as_deref().unwrap()),
                    array::<32>(vector.trusted_signer_public.as_deref().unwrap()),
                )
                .unwrap_err(),
                other => panic!("unknown fixture operation {other}"),
            };
            assert_eq!(error.code(), vector.expected_error, "{}", vector.name);
        }
    }

    #[derive(Deserialize)]
    struct DeviceFixtures {
        recipient_prefix: String,
        recipients: RecipientsSection,
        fingerprint: FingerprintSection,
        enrollment: EnrollmentSection,
    }

    #[derive(Deserialize)]
    struct RecipientsSection {
        valid: Vec<RecipientVector>,
        invalid: Vec<InvalidRecipientVector>,
    }

    #[derive(Deserialize)]
    struct RecipientVector {
        name: String,
        public_key: String,
        recipient: String,
    }

    #[derive(Deserialize)]
    struct InvalidRecipientVector {
        name: String,
        recipient: String,
        expected_error: String,
    }

    #[derive(Deserialize)]
    struct FingerprintSection {
        domain: String,
        version: u8,
        vectors: Vec<FingerprintVector>,
    }

    #[derive(Deserialize)]
    struct FingerprintVector {
        name: String,
        device_id: String,
        account_id: String,
        signing_public: String,
        recipient: String,
        expected_hex: String,
    }

    #[derive(Deserialize)]
    struct EnrollmentSection {
        domain: String,
        version: u8,
        vectors: Vec<EnrollmentVector>,
    }

    #[derive(Deserialize)]
    struct EnrollmentVector {
        name: String,
        origin: String,
        account_id: String,
        device_id: String,
        signing_secret: String,
        signing_public: String,
        recipient: String,
        challenge: String,
        expected_base64: String,
        expected_hex: String,
    }

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|byte| format!("{byte:02x}")).collect()
    }

    #[test]
    fn browser_device_fixtures_match_recipient_fingerprint_and_enrollment() {
        let fixtures: DeviceFixtures = serde_json::from_str(DEVICE_FIXTURE).unwrap();
        assert_eq!(fixtures.recipient_prefix, RECIPIENT_PREFIX);
        assert_eq!(fixtures.fingerprint.domain, FINGERPRINT_DOMAIN);
        assert_eq!(fixtures.fingerprint.version, 1);
        assert_eq!(fixtures.enrollment.domain, ENROLLMENT_DOMAIN);
        assert_eq!(fixtures.enrollment.version, 1);
        assert!(!fixtures.recipients.valid.is_empty());
        assert!(!fixtures.recipients.invalid.is_empty());
        assert!(!fixtures.fingerprint.vectors.is_empty());
        assert!(!fixtures.enrollment.vectors.is_empty());

        for vector in &fixtures.recipients.valid {
            let public = array::<32>(&vector.public_key);
            assert_eq!(
                encode_recipient(public),
                vector.recipient,
                "{}",
                vector.name
            );
            assert_eq!(
                decode_recipient(&vector.recipient).unwrap(),
                public,
                "{}",
                vector.name
            );
        }

        for vector in &fixtures.recipients.invalid {
            assert_eq!(
                decode_recipient(&vector.recipient).unwrap_err().code(),
                vector.expected_error,
                "{}",
                vector.name
            );
        }

        for vector in &fixtures.fingerprint.vectors {
            assert_eq!(
                device_fingerprint(
                    &vector.device_id,
                    &vector.account_id,
                    array::<32>(&vector.signing_public),
                    &vector.recipient,
                ),
                vector.expected_hex,
                "{}",
                vector.name
            );
        }

        for vector in &fixtures.enrollment.vectors {
            let proof = enrollment_proof(
                array::<32>(&vector.signing_secret),
                &vector.origin,
                &vector.account_id,
                &vector.device_id,
                array::<32>(&vector.signing_public),
                &vector.recipient,
                &vector.challenge,
            );
            assert_eq!(
                STANDARD.encode(&proof),
                vector.expected_base64,
                "{}",
                vector.name
            );
            assert_eq!(hex(&proof), vector.expected_hex, "{}", vector.name);
        }
    }

    #[test]
    #[ignore = "regenerates docs/workspace-format/fixtures/browser-device-v1.json"]
    fn regenerate_browser_device_fixture() {
        let signing_secret = [0x01_u8; 32];
        let signing_public = SigningKey::from_bytes(&signing_secret)
            .verifying_key()
            .to_bytes();
        let recipient_public = [0x07_u8; 32];
        let zero_recipient_public = [0x00_u8; 32];
        let recipient = encode_recipient(recipient_public);
        let device_id = "device_browser";
        let account_id = "account_owner";
        let origin = "https://app.noura.example";
        let challenge = "challenge_browser_device_1";

        let fingerprint = device_fingerprint(device_id, account_id, signing_public, &recipient);
        let proof = enrollment_proof(
            signing_secret,
            origin,
            account_id,
            device_id,
            signing_public,
            &recipient,
            challenge,
        );

        let document = json!({
            "recipient_prefix": RECIPIENT_PREFIX,
            "recipients": {
                "valid": [
                    {
                        "name": "raw-recipient-zero",
                        "public_key": STANDARD.encode(zero_recipient_public),
                        "recipient": encode_recipient(zero_recipient_public),
                    },
                    {
                        "name": "raw-recipient-seeded",
                        "public_key": STANDARD.encode(recipient_public),
                        "recipient": recipient,
                    },
                ],
                "invalid": [
                    {
                        "name": "missing_prefix",
                        "recipient": STANDARD.encode(recipient_public),
                        "expected_error": KeyEnvelopeError::InvalidRecipient.code(),
                    },
                    {
                        "name": "wrong_base64",
                        "recipient": "x25519:not-base64!!",
                        "expected_error": KeyEnvelopeError::InvalidRecipient.code(),
                    },
                    {
                        "name": "short_31_bytes",
                        "recipient": format!("x25519:{}", STANDARD.encode([0x07_u8; 31])),
                        "expected_error": KeyEnvelopeError::InvalidRecipient.code(),
                    },
                    {
                        "name": "long_33_bytes",
                        "recipient": format!("x25519:{}", STANDARD.encode([0x07_u8; 33])),
                        "expected_error": KeyEnvelopeError::InvalidRecipient.code(),
                    },
                    {
                        "name": "age_form",
                        "recipient": format!("age1{}", "q".repeat(58)),
                        "expected_error": KeyEnvelopeError::InvalidRecipient.code(),
                    },
                ],
            },
            "fingerprint": {
                "domain": FINGERPRINT_DOMAIN,
                "version": 1,
                "vectors": [{
                    "name": "browser-device-card",
                    "device_id": device_id,
                    "account_id": account_id,
                    "signing_public": STANDARD.encode(signing_public),
                    "recipient": recipient,
                    "expected_hex": fingerprint,
                }],
            },
            "enrollment": {
                "domain": ENROLLMENT_DOMAIN,
                "version": 1,
                "vectors": [{
                    "name": "browser-device-enroll",
                    "origin": origin,
                    "account_id": account_id,
                    "device_id": device_id,
                    "signing_secret": STANDARD.encode(signing_secret),
                    "signing_public": STANDARD.encode(signing_public),
                    "recipient": recipient,
                    "challenge": challenge,
                    "expected_base64": STANDARD.encode(&proof),
                    "expected_hex": hex(&proof),
                }],
            },
        });

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/workspace-format/fixtures/browser-device-v1.json");
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&document).unwrap()),
        )
        .unwrap();
    }

    fn resign(envelope: &mut WebKeyEnvelope, signing_secret: &[u8; 32]) {
        let signature = SigningKey::from_bytes(signing_secret)
            .sign(&envelope.signing_bytes().unwrap())
            .to_bytes();
        envelope.signature = STANDARD.encode(signature);
    }

    #[test]
    #[ignore = "regenerates docs/workspace-format/fixtures/browser-key-v1.json"]
    fn regenerate_browser_key_fixture() {
        let workspace_id = "workspace";
        let object_id = "object";
        let epoch = 1_u64;
        let signing_device = "device_signer";
        let device_id = "device_browser";
        let signing_secret = [0x01_u8; 32];
        let recipient_secret = [0x02_u8; 32];
        let recipient_public = x25519(recipient_secret, X25519_BASEPOINT_BYTES);
        let object_key = [0x03_u8; 32];
        let ephemeral_secret = [0x04_u8; 32];
        let salt = [0x05_u8; 32];
        let nonce = [0x06_u8; 12];
        let signing_public = SigningKey::from_bytes(&signing_secret)
            .verifying_key()
            .to_bytes();

        let envelope = wrap_key(
            workspace_id,
            object_id,
            epoch,
            signing_device,
            signing_secret,
            device_id,
            recipient_public,
            object_key,
            ephemeral_secret,
            salt,
            nonce,
        )
        .unwrap();
        let inputs = json!({
            "workspace_id": workspace_id,
            "object_id": object_id,
            "epoch": epoch,
            "signing_device": signing_device,
            "device_id": device_id,
            "signing_secret": STANDARD.encode(signing_secret),
            "recipient_public": STANDARD.encode(recipient_public),
            "object_key": STANDARD.encode(object_key),
            "ephemeral_secret": STANDARD.encode(ephemeral_secret),
            "salt": STANDARD.encode(salt),
            "nonce": STANDARD.encode(nonce),
        });
        let mut low_order = inputs.clone();
        low_order["recipient_public"] = json!(STANDARD.encode([0_u8; 32]));
        let mut zero_epoch = inputs.clone();
        zero_epoch["epoch"] = json!(0);

        let mut tampered_signature = envelope.clone();
        let mut signature = STANDARD.decode(&envelope.signature).unwrap();
        signature[0] ^= 0xFF;
        tampered_signature.signature = STANDARD.encode(signature);

        let mut tampered_wrapped_key = envelope.clone();
        let mut wrapped_key = STANDARD.decode(&envelope.wrapped_key).unwrap();
        let last = wrapped_key.len() - 1;
        wrapped_key[last] ^= 0x01;
        tampered_wrapped_key.wrapped_key = STANDARD.encode(wrapped_key);
        resign(&mut tampered_wrapped_key, &signing_secret);

        let mut tampered_aad_field = envelope.clone();
        tampered_aad_field.device_id = "device_browser_other".into();
        resign(&mut tampered_aad_field, &signing_secret);

        let mut bad_base64 = envelope.clone();
        bad_base64.signature = "not-base64!!".into();

        let mut wrong_length = envelope.clone();
        wrong_length.recipient_public_key = STANDARD.encode([0x0A_u8; 31]);

        let document = json!({
            "domain": DOMAIN,
            "version": VERSION,
            "info": std::str::from_utf8(INFO).unwrap(),
            "object_key_domain": OBJECT_KEY_DOMAIN,
            "valid": [{
                "name": "browser-key-v1-basic",
                "inputs": inputs,
                "trusted_signer_public": STANDARD.encode(signing_public),
                "recipient_secret": STANDARD.encode(recipient_secret),
                "expected_envelope": envelope,
            }],
            "invalid": [
                {
                    "name": "tampered_signature",
                    "operation": "verify",
                    "trusted_signer_public": STANDARD.encode(signing_public),
                    "envelope": tampered_signature,
                    "expected_error": KeyEnvelopeError::InvalidSignature.code(),
                },
                {
                    "name": "tampered_wrapped_key",
                    "operation": "unwrap",
                    "trusted_signer_public": STANDARD.encode(signing_public),
                    "recipient_secret": STANDARD.encode(recipient_secret),
                    "envelope": tampered_wrapped_key,
                    "expected_error": KeyEnvelopeError::UnwrapFailed.code(),
                },
                {
                    "name": "tampered_aad_field",
                    "operation": "unwrap",
                    "trusted_signer_public": STANDARD.encode(signing_public),
                    "recipient_secret": STANDARD.encode(recipient_secret),
                    "envelope": tampered_aad_field,
                    "expected_error": KeyEnvelopeError::UnwrapFailed.code(),
                },
                {
                    "name": "wrong_recipient_secret",
                    "operation": "unwrap",
                    "trusted_signer_public": STANDARD.encode(signing_public),
                    "recipient_secret": STANDARD.encode([0x09_u8; 32]),
                    "envelope": envelope.clone(),
                    "expected_error": KeyEnvelopeError::UnwrapFailed.code(),
                },
                {
                    "name": "low_order_recipient_public",
                    "operation": "wrap",
                    "inputs": low_order,
                    "expected_error": KeyEnvelopeError::InvalidEnvelope.code(),
                },
                {
                    "name": "bad_base64",
                    "operation": "verify",
                    "trusted_signer_public": STANDARD.encode(signing_public),
                    "envelope": bad_base64,
                    "expected_error": KeyEnvelopeError::InvalidBase64.code(),
                },
                {
                    "name": "wrong_length",
                    "operation": "verify",
                    "trusted_signer_public": STANDARD.encode(signing_public),
                    "envelope": wrong_length,
                    "expected_error": KeyEnvelopeError::InvalidBase64.code(),
                },
                {
                    "name": "epoch_zero",
                    "operation": "wrap",
                    "inputs": zero_epoch,
                    "expected_error": KeyEnvelopeError::InvalidEpoch.code(),
                },
            ],
        });

        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/workspace-format/fixtures/browser-key-v1.json");
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&document).unwrap()),
        )
        .unwrap();
    }
}
