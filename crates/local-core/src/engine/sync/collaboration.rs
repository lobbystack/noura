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
fn is_markdown(path: &str) -> bool {
    Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("md"))
}
fn body(path: &str, bytes: &[u8], object_id: &str) -> Result<String> {
    if bytes.len() > MAX_TEXT_BYTES {
        return Err(invalid("collaboration_unsupported_text"));
    }
    let parsed = markdown::parse_markdown(path, bytes);
    if matches!(parsed, ParsedMarkdown::Malformed { .. }) && is_markdown(path) {
        return Err(invalid("collaboration_invalid_managed_document"));
    }
    if is_markdown(path)
        && let ParsedMarkdown::Managed(object) = parsed
    {
        if object.id != object_id {
            return Err(invalid("sync_identity_changed"));
        }
        Ok(object.body)
    } else {
        let (text, _, _) = split_raw_bytes(bytes)?;
        crate::sync::collaboration::validate_text(&text)?;
        Ok(text)
    }
}
fn render(path: &str, bytes: &[u8], object_id: &str, text: &str) -> Result<Vec<u8>> {
    crate::sync::collaboration::validate_text(text)?;
    let next = if is_markdown(path)
        && let ParsedMarkdown::Managed(mut object) = markdown::parse_markdown(path, bytes)
    {
        if object.id != object_id {
            return Err(invalid("sync_identity_changed"));
        }
        object.body = text.into();
        markdown::serialize_object(&object)?
    } else {
        let (_, crlf, bom) = split_raw_bytes(bytes)?;
        compose_raw_bytes(text, crlf, bom)
    };
    if next.len() > MAX_TEXT_BYTES {
        return Err(invalid("collaboration_unsupported_text"));
    }
    if !matches!(
        markdown::parse_markdown(path, bytes),
        ParsedMarkdown::Managed(_)
    ) && matches!(
        markdown::parse_markdown(path, &next),
        ParsedMarkdown::Managed(_) | ParsedMarkdown::Malformed { .. }
    ) && is_markdown(path)
    {
        return Err(invalid("collaboration_document_mode_changed"));
    }
    Ok(next)
}

fn validate_metadata_field(field: &str) -> Result<()> {
    let valid = field == "title"
        || field.strip_prefix("property:").is_some_and(|key| {
            !key.is_empty()
                && key.len() <= 128
                && !key.contains(['\0', '\r', '\n'])
                && !matches!(key, "id" | "type" | "created" | "updated")
        });
    if !valid {
        return Err(invalid("collaboration_invalid_metadata_field"));
    }
    Ok(())
}

fn metadata_value(object: &WorkspaceObject, field: &str) -> Result<CollaborativeMetadataValue> {
    validate_metadata_field(field)?;
    if field == "title" {
        return Ok(CollaborativeMetadataValue::Value {
            value: serde_json::Value::String(object.title.clone()),
        });
    }
    let key = field
        .strip_prefix("property:")
        .ok_or_else(|| invalid("collaboration_invalid_metadata_field"))?;
    Ok(object
        .properties
        .get(key)
        .map_or(CollaborativeMetadataValue::Missing, |value| {
            CollaborativeMetadataValue::Value {
                value: value.clone(),
            }
        }))
}

fn apply_metadata_patches(
    path: &str,
    bytes: &[u8],
    object_id: &str,
    patches: &[CollaborativeMetadataPatch],
) -> Result<Vec<u8>> {
    let ParsedMarkdown::Managed(mut object) = markdown::parse_markdown(path, bytes) else {
        return Err(invalid("collaboration_invalid_managed_document"));
    };
    if object.id != object_id {
        return Err(invalid("sync_identity_changed"));
    }
    let mut fields = BTreeSet::new();
    for patch in patches {
        validate_metadata_field(&patch.field)?;
        if !fields.insert(&patch.field)
            || metadata_value(&object, &patch.field)? != patch.expected
            || patch.expected == patch.value
        {
            return Err(invalid("collaboration_metadata_conflict"));
        }
        match (patch.field.as_str(), &patch.value) {
            ("title", CollaborativeMetadataValue::Value { value }) => {
                let title = value
                    .as_str()
                    .filter(|title| !title.trim().is_empty())
                    .ok_or_else(|| invalid("collaboration_invalid_metadata_patch"))?;
                object.title = title.trim().into();
            }
            ("title", CollaborativeMetadataValue::Missing) => {
                return Err(invalid("collaboration_invalid_metadata_patch"));
            }
            (field, value) => {
                let key = field
                    .strip_prefix("property:")
                    .ok_or_else(|| invalid("collaboration_invalid_metadata_field"))?;
                match value {
                    CollaborativeMetadataValue::Missing => {
                        object.properties.remove(key);
                    }
                    CollaborativeMetadataValue::Value { value } => {
                        object.properties.insert(key.into(), value.clone());
                    }
                }
            }
        }
    }
    normalize_domain_properties(&object.object_type, &mut object.properties)?;
    let serialized = markdown::serialize_object(&object)?;
    if body(path, &serialized, object_id)? != object.body {
        return Err(invalid("collaboration_canonicalization_loss"));
    }
    Ok(serialized)
}

fn metadata_patches(
    object: &WorkspaceObject,
    target: &WorkspaceObject,
) -> Result<Vec<CollaborativeMetadataPatch>> {
    if object.id != target.id
        || object.object_type != target.object_type
        || object.created != target.created
    {
        return Err(invalid("collaboration_protected_metadata"));
    }
    let mut metadata = Vec::new();
    if target.title != object.title {
        metadata.push(CollaborativeMetadataPatch {
            field: "title".into(),
            expected: metadata_value(object, "title")?,
            value: metadata_value(target, "title")?,
        });
    }
    let property_keys = object
        .properties
        .keys()
        .chain(target.properties.keys())
        .cloned()
        .collect::<BTreeSet<_>>();
    for key in property_keys {
        let field = format!("property:{key}");
        let expected = metadata_value(object, &field)?;
        let value = metadata_value(target, &field)?;
        if expected != value {
            metadata.push(CollaborativeMetadataPatch {
                field,
                expected,
                value,
            });
        }
    }
    Ok(metadata)
}

fn text_format(path: &str, bytes: &[u8]) -> Result<Option<CollaborativeTextFormat>> {
    if is_markdown(path)
        && matches!(
            markdown::parse_markdown(path, bytes),
            ParsedMarkdown::Managed(_)
        )
    {
        return Ok(None);
    }
    let (_, uses_crlf, has_bom) = split_raw_bytes(bytes)?;
    Ok(Some(CollaborativeTextFormat { has_bom, uses_crlf }))
}

