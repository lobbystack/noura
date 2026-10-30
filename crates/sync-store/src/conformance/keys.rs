//! Conformance cases for the key records of a push (§8, §9):
//! `account_heads` changes with the envelopes their new cards need,
//! rotations and replacements, lone envelopes that answer re-wrap requests,
//! `keys/current`, and an account chain append racing pushes that record
//! the account's head.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use sync_key_envelope::v2::{DeviceKeys, EnvelopeHeader, next_link, wrap};
use sync_protocol::codec::{Encode, List, Nested, Sorted};
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::{KeyRecord, PushRequest, PushResponse, RecordRef};
use sync_protocol::records::{
    AccountHead, Card, Envelope, Epoch, EpochBundle, LinkKind, PolicyKind, Signed,
};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};

use super::{Device, World, head_of, push_failure, random_token, refusal};
use crate::{Backend, Error, Session, Store, WriteTxn};

impl<B: Backend> World<B> {
    /// Pushes `records` from `device`.
    pub async fn push_records(
        &self,
        device: usize,
        records: Vec<RecordRef>,
    ) -> crate::Result<PushResponse> {
        let body = PushRequest {
            records: List(records),
        }
        .to_bytes();
        self.store
            .push(self.devices[device].session, self.workspace, body)
            .await
    }

    /// An envelope of `key` for `card` at `epoch` of the workspace domain,
    /// wrapped by `wrapper`.
    pub fn envelope(
        &self,
        wrapper: usize,
        card: &Card,
        epoch: u64,
        key: &SecretKey,
    ) -> Signed<Envelope> {
        wrap(
            &EnvelopeHeader {
                workspace: self.workspace,
                domain: self.domain,
                epoch,
                wrapping_device: self.devices[wrapper].id(),
            },
            key,
            card,
            &self.devices[wrapper].keys,
        )
        .expect("wraps")
    }

    /// An epoch record of the workspace domain after `previous`, rotated by
    /// `rotator` at the policy head for `recipients`.
    pub fn epoch_after(
        &self,
        rotator: usize,
        epoch: u64,
        previous: &Signed<Epoch>,
        previous_key: &SecretKey,
        key: &SecretKey,
        recipients: &BTreeSet<Id>,
    ) -> Signed<Epoch> {
        Signed::sign(
            build_epoch(
                &EpochParams {
                    workspace: self.workspace,
                    domain: self.domain,
                    epoch,
                    previous: previous.digest(),
                    policy_revision: self.head.0,
                    policy_head: self.head.1,
                    rotating_device: self.devices[rotator].id(),
                    recipients: recipients.clone(),
                },
                key,
                Some(previous_key),
                sync_protocol::crypto::random_nonce().expect("random"),
            ),
            self.devices[rotator].keys.signing(),
        )
    }

    /// The workspace domain's epoch 1 record, from the store.
    pub async fn first_epoch(&self) -> Signed<Epoch> {
        let epochs = self
            .store
            .epochs(self.devices[0].session, self.workspace, self.domain, 1)
            .await
            .expect("epochs");
        epochs.epochs.0[0].0.clone()
    }

    /// A new device of the account with a session, whose card the store
    /// enrolled, added to the chain by the device at `adder`.
    pub async fn add_device(&mut self, adder: usize) -> (usize, AccountHead) {
        let keys = DeviceKeys::generate().expect("random");
        self.store
            .enroll_card(keys.card(self.account))
            .await
            .expect("enrolled");
        let session = Session::new(self.account, keys.device());
        let token = random_token();
        self.store
            .create_session(token, session)
            .await
            .expect("a session");
        let card = keys.card(self.account);
        self.devices.push(Device {
            keys,
            session,
            token,
        });
        let head = self
            .append_link(
                adder,
                LinkKind::AddDevice {
                    card: Nested(card),
                    offer: Digest::ZERO,
                },
            )
            .await;
        (self.devices.len() - 1, head)
    }

