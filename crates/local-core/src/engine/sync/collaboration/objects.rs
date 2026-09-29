//! Managed object updates, moves, and deletions on collaborative documents.

use super::*;

impl WorkspaceEngine {
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
                chat_revision: None,
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
            chat_revision: None,
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
            chat_revision: None,
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
            chat_revision: None,
        })
    }
}
