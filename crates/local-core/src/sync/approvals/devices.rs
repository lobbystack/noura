//! Device approvals and invitations, driven by the sync coordinator.

use super::*;

impl WorkspaceSyncCoordinator {
    pub async fn devices(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
    ) -> Result<Vec<SyncDevice>> {
        engine.require_sync_plugin("sync_devices")?;
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let own = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &own)?;
        let state = connection
            .transport(store)?
            .access_state(&engine.manifest().id)
            .await?;
        state
            .devices
            .into_iter()
            .map(|device| sync_device(device, &config, &own))
            .collect()
    }

    /// Explicitly approved public fingerprint, verified on the other device outside this server.
    pub async fn approve_device(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        device_id: &str,
        fingerprint: &str,
    ) -> Result<()> {
        engine.require_sync_plugin("sync_approve_device")?;
        let mut config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let devices = Self::devices(engine, connection, store).await?;
        let device = devices
            .into_iter()
            .find(|device| device.device_id == device_id && device.fingerprint == fingerprint)
            .ok_or_else(|| invalid("sync_device_changed"))?;
        if config
            .trusted_devices
            .get(device_id)
            .is_some_and(|key| key != &device.public_key)
            || config
                .approved_recipients
                .get(device_id)
                .is_some_and(|key| key != &device.encryption_recipient)
            || config
                .approved_accounts
                .get(device_id)
                .is_some_and(|account| account != &device.account_id)
        {
            return Err(invalid("sync_device_changed"));
        }
        config
            .trusted_devices
            .insert(device.device_id.clone(), device.public_key);
        config
            .approved_recipients
            .insert(device.device_id.clone(), device.encryption_recipient);
        config
            .approved_accounts
            .insert(device.device_id, device.account_id);
        engine.sync_save_configuration(&config)
    }

    pub async fn create_invitation(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        role: SyncInvitationRole,
    ) -> Result<SyncInvitationLink> {
        engine.require_sync_plugin("sync_create_invitation")?;
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let own = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &own)?;
        connection
            .transport(store)?
            .create_invitation(&engine.manifest().id, role)
            .await
    }

    pub async fn invitations(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
    ) -> Result<Vec<SyncInvitation>> {
        engine.require_sync_plugin("sync_invitations")?;
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let own = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &own)?;
        connection
            .transport(store)?
            .invitations(&engine.manifest().id)
            .await?
            .into_iter()
            .map(|invitation| {
                let devices = invitation
                    .devices
                    .into_iter()
                    .map(|device| sync_device(device, &config, &own))
                    .collect::<Result<Vec<_>>>()?;
                Ok(SyncInvitation {
                    id: invitation.id,
                    role: invitation.role,
                    account_id: invitation.account_id,
                    status: invitation.status,
                    expires_at: invitation.expires_at,
                    devices,
                })
            })
            .collect()
    }

    pub async fn approve_invited_device(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        invitation_id: &str,
        device_id: &str,
        fingerprint: &str,
    ) -> Result<()> {
        engine.require_sync_plugin("sync_approve_invited_device")?;
        identifier(invitation_id)?;
        let invitation = Self::invitations(engine, connection, store)
            .await?
            .into_iter()
            .find(|invitation| invitation.id == invitation_id)
            .ok_or_else(|| invalid("sync_invitation_unavailable"))?;
        let device = invitation
            .devices
            .iter()
            .find(|device| device.device_id == device_id && device.fingerprint == fingerprint)
            .cloned()
            .ok_or_else(|| invalid("sync_device_changed"))?;
        let mut config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        approve_device_card(&mut config, device)?;
        engine.sync_save_configuration(&config)?;
        let ready = invitation.status == SyncInvitationStatus::Accepted
            && !invitation.devices.is_empty()
            && invitation.devices.iter().all(|candidate| {
                config.approved_accounts.get(&candidate.device_id) == Some(&candidate.account_id)
                    && config.trusted_devices.get(&candidate.device_id)
                        == Some(&candidate.public_key)
                    && config.approved_recipients.get(&candidate.device_id)
                        == Some(&candidate.encryption_recipient)
            });
        if ready {
            let transport = connection.transport(store)?;
            if transport.access_transitions_available().await? {
                Self::finalize_invitation(engine, connection, store, invitation_id).await?;
            }
        }
        Ok(())
    }

    pub async fn revoke_invitation(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        invitation_id: &str,
    ) -> Result<()> {
        engine.require_sync_plugin("sync_revoke_invitation")?;
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let own = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &own)?;
        connection
            .transport(store)?
            .revoke_invitation(&engine.manifest().id, invitation_id)
            .await
    }

    pub async fn finalize_invitation(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        invitation_id: &str,
    ) -> Result<()> {
        engine.require_sync_plugin("sync_finalize_invitation")?;
        identifier(invitation_id)?;
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let transport = connection.transport(store)?;
        if let Some(transition) = engine.sync_pending_transition()? {
            if !transport.access_transitions_available().await? {
                return Err(invalid("sync_access_transition_unavailable"));
            }
            transport
                .ensure_workspace_capability(
                    &engine.manifest().id,
                    &device,
                    &config.trusted_devices,
                )
                .await?;
            return finish_access_transition(engine, &transport, &device, &config, &transition)
                .await;
        }
        let invitation = transport
            .invitations(&engine.manifest().id)
            .await?
            .into_iter()
            .find(|invitation| invitation.id == invitation_id)
            .filter(|invitation| invitation.status == SyncInvitationStatus::Accepted)
            .ok_or_else(|| invalid("sync_invitation_not_ready"))?;
        let account_id = invitation
            .account_id
            .clone()
            .ok_or_else(|| invalid("sync_invitation_not_ready"))?;
        if invitation.devices.is_empty() {
            return Err(invalid("sync_invitation_device_required"));
        }
        for invited in &invitation.devices {
            let card = sync_device(invited.clone(), &config, &device)?;
            if !card.approved {
                return Err(invalid("sync_device_approval_required"));
            }
        }
        if !transport.access_transitions_available().await? {
            return Err(invalid("sync_access_transition_unavailable"));
        }
        transport
            .ensure_workspace_capability(&engine.manifest().id, &device, &config.trusted_devices)
            .await?;

        let workspace = engine.manifest().id;
        let mut state = transport.access_state(&workspace).await?;
        if state.revision() != engine.sync_access_revision()? {
            transport.refresh_access_policies(engine, &config).await?;
            state = transport.access_state(&workspace).await?;
        }
        if state.verified_role(engine, &device, &config)? != WorkspaceRole::Owner {
            return Err(invalid("sync_owner_required"));
        }
        if state
            .members
            .iter()
            .any(|member| member.account_id == account_id)
        {
            return Err(invalid("sync_invitation_already_member"));
        }
        let mut members = state.members.clone();
        members.push(AccessMember {
            account_id: account_id.clone(),
            role: match invitation.role {
                SyncInvitationRole::Admin => WorkspaceRole::Admin,
                SyncInvitationRole::Editor => WorkspaceRole::Editor,
                SyncInvitationRole::Viewer => WorkspaceRole::Viewer,
            },
        });
        members.sort_by(|left, right| left.account_id.cmp(&right.account_id));
        let mut recipients = state.devices.clone();
        for invited in invitation.devices {
            if let Some(existing) = recipients
                .iter()
                .find(|existing| existing.device_id == invited.device_id)
            {
                if existing.account_id != invited.account_id
                    || existing.public_key != invited.public_key
                    || existing.encryption_recipient != invited.encryption_recipient
                {
                    return Err(invalid("sync_device_changed"));
                }
            } else {
                recipients.push(invited);
            }
        }
        recipients.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        let covered_sequence = engine.sync_cursor()?;
        let mut objects = Vec::new();
        let mut prepared = Vec::new();
        for object in &state.objects {
            let old_epoch = super::transport::parse_cursor(&object.epoch)?;
            let previous = state.policy.as_ref().and_then(|policy| {
                policy
                    .objects
                    .iter()
                    .find(|old| old.object_id == object.object_id)
            });
            if previous.map_or(old_epoch != 1, |old| old.epoch != old_epoch) {
                return Err(invalid("sync_invalid_access_state"));
            }
            let epoch = old_epoch
                .checked_add(1)
                .filter(|epoch| *epoch <= i64::MAX as u64)
                .ok_or_else(|| invalid("sync_invalid_epoch"))?;
            let key = ObjectKey::generate();
            let generation = uuid::Uuid::new_v4().to_string();
            let (content, blob, mode) = engine
                .collaboration_prepare_checkpoint_content(&object.object_id, &generation, &key)
                .map_err(|mut error| {
                    error.object_id = Some(object.object_id.clone());
                    error
                })?;
            let grants = previous.map(|old| old.grants.clone()).unwrap_or_default();
            let mut envelopes = Vec::new();
            for recipient in &recipients {
                if !members
                    .iter()
                    .any(|member| member.account_id == recipient.account_id)
                    && !grants
                        .iter()
                        .any(|grant| grant.account_id == recipient.account_id)
                {
                    continue;
                }
                let age = recipient
                    .encryption_recipient
                    .as_ref()
                    .ok_or_else(|| invalid("sync_device_upgrade_required"))?;
                if config.approved_recipients.get(&recipient.device_id) != Some(age)
                    || config.approved_accounts.get(&recipient.device_id)
                        != Some(&recipient.account_id)
                    || config.trusted_devices.get(&recipient.device_id)
                        != Some(&recipient.public_key)
                {
                    return Err(invalid("sync_device_approval_required"));
                }
                let envelope = device.wrap_key(
                    &workspace,
                    &object.object_id,
                    epoch,
                    &recipient.device_id,
                    age,
                    &key,
                )?;
                envelopes.push(PolicyEnvelope::from(envelope));
            }
            objects.push(AccessObject {
                document: Some(DocumentDescriptor {
                    generation: generation.clone(),
                    mode,
                }),
                object_id: object.object_id.clone(),
                epoch,
                grants,
                envelopes,
            });
            prepared.push((object.object_id.clone(), epoch, key, content, blob));
        }
        let revision = super::transport::parse_cursor(&state.revision)?
            .checked_add(1)
            .filter(|revision| *revision <= i64::MAX as u64)
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
            &device,
            members,
            objects,
        )?;
        let mut checkpoints = Vec::with_capacity(prepared.len());
        let mut blobs = Vec::new();
        for (object_id, epoch, key, content, blob) in prepared {
            checkpoints.push(
                EncryptedCheckpoint::seal(&device, &key, &policy, &covered_sequence, &content)
                    .map_err(|mut error| {
                        error.object_id = Some(object_id.clone());
                        error
                    })?,
            );
            if let Some(blob) = blob {
                blobs.push(CheckpointBlobManifest {
                    object_id,
                    epoch,
                    ciphertext_digest: blob.id,
                    ciphertext_size: blob.size,
                });
            }
        }
        let transition = AccessTransition::sign_with_blobs(
            &device,
            policy,
            covered_sequence,
            checkpoints,
            blobs,
        )?;
        finish_access_transition(engine, &transport, &device, &config, &transition).await
    }
}