    fn bundle(&self, epoch: &Signed<Epoch>, recipients: &BTreeSet<Id>) -> RecordRef {
        RecordRef::EpochBundle(EpochBundle {
            epoch: Nested(epoch.clone()),
            recipients: Sorted::new(recipients.iter().copied().collect()).expect("sorted"),
        })
    }
}

/// `account_heads` (§8): a change whose heads bring a card in carries, in
/// the same push, a current-epoch envelope for that card, else
/// `recipients_invalid`; it commits a policy entry in every domain's log
/// and makes the card a device of the workspace. A change that records
/// nothing new, one written by its own account again, is `forbidden`, a
/// head off the chain or behind the recorded one `account_conflict`, and
/// the server takes no other change kind in a push yet.
pub async fn account_heads_changes_carry_their_envelopes<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let (phone, head) = world.add_device(0).await;
    let card = world.devices[phone].keys.card(world.account);
    let change = world.change(0, vec![head.clone()], PolicyKind::AccountHeads);
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![RecordRef::Policy(change.clone())])
                .await
        ),
        ("recipients_invalid", 0),
        "the new card's envelope is missing"
    );
    let wrong_epoch = world.envelope(0, &card, 2, &world.epoch_key);
    assert_eq!(
        push_failure(
            world
                .push_records(
                    0,
                    vec![
                        RecordRef::Policy(change.clone()),
                        RecordRef::Envelope(wrong_epoch)
                    ]
                )
                .await
        ),
        ("epoch_stale", 1)
    );
    let mut off = head.clone();
    off.head = Digest([9; 32]);
    let forged = world.change(0, vec![off], PolicyKind::AccountHeads);
    assert_eq!(
        push_failure(world.push_records(0, vec![RecordRef::Policy(forged)]).await),
        ("account_conflict", 0)
    );
    let other_kind = world.change(
        0,
        vec![head.clone()],
        PolicyKind::DeleteDomain {
            domain: Id::random().expect("random"),
        },
    );
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![RecordRef::Policy(other_kind)])
                .await
        ),
        ("bad_request", 0)
    );

    let envelope = world.envelope(0, &card, 1, &world.epoch_key);
    let pushed = world
        .push_records(
            0,
            vec![
                RecordRef::Policy(change.clone()),
                RecordRef::Envelope(envelope.clone()),
            ],
        )
        .await
        .expect("the change commits");
    assert_eq!(pushed.policy_revision, 2);
    assert_eq!(pushed.receipts.len(), 1, "one entry in the one domain");
    assert_eq!(pushed.receipts[0].record_digest, change.digest());
    world.head = (2, change.digest());
    let policy = world
        .store
        .policy(world.devices[phone].session, world.workspace, 0)
        .await
        .expect("the new device reads the policy");
    assert_eq!(policy.head_revision, 2);
    let keys = world
        .store
        .keys(
            world.devices[phone].session,
            world.workspace,
            0,
            world.devices[phone].id(),
        )
        .await
        .expect("keys");
    assert!(
        keys.entries
            .iter()
            .any(|e| matches!(&e.record, KeyRecord::Envelope(env) if *env == envelope))
    );

    // The account's own change already recorded that head.
    let again = world.change(0, vec![head.clone()], PolicyKind::AccountHeads);
    assert_eq!(
        push_failure(world.push_records(0, vec![RecordRef::Policy(again)]).await),
        ("forbidden", 0)
    );
    let mut behind = head;
    behind.seqno = 1;
    behind.head = world.links[0].digest();
    let behind = world.change(0, vec![behind], PolicyKind::AccountHeads);
    assert_eq!(
        push_failure(world.push_records(0, vec![RecordRef::Policy(behind)]).await),
        ("account_conflict", 0)
    );
    // An envelope that answers no re-wrap request.
    let lone = world.envelope(0, &card, 1, &world.epoch_key);
    assert_eq!(
        push_failure(world.push_records(0, vec![RecordRef::Envelope(lone)]).await),
        ("forbidden", 0)
    );
}

