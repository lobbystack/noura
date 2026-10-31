//! Conformance cases for restricted domains in a push (§8, §9):
//! `create_domain` with its epoch 1, envelopes and marker, what an outside
//! member receives of the domain, `set_access` and `delete_domain`.

use sync_key_envelope::v2::{EnvelopeHeader, wrap};
use sync_protocol::codec::{Bytes, Encode, List, Nested, Sorted};
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::{
    KeyEntry, KeyRecord, PageStatus, PullCursor, PullRequest, PushRequest, RecordRef,
};
use sync_protocol::records::{
    Access, AccessList, Epoch, EpochBundle, Level, Operation, PolicyChange, PolicyKind, Role,
    Signed,
};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};

use super::invitations::{Invitee, requested, requested_with_cards};
use super::{World, head_of, manager, push_failure};
use crate::{Backend, Store};

/// What a push of `create_domain` carries.
struct Creating {
    domain: Id,
    marker: Id,
    key: SecretKey,
    change: Signed<PolicyChange>,
    epoch: Signed<Epoch>,
    recipients: Vec<Id>,
}

impl Creating {
    /// A `create_domain` that device `signer` signs with `access`, and its
    /// epoch 1 for the devices of the owner's account.
    fn new<B: Backend>(world: &World<B>, signer: usize, access: AccessList) -> Self {
        let domain = Id::random().expect("random");
        let marker = Id::random().expect("random");
        let key = SecretKey::random().expect("random");
        let change = world.change(
            signer,
            vec![head_of(&world.links)],
            PolicyKind::CreateDomain {
                domain,
                marker_rid: marker,
                access,
            },
        );
        let recipients: Vec<Id> = world.devices.iter().map(|d| d.id()).collect();
        let mut sorted = recipients.clone();
        sorted.sort();
        let epoch = Signed::sign(
            build_epoch(
                &EpochParams {
                    workspace: world.workspace,
                    domain,
                    epoch: 1,
                    previous: Digest::ZERO,
                    policy_revision: change.body.revision,
                    policy_head: change.digest(),
                    rotating_device: world.devices[signer].id(),
                    recipients: sorted.iter().copied().collect(),
                },
                &key,
                None,
                Nonce::ZERO,
            ),
            world.devices[signer].keys.signing(),
        );
        Self {
            domain,
            marker,
            key,
            change,
            epoch,
            recipients: sorted,
        }
    }

    fn bundle(&self) -> RecordRef {
        RecordRef::EpochBundle(EpochBundle {
            epoch: Nested(self.epoch.clone()),
            recipients: Sorted::new(self.recipients.clone()).expect("sorted"),
        })
    }

    /// The envelope of this domain's key for each device of the account.
    fn envelopes<B: Backend>(&self, world: &World<B>, wrapper: usize) -> Vec<RecordRef> {
        world
            .devices
            .iter()
            .map(|device| {
                RecordRef::Envelope(
                    wrap(
                        &EnvelopeHeader {
                            workspace: world.workspace,
                            domain: self.domain,
                            epoch: 1,
                            wrapping_device: world.devices[wrapper].id(),
                        },
                        &self.key,
                        &device.keys.card(world.account),
                        &world.devices[wrapper].keys,
                    )
                    .expect("wraps"),
                )
            })
            .collect()
    }

    /// The marker's operation, sealed under nothing the server can read.
    fn marker_op<B: Backend>(&self, world: &World<B>, signer: usize) -> RecordRef {
        RecordRef::Operation(Signed::sign(
            Operation {
                workspace: world.workspace,
                domain: self.domain,
                device: world.devices[signer].id(),
                counter: 1,
                previous: Digest::ZERO,
                epoch: 1,
                policy_revision: self.change.body.revision,
                policy_head: self.change.digest(),
                previous_attempt: Digest::ZERO,
                generation: world.store.restore_generation(),
                checkpoint_seq: 0,
                checkpoint: Digest::ZERO,
                rid: self.marker,
                delete: false,
                supersedes: Sorted::default(),
                blobs: Sorted::default(),
                nonce: Nonce::ZERO,
                ciphertext: Bytes(vec![0; 272]),
            },
            world.devices[signer].keys.signing(),
        ))
    }

