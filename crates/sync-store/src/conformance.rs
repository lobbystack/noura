//! The store conformance suite (§15, §32). Each store's tests run it with
//! [`conformance_tests!`](crate::conformance_tests), passing a function
//! that opens a fresh, migrated, empty store; every case drives the store
//! only through [`Store`] and the transaction traits, so SQLite and
//! PostgreSQL must pass the same cases.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use std::time::Duration;

use serde_json::{Value, json};
use sync_key_envelope::v2::{DeviceKeys, EnvelopeHeader, genesis_link, next_link, wrap};
use sync_protocol::codec::{Bytes, Decode, Encode, List, Nested, Sorted, Text};
use sync_protocol::crypto::random_nonce;
use sync_protocol::keys::{EpochParams, build_epoch};
use sync_protocol::live::{LiveOp, covers_for};
use sync_protocol::messages::{
    ChainConflictDetail, EntryRecord, KeyRecord, MAX_BODY, PageStatus, PullCursor, PullRequest,
    PushFailureDetail, PushRequest, PushResponse, RecordRef, server_error_status,
};
use sync_protocol::records::{
    Access, AccountHead, AccountLink, Card, Cover, EpochBundle, Level, LinkKind,
    MAX_OPERATION_CIPHERTEXT, OpClass, Operation, PolicyChange, PolicyKind, Profile,
    ProfilePayload, Signed,
};
use sync_protocol::types::{Digest, Id, Nonce, SecretKey};

use crate::{Backend, Error, LockPlan, Refusal, Session, Store, WriteTxn};

mod blobs;
mod checkpoints;
mod devices;
mod keys;
mod shares;
pub use blobs::*;
pub use checkpoints::*;
pub use devices::*;
pub use keys::*;
pub use shares::*;

/// Declares one `#[tokio::test]` per conformance case. `$open` is an async
/// function returning a fresh store and a guard that keeps its database
/// alive for the test.
#[macro_export]
macro_rules! conformance_tests {
    ($open:path) => {
        $crate::conformance_tests!(@cases $open;
            live_sets_match_the_fixture,
            creation_starts_the_domain_log,
            creation_retries_and_conflicts,
            domain_ids_belong_to_their_workspace,
            creation_checks_heads_signatures_and_recipients,
            push_returns_receipts_in_request_order,
            push_commits_all_or_none,
            retry_returns_the_original_receipt,
            operation_checks_answer_their_codes,
            empty_supersedes_on_an_existing_rid_is_a_patch,
            snapshot_required_past_the_patch_bytes,
            conflict_limit_past_64_live_states,
            removed_device_and_pending_rotation,
            policy_regression_and_stale_revisions,
            rid_belongs_to_one_domain,
            concurrent_pushes_commit_in_one_order,
            failed_pushes_leave_the_others_committed,
            pull_pages_each_domain,
            pull_stays_within_one_mebibyte,
            pull_keeps_room_for_every_page,
            pull_wakes_when_another_device_pushes,
            chain_positions_and_heads,
            keys_and_epochs_serve_records_without_recipients,
            pruning_lock_meets_no_policy_lock,
            creations_at_once_keep_the_workspace_limit,
            account_chain_appends_check_who_sends_them,
            enrolled_cards_bind_again_but_never_a_removed_id,
            sign_in_rows_work_once_and_expire,
            chain_reads_follow_the_policy,
            a_removed_member_reads_the_policy_up_to_its_removal,
            a_retired_chain_the_workspace_names_reads_in_full,
            recovery_proofs_consume_their_challenge,
            approval_requests_open_the_invitees_chain,
            device_grants_follow_rfc_8628,
            enrollment_binds_the_session_to_its_card,
            removing_links_end_sessions,
            active_devices_end_a_pending_cards_sessions,
            approval_requests_offers_and_reveals,
            account_heads_changes_carry_their_envelopes,
            rotations_follow_the_policy,
            replacements_skip_only_their_rotators_epochs,
            rewraps_reach_readers_and_close,
            rewraps_follow_the_fixture_schedule,
            profile_pushes_follow_section_25,
            blob_uploads_follow_tus,
            blob_digest_mismatch_discards_the_upload,
            pushes_list_completed_blobs_of_their_domain,
            blob_downloads_follow_the_listing,
            checkpoints_fall_due_by_volume,
            checkpoints_fall_due_by_weight,
            checkpoint_checks_match_the_fixture,
            checkpoint_heads_follow_the_fixture,
            racing_checkpoints_keep_the_first,
            a_checkpoint_goes_in_a_push_of_its_own,
            removals_make_checkpoints_due_and_leave_the_heads,
            replacements_make_checkpoints_due,
            checkpoint_routes_serve_the_chain_and_manifest,
            share_links_follow_section_24,
            share_links_outlive_their_creator_until_they_expire,
            share_limit_holds_a_thousand_links,
        );
    };
    (@cases $open:path; $($case:ident,)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                let (store, _guard) = $open().await;
                $crate::conformance::$case(store).await;
            }
        )*
    };
}

/// Declares one `#[tokio::test]` per case that races two instances of a
/// store on one database. `$open` is an async function returning two
/// instances and a guard that keeps their database alive; a store that one
/// instance serves returns that instance twice.
#[macro_export]
macro_rules! instance_tests {
    ($open:path) => {
        $crate::instance_tests!(@cases $open;
            removal_races_pushes,
            pushes_into_several_domains_race_policy_changes,
            limits_hold_across_instances,
            geneses_at_once_start_one_chain,
            enrollments_at_once_take_one_card,
            chain_appends_race_pushes_recording_the_head,
            blob_uploads_race_pushes_and_policy_changes,
            share_writes_race_pushes_and_policy_changes,
        );
    };
    (@cases $open:path; $($case:ident,)*) => {
        $(
            #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
            async fn $case() {
                let (first, second, _guard) = $open().await;
                $crate::conformance::$case(first, second).await;
            }
        )*
    };
}

/// One device of the test account.
pub struct Device {
    /// Its keys.
    pub keys: DeviceKeys,
    /// Its session.
    pub session: Session,
    /// Its session token, as a sign-in returns it.
    pub token: [u8; 32],
}

impl Device {
    /// The device ID.
    pub fn id(&self) -> Id {
        self.keys.device()
    }

    /// The bearer token's text form.
    pub fn bearer(&self) -> String {
        sync_protocol::types::to_base64url(&self.token)
    }
}

/// A store with an account of several devices, each signed in, and a
/// workspace that the first device created.
pub struct World<B: Backend> {
    /// The store.
    pub store: Arc<Store<B>>,
    /// The account.
    pub account: Id,
    /// The account chain's links.
    pub links: Vec<Signed<AccountLink>>,
    /// The devices; the first created the workspace.
    pub devices: Vec<Device>,
    /// The workspace.
    pub workspace: Id,
    /// The workspace domain.
    pub domain: Id,
    /// The genesis.
    pub genesis: Signed<PolicyChange>,
    /// The policy head's revision and digest.
    pub head: (u64, Digest),
    /// Epoch 1's key.
    pub epoch_key: SecretKey,
}

/// The head of an account chain's links.
pub fn head_of(links: &[Signed<AccountLink>]) -> AccountHead {
    let last = links.last().expect("a chain has links");
    AccountHead {
        account: last.body.account,
        genesis: links[0].digest(),
        seqno: last.body.seqno,
        head: last.digest(),
    }
}

/// An account chain of `count` devices: the first signs the genesis and
/// adds the others.
pub fn account(count: usize) -> (Id, Vec<DeviceKeys>, Vec<Signed<AccountLink>>) {
    let account = Id::random().expect("random");
    let keys: Vec<DeviceKeys> = (0..count)
        .map(|_| DeviceKeys::generate().expect("random"))
        .collect();
    let mut links = vec![genesis_link(&keys[0], account)];
    for other in &keys[1..] {
        let link = next_link(
            &keys[0],
            &head_of(&links),
            LinkKind::AddDevice {
                card: Nested(other.card(account)),
                offer: Digest::ZERO,
            },
        );
        links.push(link);
    }
    (account, keys, links)
}

/// The creation request of a workspace whose genesis records the head of
/// `links`, signed by `creator`, with envelopes for `cards`.
pub fn creation(
    creator: &DeviceKeys,
    links: &[Signed<AccountLink>],
    cards: &[Card],
) -> (Vec<u8>, Signed<PolicyChange>, SecretKey) {
    creation_with_domain(creator, links, cards, Id::random().expect("random"))
}

/// [`creation`] with a chosen workspace domain ID.
pub fn creation_with_domain(
    creator: &DeviceKeys,
    links: &[Signed<AccountLink>],
    cards: &[Card],
    domain: Id,
) -> (Vec<u8>, Signed<PolicyChange>, SecretKey) {
    let workspace = Id::random().expect("random");
    let genesis = Signed::sign(
        PolicyChange {
            workspace,
            revision: 1,
            previous: Digest::ZERO,
            signer: creator.device(),
            heads: Sorted::new(vec![head_of(links)]).expect("one head"),
            kind: PolicyKind::Genesis {
                workspace_domain: domain,
            },
        },
        creator.signing(),
    );
    let readers: BTreeSet<Id> = cards.iter().map(|c| c.device).collect();
    let key = SecretKey::random().expect("random");
    let epoch = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace,
                domain,
                epoch: 1,
                previous: Digest::ZERO,
                policy_revision: 1,
                policy_head: genesis.digest(),
                rotating_device: creator.device(),
                recipients: readers.clone(),
            },
            &key,
            None,
            Nonce::ZERO,
        ),
        creator.signing(),
    );
    let mut records = vec![
        RecordRef::Policy(genesis.clone()),
        RecordRef::EpochBundle(EpochBundle {
            epoch: Nested(epoch.clone()),
            recipients: Sorted::new(readers.into_iter().collect()).expect("sorted"),
        }),
    ];
    for card in cards {
        let envelope = wrap(
            &EnvelopeHeader {
                workspace,
                domain,
                epoch: 1,
                wrapping_device: creator.device(),
            },
            &key,
            card,
            creator,
        )
        .expect("wraps");
        records.push(RecordRef::Envelope(envelope));
    }
    let mut profile = Profile {
        workspace,
        profile_revision: 1,
        epoch: 1,
        policy_revision: 1,
        policy_head: genesis.digest(),
        signer: creator.device(),
        nonce: Nonce::ZERO,
        ciphertext: Bytes(Vec::new()),
    };
    profile.seal_payload(
        &ProfilePayload {
            name: Text("Conformance".into()),
        },
        &key,
        &epoch.digest(),
        random_nonce().expect("random"),
    );
    records.push(RecordRef::Profile(Signed::sign(profile, creator.signing())));
    let body = PushRequest {
        records: List(records),
    }
    .to_bytes();
    (body, genesis, key)
}

impl<B: Backend> World<B> {
    /// An account of `devices` devices, each with a session, and a
    /// workspace the first one created.
    pub async fn new(store: Store<B>, devices: usize) -> Self {
        Self::on(Arc::new(store), devices).await
    }

    /// [`World::new`] on a store that others share.
    pub async fn on(store: Arc<Store<B>>, devices: usize) -> Self {
        let (account, keys, links) = account(devices);
        store
            .append_account_links(links.clone())
            .await
            .expect("chain stored");
        let mut sessions = Vec::new();
        for key in keys {
            let session = Session {
                account,
                device: key.device(),
                recovery: None,
            };
            let token = random_token();
            store
                .create_session(token, session)
                .await
                .expect("session stored");
            sessions.push(Device {
                keys: key,
                session,
                token,
            });
        }
        let cards: Vec<Card> = sessions.iter().map(|d| d.keys.card(account)).collect();
        let (body, genesis, epoch_key) = creation(&sessions[0].keys, &links, &cards);
        store
            .create_workspace(sessions[0].session, body)
            .await
            .expect("workspace created");
        let PolicyKind::Genesis { workspace_domain } = genesis.body.kind else {
            unreachable!("a genesis");
        };
        Self {
            store,
            account,
            links,
            devices: sessions,
            workspace: genesis.body.workspace,
            domain: workspace_domain,
            head: (1, genesis.digest()),
            genesis,
            epoch_key,
        }
    }

    /// An operation of `device` in the workspace domain at counter 1 on a
    /// new object, with a 272-byte ciphertext, after `edit` changes it.
    pub fn op(&self, device: usize, edit: impl FnOnce(&mut Operation)) -> Signed<Operation> {
        let mut body = Operation {
            workspace: self.workspace,
            domain: self.domain,
            device: self.devices[device].id(),
            counter: 1,
            previous: Digest::ZERO,
            epoch: 1,
            policy_revision: self.head.0,
            policy_head: self.head.1,
            previous_attempt: Digest::ZERO,
            generation: self.store.restore_generation(),
            checkpoint_seq: 0,
            checkpoint: Digest::ZERO,
            rid: Id::random().expect("random"),
            delete: false,
            supersedes: Sorted::default(),
            blobs: Sorted::default(),
            nonce: Nonce::ZERO,
            ciphertext: Bytes(vec![0; 272]),
        };
        edit(&mut body);
        Signed::sign(body, self.devices[device].keys.signing())
    }

    /// Pushes `ops` from `device`.
    pub async fn push(
        &self,
        device: usize,
        ops: &[Signed<Operation>],
    ) -> crate::Result<PushResponse> {
        let body = PushRequest {
            records: List(ops.iter().cloned().map(RecordRef::Operation).collect()),
        }
        .to_bytes();
        self.store
            .push(self.devices[device].session, self.workspace, body)
            .await
    }

