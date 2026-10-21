//! Key domains (§9): key checks, lineage, epoch acceptance, replacements,
//! envelope signers and the rules that judge records by log position.

mod support;

use std::collections::{BTreeMap, BTreeSet};

use support::{Device, Person, PolicyBuilder, chains};
use sync_protocol::codec::{ExactBytes, Sorted};
use sync_protocol::crypto::{context, derive_key, hash_parts};
use sync_protocol::keys::{
    EnvelopeVerdict, EpochAcceptance, EpochChain, EpochParams, KeyError, LogJudge, PositionError,
    SkippedEpochRecord, build_epoch, check_envelope, check_replacement, judge_skipped_epoch_record,
    key_check, key_matches, open_lineage, recipients_digest, walk_lineage,
};
use sync_protocol::policy::PolicyChain;
use sync_protocol::records::{Access, Envelope, Epoch, Level, PolicyKind, Role, Signed};
use sync_protocol::tag::Tag;
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};

const WORKSPACE: Id = Id([0x77; 16]);
const WS_DOMAIN: Id = Id([0x78; 16]);
const RESTRICTED: Id = Id([0x79; 16]);

fn key(n: u8) -> SecretKey {
    SecretKey([n; 32])
}

#[test]
fn the_key_check_is_a_keyed_hash_of_the_tagged_position() {
    let epoch_key = key(1);
    let check_key = derive_key(context::KEY_CHECK, &epoch_key.0);
    let mut input = Tag::KeyCheck.bytes();
    input.extend_from_slice(&WORKSPACE.0);
    input.extend_from_slice(&WS_DOMAIN.0);
    input.extend_from_slice(&7u64.to_be_bytes());
    let expected = blake3::keyed_hash(&check_key.0, &input);
    assert_eq!(
        key_check(&epoch_key, &WORKSPACE, &WS_DOMAIN, 7).0,
        *expected.as_bytes()
    );
}

#[test]
fn the_recipients_digest_covers_the_sorted_list() {
    let ids: BTreeSet<Id> = [Id([3; 16]), Id([1; 16])].into_iter().collect();
    let mut input = Tag::EpochRecipients.bytes();
    input.extend_from_slice(&2u32.to_be_bytes());
    input.extend_from_slice(&[1; 16]);
    input.extend_from_slice(&[3; 16]);
    assert_eq!(recipients_digest(&ids), hash_parts(&[&input]));
}

struct Workspace {
    alice: Person,
    bob: Person,
    policy: PolicyBuilder,
}

/// Alice owns the workspace; Bob is an editor; Alice's restricted domain
/// grants Bob write access.
fn workspace() -> Workspace {
    let alice = Person::new("alice");
    let bob = Person::new("bob");
    let mut policy = PolicyBuilder::new(WORKSPACE);
    policy.push(
        alice.first(),
        vec![alice.chain.head()],
        PolicyKind::Genesis {
            workspace_domain: WS_DOMAIN,
        },
    );
    policy.push(
        alice.first(),
        vec![alice.chain.head(), bob.chain.head()],
        PolicyKind::AddMember {
            account: bob.account(),
            role: Role::Editor,
            invitation: Id([9; 16]),
        },
    );
    policy.push(
        alice.first(),
        vec![alice.chain.head()],
        PolicyKind::CreateDomain {
            domain: RESTRICTED,
            marker_rid: Id([8; 16]),
            access: Sorted::new(vec![
                Access {
                    account: alice.account(),
                    level: Level::Manage,
                },
                Access {
                    account: bob.account(),
                    level: Level::Write,
                },
            ])
            .unwrap(),
        },
    );
    Workspace { alice, bob, policy }
}

impl Workspace {
    fn verified(&self) -> PolicyChain {
        PolicyChain::verify(
            &self.policy.changes,
            &chains(&[&self.alice.chain, &self.bob.chain]),
        )
        .unwrap()
    }
}