/// Rotation (§9): once a recorded head takes a card out, the domain takes
/// no operation or lone envelope (`rotation_pending`) until an epoch record
/// one above the latest commits, at or above the revision that requires it
/// (else `rotation_not_due`), for exactly the domain's readers (else
/// `recipients_invalid`), with an envelope for each. A second record for
/// that epoch is `epoch_conflict`, and the older epoch's envelopes go.
pub async fn rotations_follow_the_policy<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 3).await;
    let first = world.first_epoch().await;
    let removed = world.devices[2].id();
    let head = world
        .append_link(
            0,
            LinkKind::Remove {
                device: removed,
                keep: Sorted::default(),
            },
        )
        .await;
    let change = world.change(0, vec![head], PolicyKind::AccountHeads);
    world
        .push_records(0, vec![RecordRef::Policy(change.clone())])
        .await
        .expect("the removal's head");
    world.head = (2, change.digest());

    let op = world.op(0, |_| {});
    assert_eq!(
        push_failure(world.push(0, &[op]).await),
        ("rotation_pending", 0)
    );
    let card1 = world.devices[1].keys.card(world.account);
    let lone = world.envelope(0, &card1, 1, &world.epoch_key);
    assert_eq!(
        push_failure(world.push_records(0, vec![RecordRef::Envelope(lone)]).await),
        ("rotation_pending", 0)
    );

    let readers: BTreeSet<Id> = [world.devices[0].id(), world.devices[1].id()]
        .into_iter()
        .collect();
    let key = SecretKey::random().expect("random");
    let stale = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: world.workspace,
                domain: world.domain,
                epoch: 2,
                previous: first.digest(),
                policy_revision: 1,
                policy_head: world.genesis.digest(),
                rotating_device: world.devices[0].id(),
                recipients: readers.clone(),
            },
            &key,
            Some(&world.epoch_key),
            Nonce([1; 24]),
        ),
        world.devices[0].keys.signing(),
    );
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![world.bundle(&stale, &readers)])
                .await
        ),
        ("rotation_not_due", 0),
        "an epoch signed below the removal leaves the rotation pending"
    );
    let mut with_removed = readers.clone();
    with_removed.insert(removed);
    let wrong = world.epoch_after(0, 2, &first, &world.epoch_key, &key, &with_removed);
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![world.bundle(&wrong, &with_removed)])
                .await
        ),
        ("recipients_invalid", 0)
    );
    let second = world.epoch_after(0, 2, &first, &world.epoch_key, &key, &readers);
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![world.bundle(&second, &readers)])
                .await
        ),
        ("recipients_invalid", 0),
        "a rotation carries an envelope for each recipient"
    );
    let card0 = world.devices[0].keys.card(world.account);
    let rotated = world
        .push_records(
            0,
            vec![
                world.bundle(&second, &readers),
                RecordRef::Envelope(world.envelope(0, &card0, 2, &key)),
                RecordRef::Envelope(world.envelope(0, &card1, 2, &key)),
            ],
        )
        .await
        .expect("the rotation commits");
    assert_eq!(rotated.receipts.len(), 1);
    assert_eq!(rotated.receipts[0].record_digest, second.digest());

    let again = world.epoch_after(
        1,
        2,
        &first,
        &world.epoch_key,
        &SecretKey::random().unwrap(),
        &readers,
    );
    assert_eq!(
        push_failure(
            world
                .push_records(1, vec![world.bundle(&again, &readers)])
                .await
        ),
        ("epoch_conflict", 0)
    );
    let third = world.epoch_after(0, 3, &second, &key, &SecretKey::random().unwrap(), &readers);
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![world.bundle(&third, &readers)])
                .await
        ),
        ("rotation_not_due", 0)
    );
    // Writes go on under the new epoch, and the old epoch's envelopes went.
    let op = world.op(1, |op| op.epoch = 2);
    world.push(1, &[op]).await.expect("written under epoch 2");
    let keys = world
        .store
        .keys(
            world.devices[1].session,
            world.workspace,
            0,
            world.devices[1].id(),
        )
        .await
        .expect("keys");
    let envelopes: Vec<u64> = keys
        .entries
        .iter()
        .filter_map(|e| match &e.record {
            KeyRecord::Envelope(env) => Some(env.body.epoch),
            _ => None,
        })
        .collect();
    assert_eq!(envelopes, vec![2]);
}

