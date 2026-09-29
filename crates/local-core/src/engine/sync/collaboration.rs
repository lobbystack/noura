use super::blobs::file_revision;
use super::*;
use crate::sync::{
    DocumentMode, OperationKind, SyncSecrets,
    collaboration::{
        CollaborationBootstrapRole, CollaborationOpenInput, CollaborationReceipt,
        CollaborationSession, CollaborationStatus, CollaborationStatusEvent,
        CollaborationSubmitInput, CollaborativeChange, MAX_DOCUMENT_BYTES, MAX_TEXT_BYTES,
        TextDocument,
    },
};
use std::io::{Read, Seek, Write};

mod checkpoints;
mod content;
mod external;
mod objects;
mod presence;
mod recovery;
mod sessions;
mod transactions;

use content::*;

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DocumentState {
    version: u8,
    object_id: String,
    generation: String,
    covered_sequence: String,
    path: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deleted: bool,
    revision: String,
    materialized: String,
    update: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    acknowledged: Option<DocumentSnapshot>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pending: Vec<PendingDocumentChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    acknowledged_operations: Vec<String>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DocumentSnapshot {
    sequence: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    path: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    deleted: bool,
    revision: String,
    materialized: String,
    update: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingDocumentChange {
    operation_id: String,
    updates: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<CollaborativeMetadataPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    format: Option<CollaborativeFormatPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lifecycle: Option<CollaborativeLifecycleChange>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase", deny_unknown_fields)]
enum CollaborativeLifecycleChange {
    Move {
        from: String,
        to: String,
        expected_revision: String,
    },
    Delete {
        path: String,
        expected_revision: String,
        tombstone_id: String,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborativeTextFormat {
    has_bom: bool,
    uses_crlf: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborativeFormatPatch {
    expected: CollaborativeTextFormat,
    value: CollaborativeTextFormat,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "lowercase", deny_unknown_fields)]
enum CollaborativeMetadataValue {
    Missing,
    Value { value: serde_json::Value },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborativeMetadataPatch {
    field: String,
    expected: CollaborativeMetadataValue,
    value: CollaborativeMetadataValue,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborativeTransaction {
    version: u8,
    object_id: String,
    generation: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    updates: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    metadata: Vec<CollaborativeMetadataPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    format: Option<CollaborativeFormatPatch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    lifecycle: Option<CollaborativeLifecycleChange>,
}

impl CollaborativeTransaction {
    fn validate(&self) -> Result<()> {
        crate::sync::identifier(&self.object_id)?;
        crate::sync::identifier(&self.generation)?;
        if self.version != 1
            || (self.updates.is_empty()
                && self.metadata.is_empty()
                && self.format.is_none()
                && self.lifecycle.is_none())
            || self.metadata.len() > 256
            || self
                .format
                .as_ref()
                .is_some_and(|patch| patch.expected == patch.value)
        {
            return Err(invalid("collaboration_invalid_transaction"));
        }
        if !self.updates.is_empty() {
            crate::sync::collaboration::validate_update_batch(&self.updates)?;
        }
        let mut fields = BTreeSet::new();
        for patch in &self.metadata {
            validate_metadata_field(&patch.field)?;
            if !fields.insert(&patch.field) || patch.expected == patch.value {
                return Err(invalid("collaboration_invalid_metadata_patch"));
            }
        }
        if let Some(lifecycle) = &self.lifecycle {
            match lifecycle {
                CollaborativeLifecycleChange::Move {
                    from,
                    to,
                    expected_revision,
                } if from == to || expected_revision.is_empty() => {
                    return Err(invalid("collaboration_invalid_lifecycle"));
                }
                CollaborativeLifecycleChange::Delete {
                    path,
                    expected_revision,
                    tombstone_id,
                } => {
                    crate::sync::identifier(tombstone_id)?;
                    if path.is_empty() || expected_revision.is_empty() {
                        return Err(invalid("collaboration_invalid_lifecycle"));
                    }
                }
                _ => {}
            }
        }
        if self.lifecycle.is_some()
            && (!self.updates.is_empty() || !self.metadata.is_empty() || self.format.is_some())
        {
            return Err(invalid("collaboration_invalid_lifecycle"));
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DocumentIntent {
    version: u8,
    base_revision: String,
    next: DocumentState,
    operation: EncryptedOperation,
    outgoing: bool,
    batch_id: Option<String>,
    batch_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    external_snapshot: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    previous_path: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GenerationRecoveryRecord {
    version: u8,
    transition_id: String,
    state: Option<DocumentState>,
    pending_operations: Vec<EncryptedOperation>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    rebased_operation: Option<EncryptedOperation>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborationReviewRecord {
    version: u8,
    transition_id: String,
    object_id: String,
    reason: String,
    prior: Option<DocumentState>,
    checkpoint: crate::sync::EncryptedCheckpoint,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    competing_bytes: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborationMetadataReviewRecord {
    version: u8,
    operation_id: String,
    object_id: String,
    generation: String,
    reason: String,
    operation: EncryptedOperation,
    retained: DocumentState,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborationExternalReviewRecord {
    version: u8,
    review_id: String,
    object_id: String,
    generation: String,
    reason: String,
    retained: DocumentState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    competing_bytes: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborationExternalSnapshot {
    version: u8,
    object_id: String,
    generation: String,
    revision: String,
    bytes: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CollaborationBlobReference {
    version: u8,
    blob: crate::sync::EncryptedBlob,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttachmentGenerationState {
    version: u8,
    object_id: String,
    generation: String,
    path: String,
    revision: String,
    checkpoint_digest: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AttachmentTransitionRecovery {
    version: u8,
    transition_id: String,
    checkpoint: crate::sync::EncryptedCheckpoint,
    pending_operations: Vec<EncryptedOperation>,
}

fn state_path(object: &str) -> Result<String> {
    crate::sync::identifier(object)?;
    Ok(format!(".noura/sync/collaboration/{object}/state.json"))
}

fn intent_path(object: &str) -> Result<String> {
    crate::sync::identifier(object)?;
    Ok(format!(".noura/sync/collaboration/{object}/intent.json"))
}

fn collaboration_trash_path(object: &str, tombstone: &str, path: &str) -> Result<String> {
    crate::sync::identifier(object)?;
    crate::sync::identifier(tombstone)?;
    if path.is_empty() {
        return Err(invalid("collaboration_invalid_lifecycle"));
    }
    Ok(format!(
        ".noura/trash/collaboration/{object}/{tombstone}/{path}"
    ))
}

impl WorkspaceEngine {
    #[cfg(test)]
    fn collaboration_set_mutation_fault(&self, boundary: u8) {
        *self
            .collaboration_mutation_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(boundary);
    }

    fn collaboration_mutation_boundary(&self, boundary: u8) -> Result<()> {
        #[cfg(test)]
        {
            let mut fault = self
                .collaboration_mutation_fault
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if fault.as_ref() == Some(&boundary) {
                *fault = None;
                return Err(invalid("collaboration_test_interrupted"));
            }
        }
        let _ = boundary;
        Ok(())
    }

    fn collaboration_seal_operation(
        &self,
        key: &ObjectKey,
        workspace: &str,
        object: &str,
        device: &DeviceKeys,
        authorization: (u64, &str, &str, OperationKind),
        plaintext: &[u8],
    ) -> Result<EncryptedOperation> {
        let payload = if plaintext.len() <= 700 * 1024 {
            zeroize::Zeroizing::new(plaintext.to_vec())
        } else {
            if plaintext.len() > MAX_DOCUMENT_BYTES * 2 {
                return Err(invalid("collaboration_update_limit"));
            }
            let temporary = format!(".noura/sync/blobs/pending_{}", uuid::Uuid::new_v4());
            self.sync_prepare_parent(&temporary)?;
            let temporary_path = self.sync_path(&temporary)?;
            let mut output = atomic_write_file::AtomicWriteFile::open(&temporary_path)
                .map_err(|_| invalid("sync_blob_write_failed"))?;
            let blob = crate::sync::EncryptedBlob::encrypt(
                key,
                std::io::Cursor::new(plaintext),
                &mut output,
            )?;
            output
                .sync_all()
                .map_err(|_| invalid("sync_blob_write_failed"))?;
            output
                .commit()
                .map_err(|_| invalid("sync_blob_write_failed"))?;
            let final_path = self.sync_path(&format!(".noura/sync/blobs/{}", blob.id))?;
            std::fs::rename(&temporary_path, &final_path)
                .map_err(|_| invalid("sync_blob_write_failed"))?;
            sync_parent(&final_path, "collaboration_operation_blob")?;
            zeroize::Zeroizing::new(
                serde_json::to_vec(&CollaborationBlobReference { version: 1, blob })
                    .map_err(|_| invalid("sync_serialize_failed"))?,
            )
        };
        device.signer().seal_for_document(
            key,
            workspace,
            object,
            device.device_id(),
            authorization,
            &payload,
        )
    }

    fn collaboration_operation_plaintext(
        &self,
        operation: &EncryptedOperation,
        key: &ObjectKey,
        trusted_key: &str,
    ) -> Result<zeroize::Zeroizing<Vec<u8>>> {
        let inline = operation.open(key, trusted_key)?;
        let Ok(reference) = serde_json::from_slice::<CollaborationBlobReference>(&inline) else {
            return Ok(inline);
        };
        if reference.version != 1 || reference.blob.plaintext_size > (MAX_DOCUMENT_BYTES * 2) as u64
        {
            return Err(invalid("collaboration_invalid_blob_reference"));
        }
        let mut ciphertext = self.sync_blob_file(&reference.blob, false)?;
        let mut plaintext =
            zeroize::Zeroizing::new(Vec::with_capacity(reference.blob.plaintext_size as usize));
        reference
            .blob
            .decrypt(key, &mut ciphertext, &mut *plaintext)?;
        Ok(plaintext)
    }

    pub(crate) fn collaboration_operation_blob(
        &self,
        operation: &EncryptedOperation,
        key: &ObjectKey,
        trusted_key: &str,
    ) -> Result<Option<crate::sync::EncryptedBlob>> {
        if !matches!(
            operation.kind,
            Some(OperationKind::Text | OperationKind::Metadata)
        ) {
            return Ok(None);
        }
        let inline = operation.open(key, trusted_key)?;
        let Some(reference) = serde_json::from_slice::<CollaborationBlobReference>(&inline).ok()
        else {
            return Ok(None);
        };
        if reference.version != 1 || reference.blob.plaintext_size > (MAX_DOCUMENT_BYTES * 2) as u64
        {
            return Err(invalid("collaboration_invalid_blob_reference"));
        }
        reference.blob.validate()?;
        Ok(Some(reference.blob))
    }

    fn collaboration_state(&self, object: &str) -> Result<Option<DocumentState>> {
        let Some(bytes) = read_optional(&self.sync_path(&state_path(object)?)?)? else {
            return Ok(None);
        };
        let mut state: DocumentState =
            serde_json::from_slice(&bytes).map_err(|_| invalid("collaboration_invalid_state"))?;
        if !matches!(state.version, 1 | 2) || state.object_id != object {
            return Err(invalid("collaboration_invalid_state"));
        }
        if state.version == 1 {
            let has_pending =
                self.sync_journal()?.outbox.iter().any(|operation| {
                    operation.object_id == object && operation.generation.is_some()
                });
            if has_pending {
                return Err(invalid("collaboration_baseline_required"));
            }
            state.version = 2;
            state.acknowledged = Some(DocumentSnapshot {
                sequence: state.covered_sequence.clone(),
                path: Some(state.path.clone()),
                deleted: false,
                revision: state.revision.clone(),
                materialized: state.materialized.clone(),
                update: state.update.clone(),
            });
        }
        crate::sync::identifier(&state.generation)?;
        self.sync_file_path(&state.path)?;
        let materialized = crate::sync::decode(&state.materialized, 0, MAX_TEXT_BYTES)?;
        let text = TextDocument::restore(&state.update)?.text();
        if markdown::revision(&materialized) != state.revision
            || (text != body(&state.path, &materialized, object)?
                && render(&state.path, &materialized, object, &text)? != materialized)
        {
            return Err(invalid("collaboration_invalid_state"));
        }
        self.collaboration_validate_recovery_state(&state)?;
        Ok(Some(state))
    }
}

#[cfg(test)]
mod tests;
