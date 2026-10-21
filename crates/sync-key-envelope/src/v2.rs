//! Version 2 (docs/workspace-format/sync-v2.md): device keys, cards,
//! enrollment proofs, account chain links and X-Wing key envelopes, built on
//! `sync-protocol`'s encoder so both crates use one encoding.
//!
//! A device holds an Ed25519 signing key and an X-Wing HPKE key pair, whose
//! secret is its 32-byte seed (§7). Envelopes wrap one epoch key for one
//! device with HPKE base mode: X-Wing (`0x647a`), HKDF-SHA256 (`0x0001`) and
//! ChaCha20Poly1305 (`0x0003`), with the envelope prefix as `info` and empty
//! `aad` (§9).

use hpke::aead::ChaCha20Poly1305;
use hpke::kdf::HkdfSha256;
use hpke::kem::XWing;
use hpke::{Deserializable, Kem as _, OpModeR, OpModeS, Serializable};
use rand_core::CryptoRng;
use sync_protocol::codec::{Decode, ExactBytes, Nested, Text};
use sync_protocol::crypto::{RandomError, SigningKey, random_array};
use sync_protocol::records::{
    AccountHead, AccountLink, Card, ENCAPSULATED_KEY_LEN, ENVELOPE_CIPHERTEXT_LEN, Enroll,
    Envelope, LinkKind, Signed,
};
use sync_protocol::types::{Digest, Id, Key, SecretKey};
use zeroize::Zeroizing;

/// The HPKE suite of §3: X-Wing, HKDF-SHA256, ChaCha20Poly1305.
pub const SUITE: (u16, u16, u16) = (0x647a, 0x0001, 0x0003);

/// A device's secret keys: its Ed25519 signing key and its X-Wing seed.
#[derive(Clone)]
pub struct DeviceKeys {
    signing: SigningKey,
    hpke_seed: SecretKey,
}

impl std::fmt::Debug for DeviceKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DeviceKeys({:?})", self.signing.public())
    }
}

impl DeviceKeys {
    /// Fresh keys from the operating system's CSPRNG.
    pub fn generate() -> Result<Self, RandomError> {
        let signing_seed = Zeroizing::new(SecretKey(random_array()?));
        let hpke_seed = SecretKey(random_array()?);
        Ok(Self::from_seeds(&signing_seed, hpke_seed))
    }

    /// The keys whose Ed25519 seed and X-Wing seed are given, as a
    /// credential store or a recovery kit holds them.
    pub fn from_seeds(signing_seed: &SecretKey, hpke_seed: SecretKey) -> Self {
        Self {
            signing: SigningKey::from_seed(signing_seed),
            hpke_seed,
        }
    }

    /// The signing key.
    pub fn signing(&self) -> &SigningKey {
        &self.signing
    }

    /// The X-Wing seed.
    pub fn hpke_seed(&self) -> &SecretKey {
        &self.hpke_seed
    }

    /// The device ID, derived from the signing key.
    pub fn device(&self) -> Id {
        sync_protocol::ids::device_id(&self.signing.public())
    }

    fn hpke_private(&self) -> <XWing as hpke::Kem>::PrivateKey {
        <XWing as hpke::Kem>::PrivateKey::from_bytes(self.hpke_seed.expose())
            .expect("an X-Wing seed is 32 bytes")
    }

    /// The 1,216-byte X-Wing public key.
    pub fn hpke_public(&self) -> Vec<u8> {
        XWing::sk_to_pk(&self.hpke_private()).to_bytes().to_vec()
    }

    /// The device's card for `account`.
    pub fn card(&self, account: Id) -> Card {
        Card::new(account, self.signing.public(), self.hpke_public())
            .expect("an X-Wing public key is 1,216 bytes")
    }

    /// Whether `card` binds these keys.
    pub fn matches(&self, card: &Card) -> bool {
        card.signing_public == self.signing.public()
            && card.hpke_public.as_slice() == self.hpke_public().as_slice()
    }
}

/// Builds the signed enrollment proof for `keys` as a device of `account`
/// (§7).
pub fn enroll(keys: &DeviceKeys, account: Id, origin: &str, challenge: Key) -> Signed<Enroll> {
    Signed::sign(
        Enroll {
            origin: Text::new(origin).expect("an origin fits a str"),
            card: Nested(keys.card(account)),
            challenge,
        },
        keys.signing(),
    )
}

