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
    super::crypto::decode(public_key, 32, 32)?;
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
            .into_iter()
            .find(|device| device.device_id == device_id && device.fingerprint == fingerprint)
            .ok_or_else(|| invalid("sync_device_changed"))?;
        let mut config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        approve_device_card(&mut config, device)?;
        engine.sync_save_configuration(&config)
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
        let secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        let mut objects = Vec::new();
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
                let envelope = if let Some(existing) = state.envelopes.iter().find(|entry| {
                    entry.object_id == object.object_id
                        && entry.epoch == object.epoch
                        && entry.device_id == recipient.device_id
                }) {
                    let envelope = KeyEnvelope {
                        workspace_id: workspace.clone(),
                        object_id: object.object_id.clone(),
                        epoch,
                        device_id: existing.device_id.clone(),
                        wrapped_key: existing.wrapped_key.clone(),
                        signing_device: existing.signing_device.clone(),
                        signature: existing.signature.clone(),
                    };
                    let signer = config
                        .trusted_devices
                        .get(&envelope.signing_device)
                        .ok_or_else(|| invalid("sync_untrusted_device"))?;
                    envelope.resign(&device, signer)?
                } else {
                    device.wrap_key(
                        &workspace,
                        &object.object_id,
                        epoch,
                        &recipient.device_id,
                        age,
                        key,
                    )?
                };
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
        transport
            .set_access(&policy, &device.signer().public_key())
            .await?;
        engine.sync_accept_access_policy(&state.revision, previous_digest.as_deref(), &policy)
    }
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
                    let envelope = KeyEnvelope {
                        workspace_id: workspace.clone(),
                        object_id: object.object_id.clone(),
                        epoch,
                        device_id: existing.device_id.clone(),
                        wrapped_key: existing.wrapped_key.clone(),
                        signing_device: existing.signing_device.clone(),
                        signature: existing.signature.clone(),
                    };
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
