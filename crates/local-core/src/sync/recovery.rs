//! User-held recovery kits. Secret bytes stay in native memory or the explicitly chosen backup file.
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use zeroize::{Zeroize, Zeroizing};

use super::*;
use crate::{Result, WorkspaceEngine};

const MAX_KIT: u64 = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryKit {
    version: u8,
    config: WorkspaceSyncConfig,
    identity: String,
    envelopes: Vec<KeyEnvelope>,
    signature: String,
}
impl Drop for RecoveryKit {
    fn drop(&mut self) {
        self.identity.zeroize();
    }
}
impl RecoveryKit {
    fn signing_bytes(&self) -> Result<Zeroizing<Vec<u8>>> {
        serde_json::to_vec(&(
            "noura.sync.recovery",
            self.version,
            &self.config,
            &self.identity,
            &self.envelopes,
        ))
        .map(Zeroizing::new)
        .map_err(|_| invalid("sync_invalid_recovery_file"))
    }
    fn reader(&self) -> Result<DeviceKeys> {
        self.config.validate(&self.config.workspace_id)?;
        if self.version != 1 || self.envelopes.len() > 1000 {
            return Err(invalid("sync_invalid_recovery_file"));
        }
        let public_key = self
            .config
            .trusted_devices
            .get(&self.config.device_id)
            .ok_or_else(|| invalid("sync_invalid_recovery_file"))?;
        let bytes: [u8; 32] = crypto::decode(public_key, 32, 32)?
            .try_into()
            .map_err(|_| invalid("sync_invalid_key"))?;
        VerifyingKey::from_bytes(&bytes)
            .map_err(|_| invalid("sync_invalid_key"))?
            .verify_strict(
                &self.signing_bytes()?,
                &Signature::from_slice(&crypto::decode(&self.signature, 64, 64)?)
                    .map_err(|_| invalid("sync_invalid_signature"))?,
            )
            .map_err(|_| invalid("sync_invalid_recovery_file"))?;
        let reader = DeviceKeys::recovery_reader(&self.config.device_id, &self.identity)?;
        if self.config.approved_recipients.get(&self.config.device_id) != Some(&reader.recipient())
        {
            return Err(invalid("sync_invalid_recovery_file"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for envelope in &self.envelopes {
            if envelope.workspace_id != self.config.workspace_id
                || envelope.device_id != self.config.device_id
                || !seen.insert((&envelope.object_id, envelope.epoch))
            {
                return Err(invalid("sync_invalid_recovery_file"));
            }
            let trusted = self
                .config
                .trusted_devices
                .get(&envelope.signing_device)
                .ok_or_else(|| invalid("sync_untrusted_device"))?;
            reader.unwrap_key(envelope, trusted)?;
        }
        Ok(reader)
    }
    fn load(path: &Path) -> Result<Self> {
        let meta =
            std::fs::symlink_metadata(path).map_err(|_| invalid("sync_recovery_read_failed"))?;
        if !meta.is_file() || meta.len() > MAX_KIT {
            return Err(invalid("sync_invalid_recovery_file"));
        }
        let mut bytes = Zeroizing::new(Vec::new());
        File::open(path)
            .and_then(|file| file.take(MAX_KIT + 1).read_to_end(&mut bytes))
            .map_err(|_| invalid("sync_recovery_read_failed"))?;
        if bytes.len() as u64 > MAX_KIT {
            return Err(invalid("sync_invalid_recovery_file"));
        }
        serde_json::from_slice(&bytes).map_err(|_| invalid("sync_invalid_recovery_file"))
    }
}

impl WorkspaceSyncCoordinator {
    /// Export the current workspace's trusted identities and recipient-encrypted keys with its recovery identity.
    pub fn export_recovery_kit(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        destination: &Path,
    ) -> Result<()> {
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        let envelopes = secrets
            .objects
            .keys()
            .map(|(object, epoch)| {
                engine
                    .sync_key_envelope(object, *epoch, device.device_id())?
                    .ok_or_else(|| invalid("sync_key_required"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut kit = RecoveryKit {
            version: 1,
            config,
            identity: device.recovery_secret().to_string(),
            envelopes,
            signature: String::new(),
        };
        kit.signature = device.signer().sign_bytes(&kit.signing_bytes()?);
        kit.reader()?;
        let bytes = Zeroizing::new(
            serde_json::to_vec(&kit).map_err(|_| invalid("sync_invalid_recovery_file"))?,
        );
        if bytes.len() as u64 > MAX_KIT {
            return Err(invalid("sync_recovery_too_large"));
        }
        write_backup(destination, engine.root(), &bytes)
    }

    /// Import into the selected workspace under a newly signed-in device. No old signing seed or session is restored.
    /// The workspace remains paused for the user to verify the new device and handle any required epoch rotation.
    pub async fn import_recovery_kit(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        source: &Path,
    ) -> Result<()> {
        let kit = RecoveryKit::load(source)?;
        let reader = kit.reader()?;
        if kit.config.workspace_id != engine.manifest().id
            || kit.config.origin != connection.origin
            || kit.config.approved_accounts.get(&kit.config.device_id)
                != Some(&connection.account_id)
        {
            return Err(invalid("sync_recovery_context_mismatch"));
        }
        let device = DeviceKeys::load(store, &connection.device_id)?;
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let config = prepare_recovery_configuration(config, &kit.config, connection, &device)?;
        engine.sync_save_configuration(&config)?;
        let mut recovered = engine.sync_restore_secrets(&reader, &config.trusted_devices)?;
        for envelope in &kit.envelopes {
            let signer = config
                .trusted_devices
                .get(&envelope.signing_device)
                .ok_or_else(|| invalid("sync_untrusted_device"))?;
            let key = reader.unwrap_key(envelope, signer)?;
            if let Some(existing) = recovered
                .objects
                .get(&(envelope.object_id.clone(), envelope.epoch))
            {
                if existing.secret() != key.secret() {
                    return Err(invalid("sync_object_key_changed"));
                }
            } else {
                engine.sync_store_key(envelope, &reader, signer)?;
            }
            recovered
                .objects
                .insert((envelope.object_id.clone(), envelope.epoch), key);
        }
        let transport = connection.transport(store)?;
        // Same-account recovery can retrieve later envelopes addressed to the exported identity.
        // Signer pins still come from the kit/current local approval, never from the server response.
        transport
            .receive_keys(engine, &reader, &mut recovered)
            .await?;
        let own = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        for ((object, epoch), key) in &recovered.objects {
            if let Some(existing) = own.objects.get(&(object.clone(), *epoch)) {
                if existing.secret() != key.secret() {
                    return Err(invalid("sync_object_key_changed"));
                }
                continue;
            }
            let envelope = device.wrap_key(
                &config.workspace_id,
                object,
                *epoch,
                device.device_id(),
                &device.recipient(),
                key,
            )?;
            engine.sync_store_key(&envelope, &device, &device.signer().public_key())?;
        }
        let mut secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        let mut access = transport.access_state(&config.workspace_id).await?;
        if access.revision() != engine.sync_access_revision()? {
            transport.refresh_access_policies(engine, &config).await?;
            access = transport.access_state(&config.workspace_id).await?;
        }
        access.authorize_writers(engine, &device, &config, &mut secrets)?;
        loop {
            let result = transport
                .synchronize_readonly(engine, &secrets, false)
                .await?;
            if !result.has_more {
                break;
            }
        }
        Ok(())
    }
}

fn prepare_recovery_configuration(
    mut current: WorkspaceSyncConfig,
    backup: &WorkspaceSyncConfig,
    connection: &DeviceConnection,
    device: &DeviceKeys,
) -> Result<WorkspaceSyncConfig> {
    current.validate(&backup.workspace_id)?;
    if current.workspace_id != backup.workspace_id
        || current.origin != backup.origin
        || current.origin != connection.origin
        || backup.approved_accounts.get(&backup.device_id) != Some(&connection.account_id)
    {
        return Err(invalid("sync_recovery_context_mismatch"));
    }

    if current.device_id == connection.device_id {
        WorkspaceSyncCoordinator::check_connection(&current, connection, device)?;
    } else if current.device_id != backup.device_id {
        return Err(invalid("sync_recovery_context_mismatch"));
    }

    merge_pins(&mut current.trusted_devices, &backup.trusted_devices)?;
    merge_pins(
        &mut current.approved_recipients,
        &backup.approved_recipients,
    )?;
    merge_pins(&mut current.approved_accounts, &backup.approved_accounts)?;
    merge_pins(
        &mut current.trusted_devices,
        &BTreeMap::from([(device.device_id().to_owned(), device.signer().public_key())]),
    )?;
    merge_pins(
        &mut current.approved_recipients,
        &BTreeMap::from([(device.device_id().to_owned(), device.recipient())]),
    )?;
    merge_pins(
        &mut current.approved_accounts,
        &BTreeMap::from([(device.device_id().to_owned(), connection.account_id.clone())]),
    )?;
    current.device_id = connection.device_id.clone();
    current.enabled = false;
    current.validate(&backup.workspace_id)?;
    Ok(current)
}

fn merge_pins(
    target: &mut BTreeMap<String, String>,
    backup: &BTreeMap<String, String>,
) -> Result<()> {
    for (id, value) in backup {
        if target.get(id).is_some_and(|existing| existing != value) {
            return Err(invalid("sync_device_changed"));
        }
        target.insert(id.clone(), value.clone());
    }
    Ok(())
}
fn write_backup(destination: &Path, workspace: &Path, bytes: &[u8]) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("sync_invalid_recovery_path"))?
        .canonicalize()
        .map_err(|_| invalid("sync_invalid_recovery_path"))?;
    let root = workspace
        .canonicalize()
        .map_err(|_| invalid("sync_invalid_recovery_path"))?;
    if parent.starts_with(&root)
        || parent
            .ancestors()
            .any(|path| path.join("workspace.yaml").exists())
    {
        return Err(invalid("sync_recovery_in_workspace"));
    }
    let destination = parent.join(
        destination
            .file_name()
            .ok_or_else(|| invalid("sync_invalid_recovery_path"))?,
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&destination)
        .map_err(|_| invalid("sync_recovery_write_failed"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| invalid("sync_recovery_write_failed"))?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|_| invalid("sync_recovery_write_failed"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, String>>);
    impl SyncCredentials for Memory {
        fn read(&self, name: &str) -> Result<Zeroizing<String>> {
            self.0
                .borrow()
                .get(name)
                .cloned()
                .map(Zeroizing::new)
                .ok_or_else(|| invalid("test_missing"))
        }
        fn write(&self, name: &str, value: &str) -> Result<()> {
            self.0.borrow_mut().insert(name.into(), value.into());
            Ok(())
        }
    }
    #[test]
    fn recovery_kit_binds_context_and_keys_and_never_overwrites_or_enters_a_workspace() {
        let directory = tempfile::TempDir::new().unwrap();
        let engine = WorkspaceEngine::create_with_app_data(
            directory.path().join("workspace"),
            "Recovery",
            directory.path().join("app"),
        )
        .unwrap();
        let credentials = Memory::default();
        let device = DeviceKeys::create(&credentials).unwrap();
        let connection = DeviceConnection {
            origin: "http://127.0.0.1:1900".into(),
            device_id: device.device_id().into(),
            account_id: "account".into(),
            token_reference: "test".into(),
        };
        let config = WorkspaceSyncConfig {
            version: 1,
            workspace_id: engine.manifest().id,
            origin: connection.origin.clone(),
            device_id: device.device_id().into(),
            enabled: false,
            trusted_devices: BTreeMap::from([(
                device.device_id().into(),
                device.signer().public_key(),
            )]),
            approved_recipients: BTreeMap::from([(device.device_id().into(), device.recipient())]),
            approved_accounts: BTreeMap::from([(device.device_id().into(), "account".into())]),
        };
        engine.sync_save_configuration(&config).unwrap();
        std::fs::write(engine.root().join("file.txt"), b"private recovery bytes").unwrap();
        engine
            .sync_capture_owned_file("file.txt", &device, 1)
            .unwrap();
        let path = directory.path().join("recovery.json");
        assert!(
            WorkspaceSyncCoordinator::export_recovery_kit(
                &engine,
                &connection,
                &credentials,
                &engine.root().join("leak.json")
            )
            .is_err()
        );
        WorkspaceSyncCoordinator::export_recovery_kit(&engine, &connection, &credentials, &path)
            .unwrap();
        assert!(
            WorkspaceSyncCoordinator::export_recovery_kit(
                &engine,
                &connection,
                &credentials,
                &path
            )
            .is_err()
        );
        let kit = RecoveryKit::load(&path).unwrap();
        let reader = kit.reader().unwrap();
        assert_eq!(reader.recipient(), device.recipient());
        assert_eq!(kit.envelopes.len(), 1);

        let replacement = DeviceKeys::create(&credentials).unwrap();
        let replacement_connection = DeviceConnection {
            origin: connection.origin.clone(),
            device_id: replacement.device_id().into(),
            account_id: connection.account_id.clone(),
            token_reference: "replacement-token".into(),
        };
        let recovered = prepare_recovery_configuration(
            config.clone(),
            &kit.config,
            &replacement_connection,
            &replacement,
        )
        .unwrap();
        assert_eq!(recovered.device_id, replacement.device_id());
        assert!(!recovered.enabled);
        assert_eq!(
            recovered.trusted_devices.get(replacement.device_id()),
            Some(&replacement.signer().public_key())
        );
        assert_eq!(
            recovered.approved_recipients.get(replacement.device_id()),
            Some(&replacement.recipient())
        );
        assert_eq!(
            recovered.approved_accounts.get(replacement.device_id()),
            Some(&connection.account_id)
        );
        assert!(recovered.trusted_devices.contains_key(device.device_id()));
        let mut wrong_account = replacement_connection.clone();
        wrong_account.account_id = "other-account".into();
        assert!(
            prepare_recovery_configuration(
                config.clone(),
                &kit.config,
                &wrong_account,
                &replacement
            )
            .is_err()
        );
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("private recovery bytes")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        for field in ["identity", "signature"] {
            let mut corrupted = serde_json::to_value(&kit).unwrap();
            corrupted[field] = serde_json::json!("changed");
            let corrupted: RecoveryKit = serde_json::from_value(corrupted).unwrap();
            assert!(corrupted.reader().is_err());
        }
        let mut corrupted = serde_json::to_value(&kit).unwrap();
        corrupted["config"]["workspaceId"] = serde_json::json!("other");
        assert!(
            serde_json::from_value::<RecoveryKit>(corrupted)
                .unwrap()
                .reader()
                .is_err()
        );
        #[cfg(unix)]
        {
            let link = directory.path().join("link.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(RecoveryKit::load(&link).is_err());
        }
    }
}
