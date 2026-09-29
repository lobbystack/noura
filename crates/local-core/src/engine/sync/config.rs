//! Replica setup, sync configuration and status, and wrapped object keys.

use super::*;

impl WorkspaceEngine {
    /// Create a new empty local replica with the `sync` plugin enabled.
    /// Existing workspace identities are never changed.
    pub fn create_sync_replica(
        root: impl AsRef<Path>,
        name: &str,
        workspace_id: &str,
        app_data: impl AsRef<Path>,
    ) -> Result<Self> {
        crate::sync::identifier(workspace_id)?;
        let root = root.as_ref();
        if root
            .try_exists()
            .map_err(|_| invalid("sync_replica_path_unavailable"))?
            && std::fs::read_dir(root)
                .map_err(|_| invalid("sync_replica_path_unavailable"))?
                .next()
                .is_some()
        {
            return Err(invalid("sync_replica_directory_not_empty"));
        }
        Self::create_with_identity(root, name, app_data, Some(workspace_id), &[SYNC_PLUGIN_ID])
    }

    pub fn sync_configuration(&self) -> Result<Option<crate::sync::WorkspaceSyncConfig>> {
        let _lock = self.write_lock("sync_configuration")?;
        let value = read_optional(&self.sync_path(".noura/sync/config.json")?)?
            .map(|bytes| {
                serde_json::from_slice::<crate::sync::WorkspaceSyncConfig>(&bytes)
                    .map_err(|_| invalid("sync_invalid_configuration"))
            })
            .transpose()?;
        if let Some(config) = &value {
            config.validate(&self.manifest().id)?;
        }
        Ok(value)
    }

    pub fn sync_save_configuration(&self, config: &crate::sync::WorkspaceSyncConfig) -> Result<()> {
        config.validate(&self.manifest().id)?;
        let _lock = self.write_lock("sync_configuration")?;
        self.sync_write(".noura/sync/config.json", config)?;
        // A replica whose sync setup was saved has decided its plugin state;
        // the open-time migration must not revisit it.
        self.sync_plugin_record_marker_locked()
    }

    pub fn sync_pause(&self, paused: bool) -> Result<()> {
        let mut config = self
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        config.enabled = !paused;
        self.sync_save_configuration(&config)
    }

    pub fn sync_status(&self) -> Result<crate::sync::WorkspaceSyncStatus> {
        use crate::sync::{
            WorkspaceSyncActivationStatus, WorkspaceSyncPhase, WorkspaceSyncStatus,
            WorkspaceSyncTransitionObject, WorkspaceSyncTransitionPhase,
            WorkspaceSyncTransitionStatus,
        };
        let config = self.sync_configuration()?;
        let _lock = self.write_lock("sync_status")?;
        let journal = self.sync_journal()?;
        Ok(WorkspaceSyncStatus {
            enabled: config.as_ref().is_some_and(|value| value.enabled),
            phase: match config {
                None => WorkspaceSyncPhase::Disabled,
                Some(value) if !value.enabled => WorkspaceSyncPhase::Paused,
                Some(_) => WorkspaceSyncPhase::Idle,
            },
            pending: journal.outbox.len().try_into().unwrap_or(u32::MAX),
            conflicts: journal
                .receipts
                .values()
                .filter(|receipt| receipt.outcome == ApplyOutcome::Conflict)
                .count()
                .try_into()
                .unwrap_or(u32::MAX),
            error_code: None,
            last_success: None,
            transition: journal
                .transition
                .as_ref()
                .map(|transition| {
                    let phase = match journal.transition_phase.unwrap_or(TransitionPhase::Prepare) {
                        TransitionPhase::Prepare => WorkspaceSyncTransitionPhase::Prepare,
                        TransitionPhase::Stage => WorkspaceSyncTransitionPhase::Stage,
                        TransitionPhase::ResolveCommit => {
                            WorkspaceSyncTransitionPhase::ResolveCommit
                        }
                        TransitionPhase::Install => WorkspaceSyncTransitionPhase::Install,
                        TransitionPhase::Rebase => WorkspaceSyncTransitionPhase::Rebase,
                        TransitionPhase::Complete => WorkspaceSyncTransitionPhase::Complete,
                    };
                    let objects = transition
                        .checkpoints
                        .iter()
                        .map(|checkpoint| {
                            Ok(WorkspaceSyncTransitionObject {
                                object_id: checkpoint.payload.object_id.clone(),
                                path: journal
                                    .objects
                                    .get(&checkpoint.payload.object_id)
                                    .map(|object| object.path.clone()),
                                installed: self.collaboration_generation_is_installed(
                                    &checkpoint.payload.object_id,
                                    &checkpoint.generation,
                                )?,
                            })
                        })
                        .collect::<Result<Vec<_>>>()?;
                    Ok(WorkspaceSyncTransitionStatus {
                        transition_id: transition.transition_id.clone(),
                        phase,
                        objects,
                    })
                })
                .transpose()?,
            activation: journal
                .activation
                .as_ref()
                .map(|activation| {
                    let object_id = activation.checkpoint.payload.object_id.clone();
                    Ok(WorkspaceSyncActivationStatus {
                        activation_id: activation.activation_id.clone(),
                        path: journal
                            .objects
                            .get(&object_id)
                            .map(|object| object.path.clone()),
                        installed: self.collaboration_generation_is_installed(
                            &object_id,
                            &activation.document.generation,
                        )?,
                        object_id,
                    })
                })
                .transpose()?,
        })
    }