#[allow(clippy::too_many_arguments)]
fn epoch_record(
    policy: &PolicyChain,
    domain: Id,
    epoch: u64,
    previous: Option<&Signed<Epoch>>,
    revision: u64,
    rotator: &Device,
    epoch_key: &SecretKey,
    previous_key: Option<&SecretKey>,
) -> Signed<Epoch> {
    let snapshot = policy.at(revision).unwrap();
    let params = EpochParams {
        workspace: WORKSPACE,
        domain,
        epoch,
        previous: previous.map_or(Digest::ZERO, |p| p.digest()),
        policy_revision: revision,
        policy_head: snapshot.digest,
        rotating_device: rotator.id(),
        recipients: snapshot.readers(&domain),
    };
    Signed::sign(
        build_epoch(&params, epoch_key, previous_key, Nonce([epoch as u8; 24])),
        &rotator.key,
    )
}

#[test]
fn lineage_yields_every_earlier_key_each_checked_against_its_own_record() {
    let w = workspace();
    let policy = w.verified();
    let a = w.alice.first();
    let e1 = epoch_record(&policy, WS_DOMAIN, 1, None, 3, a, &key(1), None);
    let e2 = epoch_record(
        &policy,
        WS_DOMAIN,
        2,
        Some(&e1),
        3,
        a,
        &key(2),
        Some(&key(1)),
    );
    let e3 = epoch_record(
        &policy,
        WS_DOMAIN,
        3,
        Some(&e2),
        3,
        a,
        &key(3),
        Some(&key(2)),
    );
    let keys = walk_lineage(&[e3.clone(), e2.clone(), e1.clone()], &key(3)).unwrap();
    assert_eq!(keys, vec![key(3), key(2), key(1)]);
    // A key that its own record's key check refuses stops the walk.
    let bad = epoch_record(
        &policy,
        WS_DOMAIN,
        2,
        Some(&e1),
        3,
        a,
        &key(2),
        Some(&key(9)),
    );
    assert_eq!(walk_lineage(&[bad, e1], &key(2)), Err(KeyError::KeyCheck));
}

#[test]
fn a_rotation_produces_a_key_earlier_holders_cant_derive() {
    let w = workspace();
    let policy = w.verified();
    let a = w.alice.first();
    let e1 = epoch_record(&policy, WS_DOMAIN, 1, None, 3, a, &key(1), None);
    let e2 = epoch_record(
        &policy,
        WS_DOMAIN,
        2,
        Some(&e1),
        3,
        a,
        &key(2),
        Some(&key(1)),
    );
    // The holder of the epoch 1 key can't open epoch 2's lineage, and the
    // epoch 1 key doesn't pass epoch 2's key check.
    assert_eq!(open_lineage(&e2.body, &key(1)), Err(KeyError::Lineage));
    assert!(!key_matches(&e2.body, &key(1)));
    assert!(key_matches(&e2.body, &key(2)));
}

#[test]
fn epoch_records_verify_against_recipients_computed_from_the_policy() {
    let w = workspace();
    let policy = w.verified();
    let a = w.alice.first();
    let mut chain = EpochChain::new(WORKSPACE, WS_DOMAIN);
    let e1 = epoch_record(&policy, WS_DOMAIN, 1, None, 1, a, &key(1), None);
    assert_eq!(
        chain.accept(&e1, &key(1), &policy),
        Ok(EpochAcceptance::Next)
    );
    // Recipients at revision 3 include Bob's laptop; a record claiming the
    // revision-1 recipients at revision 3 fails.
    let mut wrong = epoch_record(
        &policy,
        WS_DOMAIN,
        2,
        Some(&e1),
        3,
        a,
        &key(2),
        Some(&key(1)),
    );
    let only_alice: BTreeSet<Id> = [a.id()].into_iter().collect();
    wrong.body.recipients_digest = recipients_digest(&only_alice);
    let wrong = Signed::sign(wrong.body, &a.key);
    assert_eq!(
        chain.accept(&wrong, &key(2), &policy),
        Err(KeyError::Recipients)
    );
    let e2 = epoch_record(
        &policy,
        WS_DOMAIN,
        2,
        Some(&e1),
        3,
        a,
        &key(2),
        Some(&key(1)),
    );
    assert_eq!(
        chain.accept(&e2, &key(2), &policy),
        Ok(EpochAcceptance::Next)
    );
}

