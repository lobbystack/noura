//! Edits made outside the editor: translating them into collaborative
//! transactions, or keeping them for review when they cannot merge.

use super::*;

impl WorkspaceEngine {
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
                EventSource::External,
                serde_json::json!({"objectId":object_id,"generation":descriptor.generation,"update":delta,"revision":next.revision}),
            );
        }
        self.emit(
            "file:changed",
            EventSource::External,
            serde_json::json!({"paths":[path]}),
        );
        Ok(true)
    }

    pub(super) fn collaboration_record_review(
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

    pub(super) fn collaboration_record_review_without_prior(
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
            EventSource::Sync,
            serde_json::to_value(CollaborationStatusEvent {
                object_id: object_id.into(),
                generation: checkpoint.generation.clone(),
                status: CollaborationStatus::NeedsReview,
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        self.emit(
            "collaboration:review",
            EventSource::Sync,
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

    pub(super) fn collaboration_record_metadata_review(
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
            EventSource::Sync,
            serde_json::to_value(CollaborationStatusEvent {
                object_id: operation.object_id.clone(),
                generation: generation.into(),
                status: CollaborationStatus::NeedsReview,
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        self.emit(
            "collaboration:review",
            EventSource::Sync,
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
            EventSource::External,
            serde_json::to_value(CollaborationStatusEvent {
                object_id: state.object_id.clone(),
                generation: state.generation.clone(),
                status: CollaborationStatus::NeedsReview,
            })
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        self.emit(
            "collaboration:review",
            EventSource::External,
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
}
