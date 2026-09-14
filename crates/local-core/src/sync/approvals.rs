use std::collections::BTreeSet;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::*;
use crate::{Result, WorkspaceEngine};

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncDevice {
    pub device_id: String,
    pub account_id: String,
    pub public_key: String,
    pub encryption_recipient: String,
    pub fingerprint: String,
    pub approved: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum SyncInvitationRole {
    Admin,
    Editor,
    Viewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum SyncInvitationStatus {
    Pending,
    Accepted,
    Completed,
    Expired,
    Revoked,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SyncInvitation {
    pub id: String,
    pub role: SyncInvitationRole,
    pub account_id: Option<String>,
    pub status: SyncInvitationStatus,
    pub expires_at: String,
    pub devices: Vec<SyncDevice>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct SyncInvitationLink {
    pub id: String,
    pub role: SyncInvitationRole,
    pub invite_url: String,
    pub expires_at: String,
}

pub fn device_fingerprint(
    device: &str,
    account: &str,
    public_key: &str,
    recipient: &str,
) -> Result<String> {
    identifier(device)?;
    identifier(account)?;
    let public: [u8; 32] = super::crypto::decode(public_key, 32, 32)?
        .try_into()
        .map_err(|_| invalid("sync_invalid_key"))?;
    if recipient.starts_with(sync_key_envelope::RECIPIENT_PREFIX) {
        sync_key_envelope::decode_recipient(recipient)
            .map_err(|_| invalid("sync_invalid_recipient"))?;
        return Ok(sync_key_envelope::device_fingerprint(
            device, account, public, recipient,
        ));
    }
    recipient
        .parse::<age::x25519::Recipient>()
        .map_err(|_| invalid("sync_invalid_recipient"))?;
    let bytes = serde_json::to_vec(&(
        "noura.device.card",
        1,
        device,
        account,
        public_key,
        recipient,
    ))
    .map_err(|_| invalid("sync_serialize_failed"))?;
    Ok(blake3::hash(&bytes).to_hex().to_string())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct AccessState {
    revision: String,
    members: Vec<AccessMember>,
    objects: Vec<ObjectEpoch>,
    envelopes: Vec<StoredEnvelope>,
    devices: Vec<RemoteDevice>,
    policy: Option<AccessPolicy>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ObjectEpoch {
    object_id: String,
    epoch: String,
    generation: Option<String>,
    document_mode: Option<DocumentMode>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteDevice {
    pub device_id: String,
    pub account_id: String,
    pub public_key: String,
    pub encryption_recipient: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct RemoteInvitation {
    pub id: String,
    pub role: SyncInvitationRole,
    pub account_id: Option<String>,
    pub expires_at: String,
    pub status: SyncInvitationStatus,
    pub devices: Vec<RemoteDevice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredEnvelope {
    object_id: String,
    epoch: String,
    device_id: String,
    wrapped_key: String,
    signing_device: String,
    signature: String,
    #[serde(default)]
    construction: KeyConstruction,
    #[serde(default)]
    recipient_public_key: Option<String>,
    #[serde(default)]
    ephemeral_public_key: Option<String>,
    #[serde(default)]
    salt: Option<String>,
    #[serde(default)]
    nonce: Option<String>,
}

impl StoredEnvelope {
    /// Reconstruct the discriminated envelope for this workspace and epoch.
    fn to_key_envelope(&self, workspace: &str, object: &str, epoch: u64) -> KeyEnvelope {
        KeyEnvelope {
            workspace_id: workspace.into(),
            object_id: object.into(),
            epoch,
            device_id: self.device_id.clone(),
            wrapped_key: self.wrapped_key.clone(),
            signing_device: self.signing_device.clone(),
            signature: self.signature.clone(),
            construction: self.construction,
            recipient_public_key: self.recipient_public_key.clone(),
            ephemeral_public_key: self.ephemeral_public_key.clone(),
            salt: self.salt.clone(),
            nonce: self.nonce.clone(),
        }
    }
}

impl WorkspaceSyncCoordinator {
    pub async fn devices(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
    ) -> Result<Vec<SyncDevice>> {
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

pub(crate) async fn finish_access_transition(
    engine: &WorkspaceEngine,
    transport: &HttpSyncTransport,
    device: &DeviceKeys,
    config: &WorkspaceSyncConfig,
    transition: &AccessTransition,
) -> Result<()> {
    let public = device.signer().public_key();
    transition.verify(&public)?;
    if transition.policy.device_id != device.device_id() {
        return Err(invalid("sync_transition_owner_changed"));
    }
    engine.sync_prepare_transition(transition, &public)?;
    for object in &transition.policy.objects {
        let envelope = object
            .envelopes
            .iter()
            .find(|envelope| envelope.device_id == device.device_id())
            .ok_or_else(|| invalid("sync_key_required"))?;
        engine.sync_store_key(
            &envelope.to_key_envelope(
                &transition.policy.workspace_id,
                &object.object_id,
                object.epoch,
                &transition.policy.device_id,
            )?,
            device,
            &public,
        )?;
    }
    transport
        .submit_transition(engine, transition, &public)
        .await?;
    let current_revision = engine.sync_access_revision()?;
    if current_revision != transition.policy.revision {
        let current_digest = engine
            .sync_access_policy()?
            .as_ref()
            .map(AccessPolicy::digest)
            .transpose()?;
        engine.sync_accept_access_policy(
            &current_revision,
            current_digest.as_deref(),
            &transition.policy,
        )?;
    } else if engine
        .sync_access_policy()?
        .as_ref()
        .map(AccessPolicy::digest)
        .transpose()?
        .as_deref()
        != Some(transition.policy.digest()?.as_str())
    {
        return Err(invalid("sync_policy_chain_changed"));
    }
    let mut secrets = engine.sync_restore_secrets(device, &config.trusted_devices)?;
    (
        secrets.authorized_workspace_writers,
        secrets.authorized_object_writers,
    ) = policy_authorizations(&transition.policy, config);
    engine.sync_record_access_authorization(
        &transition.policy.revision,
        &secrets.authorized_workspace_writers,
        &secrets.authorized_object_writers,
    )?;
    engine.sync_load_access_authorizations(&mut secrets)?;
    transport
        .receive_checkpoints(engine, device, &secrets)
        .await
}

fn prepare_rotation_transition(
    engine: &WorkspaceEngine,
    device: &DeviceKeys,
    config: &WorkspaceSyncConfig,
    state: &AccessState,
    members: Vec<AccessMember>,
    mut recipients: Vec<RemoteDevice>,
) -> Result<AccessTransition> {
    recipients.sort_by(|left, right| left.device_id.cmp(&right.device_id));
    let workspace = engine.manifest().id;
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
            let expected = if recipient.device_id == device.device_id() {
                device.recipient()
            } else {
                config
                    .approved_recipients
                    .get(&recipient.device_id)
                    .cloned()
                    .ok_or_else(|| invalid("sync_device_approval_required"))?
            };
            if &expected != age
                || config.approved_accounts.get(&recipient.device_id) != Some(&recipient.account_id)
                || config.trusted_devices.get(&recipient.device_id) != Some(&recipient.public_key)
            {
                return Err(invalid("sync_device_changed"));
            }
            envelopes.push(PolicyEnvelope::from(device.wrap_key(
                &workspace,
                &object.object_id,
                epoch,
                &recipient.device_id,
                age,
                &key,
            )?));
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
        previous_digest,
        device,
        members,
        objects,
    )?;
    let mut checkpoints = Vec::with_capacity(prepared.len());
    let mut blobs = Vec::new();
    for (object_id, epoch, key, content, blob) in prepared {
        checkpoints.push(
            EncryptedCheckpoint::seal(device, &key, &policy, &covered_sequence, &content).map_err(
                |mut error| {
                    error.object_id = Some(object_id.clone());
                    error
                },
            )?,
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
    AccessTransition::sign_with_blobs(device, policy, covered_sequence, checkpoints, blobs)
}

fn sync_device(
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

fn approve_device_card(config: &mut WorkspaceSyncConfig, device: SyncDevice) -> Result<()> {
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

impl AccessState {
    pub(crate) fn revision(&self) -> &str {
        &self.revision
    }

    pub(crate) fn verified_role(
        &self,
        engine: &WorkspaceEngine,
        device: &DeviceKeys,
        config: &WorkspaceSyncConfig,
    ) -> Result<WorkspaceRole> {
        let workspace = engine.manifest().id;
        let revision = super::transport::parse_cursor(&self.revision)?;
        if revision != super::transport::parse_cursor(&engine.sync_access_revision()?)?
            || self.objects.len() > 1000
            || self.devices.len() > 1000
        {
            return Err(invalid("sync_invalid_access_state"));
        }
        let own = self
            .devices
            .iter()
            .find(|entry| entry.device_id == device.device_id())
            .ok_or_else(|| invalid("sync_access_denied"))?;
        if own.public_key != device.signer().public_key()
            || config.approved_accounts.get(device.device_id()) != Some(&own.account_id)
            || own.encryption_recipient.as_deref() != Some(&device.recipient())
        {
            return Err(invalid("sync_device_changed"));
        }
        if let Some(policy) = &self.policy {
            let accepted = engine
                .sync_access_policy()?
                .ok_or_else(|| invalid("sync_policy_chain_changed"))?;
            if accepted.digest()? != policy.digest()? {
                return Err(invalid("sync_policy_chain_changed"));
            }
            let signer = config
                .trusted_devices
                .get(&policy.device_id)
                .ok_or_else(|| invalid("sync_untrusted_device"))?;
            policy.verify(signer)?;
            if policy.revision != self.revision
                || policy.workspace_id != workspace
                || serde_json::to_value(&policy.members).ok()
                    != serde_json::to_value(&self.members).ok()
                || policy.objects.iter().any(|object| {
                    !self.objects.iter().any(|state| {
                        state.object_id == object.object_id
                            && state.epoch == object.epoch.to_string()
                    })
                })
            {
                return Err(invalid("sync_invalid_access_state"));
            }
            for object in &self.objects {
                super::transport::parse_cursor(&object.epoch)?;
                match (&object.generation, object.document_mode) {
                    (Some(generation), Some(mode)) if policy.version == 2 => {
                        identifier(generation)?;
                        if let Some(signed) = policy
                            .objects
                            .iter()
                            .find(|signed| signed.object_id == object.object_id)
                            && signed.document.as_ref()
                                != Some(&DocumentDescriptor {
                                    generation: generation.clone(),
                                    mode,
                                })
                        {
                            return Err(invalid("sync_invalid_access_state"));
                        }
                    }
                    (None, None) if policy.version == 1 => {}
                    _ => return Err(invalid("sync_invalid_access_state")),
                }
            }
        } else if engine.sync_access_policy()?.is_some()
            || revision != 0
            || self.members.len() != 1
            || self.members[0].account_id != own.account_id
            || self.members[0].role != WorkspaceRole::Owner
        {
            return Err(invalid("sync_invalid_access_state"));
        }
        self.members
            .iter()
            .find(|member| member.account_id == own.account_id)
            .map(|member| member.role)
            .ok_or_else(|| invalid("sync_access_denied"))
    }

    pub(crate) fn authorize_writers(
        &self,
        engine: &WorkspaceEngine,
        device: &DeviceKeys,
        config: &WorkspaceSyncConfig,
        secrets: &mut SyncSecrets,
    ) -> Result<WorkspaceRole> {
        let role = self.verified_role(engine, device, config)?;
        let (workspace_writers, object_writers) = authorizations(
            &self.members,
            self.policy.as_ref().map(|policy| policy.objects.as_slice()),
            config,
        );
        secrets.authorized_workspace_writers = workspace_writers;
        secrets.authorized_object_writers = object_writers;
        engine.sync_record_access_authorization(
            &self.revision,
            &secrets.authorized_workspace_writers,
            &secrets.authorized_object_writers,
        )?;
        engine.sync_load_access_authorizations(secrets)?;
        Ok(role)
    }
}

pub(crate) fn policy_authorizations(
    policy: &AccessPolicy,
    config: &WorkspaceSyncConfig,
) -> (BTreeSet<String>, BTreeSet<(String, String)>) {
    authorizations(&policy.members, Some(&policy.objects), config)
}

fn authorizations(
    members: &[AccessMember],
    objects: Option<&[AccessObject]>,
    config: &WorkspaceSyncConfig,
) -> (BTreeSet<String>, BTreeSet<(String, String)>) {
    let mut workspace_writers = BTreeSet::new();
    let mut object_writers = BTreeSet::new();
    for (device_id, account_id) in &config.approved_accounts {
        if !config.trusted_devices.contains_key(device_id) {
            continue;
        }
        if members
            .iter()
            .any(|member| member.account_id == *account_id && member.role != WorkspaceRole::Viewer)
        {
            workspace_writers.insert(device_id.clone());
        }
        if let Some(objects) = objects {
            for object in objects {
                if object.grants.iter().any(|grant| {
                    grant.account_id == *account_id && grant.role == ObjectRole::Editor
                }) {
                    object_writers.insert((object.object_id.clone(), device_id.clone()));
                }
            }
        }
    }
    (workspace_writers, object_writers)
}

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
            let age = recipient
                .encryption_recipient
                .as_ref()
                .ok_or_else(|| invalid("sync_device_upgrade_required"))?;
            // Object activation still carries only the three-field native envelope.
            if age.starts_with(sync_key_envelope::RECIPIENT_PREFIX) {
                return Err(invalid("sync_browser_activation_unsupported"));
            }
            let expected_age = if recipient.device_id == device.device_id() {
                device.recipient()
            } else {
                config
                    .approved_recipients
                    .get(&recipient.device_id)
                    .cloned()
                    .ok_or_else(|| invalid("sync_device_approval_required"))?
            };
            if &expected_age != age
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
                age,
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
        {
            let changes_readers = state.objects.iter().any(|object| {
                let grants = state
                    .policy
                    .as_ref()
                    .and_then(|policy| {
                        policy
                            .objects
                            .iter()
                            .find(|entry| entry.object_id == object.object_id)
                    })
                    .map(|entry| entry.grants.as_slice())
                    .unwrap_or_default();
                let mut expected: Vec<_> = state
                    .devices
                    .iter()
                    .filter(|candidate| {
                        state
                            .members
                            .iter()
                            .any(|member| member.account_id == candidate.account_id)
                            || grants
                                .iter()
                                .any(|grant| grant.account_id == candidate.account_id)
                    })
                    .map(|candidate| candidate.device_id.as_str())
                    .collect();
                expected.sort_unstable();
                let mut actual: Vec<_> = state
                    .envelopes
                    .iter()
                    .filter(|envelope| {
                        envelope.object_id == object.object_id && envelope.epoch == object.epoch
                    })
                    .map(|envelope| envelope.device_id.as_str())
                    .collect();
                actual.sort_unstable();
                actual != expected
            });
            if changes_readers {
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
        }
        let revision = super::transport::parse_cursor(&state.revision)?;
        let mut pending = Vec::new();
        let mut objects = Vec::new();
        let mut changed = false;
        if state.envelopes.iter().any(|envelope| {
            state.objects.iter().any(|object| {
                object.object_id == envelope.object_id && object.epoch == envelope.epoch
            }) && !state
                .devices
                .iter()
                .any(|device| device.device_id == envelope.device_id)
        }) {
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
                let age = recipient
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
                if &expected != age
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
                        age,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::BTreeMap};
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, Zeroizing<String>>>);
    impl SyncCredentials for Memory {
        fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
            self.0
                .borrow()
                .get(reference)
                .cloned()
                .ok_or_else(|| invalid("test_missing"))
        }
        fn write(&self, reference: &str, value: &str) -> Result<()> {
            self.0
                .borrow_mut()
                .insert(reference.into(), Zeroizing::new(value.into()));
            Ok(())
        }
    }

    fn remote(device: &DeviceKeys, account: &str, recipient: String) -> RemoteDevice {
        RemoteDevice {
            device_id: device.device_id().into(),
            account_id: account.into(),
            public_key: device.signer().public_key(),
            encryption_recipient: Some(recipient),
        }
    }

    #[test]
    fn browser_device_fingerprint_matches_the_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/browser-device-v1.json"
        ))
        .unwrap();
        assert_eq!(fixture["fingerprint"]["domain"], "noura.device.card.web");
        let vector = &fixture["fingerprint"]["vectors"][0];
        assert_eq!(
            device_fingerprint(
                vector["device_id"].as_str().unwrap(),
                vector["account_id"].as_str().unwrap(),
                vector["signing_public"].as_str().unwrap(),
                vector["recipient"].as_str().unwrap(),
            )
            .unwrap(),
            vector["expected_hex"].as_str().unwrap()
        );
    }

    #[test]
    fn mixed_age_and_browser_cards_approve_and_reject_changed_browser_cards() {
        let store = Memory::default();
        let own = DeviceKeys::create(&store).unwrap();
        let browser = DeviceKeys::create_browser(&store).unwrap();
        let native = DeviceKeys::create(&store).unwrap();
        let mut config = WorkspaceSyncConfig {
            version: 1,
            workspace_id: "workspace".into(),
            origin: "https://sync.example.com".into(),
            device_id: own.device_id().into(),
            enabled: true,
            trusted_devices: BTreeMap::from([(own.device_id().into(), own.signer().public_key())]),
            approved_recipients: BTreeMap::from([(own.device_id().into(), own.recipient())]),
            approved_accounts: BTreeMap::from([(own.device_id().into(), "account".into())]),
        };
        let browser_card = sync_device(
            remote(&browser, "account", browser.recipient()),
            &config,
            &own,
        )
        .unwrap();
        assert!(!browser_card.approved);
        assert_ne!(
            browser_card.fingerprint,
            device_fingerprint(
                browser.device_id(),
                "account",
                &browser.signer().public_key(),
                &native.recipient(),
            )
            .unwrap()
        );
        approve_device_card(&mut config, browser_card.clone()).unwrap();
        let native_card = sync_device(
            remote(&native, "account", native.recipient()),
            &config,
            &own,
        )
        .unwrap();
        approve_device_card(&mut config, native_card).unwrap();
        assert_eq!(
            config.approved_recipients.get(browser.device_id()),
            Some(&browser.recipient())
        );
        assert_eq!(
            config.approved_recipients.get(native.device_id()),
            Some(&native.recipient())
        );
        assert!(
            browser
                .recipient()
                .starts_with(sync_key_envelope::RECIPIENT_PREFIX)
        );
        assert!(
            !native
                .recipient()
                .starts_with(sync_key_envelope::RECIPIENT_PREFIX)
        );

        let replacement = DeviceKeys::create_browser(&store).unwrap();
        let changed = sync_device(
            remote(&browser, "account", replacement.recipient()),
            &config,
            &own,
        )
        .unwrap();
        assert_ne!(changed.fingerprint, browser_card.fingerprint);
        assert_eq!(
            approve_device_card(&mut config, changed).unwrap_err().code,
            "sync_device_changed"
        );
        assert_eq!(
            config.approved_recipients.get(browser.device_id()),
            Some(&browser.recipient())
        );
    }
}
