//! `sync-v2-recovery.json`: the recovery kit, its secret and the recovery
//! proof (§12).

use serde_json::json;
use sync_protocol::codec::{Encode, Nested, Sorted, Text};
use sync_protocol::crypto::{context, derive_key, hash};
use sync_protocol::records::{
    CheckpointAnchor, KIT_SECRET_LEN, KitPayload, KitWorkspace, RecoveryKit, RecoveryProof, Signed,
    kit_secret_text,
};
use sync_protocol::types::{Key, to_hex};

use super::common::{Actor, Fixture, Shape, id, nonce};
use super::world::{World, world};

pub const KIT_SECRET: [u8; KIT_SECRET_LEN] = [
    0x6e, 0x6f, 0x75, 0x72, 0x61, 0x20, 0x6b, 0x69, 0x74, 0x20, 0x73, 0x65, 0x63, 0x72, 0x65, 0x74,
    0x00, 0xff, 0x10, 0x01,
];

pub fn identity() -> Actor {
    Actor::new("alice", "alice recovery kit")
}

/// The recovery identity proves itself for Alice's new laptop.
pub fn proof(_: &World) -> Signed<RecoveryProof> {
    let kit = identity();
    Signed::sign(
        RecoveryProof {
            origin: Text::new("https://sync.noura.app").unwrap(),
            recovery_device: kit.device(),
            new_card_digest: Actor::new("alice", "alice new laptop").card().fingerprint(),
            challenge: Key([0x21; 32]),
        },
        kit.keys.signing(),
    )
}

pub fn fixture() -> Fixture {
    let w = world();
    let mut f = Fixture::new(
        "sync-v2-recovery.json",
        "Recovery kits (sync-v2.md §12): the kit file under a key from its 20-byte secret and salt, its plaintext with the account head and checkpoint anchors, the secret's base32 form, and the recovery proof.",
    );
    let kit = identity();
    f.actor(&kit);
    let payload = KitPayload {
        origin: Text::new("https://sync.noura.app").unwrap(),
        account: kit.account,
        account_head: w.alice.head(),
        card: Nested(kit.card()),
        signing_seed: kit.keys.signing().seed(),
        hpke_secret: kit.keys.hpke_seed().clone(),
        workspaces: Sorted::new(vec![KitWorkspace {
            workspace: w.workspace,
            genesis: w.policy.changes[0].digest(),
            revision: 3,
            head: w.policy.changes[2].digest(),
            generation: 1_760_000_000,
            anchors: Sorted::new(vec![CheckpointAnchor {
                domain: w.ws_domain,
                covered_seq: 6,
                checkpoint: w.checkpoint.checkpoint.digest(),
            }])
            .unwrap(),
        }])
        .unwrap(),
    };
    f.accept("kit_payload", "kit_payload", &payload, None, Shape::Tagged);
    let salt = id("kit salt");
    let sealed = RecoveryKit::seal(&payload, &KIT_SECRET, salt, nonce("kit"));
    f.accept("recovery_kit", "recovery_kit", &sealed, None, Shape::Tagged);
    let mut material = KIT_SECRET.to_vec();
    material.extend_from_slice(&salt.0);
    f.case(json!({
        "kind": "kit_key",
        "name": "kit_key_from_secret_and_salt",
        "context": context::RECOVERY_KIT,
        "secret": to_hex(&KIT_SECRET),
        "salt": salt.to_hex(),
        "expect": to_hex(derive_key(context::RECOVERY_KIT, &material).expose()),
    }));
    let text = kit_secret_text(&KIT_SECRET);
    f.case(json!({
        "kind": "kit_open",
        "name": "open_with_the_secret",
        "kit": to_hex(&sealed.to_bytes()),
        "secret_text": text,
        "expect": to_hex(&payload.to_bytes()),
    }));
    f.case(json!({
        "kind": "kit_open",
        "name": "secret_typed_with_spaces_hyphens_and_lowercase",
        "kit": to_hex(&sealed.to_bytes()),
        "secret_text": text.to_lowercase().replace(' ', "-"),
        "expect": to_hex(&payload.to_bytes()),
    }));
    let mut wrong = KIT_SECRET;
    wrong[0] ^= 1;
    f.case(json!({
        "kind": "kit_open",
        "name": "wrong_secret",
        "kit": to_hex(&sealed.to_bytes()),
        "secret_text": kit_secret_text(&wrong),
        "expect": "sync_recovery_kit_invalid",
    }));
    let mut tampered = sealed.to_bytes();
    let last = tampered.len() - 1;
    tampered[last] ^= 1;
    f.case(json!({
        "kind": "kit_open",
        "name": "tampered_file",
        "kit": to_hex(&tampered),
        "secret_text": text,
        "expect": "sync_recovery_kit_invalid",
    }));
    f.case(json!({
        "kind": "kit_secret_text",
        "name": "base32_in_8_groups_of_4",
        "secret": to_hex(&KIT_SECRET),
        "expect": text,
    }));
    f.case(json!({
        "kind": "kit_secret_parse",
        "name": "too_short",
        "text": "ABCD EFGH",
        "expect": "invalid",
    }));
    let proof = proof(&w);
    f.accept_signed("recovery_proof", "recovery_proof", &proof, &kit);
    f.case(json!({
        "kind": "kit_card_matches_seeds",
        "name": "the_kit_card_matches_its_seeds",
        "payload": to_hex(&payload.to_bytes()),
        "expect": true,
    }));
    let _ = hash(b"");
    f
}
