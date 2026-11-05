//! Property tests over every decoder (§4): random bytes, random bytes after
//! a valid tag, and random mutations of the fixtures' accepted records never
//! panic, and whatever a decoder accepts re-encodes to exactly its input.

use std::path::PathBuf;
use std::sync::OnceLock;

use proptest::prelude::*;
use serde_json::Value;
use sync_protocol::codec::{Decode, Encode};
use sync_protocol::messages::*;
use sync_protocol::records::*;
use sync_protocol::tag::Tag;
use sync_protocol::types::from_hex;

fn roundtrip<T: Decode + Encode>(bytes: &[u8]) -> Result<(), String> {
    match T::from_bytes(bytes) {
        Ok(value) if value.to_bytes() != bytes => {
            Err("accepted bytes re-encode differently".into())
        }
        _ => Ok(()),
    }
}

macro_rules! types {
    ($($name:literal => $ty:ty),* $(,)?) => {
        const TYPES: &[&str] = &[$($name),*];

        fn check(name: &str, bytes: &[u8]) -> Result<(), String> {
            match name {
                $($name => roundtrip::<$ty>(bytes),)*
                other => Err(format!("unknown type {other}")),
            }
        }
    };
}

types! {
    "card" => Card,
    "enroll" => Signed<Enroll>,
    "account_head" => AccountHead,
    "account_link" => Signed<AccountLink>,
    "policy_change" => Signed<PolicyChange>,
    "epoch" => Signed<Epoch>,
    "epoch_bundle" => EpochBundle,
    "envelope" => Signed<Envelope>,
    "operation" => Signed<Operation>,
    "payload" => Payload,
    "checkpoint" => Signed<Checkpoint>,
    "checkpoint_bundle" => CheckpointBundle,
    "offer" => Signed<Offer>,
    "invite_context" => InviteContext,
    "invite_payload" => InvitePayload,
    "recovery_kit" => RecoveryKit,
    "kit_payload" => KitPayload,
    "recovery_proof" => Signed<RecoveryProof>,
    "presence" => Signed<Presence>,
    "presence_payload" => PresencePayload,
    "profile" => Signed<Profile>,
    "profile_payload" => ProfilePayload,
    "share" => Signed<Share>,
    "share_payload" => SharePayload,
    "record_ref" => RecordRef,
    "entry" => Entry,
    "policy_entry" => PolicyEntry,
    "receipt" => Receipt,
    "page" => Page,
    "frame" => Frame,
    "limits_response" => LimitsResponse,
    "device_authorize_response" => DeviceAuthorizeResponse,
    "device_token_request" => DeviceTokenRequest,
    "device_token_response" => DeviceTokenResponse,
    "challenge_response" => ChallengeResponse,
    "enroll_request" => EnrollRequest,
    "devices_response" => DevicesResponse,
    "chain_append_request" => ChainAppendRequest,
    "chain_append_response" => ChainAppendResponse,
    "chains_request" => ChainsRequest,
    "chains_response" => ChainsResponse,
    "reset_request" => ResetRequest,
    "approval_request" => ApprovalRequest,
    "pending_response" => PendingResponse,
    "offer_request" => OfferRequest,
    "offers_response" => OffersResponse,
    "reveal_body" => RevealBody,
    "recovery_request" => RecoveryRequest,
    "workspaces_response" => WorkspacesResponse,
    "push_request" => PushRequest,
    "push_response" => PushResponse,
    "policy_response" => PolicyResponse,
    "keys_response" => KeysResponse,
    "keys_current_response" => KeysResponse,
    "epochs_response" => EpochsResponse,
    "rewrap_body" => RewrapBody,
    "pull_request" => PullRequest,
    "pull_response" => PullResponse,
    "head_response" => HeadResponse,
    "checkpoint_response" => CheckpointResponse,
    "checkpoint_chain_response" => CheckpointChainResponse,
    "live_response" => LiveResponse,
    "manifest_node_response" => ManifestNodeResponse,
    "manifest_leaves_response" => ManifestLeavesResponse,
    "manifest_buckets_response" => ManifestBucketsResponse,
    "missing_blobs_response" => MissingBlobsResponse,
    "create_invitation_request" => CreateInvitationRequest,
    "expires_at_response" => ExpiresAtResponse,
    "invitation_response" => InvitationResponse,
    "invitations_response" => InvitationsResponse,
    "create_share_request" => CreateShareRequest,
    "shares_response" => SharesResponse,
    "share_body" => ShareBody,
    "error_body" => ErrorBody,
    "chain_conflict_detail" => ChainConflictDetail,
    "cursor_compacted_detail" => CursorCompactedDetail,
    "push_failure_detail" => PushFailureDetail,
    "device_removed_detail" => DeviceRemovedDetail,
    "live_cursor" => LiveCursor,
    "meta" => Meta,
    "access" => Access,
    "anchor" => Anchor,
    "ref" => Ref,
}

