//! Share links (§24): who creates, lists, updates and revokes them, the
//! checks the server can make without the link's keys, expiry, the limit of
//! 1,000 active links, and share writes racing pushes and policy changes.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use sync_protocol::codec::{Bytes, Encode, List, Text};
use sync_protocol::crypto::{SigningKey, random_array, random_nonce};
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::messages::{PushRequest, RecordRef};
use sync_protocol::records::{
    Operation, PolicyChange, PolicyKind, Role, Share, SharePayload, Signed, open_share,
    share_token_hash,
};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};

use super::{World, account, add_domain, head_of, inject_change, manager, refusal};
use crate::{Backend, Session, ShareCreation, Store};

/// A link's secrets, as the creating device keeps them.
pub struct Link {
    /// The 16-byte token.
    pub token: [u8; 16],
    /// The link key.
    pub key: SecretKey,
    /// The signing key.
    pub signing: SigningKey,
}

impl Link {
    /// A fresh link.
    pub fn new() -> Self {
        Self {
            token: random_array().expect("random"),
            key: SecretKey::random().expect("random"),
            signing: SigningKey::random().expect("random"),
        }
    }

    /// The token's hash.
    pub fn hash(&self) -> Digest {
        share_token_hash(&self.token)
    }

    /// The snapshot at `revision` of a note with `markdown`.
    pub fn snapshot(&self, revision: u64, markdown: &str) -> Signed<Share> {
        Share::seal(
            self.hash(),
            revision,
            &SharePayload {
                title: Text("Plan".into()),
                markdown: Bytes(markdown.as_bytes().to_vec()),
            },
            &self.key,
            &self.signing,
            random_nonce().expect("random"),
        )
        .expect("seals")
    }

    /// The creation of this link in `domain` for `days` days.
    pub fn creation(&self, domain: Id, days: u8, markdown: &str) -> ShareCreation {
        ShareCreation {
            token_hash: self.hash(),
            domain,
            expires_in_days: days,
            share: self.snapshot(1, markdown),
        }
    }
}

impl Default for Link {
    fn default() -> Self {
        Self::new()
    }
}

/// A member account with one device, added to the world's workspace with
/// `role`.
async fn member<B: Backend>(world: &mut World<B>, role: Role) -> (Id, Session) {
    let (member, keys, links) = account(1);
    world
        .store
        .append_account_links(links.clone())
        .await
        .expect("chain");
    let add = world.change(
        0,
        vec![head_of(&world.links), head_of(&links)],
        PolicyKind::AddMember {
            account: member,
            role,
            invitation: Id::random().expect("random"),
        },
    );
    world.inject_policy(add, vec![world.domain], None).await;
    (member, Session::new(member, keys[0].device()))
}

