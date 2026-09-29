//! Local submissions and remote operations, applied as one transaction
//! against the document state.

use super::*;

impl WorkspaceEngine {
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
        self.emit("collaboration:update", if outgoing { EventSource::Application } else { EventSource::Sync }, serde_json::json!({"objectId": next.object_id, "generation":next.generation,"update":delta,"revision":next.revision}));
        self.emit(
            "file:changed",
            EventSource::Sync,
            serde_json::json!({"paths":[next.path]}),
        );
        Ok(next)
    }

    pub(super) fn collaboration_apply_transaction_locked(
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
                if outgoing { EventSource::Application } else { EventSource::Sync },
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
            if outgoing { EventSource::Application } else { EventSource::Sync },
            serde_json::json!({"id":next.object_id,"path":next.path,"from":previous_path,"to":next.path,"revision":next.revision,"deleted":deleted,"trashPath":tombstone_path}),
        );
        self.emit(
            "file:changed",
            EventSource::Sync,
            serde_json::json!({"paths":[next.path]}),
        );
        Ok(next)
    }
}
