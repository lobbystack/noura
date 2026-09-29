//! Activating pending objects and sharing their keys with approved devices.

use super::*;

impl HttpSyncTransport {
    pub(crate) async fn activate_pending_objects(
        &self,
        engine: &WorkspaceEngine,
        device: &DeviceKeys,
        config: &WorkspaceSyncConfig,
        secrets: &SyncSecrets,
        state: &AccessState,
    ) -> Result<bool> {
        if let Some(activation) = engine.sync_pending_activation()? {
            match self
                .submit_object_activation(engine, &activation, device, secrets)
                .await
            {
                Ok(()) => return Ok(true),
                Err(error) if error.code == "sync_activation_stale" => {
                    if self.object_activation_is_committed(&activation).await? {
                        return Err(invalid("sync_activation_commit_uncertain"));
                    }
                    engine.sync_abandon_activation(&activation.activation_id)?;
                }
                Err(error) => return Err(error),
            }
        }
        if !self.object_activations_available().await? {
            return Ok(false);
        }
        let Some(policy) = state.policy.as_ref().filter(|policy| policy.version == 2) else {
            return Ok(false);
        };
        let Some(operation) = engine.sync_outbox()?.into_iter().find(|operation| {
            !state
                .objects
                .iter()
                .any(|object| object.object_id == operation.object_id)
        }) else {
            return Ok(false);
        };
        if operation.epoch != 1 {
            return Err(invalid("sync_invalid_activation"));
        }
        let workspace = engine.manifest().id;
        let capability = self
            .workspace_capability(&workspace, &config.trusted_devices)
            .await?;
        let key = secrets
            .objects
            .get(&(operation.object_id.clone(), 1))
            .ok_or_else(|| invalid("sync_key_required"))?;
        let generation = uuid::Uuid::new_v4().to_string();
        let (content, blob, mode) = engine
            .collaboration_prepare_checkpoint_content(&operation.object_id, &generation, key)
            .map_err(|mut error| {
                error.object_id = Some(operation.object_id.clone());
                error
            })?;
        let checkpoint = EncryptedCheckpoint::seal_activation(
            device,
            key,
            &workspace,
            &policy.revision,
            &engine.sync_cursor()?,
            &content,
        )?;
        let mut envelopes = Vec::new();
        for recipient in &state.devices {
            if !state
                .members
                .iter()
                .any(|member| member.account_id == recipient.account_id)
            {
                continue;
            }
            let recipient_key = recipient
                .encryption_recipient
                .as_ref()
                .ok_or_else(|| invalid("sync_device_upgrade_required"))?;
            let expected_recipient = if recipient.device_id == device.device_id() {
                device.recipient()
            } else {
                config
                    .approved_recipients
                    .get(&recipient.device_id)
                    .cloned()
                    .ok_or_else(|| invalid("sync_device_approval_required"))?
            };
            if &expected_recipient != recipient_key
                || config.approved_accounts.get(&recipient.device_id) != Some(&recipient.account_id)
                || config.trusted_devices.get(&recipient.device_id) != Some(&recipient.public_key)
            {
                return Err(invalid("sync_device_changed"));
            }
            envelopes.push(PolicyEnvelope::from(device.wrap_key(
                &workspace,
                &operation.object_id,
                1,
                &recipient.device_id,
                recipient_key,
                key,
            )?));
        }
        let blobs = blob
            .map(|blob| CheckpointBlobManifest {
                object_id: operation.object_id.clone(),
                epoch: 1,
                ciphertext_digest: blob.id,
                ciphertext_size: blob.size,
            })
            .into_iter()
            .collect();
        let activation = ObjectActivation::sign(
            device,
            &capability,
            &policy.revision,
            &engine.sync_cursor()?,
            DocumentDescriptor { generation, mode },
            envelopes,
            checkpoint,
            blobs,
        )?;
        self.submit_object_activation(engine, &activation, device, secrets)
            .await?;
        Ok(true)
    }