    /// The whole push, as the creating device sends it.
    fn records<B: Backend>(&self, world: &World<B>, signer: usize) -> Vec<RecordRef> {
        let mut records = vec![RecordRef::Policy(self.change.clone()), self.bundle()];
        records.extend(self.envelopes(world, signer));
        records.push(self.marker_op(world, signer));
        records
    }
}

/// A push of `create_domain` makes the domain whole in one transaction: its
/// log starts with the change's entry, then epoch 1 and the marker, and the
/// epoch's creator, revision and envelopes are checked (§8, §9).
pub async fn create_domain_commits_with_its_epoch_and_marker<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 2).await;
    let access = manager(world.account);

    // Without its epoch the domain's readers would get no key.
    let creating = Creating::new(&world, 0, access.clone());
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![RecordRef::Policy(creating.change.clone())])
                .await
        ),
        ("recipients_invalid", 0)
    );
    // An envelope short.
    let mut short = vec![
        RecordRef::Policy(creating.change.clone()),
        creating.bundle(),
    ];
    short.extend(creating.envelopes(&world, 0).into_iter().take(1));
    assert_eq!(
        push_failure(world.push_records(0, short).await),
        ("recipients_invalid", 0)
    );
    // Epoch 1 comes from the device that signed the change.
    let mut other = creating.records(&world, 0);
    let forged = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: world.workspace,
                domain: creating.domain,
                epoch: 1,
                previous: Digest::ZERO,
                policy_revision: creating.change.body.revision,
                policy_head: creating.change.digest(),
                rotating_device: world.devices[1].id(),
                recipients: creating.recipients.iter().copied().collect(),
            },
            &creating.key,
            None,
            Nonce::ZERO,
        ),
        world.devices[0].keys.signing(),
    );
    other[1] = RecordRef::EpochBundle(EpochBundle {
        epoch: Nested(forged),
        recipients: Sorted::new(creating.recipients.clone()).expect("sorted"),
    });
    assert_eq!(
        push_failure(world.push_records(0, other).await).0,
        "forbidden",
        "the session's device signs every record it pushes"
    );
    // A refused push left no domain behind.
    let page = pull_domain(&world, 0, creating.domain).await;
    assert_eq!(page.status, PageStatus::NotFound);

    let pushed = world
        .push_records(0, creating.records(&world, 0))
        .await
        .expect("the domain commits");
    assert_eq!(pushed.policy_revision, 2);
    world.head = (2, creating.change.digest());

    let page = pull_domain(&world, 1, creating.domain).await;
    assert_eq!(page.status, PageStatus::Ok);
    let seqs: Vec<u64> = page.entries.iter().map(|e| e.seq).collect();
    assert_eq!(seqs, [1, 2, 3], "the entry, epoch 1 and the marker");
    assert!(matches!(
        page.entries[0].record,
        sync_protocol::messages::EntryRecord::Policy(_)
    ));
    assert!(matches!(
        page.entries[1].record,
        sync_protocol::messages::EntryRecord::Epoch(_)
    ));

    // A second epoch 1 finds the domain made.
    let again = creating.bundle();
    assert_eq!(
        push_failure(world.push_records(0, vec![again]).await).0,
        "epoch_conflict"
    );
}

async fn pull_domain<B: Backend>(
    world: &World<B>,
    device: usize,
    domain: Id,
) -> sync_protocol::messages::Page {
    pull_as(world, world.devices[device].session, domain).await
}

async fn pull_as<B: Backend>(
    world: &World<B>,
    session: crate::Session,
    domain: Id,
) -> sync_protocol::messages::Page {
    let request = PullRequest {
        cursors: List(vec![PullCursor { domain, after: 0 }]),
        wait_seconds: 0,
    };
    world
        .store
        .pull(session, world.workspace, request)
        .await
        .expect("a pull")
        .pages
        .0
        .remove(0)
}

