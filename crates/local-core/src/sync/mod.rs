//! Native encrypted sync. Secret material is never exposed through an IPC DTO.

mod access;
mod account;
mod activation;
mod approvals;
pub(crate) mod blobs;
mod checkpoints;
pub mod collaboration;
mod conflicts;
pub use checkpoints::{
    AccessTransition, CheckpointBlobManifest, CheckpointContent, EncryptedCheckpoint,
};
mod capability;
pub use capability::WorkspaceCapability;
mod coordinator;
mod crypto;
pub(crate) use crypto::decode;
mod keys;
mod presence;
mod recovery;
mod signin;
mod transport;
pub use access::{
    AccessMember, AccessObject, AccessPolicy, DocumentDescriptor, DocumentMode, ObjectGrant,
    ObjectRole, PolicyEnvelope, WorkspaceRole,
};
pub use account::{SyncAccount, SyncAccountPoll, SyncAccountService};
pub use activation::ObjectActivation;
pub use approvals::{
    SyncDevice, SyncInvitation, SyncInvitationLink, SyncInvitationRole, SyncInvitationStatus,
    device_fingerprint,
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
pub use blobs::EncryptedBlob;
pub use collaboration::CollaborationConflictReview;
pub use conflicts::{ResolveSyncConflict, SyncConflict, SyncResolutionChoice};
pub use coordinator::{
    RemoteSyncWorkspace, WorkspaceSyncActivationStatus, WorkspaceSyncConfig,
    WorkspaceSyncCoordinator, WorkspaceSyncPhase, WorkspaceSyncStatus,
    WorkspaceSyncTransitionObject, WorkspaceSyncTransitionPhase, WorkspaceSyncTransitionStatus,
};
pub use crypto::{EncryptedOperation, ObjectKey, OperationKind, SigningIdentity};
pub use keys::{DeviceKeys, KeyConstruction, KeyEnvelope, OsSyncCredentials, SyncCredentials};
pub use presence::{
    CollaborationPresenceEvent, CollaborationPresenceInput, CollaborationPresenceMember,
    EncryptedPresence, PresenceContext, PresenceSelection,
};
pub use recovery::{
    BROWSER_RECOVERY_FORMAT, BROWSER_RECOVERY_KDF, BROWSER_RECOVERY_MAX_CIPHERTEXT,
    BROWSER_RECOVERY_MIN_ITERATIONS, BROWSER_RECOVERY_VERSION, BrowserRecoveredObject,
    BrowserRecoveryImport, import_browser_recovery_kit,
};
use serde::{Deserialize, Serialize};
pub use signin::{DeviceConnection, DeviceSignIn, DeviceSignInInfo, DeviceSignInStatus};
pub use transport::{HttpSyncTransport, SyncPass, SyncSecrets};

/// Plaintext file change, visible only on an authorized native client.
/// `None` content is a tombstone; `None` base revision requires an absent file.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FileChange {
    pub version: u8,
    pub path: String,
    pub previous_path: Option<String>,
    pub base_revision: Option<String>,
    pub content: Option<String>,
    /// Version 2 resolution records accept only these explicitly reviewed branch revisions.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_accepted_revisions"
    )]
    pub accepted_revisions: Option<Vec<Option<String>>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_blob"
    )]
    pub blob: Option<EncryptedBlob>,
}

fn deserialize_blob<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<EncryptedBlob>, D::Error> {
    EncryptedBlob::deserialize(deserializer).map(Some)
}

fn deserialize_accepted_revisions<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<Option<Vec<Option<String>>>, D::Error> {
    Vec::<Option<String>>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ApplyOutcome {
    Applied,
    Conflict,
    Resolved,
}

use crate::{CoreError, Result};

pub(crate) fn invalid(code: &str) -> CoreError {
    CoreError::validation(code, "The sync data failed validation", "sync")
}

pub(crate) fn identifier(value: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'_' || c == b'-')
    {
        return Err(invalid("sync_invalid_identifier"));
    }
    Ok(())
}