    /// Pulls the workspace domain from `after`, without waiting.
    pub async fn pull(&self, device: usize, after: u64) -> sync_protocol::messages::Page {
        let response = self
            .store
            .pull(
                self.devices[device].session,
                self.workspace,
                PullRequest {
                    cursors: List(vec![PullCursor {
                        domain: self.domain,
                        after,
                    }]),
                    wait_seconds: 0,
                },
            )
            .await
            .expect("pull");
        response.pages.0.into_iter().next().expect("one page")
    }

    /// Commits a policy change straight into the store, with a policy entry
    /// in each listed domain, as a push of that change will once tickets 47
    /// and 50 add them. `new_domain` also creates a domain with its epoch 1.
    pub async fn inject_policy(
        &mut self,
        change: Signed<PolicyChange>,
        domains: Vec<Id>,
        new_domain: Option<(Id, Signed<sync_protocol::records::Epoch>)>,
    ) {
        let head = (change.body.revision, change.digest());
        inject_change(&self.store, change, domains, new_domain)
            .await
            .expect("policy change injected");
        self.head = head;
    }

    /// Commits a rotation's epoch straight into the store: its log entry,
    /// its bundle and its key stream row, as a push of that epoch bundle
    /// will once ticket 47 adds it.
    pub async fn inject_epoch(
        &self,
        epoch: Signed<sync_protocol::records::Epoch>,
        recipients: BTreeSet<Id>,
    ) {
        let workspace = self.workspace;
        let domain = epoch.body.domain;
        let policy_revision = self.head.0;
        self.store
            .backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    // An epoch bundle's push: the policy lock shared, the
                    // key-sequence row and the domain's head row (§15).
                    txn.lock(&LockPlan {
                        workspace: Some(workspace),
                        policy_exclusive: false,
                        key_sequence: true,
                        domains: vec![domain],
                        blobs: Vec::new(),
                    })
                    .await?;
                    let record = epoch.to_bytes();
                    let bundle = EpochBundle {
                        epoch: Nested(epoch.clone()),
                        recipients: Sorted::new(recipients.into_iter().collect()).expect("sorted"),
                    };
                    let seq = txn.append_entry(workspace, domain, 8, &record).await?;
                    txn.insert_epoch(
                        workspace,
                        domain,
                        &crate::EpochRow {
                            epoch: epoch.body.epoch,
                            seq,
                            digest: epoch.digest(),
                            previous: epoch.body.previous,
                            record: record.clone(),
                        },
                        policy_revision,
                        &bundle.to_bytes(),
                    )
                    .await?;
                    txn.append_key_entry(workspace, 3, domain, epoch.body.epoch, None, &record)
                        .await?;
                    Ok(())
                })
            }))
            .await
            .expect("epoch injected");
    }

    /// Appends a link that `signer` signs to the account chain.
    pub async fn append_link(&mut self, signer: usize, kind: LinkKind) -> AccountHead {
        let link = next_link(&self.devices[signer].keys, &head_of(&self.links), kind);
        self.store
            .append_account_links(vec![link.clone()])
            .await
            .expect("link appended");
        self.links.push(link);
        head_of(&self.links)
    }

    /// A policy change after the head, signed by `signer`.
    pub fn change(
        &self,
        signer: usize,
        heads: Vec<AccountHead>,
        kind: PolicyKind,
    ) -> Signed<PolicyChange> {
        Signed::sign(
            PolicyChange {
                workspace: self.workspace,
                revision: self.head.0 + 1,
                previous: self.head.1,
                signer: self.devices[signer].id(),
                heads: Sorted::new(heads).expect("sorted heads"),
                kind,
            },
            self.devices[signer].keys.signing(),
        )
    }
}

/// Commits a policy change straight into `store`, as a push of that change
/// will once tickets 47 and 50 add them: under the workspace's exclusive
/// policy lock and the head rows of the domains it writes, it stores the
/// change, a policy entry in each listed domain and, for `new_domain`, the
/// new domain with its epoch 1.
pub async fn inject_change<B: Backend>(
    store: &Store<B>,
    change: Signed<PolicyChange>,
    domains: Vec<Id>,
    new_domain: Option<(Id, Signed<sync_protocol::records::Epoch>)>,
) -> crate::Result<()> {
    let workspace = change.body.workspace;
    let revision = change.body.revision;
    let digest = change.digest();
    store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move {
                let mut domains = domains;
                if let Some((domain, _)) = &new_domain {
                    domains.push(*domain);
                }
                domains.sort();
                domains.dedup();
                txn.lock(&LockPlan {
                    workspace: Some(workspace),
                    policy_exclusive: true,
                    key_sequence: false,
                    domains: domains.clone(),
                    blobs: Vec::new(),
                })
                .await?;
                crate::policy::store_change(txn, workspace, &change).await?;
                txn.set_policy_head(workspace, revision, digest).await?;
                if let Some((domain, _)) = &new_domain {
                    txn.insert_domain(workspace, *domain).await?;
                }
                let entry = sync_protocol::messages::PolicyEntry {
                    revision,
                    head: digest,
                };
                for domain in domains {
                    txn.append_entry(workspace, domain, 9, &entry.to_bytes())
                        .await?;
                }
                if let Some((domain, epoch)) = new_domain {
                    let bytes = epoch.to_bytes();
                    let seq = txn.append_entry(workspace, domain, 8, &bytes).await?;
                    txn.insert_epoch(
                        workspace,
                        domain,
                        &crate::EpochRow {
                            epoch: 1,
                            seq,
                            digest: epoch.digest(),
                            previous: Digest::ZERO,
                            record: bytes.clone(),
                        },
                        revision,
                        &bytes,
                    )
                    .await?;
                }
                Ok(())
            })
        }))
        .await
}

fn random_token() -> [u8; 32] {
    sync_protocol::crypto::random_array().expect("random")
}

/// The refusal inside a store error.
#[track_caller]
pub fn refusal(result: crate::Result<impl std::fmt::Debug>) -> Refusal {
    match result {
        Err(Error::Refused(refusal)) => refusal,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// A push refusal's code and failing record index.
#[track_caller]
pub fn push_failure(result: crate::Result<impl std::fmt::Debug>) -> (&'static str, u32) {
    let refusal = refusal(result);
    let detail = PushFailureDetail::from_bytes(refusal.detail.as_deref().expect("a push detail"))
        .expect("a push failure detail");
    assert_eq!(detail.code.as_str(), refusal.code);
    assert!(
        server_error_status(refusal.code).is_some(),
        "{} is a §26 code",
        refusal.code
    );
    (refusal.code, detail.record_index)
}

fn fixture(name: &str) -> Value {
    let path = format!(
        "{}/../../docs/workspace-format/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).expect("fixture")).expect("JSON")
}

fn id_of(value: &Value) -> Id {
    Id::from_hex(value.as_str().expect("hex")).expect("an ID")
}

fn u64_of(value: &Value) -> u64 {
    value.as_u64().expect("a number")
}

fn live_op(value: &Value) -> LiveOp {
    LiveOp {
        rid: id_of(&value["rid"]),
        device: id_of(&value["device"]),
        counter: u64_of(&value["counter"]),
        seq: u64_of(&value["seq"]),
        digest: Digest::from_hex(value["digest"].as_str().expect("hex")).expect("a digest"),
        delete: value["delete"].as_bool().expect("a flag"),
        covers: serde_json::from_value(value["covers"].clone()).expect("covers"),
        weight: u64_of(&value["weight"]),
    }
}

fn name<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_value(value)
        .expect("serializes")
        .as_str()
        .expect("a name")
        .to_owned()
}

/// `sync-v2-live.json`: the store classes operations and computes live sets
/// as clients do, floors and covers included.
pub async fn live_sets_match_the_fixture<B: Backend>(store: Store<B>) {
    let doc = fixture("sync-v2-live.json");
    for case in doc["cases"].as_array().expect("cases") {
        let workspace = Id::random().expect("random");
        let domain = Id::random().expect("random");
        let label = case["name"].as_str().expect("a name");
        // Matching files without a journal is the client's alone (§15).
        if matches!(
            case["kind"].as_str(),
            Some("journal_less_match" | "archived")
        ) {
            continue;
        }
        let ops: Vec<LiveOp> = case["ops"]
            .as_array()
            .expect("ops")
            .iter()
            .map(live_op)
            .collect();
        match case["kind"].as_str().expect("a kind") {
            "live_set" => {
                let floors = case["floors"].as_array().expect("floors");
                let mut classes = Vec::new();
                for at in 0..=ops.len() {
                    for floor in floors {
                        let after = floor
                            .get("after_op")
                            .map_or(ops.len(), |a| u64_of(a) as usize);
                        if after != at {
                            continue;
                        }
                        let dropped = store
                            .apply_floor(
                                workspace,
                                domain,
                                u64_of(&floor["previous_floor"]),
                                u64_of(&floor["floor"]),
                                u64_of(&floor["covered_seq"]),
                            )
                            .await
                            .expect("floor");
                        let ours: Vec<Value> = dropped
                            .iter()
                            .map(|d| json!({"rid": d.rid.to_hex(), "device": d.op.device.to_hex(), "counter": d.op.counter}))
                            .collect();
                        assert_eq!(json!(ours), floor["dropped"], "{label}: dropped deletes");
                    }
                    if let Some(op) = ops.get(at) {
                        let class = store
                            .record_live(workspace, domain, op.clone())
                            .await
                            .expect("recorded");
                        classes.push(name(&class));
                    }
                }
                assert_eq!(json!(classes), case["classes"], "{label}: classes");
                for (rid, want) in case["expect"].as_object().expect("live sets") {
                    let live = store
                        .live_at(workspace, Id::from_hex(rid).expect("rid"), u64::MAX)
                        .await
                        .expect("live set");
                    let mut ours: Vec<(Id, u64)> =
                        live.iter().map(|(o, _)| (o.device, o.counter)).collect();
                    ours.sort();
                    let ours: Vec<Value> = ours
                        .iter()
                        .map(|(d, c)| json!({"device": d.to_hex(), "counter": c}))
                        .collect();
                    assert_eq!(&json!(ours), want, "{label}: live set");
                }
            }
            "live_check" => {
                for op in &ops {
                    store
                        .record_live(workspace, domain, op.clone())
                        .await
                        .expect("recorded");
                }
                let ours = match store
                    .check_live(workspace, live_op(&case["next"]))
                    .await
                    .expect("checked")
                {
                    Ok(class) => name(&class),
                    Err(error) => error.server_code().to_owned(),
                };
                assert_eq!(
                    ours,
                    case["expect"].as_str().expect("an outcome"),
                    "{label}"
                );
            }
            "covers_for" => {
                for op in &ops {
                    store
                        .record_live(workspace, domain, op.clone())
                        .await
                        .expect("recorded");
                }
                let live = store
                    .live_at(workspace, ops[0].rid, u64::MAX)
                    .await
                    .expect("live");
                let refs: Vec<&LiveOp> = live.iter().map(|(o, _)| o).collect();
                let chosen: BTreeSet<(Id, u64)> = case["chosen"]
                    .as_array()
                    .expect("chosen")
                    .iter()
                    .map(|c| (id_of(&c["device"]), u64_of(&c["counter"])))
                    .collect();
                let covers = covers_for(&refs, &chosen, id_of(&case["own_device"]));
                assert_eq!(
                    serde_json::to_value(covers).expect("JSON"),
                    case["expect"],
                    "{label}"
                );
            }
            other => panic!("{label}: unknown case kind {other}"),
        }
    }
}

/// Creation starts the workspace domain's log with the genesis policy entry
/// and the epoch entry, and the workspace lists with its profile.
pub async fn creation_starts_the_domain_log<B: Backend>(store: Store<B>) {
    let world = World::new(store, 2).await;
    let page = world.pull(1, 0).await;
    assert_eq!(page.status, PageStatus::Ok);
    assert_eq!(page.head, 2);
    assert_eq!(page.complete_through, 2);
    assert_eq!(page.restore_generation, world.store.restore_generation());
    assert_eq!(page.policy_revision, 1);
    page.check_shape().expect("a well-formed page");
    match &page.entries[0].record {
        EntryRecord::Policy(entry) => {
            assert_eq!(page.entries[0].seq, 1);
            assert_eq!((entry.revision, entry.head), (1, world.genesis.digest()));
        }
        other => panic!("the log starts with the genesis entry, not {other:?}"),
    }
    match &page.entries[1].record {
        EntryRecord::Epoch(epoch) => {
            assert_eq!(page.entries[1].seq, 2);
            assert_eq!(epoch.body.epoch, 1);
        }
        other => panic!("the epoch entry follows, not {other:?}"),
    }
    let listed = world
        .store
        .workspaces(world.devices[1].session)
        .await
        .expect("listed");
    assert_eq!(listed.workspaces.len(), 1);
    let info = &listed.workspaces[0];
    assert_eq!(info.workspace, world.workspace);
    assert_eq!(info.role, sync_protocol::records::Role::Owner);
    let profile = info.profile.as_ref().expect("a profile");
    assert_eq!(profile.body.profile_revision, 1);
    let policy = world
        .store
        .policy(world.devices[1].session, world.workspace, 0)
        .await
        .expect("policy");
    assert_eq!(policy.head_revision, 1);
    assert_eq!(policy.changes.len(), 1);
    assert_eq!(policy.changes[0].0, world.genesis);
    assert_eq!(policy.account_heads.to_vec(), vec![head_of(&world.links)]);
}

/// A retry with identical bytes gets the original result; other bytes
/// under the same workspace ID get `id_conflict`.
pub async fn creation_retries_and_conflicts<B: Backend>(store: Store<B>) {
    let (account, keys, links) = account(1);
    store
        .append_account_links(links.clone())
        .await
        .expect("chain");
    let session = Session {
        account,
        device: keys[0].device(),
        recovery: None,
    };
    let cards = vec![keys[0].card(account)];
    let (body, genesis, _) = creation(&keys[0], &links, &cards);
    let first = store
        .create_workspace(session, body.clone())
        .await
        .expect("created");
    let again = store
        .create_workspace(session, body.clone())
        .await
        .expect("retried");
    assert_eq!(first, again);
    assert_eq!(first.policy_revision, 1);
    assert_eq!(first.receipts.len(), 2);
    assert_eq!(first.receipts[0].record_digest, genesis.digest());
    assert_eq!(
        (first.receipts[0].seq, first.receipts[1].seq),
        (1, 2),
        "the genesis entry, then the epoch entry"
    );
    // Same workspace ID, different bytes.
    let mut records = PushRequest::from_bytes(&body).expect("decodes").records.0;
    let RecordRef::Profile(profile) = records.last().cloned().expect("a profile") else {
        panic!("the profile is last");
    };
    let mut other = profile.body.clone();
    other.nonce = random_nonce().expect("random");
    *records.last_mut().expect("last") = RecordRef::Profile(Signed::sign(other, keys[0].signing()));
    let changed = PushRequest {
        records: List(records),
    }
    .to_bytes();
    assert_eq!(
        refusal(store.create_workspace(session, changed).await).code,
        "id_conflict"
    );
}

/// The store keys domains by workspace and ID: two workspaces may name one
/// domain ID, and each keeps its own log.
pub async fn domain_ids_belong_to_their_workspace<B: Backend>(store: Store<B>) {
    let (account, keys, links) = account(1);
    store
        .append_account_links(links.clone())
        .await
        .expect("chain");
    let session = Session {
        account,
        device: keys[0].device(),
        recovery: None,
    };
    let cards = vec![keys[0].card(account)];
    let shared = Id::random().expect("random");
    let mut workspaces = Vec::new();
    for _ in 0..2 {
        let (body, genesis, _) = creation_with_domain(&keys[0], &links, &cards, shared);
        let response = store
            .create_workspace(session, body)
            .await
            .expect("both workspaces take the domain ID");
        assert_eq!(
            (response.receipts[0].domain, response.receipts[0].seq),
            (shared, 1),
            "each log starts on its own"
        );
        workspaces.push(genesis.body.workspace);
    }
    let listed = store.workspaces(session).await.expect("listed");
    assert_eq!(listed.workspaces.len(), 2);
    assert_ne!(workspaces[0], workspaces[1]);
}

/// Creation checks the genesis heads against the owner's chain, every
/// signature under the signer's card, and one envelope per active card.
pub async fn creation_checks_heads_signatures_and_recipients<B: Backend>(store: Store<B>) {
    let (account, keys, links) = account(2);
    store
        .append_account_links(links.clone())
        .await
        .expect("chain");
    let session = Session {
        account,
        device: keys[0].device(),
        recovery: None,
    };
    let cards: Vec<Card> = keys.iter().map(|k| k.card(account)).collect();

    // A head off the account's chain.
    let mut forked = links[..1].to_vec();
    let stray = DeviceKeys::generate().expect("random");
    forked.push(next_link(
        &keys[0],
        &head_of(&links[..1]),
        LinkKind::AddDevice {
            card: Nested(stray.card(account)),
            offer: Digest::ZERO,
        },
    ));
    let (body, _, _) = creation(&keys[0], &forked, &[cards[0].clone()]);
    assert_eq!(
        push_failure(store.create_workspace(session, body).await),
        ("account_conflict", 0)
    );

    // A missing envelope for the second card.
    let (body, _, _) = creation(&keys[0], &links, &cards[..1]);
    assert_eq!(
        push_failure(store.create_workspace(session, body).await).0,
        "recipients_invalid"
    );

    // A broken signature on the genesis.
    let (body, _, _) = creation(&keys[0], &links, &cards);
    let mut records = PushRequest::from_bytes(&body).expect("decodes").records.0;
    if let RecordRef::Policy(genesis) = &mut records[0] {
        genesis.sig.0[63] ^= 1;
    }
    let broken = PushRequest {
        records: List(records),
    }
    .to_bytes();
    assert_eq!(
        push_failure(store.create_workspace(session, broken).await),
        ("signature_invalid", 0)
    );

    // Another device's session can't sign the genesis.
    let (body, _, _) = creation(&keys[0], &links, &cards);
    let other = Session {
        account,
        device: keys[1].device(),
        recovery: None,
    };
    assert_eq!(
        push_failure(store.create_workspace(other, body).await),
        ("forbidden", 0)
    );
}

/// A push returns one receipt per entry, in request order.
pub async fn push_returns_receipts_in_request_order<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let first = world.op(0, |_| {});
    let second = world.op(0, |op| {
        op.counter = 2;
        op.previous = first.digest();
    });
    let response = world
        .push(0, &[first.clone(), second.clone()])
        .await
        .expect("pushed");
    assert_eq!(response.policy_revision, 1);
    let receipts: Vec<(Digest, Id, u64)> = response
        .receipts
        .iter()
        .map(|r| (r.record_digest, r.domain, r.seq))
        .collect();
    assert_eq!(
        receipts,
        vec![
            (first.digest(), world.domain, 3),
            (second.digest(), world.domain, 4)
        ]
    );
    let page = world.pull(0, 2).await;
    let pulled: Vec<Signed<Operation>> = page
        .entries
        .iter()
        .map(|e| match &e.record {
            EntryRecord::Operation(op) => op.clone(),
            other => panic!("an operation, not {other:?}"),
        })
        .collect();
    assert_eq!(pulled, vec![first, second]);
}

/// A push whose second record fails commits nothing and names that record.
pub async fn push_commits_all_or_none<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let first = world.op(0, |_| {});
    let bad = world.op(0, |op| {
        op.counter = 2;
        op.previous = Digest([7; 32]);
    });
    let result = world.push(0, &[first.clone(), bad]).await;
    let refusal = refusal(result);
    assert_eq!(refusal.code, "chain_conflict");
    let detail =
        PushFailureDetail::from_bytes(refusal.detail.as_deref().expect("detail")).expect("decodes");
    assert_eq!(detail.record_index, 1);
    let head = ChainConflictDetail::from_bytes(&detail.record_detail.expect("the head").0)
        .expect("a chain_conflict detail");
    // The head the second record met: the first record, before the
    // failure rolled both back.
    assert_eq!(
        (head.domain, head.counter, head.record_digest),
        (world.domain, 1, first.digest())
    );
    let page = world.pull(0, 0).await;
    assert_eq!(page.head, 2, "nothing committed");
    world
        .push(0, &[first])
        .await
        .expect("the first record alone commits");
}

