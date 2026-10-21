//! Runs the protocol v2 fixtures of sync-v2.md §32 from
//! docs/workspace-format/fixtures: every record decodes, re-encodes byte for
//! byte from its fields, verifies under its signer and re-signs to the same
//! bytes, every rejected variant fails as the file says, and each case's
//! verifier reaches the stated outcome.

use std::collections::BTreeMap;
use std::fmt::Debug;
use std::path::PathBuf;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sync_protocol::codec::{Decode, Encode};
use sync_protocol::crypto::SigningKey;
use sync_protocol::messages::*;
use sync_protocol::records::*;
use sync_protocol::types::{Digest, Id, Key, SecretKey, from_hex, to_hex};

/// Every fixture file §32 names.
const FILES: &[&str] = &[
    "sync-v2-encoding.json",
    "sync-v2-padding.json",
    "sync-v2-ids.json",
    "sync-v2-device.json",
    "sync-v2-account.json",
    "sync-v2-approval.json",
    "sync-v2-policy.json",
    "sync-v2-keys.json",
    "sync-v2-operation.json",
    "sync-v2-chain.json",
    "sync-v2-live.json",
    "sync-v2-domains.json",
    "sync-v2-manifest.json",
    "sync-v2-blob.json",
    "sync-v2-collaboration.json",
    "sync-v2-presence.json",
    "sync-v2-profile.json",
    "sync-v2-invitation.json",
    "sync-v2-share.json",
    "sync-v2-recovery.json",
];

fn directory() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/workspace-format/fixtures")
}

fn load(file: &str) -> Value {
    let text =
        std::fs::read_to_string(directory().join(file)).unwrap_or_else(|e| panic!("{file}: {e}"));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{file}: {e}"))
}

pub fn hex(value: &Value) -> Vec<u8> {
    from_hex(
        value
            .as_str()
            .unwrap_or_else(|| panic!("not a string: {value}")),
    )
    .unwrap_or_else(|| panic!("not hexadecimal: {value}"))
}

fn digest(value: &Value) -> Digest {
    Digest(hex(value).try_into().unwrap())
}

fn id(value: &Value) -> Id {
    Id(hex(value).try_into().unwrap())
}

fn secret(value: &Value) -> SecretKey {
    SecretKey(hex(value).try_into().unwrap())
}

struct Keys(BTreeMap<String, Value>);

impl Keys {
    fn public(&self, name: &str) -> Key {
        Key(hex(&self.0[name]["signing_public"]).try_into().unwrap())
    }

    fn signing(&self, name: &str) -> SigningKey {
        SigningKey::from_seed(&secret(&self.0[name]["signing_seed"]))
    }
}

fn mutate(base: &[u8], mutation: &Value) -> Vec<u8> {
    let mut bytes = base.to_vec();
    if let Some(n) = mutation.get("truncate") {
        bytes.truncate(bytes.len() - n.as_u64().unwrap() as usize);
    }
    if let Some(extra) = mutation.get("append") {
        bytes.extend_from_slice(&hex(extra));
    }
    if let Some(tag) = mutation.get("retag") {
        let len = usize::from(bytes[0]) + 3;
        let mut out = hex(tag);
        out.extend_from_slice(&bytes[len..]);
        bytes = out;
    }
    if let Some(version) = mutation.get("version") {
        let len = usize::from(bytes[0]) + 3;
        bytes[len - 2..len].copy_from_slice(&(version.as_u64().unwrap() as u16).to_be_bytes());
    }
    if mutation.get("flip_last_byte").is_some() {
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
    }
    bytes
}

/// Checks one record of a plain type.
fn plain<T>(record: &Value, bytes: &[u8]) -> Result<Option<T>, String>
where
    T: Decode + Encode + Serialize + DeserializeOwned + PartialEq + Debug,
{
    let expect = &record["expect"];
    let decoded = T::from_bytes(bytes);
    if let Some(kind) = expect.get("decode") {
        return match decoded {
            Err(e) if e.kind.name() == kind.as_str().unwrap() => Ok(None),
            other => Err(format!("expected decode error {kind}, got {other:?}")),
        };
    }
    let value = decoded.map_err(|e| format!("decode failed: {e}"))?;
    if value.to_bytes() != bytes {
        return Err("re-encoding differs".into());
    }
    if let Some(fields) = record.get("value") {
        let ours = serde_json::to_value(&value).unwrap();
        if &ours != fields {
            return Err(format!("fields differ:\n ours {ours}\n file {fields}"));
        }
        let rebuilt: T = serde_json::from_value(fields.clone()).map_err(|e| e.to_string())?;
        if rebuilt.to_bytes() != bytes {
            return Err("encoding from the fields differs".into());
        }
    }
    if record
        .get("digest")
        .is_some_and(|d| sync_protocol::crypto::hash(bytes) != digest(d))
    {
        return Err("digest differs".into());
    }
    Ok(Some(value))
}

