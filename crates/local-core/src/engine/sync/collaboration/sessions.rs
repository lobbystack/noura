//! Editor sessions on collaborative documents, and the guards that keep
//! other writers off an active document.

use super::*;

impl WorkspaceEngine {
    /// Whether the file at `relative_path` is a collaborative text document.
    /// Reads only the sync journal, so callers can skip credentials otherwise.
    pub fn collaboration_path_is_active(&self, relative_path: &str) -> Result<bool> {
        self.sync_file_path(relative_path)?;
        let journal = self.sync_journal()?;
        Ok(journal
            .objects
            .iter()
            .find(|(_, object)| object.path == relative_path)
            .and_then(|(id, _)| Self::sync_document_descriptor(&journal, id))
            .is_some_and(|descriptor| descriptor.mode == DocumentMode::Text))
    }

    pub fn collaboration_object_is_active(&self, object_id: &str) -> Result<bool> {
        crate::sync::identifier(object_id)?;
        let journal = self.sync_journal()?;
        Ok(Self::sync_document_descriptor(&journal, object_id)
            .is_some_and(|descriptor| descriptor.mode == DocumentMode::Text))
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
                EventSource::Sync,
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

    /// Local file mutations must not bypass a signed collaborative generation
    /// while the sync plugin is on. With the plugin off, local files stay
    /// editable like any other file: the workspace is local-first, and turning
    /// sync off must not lock you out of your own documents. When the plugin
    /// comes back, the sync scan captures those edits like external edits.
    /// Called with the engine write lock already held.
    pub(crate) fn collaboration_guard_file_mutation(&self, path: &str) -> Result<()> {
        if !self.sync_plugin_enabled() {
            return Ok(());
        }
        self.collaboration_guard_sync_change(path)
    }

    /// Guard a file, or every synced file inside a folder, before a move or
    /// delete. Only files the sync journal tracks can be collaborative.
    pub(crate) fn collaboration_guard_tree_mutation(&self, path: &str) -> Result<()> {
        self.collaboration_guard_file_mutation(path)?;
        if !self.sync_plugin_enabled() {
            return Ok(());
        }
        let prefix = format!("{}/", path.trim_end_matches('/'));
        let journal = self.sync_journal()?;
        for object in journal.objects.values() {
            if object.path.starts_with(&prefix) {
                self.collaboration_guard_sync_change(&object.path)?;
            }
        }
        Ok(())
    }

    /// Sync's own whole-file changes never bypass a signed collaborative
    /// generation, whatever the plugin state.
    /// Called with the engine write lock already held.
    pub(crate) fn collaboration_guard_sync_change(&self, path: &str) -> Result<()> {
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
}