/// The accepted records of every fixture, as seeds: (type, bytes).
fn seeds() -> &'static Vec<(String, Vec<u8>)> {
    static SEEDS: OnceLock<Vec<(String, Vec<u8>)>> = OnceLock::new();
    SEEDS.get_or_init(|| {
        let dir =
            PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/workspace-format/fixtures");
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("sync-v2-") {
                continue;
            }
            let doc: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
            for record in doc["records"].as_array().into_iter().flatten() {
                if record["expect"] != "ok" {
                    continue;
                }
                let ty = record["type"].as_str().unwrap();
                let ty = if ty.starts_with("error_body") {
                    "error_body"
                } else {
                    ty
                };
                if TYPES.contains(&ty) {
                    out.push((
                        ty.to_owned(),
                        from_hex(record["hex"].as_str().unwrap()).unwrap(),
                    ));
                }
            }
        }
        assert!(out.len() > 100, "{}", out.len());
        out
    })
}

#[test]
fn every_seed_round_trips() {
    for (ty, bytes) in seeds() {
        check(ty, bytes).unwrap();
        // The seed itself decodes.
        assert!(matches!(check(ty, bytes), Ok(())), "{ty}");
    }
}

#[derive(Clone, Debug)]
enum Mutation {
    Flip(usize, u8),
    Truncate(usize),
    Insert(usize, u8),
    Remove(usize),
    SetLength(usize, u32),
}

fn mutation() -> impl Strategy<Value = Mutation> {
    prop_oneof![
        (any::<usize>(), 1u8..).prop_map(|(i, b)| Mutation::Flip(i, b)),
        any::<usize>().prop_map(Mutation::Truncate),
        (any::<usize>(), any::<u8>()).prop_map(|(i, b)| Mutation::Insert(i, b)),
        any::<usize>().prop_map(Mutation::Remove),
        (any::<usize>(), any::<u32>()).prop_map(|(i, v)| Mutation::SetLength(i, v)),
    ]
}

fn apply(bytes: &mut Vec<u8>, m: &Mutation) {
    if bytes.is_empty() {
        return;
    }
    let len = bytes.len();
    match *m {
        Mutation::Flip(i, b) => bytes[i % len] ^= b,
        Mutation::Truncate(i) => bytes.truncate(i % len),
        Mutation::Insert(i, b) => bytes.insert(i % (len + 1), b),
        Mutation::Remove(i) => {
            bytes.remove(i % len);
        }
        Mutation::SetLength(i, v) => {
            let at = i % len;
            let end = (at + 4).min(len);
            bytes[at..end].copy_from_slice(&v.to_be_bytes()[..end - at]);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2_000))]

    #[test]
    fn random_bytes_never_panic(ty in prop::sample::select(TYPES), bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        prop_assert!(check(ty, &bytes).is_ok());
    }

    #[test]
    fn random_bytes_after_a_valid_tag_never_panic(
        ty in prop::sample::select(TYPES),
        tag in prop::sample::select(Tag::ALL),
        rest in prop::collection::vec(any::<u8>(), 0..512),
    ) {
        let mut bytes = tag.bytes();
        bytes.extend_from_slice(&rest);
        prop_assert!(check(ty, &bytes).is_ok());
    }

    #[test]
    fn mutated_records_never_panic(
        pick in any::<prop::sample::Index>(),
        mutations in prop::collection::vec(mutation(), 1..4),
    ) {
        let seeds = seeds();
        let (ty, seed) = &seeds[pick.index(seeds.len())];
        let mut bytes = seed.clone();
        for m in &mutations {
            apply(&mut bytes, m);
        }
        prop_assert!(check(ty, &bytes).is_ok(), "{} {:?}", ty, mutations);
    }
}
