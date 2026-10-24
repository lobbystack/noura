//! `sync-v2-ids.json`: identifiers and their text forms (§5, §14).

use serde_json::json;
use sync_protocol::codec::Encode;
use sync_protocol::ids::{account_id, device_id, secret_check};
use sync_protocol::messages::LiveCursor;
use sync_protocol::types::{Digest, Id, Key, SecretKey, to_base64url, to_hex};

use super::common::{Actor, Fixture, id};
use super::world::Chain;

pub fn fixture() -> Fixture {
    let mut f = Fixture::new(
        "sync-v2-ids.json",
        "Identifiers (sync-v2.md §5, §8, §14): text forms of IDs, digests, tokens, integers and live cursors; device IDs derived from signing keys; card indices; the account fingerprint; and, from a test server secret, its key check and the account ID of an email.",
    );
    let sixteen = id("an id");
    f.case(json!({"kind": "base64url", "name": "16_byte_id", "bytes": sixteen.to_hex(), "expect": sixteen.to_base64url(), "length": 22}));
    let digest = Digest([0xfb; 32]);
    f.case(json!({"kind": "base64url", "name": "32_byte_digest", "bytes": digest.to_hex(), "expect": digest.to_base64url(), "length": 43}));
    let token = Key([0x8f; 32]);
    f.case(json!({"kind": "base64url", "name": "session_token", "bytes": token.to_hex(), "expect": token.to_base64url(), "length": 43}));
    for (name, text) in [
        ("padding", format!("{}==", sixteen.to_base64url())),
        ("standard_alphabet", "+/+/+/+/+/+/+/+/+/+/+w".to_owned()),
        ("21_characters", sixteen.to_base64url()[..21].to_owned()),
        (
            "non_canonical_trailing_bits",
            format!("{}B", &sixteen.to_base64url()[..21]),
        ),
    ] {
        f.case(json!({"kind": "base64url_invalid", "name": name, "text": text, "length": 16}));
    }
    for (text, value) in [
        ("0", Some(0u64)),
        ("1", Some(1)),
        ("18446744073709551615", Some(u64::MAX)),
    ] {
        f.case(json!({"kind": "decimal", "name": format!("decimal_{text}"), "text": text, "expect": value}));
    }
    for text in [
        "01",
        "-1",
        "+1",
        "",
        "1.0",
        "1e3",
        " 1",
        "18446744073709551616",
    ] {
        f.case(json!({"kind": "decimal", "name": format!("not_decimal_{text}"), "text": text, "expect": null}));
    }
    let cursor = LiveCursor {
        rid: id("live rid"),
        device: id("live device"),
        counter: 7,
    };
    f.case(json!({
        "kind": "live_cursor",
        "name": "live_cursor_in_a_query",
        "cursor": serde_json::to_value(&cursor).unwrap(),
        "encoding": to_hex(&cursor.to_bytes()),
        "expect": cursor.to_query(),
    }));

    // Device IDs.
    let actor = Actor::new("kim", "kim laptop");
    f.actor(&actor);
    let card = actor.card();
    f.case(json!({
        "kind": "device_id",
        "name": "derived_from_the_signing_key",
        "signing_public": actor.keys.signing().public().to_hex(),
        "card": to_hex(&card.to_bytes()),
        "expect": device_id(&actor.keys.signing().public()).to_hex(),
    }));
    let mut not_derived = card.to_bytes();
    not_derived[41] ^= 1;
    f.reject(
        "card_whose_device_id_doesnt_derive",
        "card",
        &not_derived,
        json!({"decode": "device_id_mismatch"}),
    );

    // The account fingerprint is the digest of the genesis link.
    let chain = Chain::genesis(&actor);
    f.case(json!({
        "kind": "account_fingerprint",
        "name": "genesis_digest",
        "genesis": to_hex(&chain.links[0].to_bytes()),
        "expect": chain.links[0].digest().to_hex(),
    }));

    // Card indices assigned at one change to cards of two accounts.
    let (chains, policy, indices) = super::policy::two_accounts_one_change();
    f.case(json!({
        "kind": "card_indices",
        "name": "two_accounts_at_one_change",
        "account_chains": chains.iter().map(|c| c.links.iter().map(|l| to_hex(&l.to_bytes())).collect::<Vec<_>>()).collect::<Vec<_>>(),
        "changes": policy.changes.iter().map(|c| to_hex(&c.to_bytes())).collect::<Vec<_>>(),
        "expect": indices,
    }));

    // The server secret: its key check and an account ID.
    let server_secret = SecretKey([0x5e; 32]);
    f.case(json!({
        "kind": "secret_check",
        "name": "key_check_of_a_test_secret",
        "secret": to_hex(server_secret.expose()),
        "expect": to_hex(secret_check(&server_secret).expose()),
    }));
    // Only ASCII letters lowercase: `ÅSA@Example.se` and `Åsa@example.se`
    // share an ID, and `åsa@example.se` gets another, whatever Unicode's
    // case mappings say.
    for email in [
        "Alice.Example@Example.COM",
        "alice.example@example.com",
        "ÅSA@Example.se",
        "Åsa@example.se",
        "åsa@example.se",
    ] {
        f.case(json!({
            "kind": "account_id",
            "name": format!("account_id_of_{email}"),
            "secret": to_hex(server_secret.expose()),
            "email": email,
            "expect": account_id(&server_secret, email).to_hex(),
        }));
    }
    assert_eq!(
        account_id(&server_secret, "ÅSA@Example.se"),
        account_id(&server_secret, "Åsa@example.se")
    );
    assert_ne!(
        account_id(&server_secret, "Åsa@example.se"),
        account_id(&server_secret, "åsa@example.se")
    );
    f.set(
        "random_ids",
        json!({
            "length": 16,
            "source": "the operating system's CSPRNG",
            "reserved": Id::ZERO.to_hex(),
            "example_text": to_base64url(&sixteen.0),
        }),
    );
    f
}