#[test]
fn a_restricted_domains_first_epoch_comes_from_its_creator() {
    let w = workspace();
    let policy = w.verified();
    let mut chain = EpochChain::new(WORKSPACE, RESTRICTED);
    let by_bob = epoch_record(
        &policy,
        RESTRICTED,
        1,
        None,
        3,
        w.bob.first(),
        &key(1),
        None,
    );
    assert_eq!(
        chain.accept(&by_bob, &key(1), &policy),
        Err(KeyError::FirstEpochSigner)
    );
    let by_alice = epoch_record(
        &policy,
        RESTRICTED,
        1,
        None,
        3,
        w.alice.first(),
        &key(1),
        None,
    );
    assert!(chain.accept(&by_alice, &key(1), &policy).is_ok());
}

#[test]
fn a_signer_outside_the_recorded_heads_is_refused() {
    let w = workspace();
    let policy = w.verified();
    let stranger = support::device(support::account("mallory"), "mallory laptop");
    let mut chain = EpochChain::new(WORKSPACE, WS_DOMAIN);
    let e1 = epoch_record(&policy, WS_DOMAIN, 1, None, 1, &stranger, &key(1), None);
    assert_eq!(chain.accept(&e1, &key(1), &policy), Err(KeyError::Signer));
}

#[test]
fn a_replacement_passes_against_the_record_its_previous_names() {
    let w = workspace();
    let policy = w.verified();
    let a = w.alice.first();
    let mut chain = EpochChain::new(WORKSPACE, WS_DOMAIN);
    let e1 = epoch_record(&policy, WS_DOMAIN, 1, None, 1, a, &key(1), None);
    let e2 = epoch_record(
        &policy,
        WS_DOMAIN,
        2,
        Some(&e1),
        3,
        a,
        &key(2),
        Some(&key(1)),
    );
    chain.accept(&e1, &key(1), &policy).unwrap();
    chain.accept(&e2, &key(2), &policy).unwrap();
    // Epoch 3 by Bob failed for this client; it never accepted it. Alice's
    // replacement, epoch 4, names epoch 2 and carries its key.
    let e4 = epoch_record(
        &policy,
        WS_DOMAIN,
        4,
        Some(&e2),
        3,
        a,
        &key(4),
        Some(&key(2)),
    );
    assert_eq!(
        chain.accept(&e4, &key(4), &policy),
        Ok(EpochAcceptance::Replacement { skipped: 3..=3 })
    );
    assert_eq!(chain.ended_at(3), Some(3));
    // A replacement whose lineage holds another key fails.
    let mut chain = EpochChain::new(WORKSPACE, WS_DOMAIN);
    chain.accept(&e1, &key(1), &policy).unwrap();
    chain.accept(&e2, &key(2), &policy).unwrap();
    let wrong = epoch_record(
        &policy,
        WS_DOMAIN,
        4,
        Some(&e2),
        3,
        a,
        &key(4),
        Some(&key(1)),
    );
    assert_eq!(
        chain.accept(&wrong, &key(4), &policy),
        Err(KeyError::Lineage)
    );
}

#[test]
fn records_under_a_skipped_epoch_are_unreadable_or_hold_the_cursor() {
    let mut w = workspace();
    // Bob is removed at revision 4.
    let alice = w.alice.first().clone();
    w.policy.push(
        &alice,
        vec![w.alice.chain.head()],
        PolicyKind::RemoveMember {
            account: w.bob.account(),
        },
    );
    let policy = w.verified();
    let bob = w.bob.first().id();
    let rotator = Id([0x51; 16]);
    // The rotator's own record, and a removed device's: unreadable.
    assert_eq!(
        judge_skipped_epoch_record(&policy, &WS_DOMAIN, &rotator, Some(&rotator), 4),
        SkippedEpochRecord::Unreadable
    );
    assert_eq!(
        judge_skipped_epoch_record(&policy, &WS_DOMAIN, &bob, Some(&rotator), 4),
        SkippedEpochRecord::Unreadable
    );
    // A reader that kept access: an honest server would have refused the
    // replacement, so the client holds.
    assert_eq!(
        judge_skipped_epoch_record(&policy, &WS_DOMAIN, &alice.id(), Some(&rotator), 4),
        SkippedEpochRecord::Hold
    );
    // The server's replacement check agrees.
    let replacement = Epoch {
        workspace: WORKSPACE,
        domain: WS_DOMAIN,
        epoch: 4,
        previous: Digest::ZERO,
        policy_revision: 4,
        policy_head: Digest::ZERO,
        rotating_device: alice.id(),
        recipients_digest: Digest::ZERO,
        key_check: Digest::ZERO,
        lineage_nonce: Nonce::ZERO,
        lineage: Default::default(),
    };
    let only_rotator_and_removed: BTreeMap<u64, (Id, BTreeSet<Id>)> =
        [(3, (rotator, [rotator, bob].into_iter().collect()))]
            .into_iter()
            .collect();
    assert_eq!(
        check_replacement(&policy, &replacement, 2, 3, &only_rotator_and_removed),
        Ok(())
    );
    let used_by_reader: BTreeMap<u64, (Id, BTreeSet<Id>)> =
        [(3, (rotator, [alice.id()].into_iter().collect()))]
            .into_iter()
            .collect();
    assert_eq!(
        check_replacement(&policy, &replacement, 2, 3, &used_by_reader),
        Err("epoch_conflict")
    );
    // A replacement whose previous names the latest skips nothing.
    assert_eq!(
        check_replacement(&policy, &replacement, 3, 3, &BTreeMap::new()),
        Err("epoch_conflict")
    );
}