/// A retried operation gets its original receipt; another record at the
/// same position gets `chain_conflict` with the server's head.
pub async fn retry_returns_the_original_receipt<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let op = world.op(0, |_| {});
    let first = world
        .push(0, std::slice::from_ref(&op))
        .await
        .expect("pushed");
    let again = world
        .push(0, std::slice::from_ref(&op))
        .await
        .expect("retried");
    assert_eq!(first, again);
    assert_eq!(world.pull(0, 0).await.head, 3, "one copy");
    // A push that lists one operation twice would get two receipts for one
    // log entry, so it gets `bad_request` naming the second copy.
    let next = world.op(0, |o| {
        o.counter = 2;
        o.previous = op.digest();
    });
    assert_eq!(
        push_failure(world.push(0, &[next.clone(), next]).await),
        ("bad_request", 1)
    );
    assert_eq!(world.pull(0, 0).await.head, 3, "nothing committed");
    let other = world.op(0, |o| {
        o.rid = op.body.rid;
        o.ciphertext = Bytes(vec![1; 272]);
    });
    let refusal = refusal(world.push(0, &[other]).await);
    assert_eq!(refusal.code, "chain_conflict");
    let detail =
        PushFailureDetail::from_bytes(refusal.detail.as_deref().expect("detail")).expect("decodes");
    let head =
        ChainConflictDetail::from_bytes(&detail.record_detail.expect("head").0).expect("decodes");
    assert_eq!((head.counter, head.record_digest), (1, op.digest()));
}

/// Each §13 check answers its own code.
pub async fn operation_checks_answer_their_codes<B: Backend>(store: Store<B>) {
    let world = World::new(store, 2).await;
    let base = world.op(0, |_| {});
    world
        .push(0, std::slice::from_ref(&base))
        .await
        .expect("base");
    let patch = world.op(1, |op| op.rid = base.body.rid);
    world
        .push(1, &[patch])
        .await
        .expect("another device's patch");
    let other = world.devices[1].id();
    let next = |edit: &dyn Fn(&mut Operation)| {
        world.op(0, |op| {
            op.counter = 2;
            op.previous = base.digest();
            edit(op);
        })
    };
    let generation = world.store.restore_generation();
    let cases: Vec<(&str, Signed<Operation>)> = vec![
        ("policy_stale", next(&|op| op.policy_revision = 2)),
        ("policy_stale", next(&|op| op.policy_revision = 0)),
        (
            "policy_conflict",
            next(&|op| op.policy_head = Digest([9; 32])),
        ),
        ("epoch_stale", next(&|op| op.epoch = 2)),
        (
            "policy_regressed",
            next(&|op| op.generation = generation + 1),
        ),
        (
            "policy_regressed",
            next(&|op| op.generation = generation - 1),
        ),
        (
            "checkpoint_unknown",
            next(&|op| {
                op.checkpoint_seq = 2;
                op.checkpoint = Digest([5; 32]);
            }),
        ),
        (
            "checkpoint_unknown",
            next(&|op| op.checkpoint = Digest([5; 32])),
        ),
        ("chain_conflict", next(&|op| op.previous = Digest([1; 32]))),
        ("chain_conflict", next(&|op| op.counter = 1)),
        (
            "supersedes_unknown",
            next(&|op| {
                op.supersedes = Sorted::new(vec![Cover {
                    device: op.device,
                    counter: 9,
                }])
                .expect("sorted");
            }),
        ),
        // A state operation that covers the other device's patch but
        // leaves this device's own state operation live.
        (
            "supersedes_incomplete",
            next(&|op| {
                op.rid = base.body.rid;
                op.supersedes = Sorted::new(vec![Cover {
                    device: other,
                    counter: 1,
                }])
                .expect("sorted");
            }),
        ),
        (
            "blob_incomplete",
            next(&|op| {
                op.blobs = Sorted::new(vec![Id([3; 16])]).expect("sorted");
            }),
        ),
        (
            "padding_invalid",
            next(&|op| op.ciphertext = Bytes(vec![0; 300])),
        ),
        ("not_found", next(&|op| op.domain = Id([4; 16]))),
    ];
    for (code, op) in cases {
        let result = world.push(0, &[op]).await;
        assert!(result.is_err(), "{code}: {result:?}");
        assert_eq!(push_failure(result), (code, 0), "{code}");
    }
    // A ciphertext above 270,352 bytes doesn't decode as an operation.
    let big = next(&|op| op.ciphertext = Bytes(vec![0; MAX_OPERATION_CIPHERTEXT + 1]));
    assert_eq!(push_failure(world.push(0, &[big]).await), ("too_large", 0));
    // A signature that fails under the signer's card.
    let mut forged = next(&|_| {});
    forged.sig.0[0] ^= 1;
    assert_eq!(
        push_failure(world.push(0, &[forged]).await),
        ("signature_invalid", 0)
    );
    // Another account's device, signed in but not in the workspace.
    let (stranger, keys, links) = account(1);
    world
        .store
        .append_account_links(links)
        .await
        .expect("chain");
    let session = Session {
        account: stranger,
        device: keys[0].device(),
        recovery: None,
    };
    let op = Signed::sign(
        Operation {
            device: keys[0].device(),
            ..next(&|_| {}).body
        },
        keys[0].signing(),
    );
    let body = PushRequest {
        records: List(vec![RecordRef::Operation(op)]),
    }
    .to_bytes();
    assert_eq!(
        refusal(world.store.push(session, world.workspace, body).await).code,
        "forbidden"
    );
    // Nothing above committed: the head is still the patch.
    assert_eq!(world.pull(0, 0).await.head, 4);
}

/// An operation whose `supersedes` is empty on an existing `rid` counts as
/// a patch, and a cover makes a state operation.
pub async fn empty_supersedes_on_an_existing_rid_is_a_patch<B: Backend>(store: Store<B>) {
    let world = World::new(store, 2).await;
    let base = world.op(0, |_| {});
    world
        .push(0, std::slice::from_ref(&base))
        .await
        .expect("base");
    let patch = world.op(1, |op| op.rid = base.body.rid);
    world
        .push(1, std::slice::from_ref(&patch))
        .await
        .expect("patch");
    let state = world.op(1, |op| {
        op.rid = base.body.rid;
        op.counter = 2;
        op.previous = patch.digest();
        op.supersedes = Sorted::new(vec![
            Cover {
                device: world.devices[0].id(),
                counter: 1,
            },
            Cover {
                device: world.devices[1].id(),
                counter: 1,
            },
        ])
        .expect("sorted");
    });
    world
        .push(1, std::slice::from_ref(&state))
        .await
        .expect("state");
    let classes: Vec<(OpClass, u64)> = world
        .store
        .live_at(world.workspace, base.body.rid, 4)
        .await
        .expect("live")
        .into_iter()
        .map(|(op, class)| (class, op.seq))
        .collect();
    assert_eq!(classes, vec![(OpClass::State, 3), (OpClass::Patch, 4)]);
    let live: Vec<u64> = world
        .store
        .live_at(world.workspace, base.body.rid, u64::MAX)
        .await
        .expect("live")
        .into_iter()
        .map(|(op, _)| op.seq)
        .collect();
    assert_eq!(live, vec![5], "the state operation covers both");
}

