//! `sync-v2-approval.json`: the approval commitment, offer and code (§10).

use serde_json::json;
use sync_protocol::codec::{Encode, Nested, Sorted};
use sync_protocol::records::{
    AccountHead, Anchor, CheckpointAnchor, Offer, PolicyKind, Role, Signed, approval_code,
    approval_code_text, approval_commit, invite_proof,
};
use sync_protocol::types::{Digest, Id, Key, to_hex};

use super::common::{Actor, Fixture, secret};
use super::policy::{
    Person, accepted, add_member, chains, changes, genesis, ids, workspace, ws_domain,
};
use super::world::{Chain, Policy, World, world};

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
    reset_cases(&mut f);
    let _ = Digest::ZERO;
    f
}

/// The reset cases of §7 and §10: what a reset device trusts before and
/// after the workspace accepts it, and which change answers its offer.
fn reset_cases(f: &mut Fixture) {
    let owner = Person::new("tess");
    let mut admin = Person::new("uma");
    let admin_laptop = admin.first().clone();
    let admin_phone = admin.add("uma phone");
    let editor = Person::new("vic");
    let mate = Person::new("wes");
    let reset_laptop = Actor::new("wes", "wes reset laptop");
    let sibling = Actor::new("wes", "wes reset phone");
    let mut reset = Person {
        chain: Chain::genesis(&reset_laptop),
        devices: vec![reset_laptop.clone()],
    };
    let request_head = reset.head();
    let signer = reset_laptop.clone();
    reset.chain.add_device(&signer, &sibling);
    reset.devices.push(sibling.clone());
    for actor in [
        owner.first(),
        &admin_laptop,
        &admin_phone,
        editor.first(),
        mate.first(),
        &reset_laptop,
        &sibling,
    ] {
        f.actor(actor);
    }
    let own_chain = || -> Vec<String> {
        reset
            .chain
            .links
            .iter()
            .map(|l| to_hex(&l.to_bytes()))
            .collect()
    };
    let all = [
        &owner.chain,
        &admin.chain,
        &editor.chain,
        &mate.chain,
        &reset.chain,
    ];
    let anchored =
        |f: &mut Fixture, name: &str, log: &Policy, reference: u64, expect: Option<u64>| {
            f.case(json!({
                "kind": "anchored_by",
                "name": name,
                "account_chains": chains(&all),
                "changes": changes(log),
                "own_chain": own_chain(),
                "reference_seqno": reference,
                "expect": expect,
            }));
        };

    let mut before = genesis(&owner);
    add_member(&mut before, &owner, &admin, Role::Admin);
    add_member(&mut before, &owner, &editor, Role::Editor);
    add_member(&mut before, &owner, &mate, Role::Editor);
    anchored(f, "a_reset_device_before_the_acceptance", &before, 1, None);

    let accept = |log: &mut Policy, by: &Person, device: &Actor| {
        log.push(
            device,
            vec![by.head(), request_head.clone()],
            PolicyKind::AcceptReset {
                account: mate.account(),
            },
        );
    };
    let mut by_owner = before.clone();
    accept(&mut by_owner, &owner, owner.first());
    let mut recorded = by_owner.clone();
    recorded.push(&reset_laptop, vec![reset.head()], PolicyKind::AccountHeads);
    anchored(
        f,
        "the_reset_device_records_the_head_and_its_sibling_trusts_the_workspace",
        &recorded,
        2,
        Some(6),
    );
    anchored(
        f,
        "the_sibling_before_the_reset_device_records_the_head",
        &by_owner,
        2,
        None,
    );

    // A server serves a genesis it made up under the workspace's ID.
    let forger = Person::new("xena");
    f.actor(forger.first());
    let forged = genesis(&forger);
    f.case(json!({
        "kind": "anchored_by",
        "name": "a_forged_genesis_under_a_known_workspace_id",
        "account_chains": chains(&[&forger.chain, &reset.chain]),
        "changes": changes(&forged),
        "own_chain": own_chain(),
        "reference_seqno": 1,
        "expect": null,
    }));

    let anchor_revision = before.changes.len() as u64;
    let offer_from = |approver: &Actor, head: AccountHead| {
        let card = reset_laptop.card();
        Signed::sign(
            Offer {
                approver_card: Nested(approver.card()),
                candidate_card_digest: card.fingerprint(),
                candidate_commit: approval_commit(&card.fingerprint(), Some(&request_head), &NC),
                candidate_head: Some(request_head.clone()),
                approver_head: head,
                nonce: Key([0x0b; 32]),
                anchors: Sorted::new(vec![Anchor {
                    workspace: workspace(),
                    revision: anchor_revision,
                    policy_head: before.changes.last().unwrap().digest(),
                    generation: 1_760_000_000,
                    checkpoint: CheckpointAnchor {
                        domain: ws_domain(),
                        covered_seq: 6,
                        checkpoint: Digest([3; 32]),
                    },
                }])
                .unwrap(),
            },
            approver.keys.signing(),
        )
    };
    let mut by_admin = before.clone();
    accept(&mut by_admin, &admin, &admin_laptop);
    let mut other_device = before.clone();
    other_device.push(&admin_laptop, vec![admin.head()], PolicyKind::AccountHeads);
    accept(&mut other_device, &admin, &admin_phone);
    let swapped = {
        let mut log = before.clone();
        log.push(
            owner.first(),
            vec![owner.head(), reset.head()],
            PolicyKind::AcceptReset {
                account: mate.account(),
            },
        );
        log
    };
    let from_owner = offer_from(owner.first(), owner.head());
    let from_admin = offer_from(&admin_laptop, admin.head());
    let from_editor = offer_from(editor.first(), editor.head());
    let mut cases = Vec::new();
    for (name, log, offer, expect) in [
        ("no_change_answers_yet", &before, &from_owner, json!(false)),
        (
            "the_owner_committed_it",
            &by_owner,
            &from_owner,
            json!(true),
        ),
        ("an_admin_committed_it", &by_admin, &from_admin, json!(true)),
        (
            "another_device_committed_it",
            &other_device,
            &from_admin,
            json!("sync_approval_mismatch"),
        ),
        (
            "the_change_records_another_head",
            &swapped,
            &from_owner,
            json!("sync_approval_mismatch"),
        ),
        (
            "an_editor_offered",
            &by_owner,
            &from_editor,
            json!("sync_approval_mismatch"),
        ),
    ] {
        cases.push(json!({
            "kind": "reset_commit",
            "name": name,
            "account_chains": chains(&all),
            "changes": changes(log),
            "offer": to_hex(&offer.to_bytes()),
            "head": request_head,
            "expect": expect,
        }));
    }
    f.set("reset_cases", json!(cases));
}