fn envelope(wrapper: &Device, recipient: &Device, epoch: &Epoch) -> Signed<Envelope> {
    Signed::sign(
        Envelope {
            workspace: epoch.workspace,
            domain: epoch.domain,
            epoch: epoch.epoch,
            recipient: recipient.id(),
            recipient_hpke_digest: recipient.card.hpke_digest(),
            wrapping_device: wrapper.id(),
            enc: ExactBytes::new(vec![0; 1_120]).unwrap(),
            ciphertext: ExactBytes::new(vec![0; 48]).unwrap(),
        },
        &wrapper.key,
    )
}

#[test]
fn an_envelope_from_a_device_that_lost_access_fails_even_from_the_server() {
    let mut w = workspace();
    let alice = w.alice.first().clone();
    let bob = w.bob.first().clone();
    // Bob is removed at revision 4; a rotation at revision 4 follows.
    w.policy.push(
        &alice,
        vec![w.alice.chain.head()],
        PolicyKind::RemoveMember {
            account: w.bob.account(),
        },
    );
    let policy = w.verified();
    let e2 = epoch_record(&policy, WS_DOMAIN, 2, None, 4, &alice, &key(2), None).body;
    // Alice's envelope passes at her accepted head.
    let good = envelope(&alice, &alice, &e2);
    assert_eq!(
        check_envelope(&good, &e2, &key(2), &alice.card, &policy, 4, None),
        Ok(EnvelopeVerdict::Accept)
    );
    // Bob's envelope fails at the accepted head; before the epoch's entry it
    // waits, and after an entry at revision 4 it fails.
    let bad = envelope(&bob, &alice, &e2);
    assert_eq!(
        check_envelope(&bad, &e2, &key(2), &alice.card, &policy, 4, None),
        Ok(EnvelopeVerdict::Wait)
    );
    assert_eq!(
        check_envelope(&bad, &e2, &key(2), &alice.card, &policy, 4, Some(4)),
        Err(KeyError::Signer)
    );
    // A key the server supplies fails the key check.
    assert_eq!(
        check_envelope(&good, &e2, &key(7), &alice.card, &policy, 4, None),
        Err(KeyError::KeyCheck)
    );
    // An envelope for another card's X-Wing key fails.
    let mut other = good.body.clone();
    other.recipient_hpke_digest = Digest([1; 32]);
    let other = Signed::sign(other, &alice.key);
    assert_eq!(
        check_envelope(&other, &e2, &key(2), &alice.card, &policy, 4, None),
        Err(KeyError::Recipient)
    );
}