/// A recipient device is deliverable when its pinned signing key, account, and
/// recipient match the locally approved configuration.
pub(super) fn device_is_approved(
    candidate: &RemoteDevice,
    config: &WorkspaceSyncConfig,
    device: &DeviceKeys,
) -> bool {
    config.approved_accounts.get(&candidate.device_id) == Some(&candidate.account_id)
        && config.trusted_devices.get(&candidate.device_id) == Some(&candidate.public_key)
        && (candidate.device_id == device.device_id()
            || candidate
                .encryption_recipient
                .as_ref()
                .is_some_and(|recipient| {
                    config.approved_recipients.get(&candidate.device_id) == Some(recipient)
                }))
}

pub(super) fn sync_device(
    device: RemoteDevice,
    config: &WorkspaceSyncConfig,
    own: &DeviceKeys,
) -> Result<SyncDevice> {
    let recipient = device
        .encryption_recipient
        .ok_or_else(|| invalid("sync_device_upgrade_required"))?;
    let fingerprint = device_fingerprint(
        &device.device_id,
        &device.account_id,
        &device.public_key,
        &recipient,
    )?;
    let approved = config.approved_accounts.get(&device.device_id) == Some(&device.account_id)
        && config.trusted_devices.get(&device.device_id) == Some(&device.public_key)
        && (device.device_id == own.device_id()
            || config.approved_recipients.get(&device.device_id) == Some(&recipient));
    Ok(SyncDevice {
        device_id: device.device_id,
        account_id: device.account_id,
        public_key: device.public_key,
        encryption_recipient: recipient,
        fingerprint,
        approved,
    })
}

pub(super) fn approve_device_card(
    config: &mut WorkspaceSyncConfig,
    device: SyncDevice,
) -> Result<()> {
    if config
        .trusted_devices
        .get(&device.device_id)
        .is_some_and(|key| key != &device.public_key)
        || config
            .approved_recipients
            .get(&device.device_id)
            .is_some_and(|key| key != &device.encryption_recipient)
        || config
            .approved_accounts
            .get(&device.device_id)
            .is_some_and(|account| account != &device.account_id)
    {
        return Err(invalid("sync_device_changed"));
    }
    config
        .trusted_devices
        .insert(device.device_id.clone(), device.public_key);
    config
        .approved_recipients
        .insert(device.device_id.clone(), device.encryption_recipient);
    config
        .approved_accounts
        .insert(device.device_id, device.account_id);
    Ok(())
}