/// Adds `invitee` as an editor, with its envelope of the workspace domain.
async fn add_member<B: Backend>(world: &mut World<B>) -> Invitee {
    let (invite, invitee) = requested(world, Role::Editor).await;
    let add = world.change(
        0,
        vec![head_of(&world.links), invitee.head()],
        PolicyKind::AddMember {
            account: invitee.account,
            role: Role::Editor,
            invitation: invite.invitation,
        },
    );
    let envelope = world.envelope(0, &invitee.keys.card(invitee.account), 1, &world.epoch_key);
    world
        .push_records(
            0,
            vec![
                RecordRef::Policy(add.clone()),
                RecordRef::Envelope(envelope),
            ],
        )
        .await
        .expect("the member joins");
    world.head = (add.body.revision, add.digest());
    invitee
}

/// An `add_member` for an account in 1,000 workspaces answers
/// `workspace_limit` (§27), and a member already here doesn't count again.
pub async fn an_account_in_a_thousand_workspaces_joins_no_more<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let (invite, invitee) = requested(&world, Role::Editor).await;
    let account = invitee.account;
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move {
                for i in 0..1_000u32 {
                    let mut id = [9; 16];
                    id[..4].copy_from_slice(&i.to_be_bytes());
                    crate::WriteTxn::upsert_member(txn, Id(id), account, 2).await?;
                }
                Ok(())
            })
        }))
        .await
        .expect("memberships");
    let add = world.change(
        0,
        vec![head_of(&world.links), invitee.head()],
        PolicyKind::AddMember {
            account,
            role: Role::Editor,
            invitation: invite.invitation,
        },
    );
    assert_eq!(
        push_failure(world.push_records(0, vec![RecordRef::Policy(add)]).await),
        ("workspace_limit", 0)
    );
}

/// Adds a member with one device to the world's devices, and returns its
/// index.
pub(super) async fn member_device<B: Backend>(world: &mut World<B>) -> usize {
    let invitee = add_member(world).await;
    let token = [world.devices.len() as u8 + 1; 32];
    world
        .store
        .create_session(token, invitee.session)
        .await
        .expect("a session");
    world.devices.push(crate::conformance::Device {
        keys: invitee.keys,
        session: invitee.session,
        token,
    });
    world.devices.len() - 1
}

fn of_domain(entry: &KeyEntry, domain: &Id) -> bool {
    match &entry.record {
        KeyRecord::Epoch(epoch) => epoch.body.domain == *domain,
        KeyRecord::Envelope(envelope) => envelope.body.domain == *domain,
        KeyRecord::Rewrap(request) => request.domain == *domain,
    }
}

/// A member outside a restricted domain gets a forbidden, empty page for
/// it, no key entry, and no frame that names it (§9).
pub async fn an_outside_member_receives_nothing_of_a_domain<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let outsider = add_member(&mut world).await;
    let mut watch = world
        .store
        .realtime_watch(outsider.session, world.workspace)
        .await
        .expect("it watches");

    let creating = Creating::new(&world, 0, manager(world.account));
    world
        .push_records(0, creating.records(&world, 0))
        .await
        .expect("the domain commits");
    world.head = (creating.change.body.revision, creating.change.digest());

    let page = pull_as(&world, outsider.session, creating.domain).await;
    assert_eq!(page.status, PageStatus::Forbidden);
    assert!(page.entries.is_empty());
    assert_eq!((page.head, page.checkpoint_seq), (0, 0));

    let keys = world
        .store
        .keys(outsider.session, world.workspace, 0, outsider.keys.device())
        .await
        .expect("its keys");
    assert!(
        keys.entries.iter().all(|e| !of_domain(e, &creating.domain)),
        "no epoch or envelope of the domain"
    );
    let current = world
        .store
        .current_keys(outsider.session, world.workspace, outsider.keys.device())
        .await
        .expect("its current keys");
    assert!(
        current
            .entries
            .iter()
            .all(|e| !of_domain(e, &creating.domain))
    );

    let frames = world
        .store
        .realtime_frames(outsider.session, world.workspace, &mut watch)
        .await
        .expect("its frames");
    for frame in &frames {
        match frame {
            sync_protocol::messages::Frame::Records { domain, .. }
            | sync_protocol::messages::Frame::Notify { domain, .. } => {
                assert_ne!(*domain, creating.domain);
            }
            _ => {}
        }
    }
    let refused = world
        .store
        .epochs(outsider.session, world.workspace, creating.domain, 1)
        .await;
    assert_eq!(super::refusal(refused).code, "forbidden");
}

