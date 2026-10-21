//! Device keys, enrollment proofs and X-Wing envelopes (sync-v2.md §7, §9).

use sync_key_envelope::v2::{
    DeviceKeys, EnrollmentContext, EnrollmentError, EnvelopeError, EnvelopeHeader, enroll, unwrap,
    verify_enrollment, wrap,
};
use sync_protocol::codec::{Decode, Encode};
use sync_protocol::crypto::derive_key;
use sync_protocol::ids::device_id;
use sync_protocol::records::{Card, Envelope, Signed};
use sync_protocol::types::{Id, Key, SecretKey};

const ACCOUNT: Id = Id([0x11; 16]);
const ORIGIN: &str = "https://sync.noura.app";

fn keys(name: &str) -> DeviceKeys {
    DeviceKeys::from_seeds(
        &derive_key("noura test signing seed", name.as_bytes()),
        derive_key("noura test hpke seed", name.as_bytes()),
    )
}

#[test]
fn a_card_binds_both_keys_to_the_account() {
    let laptop = keys("laptop");
    let card = laptop.card(ACCOUNT);
    assert_eq!(card.device, device_id(&laptop.signing().public()));
    assert_eq!(card.hpke_public.len(), 1_216);
    assert_eq!(card.to_bytes().len(), 1_309);
    assert_eq!(Card::from_bytes(&card.to_bytes()).unwrap(), card);
    assert!(laptop.matches(&card));
    assert!(!keys("phone").matches(&card));
}

fn context(challenge: Key) -> EnrollmentContext<'static> {
    EnrollmentContext {
        origin: ORIGIN,
        account: ACCOUNT,
        challenge,
    }
}

#[test]
fn the_enrollment_verifier_accepts_a_fresh_proof() {
    let laptop = keys("laptop");
    let challenge = Key([0x42; 32]);
    let proof = enroll(&laptop, ACCOUNT, ORIGIN, challenge).to_bytes();
    assert_eq!(
        verify_enrollment(&proof, &context(challenge)),
        Ok(laptop.card(ACCOUNT))
    );
}

#[test]
fn the_enrollment_verifier_refuses_another_origin_account_key_or_challenge() {
    let laptop = keys("laptop");
    let challenge = Key([0x42; 32]);
    let other_origin = enroll(&laptop, ACCOUNT, "https://evil.example", challenge).to_bytes();
    assert_eq!(
        verify_enrollment(&other_origin, &context(challenge)),
        Err(EnrollmentError::Origin)
    );
    let other_account = enroll(&laptop, Id([0x12; 16]), ORIGIN, challenge).to_bytes();
    assert_eq!(
        verify_enrollment(&other_account, &context(challenge)),
        Err(EnrollmentError::Account)
    );
    // The proof names the laptop's card but another key signed it.
    let proof = enroll(&laptop, ACCOUNT, ORIGIN, challenge);
    let forged = Signed::sign(proof.body, keys("thief").signing()).to_bytes();
    assert_eq!(
        verify_enrollment(&forged, &context(challenge)),
        Err(EnrollmentError::Signature)
    );
    let stale = enroll(&laptop, ACCOUNT, ORIGIN, Key([0x43; 32])).to_bytes();
    assert_eq!(
        verify_enrollment(&stale, &context(challenge)),
        Err(EnrollmentError::Challenge)
    );
    // A card whose device ID doesn't derive from its key fails to decode.
    let mut bad = enroll(&laptop, ACCOUNT, ORIGIN, challenge).to_bytes();
    let device_at = 27 + 2 + ORIGIN.len() + 4 + 25 + 16;
    bad[device_at] ^= 1;
    assert!(matches!(
        verify_enrollment(&bad, &context(challenge)),
        Err(EnrollmentError::Decode(_))
    ));
}

fn header(domain: Id, wrapper: &DeviceKeys) -> EnvelopeHeader {
    EnvelopeHeader {
        workspace: Id([0x77; 16]),
        domain,
        epoch: 3,
        wrapping_device: wrapper.device(),
    }
}

#[test]
fn workspace_and_restricted_domain_keys_wrap_and_unwrap_for_a_device() {
    let laptop = keys("laptop");
    let phone = keys("phone");
    let phone_card = phone.card(ACCOUNT);
    for domain in [Id([0x78; 16]), Id([0x79; 16])] {
        let epoch_key = SecretKey([domain.0[0]; 32]);
        let envelope = wrap(&header(domain, &laptop), &epoch_key, &phone_card, &laptop).unwrap();
        let bytes = envelope.to_bytes();
        assert_eq!(bytes.len(), 1_366);
        let decoded = Signed::<Envelope>::from_bytes(&bytes).unwrap();
        decoded.verify(&laptop.signing().public()).unwrap();
        assert_eq!(decoded.body.recipient, phone.device());
        assert_eq!(decoded.body.recipient_hpke_digest, phone_card.hpke_digest());
        assert_eq!(unwrap(&decoded.body, &phone, &phone_card), Ok(epoch_key));
    }
}

#[test]
fn an_envelope_opens_only_for_its_recipient_and_info() {
    let laptop = keys("laptop");
    let phone = keys("phone");
    let tablet = keys("tablet");
    let phone_card = phone.card(ACCOUNT);
    let epoch_key = SecretKey([5; 32]);
    let envelope = wrap(
        &header(Id([0x78; 16]), &laptop),
        &epoch_key,
        &phone_card,
        &laptop,
    )
    .unwrap()
    .body;
    assert_eq!(
        unwrap(&envelope, &tablet, &tablet.card(ACCOUNT)),
        Err(EnvelopeError::Recipient)
    );
    // HPKE info binds the envelope prefix: another epoch fails to open.
    let mut moved = envelope.clone();
    moved.epoch = 4;
    assert_eq!(
        unwrap(&moved, &phone, &phone_card),
        Err(EnvelopeError::Open)
    );
    let mut tampered = envelope.clone();
    let mut ct = tampered.ciphertext.to_vec();
    ct[0] ^= 1;
    tampered.ciphertext = sync_protocol::codec::ExactBytes::new(ct).unwrap();
    assert_eq!(
        unwrap(&tampered, &phone, &phone_card),
        Err(EnvelopeError::Open)
    );
    // Two wraps of one key differ: X-Wing encapsulation is randomized.
    let again = wrap(
        &header(Id([0x78; 16]), &laptop),
        &epoch_key,
        &phone_card,
        &laptop,
    )
    .unwrap()
    .body;
    assert_ne!(again.enc, envelope.enc);
}

#[test]
fn generated_keys_are_distinct() {
    let a = DeviceKeys::generate().unwrap();
    let b = DeviceKeys::generate().unwrap();
    assert_ne!(a.device(), b.device());
    assert_ne!(a.hpke_public(), b.hpke_public());
}
