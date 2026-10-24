//! `sync-v2-recovery.json`: the recovery kit, its secret and the recovery
//! proof (§12).

use serde_json::json;
use sync_protocol::codec::{Encode, Nested, Sorted, Text};
use sync_protocol::crypto::{context, derive_key, hash};
use sync_protocol::records::{
    Card, CheckpointAnchor, KIT_SECRET_LEN, KitPayload, KitWorkspace, LinkKind, RecoveryKit,
    RecoveryProof, Signed, kit_secret_text,
};
use sync_protocol::types::{Key, to_hex};

use super::common::{Actor, Fixture, Shape, id, nonce};
use super::world::{Chain, World, world};

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
    recovery_cases(&mut f);
    f
}

/// The server's checks of a recovery proof against the session account's
/// chain and enrolled card, and the order of a recovery's links (§12).
fn recovery_cases(f: &mut Fixture) {
    let laptop = Actor::new("dora", "dora laptop");
    let kit = Actor::new("dora", "dora recovery kit");
    let phone = Actor::new("dora", "dora phone");
    for actor in [&laptop, &kit, &phone] {
        f.actor(actor);
    }
    let mut chain = Chain::genesis(&laptop);
    chain.push(
        &laptop,
        LinkKind::AddRecovery {
            card: Nested(kit.card()),
        },
    );
    let origin = "https://sync.noura.app";
    let challenge = Key([0x42; 32]);
    let proof = |signer: &Actor, origin: &str, card: &Card| {
        Signed::sign(
            RecoveryProof {
                origin: Text::new(origin).unwrap(),
                recovery_device: signer.device(),
                new_card_digest: hash(&card.to_bytes()),
                challenge,
            },
            signer.keys.signing(),
        )
    };
    let links = |chain: &Chain| -> Vec<serde_json::Value> {
        chain
            .links
            .iter()
            .map(|l| json!(to_hex(&l.to_bytes())))
            .collect()
    };
    let mut check = |name: &str, chain: &Chain, proof: &Signed<RecoveryProof>, expect: &str| {
        f.case(json!({
            "kind": "recovery_proof_check",
            "name": name,
            "chain": links(chain),
            "origin": origin,
            "session_card": to_hex(&phone.card().to_bytes()),
            "proof": to_hex(&proof.to_bytes()),
            "expect": expect,
        }));
    };
    check(
        "a_proof_of_the_active_recovery_identity",
        &chain,
        &proof(&kit, origin, &phone.card()),
        "ok",
    );
    check(
        "a_proof_naming_another_origin",
        &chain,
        &proof(&kit, "https://elsewhere.example", &phone.card()),
        "origin_mismatch",
    );
    check(
        "a_proof_from_a_device_that_isnt_the_recovery_identity",
        &chain,
        &proof(&laptop, origin, &phone.card()),
        "forbidden",
    );
    let mut forged = proof(&kit, origin, &phone.card());
    forged.sig.0[63] ^= 1;
    check(
        "a_proof_with_a_bad_signature",
        &chain,
        &forged,
        "signature_invalid",
    );
    check(
        "a_proof_for_another_card_than_the_sessions",
        &chain,
        &proof(&kit, origin, &laptop.card()),
        "forbidden",
    );
    let mut revoked = chain.clone();
    revoked.push(
        &laptop,
        LinkKind::Remove {
            device: kit.device(),
            keep: Sorted::default(),
        },
    );
    check(
        "a_proof_of_a_recovery_identity_a_link_removed",
        &revoked,
        &proof(&kit, origin, &phone.card()),
        "forbidden",
    );

    // Step 5's order: the head right after recover_device adds the
    // recovered card and removes none, and the remove links that follow
    // only take cards out, the identity included. The remove link may
    // commit before the recovered device enters its last workspace.
    let mut recovery = chain.clone();
    recovery.push(
        &kit,
        LinkKind::RecoverDevice {
            card: Nested(phone.card()),
        },
    );
    recovery.push(
        &phone,
        LinkKind::Remove {
            device: laptop.device(),
            keep: Sorted::default(),
        },
    );
    let ids = |actors: &[&Actor]| {
        let mut ids: Vec<String> = actors.iter().map(|a| a.device().to_hex()).collect();
        ids.sort();
        ids
    };
    f.case(json!({
        "kind": "recovery_order",
        "name": "remove_links_commit_before_the_recovered_device_enters_its_last_workspace",
        "links": links(&recovery),
        "kit_seqno": 2,
        "expect": {
            "after_recover": {"seqno": 3, "adds": ids(&[&phone]), "removes": ids(&[])},
            "after_removes": {"seqno": 4, "adds": ids(&[]), "removes": ids(&[&laptop, &kit])},
            "proven_session_reads_the_identitys_envelopes_until_the_recovered_card_joins": true,
        },
    }));
}