/// A manager grants with envelopes for the new readers; an owner who
/// doesn't manage the domain only removes entries; a removal makes the
/// domain due; a deleted domain answers `not_found` (§8, §9).
pub async fn set_access_and_delete_domain_follow_the_policy<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let member = add_member(&mut world).await;
    let creating = Creating::new(&world, 0, manager(world.account));
    world
        .push_records(0, creating.records(&world, 0))
        .await
        .expect("the domain commits");
    world.head = (creating.change.body.revision, creating.change.digest());

    let granted = Sorted::new({
        let mut list = vec![
            Access {
                account: world.account,
                level: Level::Manage,
            },
            Access {
                account: member.account,
                level: Level::Read,
            },
        ];
        list.sort_by_key(|a| a.account);
        list
    })
    .expect("sorted");
    let grant = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::SetAccess {
            domain: creating.domain,
            access: granted,
        },
    );
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![RecordRef::Policy(grant.clone())])
                .await
        ),
        ("recipients_invalid", 0),
        "a grant carries the new reader's envelope"
    );
    let envelope = wrap(
        &EnvelopeHeader {
            workspace: world.workspace,
            domain: creating.domain,
            epoch: 1,
            wrapping_device: world.devices[0].id(),
        },
        &creating.key,
        &member.keys.card(member.account),
        &world.devices[0].keys,
    )
    .expect("wraps");
    world
        .push_records(
            0,
            vec![
                RecordRef::Policy(grant.clone()),
                RecordRef::Envelope(envelope),
            ],
        )
        .await
        .expect("the grant commits");
    world.head = (grant.body.revision, grant.digest());
    let page = pull_as(&world, member.session, creating.domain).await;
    assert_eq!(page.status, PageStatus::BeforeHistoryFloor);

    // Taking the member off makes the domain due, so it takes no record.
    let removal = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::SetAccess {
            domain: creating.domain,
            access: manager(world.account),
        },
    );
    world
        .push_records(0, vec![RecordRef::Policy(removal.clone())])
        .await
        .expect("the removal commits");
    world.head = (removal.body.revision, removal.digest());
    let page = pull_as(&world, member.session, creating.domain).await;
    assert_eq!(page.status, PageStatus::Forbidden);

    // The domain takes no record until a rotation, and neither a rotation
    // nor an envelope from the member taken off.
    let mut pending = creating.marker_op(&world, 0);
    if let RecordRef::Operation(op) = &mut pending {
        let mut body = op.body.clone();
        body.counter = 2;
        body.previous = op.digest();
        body.rid = Id::random().expect("random");
        body.policy_revision = removal.body.revision;
        body.policy_head = removal.digest();
        *op = Signed::sign(body, world.devices[0].keys.signing());
    }
    assert_eq!(
        push_failure(world.push_records(0, vec![pending]).await),
        ("rotation_pending", 0)
    );
    let next_key = SecretKey::random().expect("random");
    let stolen = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: world.workspace,
                domain: creating.domain,
                epoch: 2,
                previous: creating.epoch.digest(),
                policy_revision: grant.body.revision,
                policy_head: grant.digest(),
                rotating_device: member.keys.device(),
                recipients: creating.recipients.iter().copied().collect(),
            },
            &next_key,
            Some(&creating.key),
            sync_protocol::crypto::random_nonce().expect("random"),
        ),
        member.keys.signing(),
    );
    let rotation = PushRequest {
        records: List(vec![RecordRef::EpochBundle(EpochBundle {
            epoch: Nested(stolen),
            recipients: Sorted::new(creating.recipients.clone()).expect("sorted"),
        })]),
    };
    assert_eq!(
        push_failure(
            world
                .store
                .push(member.session, world.workspace, rotation.to_bytes())
                .await
        ),
        ("forbidden", 0),
        "the member taken off can't rotate"
    );
    let wrapped = wrap(
        &EnvelopeHeader {
            workspace: world.workspace,
            domain: creating.domain,
            epoch: 1,
            wrapping_device: member.keys.device(),
        },
        &creating.key,
        &world.devices[0].keys.card(world.account),
        &member.keys,
    )
    .expect("wraps");
    let lone = PushRequest {
        records: List(vec![RecordRef::Envelope(wrapped)]),
    };
    assert_eq!(
        push_failure(
            world
                .store
                .push(member.session, world.workspace, lone.to_bytes())
                .await
        ),
        ("forbidden", 0),
        "the member taken off can't wrap"
    );

    let delete = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::DeleteDomain {
            domain: creating.domain,
        },
    );
    world
        .push_records(0, vec![RecordRef::Policy(delete.clone())])
        .await
        .expect("the delete commits");
    world.head = (delete.body.revision, delete.digest());
    let page = pull_domain(&world, 0, creating.domain).await;
    assert_eq!(page.status, PageStatus::NotFound);
    assert_eq!(
        push_failure(
            world
                .push_records(0, vec![creating.marker_op(&world, 0)])
                .await
        ),
        ("not_found", 0)
    );
}