/// Checks one signed record: everything `plain` checks, then the
/// signature under the signer's key, and the signature the signer's seed
/// makes over the body.
fn signed<B>(record: &Value, bytes: &[u8], keys: &Keys) -> Result<(), String>
where
    B: Decode + Encode + Serialize + DeserializeOwned + PartialEq + Debug + Clone,
{
    let expect = &record["expect"];
    if expect.get("verify").is_some() {
        let value = Signed::<B>::from_bytes(bytes).map_err(|e| format!("decode failed: {e}"))?;
        let signer = record["signer"].as_str().unwrap();
        return match value.verify(&keys.public(signer)) {
            Err(_) => Ok(()),
            Ok(()) => Err("a bad signature verified".into()),
        };
    }
    let Some(value) = plain::<Signed<B>>(record, bytes)? else {
        return Ok(());
    };
    if let Some(signer) = record.get("signer").and_then(Value::as_str) {
        value
            .verify(&keys.public(signer))
            .map_err(|e| format!("signature: {e}"))?;
        let resigned = Signed::sign(value.body.clone(), &keys.signing(signer));
        if resigned.to_bytes() != bytes {
            return Err("signing the body again differs".into());
        }
    }
    Ok(())
}

macro_rules! dispatch {
    ($ty:expr, $record:expr, $bytes:expr, $keys:expr;
     plain: [$($pname:literal => $pty:ty),* $(,)?];
     signed: [$($sname:literal => $sty:ty),* $(,)?] $(;)?) => {
        match $ty {
            $($pname => plain::<$pty>($record, $bytes).map(|_| ()),)*
            $($sname => signed::<$sty>($record, $bytes, $keys),)*
            other => Err(format!("unknown record type {other}")),
        }
    };
}

fn run_record(record: &Value, bytes: &[u8], keys: &Keys) -> Result<(), String> {
    let ty = record["type"].as_str().unwrap();
    dispatch!(ty, record, bytes, keys;
        plain: [
            "card" => Card,
            "account_head" => AccountHead,
            "epoch_bundle" => EpochBundle,
            "checkpoint_bundle" => CheckpointBundle,
            "ref" => Ref,
            "payload" => Payload,
            "invite_payload" => InvitePayload,
            "invite_context" => InviteContext,
            "kit_payload" => KitPayload,
            "recovery_kit" => RecoveryKit,
            "share_payload" => SharePayload,
            "presence_payload" => PresencePayload,
            "profile_payload" => ProfilePayload,
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
            "chain_conflict_detail" => ChainConflictDetail,
            "cursor_compacted_detail" => CursorCompactedDetail,
            "push_failure_detail" => PushFailureDetail,
            "device_removed_detail" => DeviceRemovedDetail,
            "error_body_chain_conflict" => ErrorBody,
            "error_body_rate_limited" => ErrorBody,
            "error_body_not_found_head" => ErrorBody,
            "error_body_authorization_pending" => ErrorBody,
            "error_body_device_removed" => ErrorBody,
        ];
        signed: [
            "enroll" => Enroll,
            "account_link" => AccountLink,
            "policy_change" => PolicyChange,
            "epoch" => Epoch,
            "envelope" => Envelope,
            "operation" => Operation,
            "checkpoint" => Checkpoint,
            "offer" => Offer,
            "recovery_proof" => RecoveryProof,
            "presence" => Presence,
            "profile" => Profile,
            "share" => Share,
        ];
    )
}

mod behavior;
mod cases;

#[test]
fn every_fixture_file_section_32_names_exists() {
    let missing: Vec<&str> = FILES
        .iter()
        .filter(|f| !directory().join(f).exists())
        .copied()
        .collect();
    assert!(missing.is_empty(), "missing fixtures: {missing:?}");
}

#[test]
fn every_record_passes_or_fails_as_its_fixture_says() {
    let mut failures = Vec::new();
    let mut count = 0;
    for file in FILES {
        if !directory().join(file).exists() {
            continue;
        }
        let doc = load(file);
        let keys = Keys(
            doc["keys"]
                .as_object()
                .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
                .unwrap_or_default(),
        );
        let records = doc["records"].as_array().cloned().unwrap_or_default();
        let by_name: BTreeMap<&str, &Value> = records
            .iter()
            .map(|r| (r["name"].as_str().unwrap(), r))
            .collect();
        for record in &records {
            let bytes = match record.get("from") {
                Some(base) => mutate(
                    &hex(&by_name[base.as_str().unwrap()]["hex"]),
                    &record["mutation"],
                ),
                None => hex(&record["hex"]),
            };
            count += 1;
            if let Err(e) = run_record(record, &bytes, &keys) {
                failures.push(format!("{file} {}: {e}", record["name"]));
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(count > 100, "{count}");
}

#[test]
fn every_case_reaches_its_stated_outcome() {
    let mut failures = Vec::new();
    for file in FILES {
        if !directory().join(file).exists() {
            continue;
        }
        let doc = load(file);
        for case in doc["cases"].as_array().cloned().unwrap_or_default() {
            if let Err(e) = cases::run(file, &doc, &case) {
                failures.push(format!("{file} {} ({}): {e}", case["name"], case["kind"]));
            }
        }
        if let Err(e) = cases::run_document(file, &doc) {
            failures.push(format!("{file}: {e}"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} failures:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

pub fn to_hex_string(bytes: &[u8]) -> String {
    to_hex(bytes)
}

pub fn id_of(value: &Value) -> Id {
    id(value)
}

pub fn digest_of(value: &Value) -> Digest {
    digest(value)
}

pub fn secret_of(value: &Value) -> SecretKey {
    secret(value)
}
