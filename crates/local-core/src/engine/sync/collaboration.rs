use super::*;
use crate::sync::{
    DocumentMode, OperationKind, SyncSecrets,
    collaboration::{
        CollaborationOpenInput, CollaborationReceipt, CollaborationSession,
        CollaborationSubmitInput, CollaborativeChange, MAX_TEXT_BYTES, TextDocument,
    },
};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DocumentState {
    version: u8,
    object_id: String,
    generation: String,
    covered_sequence: String,
    path: String,
    revision: String,
    materialized: String,
    update: String,
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
}
fn state_path(object: &str) -> Result<String> {
    crate::sync::identifier(object)?;
    Ok(format!(".noura/sync/collaboration/{object}/state.json"))
}
fn intent_path(object: &str) -> Result<String> {
    crate::sync::identifier(object)?;
    Ok(format!(".noura/sync/collaboration/{object}/intent.json"))
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

impl WorkspaceEngine {
    /// Legacy file mutations must never bypass a signed collaborative generation.
    /// Called with the engine write lock already held.
    pub(crate) fn collaboration_guard_file_mutation(&self, path: &str) -> Result<()> {
        let journal = self.sync_journal()?;
        let Some((id, _)) = journal.objects.iter().find(|(_, value)| value.path == path) else {
            return Ok(());
        };
        if journal.access_policy.as_ref().is_some_and(|policy| {
            policy.objects.iter().any(|object| {
                &object.object_id == id
                    && object
                        .document
                        .as_ref()
                        .is_some_and(|document| document.mode == DocumentMode::Text)
            })
        }) {
            return Err(invalid("collaboration_transaction_required"));
        }
        Ok(())
    }

    /// Called only after the checkpoint's signed policy has been accepted locally.
    pub fn collaboration_install_checkpoint(
        &self,
        checkpoint: &crate::sync::EncryptedCheckpoint,
        key: &ObjectKey,
        trusted_key: &str,
    ) -> Result<()> {
        let content = checkpoint.open(key, trusted_key)?;
        self.sync_check_workspace(&checkpoint.payload)?;
        let checkpoint_policy = self.sync_checkpoint_policy(&checkpoint.payload.policy_revision)?;
        checkpoint_policy.verify(trusted_key)?;
        if checkpoint_policy.device_id != checkpoint.payload.device_id
            || !checkpoint_policy.objects.iter().any(|object| {
                object.object_id == content.object_id
                    && object.epoch == checkpoint.payload.epoch
                    && object.document.as_ref().is_some_and(|document| {
                        document.generation == checkpoint.generation
                            && document.mode == DocumentMode::Text
                    })
            })
        {
            return Err(invalid("collaboration_checkpoint_policy_mismatch"));
        }
        let _lock = self.write_lock("collaboration_checkpoint")?;
        let mut journal = self.sync_journal()?;
        let descriptor = journal
            .access_policy
            .as_ref()
            .and_then(|policy| {
                policy
                    .objects
                    .iter()
                    .find(|object| object.object_id == content.object_id)
            })
            .filter(|object| object.epoch == checkpoint.payload.epoch)
            .and_then(|object| object.document.as_ref())
            .filter(|document| {
                document.generation == checkpoint.generation && document.mode == DocumentMode::Text
            })
            .ok_or_else(|| invalid("collaboration_checkpoint_policy_mismatch"))?;
        let generation = descriptor.generation.clone();
        let bytes = content
            .change
            .content
            .as_ref()
            .ok_or_else(|| invalid("collaboration_checkpoint_blob_required"))?;
        let bytes = crate::sync::decode(bytes, 0, MAX_TEXT_BYTES)?;
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
        if let Some(prior) = self.collaboration_state(&content.object_id)? {
            if prior.generation == generation {
                return Ok(());
            }
            // A generation boundary requires the dedicated reviewed rebase flow.
            if prior.revision != markdown::revision(&bytes)
                || journal
                    .outbox
                    .iter()
                    .any(|op| op.object_id == content.object_id)
            {
                return Err(invalid("collaboration_rebase_required"));
            }
        }
        let destination = self.sync_file_path(&content.change.path)?;
        let existing = read_optional(&destination)?;
        if existing.as_deref().is_some_and(|current| current != bytes) {
            return Err(invalid("collaboration_rebase_required"));
        }
        if existing.is_none() {
            self.sync_write_file_checked(&content.change.path, &bytes, None)?;
        }
        let state = DocumentState {
            version: 1,
            object_id: content.object_id.clone(),
            generation,
            covered_sequence: checkpoint.covered_sequence.clone(),
            path: content.change.path.clone(),
            revision: markdown::revision(&bytes),
            materialized: STANDARD.encode(&bytes),
            update: document.state()?,
        };
        self.sync_write(&checkpoint_path, checkpoint)?;
        self.sync_write(&state_path(&state.object_id)?, &state)?;
        journal.objects.insert(
            content.object_id,
            ObjectState {
                path: state.path,
                revision: Some(state.revision),
            },
        );
        self.sync_write(STATE_PATH, &journal)?;
        self.emit(
            "collaboration:activated",
            "sync",
            serde_json::json!({"objectIds":[state.object_id]}),
        );
        Ok(())
    }

    pub fn collaboration_checkpoint_needed(&self, object: &str, generation: &str) -> Result<bool> {
        let _lock = self.write_lock("collaboration_checkpoint")?;
        Ok(self
            .collaboration_state(object)?
            .is_none_or(|state| state.generation != generation))
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
        let Some(descriptor) = journal
            .access_policy
            .as_ref()
            .and_then(|policy| policy.objects.iter().find(|object| &object.object_id == id))
            .and_then(|object| object.document.as_ref())
        else {
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
        Ok(Some(CollaborationSession {
            object_id: id.clone(),
            generation: state.generation,
            session_id,
            update: state.update,
            revision: state.revision,
            read_only,
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
        let policy_object = journal
            .access_policy
            .as_ref()
            .and_then(|policy| {
                policy
                    .objects
                    .iter()
                    .find(|entry| entry.object_id == object)
            })
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if policy_object.document.as_ref().is_none_or(|document| {
            document.generation != change.generation || document.mode != DocumentMode::Text
        }) {
            return Err(invalid("collaboration_stale_generation"));
        }
        let key = secrets
            .objects
            .get(&(object.clone(), policy_object.epoch))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let operation = device.signer().seal_for_document(
            key,
            &journal.workspace_id,
            &object,
            device.device_id(),
            (
                policy_object.epoch,
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
    ) -> Result<ApplyOutcome> {
        self.sync_check_workspace(operation)?;
        if !secrets.writer_is_authorized(
            &operation.policy_revision,
            &operation.object_id,
            &operation.device_id,
        ) {
            return Err(invalid("sync_writer_not_authorized"));
        }
        if operation.kind != Some(OperationKind::Text) {
            return Err(invalid("collaboration_unsupported_operation"));
        }
        let plaintext = operation.open(key, trusted_key)?;
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
        self.collaboration_apply_change(&change, operation, false, None)?;
        Ok(ApplyOutcome::Applied)
    }

    fn collaboration_apply_change(
        &self,
        change: &CollaborativeChange,
        operation: &EncryptedOperation,
        outgoing: bool,
        batch: Option<(String, String)>,
    ) -> Result<DocumentState> {
        let state = self
            .collaboration_state(&change.object_id)?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        if state.generation != change.generation {
            return Err(invalid("collaboration_stale_generation"));
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
        let next = DocumentState {
            version: 1,
            object_id: state.object_id,
            generation: state.generation,
            covered_sequence: state.covered_sequence,
            path: state.path,
            revision: markdown::revision(&bytes),
            materialized: STANDARD.encode(&bytes),
            update: next.state()?,
        };
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

    fn collaboration_state(&self, object: &str) -> Result<Option<DocumentState>> {
        let Some(bytes) = read_optional(&self.sync_path(&state_path(object)?)?)? else {
            return Ok(None);
        };
        let state: DocumentState =
            serde_json::from_slice(&bytes).map_err(|_| invalid("collaboration_invalid_state"))?;
        if state.version != 1 || state.object_id != object {
            return Err(invalid("collaboration_invalid_state"));
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
        Ok(Some(state))
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
        let change: CollaborativeChange =
            serde_json::from_slice(&intent.operation.open(key, signer)?)
                .map_err(|_| invalid("collaboration_invalid_intent"))?;
        change.validate()?;
        if change.object_id != object
            || intent.operation.object_id != object
            || intent.operation.kind != Some(OperationKind::Text)
            || intent.operation.generation.as_ref() != Some(&change.generation)
            || intent.next.generation != change.generation
            || intent.next.version != 1
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
        if prior.generation != intent.next.generation
            || prior.path != intent.next.path
            || (prior.revision != intent.base_revision && prior.revision != intent.next.revision)
        {
            return Err(invalid("collaboration_invalid_intent"));
        }
        let reconstructed = TextDocument::restore(&prior.update)?.apply(&change.updates)?;
        let prior_bytes = crate::sync::decode(&prior.materialized, 0, MAX_TEXT_BYTES)?;
        if reconstructed.state()? != candidate.state()?
            || render(&prior.path, &prior_bytes, object, &reconstructed.text())? != materialized
        {
            return Err(invalid("collaboration_invalid_intent"));
        }
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
        self.collaboration_finish(&intent)
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
                revision: Some(next.revision.clone()),
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
            writes.insert(next.path.clone(), next.revision.clone());
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
        engine
            .collaboration_install_checkpoint(&checkpoint, &key, &device.signer().public_key())
            .unwrap();
        let secrets = SyncSecrets {
            objects: BTreeMap::from([(("text".into(), 2), key)]),
            trusted_devices: BTreeMap::from([(
                device.device_id().into(),
                device.signer().public_key(),
            )]),
            authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
            ..Default::default()
        };
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