/// The list that grants `member` read access beside the owner's manage.
fn granted(owner: Id, member: Id) -> AccessList {
    let mut list = vec![
        Access {
            account: owner,
            level: Level::Manage,
        },
        Access {
            account: member,
            level: Level::Read,
        },
    ];
    list.sort_by_key(|a| a.account);
    Sorted::new(list).expect("sorted")
}

/// A grant sets the grantee's floor in the domain, at the change's own
/// entry when its push carries no checkpoint: the grantee reads nothing
/// earlier, while the domain's other readers read from the start, and a
/// checkpoint takes no free pass from a change that grants nothing (§9).
pub async fn a_grant_sets_the_grantees_history_floor<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let member = add_member(&mut world).await;
    let creating = Creating::new(&world, 0, manager(world.account));
    world
        .push_records(0, creating.records(&world, 0))
        .await
        .expect("the domain commits");
    world.head = (creating.change.body.revision, creating.change.digest());
    let grant = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::SetAccess {
            domain: creating.domain,
            access: granted(world.account, member.account),
        },
    );
    let envelope = wrap(
        &EnvelopeHeader {
            workspace: world.workspace,
            domain: creating.domain,
            epoch: 1,
            wrapping_device: world.devices[0].id(),
        },
        &creating.key,
        &member.keys.card(member.account),
        &world.devices[0].keys,
    )
    .expect("wraps");
    world
        .push_records(
            0,
            vec![RecordRef::Policy(grant), RecordRef::Envelope(envelope)],
        )
        .await
        .expect("the grant commits");

    let page = pull_as(&world, member.session, creating.domain).await;
    assert_eq!(page.status, PageStatus::BeforeHistoryFloor);
    let page = pull_domain(&world, 0, creating.domain).await;
    assert_eq!(page.status, PageStatus::Ok);
    assert!(
        page.entries.len() >= 3,
        "the owner reads the marker the grantee can't"
    );
}