/// Live patches past 8 MiB of ciphertext answer `snapshot_required`.
pub async fn snapshot_required_past_the_patch_bytes<B: Backend>(store: Store<B>) {
    let world = World::new(store, 2).await;
    let base = world.op(0, |_| {});
    world
        .push(0, std::slice::from_ref(&base))
        .await
        .expect("base");
    let mut previous = Digest::ZERO;
    let mut counter = 0;
    let mut pushed = 0;
    loop {
        let mut batch = Vec::new();
        for _ in 0..3 {
            counter += 1;
            let op = world.op(1, |op| {
                op.rid = base.body.rid;
                op.counter = counter;
                op.previous = previous;
                op.ciphertext = Bytes(vec![0; MAX_OPERATION_CIPHERTEXT]);
            });
            previous = op.digest();
            batch.push(op);
        }
        match world.push(1, &batch).await {
            Ok(_) => pushed += 3,
            Err(error) => {
                let (code, index) = push_failure(Err::<(), _>(error));
                assert_eq!(code, "snapshot_required");
                assert_eq!(pushed + index as u64, 31, "the 32nd patch passes 8 MiB");
                return;
            }
        }
        assert!(pushed < 60, "snapshot_required never came");
    }
}

/// A 65th live state operation on one `rid` answers `conflict_limit`.
pub async fn conflict_limit_past_64_live_states<B: Backend>(store: Store<B>) {
    let world = World::new(store, 65).await;
    let base = world.op(0, |_| {});
    world
        .push(0, std::slice::from_ref(&base))
        .await
        .expect("base");
    let covers = |device: Id| Sorted::new(vec![Cover { device, counter: 1 }]).expect("sorted");
    // Devices 1 to 63 each write a branch that covers nothing of theirs but
    // names the base, which stays covered once: each is a state operation.
    for device in 1..64 {
        let op = world.op(device, |op| {
            op.rid = base.body.rid;
            op.supersedes = covers(world.devices[0].id());
        });
        world.push(device, &[op]).await.expect("a branch");
    }
    let live = world
        .store
        .live_at(world.workspace, base.body.rid, u64::MAX)
        .await
        .expect("live");
    assert_eq!(live.len(), 63);
    let op = world.op(64, |op| {
        op.rid = base.body.rid;
        op.supersedes = covers(world.devices[0].id());
    });
    world.push(64, &[op]).await.expect("the 64th branch");
    let op = world.op(0, |op| {
        op.rid = base.body.rid;
        op.counter = 2;
        op.previous = base.digest();
        op.supersedes = covers(world.devices[0].id());
    });
    assert_eq!(
        push_failure(world.push(0, &[op]).await),
        ("conflict_limit", 0)
    );
}

/// A recorded head that takes a card out removes the device from the
/// workspace, makes the domain due for rotation, and every other device's
/// operation waits for that rotation.
pub async fn removed_device_and_pending_rotation<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 2).await;
    let before = world.op(1, |_| {});
    world
        .push(1, std::slice::from_ref(&before))
        .await
        .expect("written before the removal");
    let removed = world.devices[1].id();
    let head = world
        .append_link(
            0,
            LinkKind::Remove {
                device: removed,
                keep: Sorted::default(),
            },
        )
        .await;
    // The account chain alone stops the removed card's pushes, before any
    // policy change records the new head.
    let early = world.op(1, |op| {
        op.counter = 2;
        op.previous = before.digest();
    });
    assert_eq!(refusal(world.push(1, &[early]).await).code, "forbidden");
    let change = world.change(0, vec![head], PolicyKind::AccountHeads);
    let domain = world.domain;
    world.inject_policy(change, vec![domain], None).await;

    // The removed device writes nothing and reads every domain as status 4.
    let after = world.op(1, |op| {
        op.counter = 2;
        op.previous = before.digest();
        op.policy_revision = 1;
        op.policy_head = world.genesis.digest();
    });
    assert_eq!(refusal(world.push(1, &[after]).await).code, "forbidden");
    let page = world.pull(1, 0).await;
    assert_eq!(page.status, PageStatus::Forbidden);
    page.check_shape().expect("an empty page");

    // The remaining device waits for the rotation the removal requires.
    let op = world.op(0, |_| {});
    assert_eq!(
        push_failure(world.push(0, &[op]).await),
        ("rotation_pending", 0)
    );
    let old = world.op(0, |op| {
        op.policy_revision = 1;
        op.policy_head = world.genesis.digest();
    });
    assert_eq!(
        push_failure(world.push(0, &[old]).await),
        ("rotation_pending", 0)
    );
    let page = world.pull(0, 3).await;
    assert_eq!(page.entries.len(), 1, "only the removal's policy entry");
    assert!(matches!(&page.entries[0].record, EntryRecord::Policy(e) if e.revision == 2));
}

/// Write access and policy revisions: a device added at revision 2 couldn't
/// write at revision 1, and a chain's revision never decreases.
pub async fn policy_regression_and_stale_revisions<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let keys = DeviceKeys::generate().expect("random");
    let head = world
        .append_link(
            0,
            LinkKind::AddDevice {
                card: Nested(keys.card(world.account)),
                offer: Digest::ZERO,
            },
        )
        .await;
    let session = Session {
        account: world.account,
        device: keys.device(),
        recovery: None,
    };
    let token = random_token();
    world
        .store
        .create_session(token, session)
        .await
        .expect("session");
    world.devices.push(Device {
        keys,
        session,
        token,
    });
    let change = world.change(0, vec![head], PolicyKind::AccountHeads);
    let domain = world.domain;
    world.inject_policy(change, vec![domain], None).await;

    let early = world.op(1, |op| {
        op.policy_revision = 1;
        op.policy_head = world.genesis.digest();
    });
    assert_eq!(
        push_failure(world.push(1, &[early]).await),
        ("policy_stale", 0)
    );
    world
        .push(1, &[world.op(1, |_| {})])
        .await
        .expect("at revision 2");

    let first = world.op(0, |_| {});
    world
        .push(0, std::slice::from_ref(&first))
        .await
        .expect("at revision 2");
    let regressed = world.op(0, |op| {
        op.counter = 2;
        op.previous = first.digest();
        op.policy_revision = 1;
        op.policy_head = world.genesis.digest();
    });
    assert_eq!(
        push_failure(world.push(0, &[regressed]).await),
        ("policy_regressed", 0)
    );
}

/// A `rid` belongs to the domain that first took it; domains are keyed by
/// workspace and ID.
pub async fn rid_belongs_to_one_domain<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let op = world.op(0, |_| {});
    world
        .push(0, std::slice::from_ref(&op))
        .await
        .expect("pushed");
    let restricted = Id::random().expect("random");
    let change = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::CreateDomain {
            domain: restricted,
            marker_rid: Id::random().expect("random"),
            access: manager(world.account),
        },
    );
    let creator = &world.devices[0];
    let epoch = Signed::sign(
        build_epoch(
            &EpochParams {
                workspace: world.workspace,
                domain: restricted,
                epoch: 1,
                previous: Digest::ZERO,
                policy_revision: 2,
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
    let domain = world.domain;
    world
        .inject_policy(change, vec![domain], Some((restricted, epoch)))
        .await;
    let moved = world.op(0, |o| {
        o.domain = restricted;
        o.rid = op.body.rid;
    });
    assert_eq!(
        push_failure(world.push(0, &[moved]).await),
        ("rid_domain_mismatch", 0)
    );
    let fresh = world.op(0, |o| o.domain = restricted);
    let response = world.push(0, &[fresh]).await.expect("a new object there");
    assert_eq!(
        (response.receipts[0].domain, response.receipts[0].seq),
        (restricted, 3),
        "after the domain's policy and epoch entries"
    );
}

/// Pushes from several devices at once all commit, in one order, with no
/// busy error: each domain's sequences are dense and each device's chain
/// rises with them.
pub async fn concurrent_pushes_commit_in_one_order<B: Backend>(store: Store<B>) {
    let world = Arc::new(World::new(store, 6).await);
    let mut tasks = Vec::new();
    for device in 0..6 {
        let world = world.clone();
        tasks.push(tokio::spawn(async move {
            let mut previous = Digest::ZERO;
            for counter in 1..=20u64 {
                let op = world.op(device, |op| {
                    op.counter = counter;
                    op.previous = previous;
                });
                previous = op.digest();
                world.push(device, &[op]).await.expect("no push fails");
            }
        }));
    }
    for task in tasks {
        task.await.expect("the task finishes");
    }
    let mut after = 0;
    let mut seqs = Vec::new();
    let mut counters: BTreeMap<Id, u64> = BTreeMap::new();
    loop {
        let page = world.pull(0, after).await;
        for entry in page.entries.iter() {
            seqs.push(entry.seq);
            if let EntryRecord::Operation(op) = &entry.record {
                let last = counters.entry(op.body.device).or_default();
                assert_eq!(op.body.counter, *last + 1, "each chain rises with the log");
                *last = op.body.counter;
            }
        }
        after = page.complete_through;
        if after == page.head {
            break;
        }
    }
    assert_eq!(seqs, (1..=122).collect::<Vec<u64>>());
}

/// Concurrent pushes where some fail: the failures commit nothing and every
/// other push commits.
pub async fn failed_pushes_leave_the_others_committed<B: Backend>(store: Store<B>) {
    let world = Arc::new(World::new(store, 4).await);
    let mut tasks = Vec::new();
    for device in 0..4 {
        let world = world.clone();
        tasks.push(tokio::spawn(async move {
            let mut previous = Digest::ZERO;
            let mut counter = 0;
            let mut committed = Vec::new();
            for round in 0..15u64 {
                let good = world.op(device, |op| {
                    op.counter = counter + 1;
                    op.previous = previous;
                });
                let ops = if round % 3 == 0 {
                    let bad = world.op(device, |op| {
                        op.counter = counter + 2;
                        op.previous = good.digest();
                        op.epoch = 7;
                    });
                    vec![good.clone(), bad]
                } else {
                    vec![good.clone()]
                };
                match world.push(device, &ops).await {
                    Ok(_) => {
                        counter += 1;
                        previous = good.digest();
                        committed.push(good.digest());
                    }
                    Err(error) => assert_eq!(push_failure(Err::<(), _>(error)), ("epoch_stale", 1)),
                }
            }
            committed
        }));
    }
    let mut expected = BTreeSet::new();
    for task in tasks {
        expected.extend(task.await.expect("the task finishes"));
    }
    let mut found = BTreeSet::new();
    let mut after = 0;
    loop {
        let page = world.pull(0, after).await;
        for entry in page.entries.iter() {
            if let EntryRecord::Operation(op) = &entry.record {
                found.insert(op.digest());
            }
        }
        after = page.complete_through;
        if after == page.head {
            break;
        }
    }
    assert_eq!(found, expected);
    assert_eq!(found.len(), 40, "10 of each device's 15 pushes commit");
}

/// One pull pages every listed domain; a domain the workspace doesn't hold
/// gets status 5 while the others still return.
pub async fn pull_pages_each_domain<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let response = world
        .store
        .pull(
            world.devices[0].session,
            world.workspace,
            PullRequest {
                cursors: List(vec![
                    PullCursor {
                        domain: Id([8; 16]),
                        after: 0,
                    },
                    PullCursor {
                        domain: world.domain,
                        after: 1,
                    },
                ]),
                wait_seconds: 0,
            },
        )
        .await
        .expect("pull");
    let pages = response.pages.0;
    assert_eq!(pages.len(), 2);
    assert_eq!(pages[0].status, PageStatus::NotFound);
    assert!(pages[0].entries.is_empty());
    pages[0].check_shape().expect("an empty page");
    assert_eq!(pages[1].status, PageStatus::Ok);
    assert_eq!(
        pages[1].entries.iter().map(|e| e.seq).collect::<Vec<_>>(),
        vec![2]
    );
    pages[1].check_shape().expect("a page");

    // A cursor above the head, as after a database put back by file copy,
    // keeps its place and reports the lower head, which the client reports
    // as `sync_server_rollback`.
    let page = world.pull(0, 50).await;
    assert_eq!(page.status, PageStatus::Ok);
    assert_eq!((page.complete_through, page.head), (50, 2));
    assert!(page.entries.is_empty());
}

/// A pull returns at most 1 MiB and its `complete_through` says where to
/// continue.
pub async fn pull_stays_within_one_mebibyte<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let mut previous = Digest::ZERO;
    let mut counter = 0;
    for _ in 0..4 {
        let mut batch = Vec::new();
        for _ in 0..3 {
            counter += 1;
            let op = world.op(0, |op| {
                op.counter = counter;
                op.previous = previous;
                op.ciphertext = Bytes(vec![0; MAX_OPERATION_CIPHERTEXT]);
            });
            previous = op.digest();
            batch.push(op);
        }
        world.push(0, &batch).await.expect("pushed");
    }
    let mut after = 0;
    let mut pages = 0;
    let mut ops = 0;
    loop {
        let page = world.pull(0, after).await;
        let response = sync_protocol::messages::PullResponse {
            pages: List(vec![page.clone()]),
        };
        assert!(
            response.to_bytes().len() <= MAX_BODY,
            "a pull stays within 1 MiB"
        );
        page.check_shape().expect("a page");
        ops += page
            .entries
            .iter()
            .filter(|e| matches!(e.record, EntryRecord::Operation(_)))
            .count();
        pages += 1;
        after = page.complete_through;
        if after == page.head {
            break;
        }
    }
    assert_eq!(ops, 12);
    assert!(pages >= 4, "12 operations of 270 KB take at least 4 pulls");

    // A pull lists at most one cursor per domain a workspace can hold, so
    // 256 pages fit 1 MiB with the entries, and more cursors get
    // `too_large`.
    let request = |count: usize| PullRequest {
        cursors: List(
            (0..count)
                .map(|i| PullCursor {
                    domain: if i == 0 {
                        world.domain
                    } else {
                        Id::random().expect("random")
                    },
                    after: 0,
                })
                .collect(),
        ),
        wait_seconds: 0,
    };
    let session = world.devices[0].session;
    let response = world
        .store
        .pull(session, world.workspace, request(256))
        .await
        .expect("256 cursors");
    assert_eq!(response.pages.len(), 256);
    assert!(!response.pages[0].entries.is_empty());
    assert!(
        response.to_bytes().len() <= MAX_BODY,
        "256 pages stay within 1 MiB"
    );
    assert_eq!(
        refusal(
            world
                .store
                .pull(session, world.workspace, request(257))
                .await
        )
        .code,
        "too_large"
    );
}

