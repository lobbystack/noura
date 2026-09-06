use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{
    DeviceConnection, DeviceKeys, SyncCredentials, SyncPass, WorkspaceRole, identifier, invalid,
};
use crate::{Result, WorkspaceEngine};

/// Local consent and pinned sender keys. This file is never part of a synced payload.
#[derive(Clone, Serialize, Deserialize)]
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
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        if !config.enabled {
            return Err(invalid("sync_paused"));
        }
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let transport = connection.transport(store)?;
        let mut access = transport.access_state(&engine.manifest().id).await?;
        if access.revision() != engine.sync_access_revision()? {
            transport.refresh_access_policies(engine, &config).await?;
            access = transport.access_state(&engine.manifest().id).await?;
        }
        let mut secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        // Fetch rotations before capturing new edits. Never select an epoch from an unverified key.
        transport
            .receive_keys(engine, &device, &mut secrets)
            .await?;
        let role = access.authorize_writers(engine, &device, &config, &mut secrets)?;
        if role == WorkspaceRole::Viewer {
            return transport.synchronize_readonly(engine, &secrets, wait).await;
        }
        engine.sync_capture_workspace(&device, &mut secrets)?;
        if config.approved_recipients.len() > 1 {
            transport.prepare_objects(engine).await?;
            transport
                .share_approved_keys(engine, &device, &config, &secrets)
                .await?;
        }
        if wait {
            transport.synchronize_wait(engine, &secrets).await
        } else {
            transport.synchronize(engine, &secrets).await
        }
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
}
