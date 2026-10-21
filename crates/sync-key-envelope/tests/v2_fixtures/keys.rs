//! `sync-v2-keys.json`: key domains (§9): epoch records and bundles, key
//! checks, recipients, lineage, X-Wing envelopes, replacements, envelope
//! signers, and the write-access and epoch rules judged by log position.

use std::collections::BTreeSet;

use serde_json::{Value, json};
use sync_key_envelope::v2::{EnvelopeHeader, wrap_with_rng};
use sync_protocol::codec::Encode;
use sync_protocol::crypto::{context, derive_key};
use sync_protocol::keys::{key_check, recipients_digest};
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{Envelope, Epoch, PolicyKind, Role, Signed};
use sync_protocol::types::{Id, SecretKey, to_hex};

use super::common::{Actor, Fixture, Shape, secret};
use super::world::{Chain, Policy, SeededRng, epoch_record, verified_chains, world};

fn hexes<T: Encode>(items: &[T]) -> Vec<String> {
    items.iter().map(|i| to_hex(&i.to_bytes())).collect()
}

fn chains_json(chains: &[&Chain]) -> Value {
    json!(chains.iter().map(|c| hexes(&c.links)).collect::<Vec<_>>())
}

fn step_epoch(record: &Signed<Epoch>, key: &SecretKey, expect: Value) -> Value {
    json!({"epoch": to_hex(&record.to_bytes()), "key": to_hex(key.expose()), "expect": expect})
}