/// What the server knows when it checks an enrollment proof: its own
/// origin, the session's account and the challenge it issued.
#[derive(Clone, Debug)]
pub struct EnrollmentContext<'a> {
    /// The server's configured public origin.
    pub origin: &'a str,
    /// The account of the session that enrolls.
    pub account: Id,
    /// The challenge the server issued to that session, current and unused.
    pub challenge: Key,
}

/// Why the server refuses an enrollment proof (§7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EnrollmentError {
    /// The proof doesn't decode, or its card's device ID doesn't derive
    /// from its signing key.
    #[error("bad_request: {0}")]
    Decode(sync_protocol::codec::DecodeError),
    /// The signature doesn't verify under the card's signing key.
    #[error("signature_invalid")]
    Signature,
    /// The proof names another origin.
    #[error("origin_mismatch")]
    Origin,
    /// The card names another account than the session's.
    #[error("forbidden")]
    Account,
    /// The proof answers another challenge.
    #[error("challenge_invalid")]
    Challenge,
}

impl EnrollmentError {
    /// The server error code.
    pub fn server_code(&self) -> &'static str {
        match self {
            EnrollmentError::Decode(e) => e.kind.server_code(),
            EnrollmentError::Signature => "signature_invalid",
            EnrollmentError::Origin => "origin_mismatch",
            EnrollmentError::Account => "forbidden",
            EnrollmentError::Challenge => "challenge_invalid",
        }
    }
}

/// The server's enrollment-proof verifier (§7): it checks the signature
/// under the card's own signing key, then the origin, the account and the
/// challenge. Decoding already refused a card whose device ID doesn't
/// derive from its key. The caller then checks that the device ID is new,
/// or that it holds this card byte for byte and no chain removed it.
pub fn verify_enrollment(
    proof: &[u8],
    context: &EnrollmentContext<'_>,
) -> Result<Card, EnrollmentError> {
    let proof = Signed::<Enroll>::from_bytes(proof).map_err(EnrollmentError::Decode)?;
    let card = &proof.body.card;
    proof
        .verify(&card.signing_public)
        .map_err(|_| EnrollmentError::Signature)?;
    if proof.body.origin.as_str() != context.origin {
        return Err(EnrollmentError::Origin);
    }
    if card.account != context.account {
        return Err(EnrollmentError::Account);
    }
    if proof.body.challenge != context.challenge {
        return Err(EnrollmentError::Challenge);
    }
    Ok(proof.body.card.0.clone())
}

/// The genesis link of a new account chain, signed by its own card.
pub fn genesis_link(keys: &DeviceKeys, account: Id) -> Signed<AccountLink> {
    Signed::sign(
        AccountLink {
            account,
            seqno: 1,
            previous: Digest::ZERO,
            signer: keys.device(),
            kind: LinkKind::Genesis {
                card: Nested(keys.card(account)),
            },
        },
        keys.signing(),
    )
}

/// The link after `head`, signed by `signer`.
pub fn next_link(signer: &DeviceKeys, head: &AccountHead, kind: LinkKind) -> Signed<AccountLink> {
    Signed::sign(
        AccountLink {
            account: head.account,
            seqno: head.seqno + 1,
            previous: head.head,
            signer: signer.device(),
            kind,
        },
        signer.signing(),
    )
}

/// Why wrapping or unwrapping an envelope failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EnvelopeError {
    /// The recipient card's X-Wing public key isn't valid.
    #[error("invalid recipient key")]
    RecipientKey,
    /// HPKE failed to seal.
    #[error("HPKE seal failed")]
    Seal,
    /// The envelope names another recipient or X-Wing key than these keys.
    #[error("envelope addressed to another key")]
    Recipient,
    /// HPKE failed to open: a wrong key or tampered envelope.
    #[error("HPKE open failed")]
    Open,
}

/// The envelope fields a wrapping device fills in.
#[derive(Clone, Debug)]
pub struct EnvelopeHeader {
    /// The workspace.
    pub workspace: Id,
    /// The domain.
    pub domain: Id,
    /// The epoch.
    pub epoch: u64,
    /// The wrapping device.
    pub wrapping_device: Id,
}