fn apply_format_patch(
    path: &str,
    bytes: &[u8],
    patch: &CollaborativeFormatPatch,
) -> Result<Vec<u8>> {
    let (body, uses_crlf, has_bom) = split_raw_bytes(bytes)?;
    if is_markdown(path)
        && matches!(
            markdown::parse_markdown(path, bytes),
            ParsedMarkdown::Managed(_)
        )
        || (CollaborativeTextFormat { has_bom, uses_crlf } != patch.expected)
    {
        return Err(invalid("collaboration_format_conflict"));
    }
    Ok(compose_raw_bytes(
        &body,
        patch.value.uses_crlf,
        patch.value.has_bom,
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

    pub fn collaboration_object_is_active(&self, object_id: &str) -> Result<bool> {
        crate::sync::identifier(object_id)?;
        let journal = self.sync_journal()?;
        Ok(Self::sync_document_descriptor(&journal, object_id)
            .is_some_and(|descriptor| descriptor.mode == DocumentMode::Text))
    }

    pub(crate) fn collaboration_capture_external(
        &self,
        path: &str,
        device: &DeviceKeys,
        secrets: &SyncSecrets,
    ) -> Result<bool> {
        let _lock = self.write_lock("collaboration_external")?;
        let journal = self.sync_journal()?;
        let Some((object_id, _)) = journal
            .objects
            .iter()
            .find(|(_, object)| object.path == path)
        else {
            return Ok(false);
        };
        let Some((epoch, descriptor)) = Self::sync_document(&journal, object_id) else {
            return Ok(false);
        };
        if descriptor.mode != DocumentMode::Text {
            return Ok(false);
        }
        self.collaboration_recover(object_id, secrets)?;
        let state = self
            .collaboration_state(object_id)?
            .filter(|state| state.generation == descriptor.generation)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if state.deleted {
            if let Some(actual) = read_optional(&self.sync_file_path(path)?)? {
                self.collaboration_record_external_review(
                    &state,
                    "external_recreate_after_delete",
                    Some(&actual),
                )?;
            }
            return Ok(true);
        }
        let source = self.sync_file_path(path)?;
        let Some(actual) = read_optional(&source)? else {
            self.collaboration_record_external_review(&state, "external_delete", None)?;
            return Ok(true);
        };
        open_for_durable_read(&source)
            .and_then(|file| file.sync_all())
            .map_err(|_| invalid("sync_file_read_failed"))?;
        if read_optional(&source)?.as_deref() != Some(actual.as_slice()) {
            return Err(invalid("sync_file_changed"));
        }
        let actual_revision = markdown::revision(&actual);
        if actual_revision == state.revision {
            return Ok(true);
        }
        if !secrets
            .authorized_workspace_writers
            .contains(device.device_id())
            && !secrets
                .authorized_object_writers
                .contains(&(object_id.clone(), device.device_id().into()))
        {
            self.collaboration_record_external_review(
                &state,
                "removed_writer_external_draft",
                Some(&actual),
            )?;
            return Ok(true);
        }
        let prior_bytes = crate::sync::decode(&state.materialized, 0, MAX_TEXT_BYTES)?;
        let actual_body = match body(path, &actual, object_id) {
            Ok(body) => body,
            Err(_) => {
                self.collaboration_record_external_review(
                    &state,
                    "invalid_external_document",
                    Some(&actual),
                )?;
                return Ok(true);
            }
        };
        let metadata = match (
            markdown::parse_markdown(path, &prior_bytes),
            markdown::parse_markdown(path, &actual),
        ) {
            (ParsedMarkdown::Managed(prior), ParsedMarkdown::Managed(mut target)) => {
                if target.id != prior.id
                    || target.object_type != prior.object_type
                    || target.created != prior.created
                {
                    self.collaboration_record_external_review(
                        &state,
                        "protected_metadata_changed",
                        Some(&actual),
                    )?;
                    return Ok(true);
                }
                if normalize_domain_properties(&target.object_type, &mut target.properties).is_err()
                {
                    self.collaboration_record_external_review(
                        &state,
                        "invalid_external_metadata",
                        Some(&actual),
                    )?;
                    return Ok(true);
                }
                metadata_patches(&prior, &target)?
            }
            (ParsedMarkdown::Managed(_), _) => {
                self.collaboration_record_external_review(
                    &state,
                    "managed_document_became_ambiguous",
                    Some(&actual),
                )?;
                return Ok(true);
            }
            (_, ParsedMarkdown::Managed(_) | ParsedMarkdown::Malformed { .. })
                if is_markdown(path) =>
            {
                self.collaboration_record_external_review(
                    &state,
                    "external_document_mode_changed",
                    Some(&actual),
                )?;
                return Ok(true);
            }
            _ => Vec::new(),
        };
        let prior_document = TextDocument::restore(&state.update)?;
        let format = match (
            text_format(path, &prior_bytes)?,
            text_format(path, &actual)?,
        ) {
            (Some(expected), Some(value)) if expected != value => {
                Some(CollaborativeFormatPatch { expected, value })
            }
            (Some(_), Some(_)) | (None, None) => None,
            _ => {
                self.collaboration_record_external_review(
                    &state,
                    "external_document_mode_changed",
                    Some(&actual),
                )?;
                return Ok(true);
            }
        };
        let updates = if actual_body == prior_document.text() {
            Vec::new()
        } else {
            vec![prior_document.replace_text(&actual_body)?.1]
        };
        if updates.is_empty() && metadata.is_empty() && format.is_none() {
            self.collaboration_record_external_review(
                &state,
                "external_canonical_format_change",
                Some(&actual),
            )?;
            return Ok(true);
        }
        let transaction = CollaborativeTransaction {
            version: 1,
            object_id: object_id.clone(),
            generation: descriptor.generation.clone(),
            updates,
            metadata,
            format,
            lifecycle: None,
        };
        transaction.validate()?;
        let key = secrets
            .objects
            .get(&(object_id.clone(), epoch))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let operation = self.collaboration_seal_operation(
            key,
            &journal.workspace_id,
            object_id,
            device,
            (
                epoch,
                &journal.access_revision,
                &descriptor.generation,
                OperationKind::Metadata,
            ),
            &serde_json::to_vec(&transaction).map_err(|_| invalid("sync_serialize_failed"))?,
        )?;
        let document = if transaction.updates.is_empty() {
            prior_document
        } else {
            prior_document.apply(&transaction.updates)?
        };
        let delta = (!transaction.updates.is_empty())
            .then(|| document.diff(&TextDocument::restore(&state.update)?.state_vector()))
            .transpose()?;
        let mut candidate = if transaction.updates.is_empty() {
            prior_bytes
        } else {
            render(path, &prior_bytes, object_id, &document.text())?
        };
        if !transaction.metadata.is_empty() {
            candidate = apply_metadata_patches(path, &candidate, object_id, &transaction.metadata)?;
        }
        if let Some(format) = &transaction.format {
            candidate = apply_format_patch(path, &candidate, format)?;
        }
        let snapshot_id = operation.operation_id.clone();
        self.sync_write_once(
            &format!(".noura/sync/collaboration/{object_id}/external/{snapshot_id}.json"),
            &CollaborationExternalSnapshot {
                version: 1,
                object_id: object_id.clone(),
                generation: descriptor.generation.clone(),
                revision: actual_revision.clone(),
                bytes: STANDARD.encode(&actual),
            },
        )?;
        self.collaboration_mutation_boundary(0)?;
        let mut next = DocumentState {
            version: 2,
            object_id: state.object_id,
            generation: state.generation,
            covered_sequence: state.covered_sequence,
            path: state.path,
            deleted: false,
            revision: markdown::revision(&candidate),
            materialized: STANDARD.encode(&candidate),
            update: document.state()?,
            acknowledged: state.acknowledged,
            pending: state.pending,
            acknowledged_operations: state.acknowledged_operations,
        };
        next.pending.push(PendingDocumentChange {
            operation_id: operation.operation_id.clone(),
            updates: transaction.updates,
            metadata: transaction.metadata,
            format: transaction.format,
            lifecycle: transaction.lifecycle,
        });
        self.collaboration_validate_recovery_state(&next)?;
        let intent = DocumentIntent {
            version: 1,
            base_revision: actual_revision,
            next: next.clone(),
            operation,
            outgoing: true,
            batch_id: None,
            batch_digest: None,
            external_snapshot: Some(snapshot_id),
            previous_path: None,
        };
        self.sync_write(&intent_path(object_id)?, &Some(&intent))?;
        self.collaboration_mutation_boundary(1)?;
        self.sync_write_file_checked(path, &candidate, Some(&intent.base_revision))?;
        self.collaboration_mutation_boundary(2)?;
        self.collaboration_finish(&intent)?;
        if is_markdown(path) {
            self.reindex_raw_markdown(path, &candidate, "collaboration_external")?;
        }
        if let Some(delta) = delta {
            self.emit(
                "collaboration:update",
                "external",
                serde_json::json!({"objectId":object_id,"generation":descriptor.generation,"update":delta,"revision":next.revision}),
            );
        }
        self.emit(
            "file:changed",
            "external",
            serde_json::json!({"paths":[path]}),
        );
        Ok(true)
    }

    pub fn collaboration_transport_status(&self, status: CollaborationStatus) -> Result<()> {
        if !matches!(
            status,
            CollaborationStatus::Offline | CollaborationStatus::Reconnecting
        ) {
            return Err(invalid("collaboration_invalid_status"));
        }
        if status == CollaborationStatus::Offline {
            self.collaboration_clear_presence()?;
        }
        let sessions = self
            .collaboration_sessions
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?;
        let documents = sessions
            .values()
            .map(|(object, generation, _)| (object.clone(), generation.clone()))
            .collect::<BTreeSet<_>>();
        drop(sessions);
        for (object, generation) in documents {
            self.emit(
                "collaboration:status",
                "sync",
                serde_json::to_value(CollaborationStatusEvent {
                    object_id: object,
                    generation,
                    status,
                })
                .map_err(|_| invalid("sync_serialize_failed"))?,
            );
        }
        Ok(())
    }

    pub(crate) fn collaboration_prepare_checkpoint_content(
        &self,
        object_id: &str,
        generation: &str,
        key: &ObjectKey,
    ) -> Result<(
        crate::sync::CheckpointContent,
        Option<crate::sync::EncryptedBlob>,
        DocumentMode,
    )> {
        crate::sync::identifier(object_id)?;
        crate::sync::identifier(generation)?;
        let _lock = self.write_lock("collaboration_prepare_checkpoint")?;
        let journal = self.sync_journal()?;
        let object = journal
            .objects
            .get(object_id)
            .ok_or_else(|| invalid("sync_object_missing"))?;
        let path = self.sync_file_path(&object.path)?;
        let mut source =
            open_for_durable_read(&path).map_err(|_| invalid("sync_file_read_failed"))?;
        source
            .sync_all()
            .map_err(|_| invalid("sync_file_read_failed"))?;
        let size = source
            .metadata()
            .map_err(|_| invalid("sync_file_read_failed"))?
            .len();
        if size >= crate::sync::blobs::MAX_BLOB_BYTES {
            return Err(invalid("sync_blob_too_large"));
        }
        let bytes = if size <= MAX_TEXT_BYTES as u64 {
            let mut bytes = Vec::with_capacity(size as usize);
            Read::by_ref(&mut source)
                .read_to_end(&mut bytes)
                .map_err(|_| invalid("sync_file_read_failed"))?;
            Some(bytes)
        } else {
            None
        };
        let mode = if let Some(bytes) = bytes.as_deref() {
            match body(&object.path, bytes, object_id) {
                Ok(_) => DocumentMode::Text,
                Err(error)
                    if matches!(
                        error.code.as_str(),
                        "invalid_utf8" | "collaboration_unsupported_text"
                    ) =>
                {
                    DocumentMode::Attachment
                }
                Err(error) => return Err(error),
            }
        } else {
            DocumentMode::Attachment
        };
        let revision = file_revision(&path)?.ok_or_else(|| invalid("sync_file_read_failed"))?;
        if object.revision.as_ref() != Some(&revision)
            || file_revision(&path)?.as_ref() != Some(&revision)
        {
            return Err(invalid("sync_capture_required"));
        }
        let (content, blob) =
            if let Some(bytes) = bytes.as_ref().filter(|bytes| bytes.len() <= 700 * 1024) {
                (Some(STANDARD.encode(bytes)), None)
            } else {
                source
                    .rewind()
                    .map_err(|_| invalid("sync_file_read_failed"))?;
                let temporary = format!(".noura/sync/blobs/pending_{}", uuid::Uuid::new_v4());
                self.sync_prepare_parent(&temporary)?;
                let temporary_path = self.sync_path(&temporary)?;
                let mut output = atomic_write_file::AtomicWriteFile::open(&temporary_path)
                    .map_err(|_| invalid("sync_blob_write_failed"))?;
                let blob = crate::sync::EncryptedBlob::encrypt(key, &mut source, &mut output)?;
                if blob.revision != revision || file_revision(&path)?.as_ref() != Some(&revision) {
                    return Err(invalid("sync_file_changed"));
                }
                output
                    .sync_all()
                    .map_err(|_| invalid("sync_blob_write_failed"))?;
                output
                    .commit()
                    .map_err(|_| invalid("sync_blob_write_failed"))?;
                let final_path = self.sync_path(&format!(".noura/sync/blobs/{}", blob.id))?;
                std::fs::rename(&temporary_path, &final_path)
                    .map_err(|_| invalid("sync_blob_write_failed"))?;
                sync_parent(&final_path, "collaboration_checkpoint_blob")?;
                (None, Some(blob))
            };
        Ok((
            crate::sync::CheckpointContent {
                version: 1,
                object_id: object_id.into(),
                generation: generation.into(),
                content_revision: Some(revision),
                change: FileChange {
                    version: if blob.is_some() { 3 } else { 1 },
                    path: object.path.clone(),
                    previous_path: None,
                    base_revision: None,
                    content,
                    accepted_revisions: None,
                    blob: blob.clone(),
                },
            },
            blob,
            mode,
        ))
    }

    /// Legacy file mutations must never bypass a signed collaborative generation.
    /// Called with the engine write lock already held.
    pub(crate) fn collaboration_guard_file_mutation(&self, path: &str) -> Result<()> {
        let journal = self.sync_journal()?;
        let Some((id, _)) = journal.objects.iter().find(|(_, value)| value.path == path) else {
            return Ok(());
        };
        if Self::sync_document_descriptor(&journal, id)
            .is_some_and(|document| document.mode == DocumentMode::Text)
        {
            return Err(invalid("collaboration_transaction_required"));
        }
        Ok(())
    }

    fn collaboration_install_attachment_checkpoint(
        &self,
        journal: &mut Journal,
        checkpoint: &crate::sync::EncryptedCheckpoint,
        content: &crate::sync::CheckpointContent,
        key: &ObjectKey,
    ) -> Result<()> {
        let target_revision = content
            .content_revision
            .as_ref()
            .ok_or_else(|| invalid("sync_invalid_checkpoint_revision"))?;
        let checkpoint_digest = checkpoint.digest()?;
        let checkpoint_path = format!(
            ".noura/sync/collaboration/{}/checkpoint.json",
            content.object_id
        );
        if let Some(saved) = read_optional(&self.sync_path(&checkpoint_path)?)? {
            let prior: crate::sync::EncryptedCheckpoint = serde_json::from_slice(&saved)
                .map_err(|_| invalid("collaboration_invalid_checkpoint_record"))?;
            if sync_cursor(&prior.covered_sequence)? > sync_cursor(&checkpoint.covered_sequence)?
                || prior.payload.epoch > checkpoint.payload.epoch
                || (prior.generation == checkpoint.generation
                    && prior.digest()? != checkpoint_digest)
            {
                return Err(invalid("collaboration_checkpoint_rollback"));
            }
        }
        let transition_id = journal
            .transition
            .as_ref()
            .filter(|transition| {
                transition
                    .checkpoints
                    .iter()
                    .any(|candidate| candidate.digest().ok() == Some(checkpoint_digest.clone()))
            })
            .map(|transition| transition.transition_id.clone())
            .or_else(|| {
                journal
                    .activations
                    .get(&content.object_id)
                    .filter(|activation| {
                        activation.checkpoint.digest().ok() == Some(checkpoint_digest.clone())
                    })
                    .map(|activation| activation.activation_id.clone())
            })
            .unwrap_or_else(|| checkpoint_digest.clone());
        let state_path = format!(
            ".noura/sync/collaboration/{}/attachment.json",
            content.object_id
        );
        let recovery_path = format!(
            ".noura/sync/collaboration/{}/attachment-transitions/{transition_id}.json",
            content.object_id
        );
        if let Some(saved) = read_optional(&self.sync_path(&state_path)?)? {
            let state: AttachmentGenerationState = serde_json::from_slice(&saved)
                .map_err(|_| invalid("collaboration_invalid_attachment_state"))?;
            if state.version != 1 || state.object_id != content.object_id {
                return Err(invalid("collaboration_invalid_attachment_state"));
            }
            if state.generation == checkpoint.generation {
                if state.path != content.change.path
                    || state.revision != *target_revision
                    || state.checkpoint_digest != checkpoint_digest
                {
                    return Err(invalid("collaboration_invalid_attachment_state"));
                }
                let recovery: AttachmentTransitionRecovery = serde_json::from_slice(
                    &read_optional(&self.sync_path(&recovery_path)?)?
                        .ok_or_else(|| invalid("collaboration_recovery_record_required"))?,
                )
                .map_err(|_| invalid("collaboration_invalid_recovery_record"))?;
                return self.collaboration_finish_attachment_install(journal, &state, &recovery);
            }
        }

        let destination = self.sync_file_path(&content.change.path)?;
        let current_revision = file_revision(&destination)?;
        let prior_object = journal.objects.get(&content.object_id);
        let conflict = prior_object.is_some_and(|prior| {
            prior.path != content.change.path
                || (current_revision != prior.revision
                    && current_revision.as_ref() != Some(target_revision))
        }) || (prior_object.is_none()
            && current_revision.is_some()
            && current_revision.as_ref() != Some(target_revision));
        if conflict {
            self.collaboration_record_review_without_prior(
                &transition_id,
                "attachment_destination_collision",
                &content.object_id,
                checkpoint,
                None,
                false,
            )?;
            return Err(invalid("collaboration_needs_review"));
        }
        let recovery = AttachmentTransitionRecovery {
            version: 1,
            transition_id,
            checkpoint: checkpoint.clone(),
            pending_operations: journal
                .outbox
                .iter()
                .filter(|operation| operation.object_id == content.object_id)
                .cloned()
                .collect(),
        };
        self.sync_write_once(&recovery_path, &recovery)?;
        if current_revision.as_ref() != Some(target_revision) {
            self.sync_prepare_parent(&content.change.path)?;
            let mut output = atomic_write_file::AtomicWriteFile::open(&destination)
                .map_err(|_| invalid("sync_file_write_failed"))?;
            match (&content.change.content, &content.change.blob) {
                (Some(encoded), None) => {
                    let bytes = crate::sync::decode(encoded, 0, 1024 * 1024)?;
                    output
                        .write_all(&bytes)
                        .map_err(|_| invalid("sync_file_write_failed"))?;
                }
                (None, Some(blob)) => {
                    let mut ciphertext = self.sync_blob_file(blob, false)?;
                    blob.decrypt(key, &mut ciphertext, &mut output)?;
                }
                _ => return Err(invalid("collaboration_invalid_checkpoint_content")),
            }
            output
                .sync_all()
                .map_err(|_| invalid("sync_file_write_failed"))?;
            output
                .commit()
                .map_err(|_| invalid("sync_file_write_failed"))?;
            sync_parent(&destination, "collaboration_attachment_checkpoint")?;
            if file_revision(&destination)?.as_ref() != Some(target_revision) {
                return Err(invalid("sync_file_changed"));
            }
        }
        let state = AttachmentGenerationState {
            version: 1,
            object_id: content.object_id.clone(),
            generation: checkpoint.generation.clone(),
            path: content.change.path.clone(),
            revision: target_revision.clone(),
            checkpoint_digest,
        };
        self.sync_write(&checkpoint_path, checkpoint)?;
        self.sync_write(&state_path, &state)?;
        self.collaboration_finish_attachment_install(journal, &state, &recovery)
    }

    fn collaboration_finish_attachment_install(
        &self,
        journal: &mut Journal,
        state: &AttachmentGenerationState,
        recovery: &AttachmentTransitionRecovery,
    ) -> Result<()> {
        crate::sync::identifier(&recovery.transition_id)?;
        if recovery.version != 1
            || recovery.checkpoint.digest()? != state.checkpoint_digest
            || recovery.pending_operations.len() > 1_000
            || recovery
                .pending_operations
                .iter()
                .any(|operation| operation.object_id != state.object_id)
        {
            return Err(invalid("collaboration_invalid_recovery_record"));
        }
        let pending = recovery
            .pending_operations
            .iter()
            .map(|operation| operation.operation_id.as_str())
            .collect::<BTreeSet<_>>();
        journal
            .outbox
            .retain(|operation| !pending.contains(operation.operation_id.as_str()));
        journal.objects.insert(
            state.object_id.clone(),
            ObjectState {
                path: state.path.clone(),
                revision: Some(state.revision.clone()),
            },
        );
        self.sync_write(STATE_PATH, journal)
    }

    /// Called only after the checkpoint's signed policy has been accepted locally.
    pub fn collaboration_install_checkpoint(
        &self,
        checkpoint: &crate::sync::EncryptedCheckpoint,
        key: &ObjectKey,
        trusted_key: &str,
        device: &DeviceKeys,
        secrets: &SyncSecrets,
    ) -> Result<()> {
        let content = checkpoint.open(key, trusted_key)?;
        self.sync_check_workspace(&checkpoint.payload)?;
        let journal_snapshot = self.sync_journal()?;
        let activation = journal_snapshot.activations.get(&content.object_id);
        if let Some(activation) = activation {
            activation.verify(trusted_key)?;
            let authorization = journal_snapshot
                .access_authorizations
                .get(&checkpoint.payload.policy_revision)
                .ok_or_else(|| invalid("sync_policy_history_required"))?;
            if activation.checkpoint.digest()? != checkpoint.digest()?
                || activation.document.generation != checkpoint.generation
                || checkpoint.payload.epoch != 1
                || !authorization
                    .workspace_writers
                    .contains(&checkpoint.payload.device_id)
            {
                return Err(invalid("collaboration_checkpoint_policy_mismatch"));
            }
        } else {
            let checkpoint_policy =
                self.sync_checkpoint_policy(&checkpoint.payload.policy_revision)?;
            checkpoint_policy.verify(trusted_key)?;
            if checkpoint_policy.device_id != checkpoint.payload.device_id
                || !checkpoint_policy.objects.iter().any(|object| {
                    object.object_id == content.object_id
                        && object.epoch == checkpoint.payload.epoch
                        && object
                            .document
                            .as_ref()
                            .is_some_and(|document| document.generation == checkpoint.generation)
                })
            {
                return Err(invalid("collaboration_checkpoint_policy_mismatch"));
            }
        }
        let _lock = self.write_lock("collaboration_checkpoint")?;
        let mut journal = self.sync_journal()?;
        let descriptor = Self::sync_document_descriptor(&journal, &content.object_id)
            .filter(|document| document.generation == checkpoint.generation)
            .ok_or_else(|| invalid("collaboration_checkpoint_policy_mismatch"))?;
        let generation = descriptor.generation.clone();
        if descriptor.mode == DocumentMode::Attachment {
            return self.collaboration_install_attachment_checkpoint(
                &mut journal,
                checkpoint,
                &content,
                key,
            );
        }
        let bytes = match (&content.change.content, &content.change.blob) {
            (Some(encoded), None) => crate::sync::decode(encoded, 0, MAX_TEXT_BYTES)?,
            (None, Some(blob)) => {
                blob.validate()?;
                if blob.plaintext_size > MAX_TEXT_BYTES as u64 {
                    return Err(invalid("collaboration_unsupported_text"));
                }
                let mut ciphertext = self.sync_blob_file(blob, false)?;
                let mut plaintext = Vec::with_capacity(blob.plaintext_size as usize);
                blob.decrypt(key, &mut ciphertext, &mut plaintext)?;
                plaintext
            }
            _ => return Err(invalid("collaboration_invalid_checkpoint_content")),
        };
        let text = body(&content.change.path, &bytes, &content.object_id)?;
        let document = TextDocument::fresh_generation(&text, &generation)?;
        let checkpoint_path = format!(
            ".noura/sync/collaboration/{}/checkpoint.json",
            content.object_id
        );
        if let Some(saved) = read_optional(&self.sync_path(&checkpoint_path)?)? {
            let prior: crate::sync::EncryptedCheckpoint = serde_json::from_slice(&saved)
                .map_err(|_| invalid("collaboration_invalid_checkpoint_record"))?;
            if sync_cursor(&prior.covered_sequence)? > sync_cursor(&checkpoint.covered_sequence)?
                || prior.payload.epoch > checkpoint.payload.epoch
                || (prior.generation == generation && prior.digest()? != checkpoint.digest()?)
            {
                return Err(invalid("collaboration_checkpoint_rollback"));
            }
        }
        let transition_id = journal
            .transition
            .as_ref()
            .filter(|transition| {
                transition
                    .checkpoints
                    .iter()
                    .any(|candidate| candidate.digest().ok() == checkpoint.digest().ok())
            })
            .map(|transition| transition.transition_id.clone())
            .or_else(|| {
                journal
                    .activations
                    .get(&content.object_id)
                    .filter(|activation| {
                        activation.checkpoint.digest().ok() == checkpoint.digest().ok()
                    })
                    .map(|activation| activation.activation_id.clone())
            })
            .unwrap_or(checkpoint.digest()?);
        let recovery_path = format!(
            ".noura/sync/collaboration/{}/transitions/{transition_id}.json",
            content.object_id
        );
        if let Some(prior) = self.collaboration_state(&content.object_id)?
            && prior.generation == generation
        {
            let recovery: GenerationRecoveryRecord =
                read_optional(&self.sync_path(&recovery_path)?)?
                    .map(|record| {
                        serde_json::from_slice(&record)
                            .map_err(|_| invalid("collaboration_invalid_recovery_record"))
                    })
                    .transpose()?
                    .ok_or_else(|| invalid("collaboration_recovery_record_required"))?;
            self.collaboration_finish_generation_install(
                &mut journal,
                &prior,
                &recovery,
                &device.signer().public_key(),
            )?;
            return Ok(());
        }
        let destination = self.sync_file_path(&content.change.path)?;
        let existing = read_optional(&destination)?;
        let prior = self.collaboration_state(&content.object_id)?;
        if let Some(prior) = &prior
            && (prior.path != content.change.path
                || existing
                    .as_ref()
                    .is_none_or(|current| markdown::revision(current) != prior.revision))
        {
            self.collaboration_record_review(
                &transition_id,
                "external_or_move_conflict",
                prior,
                checkpoint,
                existing.as_deref(),
            )?;
            return Err(invalid("collaboration_needs_review"));
        }
        if prior.is_none() && existing.as_deref().is_some_and(|current| current != bytes) {
            self.collaboration_record_review_without_prior(
                &transition_id,
                "destination_collision",
                &content.object_id,
                checkpoint,
                existing.as_deref(),
                false,
            )?;
            return Err(invalid("collaboration_needs_review"));
        }
        let checkpoint_snapshot = DocumentSnapshot {
            sequence: checkpoint.covered_sequence.clone(),
            path: Some(content.change.path.clone()),
            deleted: false,
            revision: markdown::revision(&bytes),
            materialized: STANDARD.encode(&bytes),
            update: document.state()?,
        };
        let mut installed_bytes = bytes.clone();
        let mut installed_document = document;
        let mut pending = Vec::new();
        let mut rebased_operation = None;
        if let Some(prior) = &prior {
            let acknowledged = prior
                .acknowledged
                .as_ref()
                .ok_or_else(|| invalid("collaboration_baseline_required"))?;
            let base = TextDocument::restore(&acknowledged.update)?.text();
            let draft = TextDocument::restore(&prior.update)?.text();
            if draft != base {
                let Some(merged) = super::super::merge_markdown_text(&base, &draft, &text) else {
                    self.collaboration_record_review(
                        &transition_id,
                        "overlapping_draft",
                        prior,
                        checkpoint,
                        None,
                    )?;
                    return Err(invalid("collaboration_needs_review"));
                };
                let can_write = secrets
                    .authorized_workspace_writers
                    .contains(device.device_id())
                    || secrets
                        .authorized_object_writers
                        .contains(&(content.object_id.clone(), device.device_id().into()));
                if !can_write {
                    self.collaboration_record_review(
                        &transition_id,
                        "removed_writer_draft",
                        prior,
                        checkpoint,
                        None,
                    )?;
                    return Err(invalid("collaboration_needs_review"));
                }
                if merged != text {
                    let (candidate, update) = installed_document.replace_text(&merged)?;
                    installed_bytes =
                        render(&content.change.path, &bytes, &content.object_id, &merged)?;
                    let change = CollaborativeChange {
                        version: 1,
                        object_id: content.object_id.clone(),
                        generation: generation.clone(),
                        updates: vec![update],
                    };
                    let operation = self.collaboration_seal_operation(
                        key,
                        &journal.workspace_id,
                        &content.object_id,
                        device,
                        (
                            checkpoint.payload.epoch,
                            &journal.access_revision,
                            &generation,
                            OperationKind::Text,
                        ),
                        &serde_json::to_vec(&change)
                            .map_err(|_| invalid("sync_serialize_failed"))?,
                    )?;
                    pending.push(PendingDocumentChange {
                        operation_id: operation.operation_id.clone(),
                        updates: change.updates,
                        metadata: Vec::new(),
                        format: None,
                        lifecycle: None,
                    });
                    rebased_operation = Some(operation);
                    installed_document = candidate;
                }
            }
        }
        let state = DocumentState {
            version: 2,
            object_id: content.object_id.clone(),
            generation,
            covered_sequence: checkpoint.covered_sequence.clone(),
            path: content.change.path.clone(),
            deleted: false,
            revision: markdown::revision(&installed_bytes),
            materialized: STANDARD.encode(&installed_bytes),
            update: installed_document.state()?,
            acknowledged: Some(checkpoint_snapshot),
            pending,
            acknowledged_operations: Vec::new(),
        };
        self.collaboration_validate_recovery_state(&state)?;
        let old_operations = journal
            .outbox
            .iter()
            .filter(|operation| operation.object_id == content.object_id)
            .cloned()
            .collect();
        let recovery = GenerationRecoveryRecord {
            version: 1,
            transition_id,
            state: prior,
            pending_operations: old_operations,
            rebased_operation,
        };
        self.sync_write(&checkpoint_path, checkpoint)?;
        self.sync_write_once(&recovery_path, &recovery)?;
        let expected = existing.as_ref().map(|current| markdown::revision(current));
        if existing.as_deref() != Some(installed_bytes.as_slice()) {
            self.sync_write_file_checked(
                &content.change.path,
                &installed_bytes,
                expected.as_deref(),
            )?;
        }
        self.sync_write(&state_path(&state.object_id)?, &state)?;
        self.collaboration_finish_generation_install(
            &mut journal,
            &state,
            &recovery,
            &device.signer().public_key(),
        )?;
        self.emit(
            "collaboration:activated",
            "sync",
            serde_json::json!({"objectIds":[state.object_id]}),
        );
        Ok(())
    }

    fn collaboration_finish_generation_install(
        &self,
        journal: &mut Journal,
        state: &DocumentState,
        recovery: &GenerationRecoveryRecord,
        local_public_key: &str,
    ) -> Result<()> {
        let install_recorded = journal.objects.get(&state.object_id).is_some_and(|object| {
            object.path == state.path && object.revision.as_ref() == Some(&state.revision)
        });
        if recovery.version != 1
            || recovery.transition_id.is_empty()
            || recovery.state.as_ref().is_some_and(|prior| {
                prior.object_id != state.object_id || prior.generation == state.generation
            })
            || recovery.pending_operations.iter().any(|saved| {
                journal
                    .outbox
                    .iter()
                    .find(|pending| pending.operation_id == saved.operation_id)
                    .is_some_and(|pending| pending != saved)
                    || (!install_recorded
                        && !journal
                            .outbox
                            .iter()
                            .any(|pending| pending.operation_id == saved.operation_id))
            })
        {
            return Err(invalid("collaboration_invalid_recovery_record"));
        }
        if let Some(operation) = &recovery.rebased_operation {
            operation.verify(local_public_key)?;
            if operation.workspace_id != journal.workspace_id
                || operation.object_id != state.object_id
                || operation.generation.as_ref() != Some(&state.generation)
                || operation.kind != Some(OperationKind::Text)
                || state
                    .pending
                    .first()
                    .is_none_or(|pending| pending.operation_id != operation.operation_id)
            {
                return Err(invalid("collaboration_invalid_recovery_record"));
            }
        } else if !state.pending.is_empty() {
            return Err(invalid("collaboration_invalid_recovery_record"));
        }
        let old_ids = recovery
            .pending_operations
            .iter()
            .map(|operation| operation.operation_id.as_str())
            .collect::<BTreeSet<_>>();
        journal
            .outbox
            .retain(|operation| !old_ids.contains(operation.operation_id.as_str()));
        if let Some(operation) = &recovery.rebased_operation
            && !journal
                .outbox
                .iter()
                .any(|pending| pending.operation_id == operation.operation_id)
        {
            journal.outbox.push(operation.clone());
        }
        journal.objects.insert(
            state.object_id.clone(),
            ObjectState {
                path: state.path.clone(),
                revision: Some(state.revision.clone()),
            },
        );
        self.sync_write(STATE_PATH, journal)
    }

    fn collaboration_record_review(
        &self,
        transition_id: &str,
        reason: &str,
        prior: &DocumentState,
        checkpoint: &crate::sync::EncryptedCheckpoint,
        competing_bytes: Option<&[u8]>,
    ) -> Result<()> {
        self.collaboration_record_review_without_prior(
            transition_id,
            reason,
            &prior.object_id,
            checkpoint,
            competing_bytes,
            true,
        )?;
        let path = format!(
            ".noura/sync/collaboration/{}/reviews/{transition_id}.json",
            prior.object_id
        );
        let mut record: CollaborationReviewRecord = serde_json::from_slice(
            &read_optional(&self.sync_path(&path)?)?
                .ok_or_else(|| invalid("collaboration_review_missing"))?,
        )
        .map_err(|_| invalid("collaboration_invalid_review"))?;
        record.prior = Some(prior.clone());
        self.sync_write(&path, &record)?;
        Ok(())
    }

    fn collaboration_record_review_without_prior(
        &self,
        transition_id: &str,
        reason: &str,
        object_id: &str,
        checkpoint: &crate::sync::EncryptedCheckpoint,
        competing_bytes: Option<&[u8]>,
        retained_draft: bool,
    ) -> Result<()> {
        crate::sync::identifier(transition_id)?;
        crate::sync::identifier(object_id)?;
        let path = format!(".noura/sync/collaboration/{object_id}/reviews/{transition_id}.json");
        self.sync_write(
            &path,
            &CollaborationReviewRecord {
                version: 1,
                transition_id: transition_id.into(),
                object_id: object_id.into(),
                reason: reason.into(),
                prior: None,
                checkpoint: checkpoint.clone(),
                competing_bytes: competing_bytes.map(|bytes| STANDARD.encode(bytes)),
            },
        )?;
        self.emit(
            "collaboration:status",
            "sync",
            serde_json::to_value(CollaborationStatusEvent {
                object_id: object_id.into(),
                generation: checkpoint.generation.clone(),
                status: CollaborationStatus::NeedsReview,
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        self.emit(
            "collaboration:review",
            "sync",
            serde_json::to_value(crate::sync::CollaborationConflictReview {
                review_id: transition_id.into(),
                object_id: object_id.into(),
                generation: checkpoint.generation.clone(),
                reason: reason.into(),
                retained_draft,
                competing_bytes: competing_bytes.is_some(),
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        Ok(())
    }

    fn collaboration_record_metadata_review(
        &self,
        operation: &EncryptedOperation,
        generation: &str,
        reason: &str,
    ) -> Result<()> {
        let state = self
            .collaboration_state(&operation.object_id)?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        let path = format!(
            ".noura/sync/collaboration/{}/reviews/{}.metadata.json",
            operation.object_id, operation.operation_id
        );
        let record = CollaborationMetadataReviewRecord {
            version: 1,
            operation_id: operation.operation_id.clone(),
            object_id: operation.object_id.clone(),
            generation: generation.into(),
            reason: reason.into(),
            operation: operation.clone(),
            retained: state,
        };
        if let Some(bytes) = read_optional(&self.sync_path(&path)?)? {
            let saved: CollaborationMetadataReviewRecord = serde_json::from_slice(&bytes)
                .map_err(|_| invalid("collaboration_invalid_review"))?;
            if saved.version != 1
                || saved.operation_id != record.operation_id
                || saved.object_id != record.object_id
                || saved.generation != record.generation
                || saved.reason != record.reason
                || saved.operation != record.operation
            {
                return Err(invalid("collaboration_invalid_review"));
            }
        } else {
            self.sync_write_once(&path, &record)?;
        }
        let mut journal = self.sync_journal()?;
        journal.receipts.insert(
            operation.operation_id.clone(),
            Receipt {
                digest: markdown::revision(
                    &serde_json::to_vec(operation).map_err(|_| invalid("sync_serialize_failed"))?,
                ),
                outcome: ApplyOutcome::Conflict,
                change_digest: None,
            },
        );
        self.sync_write(STATE_PATH, &journal)?;
        self.emit(
            "collaboration:status",
            "sync",
            serde_json::to_value(CollaborationStatusEvent {
                object_id: operation.object_id.clone(),
                generation: generation.into(),
                status: CollaborationStatus::NeedsReview,
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        self.emit(
            "collaboration:review",
            "sync",
            serde_json::to_value(crate::sync::CollaborationConflictReview {
                review_id: operation.operation_id.clone(),
                object_id: operation.object_id.clone(),
                generation: generation.into(),
                reason: reason.into(),
                retained_draft: true,
                competing_bytes: false,
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        Ok(())
    }

    fn collaboration_record_external_review(
        &self,
        state: &DocumentState,
        reason: &str,
        competing_bytes: Option<&[u8]>,
    ) -> Result<()> {
        let review_digest = markdown::revision(
            &serde_json::to_vec(&(
                "noura.collaboration.external-review",
                &state.object_id,
                &state.generation,
                &state.revision,
                reason,
                competing_bytes.map(|bytes| STANDARD.encode(bytes)),
            ))
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        let review_id = format!("external_{review_digest}");
        let path = format!(
            ".noura/sync/collaboration/{}/reviews/{review_id}.external.json",
            state.object_id
        );
        self.sync_write_once(
            &path,
            &CollaborationExternalReviewRecord {
                version: 1,
                review_id: review_id.clone(),
                object_id: state.object_id.clone(),
                generation: state.generation.clone(),
                reason: reason.into(),
                retained: state.clone(),
                competing_bytes: competing_bytes.map(|bytes| STANDARD.encode(bytes)),
            },
        )?;
        self.emit(
            "collaboration:status",
            "external",
            serde_json::to_value(CollaborationStatusEvent {
                object_id: state.object_id.clone(),
                generation: state.generation.clone(),
                status: CollaborationStatus::NeedsReview,
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        self.emit(
            "collaboration:review",
            "external",
            serde_json::to_value(crate::sync::CollaborationConflictReview {
                review_id,
                object_id: state.object_id.clone(),
                generation: state.generation.clone(),
                reason: reason.into(),
                retained_draft: true,
                competing_bytes: competing_bytes.is_some(),
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        Ok(())
    }

    pub fn collaboration_checkpoint_needed(&self, object: &str, generation: &str) -> Result<bool> {
        let _lock = self.write_lock("collaboration_checkpoint")?;
        Ok(!self.collaboration_generation_is_installed(object, generation)?)
    }

    pub fn collaboration_cover_history(
        &self,
        operation: &EncryptedOperation,
        sequence: &str,
    ) -> Result<bool> {
        let _lock = self.write_lock("collaboration_history")?;
        let Some(state) = self.collaboration_state(&operation.object_id)? else {
            return Ok(false);
        };
        if sync_cursor(sequence)? > sync_cursor(&state.covered_sequence)? {
            return Ok(false);
        }
        let mut journal = self.sync_journal()?;
        journal.receipts.insert(
            operation.operation_id.clone(),
            Receipt {
                digest: markdown::revision(
                    &serde_json::to_vec(operation).map_err(|_| invalid("sync_serialize_failed"))?,
                ),
                outcome: ApplyOutcome::Applied,
                change_digest: None,
            },
        );
        self.sync_write(STATE_PATH, &journal)?;
        Ok(true)
    }

    pub fn collaboration_open(
        &self,
        input: CollaborationOpenInput,
        device: &DeviceKeys,
        secrets: &SyncSecrets,
    ) -> Result<Option<CollaborationSession>> {
        let _lock = self.write_lock("collaboration_open")?;
        self.sync_file_path(&input.relative_path)?;
        let journal = self.sync_journal()?;
        let Some((id, _)) = journal
            .objects
            .iter()
            .find(|(_, object)| object.path == input.relative_path)
        else {
            return Ok(None);
        };
        let Some(descriptor) = Self::sync_document_descriptor(&journal, id) else {
            return Ok(None);
        };
        if descriptor.mode != DocumentMode::Text {
            return Ok(None);
        }
        self.collaboration_recover(id, secrets)?;
        let state = self
            .collaboration_state(id)?
            .filter(|state| state.generation == descriptor.generation)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if state.deleted {
            return Ok(None);
        }
        let actual = read_optional(&self.sync_file_path(&state.path)?)?
            .ok_or_else(|| invalid("collaboration_file_missing"))?;
        if markdown::revision(&actual) != state.revision {
            return Err(invalid("collaboration_external_change"));
        }
        let read_only = !secrets
            .authorized_workspace_writers
            .contains(device.device_id())
            && !secrets
                .authorized_object_writers
                .contains(&(id.clone(), device.device_id().into()));
        let mut sessions = self
            .collaboration_sessions
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?;
        // Each native lease closes independently across windows. They all address the same
        // durable per-object state; a view registry shares one lease within a window.
        let session_id = uuid::Uuid::new_v4().to_string();
        sessions.insert(
            session_id.clone(),
            (id.clone(), state.generation.clone(), read_only),
        );
        let status = if state.pending.is_empty() {
            CollaborationStatus::Synced
        } else {
            CollaborationStatus::SavedLocally
        };
        Ok(Some(CollaborationSession {
            object_id: id.clone(),
            generation: state.generation,
            session_id,
            update: state.update,
            revision: state.revision,
            read_only,
            role: if read_only {
                CollaborationBootstrapRole::Viewer
            } else {
                CollaborationBootstrapRole::Writer
            },
            status,
        }))
    }

    pub fn collaboration_flush(&self, session_id: &str, secrets: &SyncSecrets) -> Result<()> {
        let object = self
            .collaboration_sessions
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?
            .get(session_id)
            .map(|(object, _, _)| object.clone())
            .ok_or_else(|| invalid("collaboration_session_closed"))?;
        let _lock = self.write_lock("collaboration_flush")?;
        self.collaboration_recover(&object, secrets)
    }

    pub fn collaboration_close(&self, session_id: &str) -> Result<()> {
        crate::sync::identifier(session_id)?;
        self.collaboration_sessions
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?
            .remove(session_id);
        Ok(())
    }

    pub fn collaboration_seal_presence(
        &self,
        input: crate::sync::CollaborationPresenceInput,
        relay_session_id: &str,
        sequence: u64,
        device: &DeviceKeys,
        secrets: &SyncSecrets,
    ) -> Result<crate::sync::EncryptedPresence> {
        crate::sync::identifier(&input.session_id)?;
        let (object, generation, _) = self
            .collaboration_sessions
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?
            .get(&input.session_id)
            .cloned()
            .ok_or_else(|| invalid("collaboration_session_closed"))?;
        let journal = self.sync_journal()?;
        let (epoch, descriptor) = Self::sync_document(&journal, &object)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if descriptor.generation != generation {
            return Err(invalid("collaboration_stale_generation"));
        }
        let key = secrets
            .objects
            .get(&(object.clone(), epoch))
            .ok_or_else(|| invalid("sync_key_missing"))?;
        let state = self
            .collaboration_state(&object)?
            .filter(|state| state.generation == generation)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        TextDocument::restore(&state.update)?
            .validate_relative_positions(&[input.anchor.clone(), input.head.clone()])?;
        crate::sync::EncryptedPresence::seal(
            device,
            key,
            &crate::sync::PresenceContext {
                workspace_id: &journal.workspace_id,
                object_id: &object,
                generation: &generation,
                epoch,
                session_id: relay_session_id,
                sequence,
            },
            &crate::sync::PresenceSelection {
                anchor: input.anchor,
                head: input.head,
            },
        )
    }

    pub fn collaboration_receive_presence(
        &self,
        value: &crate::sync::EncryptedPresence,
        secrets: &SyncSecrets,
    ) -> Result<()> {
        let public_key = secrets
            .trusted_devices
            .get(&value.device_id)
            .ok_or_else(|| invalid("sync_untrusted_device"))?;
        value.verify(public_key)?;
        if value.workspace_id != self.manifest().id {
            return Err(invalid("sync_wrong_workspace"));
        }
        let journal = self.sync_journal()?;
        let (epoch, descriptor) = Self::sync_document(&journal, &value.object_id)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if epoch != value.epoch || descriptor.generation != value.generation {
            return Err(invalid("collaboration_stale_generation"));
        }
        let key = secrets
            .objects
            .get(&(value.object_id.clone(), epoch))
            .ok_or_else(|| invalid("sync_key_missing"))?;
        let selection = value.open(key, public_key)?;
        let state = self
            .collaboration_state(&value.object_id)?
            .filter(|state| state.generation == value.generation)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        TextDocument::restore(&state.update)?
            .validate_relative_positions(&[selection.anchor.clone(), selection.head.clone()])?;
        let color_hash = blake3::hash(value.device_id.as_bytes());
        let color = format!(
            "#{:02x}{:02x}{:02x}",
            color_hash.as_bytes()[0],
            color_hash.as_bytes()[1],
            color_hash.as_bytes()[2]
        );
        let mut presence = self
            .collaboration_presence
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?;
        presence.retain(|_, entry| entry.expires_at > std::time::Instant::now());
        if presence.get(&value.device_id).is_some_and(|entry| {
            entry.session_id == value.session_id && entry.sequence >= value.sequence
        }) {
            return Ok(());
        }
        presence.insert(
            value.device_id.clone(),
            super::CollaborationPresenceCacheEntry {
                object_id: value.object_id.clone(),
                generation: value.generation.clone(),
                session_id: value.session_id.clone(),
                sequence: value.sequence,
                expires_at: std::time::Instant::now() + std::time::Duration::from_secs(30),
                member: crate::sync::CollaborationPresenceMember {
                    device_id: value.device_id.clone(),
                    name: value.device_id.clone(),
                    color,
                    anchor: selection.anchor,
                    head: selection.head,
                },
            },
        );
        drop(presence);
        self.collaboration_emit_presence(&value.object_id, &value.generation)
    }

    pub fn collaboration_remove_presence(
        &self,
        device_id: &str,
        relay_session_id: &str,
    ) -> Result<()> {
        crate::sync::identifier(device_id)?;
        crate::sync::identifier(relay_session_id)?;
        let affected = {
            let mut presence = self
                .collaboration_presence
                .lock()
                .map_err(|_| invalid("collaboration_unavailable"))?;
            let affected = presence
                .get(device_id)
                .filter(|entry| entry.session_id == relay_session_id)
                .map(|entry| (entry.object_id.clone(), entry.generation.clone()));
            if affected.is_some() {
                presence.remove(device_id);
            }
            affected
        };
        if let Some((object, generation)) = affected {
            self.collaboration_emit_presence(&object, &generation)?;
        }
        Ok(())
    }

    pub fn collaboration_expire_presence(&self) -> Result<()> {
        let affected = {
            let mut presence = self
                .collaboration_presence
                .lock()
                .map_err(|_| invalid("collaboration_unavailable"))?;
            let now = std::time::Instant::now();
            let affected = presence
                .values()
                .filter(|entry| entry.expires_at <= now)
                .map(|entry| (entry.object_id.clone(), entry.generation.clone()))
                .collect::<BTreeSet<_>>();
            presence.retain(|_, entry| entry.expires_at > now);
            affected
        };
        for (object, generation) in affected {
            self.collaboration_emit_presence(&object, &generation)?;
        }
        Ok(())
    }

    pub fn collaboration_clear_presence(&self) -> Result<()> {
        let affected = {
            let mut presence = self
                .collaboration_presence
                .lock()
                .map_err(|_| invalid("collaboration_unavailable"))?;
            let affected = presence
                .values()
                .map(|entry| (entry.object_id.clone(), entry.generation.clone()))
                .collect::<BTreeSet<_>>();
            presence.clear();
            affected
        };
        for (object, generation) in affected {
            self.collaboration_emit_presence(&object, &generation)?;
        }
        Ok(())
    }

    fn collaboration_emit_presence(&self, object: &str, generation: &str) -> Result<()> {
        let mut members = self
            .collaboration_presence
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?
            .values()
            .filter(|entry| entry.object_id == object && entry.generation == generation)
            .map(|entry| entry.member.clone())
            .collect::<Vec<_>>();
        members.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        self.emit(
            "collaboration:presence",
            "sync",
            serde_json::to_value(crate::sync::CollaborationPresenceEvent {
                object_id: object.into(),
                generation: generation.into(),
                presence: members,
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        Ok(())
    }

    pub fn collaboration_update_object(
        &self,
        id: &str,
        patch: ObjectPatch,
        device: &DeviceKeys,
        secrets: &SyncSecrets,
    ) -> Result<MutationResult<WorkspaceObject>> {
        crate::sync::identifier(id)?;
        let _lock = self.write_lock("collaboration_metadata")?;
        self.collaboration_recover(id, secrets)?;
        let journal = self.sync_journal()?;
        let (epoch, descriptor) = Self::sync_document(&journal, id)
            .filter(|(_, descriptor)| descriptor.mode == DocumentMode::Text)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if !secrets
            .authorized_workspace_writers
            .contains(device.device_id())
            && !secrets
                .authorized_object_writers
                .contains(&(id.into(), device.device_id().into()))
        {
            return Err(invalid("sync_writer_not_authorized"));
        }
        let state = self
            .collaboration_state(id)?
            .filter(|state| state.generation == descriptor.generation)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if state.deleted {
            return Err(invalid("collaboration_lifecycle_conflict"));
        }
        if state.revision != patch.expected_revision {
            return Err(invalid("revision_conflict"));
        }
        let current = crate::sync::decode(&state.materialized, 0, MAX_TEXT_BYTES)?;
        let ParsedMarkdown::Managed(object) = markdown::parse_markdown(&state.path, &current)
        else {
            return Err(invalid("collaboration_invalid_managed_document"));
        };
        let mut target = object.clone();
        if let Some(title) = patch.title {
            if title.trim().is_empty() {
                return Err(invalid("title_required"));
            }
            target.title = title.trim().into();
        }
        for key in patch.remove_properties {
            if !matches!(key.as_str(), "id" | "type" | "created" | "updated") {
                target.properties.remove(&key);
            }
        }
        for (key, value) in patch.properties {
            if !matches!(key.as_str(), "id" | "type" | "created" | "updated") {
                target.properties.insert(key, value);
            }
        }
        normalize_domain_properties(&target.object_type, &mut target.properties)?;
        let metadata = metadata_patches(&object, &target)?;
        let document = TextDocument::restore(&state.update)?;
        let mut updates = Vec::new();
        if let Some(body) = patch.body
            && body != document.text()
        {
            updates.push(document.replace_text(&body)?.1);
        }
        if metadata.is_empty() && updates.is_empty() {
            return Ok(MutationResult {
                value: object,
                revision: state.revision,
                durability: "committed".into(),
                index_status: IndexStatus::Updated,
                warnings: Vec::new(),
            });
        }
        let transaction = CollaborativeTransaction {
            version: 1,
            object_id: id.into(),
            generation: descriptor.generation.clone(),
            updates,
            metadata,
            format: None,
            lifecycle: None,
        };
        transaction.validate()?;
        let key = secrets
            .objects
            .get(&(id.into(), epoch))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let operation = self.collaboration_seal_operation(
            key,
            &journal.workspace_id,
            id,
            device,
            (
                epoch,
                &journal.access_revision,
                &descriptor.generation,
                OperationKind::Metadata,
            ),
            &serde_json::to_vec(&transaction).map_err(|_| invalid("sync_serialize_failed"))?,
        )?;
        let state =
            self.collaboration_apply_transaction_locked(&transaction, &operation, true, None)?;
        let bytes = crate::sync::decode(&state.materialized, 0, MAX_TEXT_BYTES)?;
        let ParsedMarkdown::Managed(mut value) = markdown::parse_markdown(&state.path, &bytes)
        else {
            return Err(invalid("collaboration_invalid_managed_document"));
        };
        // The CRDT draft remains the lossless source for editor text when canonical Markdown
        // normalization cannot represent an otherwise valid trailing convention exactly.
        value.body = TextDocument::restore(&state.update)?.text();
        Ok(MutationResult {
            value,
            revision: state.revision,
            durability: "committed".into(),
            index_status: IndexStatus::Updated,
            warnings: Vec::new(),
        })
    }

    pub fn collaboration_move_object(
        &self,
        id: &str,
        destination: &str,
        expected_revision: &str,
        device: &DeviceKeys,
        secrets: &SyncSecrets,
    ) -> Result<MutationResult<WorkspaceObject>> {
        crate::sync::identifier(id)?;
        resolve_for_write(&self.root, destination, "collaboration_move")?;
        let _lock = self.write_lock("collaboration_move")?;
        self.collaboration_recover(id, secrets)?;
        let journal = self.sync_journal()?;
        let (epoch, descriptor) = Self::sync_document(&journal, id)
            .filter(|(_, descriptor)| descriptor.mode == DocumentMode::Text)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if !secrets
            .authorized_workspace_writers
            .contains(device.device_id())
            && !secrets
                .authorized_object_writers
                .contains(&(id.into(), device.device_id().into()))
        {
            return Err(invalid("sync_writer_not_authorized"));
        }
        let state = self
            .collaboration_state(id)?
            .filter(|state| state.generation == descriptor.generation)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if state.deleted {
            return Err(invalid("collaboration_lifecycle_conflict"));
        }
        if state.revision != expected_revision {
            return Err(invalid("revision_conflict"));
        }
        let transaction = CollaborativeTransaction {
            version: 1,
            object_id: id.into(),
            generation: descriptor.generation.clone(),
            updates: Vec::new(),
            metadata: Vec::new(),
            format: None,
            lifecycle: Some(CollaborativeLifecycleChange::Move {
                from: state.path.clone(),
                to: destination.into(),
                expected_revision: expected_revision.into(),
            }),
        };
        transaction.validate()?;
        let key = secrets
            .objects
            .get(&(id.into(), epoch))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let operation = self.collaboration_seal_operation(
            key,
            &journal.workspace_id,
            id,
            device,
            (
                epoch,
                &journal.access_revision,
                &descriptor.generation,
                OperationKind::Metadata,
            ),
            &serde_json::to_vec(&transaction).map_err(|_| invalid("sync_serialize_failed"))?,
        )?;
        let state =
            self.collaboration_apply_transaction_locked(&transaction, &operation, true, None)?;
        let bytes = crate::sync::decode(&state.materialized, 0, MAX_TEXT_BYTES)?;
        let ParsedMarkdown::Managed(mut value) = markdown::parse_markdown(&state.path, &bytes)
        else {
            return Err(invalid("collaboration_invalid_managed_document"));
        };
        value.body = TextDocument::restore(&state.update)?.text();
        Ok(MutationResult {
            value,
            revision: state.revision,
            durability: "committed".into(),
            index_status: IndexStatus::Updated,
            warnings: Vec::new(),
        })
    }

    pub fn collaboration_delete_object(
        &self,
        id: &str,
        expected_revision: &str,
        device: &DeviceKeys,
        secrets: &SyncSecrets,
    ) -> Result<MutationResult<WorkspaceObject>> {
        crate::sync::identifier(id)?;
        let _lock = self.write_lock("collaboration_delete")?;
        self.collaboration_recover(id, secrets)?;
        let journal = self.sync_journal()?;
        let (epoch, descriptor) = Self::sync_document(&journal, id)
            .filter(|(_, descriptor)| descriptor.mode == DocumentMode::Text)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if !secrets
            .authorized_workspace_writers
            .contains(device.device_id())
            && !secrets
                .authorized_object_writers
                .contains(&(id.into(), device.device_id().into()))
        {
            return Err(invalid("sync_writer_not_authorized"));
        }
        let state = self
            .collaboration_state(id)?
            .filter(|state| state.generation == descriptor.generation)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if state.deleted || state.revision != expected_revision {
            return Err(invalid(if state.deleted {
                "collaboration_lifecycle_conflict"
            } else {
                "revision_conflict"
            }));
        }
        let bytes = crate::sync::decode(&state.materialized, 0, MAX_TEXT_BYTES)?;
        let ParsedMarkdown::Managed(mut value) = markdown::parse_markdown(&state.path, &bytes)
        else {
            return Err(invalid("collaboration_invalid_managed_document"));
        };
        value.body = TextDocument::restore(&state.update)?.text();
        let transaction = CollaborativeTransaction {
            version: 1,
            object_id: id.into(),
            generation: descriptor.generation.clone(),
            updates: Vec::new(),
            metadata: Vec::new(),
            format: None,
            lifecycle: Some(CollaborativeLifecycleChange::Delete {
                path: state.path.clone(),
                expected_revision: expected_revision.into(),
                tombstone_id: uuid::Uuid::new_v4().to_string(),
            }),
        };
        transaction.validate()?;
        let key = secrets
            .objects
            .get(&(id.into(), epoch))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let operation = self.collaboration_seal_operation(
            key,
            &journal.workspace_id,
            id,
            device,
            (
                epoch,
                &journal.access_revision,
                &descriptor.generation,
                OperationKind::Metadata,
            ),
            &serde_json::to_vec(&transaction).map_err(|_| invalid("sync_serialize_failed"))?,
        )?;
        self.collaboration_apply_transaction_locked(&transaction, &operation, true, None)?;
        Ok(MutationResult {
            value,
            revision: state.revision,
            durability: "committed".into(),
            index_status: IndexStatus::Updated,
            warnings: Vec::new(),
        })
    }

    pub fn collaboration_submit(
        &self,
        input: CollaborationSubmitInput,
        device: &DeviceKeys,
        secrets: &SyncSecrets,
    ) -> Result<CollaborationReceipt> {
        crate::sync::identifier(&input.batch_id)?;
        let binding = self
            .collaboration_sessions
            .lock()
            .map_err(|_| invalid("collaboration_unavailable"))?
            .get(&input.session_id)
            .cloned()
            .ok_or_else(|| invalid("collaboration_session_closed"))?;
        let (object, generation, read_only) = binding;
        if read_only {
            return Err(invalid("sync_writer_not_authorized"));
        }
        if generation != input.generation {
            return Err(invalid("collaboration_stale_generation"));
        }
        if !secrets
            .authorized_workspace_writers
            .contains(device.device_id())
            && !secrets
                .authorized_object_writers
                .contains(&(object.clone(), device.device_id().into()))
        {
            return Err(invalid("sync_writer_not_authorized"));
        }
        let change = CollaborativeChange {
            version: 1,
            object_id: object.clone(),
            generation,
            updates: input.updates,
        };
        change.validate()?;
        let digest = markdown::revision(
            &serde_json::to_vec(&change).map_err(|_| invalid("sync_serialize_failed"))?,
        );
        let _lock = self.write_lock("collaboration_submit")?;
        self.collaboration_recover(&object, secrets)?;
        let receipt_path = format!(
            ".noura/sync/collaboration/{object}/batches/{}.json",
            input.batch_id
        );
        if let Some(bytes) = read_optional(&self.sync_path(&receipt_path)?)? {
            let (prior_digest, revision): (String, String) = serde_json::from_slice(&bytes)
                .map_err(|_| invalid("collaboration_invalid_receipt"))?;
            if prior_digest != digest {
                return Err(invalid("collaboration_batch_reused"));
            }
            return Ok(CollaborationReceipt { revision });
        }
        let journal = self.sync_journal()?;
        if journal.transition.as_ref().is_some_and(|transition| {
            transition
                .checkpoints
                .iter()
                .any(|cp| cp.payload.object_id == object)
        }) {
            return Err(invalid("collaboration_transition_pending"));
        }
        let (epoch, descriptor) = Self::sync_document(&journal, &object)
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if descriptor.generation != change.generation || descriptor.mode != DocumentMode::Text {
            return Err(invalid("collaboration_stale_generation"));
        }
        let key = secrets
            .objects
            .get(&(object.clone(), epoch))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let operation = self.collaboration_seal_operation(
            key,
            &journal.workspace_id,
            &object,
            device,
            (
                epoch,
                &journal.access_revision,
                &change.generation,
                OperationKind::Text,
            ),
            &serde_json::to_vec(&change).map_err(|_| invalid("sync_serialize_failed"))?,
        )?;
        let state = self.collaboration_apply_change(
            &change,
            &operation,
            true,
            Some((input.batch_id, digest)),
            None,
        )?;
        Ok(CollaborationReceipt {
            revision: state.revision,
        })
    }

    pub fn collaboration_apply_remote(
        &self,
        operation: &EncryptedOperation,
        key: &ObjectKey,
        trusted_key: &str,
        secrets: &SyncSecrets,
        sequence: &str,
    ) -> Result<ApplyOutcome> {
        sync_cursor(sequence)?;
        self.sync_check_workspace(operation)?;
        if !secrets.writer_is_authorized(
            &operation.policy_revision,
            &operation.object_id,
            &operation.device_id,
        ) {
            return Err(invalid("sync_writer_not_authorized"));
        }
        if !matches!(
            operation.kind,
            Some(OperationKind::Text | OperationKind::Metadata)
        ) {
            return Err(invalid("collaboration_unsupported_operation"));
        }
        let plaintext = self.collaboration_operation_plaintext(operation, key, trusted_key)?;
        if operation.kind == Some(OperationKind::Metadata) {
            let change: CollaborativeTransaction = serde_json::from_slice(&plaintext)
                .map_err(|_| invalid("collaboration_invalid_transaction"))?;
            change.validate()?;
            if change.object_id != operation.object_id
                || Some(&change.generation) != operation.generation.as_ref()
            {
                return Err(invalid("collaboration_stale_generation"));
            }
            let _lock = self.write_lock("collaboration_apply")?;
            self.collaboration_recover(&operation.object_id, secrets)?;
            if let Some(outcome) = self.collaboration_remote_receipt(operation)? {
                return Ok(outcome);
            }
            return match self.collaboration_apply_transaction_locked(
                &change,
                operation,
                false,
                Some(sequence),
            ) {
                Ok(_) => Ok(ApplyOutcome::Applied),
                Err(error)
                    if matches!(
                        error.code.as_str(),
                        "collaboration_metadata_conflict"
                            | "collaboration_format_conflict"
                            | "collaboration_lifecycle_conflict"
                            | "collaboration_destination_collision"
                    ) =>
                {
                    self.collaboration_record_metadata_review(
                        operation,
                        &change.generation,
                        match error.code.as_str() {
                            "collaboration_format_conflict" => "text_format_conflict",
                            "collaboration_lifecycle_conflict" => "lifecycle_conflict",
                            "collaboration_destination_collision" => "destination_collision",
                            _ => "same_field_conflict",
                        },
                    )?;
                    Ok(ApplyOutcome::Conflict)
                }
                Err(error) => Err(error),
            };
        }
        let change: CollaborativeChange = serde_json::from_slice(&plaintext)
            .map_err(|_| invalid("collaboration_invalid_update"))?;
        change.validate()?;
        if change.object_id != operation.object_id
            || Some(&change.generation) != operation.generation.as_ref()
        {
            return Err(invalid("collaboration_stale_generation"));
        }
        let _lock = self.write_lock("collaboration_apply")?;
        self.collaboration_recover(&operation.object_id, secrets)?;
        if let Some(outcome) = self.collaboration_remote_receipt(operation)? {
            return Ok(outcome);
        }
        match self.collaboration_apply_change(&change, operation, false, None, Some(sequence)) {
            Ok(_) => Ok(ApplyOutcome::Applied),
            Err(error) if error.code == "collaboration_lifecycle_conflict" => {
                self.collaboration_record_metadata_review(
                    operation,
                    &change.generation,
                    "edit_after_deletion",
                )?;
                Ok(ApplyOutcome::Conflict)
            }
            Err(error) => Err(error),
        }
    }

    fn collaboration_remote_receipt(
        &self,
        operation: &EncryptedOperation,
    ) -> Result<Option<ApplyOutcome>> {
        let journal = self.sync_journal()?;
        let Some(receipt) = journal.receipts.get(&operation.operation_id) else {
            return Ok(None);
        };
        let digest = markdown::revision(
            &serde_json::to_vec(operation).map_err(|_| invalid("sync_serialize_failed"))?,
        );
        if receipt.digest != digest {
            return Err(invalid("sync_operation_id_reused"));
        }
        Ok(Some(receipt.outcome.clone()))
    }

    fn collaboration_apply_change(
        &self,
        change: &CollaborativeChange,
        operation: &EncryptedOperation,
        outgoing: bool,
        batch: Option<(String, String)>,
        sequence: Option<&str>,
    ) -> Result<DocumentState> {
        let state = self
            .collaboration_state(&change.object_id)?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if state.generation != change.generation {
            return Err(invalid("collaboration_stale_generation"));
        }
        if state.deleted {
            return Err(invalid("collaboration_lifecycle_conflict"));
        }
        let current = read_optional(&self.sync_file_path(&state.path)?)?
            .ok_or_else(|| invalid("collaboration_file_missing"))?;
        if markdown::revision(&current) != state.revision {
            return Err(invalid("collaboration_external_change"));
        }
        let prior = TextDocument::restore(&state.update)?;
        let next = prior.apply(&change.updates)?;
        let delta = next.diff(&prior.state_vector())?;
        let bytes = render(&state.path, &current, &state.object_id, &next.text())?;
        let mut next = DocumentState {
            version: 2,
            object_id: state.object_id,
            generation: state.generation,
            covered_sequence: state.covered_sequence,
            path: state.path,
            deleted: false,
            revision: markdown::revision(&bytes),
            materialized: STANDARD.encode(&bytes),
            update: next.state()?,
            acknowledged: state.acknowledged,
            pending: state.pending,
            acknowledged_operations: state.acknowledged_operations,
        };
        if outgoing {
            if next.pending.len() >= 1_000 {
                return Err(invalid("collaboration_pending_limit"));
            }
            next.pending.push(PendingDocumentChange {
                operation_id: operation.operation_id.clone(),
                updates: change.updates.clone(),
                metadata: Vec::new(),
                format: None,
                lifecycle: None,
            });
        } else {
            let acknowledged = next
                .acknowledged
                .as_ref()
                .ok_or_else(|| invalid("collaboration_baseline_required"))?;
            let prior_bytes = crate::sync::decode(&acknowledged.materialized, 0, MAX_TEXT_BYTES)?;
            let candidate = TextDocument::restore(&acknowledged.update)?.apply(&change.updates)?;
            let materialized =
                render(&next.path, &prior_bytes, &next.object_id, &candidate.text())?;
            next.acknowledged = Some(DocumentSnapshot {
                sequence: sequence
                    .ok_or_else(|| invalid("collaboration_sequence_required"))?
                    .into(),
                path: Some(next.path.clone()),
                deleted: false,
                revision: markdown::revision(&materialized),
                materialized: STANDARD.encode(materialized),
                update: candidate.state()?,
            });
        }
        let (batch_id, batch_digest) =
            batch.map_or((None, None), |(id, digest)| (Some(id), Some(digest)));
        let intent = DocumentIntent {
            version: 1,
            base_revision: state.revision,
            next: next.clone(),
            operation: operation.clone(),
            outgoing,
            batch_id,
            batch_digest,
            external_snapshot: None,
            previous_path: None,
        };
        self.sync_write(&intent_path(&change.object_id)?, &Some(&intent))?;
        self.sync_write_file_checked(&next.path, &bytes, Some(&intent.base_revision))?;
        self.collaboration_finish(&intent)?;
        if is_markdown(&next.path) {
            self.reindex_raw_markdown(&next.path, &bytes, "collaboration_apply")?;
        }
        self.emit("collaboration:update", if outgoing { "application" } else { "sync" }, serde_json::json!({"objectId": next.object_id, "generation":next.generation,"update":delta,"revision":next.revision}));
        self.emit(
            "file:changed",
            "sync",
            serde_json::json!({"paths":[next.path]}),
        );
        Ok(next)
    }

    fn collaboration_apply_transaction_locked(
        &self,
        transaction: &CollaborativeTransaction,
        operation: &EncryptedOperation,
        outgoing: bool,
        sequence: Option<&str>,
    ) -> Result<DocumentState> {
        transaction.validate()?;
        let state = self
            .collaboration_state(&transaction.object_id)?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if state.generation != transaction.generation
            || operation.kind != Some(OperationKind::Metadata)
            || operation.generation.as_ref() != Some(&transaction.generation)
        {
            return Err(invalid("collaboration_stale_generation"));
        }
        let previous_path = state.path.clone();
        let (next_path, deleted, tombstone_path) = match &transaction.lifecycle {
            Some(CollaborativeLifecycleChange::Move {
                from,
                to,
                expected_revision,
            }) => {
                if state.deleted {
                    return Err(invalid("collaboration_lifecycle_conflict"));
                }
                self.sync_file_path(from)?;
                let destination = self.sync_file_path(to)?;
                if from != &state.path
                    || expected_revision != &state.revision
                    || is_markdown(from) != is_markdown(to)
                {
                    return Err(invalid("collaboration_lifecycle_conflict"));
                }
                if destination.exists() {
                    return Err(invalid("collaboration_destination_collision"));
                }
                (to.clone(), false, None)
            }
            Some(CollaborativeLifecycleChange::Delete {
                path,
                expected_revision,
                tombstone_id,
            }) => {
                if state.deleted || path != &state.path || expected_revision != &state.revision {
                    return Err(invalid("collaboration_lifecycle_conflict"));
                }
                self.sync_file_path(path)?;
                let trash = collaboration_trash_path(&transaction.object_id, tombstone_id, path)?;
                let destination = self.sync_path(&trash)?;
                if destination.exists() {
                    return Err(invalid("collaboration_destination_collision"));
                }
                (state.path.clone(), true, Some(trash))
            }
            None if state.deleted => {
                return Err(invalid("collaboration_lifecycle_conflict"));
            }
            None => (state.path.clone(), false, None),
        };
        let current = read_optional(&self.sync_file_path(&previous_path)?)?
            .ok_or_else(|| invalid("collaboration_file_missing"))?;
        if markdown::revision(&current) != state.revision {
            return Err(invalid("collaboration_external_change"));
        }
        let prior = TextDocument::restore(&state.update)?;
        let document = if transaction.updates.is_empty() {
            prior
        } else {
            prior.apply(&transaction.updates)?
        };
        let delta = (!transaction.updates.is_empty())
            .then(|| document.diff(&TextDocument::restore(&state.update)?.state_vector()))
            .transpose()?;
        let mut bytes = if transaction.updates.is_empty() {
            current.clone()
        } else {
            render(&state.path, &current, &state.object_id, &document.text())?
        };
        if !transaction.metadata.is_empty() {
            bytes = apply_metadata_patches(
                &state.path,
                &bytes,
                &state.object_id,
                &transaction.metadata,
            )?;
        }
        if let Some(format) = &transaction.format {
            bytes = apply_format_patch(&state.path, &bytes, format)?;
        }
        let mut next = DocumentState {
            version: 2,
            object_id: state.object_id,
            generation: state.generation,
            covered_sequence: state.covered_sequence,
            path: next_path.clone(),
            deleted,
            revision: markdown::revision(&bytes),
            materialized: STANDARD.encode(&bytes),
            update: document.state()?,
            acknowledged: state.acknowledged,
            pending: state.pending,
            acknowledged_operations: state.acknowledged_operations,
        };
        if outgoing {
            if next.pending.len() >= 1_000 {
                return Err(invalid("collaboration_pending_limit"));
            }
            next.pending.push(PendingDocumentChange {
                operation_id: operation.operation_id.clone(),
                updates: transaction.updates.clone(),
                metadata: transaction.metadata.clone(),
                format: transaction.format.clone(),
                lifecycle: transaction.lifecycle.clone(),
            });
        } else {
            let acknowledged = next
                .acknowledged
                .as_ref()
                .ok_or_else(|| invalid("collaboration_baseline_required"))?;
            let prior_bytes = crate::sync::decode(&acknowledged.materialized, 0, MAX_TEXT_BYTES)?;
            let prior_document = TextDocument::restore(&acknowledged.update)?;
            let acknowledged_document = if transaction.updates.is_empty() {
                prior_document
            } else {
                prior_document.apply(&transaction.updates)?
            };
            let mut materialized = if transaction.updates.is_empty() {
                prior_bytes
            } else {
                render(
                    &next.path,
                    &prior_bytes,
                    &next.object_id,
                    &acknowledged_document.text(),
                )?
            };
            if !transaction.metadata.is_empty() {
                materialized = apply_metadata_patches(
                    &next.path,
                    &materialized,
                    &next.object_id,
                    &transaction.metadata,
                )?;
            }
            if let Some(format) = &transaction.format {
                materialized = apply_format_patch(&next.path, &materialized, format)?;
            }
            next.acknowledged = Some(DocumentSnapshot {
                sequence: sequence
                    .ok_or_else(|| invalid("collaboration_sequence_required"))?
                    .into(),
                path: Some(next.path.clone()),
                deleted,
                revision: markdown::revision(&materialized),
                materialized: STANDARD.encode(materialized),
                update: acknowledged_document.state()?,
            });
        }
        self.collaboration_validate_recovery_state(&next)?;
        let intent = DocumentIntent {
            version: 1,
            base_revision: state.revision,
            next: next.clone(),
            operation: operation.clone(),
            outgoing,
            batch_id: None,
            batch_digest: None,
            external_snapshot: None,
            previous_path: (previous_path != next_path).then_some(previous_path.clone()),
        };
        self.sync_write(&intent_path(&transaction.object_id)?, &Some(&intent))?;
        self.collaboration_mutation_boundary(0)?;
        if let Some(trash_path) = tombstone_path.as_deref() {
            let source = self.sync_file_path(&previous_path)?;
            let trash = self.sync_path(trash_path)?;
            self.sync_prepare_parent(trash_path)?;
            std::fs::rename(&source, &trash).map_err(|_| invalid("collaboration_delete_failed"))?;
            sync_rename_parents(&source, &trash, "collaboration_delete")?;
            if let Ok(mut writes) = self.self_writes.lock() {
                writes.insert(previous_path.clone(), "<deleted>".into());
            }
        } else if previous_path == next_path {
            self.sync_write_file_checked(&next.path, &bytes, Some(&intent.base_revision))?;
        } else {
            let source = self.sync_file_path(&previous_path)?;
            let destination = self.sync_file_path(&next_path)?;
            self.sync_prepare_parent(&next_path)?;
            std::fs::rename(&source, &destination)
                .map_err(|_| invalid("collaboration_move_failed"))?;
            sync_rename_parents(&source, &destination, "collaboration_move")?;
            if let Ok(mut writes) = self.self_writes.lock() {
                writes.insert(previous_path.clone(), "<deleted>".into());
                writes.insert(next_path.clone(), next.revision.clone());
            }
        }
        self.collaboration_mutation_boundary(1)?;
        self.collaboration_finish(&intent)?;
        self.collaboration_mutation_boundary(2)?;
        if previous_path != next_path || deleted {
            let result = self
                .index
                .lock()
                .map_err(|_| lock_error("collaboration_move"))
                .and_then(|mut index| index.remove_path(&previous_path));
            let _ = self.index_outcome(result);
        }
        if !deleted && is_markdown(&next.path) {
            self.reindex_raw_markdown(&next.path, &bytes, "collaboration_metadata")?;
        }
        if let Some(delta) = delta {
            self.emit(
                "collaboration:update",
                if outgoing { "application" } else { "sync" },
                serde_json::json!({"objectId": next.object_id, "generation": next.generation, "update": delta, "revision": next.revision}),
            );
        }
        self.emit(
            if deleted {
                "object:deleted"
            } else if previous_path == next_path {
                "object:updated"
            } else {
                "object:moved"
            },
            if outgoing { "application" } else { "sync" },
            serde_json::json!({"id":next.object_id,"path":next.path,"from":previous_path,"to":next.path,"revision":next.revision,"deleted":deleted,"trashPath":tombstone_path}),
        );
        self.emit(
            "file:changed",
            "sync",
            serde_json::json!({"paths":[next.path]}),
        );
        Ok(next)
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

    pub(super) fn collaboration_generation_is_installed(
        &self,
        object: &str,
        generation: &str,
    ) -> Result<bool> {
        if self
            .collaboration_state(object)?
            .is_some_and(|state| state.generation == generation)
        {
            return Ok(true);
        }
        let path = format!(".noura/sync/collaboration/{object}/attachment.json");
        let Some(bytes) = read_optional(&self.sync_path(&path)?)? else {
            return Ok(false);
        };
        let state: AttachmentGenerationState = serde_json::from_slice(&bytes)
            .map_err(|_| invalid("collaboration_invalid_attachment_state"))?;
        if state.version != 1 || state.object_id != object {
            return Err(invalid("collaboration_invalid_attachment_state"));
        }
        Ok(state.generation == generation)
    }

    fn collaboration_validate_recovery_state(&self, state: &DocumentState) -> Result<()> {
        if state.version != 2
            || state.pending.len() > 1_000
            || state.acknowledged_operations.len() > 1_000
        {
            return Err(invalid("collaboration_invalid_state"));
        }
        let acknowledged = state
            .acknowledged
            .as_ref()
            .ok_or_else(|| invalid("collaboration_baseline_required"))?;
        sync_cursor(&acknowledged.sequence)?;
        let acknowledged_bytes =
            crate::sync::decode(&acknowledged.materialized, 0, MAX_TEXT_BYTES)?;
        let mut materialized_path = acknowledged
            .path
            .clone()
            .unwrap_or_else(|| state.path.clone());
        let mut materialized_deleted = acknowledged.deleted;
        self.sync_file_path(&materialized_path)?;
        let mut materialized = acknowledged_bytes.clone();
        let mut document = TextDocument::restore(&acknowledged.update)?;
        if markdown::revision(&acknowledged_bytes) != acknowledged.revision
            || render(
                &materialized_path,
                &acknowledged_bytes,
                &state.object_id,
                &document.text(),
            )? != acknowledged_bytes
        {
            return Err(invalid("collaboration_invalid_baseline"));
        }
        let mut seen = BTreeSet::new();
        for pending in &state.pending {
            crate::sync::identifier(&pending.operation_id)?;
            if !seen.insert(&pending.operation_id) {
                return Err(invalid("collaboration_invalid_state"));
            }
            if materialized_deleted {
                return Err(invalid("collaboration_invalid_state"));
            }
            if !pending.updates.is_empty() {
                document = document.apply(&pending.updates)?;
                materialized = render(
                    &materialized_path,
                    &materialized,
                    &state.object_id,
                    &document.text(),
                )?;
            }
            if !pending.metadata.is_empty() {
                materialized = apply_metadata_patches(
                    &materialized_path,
                    &materialized,
                    &state.object_id,
                    &pending.metadata,
                )?;
            }
            if let Some(format) = &pending.format {
                materialized = apply_format_patch(&materialized_path, &materialized, format)?;
            }
            if let Some(lifecycle) = &pending.lifecycle {
                match lifecycle {
                    CollaborativeLifecycleChange::Move {
                        from,
                        to,
                        expected_revision,
                    } => {
                        self.sync_file_path(from)?;
                        self.sync_file_path(to)?;
                        if from != &materialized_path
                            || from == to
                            || expected_revision != &markdown::revision(&materialized)
                        {
                            return Err(invalid("collaboration_invalid_state"));
                        }
                        materialized_path = to.clone();
                    }
                    CollaborativeLifecycleChange::Delete {
                        path,
                        expected_revision,
                        tombstone_id,
                    } => {
                        self.sync_file_path(path)?;
                        collaboration_trash_path(&state.object_id, tombstone_id, path)?;
                        if path != &materialized_path
                            || expected_revision != &markdown::revision(&materialized)
                        {
                            return Err(invalid("collaboration_invalid_state"));
                        }
                        materialized_deleted = true;
                    }
                }
            }
        }
        let acknowledged_ids = state
            .acknowledged_operations
            .iter()
            .map(|id| {
                crate::sync::identifier(id)?;
                Ok(id)
            })
            .collect::<Result<BTreeSet<_>>>()?;
        if acknowledged_ids.len() != state.acknowledged_operations.len()
            || seen.iter().any(|id| acknowledged_ids.contains(id))
            || document.state()? != state.update
            || materialized != crate::sync::decode(&state.materialized, 0, MAX_TEXT_BYTES)?
            || materialized_path != state.path
            || materialized_deleted != state.deleted
        {
            return Err(invalid("collaboration_invalid_state"));
        }
        Ok(())
    }

    /// Advance the recovery baseline only after the relay durably acknowledges this exact
    /// operation. A crash between this write and outbox removal is idempotently recoverable.
    pub(super) fn collaboration_acknowledge(
        &self,
        operation: &EncryptedOperation,
        sequence: &str,
    ) -> Result<()> {
        if !matches!(
            operation.kind,
            Some(OperationKind::Text | OperationKind::Metadata)
        ) {
            return Ok(());
        }
        let Some(generation) = operation.generation.as_ref() else {
            return Ok(());
        };
        let mut state = self
            .collaboration_state(&operation.object_id)?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if &state.generation != generation {
            return Err(invalid("collaboration_stale_generation"));
        }
        if state
            .acknowledged_operations
            .iter()
            .any(|id| id == &operation.operation_id)
        {
            return Ok(());
        }
        let pending = state
            .pending
            .first()
            .filter(|pending| pending.operation_id == operation.operation_id)
            .cloned()
            .ok_or_else(|| invalid("collaboration_pending_operation_required"))?;
        let acknowledged = state
            .acknowledged
            .as_ref()
            .ok_or_else(|| invalid("collaboration_baseline_required"))?;
        let mut acknowledged_path = acknowledged
            .path
            .clone()
            .unwrap_or_else(|| state.path.clone());
        let mut acknowledged_deleted = acknowledged.deleted;
        if acknowledged_deleted {
            return Err(invalid("collaboration_invalid_state"));
        }
        let prior_document = TextDocument::restore(&acknowledged.update)?;
        let document = if pending.updates.is_empty() {
            prior_document
        } else {
            prior_document.apply(&pending.updates)?
        };
        let prior_bytes = crate::sync::decode(&acknowledged.materialized, 0, MAX_TEXT_BYTES)?;
        let mut materialized = if pending.updates.is_empty() {
            prior_bytes
        } else {
            render(
                &acknowledged_path,
                &prior_bytes,
                &state.object_id,
                &document.text(),
            )?
        };
        if !pending.metadata.is_empty() {
            materialized = apply_metadata_patches(
                &acknowledged_path,
                &materialized,
                &state.object_id,
                &pending.metadata,
            )?;
        }
        if let Some(format) = &pending.format {
            materialized = apply_format_patch(&acknowledged_path, &materialized, format)?;
        }
        if let Some(lifecycle) = &pending.lifecycle {
            match lifecycle {
                CollaborativeLifecycleChange::Move {
                    from,
                    to,
                    expected_revision,
                } => {
                    if from != &acknowledged_path
                        || expected_revision != &markdown::revision(&materialized)
                    {
                        return Err(invalid("collaboration_invalid_state"));
                    }
                    acknowledged_path = to.clone();
                }
                CollaborativeLifecycleChange::Delete {
                    path,
                    expected_revision,
                    tombstone_id,
                } => {
                    collaboration_trash_path(&state.object_id, tombstone_id, path)?;
                    if path != &acknowledged_path
                        || expected_revision != &markdown::revision(&materialized)
                    {
                        return Err(invalid("collaboration_invalid_state"));
                    }
                    acknowledged_deleted = true;
                }
            }
        }
        state.acknowledged = Some(DocumentSnapshot {
            sequence: sequence.into(),
            path: Some(acknowledged_path),
            deleted: acknowledged_deleted,
            revision: markdown::revision(&materialized),
            materialized: STANDARD.encode(materialized),
            update: document.state()?,
        });
        state.pending.remove(0);
        state
            .acknowledged_operations
            .push(operation.operation_id.clone());
        if state.acknowledged_operations.len() > 1_000 {
            state.acknowledged_operations.remove(0);
        }
        self.collaboration_validate_recovery_state(&state)?;
        self.sync_write(&state_path(&state.object_id)?, &state)
    }
    fn collaboration_recover(&self, object: &str, secrets: &SyncSecrets) -> Result<()> {
        let Some(bytes) = read_optional(&self.sync_path(&intent_path(object)?)?)? else {
            return Ok(());
        };
        let intent: Option<DocumentIntent> =
            serde_json::from_slice(&bytes).map_err(|_| invalid("collaboration_invalid_intent"))?;
        let Some(intent) = intent else {
            return Ok(());
        };
        if intent.version != 1 || intent.next.object_id != object {
            return Err(invalid("collaboration_invalid_intent"));
        }
        // Workspace files are untrusted input even when they are recovery records.
        // Authenticate the operation and reconstruct its candidate before writing any bytes.
        self.sync_check_workspace(&intent.operation)?;
        let signer = secrets
            .trusted_devices
            .get(&intent.operation.device_id)
            .ok_or_else(|| invalid("sync_untrusted_device"))?;
        let key = secrets
            .objects
            .get(&(object.into(), intent.operation.epoch))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let plaintext = self.collaboration_operation_plaintext(&intent.operation, key, signer)?;
        let (change_object, change_generation, updates, metadata, format, lifecycle) =
            match intent.operation.kind {
                Some(OperationKind::Text) => {
                    let change: CollaborativeChange = serde_json::from_slice(&plaintext)
                        .map_err(|_| invalid("collaboration_invalid_intent"))?;
                    change.validate()?;
                    (
                        change.object_id,
                        change.generation,
                        change.updates,
                        Vec::new(),
                        None,
                        None,
                    )
                }
                Some(OperationKind::Metadata) => {
                    let change: CollaborativeTransaction = serde_json::from_slice(&plaintext)
                        .map_err(|_| invalid("collaboration_invalid_intent"))?;
                    change.validate()?;
                    (
                        change.object_id,
                        change.generation,
                        change.updates,
                        change.metadata,
                        change.format,
                        change.lifecycle,
                    )
                }
                _ => return Err(invalid("collaboration_invalid_intent")),
            };
        if change_object != object
            || intent.operation.object_id != object
            || intent.operation.generation.as_ref() != Some(&change_generation)
            || intent.next.generation != change_generation
            || !matches!(intent.next.version, 1 | 2)
            || intent.batch_id.is_some() != intent.batch_digest.is_some()
        {
            return Err(invalid("collaboration_invalid_intent"));
        }
        let materialized = crate::sync::decode(&intent.next.materialized, 0, MAX_TEXT_BYTES)?;
        let candidate = TextDocument::restore(&intent.next.update)?;
        if markdown::revision(&materialized) != intent.next.revision
            || (candidate.text() != body(&intent.next.path, &materialized, object)?
                && render(&intent.next.path, &materialized, object, &candidate.text())?
                    != materialized)
        {
            return Err(invalid("collaboration_invalid_intent"));
        }
        let prior = self
            .collaboration_state(object)?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if let Some(snapshot_id) = intent.external_snapshot.as_deref() {
            crate::sync::identifier(snapshot_id)?;
            if snapshot_id != intent.operation.operation_id {
                return Err(invalid("collaboration_invalid_external_snapshot"));
            }
            let snapshot: CollaborationExternalSnapshot = serde_json::from_slice(
                &read_optional(&self.sync_path(&format!(
                    ".noura/sync/collaboration/{object}/external/{snapshot_id}.json"
                ))?)?
                .ok_or_else(|| invalid("collaboration_external_snapshot_required"))?,
            )
            .map_err(|_| invalid("collaboration_invalid_external_snapshot"))?;
            let snapshot_bytes = crate::sync::decode(&snapshot.bytes, 0, MAX_TEXT_BYTES)?;
            if snapshot.version != 1
                || snapshot.object_id != object
                || snapshot.generation != intent.next.generation
                || snapshot.revision != intent.base_revision
                || markdown::revision(&snapshot_bytes) != snapshot.revision
            {
                return Err(invalid("collaboration_invalid_external_snapshot"));
            }
        }
        let prior_path = intent.previous_path.as_deref().unwrap_or(&intent.next.path);
        if prior.generation != intent.next.generation
            || prior.path != prior_path
            || prior.deleted
            || intent
                .previous_path
                .as_ref()
                .is_some_and(|path| path == &intent.next.path)
            || (intent.external_snapshot.is_none()
                && prior.revision != intent.base_revision
                && prior.revision != intent.next.revision)
        {
            return Err(invalid("collaboration_invalid_intent"));
        }
        let prior_document = TextDocument::restore(&prior.update)?;
        let reconstructed = if updates.is_empty() {
            prior_document
        } else {
            prior_document.apply(&updates)?
        };
        let prior_bytes = crate::sync::decode(&prior.materialized, 0, MAX_TEXT_BYTES)?;
        let mut reconstructed_bytes = if updates.is_empty() {
            prior_bytes
        } else {
            render(&prior.path, &prior_bytes, object, &reconstructed.text())?
        };
        if !metadata.is_empty() {
            reconstructed_bytes =
                apply_metadata_patches(&prior.path, &reconstructed_bytes, object, &metadata)?;
        }
        if let Some(format) = &format {
            reconstructed_bytes = apply_format_patch(&prior.path, &reconstructed_bytes, format)?;
        }
        let mut reconstructed_path = prior.path.clone();
        let mut reconstructed_deleted = false;
        let mut recovered_trash_path = None;
        if let Some(lifecycle) = &lifecycle {
            match lifecycle {
                CollaborativeLifecycleChange::Move {
                    from,
                    to,
                    expected_revision,
                } => {
                    self.sync_file_path(from)?;
                    self.sync_file_path(to)?;
                    if from != &reconstructed_path
                        || from == to
                        || expected_revision != &markdown::revision(&reconstructed_bytes)
                        || is_markdown(from) != is_markdown(to)
                    {
                        return Err(invalid("collaboration_invalid_intent"));
                    }
                    reconstructed_path = to.clone();
                }
                CollaborativeLifecycleChange::Delete {
                    path,
                    expected_revision,
                    tombstone_id,
                } => {
                    self.sync_file_path(path)?;
                    if path != &reconstructed_path
                        || expected_revision != &markdown::revision(&reconstructed_bytes)
                    {
                        return Err(invalid("collaboration_invalid_intent"));
                    }
                    recovered_trash_path =
                        Some(collaboration_trash_path(object, tombstone_id, path)?);
                    reconstructed_deleted = true;
                }
            }
        }
        if reconstructed.state()? != candidate.state()?
            || reconstructed_bytes != materialized
            || reconstructed_path != intent.next.path
            || reconstructed_deleted != intent.next.deleted
            || matches!(lifecycle, Some(CollaborativeLifecycleChange::Move { .. }))
                != intent.previous_path.is_some()
        {
            return Err(invalid("collaboration_invalid_intent"));
        }
        if let Some(trash_path) = recovered_trash_path.as_deref() {
            let source_path = self.sync_file_path(&intent.next.path)?;
            let trash_destination = self.sync_path(trash_path)?;
            let source = read_optional(&source_path)?;
            let trash = read_optional(&trash_destination)?;
            match (source, trash) {
                (Some(source), None) if markdown::revision(&source) == intent.base_revision => {
                    self.sync_prepare_parent(trash_path)?;
                    std::fs::rename(&source_path, &trash_destination)
                        .map_err(|_| invalid("collaboration_delete_failed"))?;
                    sync_rename_parents(&source_path, &trash_destination, "collaboration_delete")?;
                }
                (None, Some(trash)) if markdown::revision(&trash) == intent.next.revision => {}
                _ => return Err(invalid("collaboration_external_change")),
            }
            if let Ok(mut writes) = self.self_writes.lock() {
                writes.insert(intent.next.path.clone(), "<deleted>".into());
            }
        } else if let Some(previous_path) = intent.previous_path.as_deref() {
            let source_path = self.sync_file_path(previous_path)?;
            let destination_path = self.sync_file_path(&intent.next.path)?;
            let source = read_optional(&source_path)?;
            let destination = read_optional(&destination_path)?;
            match (source, destination) {
                (Some(source), None) if markdown::revision(&source) == intent.base_revision => {
                    self.sync_prepare_parent(&intent.next.path)?;
                    std::fs::rename(&source_path, &destination_path)
                        .map_err(|_| invalid("collaboration_move_failed"))?;
                    sync_rename_parents(&source_path, &destination_path, "collaboration_move")?;
                }
                (None, Some(destination))
                    if markdown::revision(&destination) == intent.next.revision => {}
                _ => return Err(invalid("collaboration_external_change")),
            }
            if let Ok(mut writes) = self.self_writes.lock() {
                writes.insert(previous_path.into(), "<deleted>".into());
                writes.insert(intent.next.path.clone(), intent.next.revision.clone());
            }
        } else {
            let current = read_optional(&self.sync_file_path(&intent.next.path)?)?
                .ok_or_else(|| invalid("collaboration_file_missing"))?;
            let revision = markdown::revision(&current);
            if revision == intent.base_revision {
                let next = crate::sync::decode(&intent.next.materialized, 0, MAX_TEXT_BYTES)?;
                if markdown::revision(&next) != intent.next.revision {
                    return Err(invalid("collaboration_invalid_intent"));
                }
                self.sync_write_file_checked(&intent.next.path, &next, Some(&revision))?;
            } else if revision != intent.next.revision {
                return Err(invalid("collaboration_external_change"));
            }
        }
        self.collaboration_finish(&intent)?;
        if intent.next.deleted {
            let result = self
                .index
                .lock()
                .map_err(|_| lock_error("collaboration_delete"))
                .and_then(|mut index| index.remove_path(&intent.next.path));
            let _ = self.index_outcome(result);
        } else if let Some(previous_path) = intent.previous_path.as_deref() {
            let result = self
                .index
                .lock()
                .map_err(|_| lock_error("collaboration_move"))
                .and_then(|mut index| index.remove_path(previous_path));
            let _ = self.index_outcome(result);
            if is_markdown(&intent.next.path) {
                self.reindex_raw_markdown(&intent.next.path, &materialized, "collaboration_move")?;
            }
        }
        Ok(())
    }
    fn collaboration_finish(&self, intent: &DocumentIntent) -> Result<()> {
        let next = &intent.next;
        self.sync_write(&state_path(&next.object_id)?, next)?;
        let mut journal = self.sync_journal()?;
        if intent.outgoing
            && !journal
                .outbox
                .iter()
                .any(|op| op.operation_id == intent.operation.operation_id)
        {
            journal.outbox.push(intent.operation.clone());
        }
        if !intent.outgoing {
            journal.receipts.insert(
                intent.operation.operation_id.clone(),
                Receipt {
                    digest: markdown::revision(
                        &serde_json::to_vec(&intent.operation)
                            .map_err(|_| invalid("sync_serialize_failed"))?,
                    ),
                    outcome: ApplyOutcome::Applied,
                    change_digest: None,
                },
            );
        }
        journal.objects.insert(
            next.object_id.clone(),
            ObjectState {
                path: next.path.clone(),
                revision: (!next.deleted).then(|| next.revision.clone()),
            },
        );
        self.sync_write(STATE_PATH, &journal)?;
        if let (Some(id), Some(digest)) = (&intent.batch_id, &intent.batch_digest) {
            crate::sync::identifier(id)?;
            self.sync_write_once(
                &format!(
                    ".noura/sync/collaboration/{}/batches/{id}.json",
                    next.object_id
                ),
                &(digest, &next.revision),
            )?;
        }
        if let Ok(mut writes) = self.self_writes.lock() {
            writes.insert(
                next.path.clone(),
                if next.deleted {
                    "<deleted>".into()
                } else {
                    next.revision.clone()
                },
            );
        }
        self.sync_write(
            &intent_path(&next.object_id)?,
            &Option::<DocumentIntent>::None,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{
        AccessMember, AccessObject, AccessPolicy, CheckpointContent, DocumentDescriptor,
        EncryptedCheckpoint, SyncCredentials, WorkspaceRole,
    };
    use zeroize::Zeroizing;
    struct Credentials;
    impl SyncCredentials for Credentials {
        fn read(&self, _: &str) -> Result<Zeroizing<String>> {
            Err(invalid("test_missing"))
        }
        fn write(&self, _: &str, _: &str) -> Result<()> {
            Ok(())
        }
    }
    fn copy_tree(source: &Path, destination: &Path) {
        std::fs::create_dir_all(destination).unwrap();
        for entry in std::fs::read_dir(source).unwrap() {
            let entry = entry.unwrap();
            let target = destination.join(entry.file_name());
            if entry.file_type().unwrap().is_dir() {
                copy_tree(&entry.path(), &target);
            } else {
                std::fs::copy(entry.path(), target).unwrap();
            }
        }
    }
    struct Fixture {
        directory: tempfile::TempDir,
        engine: WorkspaceEngine,
        device: DeviceKeys,
        secrets: SyncSecrets,
    }
    fn fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let engine = WorkspaceEngine::create_with_app_data(
            directory.path().join("workspace"),
            "Collaboration",
            directory.path().join("app"),
        )
        .unwrap();
        let device = DeviceKeys::create(&Credentials).unwrap();
        let key = ObjectKey::generate();
        let policy = AccessPolicy::sign(
            &engine.manifest().id,
            "1",
            None,
            &device,
            vec![AccessMember {
                account_id: "owner".into(),
                role: WorkspaceRole::Owner,
            }],
            vec![AccessObject {
                object_id: "text".into(),
                epoch: 2,
                grants: vec![],
                envelopes: vec![],
                document: Some(DocumentDescriptor {
                    generation: "generation-one".into(),
                    mode: DocumentMode::Text,
                }),
            }],
        )
        .unwrap();
        engine
            .sync_accept_access_policy("0", None, &policy)
            .unwrap();
        let bytes = b"\xef\xbb\xbfA\xf0\x9f\x98\x80\r\nsecond\r\n";
        let content = CheckpointContent {
            version: 1,
            object_id: "text".into(),
            generation: "generation-one".into(),
            content_revision: Some(markdown::revision(bytes)),
            change: FileChange {
                version: 1,
                path: "note.md".into(),
                previous_path: None,
                base_revision: None,
                content: Some(STANDARD.encode(bytes)),
                accepted_revisions: None,
                blob: None,
            },
        };
        let checkpoint = EncryptedCheckpoint::seal(&device, &key, &policy, "0", &content).unwrap();
        let secrets = SyncSecrets {
            objects: BTreeMap::from([(("text".into(), 2), key)]),
            trusted_devices: BTreeMap::from([(
                device.device_id().into(),
                device.signer().public_key(),
            )]),
            authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
            ..Default::default()
        };
        engine
            .collaboration_install_checkpoint(
                &checkpoint,
                secrets.objects.get(&("text".into(), 2)).unwrap(),
                &device.signer().public_key(),
                &device,
                &secrets,
            )
            .unwrap();
        Fixture {
            directory,
            engine,
            device,
            secrets,
        }
    }
    fn managed_fixture() -> Fixture {
        let directory = tempfile::tempdir().unwrap();
        let engine = WorkspaceEngine::create_with_app_data(
            directory.path().join("workspace"),
            "Managed collaboration",
            directory.path().join("app"),
        )
        .unwrap();
        let device = DeviceKeys::create(&Credentials).unwrap();
        let key = ObjectKey::generate();
        let policy = AccessPolicy::sign(
            &engine.manifest().id,
            "1",
            None,
            &device,
            vec![AccessMember {
                account_id: "owner".into(),
                role: WorkspaceRole::Owner,
            }],
            vec![AccessObject {
                object_id: "note_01j00000000000000000000000".into(),
                epoch: 2,
                grants: vec![],
                envelopes: vec![],
                document: Some(DocumentDescriptor {
                    generation: "generation-one".into(),
                    mode: DocumentMode::Text,
                }),
            }],
        )
        .unwrap();
        engine
            .sync_accept_access_policy("0", None, &policy)
            .unwrap();
        let object = WorkspaceObject {
            id: "note_01j00000000000000000000000".into(),
            object_type: "note".into(),
            title: "Original".into(),
            body: "Body\n".into(),
            relative_path: "note.md".into(),
            revision: String::new(),
            created: Some("2026-09-07T00:00:00Z".into()),
            updated: Some("2026-09-07T00:00:00Z".into()),
            properties: BTreeMap::from([(
                "status".into(),
                serde_json::Value::String("todo".into()),
            )]),
        };
        let bytes = markdown::serialize_object(&object).unwrap();
        let content = CheckpointContent {
            version: 1,
            object_id: "note_01j00000000000000000000000".into(),
            generation: "generation-one".into(),
            content_revision: Some(markdown::revision(&bytes)),
            change: FileChange {
                version: 1,
                path: "note.md".into(),
                previous_path: None,
                base_revision: None,
                content: Some(STANDARD.encode(&bytes)),
                accepted_revisions: None,
                blob: None,
            },
        };
        let checkpoint = EncryptedCheckpoint::seal(&device, &key, &policy, "0", &content).unwrap();
        let secrets = SyncSecrets {
            objects: BTreeMap::from([(("note_01j00000000000000000000000".into(), 2), key)]),
            trusted_devices: BTreeMap::from([(
                device.device_id().into(),
                device.signer().public_key(),
            )]),
            authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
            ..Default::default()
        };
        engine
            .collaboration_install_checkpoint(
                &checkpoint,
                secrets
                    .objects
                    .get(&("note_01j00000000000000000000000".into(), 2))
                    .unwrap(),
                &device.signer().public_key(),
                &device,
                &secrets,
            )
            .unwrap();
        Fixture {
            directory,
            engine,
            device,
            secrets,
        }
    }
    fn open(f: &Fixture) -> CollaborationSession {
        f.engine
            .collaboration_open(
                CollaborationOpenInput {
                    relative_path: "note.md".into(),
                },
                &f.device,
                &f.secrets,
            )
            .unwrap()
            .unwrap()
    }
    fn edit(session: &CollaborationSession, text: &str) -> CollaborationSubmitInput {
        let (_, update) = TextDocument::restore(&session.update)
            .unwrap()
            .replace_text(text)
            .unwrap();
        CollaborationSubmitInput {
            session_id: session.session_id.clone(),
            generation: session.generation.clone(),
            batch_id: uuid::Uuid::new_v4().to_string(),
            updates: vec![update],
        }
    }

    #[test]
    fn checkpoint_blob_round_trip_installs_large_text_without_inline_plaintext() {
        let directory = tempfile::tempdir().unwrap();
        let engine = WorkspaceEngine::create_with_app_data(
            directory.path().join("workspace"),
            "Large checkpoint",
            directory.path().join("app"),
        )
        .unwrap();
        let device = DeviceKeys::create(&Credentials).unwrap();
        let public = device.signer().public_key();
        let bytes = vec![b'a'; MAX_TEXT_BYTES];
        std::fs::write(engine.root.join("large.txt"), &bytes).unwrap();
        let mut capture_secrets = SyncSecrets {
            trusted_devices: BTreeMap::from([(device.device_id().into(), public.clone())]),
            authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
            ..Default::default()
        };
        engine
            .sync_capture_workspace(&device, &mut capture_secrets)
            .unwrap();
        let object_id = engine.sync_outbox().unwrap()[0].object_id.clone();
        let generation = "large-generation";
        let key = ObjectKey::generate();
        let (content, blob, mode) = engine
            .collaboration_prepare_checkpoint_content(&object_id, generation, &key)
            .unwrap();
        assert_eq!(mode, DocumentMode::Text);
        let blob = blob.expect("large checkpoint must use a blob");
        assert!(content.change.content.is_none());
        assert_eq!(content.change.blob.as_ref(), Some(&blob));
        let policy = AccessPolicy::sign(
            &engine.manifest().id,
            "1",
            None,
            &device,
            vec![AccessMember {
                account_id: "owner".into(),
                role: WorkspaceRole::Owner,
            }],
            vec![AccessObject {
                object_id: object_id.clone(),
                epoch: 2,
                grants: vec![],
                envelopes: vec![],
                document: Some(DocumentDescriptor {
                    generation: generation.into(),
                    mode: DocumentMode::Text,
                }),
            }],
        )
        .unwrap();
        engine
            .sync_accept_access_policy("0", None, &policy)
            .unwrap();
        let checkpoint = EncryptedCheckpoint::seal(&device, &key, &policy, "0", &content).unwrap();
        let secrets = SyncSecrets {
            objects: BTreeMap::from([((object_id.clone(), 2), key)]),
            trusted_devices: BTreeMap::from([(device.device_id().into(), public.clone())]),
            authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
            ..Default::default()
        };
        engine
            .collaboration_install_checkpoint(
                &checkpoint,
                secrets.objects.get(&(object_id.clone(), 2)).unwrap(),
                &public,
                &device,
                &secrets,
            )
            .unwrap();
        assert_eq!(std::fs::read(engine.root.join("large.txt")).unwrap(), bytes);
        assert!(
            engine
                .collaboration_generation_is_installed(&object_id, generation)
                .unwrap()
        );
    }

    #[test]
    fn large_live_updates_use_durable_encrypted_blobs_and_apply_on_a_peer() {
        let mut f = fixture();
        f.secrets
            .historical_workspace_writers
            .insert("1".into(), BTreeSet::from([f.device.device_id().into()]));
        let peer_root = f.directory.path().join("peer");
        copy_tree(&f.engine.root, &peer_root);
        let peer =
            WorkspaceEngine::open_with_app_data(&peer_root, f.directory.path().join("peer-app"))
                .unwrap();
        let session = open(&f);
        let text = "large 😀\n".repeat(100_000);
        f.engine
            .collaboration_submit(edit(&session, &text), &f.device, &f.secrets)
            .unwrap();
        let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
        assert!(serde_json::to_vec(&operation).unwrap().len() < 1024 * 1024);
        let key = f.secrets.objects.get(&("text".into(), 2)).unwrap();
        let blob = f
            .engine
            .collaboration_operation_blob(&operation, key, &f.device.signer().public_key())
            .unwrap()
            .expect("large live update must use a blob");
        let mut source = f.engine.sync_blob_file(&blob, false).unwrap();
        blob.verify(&mut source).unwrap();
        source.rewind().unwrap();
        let mut destination = peer.sync_blob_file(&blob, true).unwrap();
        std::io::copy(&mut source, &mut destination).unwrap();
        destination.sync_all().unwrap();
        drop(destination);
        assert_eq!(
            peer.collaboration_apply_remote(
                &operation,
                key,
                &f.device.signer().public_key(),
                &f.secrets,
                "1",
            )
            .unwrap(),
            ApplyOutcome::Applied
        );
        assert_eq!(
            TextDocument::restore(&peer.collaboration_state("text").unwrap().unwrap().update)
                .unwrap()
                .text(),
            text
        );
    }

    #[test]
    fn unsupported_text_rotates_into_attachment_mode_and_archives_old_operations() {
        let directory = tempfile::tempdir().unwrap();
        let engine = WorkspaceEngine::create_with_app_data(
            directory.path().join("workspace"),
            "Attachment checkpoint",
            directory.path().join("app"),
        )
        .unwrap();
        let device = DeviceKeys::create(&Credentials).unwrap();
        let public = device.signer().public_key();
        let bytes = vec![0xff; MAX_TEXT_BYTES + 1];
        std::fs::write(engine.root.join("binary.dat"), &bytes).unwrap();
        let mut capture_secrets = SyncSecrets {
            trusted_devices: BTreeMap::from([(device.device_id().into(), public.clone())]),
            authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
            ..Default::default()
        };
        engine
            .sync_capture_workspace(&device, &mut capture_secrets)
            .unwrap();
        let operation = engine.sync_outbox().unwrap()[0].clone();
        let object_id = operation.object_id.clone();
        let generation = "attachment-generation";
        let key = ObjectKey::generate();
        let (content, blob, mode) = engine
            .collaboration_prepare_checkpoint_content(&object_id, generation, &key)
            .unwrap();
        assert_eq!(mode, DocumentMode::Attachment);
        assert!(blob.is_some());
        assert!(content.change.content.is_none());
        let policy = AccessPolicy::sign(
            &engine.manifest().id,
            "1",
            None,
            &device,
            vec![AccessMember {
                account_id: "owner".into(),
                role: WorkspaceRole::Owner,
            }],
            vec![AccessObject {
                object_id: object_id.clone(),
                epoch: 2,
                grants: vec![],
                envelopes: vec![],
                document: Some(DocumentDescriptor {
                    generation: generation.into(),
                    mode,
                }),
            }],
        )
        .unwrap();
        engine
            .sync_accept_access_policy("0", None, &policy)
            .unwrap();
        let checkpoint = EncryptedCheckpoint::seal(&device, &key, &policy, "0", &content).unwrap();
        let mut secrets = SyncSecrets {
            objects: BTreeMap::from([((object_id.clone(), 2), key)]),
            trusted_devices: BTreeMap::from([(device.device_id().into(), public.clone())]),
            authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
            ..Default::default()
        };
        engine
            .collaboration_install_checkpoint(
                &checkpoint,
                secrets.objects.get(&(object_id.clone(), 2)).unwrap(),
                &public,
                &device,
                &secrets,
            )
            .unwrap();
        assert_eq!(
            std::fs::read(engine.root.join("binary.dat")).unwrap(),
            bytes
        );
        assert!(engine.sync_outbox().unwrap().is_empty());
        assert!(
            engine
                .collaboration_generation_is_installed(&object_id, generation)
                .unwrap()
        );
        assert!(
            engine
                .collaboration_open(
                    CollaborationOpenInput {
                        relative_path: "binary.dat".into(),
                    },
                    &device,
                    &secrets,
                )
                .unwrap()
                .is_none()
        );
        let mut edited = bytes;
        edited[0] = 0xfe;
        std::fs::write(engine.root.join("binary.dat"), &edited).unwrap();
        engine
            .sync_capture_workspace(&device, &mut secrets)
            .unwrap();
        let operations = engine.sync_outbox().unwrap();
        assert_eq!(operations.len(), 1);
        let operation = &operations[0];
        assert_eq!(operation.version, 2);
        assert_eq!(operation.generation.as_deref(), Some(generation));
        assert_eq!(operation.kind, Some(OperationKind::File));
        assert_eq!(operation.epoch, 2);
        let plaintext = operation
            .open(
                secrets.objects.get(&(object_id.clone(), 2)).unwrap(),
                &public,
            )
            .unwrap();
        let change: FileChange = serde_json::from_slice(&plaintext).unwrap();
        assert!(change.content.is_none());
        assert!(change.blob.is_some());
        engine.sync_acknowledge(operation, "1").unwrap();

        let inline_edit = vec![0xfd; 1024];
        std::fs::write(engine.root.join("binary.dat"), &inline_edit).unwrap();
        engine
            .sync_capture_workspace(&device, &mut secrets)
            .unwrap();
        let operations = engine.sync_outbox().unwrap();
        assert_eq!(operations.len(), 1);
        let operation = &operations[0];
        assert_eq!(operation.version, 2);
        assert_eq!(operation.generation.as_deref(), Some(generation));
        assert_eq!(operation.kind, Some(OperationKind::File));
        let plaintext = operation
            .open(
                secrets.objects.get(&(object_id.clone(), 2)).unwrap(),
                &public,
            )
            .unwrap();
        let change: FileChange = serde_json::from_slice(&plaintext).unwrap();
        assert!(change.blob.is_none());
        assert!(change.content.is_some());
    }

    #[test]
    fn writer_object_activation_is_durable_and_installs_its_fresh_generation() {
        let directory = tempfile::tempdir().unwrap();
        let engine = WorkspaceEngine::create_with_app_data(
            directory.path().join("workspace"),
            "Object activation",
            directory.path().join("app"),
        )
        .unwrap();
        let device = DeviceKeys::create(&Credentials).unwrap();
        let public = device.signer().public_key();
        let policy = AccessPolicy::sign(
            &engine.manifest().id,
            "1",
            None,
            &device,
            vec![AccessMember {
                account_id: "writer".into(),
                role: WorkspaceRole::Owner,
            }],
            vec![],
        )
        .unwrap();
        engine
            .sync_accept_access_policy("0", None, &policy)
            .unwrap();
        engine
            .sync_record_access_authorization(
                "1",
                &BTreeSet::from([device.device_id().into()]),
                &BTreeSet::new(),
            )
            .unwrap();
        std::fs::write(engine.root.join("created.txt"), b"created by a writer").unwrap();
        let mut secrets = SyncSecrets {
            trusted_devices: BTreeMap::from([(device.device_id().into(), public.clone())]),
            authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
            historical_workspace_writers: BTreeMap::from([(
                "1".into(),
                BTreeSet::from([device.device_id().into()]),
            )]),
            ..Default::default()
        };
        engine
            .sync_capture_workspace(&device, &mut secrets)
            .unwrap();
        let operation = engine.sync_outbox().unwrap().pop().unwrap();
        let key = secrets
            .objects
            .get(&(operation.object_id.clone(), 1))
            .unwrap();
        let generation = "writer-generation";
        let (content, blob, mode) = engine
            .collaboration_prepare_checkpoint_content(&operation.object_id, generation, key)
            .unwrap();
        assert!(blob.is_none());
        let checkpoint = crate::sync::EncryptedCheckpoint::seal_activation(
            &device,
            key,
            &engine.manifest().id,
            "1",
            "0",
            &content,
        )
        .unwrap();
        let envelope = crate::sync::PolicyEnvelope::from(
            device
                .wrap_key(
                    &engine.manifest().id,
                    &operation.object_id,
                    1,
                    device.device_id(),
                    &device.recipient(),
                    key,
                )
                .unwrap(),
        );
        let capability =
            crate::sync::WorkspaceCapability::sign(&engine.manifest().id, &device).unwrap();
        let activation = crate::sync::ObjectActivation::sign(
            &device,
            &capability,
            "1",
            "0",
            DocumentDescriptor {
                generation: generation.into(),
                mode,
            },
            vec![envelope],
            checkpoint.clone(),
            vec![],
        )
        .unwrap();
        engine
            .sync_prepare_activation(&activation, &public)
            .unwrap();
        drop(engine);
        let engine = WorkspaceEngine::open_with_app_data(
            directory.path().join("workspace"),
            directory.path().join("app"),
        )
        .unwrap();
        assert_eq!(
            engine
                .sync_pending_activation()
                .unwrap()
                .unwrap()
                .digest()
                .unwrap(),
            activation.digest().unwrap()
        );
        engine
            .sync_abandon_activation(&activation.activation_id)
            .unwrap();
        assert_eq!(engine.sync_outbox().unwrap().len(), 1);
        engine
            .sync_prepare_activation(&activation, &public)
            .unwrap();
        let activation_status = engine.sync_status().unwrap().activation.unwrap();
        assert_eq!(activation_status.activation_id, activation.activation_id);
        assert_eq!(activation_status.object_id, operation.object_id);
        assert_eq!(activation_status.path.as_deref(), Some("created.txt"));
        assert!(!activation_status.installed);
        engine.sync_accept_activation(&activation, &public).unwrap();
        engine
            .collaboration_install_checkpoint(&checkpoint, key, &public, &device, &secrets)
            .unwrap();
        assert!(engine.sync_status().unwrap().activation.unwrap().installed);
        engine
            .sync_finish_activation(&activation.activation_id)
            .unwrap();
        assert!(engine.sync_status().unwrap().activation.is_none());
        assert!(engine.sync_pending_activation().unwrap().is_none());
        assert!(engine.sync_outbox().unwrap().is_empty());
        let session = engine
            .collaboration_open(
                CollaborationOpenInput {
                    relative_path: "created.txt".into(),
                },
                &device,
                &secrets,
            )
            .unwrap()
            .unwrap();
        assert_eq!(session.generation, generation);
        assert_eq!(
            TextDocument::restore(&session.update).unwrap().text(),
            "created by a writer"
        );
        let invalid_presence = engine
            .collaboration_seal_presence(
                crate::sync::CollaborationPresenceInput {
                    session_id: session.session_id.clone(),
                    anchor: STANDARD.encode([1, 2, 3]),
                    head: STANDARD.encode([4, 5, 6]),
                },
                "relay-session",
                1,
                &device,
                &secrets,
            )
            .unwrap_err();
        assert_eq!(invalid_presence.code, "collaboration_invalid_presence");
        let presence = engine
            .collaboration_seal_presence(
                crate::sync::CollaborationPresenceInput {
                    session_id: session.session_id,
                    // Yjs relative position for the end of root text `content`.
                    anchor: "AQdjb250ZW50AA==".into(),
                    head: "AQdjb250ZW50AA==".into(),
                },
                "relay-session",
                1,
                &device,
                &secrets,
            )
            .unwrap();
        assert_eq!(presence.object_id, operation.object_id);
        assert_eq!(presence.generation, generation);
        assert_eq!(presence.epoch, 1);
        assert_eq!(presence.session_id, "relay-session");
        presence.verify(&public).unwrap();

        let remote = DeviceKeys::create(&Credentials).unwrap();
        secrets
            .trusted_devices
            .insert(remote.device_id().into(), remote.signer().public_key());
        let remote_presence = crate::sync::EncryptedPresence::seal(
            &remote,
            secrets
                .objects
                .get(&(operation.object_id.clone(), 1))
                .unwrap(),
            &crate::sync::PresenceContext {
                workspace_id: &engine.manifest().id,
                object_id: &operation.object_id,
                generation,
                epoch: 1,
                session_id: "remote-session",
                sequence: 1,
            },
            &crate::sync::PresenceSelection {
                anchor: "AQdjb250ZW50AA==".into(),
                head: "AQdjb250ZW50AA==".into(),
            },
        )
        .unwrap();
        let mut events = engine.subscribe();
        engine
            .collaboration_receive_presence(&remote_presence, &secrets)
            .unwrap();
        let received = events.try_recv().unwrap();
        assert_eq!(received.event_type, "collaboration:presence");
        assert_eq!(
            received.payload["presence"][0]["deviceId"],
            remote.device_id()
        );
        engine
            .collaboration_receive_presence(&remote_presence, &secrets)
            .unwrap();
        assert!(events.try_recv().is_err());
        engine
            .collaboration_remove_presence(remote.device_id(), "remote-session")
            .unwrap();
        let removed = events.try_recv().unwrap();
        assert_eq!(removed.payload["presence"], serde_json::json!([]));
    }

    #[test]
    fn closing_one_window_does_not_close_another_windows_lease() {
        let f = fixture();
        let left = open(&f);
        let right = open(&f);
        assert_ne!(left.session_id, right.session_id);
        f.engine.collaboration_close(&left.session_id).unwrap();
        f.engine
            .collaboration_submit(edit(&right, "another window"), &f.device, &f.secrets)
            .unwrap();
        assert_eq!(
            std::fs::read(f.directory.path().join("workspace/note.md")).unwrap(),
            "\u{feff}another window".as_bytes()
        );
    }

    #[test]
    fn managed_body_and_metadata_commit_as_one_generation_bound_operation() {
        let mut f = managed_fixture();
        let session = open(&f);
        let result = f
            .engine
            .collaboration_update_object(
                "note_01j00000000000000000000000",
                ObjectPatch {
                    title: Some("Renamed".into()),
                    body: Some("Changed body 😀\n".into()),
                    properties: BTreeMap::from([(
                        "priority".into(),
                        serde_json::Value::String("high".into()),
                    )]),
                    remove_properties: vec!["status".into()],
                    expected_revision: session.revision,
                },
                &f.device,
                &f.secrets,
            )
            .unwrap();
        assert_eq!(result.value.title, "Renamed");
        assert_eq!(result.value.body, "Changed body 😀\n");
        assert_eq!(result.value.properties["priority"], "high");
        assert!(!result.value.properties.contains_key("status"));
        let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
        assert_eq!(operation.kind, Some(OperationKind::Metadata));
        assert_eq!(operation.generation.as_deref(), Some("generation-one"));
        let pending = f
            .engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        assert_eq!(pending.pending.len(), 1);
        assert!(!pending.pending[0].updates.is_empty());
        assert_eq!(pending.pending[0].metadata.len(), 3);
        f.engine.collaboration_acknowledge(&operation, "1").unwrap();
        let reopened = open(&f);
        assert_eq!(reopened.status, CollaborationStatus::Synced);
        assert_eq!(
            TextDocument::restore(&reopened.update).unwrap().text(),
            "Changed body 😀\n"
        );
        let remote = DeviceKeys::create(&Credentials).unwrap();
        f.secrets
            .trusted_devices
            .insert(remote.device_id().into(), remote.signer().public_key());
        f.secrets
            .historical_workspace_writers
            .insert("1".into(), BTreeSet::from([remote.device_id().into()]));
        let remote_change = CollaborativeTransaction {
            version: 1,
            object_id: "note_01j00000000000000000000000".into(),
            generation: "generation-one".into(),
            updates: Vec::new(),
            metadata: vec![CollaborativeMetadataPatch {
                field: "property:status".into(),
                expected: CollaborativeMetadataValue::Missing,
                value: CollaborativeMetadataValue::Value {
                    value: serde_json::Value::String("done".into()),
                },
            }],
            format: None,
            lifecycle: None,
        };
        let key = f
            .secrets
            .objects
            .get(&("note_01j00000000000000000000000".into(), 2))
            .unwrap();
        let remote_operation = remote
            .signer()
            .seal_for_document(
                key,
                &f.engine.manifest().id,
                "note_01j00000000000000000000000",
                remote.device_id(),
                (2, "1", "generation-one", OperationKind::Metadata),
                &serde_json::to_vec(&remote_change).unwrap(),
            )
            .unwrap();
        assert_eq!(
            f.engine
                .collaboration_apply_remote(
                    &remote_operation,
                    key,
                    &remote.signer().public_key(),
                    &f.secrets,
                    "2",
                )
                .unwrap(),
            ApplyOutcome::Applied
        );
        let bytes = std::fs::read(f.engine.root.join("note.md")).unwrap();
        let ParsedMarkdown::Managed(object) = markdown::parse_markdown("note.md", &bytes) else {
            panic!("managed object expected")
        };
        assert_eq!(object.properties["status"], "done");
    }

    #[test]
    fn same_field_metadata_races_preserve_the_losing_operation_for_review() {
        let mut f = managed_fixture();
        let session = open(&f);
        f.engine
            .collaboration_update_object(
                "note_01j00000000000000000000000",
                ObjectPatch {
                    title: None,
                    body: None,
                    properties: BTreeMap::from([(
                        "priority".into(),
                        serde_json::Value::String("high".into()),
                    )]),
                    remove_properties: Vec::new(),
                    expected_revision: session.revision,
                },
                &f.device,
                &f.secrets,
            )
            .unwrap();
        let remote = DeviceKeys::create(&Credentials).unwrap();
        f.secrets
            .trusted_devices
            .insert(remote.device_id().into(), remote.signer().public_key());
        f.secrets
            .historical_workspace_writers
            .insert("1".into(), BTreeSet::from([remote.device_id().into()]));
        let change = CollaborativeTransaction {
            version: 1,
            object_id: "note_01j00000000000000000000000".into(),
            generation: "generation-one".into(),
            updates: Vec::new(),
            metadata: vec![CollaborativeMetadataPatch {
                field: "property:priority".into(),
                expected: CollaborativeMetadataValue::Missing,
                value: CollaborativeMetadataValue::Value {
                    value: serde_json::Value::String("low".into()),
                },
            }],
            format: None,
            lifecycle: None,
        };
        let key = f
            .secrets
            .objects
            .get(&("note_01j00000000000000000000000".into(), 2))
            .unwrap();
        let operation = remote
            .signer()
            .seal_for_document(
                key,
                &f.engine.manifest().id,
                "note_01j00000000000000000000000",
                remote.device_id(),
                (2, "1", "generation-one", OperationKind::Metadata),
                &serde_json::to_vec(&change).unwrap(),
            )
            .unwrap();
        assert_eq!(
            f.engine
                .collaboration_apply_remote(
                    &operation,
                    key,
                    &remote.signer().public_key(),
                    &f.secrets,
                    "1",
                )
                .unwrap(),
            ApplyOutcome::Conflict
        );
        let bytes = std::fs::read(f.engine.root.join("note.md")).unwrap();
        let ParsedMarkdown::Managed(object) = markdown::parse_markdown("note.md", &bytes) else {
            panic!("managed object expected")
        };
        assert_eq!(object.properties["priority"], "high");
        assert!(
            f.engine
                .sync_path(&format!(
                    ".noura/sync/collaboration/note_01j00000000000000000000000/reviews/{}.metadata.json",
                    operation.operation_id
                ))
                .unwrap()
                .exists()
        );
        assert_eq!(
            f.engine.sync_journal().unwrap().receipts[&operation.operation_id].outcome,
            ApplyOutcome::Conflict
        );
    }

    #[test]
    fn managed_moves_are_generation_bound_and_advance_the_acknowledged_path() {
        let f = managed_fixture();
        let session = open(&f);
        let result = f
            .engine
            .collaboration_move_object(
                "note_01j00000000000000000000000",
                "moved/note.md",
                &session.revision,
                &f.device,
                &f.secrets,
            )
            .unwrap();
        assert_eq!(result.value.id, "note_01j00000000000000000000000");
        assert_eq!(result.value.relative_path, "moved/note.md");
        assert!(!f.engine.root.join("note.md").exists());
        assert!(f.engine.root.join("moved/note.md").exists());
        let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
        assert_eq!(operation.kind, Some(OperationKind::Metadata));
        assert_eq!(operation.generation.as_deref(), Some("generation-one"));
        let pending = f
            .engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        assert!(matches!(
            pending.pending[0].lifecycle,
            Some(CollaborativeLifecycleChange::Move { ref from, ref to, .. })
                if from == "note.md" && to == "moved/note.md"
        ));
        let reopened = f
            .engine
            .collaboration_open(
                CollaborationOpenInput {
                    relative_path: "moved/note.md".into(),
                },
                &f.device,
                &f.secrets,
            )
            .unwrap()
            .unwrap();
        assert_eq!(reopened.object_id, "note_01j00000000000000000000000");
        f.engine.collaboration_acknowledge(&operation, "1").unwrap();
        let acknowledged = f
            .engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        assert!(acknowledged.pending.is_empty());
        assert_eq!(
            acknowledged.acknowledged.unwrap().path.as_deref(),
            Some("moved/note.md")
        );
    }

    #[test]
    fn remote_moves_apply_and_destination_collisions_are_preserved_for_review() {
        for collision in [false, true] {
            let mut f = managed_fixture();
            let remote = DeviceKeys::create(&Credentials).unwrap();
            f.secrets
                .trusted_devices
                .insert(remote.device_id().into(), remote.signer().public_key());
            f.secrets
                .historical_workspace_writers
                .insert("1".into(), BTreeSet::from([remote.device_id().into()]));
            let state = f
                .engine
                .collaboration_state("note_01j00000000000000000000000")
                .unwrap()
                .unwrap();
            let change = CollaborativeTransaction {
                version: 1,
                object_id: state.object_id.clone(),
                generation: state.generation.clone(),
                updates: Vec::new(),
                metadata: Vec::new(),
                format: None,
                lifecycle: Some(CollaborativeLifecycleChange::Move {
                    from: state.path.clone(),
                    to: "remote/note.md".into(),
                    expected_revision: state.revision.clone(),
                }),
            };
            let key = f
                .secrets
                .objects
                .get(&(state.object_id.clone(), 2))
                .unwrap();
            let operation = remote
                .signer()
                .seal_for_document(
                    key,
                    &f.engine.manifest().id,
                    &state.object_id,
                    remote.device_id(),
                    (2, "1", &state.generation, OperationKind::Metadata),
                    &serde_json::to_vec(&change).unwrap(),
                )
                .unwrap();
            if collision {
                std::fs::create_dir_all(f.engine.root.join("remote")).unwrap();
                std::fs::write(f.engine.root.join("remote/note.md"), b"occupied").unwrap();
            }
            let outcome = f
                .engine
                .collaboration_apply_remote(
                    &operation,
                    key,
                    &remote.signer().public_key(),
                    &f.secrets,
                    "1",
                )
                .unwrap();
            if collision {
                assert_eq!(outcome, ApplyOutcome::Conflict);
                assert!(f.engine.root.join("note.md").exists());
                assert_eq!(
                    std::fs::read(f.engine.root.join("remote/note.md")).unwrap(),
                    b"occupied"
                );
                assert!(
                    f.engine
                        .sync_path(&format!(
                            ".noura/sync/collaboration/{}/reviews/{}.metadata.json",
                            state.object_id, operation.operation_id
                        ))
                        .unwrap()
                        .exists()
                );
            } else {
                assert_eq!(outcome, ApplyOutcome::Applied);
                assert!(!f.engine.root.join("note.md").exists());
                assert!(f.engine.root.join("remote/note.md").exists());
                assert_eq!(
                    f.engine
                        .collaboration_state(&state.object_id)
                        .unwrap()
                        .unwrap()
                        .path,
                    "remote/note.md"
                );
            }
        }
    }

    #[test]
    fn managed_move_recovers_every_durable_boundary_after_restart() {
        for boundary in 0..3 {
            let f = managed_fixture();
            let workspace = f.directory.path().join("workspace");
            let app_data = f.directory.path().join("app");
            let state = f
                .engine
                .collaboration_state("note_01j00000000000000000000000")
                .unwrap()
                .unwrap();
            f.engine.collaboration_set_mutation_fault(boundary);
            assert_eq!(
                f.engine
                    .collaboration_move_object(
                        &state.object_id,
                        "recovered/note.md",
                        &state.revision,
                        &f.device,
                        &f.secrets,
                    )
                    .unwrap_err()
                    .code,
                "collaboration_test_interrupted"
            );
            let device = f.device;
            let secrets = f.secrets;
            drop(f.engine);
            let engine = WorkspaceEngine::open_with_app_data(&workspace, &app_data).unwrap();
            engine
                .collaboration_recover("note_01j00000000000000000000000", &secrets)
                .unwrap();
            assert!(!workspace.join("note.md").exists());
            assert!(workspace.join("recovered/note.md").exists());
            let recovered = engine
                .collaboration_open(
                    CollaborationOpenInput {
                        relative_path: "recovered/note.md".into(),
                    },
                    &device,
                    &secrets,
                )
                .unwrap()
                .unwrap();
            assert_eq!(recovered.object_id, "note_01j00000000000000000000000");
            assert_eq!(engine.sync_outbox().unwrap().len(), 1);
        }
    }

    #[test]
    fn managed_deletion_preserves_pending_drafts_until_tombstone_acknowledgment() {
        let f = managed_fixture();
        let session = open(&f);
        let edited = f
            .engine
            .collaboration_update_object(
                "note_01j00000000000000000000000",
                ObjectPatch {
                    title: None,
                    body: Some("Unsent draft\n".into()),
                    properties: BTreeMap::new(),
                    remove_properties: Vec::new(),
                    expected_revision: session.revision,
                },
                &f.device,
                &f.secrets,
            )
            .unwrap();
        let deleted = f
            .engine
            .collaboration_delete_object(
                "note_01j00000000000000000000000",
                &edited.revision,
                &f.device,
                &f.secrets,
            )
            .unwrap();
        assert_eq!(deleted.value.body, "Unsent draft\n");
        assert!(!f.engine.root.join("note.md").exists());
        let operations = f.engine.sync_outbox().unwrap();
        assert_eq!(operations.len(), 2);
        let delete_operation = operations.last().unwrap();
        let key = f
            .secrets
            .objects
            .get(&("note_01j00000000000000000000000".into(), 2))
            .unwrap();
        let transaction: CollaborativeTransaction = serde_json::from_slice(
            &delete_operation
                .open(key, &f.device.signer().public_key())
                .unwrap(),
        )
        .unwrap();
        let Some(CollaborativeLifecycleChange::Delete {
            path, tombstone_id, ..
        }) = transaction.lifecycle
        else {
            panic!("delete lifecycle expected")
        };
        let trash =
            collaboration_trash_path("note_01j00000000000000000000000", &tombstone_id, &path)
                .unwrap();
        assert!(f.engine.root.join(trash).exists());
        let state = f
            .engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        assert!(state.deleted);
        assert_eq!(state.pending.len(), 2);
        assert_eq!(
            TextDocument::restore(&state.update).unwrap().text(),
            "Unsent draft\n"
        );
        assert!(
            f.engine
                .collaboration_open(
                    CollaborationOpenInput {
                        relative_path: "note.md".into(),
                    },
                    &f.device,
                    &f.secrets,
                )
                .unwrap()
                .is_none()
        );
        f.engine
            .collaboration_acknowledge(&operations[0], "1")
            .unwrap();
        let pending_delete = f
            .engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        assert!(pending_delete.deleted);
        assert_eq!(pending_delete.pending.len(), 1);
        assert_eq!(
            TextDocument::restore(&pending_delete.acknowledged.unwrap().update)
                .unwrap()
                .text(),
            "Unsent draft\n"
        );
        f.engine
            .collaboration_acknowledge(delete_operation, "2")
            .unwrap();
        let acknowledged = f
            .engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        assert!(acknowledged.pending.is_empty());
        assert!(acknowledged.acknowledged.unwrap().deleted);
    }

    #[test]
    fn remote_delete_conflicting_with_a_local_draft_enters_review() {
        let mut f = managed_fixture();
        let baseline = open(&f);
        f.engine
            .collaboration_update_object(
                "note_01j00000000000000000000000",
                ObjectPatch {
                    title: None,
                    body: Some("Retained locally\n".into()),
                    properties: BTreeMap::new(),
                    remove_properties: Vec::new(),
                    expected_revision: baseline.revision.clone(),
                },
                &f.device,
                &f.secrets,
            )
            .unwrap();
        let remote = DeviceKeys::create(&Credentials).unwrap();
        f.secrets
            .trusted_devices
            .insert(remote.device_id().into(), remote.signer().public_key());
        f.secrets
            .historical_workspace_writers
            .insert("1".into(), BTreeSet::from([remote.device_id().into()]));
        let change = CollaborativeTransaction {
            version: 1,
            object_id: "note_01j00000000000000000000000".into(),
            generation: "generation-one".into(),
            updates: Vec::new(),
            metadata: Vec::new(),
            format: None,
            lifecycle: Some(CollaborativeLifecycleChange::Delete {
                path: "note.md".into(),
                expected_revision: baseline.revision,
                tombstone_id: uuid::Uuid::new_v4().to_string(),
            }),
        };
        let key = f
            .secrets
            .objects
            .get(&("note_01j00000000000000000000000".into(), 2))
            .unwrap();
        let operation = remote
            .signer()
            .seal_for_document(
                key,
                &f.engine.manifest().id,
                "note_01j00000000000000000000000",
                remote.device_id(),
                (2, "1", "generation-one", OperationKind::Metadata),
                &serde_json::to_vec(&change).unwrap(),
            )
            .unwrap();
        assert_eq!(
            f.engine
                .collaboration_apply_remote(
                    &operation,
                    key,
                    &remote.signer().public_key(),
                    &f.secrets,
                    "1",
                )
                .unwrap(),
            ApplyOutcome::Conflict
        );
        assert!(f.engine.root.join("note.md").exists());
        let retained = f
            .engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        assert!(!retained.deleted);
        assert_eq!(
            TextDocument::restore(&retained.update).unwrap().text(),
            "Retained locally\n"
        );
        assert_eq!(retained.pending.len(), 1);
    }

    #[test]
    fn remote_delete_tombstones_a_clean_document_and_replay_is_idempotent() {
        let mut f = managed_fixture();
        let baseline = open(&f);
        let remote = DeviceKeys::create(&Credentials).unwrap();
        f.secrets
            .trusted_devices
            .insert(remote.device_id().into(), remote.signer().public_key());
        f.secrets
            .historical_workspace_writers
            .insert("1".into(), BTreeSet::from([remote.device_id().into()]));
        let change = CollaborativeTransaction {
            version: 1,
            object_id: "note_01j00000000000000000000000".into(),
            generation: "generation-one".into(),
            updates: Vec::new(),
            metadata: Vec::new(),
            format: None,
            lifecycle: Some(CollaborativeLifecycleChange::Delete {
                path: "note.md".into(),
                expected_revision: baseline.revision,
                tombstone_id: uuid::Uuid::new_v4().to_string(),
            }),
        };
        let key = f
            .secrets
            .objects
            .get(&(change.object_id.clone(), 2))
            .unwrap();
        let operation = remote
            .signer()
            .seal_for_document(
                key,
                &f.engine.manifest().id,
                &change.object_id,
                remote.device_id(),
                (2, "1", &change.generation, OperationKind::Metadata),
                &serde_json::to_vec(&change).unwrap(),
            )
            .unwrap();
        assert_eq!(
            f.engine
                .collaboration_apply_remote(
                    &operation,
                    key,
                    &remote.signer().public_key(),
                    &f.secrets,
                    "1",
                )
                .unwrap(),
            ApplyOutcome::Applied
        );
        assert!(!f.engine.root.join("note.md").exists());
        let state = f
            .engine
            .collaboration_state(&change.object_id)
            .unwrap()
            .unwrap();
        assert!(state.deleted);
        assert!(state.acknowledged.unwrap().deleted);
        assert_eq!(
            f.engine
                .collaboration_apply_remote(
                    &operation,
                    key,
                    &remote.signer().public_key(),
                    &f.secrets,
                    "1",
                )
                .unwrap(),
            ApplyOutcome::Applied
        );
    }

    #[test]
    fn managed_delete_recovers_every_durable_boundary_after_restart() {
        for boundary in 0..3 {
            let f = managed_fixture();
            let workspace = f.directory.path().join("workspace");
            let app_data = f.directory.path().join("app");
            let state = f
                .engine
                .collaboration_state("note_01j00000000000000000000000")
                .unwrap()
                .unwrap();
            f.engine.collaboration_set_mutation_fault(boundary);
            assert_eq!(
                f.engine
                    .collaboration_delete_object(
                        &state.object_id,
                        &state.revision,
                        &f.device,
                        &f.secrets,
                    )
                    .unwrap_err()
                    .code,
                "collaboration_test_interrupted"
            );
            let device = f.device;
            let secrets = f.secrets;
            drop(f.engine);
            let engine = WorkspaceEngine::open_with_app_data(&workspace, &app_data).unwrap();
            engine
                .collaboration_recover("note_01j00000000000000000000000", &secrets)
                .unwrap();
            assert!(!workspace.join("note.md").exists());
            let recovered = engine
                .collaboration_state("note_01j00000000000000000000000")
                .unwrap()
                .unwrap();
            assert!(recovered.deleted);
            assert_eq!(recovered.pending.len(), 1);
            assert!(
                engine
                    .collaboration_open(
                        CollaborationOpenInput {
                            relative_path: "note.md".into(),
                        },
                        &device,
                        &secrets,
                    )
                    .unwrap()
                    .is_none()
            );
            assert_eq!(engine.sync_outbox().unwrap().len(), 1);
        }
    }

    #[test]
    fn external_managed_saves_translate_to_recoverable_native_transactions() {
        let mut f = managed_fixture();
        let path = f.engine.root.join("note.md");
        let original = std::fs::read(&path).unwrap();
        let ParsedMarkdown::Managed(mut external) = markdown::parse_markdown("note.md", &original)
        else {
            panic!("managed object expected")
        };
        external.title = "Changed outside".into();
        external.body = "External body 😀\n".into();
        external.properties.insert(
            "priority".into(),
            serde_json::Value::String("medium".into()),
        );
        let external_bytes = markdown::serialize_object(&external).unwrap();
        std::fs::write(&path, &external_bytes).unwrap();
        f.engine
            .sync_capture_workspace(&f.device, &mut f.secrets)
            .unwrap();
        let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
        assert_eq!(operation.kind, Some(OperationKind::Metadata));
        let snapshot: CollaborationExternalSnapshot = serde_json::from_slice(
            &std::fs::read(
                f.engine
                    .sync_path(&format!(
                        ".noura/sync/collaboration/note_01j00000000000000000000000/external/{}.json",
                        operation.operation_id
                    ))
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(STANDARD.decode(snapshot.bytes).unwrap(), external_bytes);
        let reopened = open(&f);
        assert_eq!(
            TextDocument::restore(&reopened.update).unwrap().text(),
            "External body 😀"
        );
        let materialized = std::fs::read(&path).unwrap();
        let ParsedMarkdown::Managed(materialized) =
            markdown::parse_markdown("note.md", &materialized)
        else {
            panic!("managed object expected")
        };
        assert_eq!(materialized.title, "Changed outside");
        assert_eq!(materialized.properties["priority"], "medium");
    }

    #[test]
    fn invalid_external_managed_saves_and_deletes_are_preserved_for_review() {
        let mut f = managed_fixture();
        let path = f.engine.root.join("note.md");
        let invalid = b"---\nid: forged\ntype: note\ntitle: Forged\n---\n\nbytes";
        std::fs::write(&path, invalid).unwrap();
        f.engine
            .sync_capture_workspace(&f.device, &mut f.secrets)
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), invalid);
        assert!(f.engine.sync_outbox().unwrap().is_empty());
        let reviews = f
            .engine
            .sync_path(".noura/sync/collaboration/note_01j00000000000000000000000/reviews")
            .unwrap();
        assert_eq!(std::fs::read_dir(&reviews).unwrap().count(), 1);
        std::fs::remove_file(&path).unwrap();
        f.engine
            .sync_capture_workspace(&f.device, &mut f.secrets)
            .unwrap();
        assert!(!path.exists());
        assert_eq!(std::fs::read_dir(reviews).unwrap().count(), 2);
    }

    #[test]
    fn external_raw_format_changes_preserve_bom_and_newline_intent() {
        let mut f = fixture();
        let path = f.engine.root.join("note.md");
        let external = "A😀\nsecond\n".as_bytes();
        std::fs::write(&path, external).unwrap();
        f.engine
            .sync_capture_workspace(&f.device, &mut f.secrets)
            .unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), external);
        let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
        assert_eq!(operation.kind, Some(OperationKind::Metadata));
        let key = f.secrets.objects.get(&("text".into(), 2)).unwrap();
        let transaction: CollaborativeTransaction = serde_json::from_slice(
            &operation
                .open(key, &f.device.signer().public_key())
                .unwrap(),
        )
        .unwrap();
        assert!(transaction.updates.is_empty());
        assert!(transaction.metadata.is_empty());
        assert_eq!(
            transaction.format,
            Some(CollaborativeFormatPatch {
                expected: CollaborativeTextFormat {
                    has_bom: true,
                    uses_crlf: true,
                },
                value: CollaborativeTextFormat {
                    has_bom: false,
                    uses_crlf: false,
                },
            })
        );
        f.engine.collaboration_acknowledge(&operation, "1").unwrap();
        assert_eq!(open(&f).status, CollaborationStatus::Synced);
    }

    #[test]
    fn external_translation_recovers_each_durable_boundary_after_restart() {
        for boundary in 0..3 {
            let mut f = managed_fixture();
            let workspace = f.directory.path().join("workspace");
            let app_data = f.directory.path().join("app");
            let path = workspace.join("note.md");
            let bytes = std::fs::read(&path).unwrap();
            let ParsedMarkdown::Managed(mut external) = markdown::parse_markdown("note.md", &bytes)
            else {
                panic!("managed object expected")
            };
            external.title = format!("External boundary {boundary}");
            external.body = format!("Recovered {boundary}\n");
            std::fs::write(&path, markdown::serialize_object(&external).unwrap()).unwrap();
            f.engine.collaboration_set_mutation_fault(boundary);
            assert_eq!(
                f.engine
                    .sync_capture_workspace(&f.device, &mut f.secrets)
                    .unwrap_err()
                    .code,
                "collaboration_test_interrupted"
            );
            let device = f.device;
            let mut secrets = f.secrets;
            drop(f.engine);
            let engine = WorkspaceEngine::open_with_app_data(&workspace, &app_data).unwrap();
            engine
                .sync_capture_workspace(&device, &mut secrets)
                .unwrap();
            assert_eq!(engine.sync_outbox().unwrap().len(), 1);
            let session = engine
                .collaboration_open(
                    CollaborationOpenInput {
                        relative_path: "note.md".into(),
                    },
                    &device,
                    &secrets,
                )
                .unwrap()
                .unwrap();
            assert_eq!(
                TextDocument::restore(&session.update).unwrap().text(),
                format!("Recovered {boundary}")
            );
        }
    }

    #[test]
    fn legacy_raw_autosave_cannot_overwrite_an_active_generation() {
        let f = fixture();
        let session = open(&f);
        let error = f
            .engine
            .save_raw_markdown(crate::RawSaveInput {
                relative_path: "note.md".into(),
                base_revision: session.revision,
                base_body: "A😀\nsecond\n".into(),
                local_body: "legacy overwrite".into(),
            })
            .unwrap_err();
        assert_eq!(error.code, "collaboration_transaction_required");
        assert!(f.engine.sync_outbox().unwrap().is_empty());
        assert_eq!(
            TextDocument::restore(&open(&f).update).unwrap().text(),
            "A😀\nsecond\n"
        );
        let replacement = FileChange {
            version: 1,
            path: "note.md".into(),
            previous_path: None,
            base_revision: Some(open(&f).revision),
            content: Some(STANDARD.encode(b"relay replacement")),
            accepted_revisions: None,
            blob: None,
        };
        let key = f.secrets.objects.get(&("text".into(), 2)).unwrap();
        let operation = f
            .device
            .signer()
            .seal_at_revision(
                key,
                &f.engine.manifest().id,
                "text",
                f.device.device_id(),
                (2, "1"),
                &serde_json::to_vec(&replacement).unwrap(),
            )
            .unwrap();
        assert_eq!(
            f.engine
                .sync_apply_file(&operation, key, &f.device.signer().public_key())
                .unwrap_err()
                .code,
            "collaboration_transaction_required"
        );
        assert_eq!(
            TextDocument::restore(&open(&f).update).unwrap().text(),
            "A😀\nsecond\n"
        );
    }
    #[test]
    fn durable_save_retry_restart_and_sqlite_rebuild_preserve_pending_edits() {
        let f = fixture();
        let session = open(&f);
        let input = edit(&session, "A😀 changed\nsecond\n");
        let receipt = f
            .engine
            .collaboration_submit(input.clone(), &f.device, &f.secrets)
            .unwrap();
        assert_eq!(
            f.engine
                .collaboration_submit(input, &f.device, &f.secrets)
                .unwrap()
                .revision,
            receipt.revision
        );
        assert_eq!(f.engine.sync_outbox().unwrap().len(), 1);
        assert_eq!(
            std::fs::read(f.directory.path().join("workspace/note.md")).unwrap(),
            "\u{feff}A😀 changed\r\nsecond\r\n".as_bytes()
        );
        // A restart closes the first engine before reopening the workspace;
        // Windows refuses to replace an index.sqlite that another handle holds.
        drop(session);
        drop(f.engine);
        let reopened = WorkspaceEngine::open_with_app_data(
            f.directory.path().join("workspace"),
            f.directory.path().join("app"),
        )
        .unwrap();
        reopened.rebuild_index().unwrap();
        let restored = reopened
            .collaboration_open(
                CollaborationOpenInput {
                    relative_path: "note.md".into(),
                },
                &f.device,
                &f.secrets,
            )
            .unwrap()
            .unwrap();
        assert_eq!(restored.revision, receipt.revision);
        assert_eq!(
            TextDocument::restore(&restored.update).unwrap().text(),
            "A😀 changed\nsecond\n"
        );
        assert_eq!(reopened.sync_outbox().unwrap().len(), 1);
    }

    #[test]
    fn acknowledgements_advance_only_the_verified_baseline_and_keep_later_drafts() {
        let f = fixture();
        let first_session = open(&f);
        f.engine
            .collaboration_submit(edit(&first_session, "first draft\n"), &f.device, &f.secrets)
            .unwrap();
        let second_session = open(&f);
        f.engine
            .collaboration_submit(
                edit(&second_session, "second draft\n"),
                &f.device,
                &f.secrets,
            )
            .unwrap();
        let pending = f.engine.sync_outbox().unwrap();
        assert_eq!(pending.len(), 2);

        f.engine.sync_acknowledge(&pending[0], "1").unwrap();
        let state = f.engine.collaboration_state("text").unwrap().unwrap();
        let baseline = state.acknowledged.as_ref().unwrap();
        assert_eq!(baseline.sequence, "1");
        assert_eq!(
            TextDocument::restore(&baseline.update).unwrap().text(),
            "first draft\n"
        );
        assert_eq!(
            TextDocument::restore(&state.update).unwrap().text(),
            "second draft\n"
        );
        assert_eq!(state.pending.len(), 1);
        assert_eq!(state.pending[0].operation_id, pending[1].operation_id);

        f.engine.sync_acknowledge(&pending[1], "2").unwrap();
        let state = f.engine.collaboration_state("text").unwrap().unwrap();
        assert!(state.pending.is_empty());
        assert_eq!(
            TextDocument::restore(&state.acknowledged.as_ref().unwrap().update)
                .unwrap()
                .text(),
            "second draft\n"
        );
        assert!(f.engine.sync_outbox().unwrap().is_empty());
    }

    fn rotate_checkpoint(f: &mut Fixture, checkpoint_text: &[u8]) -> EncryptedCheckpoint {
        let previous = f.engine.sync_access_policy().unwrap().unwrap();
        let previous_digest = previous.digest().unwrap();
        let policy = AccessPolicy::sign(
            &f.engine.manifest().id,
            "2",
            Some(previous_digest.clone()),
            &f.device,
            previous.members,
            vec![AccessObject {
                object_id: "text".into(),
                epoch: 3,
                grants: vec![],
                envelopes: vec![],
                document: Some(DocumentDescriptor {
                    generation: "generation-two".into(),
                    mode: DocumentMode::Text,
                }),
            }],
        )
        .unwrap();
        f.engine
            .sync_accept_access_policy("1", Some(&previous_digest), &policy)
            .unwrap();
        let key = ObjectKey::generate();
        let checkpoint = EncryptedCheckpoint::seal(
            &f.device,
            &key,
            &policy,
            "0",
            &CheckpointContent {
                version: 1,
                object_id: "text".into(),
                generation: "generation-two".into(),
                content_revision: Some(markdown::revision(checkpoint_text)),
                change: FileChange {
                    version: 1,
                    path: "note.md".into(),
                    previous_path: None,
                    base_revision: None,
                    content: Some(STANDARD.encode(checkpoint_text)),
                    accepted_revisions: None,
                    blob: None,
                },
            },
        )
        .unwrap();
        f.secrets.objects.insert(("text".into(), 3), key);
        checkpoint
    }

    #[test]
    fn generation_rotation_rebases_non_overlapping_drafts_as_fresh_updates() {
        let mut f = fixture();
        let session = open(&f);
        f.engine
            .collaboration_submit(
                edit(&session, "LOCAL\nA😀\nsecond\n"),
                &f.device,
                &f.secrets,
            )
            .unwrap();
        let old_operation = f.engine.sync_outbox().unwrap()[0].operation_id.clone();
        let checkpoint = rotate_checkpoint(&mut f, b"A\xf0\x9f\x98\x80\r\nsecond\r\nREMOTE\r\n");
        f.engine
            .collaboration_install_checkpoint(
                &checkpoint,
                f.secrets.objects.get(&("text".into(), 3)).unwrap(),
                &f.device.signer().public_key(),
                &f.device,
                &f.secrets,
            )
            .unwrap();
        let state = f.engine.collaboration_state("text").unwrap().unwrap();
        assert_eq!(state.generation, "generation-two");
        assert_eq!(
            TextDocument::restore(&state.update).unwrap().text(),
            "LOCAL\nA😀\nsecond\nREMOTE\n"
        );
        assert_eq!(state.pending.len(), 1);
        let outbox = f.engine.sync_outbox().unwrap();
        assert_eq!(outbox.len(), 1);
        assert_ne!(outbox[0].operation_id, old_operation);
        assert_eq!(outbox[0].generation.as_deref(), Some("generation-two"));
        f.engine
            .collaboration_install_checkpoint(
                &checkpoint,
                f.secrets.objects.get(&("text".into(), 3)).unwrap(),
                &f.device.signer().public_key(),
                &f.device,
                &f.secrets,
            )
            .unwrap();
        assert_eq!(f.engine.sync_outbox().unwrap(), outbox);
    }

    #[test]
    fn generation_rotation_preserves_overlapping_drafts_for_review() {
        let mut f = fixture();
        let session = open(&f);
        f.engine
            .collaboration_submit(edit(&session, "local\n"), &f.device, &f.secrets)
            .unwrap();
        let before = std::fs::read(f.directory.path().join("workspace/note.md")).unwrap();
        let pending = f.engine.sync_outbox().unwrap();
        let checkpoint = rotate_checkpoint(&mut f, b"remote\n");
        assert_eq!(
            f.engine
                .collaboration_install_checkpoint(
                    &checkpoint,
                    f.secrets.objects.get(&("text".into(), 3)).unwrap(),
                    &f.device.signer().public_key(),
                    &f.device,
                    &f.secrets,
                )
                .unwrap_err()
                .code,
            "collaboration_needs_review"
        );
        assert_eq!(
            std::fs::read(f.directory.path().join("workspace/note.md")).unwrap(),
            before
        );
        assert_eq!(f.engine.sync_outbox().unwrap(), pending);
        assert_eq!(
            f.engine
                .collaboration_state("text")
                .unwrap()
                .unwrap()
                .generation,
            "generation-one"
        );
    }
    #[test]
    fn viewers_stale_generations_and_competing_external_bytes_are_rejected() {
        let mut f = fixture();
        let session = open(&f);
        let mut input = edit(&session, "draft");
        input.generation = "stale".into();
        assert_eq!(
            f.engine
                .collaboration_submit(input, &f.device, &f.secrets)
                .unwrap_err()
                .code,
            "collaboration_stale_generation"
        );
        f.secrets.authorized_workspace_writers.clear();
        assert_eq!(
            f.engine
                .collaboration_submit(edit(&session, "draft"), &f.device, &f.secrets)
                .unwrap_err()
                .code,
            "sync_writer_not_authorized"
        );
        f.secrets
            .authorized_workspace_writers
            .insert(f.device.device_id().into());
        std::fs::write(
            f.directory.path().join("workspace/note.md"),
            "external bytes",
        )
        .unwrap();
        assert_eq!(
            f.engine
                .collaboration_submit(edit(&session, "draft"), &f.device, &f.secrets)
                .unwrap_err()
                .code,
            "collaboration_external_change"
        );
        assert_eq!(
            std::fs::read_to_string(f.directory.path().join("workspace/note.md")).unwrap(),
            "external bytes"
        );
        assert!(f.engine.sync_outbox().unwrap().is_empty());
    }
    #[test]
    fn lost_ack_recovery_finishes_each_durable_boundary_and_rejects_tampering() {
        for boundary in 0..3 {
            let f = fixture();
            let prior = f.engine.collaboration_state("text").unwrap().unwrap();
            let session = open(&f);
            let input = edit(&session, "recovered 😀\n");
            let change = CollaborativeChange {
                version: 1,
                object_id: "text".into(),
                generation: session.generation,
                updates: input.updates,
            };
            let key = f.secrets.objects.get(&("text".into(), 2)).unwrap();
            let operation = f
                .device
                .signer()
                .seal_for_document(
                    key,
                    &f.engine.manifest().id,
                    "text",
                    f.device.device_id(),
                    (2, "1", &change.generation, OperationKind::Text),
                    &serde_json::to_vec(&change).unwrap(),
                )
                .unwrap();
            let candidate = TextDocument::restore(&prior.update)
                .unwrap()
                .apply(&change.updates)
                .unwrap();
            let bytes = render(
                &prior.path,
                &STANDARD.decode(&prior.materialized).unwrap(),
                "text",
                &candidate.text(),
            )
            .unwrap();
            let next = DocumentState {
                revision: markdown::revision(&bytes),
                materialized: STANDARD.encode(&bytes),
                update: candidate.state().unwrap(),
                pending: vec![PendingDocumentChange {
                    operation_id: operation.operation_id.clone(),
                    updates: change.updates.clone(),
                    metadata: Vec::new(),
                    format: None,
                    lifecycle: None,
                }],
                ..prior.clone()
            };
            let intent = DocumentIntent {
                version: 1,
                base_revision: prior.revision,
                next,
                operation,
                outgoing: true,
                batch_id: None,
                batch_digest: None,
                external_snapshot: None,
                previous_path: None,
            };
            f.engine
                .sync_write(&intent_path("text").unwrap(), &Some(&intent))
                .unwrap();
            if boundary >= 1 {
                f.engine
                    .sync_write_file_checked("note.md", &bytes, Some(&intent.base_revision))
                    .unwrap();
            }
            if boundary >= 2 {
                f.engine
                    .sync_write(&state_path("text").unwrap(), &intent.next)
                    .unwrap();
            }
            f.engine
                .collaboration_flush(&session.session_id, &f.secrets)
                .unwrap();
            assert_eq!(
                std::fs::read(f.directory.path().join("workspace/note.md")).unwrap(),
                bytes
            );
            assert_eq!(f.engine.sync_outbox().unwrap().len(), 1);
            // A forged intent cannot change canonical metadata or a candidate's text.
            let mut forged = intent;
            forged.next.materialized = STANDARD.encode(b"forged");
            forged.next.revision = markdown::revision(b"forged");
            f.engine
                .sync_write(&intent_path("text").unwrap(), &Some(forged))
                .unwrap();
            assert!(
                f.engine
                    .collaboration_flush(&session.session_id, &f.secrets)
                    .is_err()
            );
            assert_eq!(
                std::fs::read(f.directory.path().join("workspace/note.md")).unwrap(),
                bytes
            );
        }
    }
}
