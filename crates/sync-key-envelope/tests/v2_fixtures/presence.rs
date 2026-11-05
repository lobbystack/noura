//! `sync-v2-presence.json`: presence records (§23).

use serde_json::json;
use sync_protocol::codec::{Bytes, Encode, List};
use sync_protocol::crypto::{context, derive_key};
use sync_protocol::records::{
    Presence, PresencePayload, PresenceState, Selection, Signed, presence_channel,
};
use sync_protocol::types::{Id, Nonce, to_hex};

use super::common::{Fixture, Shape, id, nonce};
use super::world::{World, world};

pub const MINUTE: u32 = 29_333_333;

pub fn payload(w: &World) -> PresencePayload {
    PresencePayload {
        rid: w.rid,
        state: PresenceState::Active,
        selections: List(vec![Selection {
            anchor: Bytes(vec![0x01, 0x02, 0x03]),
            head: Bytes(vec![0x01, 0x02, 0x05]),
        }]),
    }
}

pub fn presence(w: &World) -> Signed<Presence> {
    let mut body = Presence {
        workspace: w.workspace,
        domain: w.ws_domain,
        epoch: 2,
        device: w.alice_laptop.device(),
        session: id("presence session"),
        sequence: 7,
        minute: MINUTE,
        channel: presence_channel(&w.ws_key2, &w.rid),
        nonce: Nonce::ZERO,
        ciphertext: Bytes::default(),
    };
    body.seal_payload(
        &payload(w),
        &w.ws_key2,
        &w.ws_epoch2.digest(),
        nonce("presence"),
    )
    .unwrap();
    Signed::sign(body, w.alice_laptop.keys.signing())
}

pub fn fixture() -> Fixture {
    let w = world();
    let mut f = Fixture::new(
        "sync-v2-presence.json",
        "Presence (sync-v2.md §23): the signed record, its padded payload under the presence subkey with the epoch record's digest in its associated data, the note's channel, and the receivers' clock window.",
    );
    f.case(json!({
        "kind": "derive_key",
        "name": "presence_key",
        "context": context::PRESENCE_KEY,
        "input": to_hex(w.ws_key2.expose()),
        "expect": to_hex(derive_key(context::PRESENCE_KEY, w.ws_key2.expose()).expose()),
    }));
    f.case(json!({
        "kind": "presence_channel",
        "name": "channel_of_a_note",
        "epoch_key": to_hex(w.ws_key2.expose()),
        "rid": w.rid.to_hex(),
        "expect": presence_channel(&w.ws_key2, &w.rid).to_hex(),
    }));
    f.case(json!({
        "kind": "presence_channel",
        "name": "the_channel_changes_with_each_epoch",
        "epoch_key": to_hex(w.ws_key1.expose()),
        "rid": w.rid.to_hex(),
        "expect": presence_channel(&w.ws_key1, &w.rid).to_hex(),
    }));
    f.accept(
        "presence_payload",
        "presence_payload",
        &payload(&w),
        None,
        Shape::Tagged,
    );
    let record = presence(&w);
    f.accept_signed("presence", "presence", &record, &w.alice_laptop);
    f.case(json!({
        "kind": "presence_open",
        "name": "open_under_the_current_epoch",
        "presence": to_hex(&record.to_bytes()),
        "epoch_key": to_hex(w.ws_key2.expose()),
        "epoch_record": w.ws_epoch2.digest().to_hex(),
        "expect": to_hex(&payload(&w).to_bytes()),
    }));
    f.case(json!({
        "kind": "presence_open",
        "name": "another_epoch_record_fails",
        "presence": to_hex(&record.to_bytes()),
        "epoch_key": to_hex(w.ws_key2.expose()),
        "epoch_record": w.ws_epoch1.digest().to_hex(),
        "expect": "invalid",
    }));
    for (now, ok) in [
        (MINUTE, true),
        (MINUTE + 2, true),
        (MINUTE - 2, true),
        (MINUTE + 3, false),
        (MINUTE - 3, false),
    ] {
        f.case(json!({
            "kind": "clock_window",
            "name": format!("receiver_at_{}", i64::from(now) - i64::from(MINUTE)),
            "minute": MINUTE,
            "now": now,
            "expect": ok,
        }));
    }
    let mut too_many = payload(&w);
    too_many.selections = List(vec![too_many.selections[0].clone(); 9]);
    f.reject(
        "presence_payload_nine_selections",
        "presence_payload",
        &too_many.to_bytes(),
        json!({"decode": "too_long"}),
    );
    let _ = Id::ZERO;
    f
}
