//! Access transitions: deciding when the reader set changed and preparing
//! the key rotation that follows.

use super::*;

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

/// Whether the currently delivered recipients differ from the approved
/// recipients that should hold an envelope. A device that is enrolled but not
/// yet approved locally cannot receive a wrapped key, so it must not force a key
/// rotation or block delivery to approved devices.
pub(super) fn readers_changed(
    state: &AccessState,
    config: &WorkspaceSyncConfig,
    device: &DeviceKeys,
) -> bool {
    let approved: BTreeSet<&str> = state
        .devices
        .iter()
        .filter(|candidate| device_is_approved(candidate, config, device))
        .map(|candidate| candidate.device_id.as_str())
        .collect();
    state.objects.iter().any(|object| {
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
                approved.contains(candidate.device_id.as_str())
                    && (state
                        .members
                        .iter()
                        .any(|member| member.account_id == candidate.account_id)
                        || grants
                            .iter()
                            .any(|grant| grant.account_id == candidate.account_id))
            })
            .map(|candidate| candidate.device_id.as_str())
            .collect();
        expected.sort_unstable();
        let mut actual: Vec<_> = state
            .envelopes
            .iter()
            .filter(|envelope| {
                envelope.object_id == object.object_id
                    && envelope.epoch == object.epoch
                    && approved.contains(envelope.device_id.as_str())
            })
            .map(|envelope| envelope.device_id.as_str())
            .collect();
        actual.sort_unstable();
        actual != expected
    })
}

/// True when a current-epoch envelope exists for a device the relay no longer
/// lists for this workspace. A revoked device disappears from the active roster
/// but its envelope lingers until a rotation replaces the epoch, so observing
/// this orphan must force a rotation instead of aborting the pass.
pub(super) fn orphan_reader(state: &AccessState) -> bool {
    state.envelopes.iter().any(|envelope| {
        state
            .objects
            .iter()
            .any(|object| object.object_id == envelope.object_id && object.epoch == envelope.epoch)
            && !state
                .devices
                .iter()
                .any(|device| device.device_id == envelope.device_id)
    })
}

pub(super) fn prepare_rotation_transition(
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
