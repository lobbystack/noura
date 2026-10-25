//! `sync-v2-encoding.json`: field types, tags, `derive_key` contexts,
//! decoding rules, field limits, every message body, error codes and
//! details, pull page statuses and realtime frames.

use serde_json::{Value, json};
use sync_protocol::codec::{Bytes, Encode, List, Nested, Sorted, Text};
use sync_protocol::crypto::{context, derive_key};
use sync_protocol::messages::*;
use sync_protocol::records::*;
use sync_protocol::tag::Tag;
use sync_protocol::types::{Digest, Id, IdOrZero, Key, to_hex};

use super::common::{Actor, Fixture, Shape, id, secret};
use super::world::{World, world};

fn entry_op(w: &World, seq: u64) -> Entry {
    Entry {
        seq,
        record: EntryRecord::Operation(w.op.clone()),
    }
}

fn page(w: &World, status: PageStatus) -> Page {
    let ok = status == PageStatus::Ok;
    Page {
        domain: w.ws_domain,
        status,
        after: 5,
        complete_through: if ok { 6 } else { 5 },
        head: if ok { 6 } else { 0 },
        retention_floor: if matches!(status, PageStatus::Ok | PageStatus::Compacted) {
            2
        } else {
            0
        },
        checkpoint_seq: if matches!(status, PageStatus::Ok | PageStatus::Compacted) {
            6
        } else {
            0
        },
        restore_generation: 1_760_000_000,
        policy_revision: 3,
        checkpoint_due: if ok { 10_006 } else { 0 },
        entries: if ok {
            Sorted::new(vec![entry_op(w, 6)]).unwrap()
        } else {
            Sorted::default()
        },
    }
}

fn link_bytes(link: &Signed<AccountLink>) -> LinkBytes {
    Nested(link.clone())
}

fn body<T: Encode + serde::Serialize>(f: &mut Fixture, name: &str, value: &T) {
    f.accept(name, name, value, None, Shape::Body);
}

