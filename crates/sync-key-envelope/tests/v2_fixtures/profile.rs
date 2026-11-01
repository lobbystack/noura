//! `sync-v2-profile.json`: the workspace profile (§25).

use serde_json::json;
use sync_protocol::codec::{Encode, Text};
use sync_protocol::crypto::{context, derive_key};
use sync_protocol::records::ProfilePayload;
use sync_protocol::types::to_hex;

use super::common::{Fixture, Shape};
use super::world::world;

pub fn fixture() -> Fixture {
    let w = world();
    let mut f = Fixture::new(
        "sync-v2-profile.json",
        "The workspace profile (sync-v2.md §25): the signed record and its padded payload under the profile subkey of the workspace domain's epoch key, with the epoch record's digest in its associated data.",
    );
    f.case(json!({
        "kind": "derive_key",
        "name": "profile_key",
        "context": context::PROFILE_KEY,
        "input": to_hex(w.ws_key2.expose()),
        "expect": to_hex(derive_key(context::PROFILE_KEY, w.ws_key2.expose()).expose()),
    }));
    let payload = ProfilePayload {
        name: Text::new("Garden plans").unwrap(),
    };
    f.accept(
        "profile_payload",
        "profile_payload",
        &payload,
        None,
        Shape::Tagged,
    );
    f.accept_signed("profile", "profile", &w.profile, &w.alice_laptop);
    f.case(json!({
        "kind": "profile_open",
        "name": "open_under_its_epoch",
        "profile": to_hex(&w.profile.to_bytes()),
        "epoch_key": to_hex(w.ws_key2.expose()),
        "epoch_record": w.ws_epoch2.digest().to_hex(),
        "expect": to_hex(&payload.to_bytes()),
    }));
    f.case(json!({
        "kind": "profile_open",
        "name": "another_epoch_key_fails",
        "profile": to_hex(&w.profile.to_bytes()),
        "epoch_key": to_hex(w.ws_key1.expose()),
        "epoch_record": w.ws_epoch2.digest().to_hex(),
        "expect": "invalid",
    }));
    f
}