/// A replacement (§9) skips at least one epoch: the server takes it while
/// only the skipped epoch's rotator wrote under it, and answers
/// `epoch_conflict` once another device that still reads the domain did.
pub async fn replacements_skip_only_their_rotators_epochs<B: Backend>(store: Store<B>) {
    let world = World::new(store, 2).await;
    let first = world.first_epoch().await;
    let readers: BTreeSet<Id> = world.devices.iter().map(Device::id).collect();
    let cards: Vec<Card> = world
        .devices
        .iter()
        .map(|d| d.keys.card(world.account))
        .collect();
    // Epoch 2, as a server would hold one that fails for its readers.
    let bad = SecretKey::random().expect("random");
    let second = world.epoch_after(1, 2, &first, &world.epoch_key, &bad, &readers);
    world.inject_epoch(second.clone(), readers.clone()).await;
    let key = SecretKey::random().expect("random");
    let replacement = world.epoch_after(0, 3, &first, &world.epoch_key, &key, &readers);
    let records = |replacement: &Signed<Epoch>, key: &SecretKey| {
        let mut records = vec![world.bundle(replacement, &readers)];
        for card in &cards {
            records.push(RecordRef::Envelope(world.envelope(
                0,
                card,
                replacement.body.epoch,
                key,
            )));
        }
        records
    };
    // The rotator of epoch 2 and a second device wrote under it.
    let written = world.op(0, |op| op.epoch = 2);
    world
        .push(0, &[written])
        .await
        .expect("written under epoch 2");
    assert_eq!(
        push_failure(world.push_records(0, records(&replacement, &key)).await),
        ("epoch_conflict", 0),
        "a device that still reads the domain used the skipped epoch"
    );

    // Only the skipped epoch's rotator used it in another workspace.
    let world = World::on(world.store.clone(), 2).await;
    let first = world.first_epoch().await;
    let readers: BTreeSet<Id> = world.devices.iter().map(Device::id).collect();
    let cards: Vec<Card> = world
        .devices
        .iter()
        .map(|d| d.keys.card(world.account))
        .collect();
    let second = world.epoch_after(1, 2, &first, &world.epoch_key, &bad, &readers);
    world.inject_epoch(second.clone(), readers.clone()).await;
    let written = world.op(1, |op| op.epoch = 2);
    world.push(1, &[written]).await.expect("the rotator wrote");
    let replacement = world.epoch_after(0, 3, &first, &world.epoch_key, &key, &readers);
    let mut records = vec![world.bundle(&replacement, &readers)];
    for card in &cards {
        records.push(RecordRef::Envelope(world.envelope(0, card, 3, &key)));
    }
    world
        .push_records(0, records)
        .await
        .expect("the replacement commits");
    // A replacement whose previous names the latest epoch skips none: with
    // no rotation due, the server answers rotation_not_due.
    let latest = world.epoch_after(
        0,
        4,
        &replacement,
        &key,
        &SecretKey::random().unwrap(),
        &readers,
    );
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![world.bundle(&latest, &readers)])
                .await
        ),
        ("rotation_not_due", 0)
    );
}