    pub(crate) async fn share_approved_keys(
        &self,
        engine: &WorkspaceEngine,
        device: &DeviceKeys,
        config: &WorkspaceSyncConfig,
        secrets: &SyncSecrets,
    ) -> Result<()> {
        let workspace = engine.manifest().id;
        let state = self.access_state(&workspace).await?;
        let role = state.verified_role(engine, device, config)?;
        if role == WorkspaceRole::Viewer {
            return Ok(());
        }
        if state
            .policy
            .as_ref()
            .is_some_and(|policy| policy.version == 2)
            && (readers_changed(&state, config, device) || orphan_reader(&state))
        {
            if !self.access_transitions_available().await? {
                return Err(invalid("sync_access_transition_unavailable"));
            }
            self.ensure_workspace_capability(&workspace, device, &config.trusted_devices)
                .await?;
            let transition = prepare_rotation_transition(
                engine,
                device,
                config,
                &state,
                state.members.clone(),
                state.devices.clone(),
            )?;
            return finish_access_transition(engine, self, device, config, &transition).await;
        }
        let revision = super::transport::parse_cursor(&state.revision)?;
        let mut pending = Vec::new();
        let mut objects = Vec::new();
        let mut changed = false;
        if orphan_reader(&state) {
            return Err(invalid("sync_key_rotation_required"));
        }
        if state.policy.as_ref().is_some_and(|policy| {
            policy.objects.iter().any(|old| {
                !state
                    .objects
                    .iter()
                    .any(|object| object.object_id == old.object_id)
            })
        }) {
            return Err(invalid("sync_invalid_access_state"));
        }
        for object in &state.objects {
            let epoch = super::transport::parse_cursor(&object.epoch)?;
            let previous = state.policy.as_ref().and_then(|policy| {
                policy
                    .objects
                    .iter()
                    .find(|old| old.object_id == object.object_id)
            });
            if previous.map_or(epoch != 1, |old| old.epoch != epoch) {
                return Err(invalid("sync_invalid_access_state"));
            }
            let key = secrets
                .objects
                .get(&(object.object_id.clone(), epoch))
                .ok_or_else(|| invalid("sync_key_required"))?;
            let grants = previous.map(|old| old.grants.clone()).unwrap_or_default();
            let mut envelopes = Vec::new();
            for recipient in &state.devices {
                if !state
                    .members
                    .iter()
                    .any(|member| member.account_id == recipient.account_id)
                    && !grants
                        .iter()
                        .any(|grant| grant.account_id == recipient.account_id)
                {
                    continue;
                }
                // An enrolled device that is not yet approved locally cannot
                // receive a wrapped key. Skip it so approved devices still do.
                if recipient.device_id != device.device_id()
                    && !config
                        .approved_recipients
                        .contains_key(&recipient.device_id)
                {
                    continue;
                }
                let recipient_key = recipient
                    .encryption_recipient
                    .as_ref()
                    .ok_or_else(|| invalid("sync_device_upgrade_required"))?;
                let expected = if recipient.device_id == device.device_id() {
                    device.recipient()
                } else {
                    config
                        .approved_recipients
                        .get(&recipient.device_id)
                        .cloned()
                        .ok_or_else(|| invalid("sync_device_approval_required"))?
                };
                if &expected != recipient_key
                    || config.approved_accounts.get(&recipient.device_id)
                        != Some(&recipient.account_id)
                    || config.trusted_devices.get(&recipient.device_id)
                        != Some(&recipient.public_key)
                {
                    return Err(invalid("sync_device_changed"));
                }
                let existing = state.envelopes.iter().find(|entry| {
                    entry.object_id == object.object_id
                        && entry.epoch == object.epoch
                        && entry.device_id == recipient.device_id
                });
                let envelope = if let Some(existing) = existing {
                    let envelope = existing.to_key_envelope(&workspace, &object.object_id, epoch);
                    let signer = config
                        .trusted_devices
                        .get(&envelope.signing_device)
                        .ok_or_else(|| invalid("sync_untrusted_device"))?;
                    envelope.resign(device, signer)?
                } else if recipient.device_id == device.device_id() {
                    changed = true;
                    engine
                        .sync_key_envelope(&object.object_id, epoch, device.device_id())?
                        .ok_or_else(|| invalid("sync_key_required"))?
                        .resign(device, &device.signer().public_key())?
                } else {
                    changed = true;
                    device.wrap_key(
                        &workspace,
                        &object.object_id,
                        epoch,
                        &recipient.device_id,
                        recipient_key,
                        key,
                    )?
                };
                if existing.is_none() {
                    pending.push(envelope.clone());
                }
                envelopes.push(PolicyEnvelope::from(envelope));
            }
            objects.push(AccessObject {
                document: None,
                object_id: object.object_id.clone(),
                epoch,
                grants,
                envelopes,
            });
        }
        if !changed {
            return Ok(());
        }
        if role == WorkspaceRole::Editor {
            for batch in pending.chunks(100) {
                self.share_keys(batch).await?;
            }
            return Ok(());
        }
        let revision = revision
            .checked_add(1)
            .filter(|value| *value <= i64::MAX as u64)
            .ok_or_else(|| invalid("sync_invalid_revision"))?;
        let previous_digest = state
            .policy
            .as_ref()
            .map(AccessPolicy::digest)
            .transpose()?;
        let policy = AccessPolicy::sign(
            &workspace,
            &revision.to_string(),
            previous_digest.clone(),
            device,
            state.members,
            objects,
        )?;
        self.set_access(&policy, &device.signer().public_key())
            .await?;
        engine.sync_accept_access_policy(&state.revision, previous_digest.as_deref(), &policy)
    }
}