/// The workspace's limits (§27): a 257th domain, a 513th active reader,
/// and an access list over 1,024 entries. A 16,777,216th card index can't
/// be reached here; `next_card_index` and `PolicyError::server_code` hold
/// that rule in `sync-protocol`.
pub async fn limits_follow_section_27<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    for _ in 0..255 {
        let creating = Creating::new(&world, 0, manager(world.account));
        world
            .push_records(0, creating.records(&world, 0))
            .await
            .expect("a domain within the limit");
        world.head = (creating.change.body.revision, creating.change.digest());
    }
    let creating = Creating::new(&world, 0, manager(world.account));
    assert_eq!(
        push_failure(world.push_records(0, creating.records(&world, 0)).await),
        ("domain_limit", 0),
        "a 257th domain"
    );

    // Two accounts of 256 cards beside the owner's one.
    let (invite, full) = requested_with_cards(&world, Role::Editor, 256).await;
    let add = world.change(
        0,
        vec![head_of(&world.links), full.0.head()],
        PolicyKind::AddMember {
            account: full.0.account,
            role: Role::Editor,
            invitation: invite.invitation,
        },
    );
    let mut records = vec![RecordRef::Policy(add.clone())];
    for keys in std::iter::once(&full.0.keys).chain(full.1.iter()) {
        records.push(RecordRef::Envelope(world.envelope(
            0,
            &keys.card(full.0.account),
            1,
            &world.epoch_key,
        )));
    }
    world
        .push_records(0, records)
        .await
        .expect("512 readers fit");
    world.head = (add.body.revision, add.digest());
    let (invite, over) = requested_with_cards(&world, Role::Editor, 256).await;
    let add = world.change(
        0,
        vec![head_of(&world.links), over.0.head()],
        PolicyKind::AddMember {
            account: over.0.account,
            role: Role::Editor,
            invitation: invite.invitation,
        },
    );
    assert_eq!(
        push_failure(world.push_records(0, vec![RecordRef::Policy(add)]).await),
        ("recipient_limit", 0),
        "a 513th reader of the workspace domain"
    );

    // 1,025 entries: the decoder refuses the list before any check.
    let mut list: Vec<Access> = (0u16..1_024)
        .map(|n| {
            let mut id = [0; 16];
            id[..2].copy_from_slice(&n.to_be_bytes());
            Access {
                account: Id(id),
                level: Level::Read,
            }
        })
        .collect();
    list.sort_by_key(|a| a.account);
    let change = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::SetAccess {
            domain: creating.domain,
            access: Sorted::new(list.clone()).expect("1,024 entries fit"),
        },
    );
    let entry = list[1_023].to_bytes();
    let mut extra = entry.clone();
    extra[..2].copy_from_slice(&[0xff, 0xff]);
    let bytes = change.to_bytes();
    let tail = bytes
        .windows(entry.len())
        .rposition(|window| window == entry)
        .expect("the last entry")
        + entry.len();
    let mut long = bytes[..tail].to_vec();
    long.extend_from_slice(&extra);
    long.extend_from_slice(&bytes[tail..]);
    let count = 1_024u32.to_be_bytes();
    let at = long
        .windows(4)
        .position(|window| window == count)
        .expect("the list's count");
    long[at..at + 4].copy_from_slice(&1_025u32.to_be_bytes());
    let record = RecordRef::Policy(change);
    let mut body = sync_protocol::messages::PushRequest {
        records: List(vec![record]),
    }
    .to_bytes();
    let start = body
        .windows(bytes.len())
        .position(|window| window == bytes)
        .expect("the record");
    body.splice(start..start + bytes.len(), long.iter().copied());
    let length = (long.len() as u32).to_be_bytes();
    body[start - 4..start].copy_from_slice(&length);
    let refused = world
        .store
        .push(world.devices[0].session, world.workspace, body)
        .await
        .expect_err("over the limit");
    assert_eq!(
        super::refusal(Err::<(), _>(refused)).code,
        "too_large",
        "1,025 access entries"
    );
}