/// One domain of small operations can't crowd out the other pages: the
/// pull sets aside every page's fixed fields before it fills any page, so
/// the response stays within 1 MiB however many cursors it lists.
pub async fn pull_keeps_room_for_every_page<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let mut previous = Digest::ZERO;
    let mut counter = 0;
    for _ in 0..4 {
        let mut batch = Vec::new();
        for _ in 0..500 {
            counter += 1;
            let op = world.op(0, |op| {
                op.counter = counter;
                op.previous = previous;
            });
            previous = op.digest();
            batch.push(op);
        }
        world.push(0, &batch).await.expect("pushed");
    }
    let session = world.devices[0].session;
    for count in [1, 2, 8, 256] {
        let mut cursors = vec![PullCursor {
            domain: world.domain,
            after: 0,
        }];
        cursors.extend((1..count).map(|_| PullCursor {
            domain: Id::random().expect("random"),
            after: 0,
        }));
        let response = world
            .store
            .pull(
                session,
                world.workspace,
                PullRequest {
                    cursors: List(cursors),
                    wait_seconds: 0,
                },
            )
            .await
            .expect("pull");
        assert_eq!(response.pages.len(), count);
        let first = &response.pages[0];
        first.check_shape().expect("a page");
        assert!(
            first.complete_through < first.head,
            "2,000 operations fill more than one pull"
        );
        let size = response.to_bytes().len();
        assert!(
            size <= MAX_BODY,
            "{count} cursors: {size} bytes, {} over 1 MiB",
            size.saturating_sub(MAX_BODY)
        );
    }
}

/// A pull that waits wakes when another device's push commits.
pub async fn pull_wakes_when_another_device_pushes<B: Backend>(store: Store<B>) {
    let world = Arc::new(World::new(store, 2).await);
    let waiting = {
        let world = world.clone();
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let response = world
                .store
                .pull(
                    world.devices[1].session,
                    world.workspace,
                    PullRequest {
                        cursors: List(vec![PullCursor {
                            domain: world.domain,
                            after: 2,
                        }]),
                        wait_seconds: 20,
                    },
                )
                .await
                .expect("pull");
            (started.elapsed(), response)
        })
    };
    tokio::time::sleep(Duration::from_millis(300)).await;
    let op = world.op(0, |_| {});
    world
        .push(0, std::slice::from_ref(&op))
        .await
        .expect("pushed");
    let (elapsed, response) = waiting.await.expect("the pull returns");
    assert!(elapsed < Duration::from_secs(10), "woke after {elapsed:?}");
    let page = &response.pages[0];
    assert_eq!(page.entries.len(), 1);
    assert!(matches!(&page.entries[0].record, EntryRecord::Operation(o) if *o == op));
}

/// `chain/{device}/{counter}` returns the record at that counter or the
/// next above it, and `heads/{device}` answers `not_found` for a device
/// with no record.
pub async fn chain_positions_and_heads<B: Backend>(store: Store<B>) {
    let world = World::new(store, 2).await;
    let first = world.op(0, |_| {});
    // The device skipped counters 2 to 1,024 after a restart.
    let next = world.op(0, |op| {
        op.counter = 1_025;
        op.previous = first.digest();
    });
    world
        .push(0, &[first.clone(), next.clone()])
        .await
        .expect("pushed");
    let session = world.devices[1].session;
    let device = world.devices[0].id();
    let record = |entry: sync_protocol::messages::Entry| match entry.record {
        EntryRecord::Operation(op) => (entry.seq, op),
        other => panic!("an operation, not {other:?}"),
    };
    let at = world
        .store
        .chain_record(session, world.workspace, world.domain, device, 1)
        .await;
    assert_eq!(record(at.expect("found")), (3, first));
    let skipped = world
        .store
        .chain_record(session, world.workspace, world.domain, device, 2)
        .await;
    assert_eq!(record(skipped.expect("found")), (4, next.clone()));
    let past = world
        .store
        .chain_record(session, world.workspace, world.domain, device, 1_026)
        .await;
    assert_eq!(refusal(past).code, "not_found");
    let head = world
        .store
        .chain_head(session, world.workspace, world.domain, device)
        .await
        .expect("a head");
    assert_eq!(
        (head.counter, head.record_digest, head.seq),
        (1_025, next.digest(), 4)
    );
    let none = world
        .store
        .chain_head(
            session,
            world.workspace,
            world.domain,
            world.devices[1].id(),
        )
        .await;
    assert_eq!(refusal(none).code, "not_found");
}

/// `keys` serves epoch records without recipients and the caller's own
/// envelopes; `epochs/{epoch}` walks `previous` links.
pub async fn keys_and_epochs_serve_records_without_recipients<B: Backend>(store: Store<B>) {
    let world = World::new(store, 3).await;
    let device = &world.devices[2];
    let keys = world
        .store
        .keys(device.session, world.workspace, 0, device.id())
        .await
        .expect("keys");
    assert_eq!(keys.head_key_seq, 4, "the epoch record and three envelopes");
    let records: Vec<&KeyRecord> = keys.entries.iter().map(|e| &e.record).collect();
    assert_eq!(
        records.len(),
        2,
        "the epoch record and this device's envelope"
    );
    let KeyRecord::Epoch(epoch) = records[0] else {
        panic!("the epoch record first");
    };
    let KeyRecord::Envelope(envelope) = records[1] else {
        panic!("then the envelope");
    };
    assert_eq!(envelope.body.recipient, device.id());
    let key = sync_key_envelope::v2::unwrap(
        &envelope.body,
        &device.keys,
        &device.keys.card(world.account),
    )
    .expect("unwraps");
    assert_eq!(key, world.epoch_key);
    assert!(sync_protocol::keys::key_matches(&epoch.body, &key));
    let other = world
        .store
        .keys(device.session, world.workspace, 0, world.devices[0].id())
        .await;
    assert_eq!(refusal(other).code, "forbidden");
    let epochs = world
        .store
        .epochs(device.session, world.workspace, world.domain, 1)
        .await
        .expect("epochs");
    assert_eq!(epochs.epochs.len(), 1);
    assert_eq!(&epochs.epochs[0].0, epoch);
    let missing = world
        .store
        .epochs(device.session, world.workspace, world.domain, 2)
        .await;
    assert_eq!(refusal(missing).code, "not_found");

    // Each rotation's record names the one before; the route walks those
    // links back from the epoch asked for, newest first.
    let readers: BTreeSet<Id> = world.devices.iter().map(Device::id).collect();
    let mut chain = vec![epoch.clone()];
    for number in 2..=3 {
        let next = Signed::sign(
            build_epoch(
                &EpochParams {
                    workspace: world.workspace,
                    domain: world.domain,
                    epoch: number,
                    previous: chain.last().expect("an epoch").digest(),
                    policy_revision: world.head.0,
                    policy_head: world.head.1,
                    rotating_device: world.devices[0].id(),
                    recipients: readers.clone(),
                },
                &SecretKey::random().expect("random"),
                None,
                Nonce::ZERO,
            ),
            world.devices[0].keys.signing(),
        );
        world.inject_epoch(next.clone(), readers.clone()).await;
        chain.push(next);
    }
    for (asked, expected) in [(3, vec![3, 2, 1]), (2, vec![2, 1]), (1, vec![1])] {
        let epochs = world
            .store
            .epochs(device.session, world.workspace, world.domain, asked)
            .await
            .expect("epochs");
        let numbers: Vec<u64> = epochs.epochs.iter().map(|e| e.0.body.epoch).collect();
        assert_eq!(numbers, expected, "epochs/{asked}");
        for walked in epochs.epochs.iter() {
            assert_eq!(walked.0, chain[walked.0.body.epoch as usize - 1]);
        }
    }
}

/// A write transaction on `store` that takes `domain`'s pruning lock, says
/// so, and keeps the lock until `release` fires or ten seconds pass. It
/// returns whether `release` fired.
fn hold_pruning<B: Backend>(
    store: Arc<Store<B>>,
    workspace: Id,
    domain: Id,
) -> (
    tokio::sync::oneshot::Receiver<()>,
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<bool>,
) {
    let (locked, on_locked) = tokio::sync::oneshot::channel();
    let (release, on_release) = tokio::sync::oneshot::channel::<()>();
    let task = tokio::spawn(async move {
        store
            .backend()
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_pruning(workspace, domain).await?;
                    let _ = locked.send(());
                    Ok(tokio::time::timeout(Duration::from_secs(10), on_release)
                        .await
                        .is_ok())
                })
            }))
            .await
            .expect("the pruning transaction commits")
    });
    (on_locked, release, task)
}

/// A write transaction on `store` that takes `domain`'s pruning lock and
/// commits.
async fn prune_lock_once<B: Backend>(store: &Store<B>, workspace: Id, domain: Id) {
    store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move { txn.lock_pruning(workspace, domain).await })
        }))
        .await
        .expect("the pruning lock is taken");
}

/// A write transaction on `store` that takes the workspace's policy lock
/// exclusively, as a policy change does, and commits.
async fn policy_lock_once<B: Backend>(store: &Store<B>, workspace: Id) {
    store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move {
                txn.lock(&LockPlan {
                    workspace: Some(workspace),
                    policy_exclusive: true,
                    ..LockPlan::default()
                })
                .await
            })
        }))
        .await
        .expect("the policy lock is taken");
}

/// The pruning lock and the policy lock use separate lock forms, so a push
/// or a policy change never waits on a domain's pruning; two domains whose
/// 4-byte prefixes collide only delay each other's pruning, and other
/// domains prune meanwhile (§15). A store with one writer runs them one
/// after another instead.
pub async fn pruning_lock_meets_no_policy_lock<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let parallel = world.store.backend().parallel_writes();
    let (workspace, domain) = (world.workspace, world.domain);
    let mut collides = Id::random().expect("random");
    collides.0[..4].copy_from_slice(&domain.0[..4]);
    let mut apart = Id::random().expect("random");
    apart.0[0] = domain.0[0] ^ 0xff;
    let soon = Duration::from_secs(3);

    // A push and a policy change while the domain prunes.
    let (locked, release, holder) = hold_pruning(world.store.clone(), workspace, domain);
    locked.await.expect("the pruning lock is held");
    if !parallel {
        let _ = release.send(());
        assert!(holder.await.expect("the holder ends"));
        world.push(0, &[world.op(0, |_| {})]).await.expect("pushed");
        policy_lock_once(&world.store, workspace).await;
    } else {
        let op = world.op(0, |_| {});
        tokio::time::timeout(soon, world.push(0, std::slice::from_ref(&op)))
            .await
            .expect("a push doesn't wait on the pruning lock")
            .expect("pushed");
        tokio::time::timeout(soon, policy_lock_once(&world.store, workspace))
            .await
            .expect("a policy change doesn't wait on the pruning lock");
        let _ = release.send(());
        assert!(
            holder.await.expect("the holder ends"),
            "the pruning lock stayed held throughout"
        );
    }

    // Two domains whose prefixes collide, and one whose prefix differs.
    let (locked, release, holder) = hold_pruning(world.store.clone(), workspace, domain);
    locked.await.expect("the pruning lock is held");
    let colliding = {
        let store = world.store.clone();
        tokio::spawn(async move { prune_lock_once(&store, workspace, collides).await })
    };
    if parallel {
        tokio::time::timeout(soon, prune_lock_once(&world.store, workspace, apart))
            .await
            .expect("another prefix prunes meanwhile");
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(
            !colliding.is_finished(),
            "a colliding prefix waits for the pruning lock"
        );
    }
    let _ = release.send(());
    assert!(holder.await.expect("the holder ends"));
    tokio::time::timeout(soon, colliding)
        .await
        .expect("the colliding domain prunes once the lock goes")
        .expect("the task ends");
    if !parallel {
        prune_lock_once(&world.store, workspace, apart).await;
    }
}

