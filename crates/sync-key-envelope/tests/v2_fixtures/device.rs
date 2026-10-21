//! `sync-v2-device.json`: the device card with an X-Wing key, its
//! fingerprint, and the enrollment proof (§7).

use serde_json::json;
use sync_key_envelope::v2::enroll;
use sync_protocol::codec::Encode;
use sync_protocol::records::fingerprint_text;
use sync_protocol::types::{Id, Key, to_hex};

use super::common::{Actor, Fixture, Shape};

pub const ORIGIN: &str = "https://sync.noura.app";
pub const CHALLENGE: Key = Key([0x42; 32]);

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-device.json",
        "Devices (sync-v2.md §7): a card with a real X-Wing public key and its fingerprint, and the enrollment proof that the server's verifier accepts, refusing a proof for another origin, account, key or challenge.",
    );
    let laptop = Actor::new("ida", "ida laptop");
    f.actor(&laptop);
    let card = laptop.card();
    f.accept(
        "card_with_an_x_wing_key",
        "card",
        &card,
        None,
        Shape::Tagged,
    );
    f.case(json!({
        "kind": "fingerprint",
        "name": "fingerprint_of_the_card",
        "card": to_hex(&card.to_bytes()),
        "expect": card.fingerprint().to_hex(),
        "text": fingerprint_text(&card.fingerprint()),
    }));
    f.case(json!({
        "kind": "hpke_card",
        "name": "the_card_carries_the_x_wing_key_of_its_seed",
        "actor": "ida laptop",
        "card": to_hex(&card.to_bytes()),
        "expect": true,
    }));
    let proof = enroll(&laptop.keys, laptop.account, ORIGIN, CHALLENGE);
    f.accept_signed("enroll", "enroll", &proof, &laptop);
    let context =
        |name: &str, proof: Vec<u8>, origin: &str, account: Id, challenge: Key, expect: &str| {
            json!({
                "kind": "enrollment",
                "name": name,
                "proof": to_hex(&proof),
                "origin": origin,
                "account": account.to_hex(),
                "challenge": challenge.to_hex(),
                "expect": expect,
            })
        };
    f.case(context(
        "fresh_proof",
        proof.to_bytes(),
        ORIGIN,
        laptop.account,
        CHALLENGE,
        "ok",
    ));
    f.case(context(
        "proof_for_another_origin",
        enroll(
            &laptop.keys,
            laptop.account,
            "https://evil.example",
            CHALLENGE,
        )
        .to_bytes(),
        ORIGIN,
        laptop.account,
        CHALLENGE,
        "origin_mismatch",
    ));
    let other = Actor::new("jo", "jo laptop");
    f.case(context(
        "proof_for_another_account",
        proof.to_bytes(),
        ORIGIN,
        other.account,
        CHALLENGE,
        "forbidden",
    ));
    let forged = sync_protocol::records::Signed::sign(proof.body.clone(), other.keys.signing());
    f.case(context(
        "proof_signed_by_another_key",
        forged.to_bytes(),
        ORIGIN,
        laptop.account,
        CHALLENGE,
        "signature_invalid",
    ));
    f.case(context(
        "proof_for_another_challenge",
        proof.to_bytes(),
        ORIGIN,
        laptop.account,
        Key([0x43; 32]),
        "challenge_invalid",
    ));
    let mut mismatch = proof.to_bytes();
    let device_at = 27 + 2 + ORIGIN.len() + 4 + 25 + 16;
    mismatch[device_at] ^= 1;
    f.case(context(
        "card_whose_device_id_doesnt_derive",
        mismatch,
        ORIGIN,
        laptop.account,
        CHALLENGE,
        "bad_request",
    ));
    f
}
