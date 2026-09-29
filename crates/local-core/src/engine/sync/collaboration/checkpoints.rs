//! Checkpoints: preparing a generation's content and installing a
//! received one, for text documents and attachments.

use super::*;

impl WorkspaceEngine {
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
            EventSource::Sync,
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

    pub(in crate::engine::sync) fn collaboration_generation_is_installed(
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
}