/// Every entry of the workspace domain, read through `store`.
async fn domain_log<B: Backend>(
    store: &Store<B>,
    session: Session,
    workspace: Id,
    domain: Id,
) -> Vec<sync_protocol::messages::Entry> {
    let mut entries = Vec::new();
    let mut after = 0;
    loop {
        let response = store
            .pull(
                session,
                workspace,
                PullRequest {
                    cursors: List(vec![PullCursor { domain, after }]),
                    wait_seconds: 0,
                },
            )
            .await
            .expect("pull");
        let page = response.pages.0.into_iter().next().expect("one page");
        entries.extend(page.entries.into_vec());
        after = page.complete_through;
        if after >= page.head {
            return entries;
        }
    }
}

/// Creations that one account sends at once never take it past the 100
/// workspaces an account owns (§27): each counts the account's workspaces
/// under its chain row, so exactly the ones that fit commit and the others
/// get `workspace_limit`.
pub async fn creations_at_once_keep_the_workspace_limit<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let creator = &world.devices[0];
    let cards = vec![creator.keys.card(world.account)];
    let body = || creation(&creator.keys, &world.links, &cards).0;
    for _ in 1..96 {
        world
            .store
            .create_workspace(creator.session, body())
            .await
            .expect("created");
    }
    let bodies: Vec<Vec<u8>> = (0..10).map(|_| body()).collect();
    let creations: Vec<_> = bodies
        .into_iter()
        .map(|body| {
            let store = world.store.clone();
            let session = creator.session;
            tokio::spawn(async move { store.create_workspace(session, body).await })
        })
        .collect();
    let mut created = 0;
    let mut refused = Vec::new();
    for creation in creations {
        match creation.await.expect("the creation ends") {
            Ok(_) => created += 1,
            Err(error) => refused.push(refusal(Err::<(), _>(error)).code),
        }
    }
    assert_eq!(created, 4, "the four creations that fit commit");
    assert!(
        refused.iter().all(|code| *code == "workspace_limit"),
        "{refused:?}"
    );
    let listed = world
        .store
        .workspaces(creator.session)
        .await
        .expect("listed");
    assert_eq!(listed.workspaces.len(), 100);
}

/// A change that takes a device out of the workspace races that device's
/// pushes, one every 500 ms through the second instance: no record of the
/// device commits after the change's entry, and every push the change
/// overtakes is refused (§15).
pub async fn removal_races_pushes<B: Backend>(first: Arc<Store<B>>, second: Arc<Store<B>>) {
    let world = Arc::new(World::on(first, 2).await);
    let removed = world.devices[1].id();
    let (started, mut on_started) = tokio::sync::mpsc::unbounded_channel::<u64>();
    let pushing = {
        let world = world.clone();
        tokio::spawn(async move {
            let mut previous = Digest::ZERO;
            let mut committed = Vec::new();
            let mut refused = Vec::new();
            for counter in 1..=8u64 {
                let tick = tokio::time::Instant::now() + Duration::from_millis(500);
                let op = world.op(1, |op| {
                    op.counter = counter;
                    op.previous = previous;
                });
                let body = PushRequest {
                    records: List(vec![RecordRef::Operation(op.clone())]),
                }
                .to_bytes();
                let _ = started.send(counter);
                match second
                    .push(world.devices[1].session, world.workspace, body)
                    .await
                {
                    Ok(response) => {
                        committed.push(response.receipts[0].seq);
                        previous = op.digest();
                    }
                    Err(error) => refused.push(refusal(Err::<(), _>(error)).code),
                }
                tokio::time::sleep_until(tick).await;
            }
            (committed, refused)
        })
    };
    // The removal starts while the device's third push is in flight.
    while on_started.recv().await.is_some_and(|counter| counter < 3) {}
    tokio::time::sleep(Duration::from_millis(20)).await;
    let link = next_link(
        &world.devices[0].keys,
        &head_of(&world.links),
        LinkKind::Remove {
            device: removed,
            keep: Sorted::default(),
        },
    );
    world
        .store
        .append_account_links(vec![link.clone()])
        .await
        .expect("the link is appended");
    let mut links = world.links.clone();
    links.push(link);
    let change = world.change(0, vec![head_of(&links)], PolicyKind::AccountHeads);
    inject_change(&world.store, change, vec![world.domain], None)
        .await
        .expect("the removal commits");
    let (committed, refused) = pushing.await.expect("the pushes end");

    let log = domain_log(
        &world.store,
        world.devices[0].session,
        world.workspace,
        world.domain,
    )
    .await;
    let entry = log
        .iter()
        .find(|e| matches!(&e.record, EntryRecord::Policy(p) if p.revision == 2))
        .expect("the removal's entry")
        .seq;
    let late: Vec<u64> = log
        .iter()
        .filter(|e| matches!(&e.record, EntryRecord::Operation(op) if op.body.device == removed))
        .map(|e| e.seq)
        .filter(|seq| *seq > entry)
        .collect();
    assert!(
        late.is_empty(),
        "records after the removal's entry: {late:?}"
    );
    assert!(committed.iter().all(|seq| *seq < entry));
    assert!(committed.len() >= 2, "pushes before the removal commit");
    assert!(!refused.is_empty(), "pushes after the removal fail");
    assert!(
        refused.iter().all(|code| *code == "forbidden"),
        "{refused:?}"
    );
}