    /// Restore encrypted local keys without consulting a derived database or trusting server metadata.
    pub fn sync_restore_secrets(
        &self,
        device: &DeviceKeys,
        trusted: &BTreeMap<String, String>,
    ) -> Result<crate::sync::SyncSecrets> {
        let _lock = self.write_lock("sync_restore_keys")?;
        let mut result = crate::sync::SyncSecrets {
            objects: BTreeMap::new(),
            trusted_devices: trusted.clone(),
            authorized_workspace_writers: Default::default(),
            authorized_object_writers: Default::default(),
            ..Default::default()
        };
        let root = self.sync_path(".noura/sync/keys")?;
        if !root.exists() {
            return Ok(result);
        }
        for object in std::fs::read_dir(&root).map_err(|_| invalid("sync_key_read_failed"))? {
            let object = object.map_err(|_| invalid("sync_key_read_failed"))?;
            let name = object
                .file_name()
                .into_string()
                .map_err(|_| invalid("sync_unsupported_path"))?;
            crate::sync::identifier(&name)?;
            let directory = self.sync_path(&format!(".noura/sync/keys/{name}"))?;
            for epoch in
                std::fs::read_dir(directory).map_err(|_| invalid("sync_key_read_failed"))?
            {
                let epoch = epoch
                    .map_err(|_| invalid("sync_key_read_failed"))?
                    .file_name()
                    .into_string()
                    .map_err(|_| invalid("sync_unsupported_path"))?;
                let number = sync_cursor(&epoch)?;
                let path = key_path(&name, number, device.device_id())?;
                let Some(bytes) = read_optional(&self.sync_path(&path)?)? else {
                    continue;
                };
                let envelope: KeyEnvelope = serde_json::from_slice(&bytes)
                    .map_err(|_| invalid("sync_invalid_wrapped_key"))?;
                if envelope.workspace_id != self.manifest().id
                    || envelope.object_id != name
                    || envelope.epoch != number
                {
                    return Err(invalid("sync_invalid_wrapped_key"));
                }
                let signer = trusted
                    .get(&envelope.signing_device)
                    .ok_or_else(|| invalid("sync_untrusted_device"))?;
                result.objects.insert(
                    (name.clone(), number),
                    device.unwrap_key(&envelope, signer)?,
                );
            }
        }
        Ok(result)
    }

    pub fn sync_key_envelope(
        &self,
        object: &str,
        epoch: u64,
        device: &str,
    ) -> Result<Option<KeyEnvelope>> {
        let _lock = self.write_lock("sync_read_key")?;
        read_optional(&self.sync_path(&key_path(object, epoch, device)?)?)?
            .map(|bytes| {
                serde_json::from_slice(&bytes).map_err(|_| invalid("sync_invalid_wrapped_key"))
            })
            .transpose()
    }

    /// Persist only an authenticated, recipient-encrypted key. Plain object keys never enter files.
    /// The caller must obtain `trusted_signer` from a prior device approval, not the response being stored.
    pub fn sync_store_key(
        &self,
        envelope: &KeyEnvelope,
        device: &DeviceKeys,
        trusted_signer: &str,
    ) -> Result<()> {
        if envelope.workspace_id != self.manifest().id {
            return Err(invalid("sync_wrong_workspace"));
        }
        let _key = device.unwrap_key(envelope, trusted_signer)?;
        let _lock = self.write_lock("sync_store_key")?;
        let path = key_path(&envelope.object_id, envelope.epoch, device.device_id())?;
        self.sync_write_once(&path, envelope)
    }

    /// Restore a content key from its durable encrypted envelope after an index rebuild or restart.
    pub fn sync_load_key(
        &self,
        object: &str,
        epoch: u64,
        device: &DeviceKeys,
        trusted_signer: &str,
    ) -> Result<ObjectKey> {
        let _lock = self.write_lock("sync_load_key")?;
        let path = key_path(object, epoch, device.device_id())?;
        let bytes =
            read_optional(&self.sync_path(&path)?)?.ok_or_else(|| invalid("sync_key_required"))?;
        let envelope: KeyEnvelope =
            serde_json::from_slice(&bytes).map_err(|_| invalid("sync_invalid_wrapped_key"))?;
        if envelope.workspace_id != self.manifest().id
            || envelope.object_id != object
            || envelope.epoch != epoch
        {
            return Err(invalid("sync_invalid_wrapped_key"));
        }
        device.unwrap_key(&envelope, trusted_signer)
    }
}