pub fn validate_file_change(change: &FileChange) -> Result<()> {
    if !matches!(change.version, 1..=3)
        || (change.version != 3 && change.blob.is_some())
        || (change.version == 3
            && (change.blob.is_none()
                || change.content.is_some()
                || change.accepted_revisions.is_some()))
        || (change.version == 1 && change.accepted_revisions.is_some())
        || (change.version == 2
            && (change.previous_path.is_some()
                || change
                    .accepted_revisions
                    .as_ref()
                    .is_none_or(|revisions| revisions.is_empty() || revisions.len() > 2)))
    {
        return Err(invalid("sync_invalid_change"));
    }
    if let Some(blob) = &change.blob {
        blob.validate()?;
    }
    if let Some(revisions) = &change.accepted_revisions {
        if !revisions.contains(&change.base_revision)
            || (revisions.len() == 2 && revisions[0] == revisions[1])
        {
            return Err(invalid("sync_invalid_change"));
        }
        for revision in revisions.iter().flatten() {
            if revision.len() != 64
                || !revision
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            {
                return Err(invalid("sync_invalid_revision"));
            }
        }
    }
    for path in std::iter::once(&change.path).chain(change.previous_path.iter()) {
        if path.len() > 4096
            || path
                .chars()
                .any(|c| c.is_ascii_control() || "\\:<>\"|?*".contains(c))
            || path.split('/').any(|part| {
                let stem = part
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .trim_matches(' ')
                    .to_ascii_uppercase();
                part.is_empty()
                    || part == "."
                    || part == ".."
                    || part.ends_with(['.', ' '])
                    || matches!(
                        stem.as_str(),
                        "CON"
                            | "CONIN$"
                            | "CONOUT$"
                            | "PRN"
                            | "AUX"
                            | "NUL"
                            | "COM1"
                            | "COM2"
                            | "COM3"
                            | "COM4"
                            | "COM5"
                            | "COM6"
                            | "COM7"
                            | "COM8"
                            | "COM9"
                            | "COM¹"
                            | "COM²"
                            | "COM³"
                            | "LPT1"
                            | "LPT2"
                            | "LPT3"
                            | "LPT4"
                            | "LPT5"
                            | "LPT6"
                            | "LPT7"
                            | "LPT8"
                            | "LPT9"
                            | "LPT¹"
                            | "LPT²"
                            | "LPT³"
                    )
            })
            || matches!(
                path.split('/')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase()
                    .as_str(),
                ".noura" | ".git" | "node_modules" | "target"
            )
        {
            return Err(invalid("sync_unsafe_path"));
        }
        crate::path::validate_relative(path, "sync")?;
    }
    if change.previous_path.as_ref() == Some(&change.path)
        || (change.previous_path.is_some() && change.content.is_none() && change.blob.is_none())
        || change.base_revision.as_ref().is_some_and(|rev| {
            rev.len() != 64
                || !rev
                    .bytes()
                    .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
        })
    {
        return Err(invalid("sync_invalid_change"));
    }
    if let Some(content) = &change.content {
        let decoded = STANDARD
            .decode(content)
            .map_err(|_| invalid("sync_invalid_content"))?;
        if base64::engine::general_purpose::STANDARD.encode(decoded) != *content {
            return Err(invalid("sync_invalid_content"));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn file_changes_match_shared_conformance_and_canonical_bytes() {
        let fixtures: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/sync-v1.json"
        ))
        .unwrap();
        for fixture in fixtures["valid"].as_array().unwrap() {
            let change: FileChange = serde_json::from_value(fixture["input"].clone()).unwrap();
            validate_file_change(&change).unwrap();
            assert_eq!(
                serde_json::to_string(&change).unwrap(),
                fixture["canonical"].as_str().unwrap()
            );
        }
        for fixture in fixtures["invalid"].as_array().unwrap() {
            let result = serde_json::from_value::<FileChange>(fixture.clone());
            assert!(
                result.is_err() || validate_file_change(&result.unwrap()).is_err(),
                "{fixture}"
            );
        }
    }
}