/// Devices push records into several domains at once, listing the domains
/// in different orders, through both instances while policy changes keep
/// creating domains: every push and every change commits, none deadlocks.
pub async fn pushes_into_several_domains_race_policy_changes<B: Backend>(
    first: Arc<Store<B>>,
    second: Arc<Store<B>>,
) {
    let mut world = World::on(first, 3).await;
    let mut domains = vec![world.domain];
    let epoch_for = |world: &World<B>, domain: Id, change: &Signed<PolicyChange>| {
        let creator = &world.devices[0];
        Signed::sign(
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
        )
    };
    let create = |world: &World<B>, domain: Id| {
        world.change(
            0,
            vec![head_of(&world.links)],
            PolicyKind::CreateDomain {
                domain,
                marker_rid: Id::random().expect("random"),
                access: manager(world.account),
            },
        )
    };
    for _ in 0..2 {
        let domain = Id::random().expect("random");
        let change = create(&world, domain);
        let epoch = epoch_for(&world, domain, &change);
        world
            .inject_policy(change, domains.clone(), Some((domain, epoch)))
            .await;
        domains.push(domain);
    }
    let written = domains.clone();
    let world = Arc::new(world);
    let head = Arc::new(std::sync::Mutex::new(world.head));
    let stores = [world.store.clone(), second];

    let mut pushers = Vec::new();
    for device in 0..3 {
        let world = world.clone();
        let head = head.clone();
        let store = stores[device % 2].clone();
        let mut order = written.clone();
        order.rotate_left(device);
        if device == 1 {
            order.reverse();
        }
        pushers.push(tokio::spawn(async move {
            let mut chains: BTreeMap<Id, (u64, Digest)> = BTreeMap::new();
            let mut digests = Vec::new();
            for _ in 0..12 {
                let (revision, policy_head) = *head.lock().expect("the head");
                let ops: Vec<Signed<Operation>> = order
                    .iter()
                    .map(|domain| {
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
                        op
                    })
                    .collect();
                let body = PushRequest {
                    records: List(ops.iter().cloned().map(RecordRef::Operation).collect()),
                }
                .to_bytes();
                store
                    .push(world.devices[device].session, world.workspace, body)
                    .await
                    .expect("no push fails or deadlocks");
                digests.extend(ops.iter().map(Signed::digest));
            }
            digests
        }));
    }
    let changer = {
        let world = world.clone();
        let head = head.clone();
        let stores = stores.clone();
        tokio::spawn(async move {
            let mut domains = domains;
            for round in 0..6 {
                let (revision, digest) = *head.lock().expect("the head");
                let domain = Id::random().expect("random");
                let change = Signed::sign(
                    PolicyChange {
                        revision: revision + 1,
                        previous: digest,
                        ..create(&world, domain).body
                    },
                    world.devices[0].keys.signing(),
                );
                let epoch = epoch_for(&world, domain, &change);
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
    let pushed = tokio::time::timeout(Duration::from_secs(120), async {
        let mut pushed = BTreeSet::new();
        for pusher in pushers {
            pushed.extend(pusher.await.expect("the pushes end"));
        }
        changer.await.expect("the changes end");
        pushed
    })
    .await
    .expect("nothing deadlocks");
    let mut found = BTreeSet::new();
    for domain in &written {
        for entry in domain_log(
            &world.store,
            world.devices[0].session,
            world.workspace,
            *domain,
        )
        .await
        {
            if let EntryRecord::Operation(op) = entry.record {
                found.insert(op.digest());
            }
        }
    }
    assert_eq!(found, pushed, "every pushed record is in its domain's log");
    assert_eq!(pushed.len(), 3 * 12 * written.len());
}

/// The limiter of each mode: an allowance per key and window that both
/// instances count against together, a cost above the allowance that never
/// fits, a refusal that says when the window ends, and a new window that
/// starts afresh.
pub async fn limits_hold_across_instances<B: Backend>(first: Arc<Store<B>>, second: Arc<Store<B>>) {
    use crate::limiter::{Limit, Taken, now_ms};
    let (one, two) = (first.backend().limiter(), second.backend().limiter());
    let key = Id::random().expect("random").0;
    let hour = Limit {
        allowance: 10,
        window: Duration::from_secs(3_600),
    };
    assert_eq!(
        one.take(&key, 4, hour).await.expect("taken"),
        Taken::Allowed
    );
    assert_eq!(
        two.take(&key, 6, hour).await.expect("taken"),
        Taken::Allowed
    );
    for limiter in [&one, &two] {
        match limiter.take(&key, 1, hour).await.expect("taken") {
            Taken::Refused { retry_after } => {
                assert!(retry_after > Duration::ZERO && retry_after <= hour.window);
            }
            Taken::Allowed => panic!("the allowance is used up on both instances"),
        }
    }
    let other = Id::random().expect("random").0;
    assert_eq!(
        one.take(&other, 10, hour).await.expect("taken"),
        Taken::Allowed
    );
    assert!(matches!(
        two.take(&Id::random().expect("random").0, 11, hour)
            .await
            .expect("taken"),
        Taken::Refused { .. }
    ));

    // Takes from both instances at once never pass the allowance together.
    let shared = Id::random().expect("random").0;
    let mut tasks = Vec::new();
    for i in 0..40 {
        let limiter = if i % 2 == 0 { one.clone() } else { two.clone() };
        tasks.push(tokio::spawn(async move {
            limiter
                .take(
                    &shared,
                    1,
                    Limit {
                        allowance: 25,
                        ..hour
                    },
                )
                .await
        }));
    }
    let mut allowed = 0;
    for task in tasks {
        if task.await.expect("the take ends").expect("taken") == Taken::Allowed {
            allowed += 1;
        }
    }
    assert_eq!(allowed, 25);

    // Limits with different windows on one key count apart, even when both
    // windows end at once, as a second's and two seconds' do every other
    // second.
    let short = Limit {
        allowance: 2,
        window: Duration::from_secs(1),
    };
    let long = Limit {
        allowance: 3,
        window: Duration::from_secs(2),
    };
    while !(1_000..1_400).contains(&(now_ms() % 2_000)) {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(short.window_ends(now_ms()), long.window_ends(now_ms()));
    let key = Id::random().expect("random").0;
    assert_eq!(
        one.take(&key, 2, short).await.expect("taken"),
        Taken::Allowed
    );
    assert_eq!(
        two.take(&key, 3, long).await.expect("taken"),
        Taken::Allowed,
        "the longer limit doesn't count the shorter one's takes"
    );
    assert!(matches!(
        one.take(&key, 1, long).await.expect("taken"),
        Taken::Refused { .. }
    ));
    assert!(matches!(
        two.take(&key, 1, short).await.expect("taken"),
        Taken::Refused { .. }
    ));

    // A new window starts afresh. Start early in a window, so the filling
    // takes all land in one.
    let second_window = Limit {
        allowance: 2,
        window: Duration::from_secs(1),
    };
    while now_ms() % 1_000 > 300 {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let key = Id::random().expect("random").0;
    assert_eq!(
        one.take(&key, 2, second_window).await.expect("taken"),
        Taken::Allowed
    );
    let Taken::Refused { retry_after } = two.take(&key, 1, second_window).await.expect("taken")
    else {
        panic!("the window's allowance is used up");
    };
    assert!(retry_after <= second_window.window);
    tokio::time::sleep(retry_after + Duration::from_millis(50)).await;
    assert_eq!(
        two.take(&key, 2, second_window).await.expect("taken"),
        Taken::Allowed
    );

    // Pruning deletes the counts of ended windows, whichever instance
    // prunes.
    tokio::time::sleep(second_window.window + Duration::from_millis(50)).await;
    assert!(one.prune().await.expect("pruned") >= 1);
    assert_eq!(
        two.prune().await.expect("pruned"),
        0,
        "nothing ended is left"
    );
}

fn head_link(links: &[Signed<AccountLink>]) -> AccountHead {
    head_of(links)
}

fn add_device(
    signer: &DeviceKeys,
    links: &[Signed<AccountLink>],
    new: &Card,
) -> Signed<AccountLink> {
    next_link(
        signer,
        &head_link(links),
        LinkKind::AddDevice {
            card: Nested(new.clone()),
            offer: Digest([9; 32]),
        },
    )
}

/// `POST /v2/accounts/chain` (§7): a link comes from a session of its
/// signer and adds only a card a session of the account enrolled; a link
/// after a stale head gets `account_conflict`; and links commit all or
/// none.
pub async fn account_chain_appends_check_who_sends_them<B: Backend>(store: Store<B>) {
    let account = Id::random().expect("random");
    let laptop = DeviceKeys::generate().expect("random");
    let phone = DeviceKeys::generate().expect("random");
    let laptop_session = Session::new(account, laptop.device());
    let phone_session = Session::new(account, phone.device());
    let genesis = genesis_link(&laptop, account);
    assert_eq!(
        refusal(
            store
                .append_chain(laptop_session, vec![genesis.clone()])
                .await
        )
        .code,
        "forbidden",
        "the genesis card isn't enrolled"
    );
    store
        .enroll_card(laptop.card(account))
        .await
        .expect("enrolled");
    assert_eq!(
        refusal(
            store
                .append_chain(phone_session, vec![genesis.clone()])
                .await
        )
        .code,
        "forbidden",
        "another device's session"
    );
    let head = store
        .append_chain(laptop_session, vec![genesis.clone()])
        .await
        .expect("the genesis");
    assert_eq!(head.seqno, 1);
    let mut links = vec![genesis];
    let add = add_device(&laptop, &links, &phone.card(account));
    assert_eq!(
        refusal(store.append_chain(laptop_session, vec![add.clone()]).await).code,
        "forbidden",
        "the phone's card isn't enrolled"
    );
    store
        .enroll_card(phone.card(account))
        .await
        .expect("enrolled");
    // All or none: the add and a stale link after it store neither.
    let stale = add_device(&laptop, &links, &phone.card(account));
    assert_eq!(
        refusal(
            store
                .append_chain(laptop_session, vec![add.clone(), stale])
                .await
        )
        .code,
        "account_conflict"
    );
    assert_eq!(
        store
            .append_chain(laptop_session, vec![add.clone()])
            .await
            .expect("the add")
            .seqno,
        2
    );
    links.push(add);
    let again = genesis_link(&phone, account);
    assert_eq!(
        refusal(store.append_chain(phone_session, vec![again]).await).code,
        "forbidden",
        "the account already has a chain"
    );
    let chains = store
        .read_chains(
            phone_session,
            sync_protocol::messages::ChainsRequest {
                cursors: List(vec![sync_protocol::messages::ChainCursor {
                    account,
                    genesis: None,
                    after: 0,
                }]),
            },
        )
        .await
        .expect("its own chain");
    let served: Vec<_> = chains.pages.0[0]
        .links
        .0
        .iter()
        .map(|l| l.0.clone())
        .collect();
    assert_eq!(served, links);
}

/// Enrolled cards (§7): the same card binds again byte for byte, and a
/// device ID that a link of its account removed never enrolls again, with
/// any bytes.
pub async fn enrolled_cards_bind_again_but_never_a_removed_id<B: Backend>(store: Store<B>) {
    let (account, keys, links) = account(2);
    store
        .append_account_links(links.clone())
        .await
        .expect("chain");
    store
        .enroll_card(keys[1].card(account))
        .await
        .expect("enrolled");
    store
        .enroll_card(keys[1].card(account))
        .await
        .expect("the same bytes again");
    let other = Id::random().expect("random");
    assert_eq!(
        refusal(store.enroll_card(keys[1].card(other)).await).code,
        "id_conflict"
    );
    let remove = next_link(
        &keys[0],
        &head_of(&links),
        LinkKind::Remove {
            device: keys[1].device(),
            keep: Sorted::default(),
        },
    );
    store
        .append_account_links(vec![remove])
        .await
        .expect("removed");
    assert_eq!(
        refusal(store.enroll_card(keys[1].card(account)).await).code,
        "id_conflict"
    );
}

/// Sign-in rows (§7, §29): a magic link, a passkey that waits for its
/// email, a ceremony and an enrollment challenge each work once, expire 10
/// minutes after the store issued them, and go when the store deletes
/// expired rows; a web session lasts until its end, with no expiry.
pub async fn sign_in_rows_work_once_and_expire<B: Backend>(store: Store<B>) {
    let store = store.with_secret(SecretKey([0x5e; 32]));
    let email = "ada@example.com";
    let link = store.issue_magic_link(email).await.expect("a link");
    let (account, session) = store
        .sign_in_with_magic_link(&link)
        .await
        .expect("used")
        .expect("a current link");
    assert_eq!(account, store.account_id(email).expect("an ID"));
    assert_eq!(
        store.account_email(account).await.expect("read"),
        Some(email.into())
    );
    assert_eq!(
        store.sign_in_with_magic_link(&link).await.expect("used"),
        None
    );
    assert_eq!(
        store.web_session(&session).await.expect("read"),
        Some(account)
    );
    store.clock().advance(Duration::from_secs(365 * 24 * 3600));
    assert_eq!(
        store.web_session(&session).await.expect("read"),
        Some(account)
    );
    store.end_web_session(&session).await.expect("ended");
    assert_eq!(store.web_session(&session).await.expect("read"), None);

    let late = store.issue_magic_link(email).await.expect("a link");
    store.clock().advance(Duration::from_secs(600));
    assert_eq!(
        store.sign_in_with_magic_link(&late).await.expect("used"),
        None
    );

    // A held passkey confirms only for an email without an account.
    let existing = store
        .hold_passkey(email, vec![9], b"{}".to_vec())
        .await
        .expect("held");
    assert_eq!(
        store.confirm_passkey(&existing).await.expect("refused"),
        None
    );
    assert!(store.passkeys(account).await.expect("read").is_empty());
    let carol = "carol@example.com";
    let held = store
        .hold_passkey(carol, vec![1, 2, 3], b"{}".to_vec())
        .await
        .expect("held");
    let (confirmed, _) = store
        .confirm_passkey(&held)
        .await
        .expect("confirmed")
        .expect("a current confirmation");
    assert_eq!(confirmed, store.account_id(carol).expect("an ID"));
    assert_eq!(store.passkeys(confirmed).await.expect("read").len(), 1);
    assert_eq!(store.confirm_passkey(&held).await.expect("used"), None);
    // A credential ID that another passkey holds doesn't confirm, and
    // leaves no account behind.
    let bob = "bob@example.com";
    let taken = store
        .hold_passkey(bob, vec![1, 2, 3], b"{}".to_vec())
        .await
        .expect("held");
    assert_eq!(store.confirm_passkey(&taken).await.expect("refused"), None);
    let bob_account = store.account_id(bob).expect("an ID");
    assert_eq!(store.account_email(bob_account).await.expect("read"), None);

    let ceremony = store
        .begin_ceremony(
            crate::CeremonyKind::SignIn,
            Some(account),
            None,
            b"state".to_vec(),
        )
        .await
        .expect("begun");
    assert!(
        store
            .finish_ceremony(ceremony, crate::CeremonyKind::SignUp)
            .await
            .expect("read")
            .is_none(),
        "another kind takes the row and finds nothing"
    );
    let ceremony = store
        .begin_ceremony(
            crate::CeremonyKind::SignIn,
            Some(account),
            None,
            b"state".to_vec(),
        )
        .await
        .expect("begun");
    let row = store
        .finish_ceremony(ceremony, crate::CeremonyKind::SignIn)
        .await
        .expect("read")
        .expect("a current ceremony");
    assert_eq!(row.state, b"state");
    assert!(
        store
            .finish_ceremony(ceremony, crate::CeremonyKind::SignIn)
            .await
            .expect("read")
            .is_none()
    );

    // Every kind of row goes 10 minutes after its issue.
    store.issue_magic_link(email).await.expect("a link");
    store
        .hold_passkey(email, vec![4], b"{}".to_vec())
        .await
        .expect("held");
    store
        .begin_ceremony(
            crate::CeremonyKind::SignUp,
            None,
            Some(email.into()),
            Vec::new(),
        )
        .await
        .expect("begun");
    store
        .challenge(Session::new(account, Id::random().expect("random")))
        .await
        .expect("a challenge");
    store.clock().advance(Duration::from_secs(599));
    assert_eq!(store.delete_expired().await.expect("swept"), 0);
    store.clock().advance(Duration::from_secs(1));
    assert_eq!(store.delete_expired().await.expect("swept"), 4);
}

/// `GET .../policy` (§8): a device whose account `remove_member` removed
/// may still fetch the policy chain up to and including that change, and
/// nothing after it, while every other route refuses it; a device that no
/// change ever counted gets nothing.
pub async fn a_removed_member_reads_the_policy_up_to_its_removal<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let (member, member_keys, member_links) = account(2);
    world
        .store
        .append_account_links(member_links.clone())
        .await
        .expect("chain");
    let member_session = Session::new(member, member_keys[0].device());
    let add = world.change(
        0,
        vec![head_of(&world.links), head_of(&member_links[..1])],
        PolicyKind::AddMember {
            account: member,
            role: sync_protocol::records::Role::Editor,
            invitation: Id::random().expect("random"),
        },
    );
    world.inject_policy(add, vec![world.domain], None).await;
    let remove = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::RemoveMember { account: member },
    );
    let removed_at = remove.body.revision;
    world.inject_policy(remove, vec![world.domain], None).await;
    let stranger = DeviceKeys::generate().expect("random");
    let link = next_link(
        &world.devices[0].keys,
        &head_of(&world.links),
        LinkKind::AddDevice {
            card: Nested(stranger.card(world.account)),
            offer: Digest([9; 32]),
        },
    );
    world
        .store
        .append_account_links(vec![link.clone()])
        .await
        .expect("grown");
    world.links.push(link);
    let later = world.change(0, vec![head_of(&world.links)], PolicyKind::AccountHeads);
    world.inject_policy(later, vec![world.domain], None).await;

    let served = world
        .store
        .policy(member_session, world.workspace, 0)
        .await
        .expect("up to the removal");
    assert_eq!(served.head_revision, removed_at);
    let revisions: Vec<u64> = served.changes.0.iter().map(|c| c.0.body.revision).collect();
    assert_eq!(revisions, (1..=removed_at).collect::<Vec<_>>());
    let keys = world
        .store
        .keys(member_session, world.workspace, 0, member_session.device)
        .await;
    assert_eq!(refusal(keys).code, "forbidden");
    let outsider = Session::new(Id::random().expect("random"), Id::random().expect("random"));
    let policy = world.store.policy(outsider, world.workspace, 0).await;
    assert_eq!(refusal(policy).code, "forbidden");
}

/// `POST /v2/accounts/chains` (§7): a member's current chain in full to
/// the workspace's devices, a removed member's chain up to its last
/// recorded head, the remover's chain to the removed member up to its
/// removal, and nothing to an account that shares no workspace.
pub async fn chain_reads_follow_the_policy<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let (member, member_keys, member_links) = account(2);
    world
        .store
        .append_account_links(member_links.clone())
        .await
        .expect("chain");
    let member_session = Session::new(member, member_keys[0].device());
    let shared = world.store.clone();
    let read = |session: Session, account: Id| {
        let store = shared.clone();
        async move {
            store
                .read_chains(
                    session,
                    sync_protocol::messages::ChainsRequest {
                        cursors: List(vec![sync_protocol::messages::ChainCursor {
                            account,
                            genesis: None,
                            after: 0,
                        }]),
                    },
                )
                .await
                .map(|r| r.pages.0[0].head_seqno)
        }
    };
    let owner = world.devices[0].session;
    assert_eq!(refusal(read(owner, member).await).code, "not_found");
    let add = world.change(
        0,
        vec![head_of(&world.links), head_of(&member_links[..1])],
        PolicyKind::AddMember {
            account: member,
            role: sync_protocol::records::Role::Editor,
            invitation: Id::random().expect("random"),
        },
    );
    world.inject_policy(add, vec![world.domain], None).await;
    assert_eq!(read(owner, member).await.expect("in full"), 2);
    assert_eq!(
        read(member_session, world.account).await.expect("recorded"),
        1
    );
    let remove = world.change(
        0,
        vec![head_of(&world.links)],
        PolicyKind::RemoveMember { account: member },
    );
    world.inject_policy(remove, vec![world.domain], None).await;
    assert_eq!(read(owner, member).await.expect("as recorded"), 1);
    let stranger = DeviceKeys::generate().expect("random");
    let link = next_link(
        &world.devices[0].keys,
        &head_of(&world.links),
        LinkKind::AddDevice {
            card: Nested(stranger.card(world.account)),
            offer: Digest([9; 32]),
        },
    );
    world
        .store
        .append_account_links(vec![link.clone()])
        .await
        .expect("grown");
    world.links.push(link);
    let heads = world.change(0, vec![head_of(&world.links)], PolicyKind::AccountHeads);
    world.inject_policy(heads, vec![world.domain], None).await;
    assert_eq!(
        read(member_session, world.account)
            .await
            .expect("to its removal"),
        1
    );
    assert_eq!(read(owner, world.account).await.expect("its own"), 2);
}

/// A chain that a reset retired while a workspace still names it for a
/// current member reaches that workspace's devices in full, as it did while
/// it was current, until the workspace accepts the reset (§7). The new
/// chain, which the workspace doesn't record yet, reads not at all through
/// it.
pub async fn a_retired_chain_the_workspace_names_reads_in_full<B: Backend>(store: Store<B>) {
    let mut world = World::new(store, 1).await;
    let (member, _, member_links) = account(2);
    world
        .store
        .append_account_links(member_links.clone())
        .await
        .expect("chain");
    let add = world.change(
        0,
        vec![head_of(&world.links), head_of(&member_links[..1])],
        PolicyKind::AddMember {
            account: member,
            role: sync_protocol::records::Role::Editor,
            invitation: Id::random().expect("random"),
        },
    );
    world.inject_policy(add, vec![world.domain], None).await;
    let owner = world.devices[0].session;
    let shared = world.store.clone();
    let read = |genesis: Digest| {
        let store = shared.clone();
        async move {
            store
                .read_chains(
                    owner,
                    sync_protocol::messages::ChainsRequest {
                        cursors: List(vec![sync_protocol::messages::ChainCursor {
                            account: member,
                            genesis: Some(genesis),
                            after: 0,
                        }]),
                    },
                )
                .await
                .map(|r| (r.pages.0[0].status, r.pages.0[0].head_seqno))
        }
    };
    let old = member_links[0].digest();
    assert_eq!(
        read(old).await.expect("in full"),
        (sync_protocol::messages::ChainStatus::Current, 2)
    );

    // A reset: a new genesis becomes the member's current chain.
    let reset = genesis_link(&DeviceKeys::generate().expect("random"), member);
    let row = crate::LinkRow {
        genesis: reset.digest(),
        seqno: 1,
        link: reset.to_bytes(),
    };
    let new = reset.digest();
    world
        .store
        .backend()
        .write(Box::new(move |txn: &mut B::Write| {
            Box::pin(async move {
                txn.insert_account_link(member, &row, new).await?;
                txn.set_current_genesis(member, new).await
            })
        }))
        .await
        .expect("reset");
    assert_eq!(
        read(old)
            .await
            .expect("in full while the workspace names it"),
        (sync_protocol::messages::ChainStatus::Retired, 2)
    );
    assert_eq!(refusal(read(new).await).code, "not_found");
}

/// `POST /v2/recovery` (§12): a proof from the active recovery identity for
/// the session's enrolled card, at the server's origin, consumes a current
/// challenge once and lets the session read the identity's envelopes.
pub async fn recovery_proofs_consume_their_challenge<B: Backend>(store: Store<B>) {
    let (account, keys, mut links) = account(1);
    store
        .append_account_links(links.clone())
        .await
        .expect("chain");
    let kit = DeviceKeys::generate().expect("random");
    let add = next_link(
        &keys[0],
        &head_of(&links),
        LinkKind::AddRecovery {
            card: Nested(kit.card(account)),
        },
    );
    store
        .append_account_links(vec![add.clone()])
        .await
        .expect("kit");
    links.push(add);
    let phone = DeviceKeys::generate().expect("random");
    store
        .enroll_card(phone.card(account))
        .await
        .expect("enrolled");
    let token = random_token();
    let session = Session::new(account, phone.device());
    store.create_session(token, session).await.expect("session");
    let hash = sync_protocol::crypto::hash(&token);
    let origin = "https://sync.example.com";
    let challenge = store
        .challenge(session)
        .await
        .expect("a challenge")
        .challenge;
    let proof =
        sync_key_envelope::v2::recovery_proof(&kit, origin, &phone.card(account), challenge);
    assert_eq!(
        refusal(
            store
                .prove_recovery(session, hash, "https://elsewhere.example", proof.clone())
                .await
        )
        .code,
        "origin_mismatch"
    );
    store
        .prove_recovery(session, hash, origin, proof.clone())
        .await
        .expect("proven");
    assert_eq!(
        refusal(store.prove_recovery(session, hash, origin, proof).await).code,
        "challenge_invalid"
    );
    let proven = store
        .session(&sync_protocol::types::to_base64url(&token))
        .await
        .expect("the session");
    assert_eq!(proven.recovery, Some(kit.device()));
}

/// Reads the current chain of `account` as `session` and returns the
/// served head seqno.
async fn served_seqno<B: Backend>(
    store: &Store<B>,
    session: Session,
    account: Id,
) -> crate::Result<u64> {
    store
        .read_chains(
            session,
            sync_protocol::messages::ChainsRequest {
                cursors: List(vec![sync_protocol::messages::ChainCursor {
                    account,
                    genesis: None,
                    after: 0,
                }]),
            },
        )
        .await
        .map(|r| r.pages.0[0].head_seqno)
}

/// Approval requests (§7, §10): while an account's approval request that
/// names an invitation to a workspace waits, the sessions of that
/// workspace's member accounts read the account's current chain in full,
/// and nobody else reads it through the request. A request that names no
/// invitation, or an invitation to another workspace, opens nothing, and
/// neither does an expired one, which the store deletes.
pub async fn approval_requests_open_the_invitees_chain<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let (invitee, invitee_keys, invitee_links) = account(2);
    world
        .store
        .append_account_links(invitee_links)
        .await
        .expect("chain");
    let (outsider, outsider_keys, outsider_links) = account(1);
    world
        .store
        .append_account_links(outsider_links)
        .await
        .expect("chain");
    let store = &world.store;
    let owner = world.devices[0].session;
    let outsider = Session::new(outsider, outsider_keys[0].device());
    let request = Session::new(invitee, invitee_keys[1].device());
    assert_eq!(
        refusal(served_seqno(store, owner, invitee).await).code,
        "not_found"
    );
    store
        .store_approval_request(request, Digest([1; 32]), None)
        .await
        .expect("stored");
    let elsewhere = (Id::random().expect("random"), Id::random().expect("random"));
    store
        .store_approval_request(request, Digest([2; 32]), Some(elsewhere))
        .await
        .expect("stored");
    assert_eq!(
        refusal(served_seqno(store, owner, invitee).await).code,
        "not_found",
        "no request names an invitation to the owner's workspace"
    );
    let invitation = Id::random().expect("random");
    store
        .store_approval_request(
            request,
            Digest([3; 32]),
            Some((invitation, world.workspace)),
        )
        .await
        .expect("stored");
    assert_eq!(
        served_seqno(store, owner, invitee).await.expect("in full"),
        2
    );
    assert_eq!(
        refusal(served_seqno(store, outsider, invitee).await).code,
        "not_found",
        "the outsider's account doesn't belong to the workspace"
    );
    store
        .clock()
        .advance(Duration::from_secs(crate::SIGN_IN_TTL));
    assert_eq!(
        refusal(served_seqno(store, owner, invitee).await).code,
        "not_found",
        "the request expired"
    );
    assert_eq!(store.delete_expired().await.expect("deleted"), 3);
}

/// Two geneses for one new account at once, through two instances (§7,
/// §15): one starts the account's chain, and the other gets `forbidden`,
/// as it would after the first committed.
pub async fn geneses_at_once_start_one_chain<B: Backend>(
    first: Arc<Store<B>>,
    second: Arc<Store<B>>,
) {
    for _ in 0..8 {
        let account = Id::random().expect("random");
        let keys = [
            DeviceKeys::generate().expect("random"),
            DeviceKeys::generate().expect("random"),
        ];
        for key in &keys {
            first
                .enroll_card(key.card(account))
                .await
                .expect("enrolled");
        }
        let geneses: Vec<Signed<AccountLink>> =
            keys.iter().map(|k| genesis_link(k, account)).collect();
        let tasks: Vec<_> = [first.clone(), second.clone()]
            .into_iter()
            .zip(keys.iter().zip(&geneses))
            .map(|(store, (key, genesis))| {
                let session = Session::new(account, key.device());
                let genesis = genesis.clone();
                tokio::spawn(async move { store.append_chain(session, vec![genesis]).await })
            })
            .collect();
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.expect("the append ends"));
        }
        let winner = results
            .iter()
            .position(|r| r.is_ok())
            .expect("one genesis starts the chain");
        let loser = 1 - winner;
        assert_eq!(
            refusal(results.swap_remove(loser)).code,
            "forbidden",
            "the later genesis"
        );
        let session = Session::new(account, keys[winner].device());
        let page = first
            .read_chains(
                session,
                sync_protocol::messages::ChainsRequest {
                    cursors: List(vec![sync_protocol::messages::ChainCursor {
                        account,
                        genesis: None,
                        after: 0,
                    }]),
                },
            )
            .await
            .expect("its own chain");
        let page = &page.pages.0[0];
        assert_eq!(page.genesis, geneses[winner].digest());
        assert_eq!(page.head_seqno, 1);
        assert_eq!(
            refusal(
                second
                    .read_chains(
                        session,
                        sync_protocol::messages::ChainsRequest {
                            cursors: List(vec![sync_protocol::messages::ChainCursor {
                                account,
                                genesis: Some(geneses[loser].digest()),
                                after: 0,
                            }]),
                        },
                    )
                    .await
            )
            .code,
            "not_found",
            "the store kept no link of the later genesis"
        );
    }
}