pub fn fixture() -> Fixture {
    let w = world();
    let mut f = Fixture::new(
        "sync-v2-encoding.json",
        "Binary encoding of sync protocol v2 (sync-v2.md §4, §15, §23, §26): tags, derive_key contexts, decoding rules and field limits, every message body, error codes and details, pull pages and realtime frames.",
    );
    f.actor(&w.alice_laptop);
    f.actor(&w.alice_phone);
    f.actor(&w.bob_laptop);

    // Tags and contexts.
    let tags: Vec<Value> = Tag::ALL
        .iter()
        .map(|t| json!({"domain": t.domain(), "length": t.domain().len(), "bytes": to_hex(&t.bytes())}))
        .collect();
    f.set("tags", json!(tags));
    let input = secret("context input");
    let contexts: Vec<Value> = context::ALL
        .iter()
        .map(|c| {
            json!({
                "context": c,
                "input": to_hex(input.expose()),
                "output": to_hex(derive_key(c, input.expose()).expose()),
            })
        })
        .collect();
    f.set("contexts", json!(contexts));

    // General and per-field limits (§4, §27).
    f.set(
        "limits",
        json!({
            "general": {
                "str_bytes": 4096,
                "bytes_and_list": "at most the bytes left in the body, each list element counted at its smallest encoding",
            },
            "fields": [
                {"field": "card.hpke_public", "exactly": 1216},
                {"field": "envelope.enc", "exactly": 1120},
                {"field": "envelope.ciphertext", "exactly": 48},
                {"field": "epoch.lineage", "one_of": [0, 48]},
                {"field": "epoch_bundle.recipients", "max_count": 512, "sorted": "whole"},
                {"field": "operation.supersedes", "max_count": 256, "sorted": "device"},
                {"field": "operation.blobs", "max_count": 4, "sorted": "whole"},
                {"field": "operation.ciphertext", "max_bytes": 270352},
                {"field": "operation.counter", "min": 1},
                {"field": "payload.path", "max_bytes": 4096},
                {"field": "payload.text_update.load", "min": 1, "max": 536870911},
                {"field": "payload.meta", "sorted": "key"},
                {"field": "policy.heads", "sorted": "account"},
                {"field": "access", "max_count": 1024, "sorted": "account"},
                {"field": "account_link.remove.keep", "max_count": 256, "sorted": "whole"},
                {"field": "checkpoint.number", "min": 1},
                {"field": "checkpoint_bundle.heads", "sorted": "device"},
                {"field": "offer.anchors", "sorted": "workspace"},
                {"field": "kit_payload.workspaces", "sorted": "workspace"},
                {"field": "kit_workspace.anchors", "sorted": "domain"},
                {"field": "presence.ciphertext", "max_bytes": 4112},
                {"field": "presence_payload.selections", "max_count": 8},
                {"field": "selection.anchor", "max_bytes": 192},
                {"field": "selection.head", "max_bytes": 192},
                {"field": "share.ciphertext", "max_bytes": 983056},
                {"field": "entries", "sorted": "seq"},
                {"field": "manifest_node_response.children", "max_count": 16},
                {"field": "manifest_buckets_response.buckets", "max_count": 4096},
                {"field": "manifest_leaves_response.leaves", "sorted": "rid, device, counter"},
                {"field": "frame.body", "max_bytes": 65536},
                {"field": "subscribe.channels", "max_count": 256},
                {"field": "epoch.epoch", "min": 1},
                {"field": "id", "nonzero": true, "except": ["frame.error.domain", "frame.error.session"]},
            ],
        }),
    );

    // Field types.
    f.accept(
        "account_head",
        "account_head",
        &w.alice.head(),
        None,
        Shape::Body,
    );

    // Decoding rules.
    let card = w.alice_laptop.card();
    let card_bytes = card.to_bytes();
    let mut trailing = card_bytes.clone();
    trailing.push(0);
    f.reject(
        "card_trailing_bytes",
        "card",
        &trailing,
        json!({"decode": "trailing_bytes"}),
    );
    let mut short_key = card_bytes.clone();
    let at = 25 + 16 + 16 + 32;
    short_key[at..at + 4].copy_from_slice(&1215u32.to_be_bytes());
    short_key.remove(short_key.len() - 1);
    f.reject(
        "card_hpke_public_1215_bytes",
        "card",
        &short_key,
        json!({"decode": "wrong_length"}),
    );
    let mut unknown = card_bytes.clone();
    unknown[5] = b'X';
    f.reject(
        "card_unknown_tag",
        "card",
        &unknown,
        json!({"decode": "unknown_tag"}),
    );
    f.reject(
        "v1_json_tuple",
        "card",
        br#"["noura.sync.device.card",1]"#,
        json!({"decode": "unknown_tag"}),
    );
    let mut zero = card_bytes.clone();
    zero[25..41].fill(0);
    f.reject(
        "card_zero_account",
        "card",
        &zero,
        json!({"decode": "zero_id"}),
    );
    let mut mismatch = card_bytes.clone();
    mismatch[41] ^= 1;
    f.reject(
        "card_device_id_not_derived",
        "card",
        &mismatch,
        json!({"decode": "device_id_mismatch"}),
    );
    // A str above 4,096 bytes, refused by its length alone.
    let mut long_origin = Tag::DeviceEnroll.bytes();
    long_origin.extend_from_slice(&4097u16.to_be_bytes());
    f.reject(
        "enroll_origin_4097_bytes",
        "enroll",
        &long_origin,
        json!({"decode": "too_long"}),
    );
    let mut bad_utf8 = Tag::DeviceEnroll.bytes();
    bad_utf8.extend_from_slice(&2u16.to_be_bytes());
    bad_utf8.extend_from_slice(&[0xc3, 0x28]);
    f.reject(
        "enroll_origin_invalid_utf8",
        "enroll",
        &bad_utf8,
        json!({"decode": "invalid_utf8"}),
    );
    // An unknown enum value: policy kind 11.
    let mut kind = w.policy.changes[0].to_bytes();
    let kind_at = kind.len() - 64 - 16 - 1;
    kind[kind_at] = 11;
    f.reject(
        "policy_unknown_kind",
        "policy_change",
        &kind,
        json!({"decode": "unknown_value"}),
    );
    // A list the spec calls sorted with a duplicate key: an access list
    // naming one account twice.
    let duplicate = w.policy.next(
        &w.alice_laptop,
        vec![w.alice.head()],
        PolicyKind::SetAccess {
            domain: w.restricted,
            access: Sorted::unchecked(vec![
                Access {
                    account: w.bob_laptop.account,
                    level: Level::Read,
                },
                Access {
                    account: w.bob_laptop.account,
                    level: Level::Write,
                },
            ]),
        },
    );
    f.reject(
        "access_list_naming_one_account_twice",
        "policy_change",
        &duplicate.to_bytes(),
        json!({"decode": "not_sorted"}),
    );
    // A tag that doesn't match its position: a record_ref of type 1
    // (operation) holding a policy change.
    let mut misplaced = vec![1u8];
    let change = w.policy.changes[0].to_bytes();
    misplaced.extend_from_slice(&(change.len() as u32).to_be_bytes());
    misplaced.extend_from_slice(&change);
    f.reject(
        "record_ref_tag_mismatch",
        "record_ref",
        &misplaced,
        json!({"decode": "wrong_tag"}),
    );
    let mut unknown_type = misplaced.clone();
    unknown_type[0] = 7;
    f.reject(
        "record_ref_unknown_type",
        "record_ref",
        &unknown_type,
        json!({"decode": "unknown_value"}),
    );
    // An opt discriminator other than 0 or 1.
    let request = ApprovalRequest {
        commit: Digest([1; 32]),
        account: None,
        invitation: None,
        invite_proof: None,
        workspace: None,
    };
    let mut bad_opt = request.to_bytes();
    bad_opt[32] = 2;
    f.reject(
        "opt_discriminator_2",
        "approval_request",
        &bad_opt,
        json!({"decode": "bad_option"}),
    );
    // A bytes field above its own limit, refused by its length prefix
    // before any of its 270,353 bytes: the record ends right after it.
    let mut huge = w.op.clone();
    huge.body.ciphertext = Bytes(Vec::new());
    let mut huge = huge.to_bytes();
    huge.truncate(huge.len() - 64 - 4);
    huge.extend_from_slice(&270_353u32.to_be_bytes());
    f.reject(
        "operation_ciphertext_270353_bytes",
        "operation",
        &huge,
        json!({"decode": "too_long"}),
    );
    // A list count above the bytes left.
    let mut count = DevicesResponse {
        devices: List::default(),
    }
    .to_bytes();
    count.copy_from_slice(&u32::MAX.to_be_bytes());
    f.reject(
        "list_count_above_body",
        "devices_response",
        &count,
        json!({"decode": "truncated"}),
    );

    // Records the other files don't own.
    f.accept(
        "record_ref_operation",
        "record_ref",
        &RecordRef::Operation(w.op.clone()),
        None,
        Shape::Body,
    );
    f.accept(
        "entry_policy",
        "entry",
        &Entry {
            seq: 1,
            record: EntryRecord::Policy(PolicyEntry {
                revision: 1,
                head: w.policy.changes[0].digest(),
            }),
        },
        None,
        Shape::Body,
    );
    f.accept(
        "policy_entry",
        "policy_entry",
        &PolicyEntry {
            revision: 3,
            head: w.policy.changes[2].digest(),
        },
        None,
        Shape::Body,
    );
    f.accept(
        "receipt",
        "receipt",
        &Receipt {
            record_digest: w.op.digest(),
            domain: w.ws_domain,
            seq: 6,
        },
        None,
        Shape::Body,
    );

    // Every message body of §26.
    body(
        &mut f,
        "limits_response",
        &LimitsResponse {
            protocol: 2,
            restore_generation: 1_760_000_000,
            max_body: 1_048_576,
            max_inline: 262_144,
            max_blob: 2_147_483_648,
            max_covers: 256,
            max_recipients: 512,
            presence_per_second: 2,
            retained_bytes: 1_073_741_824,
        },
    );
    body(
        &mut f,
        "device_authorize_response",
        &DeviceAuthorizeResponse {
            device_code: Text::new(Key([7; 32]).to_base64url()).unwrap(),
            user_code: Text::new("BCDF-GHJK").unwrap(),
            verification_uri: Text::new("https://sync.noura.app/device").unwrap(),
            expires_in: 600,
            interval: 5,
        },
    );
    body(
        &mut f,
        "device_token_request",
        &DeviceTokenRequest {
            device_code: Text::new(Key([7; 32]).to_base64url()).unwrap(),
        },
    );
    body(
        &mut f,
        "device_token_response",
        &DeviceTokenResponse {
            session_token: Text::new(Key([8; 32]).to_base64url()).unwrap(),
            account: Id([0x0a; 16]),
        },
    );
    body(
        &mut f,
        "challenge_response",
        &ChallengeResponse {
            challenge: Key([9; 32]),
            expires_in: 600,
        },
    );
    let enroll = sync_key_envelope::v2::enroll(
        &w.alice_laptop.keys,
        w.alice_laptop.account,
        "https://sync.noura.app",
        Key([9; 32]),
    );
    body(
        &mut f,
        "enroll_request",
        &EnrollRequest {
            enroll: Nested(enroll),
        },
    );
    body(
        &mut f,
        "devices_response",
        &DevicesResponse {
            devices: List(vec![
                DeviceInfo {
                    card: Nested(w.alice_laptop.card()),
                    sessions: 1,
                    state: DeviceState::Active,
                },
                DeviceInfo {
                    card: Nested(Actor::new("alice", "alice unlisted").card()),
                    sessions: 1,
                    state: DeviceState::Enrolled,
                },
            ]),
        },
    );
    body(
        &mut f,
        "chain_append_request",
        &ChainAppendRequest {
            links: List(vec![link_bytes(&w.alice.links[1])]),
        },
    );
    body(
        &mut f,
        "chain_append_response",
        &ChainAppendResponse { seqno: 2 },
    );
    body(
        &mut f,
        "chains_request",
        &ChainsRequest {
            cursors: List(vec![
                ChainCursor {
                    account: w.alice_laptop.account,
                    genesis: None,
                    after: 0,
                },
                ChainCursor {
                    account: w.bob_laptop.account,
                    genesis: Some(w.bob.links[0].digest()),
                    after: 0,
                },
            ]),
        },
    );
    body(
        &mut f,
        "chains_response",
        &ChainsResponse {
            pages: List(vec![
                ChainPage {
                    account: w.alice_laptop.account,
                    genesis: w.alice.links[0].digest(),
                    status: ChainStatus::Current,
                    head_seqno: 2,
                    links: List(w.alice.links.iter().map(link_bytes).collect()),
                },
                ChainPage {
                    account: w.bob_laptop.account,
                    genesis: w.bob.links[0].digest(),
                    status: ChainStatus::Retired,
                    head_seqno: 1,
                    links: List(vec![link_bytes(&w.bob.links[0])]),
                },
            ]),
        },
    );
    body(
        &mut f,
        "reset_request",
        &ResetRequest {
            code: Text::new("BCDFGHJK").unwrap(),
            genesis: link_bytes(
                &super::world::Chain::genesis(&Actor::new("bob", "bob reset laptop")).links[0],
            ),
        },
    );
    body(
        &mut f,
        "approval_request",
        &ApprovalRequest {
            commit: Digest([1; 32]),
            account: Some(w.bob.head()),
            invitation: Some(id("invitation")),
            invite_proof: Some(Digest([2; 32])),
            workspace: None,
        },
    );
    body(
        &mut f,
        "pending_response",
        &PendingResponse {
            pending: List(vec![Pending {
                card: Nested(w.bob_laptop.card()),
                commit: Digest([1; 32]),
                account: Some(w.bob.head()),
                invitation: Some(id("invitation")),
                invite_proof: Some(Digest([2; 32])),
                workspace: None,
            }]),
        },
    );
    let offer = super::approval::offer(&w);
    body(
        &mut f,
        "offer_request",
        &OfferRequest {
            offer: Nested(offer.clone()),
        },
    );
    body(
        &mut f,
        "offers_response",
        &OffersResponse {
            offers: List(vec![Nested(offer)]),
        },
    );
    body(&mut f, "reveal_body", &RevealBody { nc: Key([3; 32]) });
    body(
        &mut f,
        "recovery_request",
        &RecoveryRequest {
            recovery_proof: Nested(super::recovery::proof(&w)),
        },
    );
    body(
        &mut f,
        "workspaces_response",
        &WorkspacesResponse {
            workspaces: List(vec![WorkspaceInfo {
                workspace: w.workspace,
                role: Role::Owner,
                used_bytes: 4_096,
                history_bytes: 0,
                quota_bytes: None,
                profile: Some(Nested(w.profile.clone())),
            }]),
        },
    );
    let envelope_alice = w.envelope_bob.clone();
    body(
        &mut f,
        "push_request",
        &PushRequest {
            records: List(vec![
                RecordRef::Policy(w.policy.changes[2].clone()),
                RecordRef::EpochBundle(w.ws_bundle2.clone()),
                RecordRef::Envelope(envelope_alice),
                RecordRef::Operation(w.op.clone()),
                RecordRef::CheckpointBundle(w.checkpoint.clone()),
                RecordRef::Profile(w.profile.clone()),
            ]),
        },
    );
    body(
        &mut f,
        "push_response",
        &PushResponse {
            policy_revision: 3,
            receipts: List(vec![Receipt {
                record_digest: w.op.digest(),
                domain: w.ws_domain,
                seq: 6,
            }]),
        },
    );
    body(
        &mut f,
        "policy_response",
        &PolicyResponse {
            head_revision: 3,
            changes: List(w.policy.changes.iter().cloned().map(Nested).collect()),
            account_heads: Sorted::new(vec![w.alice.head(), w.bob.head()]).unwrap(),
        },
    );
    body(
        &mut f,
        "keys_response",
        &KeysResponse {
            head_key_seq: 4,
            entries: List(vec![
                KeyEntry {
                    key_seq: 2,
                    record: KeyRecord::Epoch(w.ws_epoch2.clone()),
                },
                KeyEntry {
                    key_seq: 3,
                    record: KeyRecord::Envelope(w.envelope_bob.clone()),
                },
                KeyEntry {
                    key_seq: 4,
                    record: KeyRecord::Rewrap(RewrapRequest {
                        device: w.bob_laptop.device(),
                        epoch: 2,
                    }),
                },
            ]),
        },
    );
    body(
        &mut f,
        "keys_current_response",
        &KeysResponse {
            head_key_seq: 3,
            entries: List(vec![
                KeyEntry {
                    key_seq: 2,
                    record: KeyRecord::Epoch(w.ws_epoch2.clone()),
                },
                KeyEntry {
                    key_seq: 3,
                    record: KeyRecord::Envelope(w.envelope_bob.clone()),
                },
            ]),
        },
    );
    body(
        &mut f,
        "epochs_response",
        &EpochsResponse {
            epochs: List(vec![
                Nested(w.ws_epoch2.clone()),
                Nested(w.ws_epoch1.clone()),
            ]),
        },
    );
    body(&mut f, "rewrap_body", &RewrapBody { epoch: 2 });
    body(
        &mut f,
        "pull_request",
        &PullRequest {
            cursors: List(vec![
                PullCursor {
                    domain: w.ws_domain,
                    after: 5,
                },
                PullCursor {
                    domain: w.restricted,
                    after: 0,
                },
            ]),
            wait_seconds: 25,
        },
    );
    body(
        &mut f,
        "pull_response",
        &PullResponse {
            pages: List(vec![
                page(&w, PageStatus::Ok),
                page(&w, PageStatus::Forbidden),
            ]),
        },
    );
    body(&mut f, "entry", &entry_op(&w, 6));
    body(
        &mut f,
        "head_response",
        &HeadResponse {
            counter: 1,
            record_digest: w.op.digest(),
            seq: 6,
        },
    );
    body(
        &mut f,
        "checkpoint_response",
        &CheckpointResponse {
            seq: 7,
            bundle: w.checkpoint.clone(),
        },
    );
    body(
        &mut f,
        "checkpoint_chain_response",
        &CheckpointChainResponse {
            checkpoints: List(vec![w.checkpoint.checkpoint.clone()]),
        },
    );
    body(
        &mut f,
        "live_response",
        &LiveResponse {
            at: 6,
            next: Some(LiveCursor {
                rid: w.rid,
                device: w.alice_laptop.device(),
                counter: 1,
            }),
            entries: List(vec![entry_op(&w, 6)]),
        },
    );
    let manifest = sync_protocol::manifest::Manifest::new(
        [(w.rid, w.checkpoint.heads.to_vec())].into_iter().collect(),
    );
    let buckets = manifest.bucket_hashes();
    body(
        &mut f,
        "manifest_node_response",
        &ManifestNodeResponse {
            children: List(sync_protocol::manifest::node_children(&buckets, 0, 0).unwrap()),
        },
    );
    body(
        &mut f,
        "manifest_leaves_response",
        &ManifestLeavesResponse {
            leaves: Sorted::new(vec![LeafRef {
                rid: w.rid,
                op: w.checkpoint.heads[0].clone(),
            }])
            .unwrap(),
            next: None,
        },
    );
    // All 4,096 bucket hashes make a 131 KB body: the case gives the
    // manifest's leaves and the encoded body's length and digest.
    let all = ManifestBucketsResponse {
        buckets: List(buckets),
    };
    let all_bytes = all.to_bytes();
    f.case(json!({
        "kind": "buckets_body",
        "name": "manifest_buckets_response",
        "leaves": [{"rid": w.rid.to_hex(), "refs": serde_json::to_value(w.checkpoint.heads.to_vec()).unwrap()}],
        "length": all_bytes.len(),
        "digest": sync_protocol::crypto::hash(&all_bytes).to_hex(),
    }));
    body(
        &mut f,
        "missing_blobs_response",
        &MissingBlobsResponse {
            blobs: List(vec![id("missing blob")]),
        },
    );
    let context = super::invitation::context(&w);
    body(
        &mut f,
        "create_invitation_request",
        &CreateInvitationRequest {
            invitation: id("invitation"),
            token_hash: invite_token_hash(&[4; 16]),
            role: Role::Editor,
            invite_context: Nested(context.clone()),
        },
    );
    body(
        &mut f,
        "expires_at_response",
        &ExpiresAtResponse {
            expires_at: 1_761_000_000,
        },
    );
    body(
        &mut f,
        "invitation_response",
        &InvitationResponse {
            workspace: w.workspace,
            invitation: id("invitation"),
            role: Role::Editor,
            status: InvitationStatus::Pending,
            invite_context: Nested(context.clone()),
        },
    );
    body(
        &mut f,
        "invitations_response",
        &InvitationsResponse {
            invitations: List(vec![InvitationInfo {
                invitation: id("invitation"),
                role: Role::Editor,
                status: InvitationStatus::Accepted,
                accepted_by: Some(w.bob_laptop.account),
                expires_at: 1_761_000_000,
                invite_context: Nested(context),
            }]),
        },
    );
    let share = super::share::share(&w);
    body(
        &mut f,
        "create_share_request",
        &CreateShareRequest {
            token_hash: share.body.token_hash,
            domain: w.ws_domain,
            expires_in_days: 30,
            share: Nested(share.clone()),
        },
    );
    body(
        &mut f,
        "shares_response",
        &SharesResponse {
            shares: List(vec![ShareInfo {
                token_hash: share.body.token_hash,
                domain: w.ws_domain,
                expires_at: 1_763_000_000,
            }]),
        },
    );
    body(
        &mut f,
        "share_body",
        &ShareBody {
            share: Nested(share),
        },
    );

    // Errors, their statuses and details.
    let errors: Vec<Value> = SERVER_ERRORS
        .iter()
        .map(|(code, status)| json!({"code": code, "status": status}))
        .collect();
    f.set("server_errors", json!(errors));
    f.set("client_errors", json!(CLIENT_ERRORS));
    f.set(
        "restore_generation_header",
        json!(RESTORE_GENERATION_HEADER),
    );
    f.set("media_type", json!(MEDIA_TYPE));
    let chain_conflict = ChainConflictDetail {
        domain: w.ws_domain,
        counter: 1,
        record_digest: w.op.digest(),
    };
    body(&mut f, "chain_conflict_detail", &chain_conflict);
    body(
        &mut f,
        "cursor_compacted_detail",
        &CursorCompactedDetail {
            retention_floor: 2,
            checkpoint_covered_seq: 6,
        },
    );
    body(
        &mut f,
        "push_failure_detail",
        &PushFailureDetail {
            record_index: 3,
            code: Text::new("chain_conflict").unwrap(),
            record_detail: Some(Bytes(chain_conflict.to_bytes())),
        },
    );
    body(
        &mut f,
        "device_removed_detail",
        &DeviceRemovedDetail {
            genesis: w.alice.links[0].digest(),
            seqno: 2,
        },
    );
    let error = |code: &str, retry: Option<u64>, detail: Option<Vec<u8>>| ErrorBody {
        code: Text::new(code).unwrap(),
        message: Text::new(format!("{code} example")).unwrap(),
        retry_after_seconds: retry,
        detail: detail.map(Bytes),
    };
    body(
        &mut f,
        "error_body_chain_conflict",
        &error("chain_conflict", None, Some(chain_conflict.to_bytes())),
    );
    body(
        &mut f,
        "error_body_rate_limited",
        &error("rate_limited", Some(30), None),
    );
    body(
        &mut f,
        "error_body_not_found_head",
        &error("not_found", None, None),
    );
    body(
        &mut f,
        "error_body_authorization_pending",
        &error("authorization_pending", None, None),
    );
    body(
        &mut f,
        "error_body_device_removed",
        &error(
            "device_removed",
            None,
            Some(
                DeviceRemovedDetail {
                    genesis: w.alice.links[0].digest(),
                    seqno: 2,
                }
                .to_bytes(),
            ),
        ),
    );

    // Pull pages for every status.
    for status in [
        PageStatus::Ok,
        PageStatus::Compacted,
        PageStatus::BeforeHistoryFloor,
        PageStatus::Forbidden,
        PageStatus::NotFound,
    ] {
        let p = page(&w, status);
        let name = format!(
            "page_{}",
            serde_json::to_value(status).unwrap().as_str().unwrap()
        );
        f.accept(&name, "page", &p, None, Shape::Body);
        f.case(json!({"kind": "page_shape", "name": name, "page": to_hex(&p.to_bytes()), "expect": "ok"}));
    }
    let mut bad = page(&w, PageStatus::Compacted);
    bad.entries = Sorted::new(vec![entry_op(&w, 6)]).unwrap();
    f.case(json!({"kind": "page_shape", "name": "compacted_page_with_entries", "page": to_hex(&bad.to_bytes()), "expect": "not_empty"}));
    let mut outside = page(&w, PageStatus::Ok);
    outside.complete_through = 5;
    f.case(json!({"kind": "page_shape", "name": "entry_outside_range", "page": to_hex(&outside.to_bytes()), "expect": "out_of_range"}));
    let mut status = page(&w, PageStatus::Ok).to_bytes();
    status[16] = 6;
    f.reject(
        "page_unknown_status",
        "page",
        &status,
        json!({"decode": "unknown_value"}),
    );

    // Every realtime frame.
    let presence = super::presence::presence(&w);
    let frames = vec![
        (
            "frame_notify",
            Frame::Notify {
                domain: w.ws_domain,
                head: 7,
                checkpoint_due: 0,
            },
        ),
        (
            "frame_presence",
            Frame::Presence {
                record: presence.clone(),
            },
        ),
        (
            "frame_presence_gone",
            Frame::PresenceGone {
                device: w.alice_laptop.device(),
                session: presence.body.session,
            },
        ),
        ("frame_policy_changed", Frame::PolicyChanged { revision: 3 }),
        ("frame_keys_changed", Frame::KeysChanged),
        (
            "frame_records",
            Frame::Records {
                domain: w.ws_domain,
                previous_seq: 5,
                checkpoint_due: 10_006,
                entries: Sorted::new(vec![entry_op(&w, 6)]).unwrap(),
            },
        ),
        (
            "frame_records_first_entry",
            Frame::Records {
                domain: w.ws_domain,
                previous_seq: 0,
                checkpoint_due: 0,
                entries: Sorted::new(vec![Entry {
                    seq: 1,
                    record: EntryRecord::Policy(PolicyEntry {
                        revision: 1,
                        head: w.policy.changes[0].digest(),
                    }),
                }])
                .unwrap(),
            },
        ),
        (
            "frame_subscribe",
            Frame::Subscribe {
                channels: List(vec![presence.body.channel]),
            },
        ),
        (
            "frame_restored",
            Frame::Restored {
                restore_generation: 1_760_000_000,
            },
        ),
        (
            "frame_error_presence",
            Frame::Error {
                rejected_type: 2,
                domain: IdOrZero(w.ws_domain.0),
                session: IdOrZero(presence.body.session.0),
                sequence: presence.body.sequence,
                code: Text::new("rotation_pending").unwrap(),
                detail: None,
            },
        ),
        (
            "frame_error_subscribe",
            Frame::Error {
                rejected_type: 7,
                domain: IdOrZero::ZERO,
                session: IdOrZero::ZERO,
                sequence: 0,
                code: Text::new("too_large").unwrap(),
                detail: None,
            },
        ),
        (
            "frame_account_changed",
            Frame::AccountChanged {
                account: w.alice_laptop.account,
                genesis: w.alice.links[0].digest(),
                seqno: 2,
            },
        ),
    ];
    for (name, frame) in frames {
        f.accept(name, "frame", &frame, None, Shape::Body);
    }
    let mut unknown_frame = Frame::KeysChanged.to_bytes();
    unknown_frame[0] = 11;
    f.reject(
        "frame_unknown_type",
        "frame",
        &unknown_frame,
        json!({"decode": "unknown_value"}),
    );

    // What a client does with a records frame for each cursor position.
    let (previous_seq, first, last) = (5u64, 6u64, 8u64);
    for (cursor, action) in [
        (3, "pull"),
        (5, "apply"),
        (6, "pull"),
        (7, "pull"),
        (8, "ignore"),
        (9, "ignore"),
    ] {
        f.case(json!({
            "kind": "records_frame_action",
            "cursor": cursor,
            "previous_seq": previous_seq,
            "first": first,
            "last": last,
            "expect": action,
        }));
    }
    f.case(json!({"kind": "records_frame_action", "cursor": 0, "previous_seq": 0, "first": 1, "last": 1, "expect": "apply"}));

    // What an approval request asks for (§10, §26): it names at most one
    // of an invitation and a workspace, else `bad_request`, and only the
    // account's own new device's request, which names neither and carries
    // no account head, reaches `GET /v2/approval-requests`.
    let head = Some(w.bob.head());
    let request = |account: Option<AccountHead>,
                   invitation: Option<Id>,
                   invite_proof: Option<Digest>,
                   workspace: Option<Id>| ApprovalRequest {
        commit: Digest([1; 32]),
        account,
        invitation,
        invite_proof,
        workspace,
    };
    let proof = Some(Digest([2; 32]));
    for (name, body, expect) in [
        (
            "the_accounts_own_new_device",
            request(None, None, None, None),
            json!({"target": "own_device", "listed_for_own_account": true}),
        ),
        (
            "an_invited_device",
            request(head.clone(), Some(id("invitation")), proof, None),
            json!({"target": "invitation", "listed_for_own_account": false}),
        ),
        (
            "a_reset_device",
            request(head.clone(), None, None, Some(id("reset workspace"))),
            json!({"target": "reset", "listed_for_own_account": false}),
        ),
        (
            "an_invitation_and_a_workspace",
            request(
                head.clone(),
                Some(id("invitation")),
                proof,
                Some(id("reset workspace")),
            ),
            json!({"error": "bad_request"}),
        ),
        (
            "neither_with_an_account_head",
            request(head.clone(), None, None, None),
            json!({"error": "bad_request"}),
        ),
        (
            "a_workspace_without_an_account_head",
            request(None, None, None, Some(id("reset workspace"))),
            json!({"error": "bad_request"}),
        ),
        (
            "an_invitation_without_its_proof",
            request(head.clone(), Some(id("invitation")), None, None),
            json!({"error": "bad_request"}),
        ),
    ] {
        f.case(json!({
            "kind": "approval_request_target",
            "name": name,
            "request": to_hex(&body.to_bytes()),
            "expect": expect,
        }));
    }
    let _ = Id::ZERO;
    f
}