/// Wraps `epoch_key` for the device whose card is `recipient`, taking the
/// card byte for byte from the recipient's account chain, and signs the
/// envelope with `wrapper`. `rng` supplies the X-Wing encapsulation's
/// randomness.
pub fn wrap_with_rng(
    header: &EnvelopeHeader,
    epoch_key: &SecretKey,
    recipient: &Card,
    wrapper: &DeviceKeys,
    rng: &mut impl CryptoRng,
) -> Result<Signed<Envelope>, EnvelopeError> {
    let public = <XWing as hpke::Kem>::PublicKey::from_bytes(recipient.hpke_public.as_slice())
        .map_err(|_| EnvelopeError::RecipientKey)?;
    let mut envelope = Envelope {
        workspace: header.workspace,
        domain: header.domain,
        epoch: header.epoch,
        recipient: recipient.device,
        recipient_hpke_digest: recipient.hpke_digest(),
        wrapping_device: header.wrapping_device,
        enc: ExactBytes::new(vec![0; ENCAPSULATED_KEY_LEN]).expect("fixed length"),
        ciphertext: ExactBytes::new(vec![0; ENVELOPE_CIPHERTEXT_LEN]).expect("fixed length"),
    };
    let (enc, ciphertext) = hpke::single_shot_seal_with_rng::<ChaCha20Poly1305, HkdfSha256, XWing>(
        &OpModeS::Base,
        &public,
        &envelope.info(),
        epoch_key.expose(),
        &[],
        rng,
    )
    .map_err(|_| EnvelopeError::Seal)?;
    envelope.enc = ExactBytes::new(enc.to_bytes().to_vec()).map_err(|_| EnvelopeError::Seal)?;
    envelope.ciphertext = ExactBytes::new(ciphertext).map_err(|_| EnvelopeError::Seal)?;
    Ok(Signed::sign(envelope, wrapper.signing()))
}

/// Wraps with the operating system's CSPRNG.
pub fn wrap(
    header: &EnvelopeHeader,
    epoch_key: &SecretKey,
    recipient: &Card,
    wrapper: &DeviceKeys,
) -> Result<Signed<Envelope>, EnvelopeError> {
    wrap_with_rng(header, epoch_key, recipient, wrapper, &mut SystemRng)
}

/// Unwraps an envelope addressed to `keys`, whose card is `card`. The
/// caller still checks the key against the epoch record's `key_check` and
/// the wrapping device's authority (`sync_protocol::keys::check_envelope`).
pub fn unwrap(
    envelope: &Envelope,
    keys: &DeviceKeys,
    card: &Card,
) -> Result<SecretKey, EnvelopeError> {
    if envelope.recipient != card.device
        || envelope.recipient_hpke_digest != card.hpke_digest()
        || !keys.matches(card)
    {
        return Err(EnvelopeError::Recipient);
    }
    let enc = <XWing as hpke::Kem>::EncappedKey::from_bytes(envelope.enc.as_slice())
        .map_err(|_| EnvelopeError::Open)?;
    let plaintext = Zeroizing::new(
        hpke::single_shot_open::<ChaCha20Poly1305, HkdfSha256, XWing>(
            &OpModeR::Base,
            &keys.hpke_private(),
            &enc,
            &envelope.info(),
            envelope.ciphertext.as_slice(),
            &[],
        )
        .map_err(|_| EnvelopeError::Open)?,
    );
    let key: [u8; 32] = plaintext
        .as_slice()
        .try_into()
        .map_err(|_| EnvelopeError::Open)?;
    Ok(SecretKey(key))
}

/// The operating system's CSPRNG as a `rand_core` generator.
pub struct SystemRng;

impl rand_core::TryRng for SystemRng {
    type Error = std::convert::Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        let mut bytes = [0u8; 4];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u32::from_le_bytes(bytes))
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        let mut bytes = [0u8; 8];
        self.try_fill_bytes(&mut bytes)?;
        Ok(u64::from_le_bytes(bytes))
    }

    fn try_fill_bytes(&mut self, dst: &mut [u8]) -> Result<(), Self::Error> {
        sync_protocol::crypto::fill_random(dst)
            .expect("the operating system's random source works");
        Ok(())
    }
}

impl rand_core::TryCryptoRng for SystemRng {}
