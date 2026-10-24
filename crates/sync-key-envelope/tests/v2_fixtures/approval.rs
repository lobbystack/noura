//! `sync-v2-approval.json`: the approval commitment, offer and code (§10).

use serde_json::json;
use sync_protocol::codec::{Encode, Nested, Sorted};
use sync_protocol::records::{
    Anchor, CheckpointAnchor, Offer, PolicyKind, Signed, approval_code, approval_code_text,
    approval_commit, invite_proof,
};
use sync_protocol::types::{Digest, Id, Key, to_hex};

use super::common::{Fixture, secret};
use super::policy::{Person, accepted, genesis, ids};
use super::world::{World, world};

pub const NC: Key = Key([0x3c; 32]);

pub fn anchor(w: &World) -> Anchor {
    Anchor {
        workspace: w.workspace,
        revision: 3,
        policy_head: w.policy.changes[2].digest(),
        generation: 1_760_000_000,
        checkpoint: CheckpointAnchor {
            domain: w.ws_domain,
            covered_seq: 6,
            checkpoint: w.checkpoint.checkpoint.digest(),
        },
    }
}

/// Alice's laptop offers to approve Bob's laptop, invited to her workspace.
pub fn offer(w: &World) -> Signed<Offer> {
    let candidate = w.bob_laptop.card();
    let commit = approval_commit(&candidate.fingerprint(), Some(&w.bob.head()), &NC);
    Signed::sign(
        Offer {
            approver_card: Nested(w.alice_laptop.card()),
            candidate_card_digest: candidate.fingerprint(),
            candidate_commit: commit,
            candidate_head: Some(w.bob.head()),
            approver_head: w.alice.head(),
            nonce: Key([0x0a; 32]),
            anchors: Sorted::new(vec![anchor(w)]).unwrap(),
        },
        w.alice_laptop.keys.signing(),
    )
}

pub fn fixture() -> Fixture {
    let w = world();
    let mut f = Fixture::new(
        "sync-v2-approval.json",
        "Device approval (sync-v2.md §10): the candidate's commitment, the approver's signed offer with its account heads and anchors, and the 8-digit code.",
    );
    let candidate = w.bob_laptop.card();
    let commit = approval_commit(&candidate.fingerprint(), Some(&w.bob.head()), &NC);
    f.case(json!({
        "kind": "commit",
        "name": "invited_candidate",
        "candidate_card_digest": candidate.fingerprint().to_hex(),
        "candidate_head": serde_json::to_value(w.bob.head()).unwrap(),
        "nc": NC.to_hex(),
        "expect": commit.to_hex(),
    }));
    let own_device = approval_commit(&w.alice_phone.card().fingerprint(), None, &NC);
    f.case(json!({
        "kind": "commit",
        "name": "own_new_device",
        "candidate_card_digest": w.alice_phone.card().fingerprint().to_hex(),
        "candidate_head": null,
        "nc": NC.to_hex(),
        "expect": own_device.to_hex(),
    }));

    let offer = offer(&w);
    f.accept_signed("offer", "offer", &offer, &w.alice_laptop);
    let code = approval_code(&offer.digest(), &NC);
    f.case(json!({
        "kind": "code",
        "name": "code_for_the_offer",
        "offer_digest": offer.digest().to_hex(),
        "nc": NC.to_hex(),
        "expect": code,
        "text": approval_code_text(code),
    }));
    f.case(json!({
        "kind": "code_text",
        "name": "leading_zeros",
        "code": 42,
        "expect": "0000 0042",
    }));

    // The candidate's reveal checks: its own card digest, commitment and
    // head. A nonce that doesn't match the commitment, and a swapped head,
    // fail; the candidate reports sync_approval_mismatch.
    f.case(json!({
        "kind": "reveal_check",
        "name": "matching_reveal",
        "offer": to_hex(&offer.to_bytes()),
        "candidate_card": to_hex(&candidate.to_bytes()),
        "candidate_head": serde_json::to_value(w.bob.head()).unwrap(),
        "nc": NC.to_hex(),
        "expect": "ok",
    }));
    f.case(json!({
        "kind": "reveal_check",
        "name": "reveal_that_doesnt_match_its_commitment",
        "offer": to_hex(&offer.to_bytes()),
        "candidate_card": to_hex(&candidate.to_bytes()),
        "candidate_head": serde_json::to_value(w.bob.head()).unwrap(),
        "nc": Key([0x3d; 32]).to_hex(),
        "expect": "sync_approval_mismatch",
    }));
    let mut swapped = w.bob.head();
    swapped.seqno += 1;
    f.case(json!({
        "kind": "reveal_check",
        "name": "swapped_account_head",
        "offer": to_hex(&offer.to_bytes()),
        "candidate_card": to_hex(&candidate.to_bytes()),
        "candidate_head": serde_json::to_value(swapped).unwrap(),
        "nc": NC.to_hex(),
        "expect": "sync_approval_mismatch",
    }));

    // An offer for 1,000 workspaces stays within a 1 MiB body.
    let mut anchors = Vec::new();
    for i in 0..1_000u32 {
        let mut a = anchor(&w);
        let mut bytes = [0u8; 16];
        bytes[..4].copy_from_slice(&(i + 1).to_be_bytes());
        a.workspace = Id(bytes);
        anchors.push(a);
    }
    let mut big = offer.body.clone();
    big.anchors = Sorted::new(anchors).unwrap();
    let big = Signed::sign(big, w.alice_laptop.keys.signing());
    let len = big.to_bytes().len();
    assert!(len < 1_048_576);
    f.case(json!({
        "kind": "offer_size",
        "name": "offer_for_1000_workspaces",
        "workspaces": 1000,
        "anchor_bytes": 120,
        "length": len,
        "digest": big.digest().to_hex(),
        "max_body": 1_048_576,
    }));

    // The invitation proof the request carries.
    let invite_secret = secret("invitation secret");
    f.case(json!({
        "kind": "invite_proof",
        "name": "proof_over_the_card",
        "secret": to_hex(invite_secret.expose()),
        "card": to_hex(&candidate.to_bytes()),
        "expect": invite_proof(&invite_secret, &candidate.to_bytes()).to_hex(),
    }));

    // Review of 0eef7d5: only the add_device link comes from the
    // approver's card. Another device of the account records the new head
    // in the workspace first, and the log, with the new card among its
    // devices, verifies (§10 step 6).
    let mut nora = Person::new("nora");
    let mut log = genesis(&nora);
    let phone = nora.add("nora phone");
    log.push(nora.first(), vec![nora.head()], PolicyKind::AccountHeads);
    let desktop = nora.add("nora desktop");
    log.push(&phone, vec![nora.head()], PolicyKind::AccountHeads);
    for actor in [nora.first(), &phone, &desktop] {
        f.actor(actor);
    }
    accepted(
        &mut f,
        "the_new_head_recorded_first_by_another_device_of_the_account",
        &[&nora.chain],
        &log,
        json!({"devices": {"3": ids(&[nora.first(), &phone, &desktop])}}),
    );
    let _ = Digest::ZERO;
    f
}
