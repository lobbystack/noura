use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{
    DeviceConnection, DeviceKeys, SyncCredentials, SyncPass, WorkspaceRole, identifier, invalid,
};
use crate::{Result, WorkspaceEngine};

/// Local consent and pinned sender keys. This file is never part of a synced payload.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceSyncConfig {
    pub version: u8,
    pub workspace_id: String,
    pub origin: String,
    pub device_id: String,
    pub enabled: bool,
    pub trusted_devices: BTreeMap<String, String>,
    #[serde(default)]
    pub approved_recipients: BTreeMap<String, String>,
    #[serde(default)]
    pub approved_accounts: BTreeMap<String, String>,
}

impl WorkspaceSyncConfig {
    pub(crate) fn validate(&self, workspace: &str) -> Result<()> {
        if self.version != 1 || self.workspace_id != workspace {
            return Err(invalid("sync_invalid_configuration"));
        }
        super::transport::validate_origin(&self.origin)?;
        identifier(&self.device_id)?;
        if self.trusted_devices.len() > 1000 || !self.trusted_devices.contains_key(&self.device_id)
        {
            return Err(invalid("sync_invalid_configuration"));
        }
        for (device, key) in &self.trusted_devices {
            identifier(device)?;
            let bytes: [u8; 32] = super::crypto::decode(key, 32, 32)?
                .try_into()
                .map_err(|_| invalid("sync_invalid_key"))?;
            ed25519_dalek::VerifyingKey::from_bytes(&bytes)
                .map_err(|_| invalid("sync_invalid_key"))?;
        }
        if self.approved_recipients.len() > 1000 {
            return Err(invalid("sync_invalid_configuration"));
        }
        for (device, recipient) in &self.approved_recipients {
            let key = self
                .trusted_devices
                .get(device)
                .ok_or_else(|| invalid("sync_invalid_configuration"))?;
            let account = self
                .approved_accounts
                .get(device)
                .ok_or_else(|| invalid("sync_device_approval_required"))?;
            super::device_fingerprint(device, account, key, recipient)?;
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSyncStatus {
    pub enabled: bool,
    pub phase: WorkspaceSyncPhase,
    pub pending: u32,
    pub conflicts: u32,
    pub error_code: Option<String>,
    pub last_success: Option<String>,
    pub transition: Option<WorkspaceSyncTransitionStatus>,
    pub activation: Option<WorkspaceSyncActivationStatus>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSyncActivationStatus {
    pub activation_id: String,
    pub object_id: String,
    pub path: Option<String>,
    pub installed: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSyncTransitionStatus {
    pub transition_id: String,
    pub phase: WorkspaceSyncTransitionPhase,
    pub objects: Vec<WorkspaceSyncTransitionObject>,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceSyncTransitionObject {
    pub object_id: String,
    pub path: Option<String>,
    pub installed: bool,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSyncTransitionPhase {
    Prepare,
    Stage,
    ResolveCommit,
    Install,
    Rebase,
    Complete,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceSyncPhase {
    Disabled,
    Paused,
    Idle,
    Syncing,
    Error,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteSyncWorkspace {
    pub id: String,
    pub role: String,
}

/// One pass at a time, serialized by the native host. Progress and consent survive process exit.
pub struct WorkspaceSyncCoordinator;

impl WorkspaceSyncCoordinator {
    pub fn collaboration_flush(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        session_id: &str,
    ) -> Result<()> {
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        engine.collaboration_flush(session_id, &secrets)
    }
    pub fn conflicts(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
    ) -> Result<Vec<super::SyncConflict>> {
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        engine.sync_conflicts(&secrets)
    }

    pub fn resolve_conflict(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        input: &super::ResolveSyncConflict,
    ) -> Result<()> {
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        engine.sync_resolve_conflict(input, &device, &secrets)
    }
    pub async fn join(
        root: impl AsRef<std::path::Path>,
        name: &str,
        workspace: &str,
        app_data: impl AsRef<std::path::Path>,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
    ) -> Result<WorkspaceEngine> {
        identifier(workspace)?;
        let remote = connection.transport(store)?.workspaces().await?;
        if !remote.iter().any(|entry| entry.id == workspace) {
            return Err(invalid("sync_membership_required"));
        }
        let engine = WorkspaceEngine::create_sync_replica(root, name, workspace, app_data)?;
        Self::enable(&engine, connection, store).await?;
        engine.sync_pause(true)?;
        Ok(engine)
    }
    pub async fn enable(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
    ) -> Result<()> {
        let device = DeviceKeys::load(store, &connection.device_id)?;
        if let Some(config) = engine.sync_configuration()? {
            Self::check_connection(&config, connection, &device)?;
        }
        let transport = connection.transport(store)?;
        if !transport
            .workspaces()
            .await?
            .iter()
            .any(|workspace| workspace.id == engine.manifest().id)
        {
            transport.create_workspace(&engine.manifest().id).await?;
        }
        let mut config = engine
            .sync_configuration()?
            .unwrap_or_else(|| WorkspaceSyncConfig {
                version: 1,
                workspace_id: engine.manifest().id,
                origin: connection.origin.clone(),
                device_id: connection.device_id.clone(),
                enabled: false,
                trusted_devices: BTreeMap::from([(
                    connection.device_id.clone(),
                    device.signer().public_key(),
                )]),
                approved_recipients: BTreeMap::from([(
                    connection.device_id.clone(),
                    device.recipient(),
                )]),
                approved_accounts: BTreeMap::from([(
                    connection.device_id.clone(),
                    connection.account_id.clone(),
                )]),
            });
        config.enabled = true;
        engine.sync_save_configuration(&config)
    }

    pub async fn pass(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
    ) -> Result<SyncPass> {
        Self::pass_inner(engine, connection, store, false).await
    }

    pub async fn pass_wait(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
    ) -> Result<SyncPass> {
        Self::pass_inner(engine, connection, store, true).await
    }

    async fn pass_inner(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        wait: bool,
    ) -> Result<SyncPass> {
        let persisted = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        if !persisted.enabled {
            return Err(invalid("sync_paused"));
        }
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&persisted, connection, &device)?;
        let transport = connection.transport(store)?;
        if let Some(transition) = engine.sync_pending_transition()? {
            super::approvals::finish_access_transition(
                engine,
                &transport,
                &device,
                &persisted,
                &transition,
            )
            .await?;
        }
        let mut access = transport.access_state(&engine.manifest().id).await?;
        if access.revision() != engine.sync_access_revision()? {
            // The chain is refreshed with the persisted pins, which keeps every
            // historical policy signature verifiable. Only after this succeeds
            // is the effective configuration safe to prune.
            transport
                .refresh_access_policies(engine, &persisted)
                .await?;
            access = transport.access_state(&engine.manifest().id).await?;
        }
        // Drop approvals for devices the relay no longer lists for this
        // workspace. A revoked device's stale recipient and account must not
        // keep it in the authorized-writer set or the sharing gate. Persist the
        // drop only once the local revision matches the relay head, so a later
        // chain refresh can still verify policies signed before the revocation.
        let config = access.effective_config(&persisted, device.device_id());
        if config != persisted && access.revision() == engine.sync_access_revision()? {
            engine.sync_save_configuration(&config)?;
        }
        let mut secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        // Fetch rotations before capturing new edits. Never select an epoch from an unverified key.
        transport
            .receive_keys(engine, &device, &mut secrets)
            .await?;
        let role = access.authorize_writers(engine, &device, &config, &mut secrets)?;
        transport
            .receive_activations(engine, &device, &mut secrets)
            .await?;
        transport
            .receive_checkpoints(engine, &device, &secrets)
            .await?;
        if role == WorkspaceRole::Viewer {
            return transport.synchronize_readonly(engine, &secrets, wait).await;
        }
        // Reach the relay's durable operation boundary before deriving a new object's
        // checkpoint. An activation never snapshots stale visible content.
        transport
            .synchronize_readonly(engine, &secrets, false)
            .await?;
        engine.sync_capture_workspace(&device, &mut secrets)?;
        let mut warnings = Vec::new();
        if config.approved_recipients.len() > 1 {
            let share = match transport
                .activate_pending_objects(engine, &device, &config, &secrets, &access)
                .await
            {
                Ok(true) => false,
                Ok(false) => true,
                Err(error) if activation_error_is_recoverable(&error) => {
                    // The pending activation stays durable and is retried; a
                    // browser recipient or a device still awaiting local
                    // approval must not stop already-shared objects from
                    // receiving their keys in this pass.
                    warnings.push(error.code.clone());
                    true
                }
                Err(error) => return Err(error),
            };
            if share {
                transport.prepare_objects(engine).await?;
                transport
                    .share_approved_keys(engine, &device, &config, &secrets)
                    .await?;
            }
        }
        let mut pass = if wait {
            transport.synchronize_wait(engine, &secrets).await?
        } else {
            transport.synchronize(engine, &secrets).await?
        };
        pass.warnings.extend(warnings);
        Ok(pass)
    }

    pub(crate) fn check_connection(
        config: &WorkspaceSyncConfig,
        connection: &DeviceConnection,
        device: &DeviceKeys,
    ) -> Result<()> {
        if config.origin != connection.origin
            || config.device_id != connection.device_id
            || config.approved_accounts.get(&connection.device_id) != Some(&connection.account_id)
            || config.trusted_devices.get(device.device_id()) != Some(&device.signer().public_key())
        {
            return Err(invalid("sync_connection_changed"));
        }
        Ok(())
    }
}

impl WorkspaceSyncCoordinator {
    pub fn collaboration_open(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        input: super::collaboration::CollaborationOpenInput,
    ) -> Result<Option<super::collaboration::CollaborationSession>> {
        let Some(config) = engine.sync_configuration()? else {
            return Ok(None);
        };
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let Some(policy) = engine.sync_access_policy()? else {
            return Ok(None);
        };
        let mut secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        (
            secrets.authorized_workspace_writers,
            secrets.authorized_object_writers,
        ) = super::approvals::policy_authorizations(&policy, &config);
        engine.collaboration_open(input, &device, &secrets)
    }
    pub fn collaboration_submit(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        input: super::collaboration::CollaborationSubmitInput,
    ) -> Result<super::collaboration::CollaborationReceipt> {
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let policy = engine
            .sync_access_policy()?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        let mut secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        (
            secrets.authorized_workspace_writers,
            secrets.authorized_object_writers,
        ) = super::approvals::policy_authorizations(&policy, &config);
        engine.collaboration_submit(input, &device, &secrets)
    }

    pub fn collaboration_update_object(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        id: &str,
        patch: crate::ObjectPatch,
    ) -> Result<crate::MutationResult<crate::WorkspaceObject>> {
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let policy = engine
            .sync_access_policy()?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        let mut secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        (
            secrets.authorized_workspace_writers,
            secrets.authorized_object_writers,
        ) = super::approvals::policy_authorizations(&policy, &config);
        engine.collaboration_update_object(id, patch, &device, &secrets)
    }

    pub fn collaboration_create_object(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        input: crate::CreateObjectInput,
    ) -> Result<crate::MutationResult<crate::WorkspaceObject>> {
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let policy = engine
            .sync_access_policy()?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        let mut secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        (
            secrets.authorized_workspace_writers,
            secrets.authorized_object_writers,
        ) = super::approvals::policy_authorizations(&policy, &config);
        if !secrets
            .authorized_workspace_writers
            .contains(device.device_id())
        {
            return Err(invalid("sync_writer_not_authorized"));
        }
        let mut result = engine.create_object(input)?;
        if engine
            .sync_capture_path(&result.value.relative_path, &device, &mut secrets)
            .is_err()
        {
            result.warnings.push(crate::CoreWarning {
                code: "collaboration_capture_pending".into(),
                message: "The object is saved locally and will be prepared for sharing on the next synchronization pass".into(),
            });
        }
        Ok(result)
    }

    pub fn collaboration_move_object(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        id: &str,
        destination: &str,
        expected_revision: &str,
    ) -> Result<crate::MutationResult<crate::WorkspaceObject>> {
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let policy = engine
            .sync_access_policy()?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        let mut secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        (
            secrets.authorized_workspace_writers,
            secrets.authorized_object_writers,
        ) = super::approvals::policy_authorizations(&policy, &config);
        engine.collaboration_move_object(id, destination, expected_revision, &device, &secrets)
    }

    pub fn collaboration_delete_object(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        id: &str,
        expected_revision: &str,
    ) -> Result<crate::MutationResult<crate::WorkspaceObject>> {
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let policy = engine
            .sync_access_policy()?
            .ok_or_else(|| invalid("collaboration_checkpoint_required"))?;
        let mut secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        (
            secrets.authorized_workspace_writers,
            secrets.authorized_object_writers,
        ) = super::approvals::policy_authorizations(&policy, &config);
        engine.collaboration_delete_object(id, expected_revision, &device, &secrets)
    }
}

/// Activation failures that must not abort a synchronization pass. The pending
/// activation remains durable and is retried, while already-shared objects still
/// receive keys. Every other error is still fatal.
fn activation_error_is_recoverable(error: &crate::CoreError) -> bool {
    matches!(
        error.code.as_str(),
        "sync_browser_activation_unsupported" | "sync_device_approval_required"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, String>>);
    impl SyncCredentials for Memory {
        fn read(&self, key: &str) -> Result<Zeroizing<String>> {
            self.0
                .borrow()
                .get(key)
                .cloned()
                .map(Zeroizing::new)
                .ok_or_else(|| invalid("test_missing"))
        }
        fn write(&self, key: &str, value: &str) -> Result<()> {
            self.0.borrow_mut().insert(key.into(), value.into());
            Ok(())
        }
    }

    #[test]
    fn scans_and_consent_survive_restart_and_fingerprints_bind_accounts() {
        let directory = tempfile::TempDir::new().unwrap();
        let root = directory.path().join("workspace");
        let app = directory.path().join("app");
        let engine = WorkspaceEngine::create_with_app_data(&root, "Replica", &app).unwrap();
        let credentials = Memory::default();
        let device = DeviceKeys::create(&credentials).unwrap();
        let key = device.signer().public_key();
        let config = WorkspaceSyncConfig {
            version: 1,
            workspace_id: engine.manifest().id,
            origin: "https://sync.example.com".into(),
            device_id: device.device_id().into(),
            enabled: true,
            trusted_devices: BTreeMap::from([(device.device_id().into(), key.clone())]),
            approved_recipients: BTreeMap::from([(device.device_id().into(), device.recipient())]),
            approved_accounts: BTreeMap::from([(device.device_id().into(), "account".into())]),
        };
        engine.sync_save_configuration(&config).unwrap();
        std::fs::write(root.join("ordinary.txt"), b"canonical bytes").unwrap();
        let mut secrets = engine
            .sync_restore_secrets(&device, &config.trusted_devices)
            .unwrap();
        engine
            .sync_capture_workspace(&device, &mut secrets)
            .unwrap();
        let first = engine.sync_outbox().unwrap();
        assert_eq!(first.len(), 1);
        assert_eq!(secrets.objects.len(), 1);
        engine.sync_pause(true).unwrap();
        drop(engine);
        let engine = WorkspaceEngine::open_with_app_data(&root, &app).unwrap();
        assert!(!engine.sync_status().unwrap().enabled);
        let mut restored = engine
            .sync_restore_secrets(&device, &config.trusted_devices)
            .unwrap();
        assert_eq!(restored.objects.len(), 1);
        engine.rebuild_index().unwrap();
        std::fs::rename(root.join("ordinary.txt"), root.join("moved.txt")).unwrap();
        engine
            .sync_capture_workspace(&device, &mut restored)
            .unwrap();
        let pending = engine.sync_outbox().unwrap();
        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].object_id, pending[1].object_id);
        let old = super::super::device_fingerprint(
            device.device_id(),
            "account",
            &key,
            &device.recipient(),
        )
        .unwrap();
        let relabeled = super::super::device_fingerprint(
            device.device_id(),
            "other_account",
            &key,
            &device.recipient(),
        )
        .unwrap();
        assert_ne!(old, relabeled);
        let mut invalid_config = config.clone();
        invalid_config.approved_accounts.clear();
        assert!(engine.sync_save_configuration(&invalid_config).is_err());
        let other = WorkspaceEngine::create_with_app_data(
            directory.path().join("other"),
            "Other",
            directory.path().join("other_app"),
        )
        .unwrap();
        assert!(other.sync_save_configuration(&config).is_err());
        assert!(
            WorkspaceEngine::create_sync_replica(&root, "Overwrite", "other_workspace", &app)
                .is_err()
        );
        assert_eq!(
            std::fs::read(root.join("moved.txt")).unwrap(),
            b"canonical bytes"
        );
    }

    #[test]
    fn only_deferred_activation_failures_continue_the_pass() {
        for code in [
            "sync_browser_activation_unsupported",
            "sync_device_approval_required",
        ] {
            assert!(
                activation_error_is_recoverable(&invalid(code)),
                "{code} must not abort the pass"
            );
        }
        for code in [
            "sync_key_required",
            "sync_writer_not_authorized",
            "sync_invalid_object_activation",
        ] {
            assert!(
                !activation_error_is_recoverable(&invalid(code)),
                "{code} must stay fatal"
            );
        }
    }
}