/// Creation, retries, the checks on a snapshot, listing, Update link from
/// the creating device only, revocation by another owner or admin device,
/// and editors kept out (§24).
pub async fn share_links_follow_section_24<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 2).await;
    let (owner, other) = (world.devices[0].session, world.devices[1].session);
    let (w, d) = (world.workspace, world.domain);
    let store = world.store.clone();
    let link = Link::new();
    let creation = link.creation(d, 30, "# Plan\n\nTomatoes in May.\n");
    let now = store.clock().now();
    let expires_at = store
        .create_share(owner, w, creation.clone())
        .await
        .expect("an owner creates a link");
    assert!((now + 30 * 86_400..now + 30 * 86_400 + 5).contains(&expires_at));
    assert_eq!(
        store.create_share(owner, w, creation.clone()).await.ok(),
        Some(expires_at),
        "a retry with identical bytes returns the original result"
    );
    let other_bytes = ShareCreation {
        share: link.snapshot(1, "Other"),
        ..creation.clone()
    };
    assert_eq!(
        refusal(store.create_share(owner, w, other_bytes).await).code,
        "id_conflict"
    );
    let shorter = ShareCreation {
        expires_in_days: 29,
        ..creation.clone()
    };
    assert_eq!(
        refusal(store.create_share(owner, w, shorter).await).code,
        "id_conflict"
    );

    let fresh = Link::new();
    for days in [0, 31] {
        let creation = fresh.creation(d, days, "x");
        assert_eq!(
            refusal(store.create_share(owner, w, creation).await).code,
            "bad_request",
            "{days} days"
        );
    }
    let mut unpadded = fresh.creation(d, 1, "x");
    unpadded.share.body.ciphertext.0.push(0);
    assert_eq!(
        refusal(store.create_share(owner, w, unpadded).await).code,
        "padding_invalid"
    );
    let mut misnamed = fresh.creation(d, 1, "x");
    misnamed.token_hash = link.hash();
    assert_eq!(
        refusal(store.create_share(owner, w, misnamed).await).code,
        "bad_request"
    );
    let mut later = fresh.creation(d, 1, "x");
    later.share = fresh.snapshot(2, "x");
    assert_eq!(
        refusal(store.create_share(owner, w, later).await).code,
        "bad_request"
    );
    let elsewhere = fresh.creation(Id::random().expect("random"), 1, "x");
    assert_eq!(
        refusal(store.create_share(owner, w, elsewhere).await).code,
        "not_found"
    );

    let served = store
        .read_share(link.token)
        .await
        .expect("the viewer reads it");
    let (_, payload) =
        open_share(&served.to_bytes(), &link.key, &link.signing.public()).expect("opens");
    assert_eq!(payload.markdown.0, b"# Plan\n\nTomatoes in May.\n");
    let listed = store.shares(other, w).await.expect("listed");
    assert_eq!(listed.shares.0.len(), 1);
    let info = &listed.shares.0[0];
    assert_eq!(
        (info.token_hash, info.domain, info.device, info.expires_at),
        (link.hash(), d, owner.device, expires_at)
    );

    store
        .update_share(owner, w, link.hash(), link.snapshot(2, "Peppers too."))
        .await
        .expect("the creating device updates its link");
    let (share, payload) = open_share(
        &store.read_share(link.token).await.expect("read").to_bytes(),
        &link.key,
        &link.signing.public(),
    )
    .expect("opens");
    assert_eq!(share.body.snapshot_revision, 2);
    assert_eq!(payload.markdown.0, b"Peppers too.");
    assert_eq!(
        refusal(
            store
                .update_share(owner, w, link.hash(), link.snapshot(2, "Again"))
                .await
        )
        .code,
        "bad_request",
        "an update raises snapshot_revision"
    );
    assert_eq!(
        refusal(
            store
                .update_share(other, w, link.hash(), link.snapshot(3, "Mine"))
                .await
        )
        .code,
        "forbidden",
        "only the creating device updates a link"
    );

    let (_, editor) = member(&mut world, Role::Editor).await;
    let (_, admin) = member(&mut world, Role::Admin).await;
    let store = world.store.clone();
    let by_editor = Link::new().creation(d, 1, "x");
    assert_eq!(
        refusal(store.create_share(editor, w, by_editor).await).code,
        "forbidden"
    );
    assert_eq!(refusal(store.shares(editor, w).await).code, "forbidden");
    assert_eq!(
        refusal(store.revoke_share(editor, w, link.hash()).await).code,
        "forbidden"
    );
    let outsider = Session::new(Id::random().expect("random"), Id::random().expect("random"));
    assert_eq!(refusal(store.shares(outsider, w).await).code, "forbidden");

    store
        .revoke_share(admin, w, link.hash())
        .await
        .expect("an admin revokes an owner's link");
    assert_eq!(
        refusal(store.read_share(link.token).await).code,
        "not_found"
    );
    assert!(
        store
            .shares(owner, w)
            .await
            .expect("listed")
            .shares
            .0
            .is_empty()
    );
    store
        .revoke_share(owner, w, link.hash())
        .await
        .expect("revoking a link that's gone succeeds");
    assert_eq!(
        refusal(
            store
                .update_share(owner, w, link.hash(), link.snapshot(3, "Back"))
                .await
        )
        .code,
        "not_found"
    );
}

/// A link whose creating account left the workspace can't change any more
/// and stays listed with its creating device until someone revokes it or
/// it expires; expiry deletes the snapshot (§24).
pub async fn share_links_outlive_their_creator_until_they_expire<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let owner = world.devices[0].session;
    let (w, d) = (world.workspace, world.domain);
    let (admin_account, admin) = member(&mut world, Role::Admin).await;
    let store = world.store.clone();
    let link = Link::new();
    store
        .create_share(admin, w, link.creation(d, 2, "Draft"))
        .await
        .expect("an admin creates a link");
    let remove = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::RemoveMember {
            account: admin_account,
        },
    );
    world.inject_policy(remove, vec![world.domain], None).await;
    let store = world.store.clone();
    assert_eq!(
        refusal(
            store
                .update_share(admin, w, link.hash(), link.snapshot(2, "Forged"))
                .await
        )
        .code,
        "forbidden"
    );
    let listed = store.shares(owner, w).await.expect("listed").shares.0;
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].device, admin.device, "the list names its creator");
    store.read_share(link.token).await.expect("it stays");

    store.clock().advance(Duration::from_secs(2 * 86_400));
    assert_eq!(
        refusal(store.read_share(link.token).await).code,
        "not_found"
    );
    assert!(
        store
            .shares(owner, w)
            .await
            .expect("listed")
            .shares
            .0
            .is_empty()
    );
    assert!(store.delete_expired().await.expect("deleted") >= 1);
    let hash = link.hash();
    let row = store
        .backend()
        .read(Box::new(move |txn: &mut B::Read| {
            Box::pin(async move { crate::ReadTxn::share(txn, hash).await })
        }))
        .await
        .expect("read");
    assert_eq!(row, None, "expiry deleted the snapshot");
}