/// Re-wrapping (§9): a reader whose envelope failed asks once per epoch,
/// and the request reaches every reader in the key stream and in
/// `keys/current`. Other readers answer with lone envelopes, one each and
/// at most three in all; the requester's next key fetch that passes an
/// answer closes the request, and `keys/current` then lists it no more.
pub async fn rewraps_reach_readers_and_close<B: Backend>(store: Store<B>) {
    let world = World::new(store, 5).await;
    let requester = &world.devices[4];
    assert_eq!(
        refusal(
            world
                .store
                .rewrap(requester.session, world.workspace, world.domain, 2)
                .await
        )
        .code,
        "epoch_stale"
    );
    world
        .store
        .rewrap(requester.session, world.workspace, world.domain, 1)
        .await
        .expect("asked");
    world
        .store
        .rewrap(requester.session, world.workspace, world.domain, 1)
        .await
        .expect("asking again returns the original result");
    let reader = &world.devices[1];
    let current = world
        .store
        .current_keys(reader.session, world.workspace, reader.id())
        .await
        .expect("keys/current");
    let kinds: Vec<&str> = current
        .entries
        .iter()
        .map(|e| match &e.record {
            KeyRecord::Epoch(_) => "epoch",
            KeyRecord::Envelope(_) => "envelope",
            KeyRecord::Rewrap(r) => {
                assert_eq!((r.device, r.epoch), (requester.id(), 1));
                "rewrap"
            }
        })
        .collect();
    assert_eq!(kinds, vec!["epoch", "envelope", "rewrap"]);
    let stream = world
        .store
        .keys(reader.session, world.workspace, 0, reader.id())
        .await
        .expect("keys");
    assert!(
        stream
            .entries
            .iter()
            .any(|e| matches!(e.record, KeyRecord::Rewrap(_)))
    );

    let card = requester.keys.card(world.account);
    for answerer in 0..3 {
        world
            .push_records(
                answerer,
                vec![RecordRef::Envelope(world.envelope(
                    answerer,
                    &card,
                    1,
                    &world.epoch_key,
                ))],
            )
            .await
            .expect("an answer");
    }
    assert_eq!(
        push_failure(
            world
                .push_records(
                    0,
                    vec![RecordRef::Envelope(world.envelope(
                        0,
                        &card,
                        1,
                        &world.epoch_key
                    ))]
                )
                .await
        ),
        ("forbidden", 0),
        "one answer per wrapping device"
    );
    assert_eq!(
        push_failure(
            world
                .push_records(
                    3,
                    vec![RecordRef::Envelope(world.envelope(
                        3,
                        &card,
                        1,
                        &world.epoch_key
                    ))]
                )
                .await
        ),
        ("forbidden", 0),
        "the first few holders answer"
    );
    let head = world
        .store
        .keys(requester.session, world.workspace, 0, requester.id())
        .await
        .expect("keys")
        .head_key_seq;
    world
        .store
        .keys(requester.session, world.workspace, head, requester.id())
        .await
        .expect("the fetch that passes the answers");
    let current = world
        .store
        .current_keys(reader.session, world.workspace, reader.id())
        .await
        .expect("keys/current");
    assert!(
        !current
            .entries
            .iter()
            .any(|e| matches!(e.record, KeyRecord::Rewrap(_))),
        "the request closed"
    );
}

