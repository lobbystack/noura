//! The fixture cases that need X-Wing keys or the enrollment verifier:
//! `sync-v2-device.json` and the envelope cases of `sync-v2-keys.json`.

use std::path::PathBuf;

use serde_json::Value;
use sync_key_envelope::v2::{
    DeviceKeys, EnrollmentContext, EnvelopeError, unwrap, verify_enrollment,
};
use sync_protocol::codec::Decode;
use sync_protocol::records::{Card, Envelope, KitPayload, Signed};
use sync_protocol::types::{Id, Key, SecretKey, from_hex, to_hex};

fn load(file: &str) -> Value {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/workspace-format/fixtures")
        .join(file);
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn hex(value: &Value) -> Vec<u8> {
    from_hex(value.as_str().unwrap()).unwrap()
}

fn keys(doc: &Value, actor: &str) -> DeviceKeys {
    let entry = &doc["keys"][actor];
    DeviceKeys::from_seeds(
        &SecretKey(hex(&entry["signing_seed"]).try_into().unwrap()),
        SecretKey(hex(&entry["hpke_seed"]).try_into().unwrap()),
    )
}

fn cases<'a>(doc: &'a Value, kind: &'a str) -> impl Iterator<Item = &'a Value> {
    doc["cases"]
        .as_array()
        .unwrap()
        .iter()
        .filter(move |c| c["kind"] == kind)
}

#[test]
fn cards_carry_the_x_wing_key_of_their_seed() {
    let doc = load("sync-v2-device.json");
    let mut checked = 0;
    for case in cases(&doc, "hpke_card") {
        let card = Card::from_bytes(&hex(&case["card"])).unwrap();
        let keys = keys(&doc, case["actor"].as_str().unwrap());
        assert_eq!(
            keys.matches(&card),
            case["expect"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        checked += 1;
    }
    assert!(checked > 0);
}

#[test]
fn the_enrollment_verifier_reaches_each_cases_outcome() {
    let doc = load("sync-v2-device.json");
    let mut checked = 0;
    for case in cases(&doc, "enrollment") {
        let context = EnrollmentContext {
            origin: case["origin"].as_str().unwrap(),
            account: Id(hex(&case["account"]).try_into().unwrap()),
            challenge: Key(hex(&case["challenge"]).try_into().unwrap()),
        };
        let ours = match verify_enrollment(&hex(&case["proof"]), &context) {
            Ok(_) => "ok",
            Err(e) => e.server_code(),
        };
        assert_eq!(ours, case["expect"].as_str().unwrap(), "{}", case["name"]);
        checked += 1;
    }
    assert!(checked >= 6);
}

#[test]
fn envelopes_unwrap_for_their_recipient_alone() {
    let doc = load("sync-v2-keys.json");
    let mut checked = 0;
    for case in cases(&doc, "envelope_unwrap") {
        let envelope = Signed::<Envelope>::from_bytes(&hex(&case["envelope"])).unwrap();
        assert_eq!(envelope.body.info(), hex(&case["info"]));
        let card = Card::from_bytes(&hex(&case["recipient_card"])).unwrap();
        let keys = keys(&doc, case["recipient"].as_str().unwrap());
        let ours = match unwrap(&envelope.body, &keys, &card) {
            Ok(key) => to_hex(key.expose()),
            Err(EnvelopeError::Recipient) => "recipient".to_owned(),
            Err(e) => format!("{e:?}"),
        };
        assert_eq!(ours, case["expect"].as_str().unwrap(), "{}", case["name"]);
        checked += 1;
    }
    assert!(checked >= 2);
}

#[test]
fn a_recovery_kits_card_matches_both_its_seeds() {
    let doc = load("sync-v2-recovery.json");
    for case in cases(&doc, "kit_card_matches_seeds") {
        let payload = KitPayload::from_bytes(&hex(&case["payload"])).unwrap();
        let keys = DeviceKeys::from_seeds(&payload.signing_seed, payload.hpke_secret.clone());
        assert_eq!(
            keys.matches(&payload.card),
            case["expect"].as_bool().unwrap()
        );
    }
}