/// A workspace holds at most 1,000 active links; revoking one makes room.
pub async fn share_limit_holds_a_thousand_links<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let owner = world.devices[0].session;
    let (w, d) = (world.workspace, world.domain);
    let store = world.store.clone();
    let mut first = None;
    for _ in 0..crate::MAX_SHARES {
        let link = Link::new();
        store
            .create_share(owner, w, link.creation(d, 1, "x"))
            .await
            .expect("within the limit");
        first.get_or_insert(link.hash());
    }
    let extra = Link::new();
    assert_eq!(
        refusal(
            store
                .create_share(owner, w, extra.creation(d, 1, "x"))
                .await
        )
        .code,
        "share_limit"
    );
    store
        .revoke_share(owner, w, first.expect("a link"))
        .await
        .expect("revoked");
    store
        .create_share(owner, w, extra.creation(d, 1, "x"))
        .await
        .expect("room again");
}

/// Share creations, updates and revocations in several domains race
/// pushes into those domains and policy changes, on both instances,
/// without a deadlock (§15).
pub async fn share_writes_race_pushes_and_policy_changes<B: Backend>(
    first: Arc<Store<B>>,
    second: Arc<Store<B>>,
) {
    let mut world = World::on(first, 3).await;
    let mut domains = vec![world.domain];
    for _ in 0..2 {
        domains.push(add_domain(&mut world).await);
    }
    let written = domains.clone();
    let world = Arc::new(world);
    let head = Arc::new(std::sync::Mutex::new(world.head));
    let stores = [world.store.clone(), second];

    let mut writers = Vec::new();
    for device in 0..3 {
        let world = world.clone();
        let head = head.clone();
        let store = stores[device % 2].clone();
        let mut order = written.clone();
        order.rotate_left(device);
        writers.push(tokio::spawn(async move {
            let session = world.devices[device].session;
            let mut chains: BTreeMap<Id, (u64, Digest)> = BTreeMap::new();
            for _ in 0..6 {
                let mut ops: Vec<Signed<Operation>> = Vec::new();
                for domain in &order {
                    let link = Link::new();
                    store
                        .create_share(session, world.workspace, link.creation(*domain, 1, "x"))
                        .await
                        .expect("no creation fails or deadlocks");
                    store
                        .update_share(session, world.workspace, link.hash(), link.snapshot(2, "y"))
                        .await
                        .expect("no update fails or deadlocks");
                    let (revision, policy_head) = *head.lock().expect("the head");
                    let (counter, previous) =
                        chains.get(domain).copied().unwrap_or((0, Digest::ZERO));
                    let op = world.op(device, |op| {
                        op.domain = *domain;
                        op.counter = counter + 1;
                        op.previous = previous;
                        op.policy_revision = revision;
                        op.policy_head = policy_head;
                    });
                    chains.insert(*domain, (counter + 1, op.digest()));
                    ops.push(op);
                    if counter % 2 == 0 {
                        store
                            .revoke_share(session, world.workspace, link.hash())
                            .await
                            .expect("no revocation fails or deadlocks");
                    }
                }
                let body = PushRequest {
                    records: List(ops.iter().cloned().map(RecordRef::Operation).collect()),
                }
                .to_bytes();
                store
                    .push(session, world.workspace, body)
                    .await
                    .expect("no push fails or deadlocks");
            }
        }));
    }
    let changer = {
        let world = world.clone();
        let head = head.clone();
        let stores = stores.clone();
        tokio::spawn(async move {
            let mut domains = domains;
            for round in 0..4 {
                let (revision, digest) = *head.lock().expect("the head");
                let domain = Id::random().expect("random");
                let body = PolicyChange {
                    revision: revision + 1,
                    previous: digest,
                    ..world
                        .change(
                            0,
                            vec![head_of(&world.links)],
                            PolicyKind::CreateDomain {
                                domain,
                                marker_rid: Id::random().expect("random"),
                                access: manager(world.account),
                            },
                        )
                        .body
                };
                let change = Signed::sign(body, world.devices[0].keys.signing());
                let creator = &world.devices[0];
                let epoch = Signed::sign(
                    build_epoch(
                        &EpochParams {
                            workspace: world.workspace,
                            domain,
                            epoch: 1,
                            previous: Digest::ZERO,
                            policy_revision: change.body.revision,
                            policy_head: change.digest(),
                            rotating_device: creator.id(),
                            recipients: BTreeSet::from([creator.id()]),
                        },
                        &SecretKey::random().expect("random"),
                        None,
                        Nonce::ZERO,
                    ),
                    creator.keys.signing(),
                );
                let next = (change.body.revision, change.digest());
                inject_change(
                    &stores[round % 2],
                    change,
                    domains.clone(),
                    Some((domain, epoch)),
                )
                .await
                .expect("no policy change fails or deadlocks");
                domains.push(domain);
                *head.lock().expect("the head") = next;
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
        })
    };
    tokio::time::timeout(Duration::from_secs(120), async {
        for writer in writers {
            writer.await.expect("the writers end");
        }
        changer.await.expect("the changes end");
    })
    .await
    .expect("nothing deadlocks");
    let listed = world
        .store
        .shares(world.devices[0].session, world.workspace)
        .await
        .expect("listed")
        .shares
        .0;
    let devices: BTreeSet<Id> = listed.iter().map(|info| info.device).collect();
    assert_eq!(devices.len(), 3, "every device kept some links");
}