pub fn fixture() -> Fixture {
    let w = world();
    let mut f = Fixture::new(
        "sync-v2-keys.json",
        "Key domains (sync-v2.md §9): epoch records, bundles and envelopes, the key check, recipients digest, lineage, X-Wing envelope wrapping, epoch acceptance with replacements, envelope signers, and the write-access and epoch rules judged by log position.",
    );
    for a in [&w.alice_laptop, &w.alice_phone, &w.bob_laptop] {
        f.actor(a);
    }

    // Records.
    f.accept_signed("epoch_1", "epoch", &w.ws_epoch1, &w.alice_laptop);
    f.accept_signed("epoch_2", "epoch", &w.ws_epoch2, &w.alice_laptop);
    f.accept_signed(
        "restricted_epoch_1",
        "epoch",
        &w.restricted_epoch1,
        &w.alice_laptop,
    );
    f.accept(
        "epoch_bundle",
        "epoch_bundle",
        &w.ws_bundle2,
        None,
        Shape::Body,
    );
    f.accept_signed("envelope", "envelope", &w.envelope_bob, &w.alice_laptop);
    let mut lineage_49 = w.ws_epoch2.to_bytes();
    // The lineage's length prefix sits before its 48 bytes and the signature.
    let at = lineage_49.len() - 64 - 48 - 4;
    lineage_49[at..at + 4].copy_from_slice(&47u32.to_be_bytes());
    lineage_49.remove(at + 4);
    f.reject(
        "epoch_lineage_47_bytes",
        "epoch",
        &lineage_49,
        json!({"decode": "wrong_length"}),
    );
    let mut short_enc = w.envelope_bob.to_bytes();
    let enc_at = 19 + 3 + 16 * 2 + 8 + 16 + 32 + 16;
    short_enc[enc_at..enc_at + 4].copy_from_slice(&1119u32.to_be_bytes());
    f.reject(
        "envelope_enc_1119_bytes",
        "envelope",
        &short_enc,
        json!({"decode": "wrong_length"}),
    );

    // Key check, recipients, subkeys.
    f.case(json!({
        "kind": "key_check",
        "name": "key_check_of_epoch_2",
        "epoch_key": to_hex(w.ws_key2.expose()),
        "workspace": w.workspace.to_hex(),
        "domain": w.ws_domain.to_hex(),
        "epoch": 2,
        "expect": key_check(&w.ws_key2, &w.workspace, &w.ws_domain, 2).to_hex(),
    }));
    let recipients: BTreeSet<Id> = w.ws_bundle2.recipients.iter().copied().collect();
    f.case(json!({
        "kind": "recipients_digest",
        "name": "recipients_of_epoch_2",
        "recipients": recipients.iter().map(Id::to_hex).collect::<Vec<_>>(),
        "expect": recipients_digest(&recipients).to_hex(),
    }));
    for (name, ctx) in [
        ("lineage_key", context::LINEAGE_KEY),
        ("key_check_key", context::KEY_CHECK),
    ] {
        f.case(json!({
            "kind": "derive_key",
            "name": name,
            "context": ctx,
            "input": to_hex(w.ws_key2.expose()),
            "expect": to_hex(derive_key(ctx, w.ws_key2.expose()).expose()),
        }));
    }

    // Lineage: the current key yields every earlier one.
    let ws_key3 = secret("workspace epoch 3");
    let e3 = epoch_record(
        &w.verified,
        w.workspace,
        w.ws_domain,
        3,
        Some(&w.ws_epoch2),
        3,
        &w.alice_laptop,
        &ws_key3,
        Some(&w.ws_key2),
    );
    f.case(json!({
        "kind": "lineage",
        "name": "every_earlier_key",
        "records": hexes(&[e3.clone(), w.ws_epoch2.clone(), w.ws_epoch1.clone()]),
        "key": to_hex(ws_key3.expose()),
        "expect": [to_hex(ws_key3.expose()), to_hex(w.ws_key2.expose()), to_hex(w.ws_key1.expose())],
    }));
    let bad = epoch_record(
        &w.verified,
        w.workspace,
        w.ws_domain,
        2,
        Some(&w.ws_epoch1),
        3,
        &w.alice_laptop,
        &w.ws_key2,
        Some(&secret("not epoch 1")),
    );
    f.case(json!({
        "kind": "lineage",
        "name": "a_derived_key_that_fails_its_own_key_check",
        "records": hexes(&[bad, w.ws_epoch1.clone()]),
        "key": to_hex(w.ws_key2.expose()),
        "expect": "key_check",
    }));
    f.case(json!({
        "kind": "rotation_secrecy",
        "name": "an_earlier_holder_cant_derive_the_new_key",
        "epoch": to_hex(&w.ws_epoch2.to_bytes()),
        "earlier_key": to_hex(w.ws_key1.expose()),
        "expect": "lineage",
    }));

    // Epoch acceptance, computed from the verified policy.
    let chains = chains_json(&[&w.alice, &w.bob]);
    let changes = hexes(&w.policy.changes);
    let mut wrong_recipients = w.ws_epoch2.body.clone();
    let only_alice: BTreeSet<Id> = [w.alice_laptop.device()].into_iter().collect();
    wrong_recipients.recipients_digest = recipients_digest(&only_alice);
    let wrong_recipients = Signed::sign(wrong_recipients, w.alice_laptop.keys.signing());
    f.case(json!({
        "kind": "epoch_chain",
        "name": "epochs_checked_against_recipients_from_the_policy",
        "account_chains": chains,
        "changes": changes,
        "domain": w.ws_domain.to_hex(),
        "steps": [
            step_epoch(&w.ws_epoch1, &w.ws_key1, json!("next")),
            step_epoch(&wrong_recipients, &w.ws_key2, json!({"error": "recipients"})),
            step_epoch(&w.ws_epoch2, &w.ws_key2, json!("next")),
        ],
    }));
    let by_bob = epoch_record(
        &w.verified,
        w.workspace,
        w.restricted,
        1,
        None,
        3,
        &w.bob_laptop,
        &w.restricted_key1,
        None,
    );
    f.case(json!({
        "kind": "epoch_chain",
        "name": "restricted_epoch_1_from_another_device_than_the_creator",
        "account_chains": chains,
        "changes": changes,
        "domain": w.restricted.to_hex(),
        "steps": [
            step_epoch(&by_bob, &w.restricted_key1, json!({"error": "first_epoch_signer"})),
            step_epoch(&w.restricted_epoch1, &w.restricted_key1, json!("next")),
        ],
    }));
    let stranger = Actor::new("mallory", "mallory laptop");
    f.actor(&stranger);
    let mut foreign = w.ws_epoch1.body.clone();
    foreign.rotating_device = stranger.device();
    let foreign = Signed::sign(foreign, stranger.keys.signing());
    f.case(json!({
        "kind": "epoch_chain",
        "name": "signer_outside_the_recorded_heads",
        "account_chains": chains,
        "changes": changes,
        "domain": w.ws_domain.to_hex(),
        "steps": [step_epoch(&foreign, &w.ws_key1, json!({"error": "signer"}))],
    }));
    let ws_key4 = secret("workspace epoch 4");
    let e4 = epoch_record(
        &w.verified,
        w.workspace,
        w.ws_domain,
        4,
        Some(&w.ws_epoch2),
        3,
        &w.alice_laptop,
        &ws_key4,
        Some(&w.ws_key2),
    );
    let e4_bad = epoch_record(
        &w.verified,
        w.workspace,
        w.ws_domain,
        4,
        Some(&w.ws_epoch2),
        3,
        &w.alice_laptop,
        &ws_key4,
        Some(&w.ws_key1),
    );
    f.case(json!({
        "kind": "epoch_chain",
        "name": "replacement_skips_an_epoch_this_client_never_accepted",
        "account_chains": chains,
        "changes": changes,
        "domain": w.ws_domain.to_hex(),
        "steps": [
            step_epoch(&w.ws_epoch1, &w.ws_key1, json!("next")),
            step_epoch(&w.ws_epoch2, &w.ws_key2, json!("next")),
            step_epoch(&e4_bad, &ws_key4, json!({"error": "lineage"})),
            step_epoch(&e4, &ws_key4, json!({"replacement": [3, 3]})),
        ],
    }));

    // A removal at revision 4, for envelopes, skipped epochs and positions.
    let mut policy = w.policy.clone();
    policy.push(
        &w.alice_laptop,
        vec![w.alice.head()],
        PolicyKind::RemoveMember {
            account: w.bob_laptop.account,
        },
    );
    let removal_changes = hexes(&policy.changes);
    let verified =
        PolicyChain::verify(&policy.changes, &verified_chains(&[&w.alice, &w.bob])).unwrap();
    let key3 = secret("after the removal");
    let e3_after = epoch_record(
        &verified,
        w.workspace,
        w.ws_domain,
        3,
        Some(&w.ws_epoch2),
        4,
        &w.alice_laptop,
        &key3,
        Some(&w.ws_key2),
    );
    let header = |wrapper: &Actor| EnvelopeHeader {
        workspace: w.workspace,
        domain: w.ws_domain,
        epoch: 3,
        wrapping_device: wrapper.device(),
    };
    let from_alice = wrap_with_rng(
        &header(&w.alice_laptop),
        &key3,
        &w.alice_phone.card(),
        &w.alice_laptop.keys,
        &mut SeededRng::new("alice to phone"),
    )
    .unwrap();
    let from_bob = wrap_with_rng(
        &header(&w.bob_laptop),
        &key3,
        &w.alice_phone.card(),
        &w.bob_laptop.keys,
        &mut SeededRng::new("bob to phone"),
    )
    .unwrap();
    let envelope_case = |name: &str,
                         envelope: &Signed<Envelope>,
                         key: &SecretKey,
                         entry: Option<u64>,
                         expect: &str| {
        json!({
            "kind": "envelope_check",
            "name": name,
            "account_chains": chains_json(&[&w.alice, &w.bob]),
            "changes": removal_changes,
            "epoch": to_hex(&e3_after.to_bytes()),
            "envelope": to_hex(&envelope.to_bytes()),
            "recipient": "alice phone",
            "recipient_card": to_hex(&w.alice_phone.card().to_bytes()),
            "unwrapped_key": to_hex(key.expose()),
            "accepted_head": 4,
            "entry_revision": entry,
            "expect": expect,
        })
    };
    f.case(envelope_case(
        "envelope_from_a_reader",
        &from_alice,
        &key3,
        None,
        "accept",
    ));
    f.case(envelope_case(
        "envelope_from_a_removed_device_before_the_entry_waits",
        &from_bob,
        &key3,
        None,
        "wait",
    ));
    f.case(envelope_case(
        "envelope_from_a_removed_device_after_the_entry_fails",
        &from_bob,
        &key3,
        Some(4),
        "signer",
    ));
    f.case(envelope_case(
        "key_supplied_by_the_server_fails",
        &from_alice,
        &secret("server key"),
        None,
        "key_check",
    ));
    f.case(json!({
        "kind": "envelope_unwrap",
        "name": "bob_unwraps_epoch_2",
        "envelope": to_hex(&w.envelope_bob.to_bytes()),
        "recipient": "bob laptop",
        "recipient_card": to_hex(&w.bob_laptop.card().to_bytes()),
        "info": to_hex(&w.envelope_bob.body.info()),
        "expect": to_hex(w.ws_key2.expose()),
    }));
    f.case(json!({
        "kind": "envelope_unwrap",
        "name": "another_device_cant_unwrap",
        "envelope": to_hex(&w.envelope_bob.to_bytes()),
        "recipient": "alice phone",
        "recipient_card": to_hex(&w.alice_phone.card().to_bytes()),
        "info": to_hex(&w.envelope_bob.body.info()),
        "expect": "recipient",
    }));

    // Records under a skipped epoch.
    let rotator = w.alice_laptop.clone();
    for (name, signer, expect) in [
        (
            "record_of_the_skipped_epochs_rotator",
            &rotator,
            "unreadable",
        ),
        (
            "record_of_a_device_that_lost_access",
            &w.bob_laptop,
            "unreadable",
        ),
        (
            "record_of_a_reader_that_kept_access",
            &w.alice_phone,
            "hold",
        ),
    ] {
        f.case(json!({
            "kind": "skipped_epoch_record",
            "name": name,
            "account_chains": chains_json(&[&w.alice, &w.bob]),
            "changes": removal_changes,
            "domain": w.ws_domain.to_hex(),
            "signer": signer.device().to_hex(),
            "rotator": rotator.device().to_hex(),
            "replacement_revision": 4,
            "expect": expect,
        }));
    }

    // Positions: write access and epoch validity.
    let mut demoted = w.policy.clone();
    demoted.push(
        &w.alice_laptop,
        vec![w.alice.head()],
        PolicyKind::SetRole {
            account: w.bob_laptop.account,
            role: Role::Viewer,
        },
    );
    demoted.push(
        &w.alice_laptop,
        vec![w.alice.head()],
        PolicyKind::RemoveMember {
            account: w.bob_laptop.account,
        },
    );
    let demoted_verified =
        PolicyChain::verify(&demoted.changes, &verified_chains(&[&w.alice, &w.bob])).unwrap();
    let e2_after = epoch_record(
        &demoted_verified,
        w.workspace,
        w.ws_domain,
        2,
        Some(&w.ws_epoch1),
        5,
        &w.alice_laptop,
        &w.ws_key2,
        Some(&w.ws_key1),
    );
    let backdated = epoch_record(
        &demoted_verified,
        w.workspace,
        w.ws_domain,
        2,
        Some(&w.ws_epoch1),
        3,
        &w.bob_laptop,
        &w.ws_key2,
        Some(&w.ws_key1),
    );
    let op = |device: &Actor, revision: u64, epoch: u64, expect: &str| json!({"operation": {"device": device.device().to_hex(), "policy_revision": revision, "epoch": epoch}, "expect": expect});
    let bob = &w.bob_laptop;
    let alice = &w.alice_laptop;
    f.case(json!({
        "kind": "log_judge",
        "name": "records_judged_after_policy_and_epoch_entries",
        "account_chains": chains_json(&[&w.alice, &w.bob]),
        "changes": hexes(&demoted.changes),
        "domain": w.ws_domain.to_hex(),
        "start": null,
        "steps": [
            {"policy_entry": 1}, {"policy_entry": 2}, {"policy_entry": 3},
            {"epoch_entry": to_hex(&w.ws_epoch1.to_bytes())},
            op(bob, 3, 1, "ok"),
            op(bob, 4, 1, "writer_not_authorized"),
            {"policy_entry": 4},
            op(bob, 3, 1, "writer_not_authorized"),
            op(alice, 4, 1, "ok"),
            {"policy_entry": 5},
            {"rotation_pending": true},
            op(alice, 5, 1, "epoch_stale"),
            {"epoch_entry": to_hex(&e2_after.to_bytes())},
            op(alice, 5, 2, "ok"),
            op(alice, 5, 1, "epoch_stale"),
        ],
    }));
    f.case(json!({
        "kind": "log_judge",
        "name": "an_epoch_signed_before_a_removal_leaves_the_rotation_pending",
        "account_chains": chains_json(&[&w.alice, &w.bob]),
        "changes": hexes(&demoted.changes),
        "domain": w.ws_domain.to_hex(),
        "start": null,
        "steps": [
            {"policy_entry": 1}, {"policy_entry": 2}, {"policy_entry": 3}, {"policy_entry": 4}, {"policy_entry": 5},
            {"epoch_entry": to_hex(&w.ws_epoch1.to_bytes())},
            {"epoch_entry": to_hex(&backdated.to_bytes())},
            {"rotation_pending": true},
            op(alice, 5, 2, "epoch_stale"),
        ],
    }));
    for (name, epoch, epoch_revision, expect) in [
        (
            "after_a_bootstrap_the_rotation_stays_pending",
            1,
            1,
            "epoch_stale",
        ),
        ("after_a_bootstrap_past_the_rotation", 2, 5, "ok"),
    ] {
        f.case(json!({
            "kind": "log_judge",
            "name": name,
            "account_chains": chains_json(&[&w.alice, &w.bob]),
            "changes": hexes(&demoted.changes),
            "domain": w.ws_domain.to_hex(),
            "start": {"policy_revision": 5, "epoch": epoch, "epoch_policy_revision": epoch_revision},
            "steps": [op(alice, 5, epoch, expect)],
        }));
    }
    let _: Policy = policy;
    f
}