#[test]
fn records_are_judged_by_their_position_after_policy_and_epoch_entries() {
    let mut w = workspace();
    let alice = w.alice.first().clone();
    let bob = w.bob.first().clone();
    // Revision 4 demotes Bob to viewer, which ends his write access to the
    // workspace domain but takes nobody out, so nothing rotates there.
    w.policy.push(
        &alice,
        vec![w.alice.chain.head()],
        PolicyKind::SetRole {
            account: w.bob.account(),
            role: Role::Viewer,
        },
    );
    // Revision 5 removes Bob, which requires a rotation.
    w.policy.push(
        &alice,
        vec![w.alice.chain.head()],
        PolicyKind::RemoveMember {
            account: w.bob.account(),
        },
    );
    let policy = w.verified();
    let e1 = epoch_record(&policy, WS_DOMAIN, 1, None, 1, &alice, &key(1), None);
    let e2 = epoch_record(
        &policy,
        WS_DOMAIN,
        2,
        Some(&e1),
        5,
        &alice,
        &key(2),
        Some(&key(1)),
    );

    let mut judge = LogJudge::new(WS_DOMAIN);
    for revision in 1..=3 {
        judge.policy_entry(revision, &policy).unwrap();
    }
    judge.epoch_entry(&e1.body);
    assert_eq!(judge.check_operation(&bob.id(), 3, 1, &policy), Ok(()));
    // A record whose policy_revision lies above the latest entry.
    assert_eq!(
        judge.check_operation(&bob.id(), 4, 1, &policy),
        Err(PositionError::WriterNotAuthorized)
    );
    judge.policy_entry(4, &policy).unwrap();
    // After the demotion's entry, Bob's record at revision 3 fails.
    assert_eq!(
        judge.check_operation(&bob.id(), 3, 1, &policy),
        Err(PositionError::WriterNotAuthorized)
    );
    assert_eq!(judge.check_operation(&alice.id(), 4, 1, &policy), Ok(()));
    judge.policy_entry(5, &policy).unwrap();
    // Between the removal's entry and the next epoch entry, nothing is
    // epoch-valid.
    assert!(judge.rotation_pending(&policy));
    assert_eq!(
        judge.check_operation(&alice.id(), 5, 1, &policy),
        Err(PositionError::EpochStale)
    );
    judge.epoch_entry(&e2.body);
    assert_eq!(judge.check_operation(&alice.id(), 5, 2, &policy), Ok(()));
    // An old-epoch record after a newer epoch entry.
    assert_eq!(
        judge.check_operation(&alice.id(), 5, 1, &policy),
        Err(PositionError::EpochStale)
    );

    // The same rules after a bootstrap from a checkpoint that covered the
    // removal's entry but not the rotation: still pending.
    let judge = LogJudge::from_checkpoint(WS_DOMAIN, 5, 1, 1);
    assert_eq!(
        judge.check_operation(&alice.id(), 5, 1, &policy),
        Err(PositionError::EpochStale)
    );
    let judge = LogJudge::from_checkpoint(WS_DOMAIN, 5, 2, 5);
    assert_eq!(judge.check_operation(&alice.id(), 5, 2, &policy), Ok(()));
}

#[test]
fn an_epoch_entry_signed_before_a_removal_leaves_the_rotation_pending() {
    // A malicious server places, after removal r's entry, an epoch record
    // that a former reader signed at a revision below r: the rotation that r
    // requires stays pending, so nothing under that epoch is valid.
    let mut w = workspace();
    let alice = w.alice.first().clone();
    let bob = w.bob.first().clone();
    w.policy.push(
        &alice,
        vec![w.alice.chain.head()],
        PolicyKind::RemoveMember {
            account: w.bob.account(),
        },
    );
    let policy = w.verified();
    let e1 = epoch_record(&policy, WS_DOMAIN, 1, None, 1, &alice, &key(1), None);
    let backdated = epoch_record(
        &policy,
        WS_DOMAIN,
        2,
        Some(&e1),
        3,
        &bob,
        &key(2),
        Some(&key(1)),
    );
    let mut judge = LogJudge::new(WS_DOMAIN);
    for revision in 1..=4 {
        judge.policy_entry(revision, &policy).unwrap();
    }
    judge.epoch_entry(&e1.body);
    judge.epoch_entry(&backdated.body);
    assert!(judge.rotation_pending(&policy));
    assert_eq!(
        judge.check_operation(&alice.id(), 4, 2, &policy),
        Err(PositionError::EpochStale)
    );
    // And a client that accepted r refuses the record itself: the signer
    // can't read the domain at the head it names? It could at 3, so the
    // record verifies, but an envelope from Bob for a client at head 4
    // fails.
    let env = envelope(&bob, &alice, &backdated.body);
    assert_eq!(
        check_envelope(
            &env,
            &backdated.body,
            &key(2),
            &alice.card,
            &policy,
            4,
            Some(4)
        ),
        Err(KeyError::Signer)
    );
    let _ = derive_key(context::OPERATION_KEY, &[0; 32]);
}
