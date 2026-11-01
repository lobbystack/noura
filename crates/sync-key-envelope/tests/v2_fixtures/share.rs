//! `sync-v2-share.json`: share link snapshots (§24).

use serde_json::json;
use sync_protocol::codec::{Bytes, Encode, Text};
use sync_protocol::crypto::{SigningKey, context, derive_key};
use sync_protocol::records::{Share, SharePayload, Signed, share_fragment, share_token_hash};
use sync_protocol::tag::Tag;
use sync_protocol::types::{SecretKey, to_hex};

use super::common::{Actor, Fixture, Shape, nonce, secret};
use super::world::{World, world};

pub const TOKEN: [u8; 16] = [0x53; 16];

pub fn link_key() -> SecretKey {
    secret("share link key")
}

pub fn link_signing() -> SigningKey {
    SigningKey::from_seed(&secret("share link signing seed"))
}

pub fn payload() -> SharePayload {
    SharePayload {
        title: Text::new("Plan").unwrap(),
        markdown: Bytes(b"# Plan\n\nPlant tomatoes in May.\n".to_vec()),
    }
}

pub fn share(_: &World) -> Signed<Share> {
    Share::seal(
        share_token_hash(&TOKEN),
        1,
        &payload(),
        &link_key(),
        &link_signing(),
        nonce("share"),
    )
    .unwrap()
}

pub fn fixture() -> Fixture {
    let w = world();
    let mut f = Fixture::new(
        "sync-v2-share.json",
        "Share links (sync-v2.md §24): the token hash, the snapshot signed with the link's key and encrypted under the share key, its padded payload, and the URL fragment.",
    );
    let mut input = Tag::ShareToken.bytes();
    input.extend_from_slice(&TOKEN);
    f.case(json!({
        "kind": "token_hash",
        "name": "token_hash",
        "token": to_hex(&TOKEN),
        "input": to_hex(&input),
        "expect": share_token_hash(&TOKEN).to_hex(),
    }));
    f.case(json!({
        "kind": "derive_key",
        "name": "share_key",
        "context": context::SHARE_KEY,
        "input": to_hex(link_key().expose()),
        "expect": to_hex(derive_key(context::SHARE_KEY, link_key().expose()).expose()),
    }));
    f.accept(
        "share_payload",
        "share_payload",
        &payload(),
        None,
        Shape::Tagged,
    );
    let share = share(&w);
    // The link's own key pair signs; list it like a device's.
    let link = Actor {
        name: "share link".to_owned(),
        keys: sync_key_envelope::v2::DeviceKeys::from_seeds(
            &secret("share link signing seed"),
            secret("unused"),
        ),
        account: w.alice_laptop.account,
    };
    f.accept_signed("share", "share", &share, &link);
    let fragment = share_fragment(&link_key(), &link_signing().public());
    f.case(json!({
        "kind": "share_open",
        "name": "open_from_the_fragment",
        "share": to_hex(&share.to_bytes()),
        "fragment": fragment,
        "expect": to_hex(&payload().to_bytes()),
    }));
    let other_fragment = share_fragment(&secret("another link key"), &link_signing().public());
    f.case(json!({
        "kind": "share_open",
        "name": "wrong_link_key",
        "share": to_hex(&share.to_bytes()),
        "fragment": other_fragment,
        "expect": "invalid",
    }));
    let mut tampered = share.clone();
    tampered.body.snapshot_revision = 2;
    f.case(json!({
        "kind": "share_open",
        "name": "bad_link_signature",
        "share": to_hex(&tampered.to_bytes()),
        "fragment": fragment,
        "expect": "invalid",
    }));
    f.set(
        "link",
        json!({
            "format": "{origin}/s/{base64url(token)}#{base64url(link_key || link_verify_key)}",
            "example": format!(
                "https://sync.noura.app/s/{}#{}",
                sync_protocol::types::to_base64url(&TOKEN),
                fragment
            ),
        }),
    );
    f
}