/// An account chain append that races pushes recording that account's
/// head never deadlocks (§15, §32): the append locks only the account's
/// chain row, and a push reads the chain after its policy lock.
pub async fn chain_appends_race_pushes_recording_the_head<B: Backend>(
    first: Arc<Store<B>>,
    second: Arc<Store<B>>,
) {
    let world = World::on(first.clone(), 1).await;
    let account = world.account;
    let links = Arc::new(tokio::sync::Mutex::new(world.links.clone()));
    let adder = world.devices[0].keys.clone();
    let rounds = 8;
    let appends = {
        let links = links.clone();
        let first = first.clone();
        tokio::spawn(async move {
            for _ in 0..rounds {
                let keys = DeviceKeys::generate().expect("random");
                first
                    .enroll_card(keys.card(account))
                    .await
                    .expect("enrolled");
                let mut links = links.lock().await;
                let link = next_link(
                    &adder,
                    &head_of(&links),
                    LinkKind::AddDevice {
                        card: Nested(keys.card(account)),
                        offer: Digest::ZERO,
                    },
                );
                first
                    .append_account_links(vec![link.clone()])
                    .await
                    .expect("appended");
                links.push(link);
            }
        })
    };
    let world = Arc::new(tokio::sync::Mutex::new(world));
    let pushes = {
        let links = links.clone();
        let world = world.clone();
        let second = second.clone();
        tokio::spawn(async move {
            let mut committed = 0;
            let mut recorded = 1;
            for _ in 0..rounds * 2 {
                let snapshot = links.lock().await.clone();
                let mut world = world.lock().await;
                let head = head_of(&snapshot);
                if head.seqno <= recorded {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                    continue;
                }
                let change = world.change(0, vec![head.clone()], PolicyKind::AccountHeads);
                let mut records = vec![RecordRef::Policy(change.clone())];
                for link in &snapshot[recorded as usize..] {
                    if let LinkKind::AddDevice { card, .. } = &link.body.kind {
                        records.push(RecordRef::Envelope(world.envelope(
                            0,
                            &card.0,
                            1,
                            &world.epoch_key,
                        )));
                    }
                }
                let body = PushRequest {
                    records: List(records),
                }
                .to_bytes();
                match second
                    .push(world.devices[0].session, world.workspace, body)
                    .await
                {
                    Ok(_) => {
                        world.head = (change.body.revision, change.digest());
                        recorded = head.seqno;
                        committed += 1;
                    }
                    Err(error) => panic!("a push recording a head on the chain: {error}"),
                }
            }
            committed
        })
    };
    let finished = tokio::time::timeout(Duration::from_secs(60), async {
        appends.await.expect("the appends end");
        pushes.await.expect("the pushes end")
    })
    .await
    .expect("no deadlock");
    assert!(finished >= 1, "some pushes recorded a later head");
}

