//! Durable intents and acknowledgements. Each mutation records an intent
//! first, so a restart can finish the step it interrupted.

use super::*;

impl WorkspaceEngine {
    pub(super) fn collaboration_validate_recovery_state(
        &self,
        state: &DocumentState,
    ) -> Result<()> {
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
    pub(in crate::engine::sync) fn collaboration_acknowledge(
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

    pub(super) fn collaboration_recover(&self, object: &str, secrets: &SyncSecrets) -> Result<()> {
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

    pub(super) fn collaboration_finish(&self, intent: &DocumentIntent) -> Result<()> {
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