/// Enrollments of one device ID at once, through two instances, for
/// accounts without a chain (§7): the same card enrolls on both, and of two
/// accounts' cards one enrolls and the other gets `id_conflict`, never
/// `internal`.
pub async fn enrollments_at_once_take_one_card<B: Backend>(
    first: Arc<Store<B>>,
    second: Arc<Store<B>>,
) {
    for _ in 0..8 {
        let keys = DeviceKeys::generate().expect("random");
        let account = Id::random().expect("random");
        let card = keys.card(account);
        let tasks: Vec<_> = [first.clone(), second.clone()]
            .into_iter()
            .map(|store| {
                let card = card.clone();
                tokio::spawn(async move { store.enroll_card(card).await })
            })
            .collect();
        for task in tasks {
            task.await
                .expect("the enrollment ends")
                .expect("the same card enrolls twice");
        }

        let keys = DeviceKeys::generate().expect("random");
        let cards = [
            keys.card(Id::random().expect("random")),
            keys.card(Id::random().expect("random")),
        ];
        let tasks: Vec<_> = [first.clone(), second.clone()]
            .into_iter()
            .zip(cards)
            .map(|(store, card)| tokio::spawn(async move { store.enroll_card(card).await }))
            .collect();
        let mut results = Vec::new();
        for task in tasks {
            results.push(task.await.expect("the enrollment ends"));
        }
        let enrolled = results.iter().filter(|r| r.is_ok()).count();
        assert_eq!(enrolled, 1, "one card takes the device ID");
        let loser = results.into_iter().find(|r| r.is_err()).expect("a refusal");
        assert_eq!(refusal(loser).code, "id_conflict");
    }
}

/// The access list naming `account` as a manager.
pub fn manager(account: Id) -> sync_protocol::records::AccessList {
    Sorted::new(vec![Access {
        account,
        level: Level::Manage,
    }])
    .expect("one entry")
}

/// A pushed profile (§25): an owner or admin device signs it at a policy
/// revision the server holds, under the workspace domain's current epoch,
/// one above the latest profile, with a padded ciphertext, and the
/// workspace list serves the latest.
pub async fn profile_pushes_follow_section_25<B: Backend>(store: Store<B>) {
    let world = World::new(store, 1).await;
    let epoch_record = {
        let response = world
            .store
            .epochs(world.devices[0].session, world.workspace, world.domain, 1)
            .await
            .expect("epochs");
        response.epochs.0[0].0.digest()
    };
    let profile = |revision: u64, edit: &dyn Fn(&mut Profile)| {
        let mut body = Profile {
            workspace: world.workspace,
            profile_revision: revision,
            epoch: 1,
            policy_revision: world.head.0,
            policy_head: world.head.1,
            signer: world.devices[0].id(),
            nonce: Nonce::ZERO,
            ciphertext: Bytes(Vec::new()),
        };
        body.seal_payload(
            &ProfilePayload {
                name: Text("Renamed".into()),
            },
            &world.epoch_key,
            &epoch_record,
            random_nonce().expect("random"),
        );
        edit(&mut body);
        Signed::sign(body, world.devices[0].keys.signing())
    };
    let push = |record: Signed<Profile>| {
        let body = PushRequest {
            records: List(vec![RecordRef::Profile(record)]),
        }
        .to_bytes();
        world
            .store
            .push(world.devices[0].session, world.workspace, body)
    };
    let renamed = profile(2, &|_| {});
    let response = push(renamed.clone()).await.expect("the profile commits");
    assert!(
        response.receipts.0.is_empty(),
        "a profile makes no log entry"
    );
    let listed = world
        .store
        .workspaces(world.devices[0].session)
        .await
        .expect("listed");
    let latest = listed.workspaces.0[0].profile.clone().expect("a profile").0;
    assert_eq!(latest, renamed);

    for (revision, edit, code) in [
        (
            2u64,
            (&|_: &mut Profile| {}) as &dyn Fn(&mut Profile),
            "profile_conflict",
        ),
        (4, &|_: &mut Profile| {}, "profile_conflict"),
        (3, &|p: &mut Profile| p.policy_revision = 9, "policy_stale"),
        (
            3,
            &|p: &mut Profile| p.policy_head = Digest([3; 32]),
            "policy_conflict",
        ),
        (3, &|p: &mut Profile| p.epoch = 2, "epoch_stale"),
        (
            3,
            &|p: &mut Profile| p.ciphertext = Bytes(vec![0; 300]),
            "padding_invalid",
        ),
    ] {
        assert_eq!(
            push_failure(push(profile(revision, edit)).await),
            (code, 0),
            "{code}"
        );
    }
    let mut forged = profile(3, &|_| {});
    forged.sig.0[0] ^= 1;
    assert_eq!(push_failure(push(forged).await), ("signature_invalid", 0));
    push(profile(3, &|_| {}))
        .await
        .expect("the next revision commits");
}