/// The re-wrap schedule of `sync-v2-keys.json` (§9, §26), replayed against
/// the store: which requests the server takes, the three answers it
/// takes for each, what `keys` and `keys/current` list to each device, and
/// how a request closes, on the requester's fetch or with the domain's
/// next epoch.
pub async fn rewraps_follow_the_fixture_schedule<B: Backend>(store: Store<B>) {
    let doc = super::fixture("sync-v2-keys.json");
    let case = doc["cases"]
        .as_array()
        .expect("cases")
        .iter()
        .find(|c| c["kind"] == "rewrap_schedule")
        .expect("a re-wrap schedule");
    let setup = &case["setup"];
    let alice_names: Vec<&str> = setup["accounts"]["alice"]
        .as_array()
        .expect("alice's devices")
        .iter()
        .map(|n| n.as_str().expect("a name"))
        .collect();
    let mut world = World::new(store, alice_names.len()).await;
    let mut names: Vec<String> = alice_names.iter().map(|n| (*n).to_owned()).collect();

    // Bob joins as an editor through a change that carries no envelope.
    let (bob, bob_keys, bob_links) = super::account(1);
    world
        .store
        .append_account_links(bob_links.clone())
        .await
        .expect("bob's chain");
    let add = world.change(
        0,
        vec![head_of(&world.links), head_of(&bob_links)],
        PolicyKind::AddMember {
            account: bob,
            role: sync_protocol::records::Role::Editor,
            invitation: Id::random().expect("random"),
        },
    );
    let domain = world.domain;
    world.inject_policy(add, vec![domain], None).await;
    for keys in bob_keys {
        let session = Session::new(bob, keys.device());
        let token = random_token();
        world
            .store
            .create_session(token, session)
            .await
            .expect("a session");
        world.devices.push(Device {
            keys,
            session,
            token,
        });
    }
    names.push(
        setup["accounts"]["bob"][0]
            .as_str()
            .expect("bob's device")
            .to_owned(),
    );

    // The restricted domain, which only alice's devices read, with the
    // envelopes the setup lists.
    let restricted = Id::random().expect("random");
    let creator = names
        .iter()
        .position(|n| n == setup["restricted"]["created_by"].as_str().expect("a name"))
        .expect("the creator");
    let create = world.change(
        creator,
        vec![head_of(&world.links)],
        PolicyKind::CreateDomain {
            domain: restricted,
            marker_rid: Id::random().expect("random"),
            access: super::manager(world.account),
        },
    );
    let restricted_key = SecretKey::random().expect("random");
    let alice_devices: BTreeSet<Id> = world.devices[..alice_names.len()]
        .iter()
        .map(|d| d.id())
        .collect();
    let restricted_epoch = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: world.workspace,
                domain: restricted,
                epoch: 1,
                previous: Digest::ZERO,
                policy_revision: world.head.0 + 1,
                policy_head: create.digest(),
                rotating_device: world.devices[creator].id(),
                recipients: alice_devices,
            },
            &restricted_key,
            None,
            Nonce::ZERO,
        ),
        world.devices[creator].keys.signing(),
    );
    world
        .inject_policy(
            create,
            vec![domain],
            Some((restricted, restricted_epoch.clone())),
        )
        .await;
    let mut rows = vec![(crate::push::KEY_EPOCH, None, restricted_epoch.to_bytes())];
    for holder in setup["restricted"]["envelopes"]
        .as_array()
        .expect("envelopes")
    {
        let at = names
            .iter()
            .position(|n| n == holder.as_str().expect("a name"))
            .expect("a device");
        let device = &world.devices[at];
        let envelope = wrap(
            &EnvelopeHeader {
                workspace: world.workspace,
                domain: restricted,
                epoch: 1,
                wrapping_device: world.devices[creator].id(),
            },
            &restricted_key,
            &device.keys.card(world.account),
            &world.devices[creator].keys,
        )
        .expect("wraps");
        rows.push((
            crate::push::KEY_ENVELOPE,
            Some(device.id()),
            envelope.to_bytes(),
        ));
    }
    let workspace = world.workspace;
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move {
                txn.lock(&crate::LockPlan {
                    workspace: Some(workspace),
                    policy_exclusive: false,
                    key_sequence: true,
                    domains: vec![restricted],
                    blobs: Vec::new(),
                })
                .await?;
                for (kind, recipient, record) in rows {
                    txn.append_key_entry(workspace, kind, restricted, 1, recipient, &record)
                        .await?;
                }
                Ok(())
            })
        }))
        .await
        .expect("the restricted domain's key rows");

    // The replay.
    let at = |name: &Value| -> usize {
        let name = name.as_str().expect("a device name");
        names
            .iter()
            .position(|n| n == name)
            .unwrap_or_else(|| panic!("no device {name}"))
    };
    let domain_of = |name: &Value| -> Id {
        match name.as_str().expect("a domain name") {
            "workspace" => domain,
            "restricted" => restricted,
            other => panic!("no domain {other}"),
        }
    };
    let domain_name = |id: Id| {
        if id == domain {
            "workspace"
        } else {
            "restricted"
        }
    };
    let outcome = |result: crate::Result<()>| -> String {
        match result {
            Ok(()) => "ok".to_owned(),
            Err(Error::Refused(refusal)) => refusal.code.to_owned(),
            Err(other) => panic!("a store failure: {other:?}"),
        }
    };
    let mut workspace_epochs: Vec<(Signed<Epoch>, SecretKey)> =
        vec![(world.first_epoch().await, world.epoch_key.clone())];
    let mut removed: BTreeSet<Id> = BTreeSet::new();
    for (index, step) in case["steps"].as_array().expect("steps").iter().enumerate() {
        let label = format!("step {index}: {step}");
        let device = step.get("device").map(at);
        match step["action"].as_str().expect("an action") {
            "rewrap" => {
                let device = &world.devices[device.expect("a device")];
                let result = world
                    .store
                    .rewrap(
                        device.session,
                        workspace,
                        domain_of(&step["domain"]),
                        step["epoch"].as_u64().expect("an epoch"),
                    )
                    .await;
                assert_eq!(outcome(result), step["expect"], "{label}");
            }
            "answer" => {
                let wrapper = at(&step["wrapper"]);
                let recipient = &world.devices[at(&step["recipient"])];
                let target = domain_of(&step["domain"]);
                let epoch = step["epoch"].as_u64().expect("an epoch");
                let key = if target == restricted {
                    restricted_key.clone()
                } else {
                    workspace_epochs[epoch as usize - 1].1.clone()
                };
                let envelope = wrap(
                    &EnvelopeHeader {
                        workspace,
                        domain: target,
                        epoch,
                        wrapping_device: world.devices[wrapper].id(),
                    },
                    &key,
                    &recipient.keys.card(recipient.session.account),
                    &world.devices[wrapper].keys,
                )
                .expect("wraps");
                let result = world
                    .push_records(wrapper, vec![RecordRef::Envelope(envelope)])
                    .await
                    .map(|_| ());
                assert_eq!(outcome(result), step["expect"], "{label}");
            }
            kind @ ("current_rewraps" | "keys_rewraps") => {
                let device = &world.devices[device.expect("a device")];
                let response = if kind == "current_rewraps" {
                    world
                        .store
                        .current_keys(device.session, workspace, device.id())
                        .await
                } else {
                    world
                        .store
                        .keys(device.session, workspace, 0, device.id())
                        .await
                }
                .expect("a key read");
                let listed: Vec<Value> = response
                    .entries
                    .iter()
                    .filter_map(|e| match &e.record {
                        KeyRecord::Rewrap(r) => Some(json!({
                            "domain": domain_name(r.domain),
                            "device": names[world.devices.iter().position(|d| d.id() == r.device).expect("a device")],
                            "epoch": r.epoch,
                        })),
                        _ => None,
                    })
                    .collect();
                assert_eq!(json!(listed), step["expect"], "{label}");
            }
            "fetch_keys" => {
                let device = &world.devices[device.expect("a device")];
                let head = world
                    .store
                    .keys(device.session, workspace, 0, device.id())
                    .await
                    .expect("keys")
                    .head_key_seq;
                world
                    .store
                    .keys(device.session, workspace, head, device.id())
                    .await
                    .expect("the fetch after the head");
            }
            "current_keys" => {
                let device = &world.devices[device.expect("a device")];
                world
                    .store
                    .current_keys(device.session, workspace, device.id())
                    .await
                    .expect("keys/current");
            }
            "remove" => {
                let signer = at(&step["signer"]);
                let gone = world.devices[at(&step["device"])].id();
                let head = world
                    .append_link(
                        signer,
                        LinkKind::Remove {
                            device: gone,
                            keep: Sorted::default(),
                        },
                    )
                    .await;
                let change = world.change(signer, vec![head], PolicyKind::AccountHeads);
                let result = world
                    .push_records(signer, vec![RecordRef::Policy(change.clone())])
                    .await
                    .map(|_| ());
                assert_eq!(outcome(result), step["expect"], "{label}");
                world.head = (change.body.revision, change.digest());
                removed.insert(gone);
            }
            "rotate" => {
                assert_eq!(
                    step["domain"], "workspace",
                    "{label}: rotations of the workspace domain"
                );
                let signer = at(&step["signer"]);
                let readers: BTreeSet<Id> = world
                    .devices
                    .iter()
                    .map(|d| d.id())
                    .filter(|id| !removed.contains(id))
                    .collect();
                let (previous, previous_key) = workspace_epochs.last().expect("an epoch").clone();
                let key = SecretKey::random().expect("random");
                let next = world.epoch_after(
                    signer,
                    previous.body.epoch + 1,
                    &previous,
                    &previous_key,
                    &key,
                    &readers,
                );
                let mut records = vec![world.bundle(&next, &readers)];
                for reader in world.devices.iter().filter(|d| readers.contains(&d.id())) {
                    records.push(RecordRef::Envelope(world.envelope(
                        signer,
                        &reader.keys.card(reader.session.account),
                        next.body.epoch,
                        &key,
                    )));
                }
                let result = world.push_records(signer, records).await.map(|_| ());
                assert_eq!(outcome(result), step["expect"], "{label}");
                workspace_epochs.push((next, key));
            }
            other => panic!("unknown action {other}"),
        }
    }
}
