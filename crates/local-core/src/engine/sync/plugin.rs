//! The `sync` plugin switch for native sync and collaboration.
//!
//! Whether sync runs for a workspace replica is decided by `enabled_plugins`
//! in `.noura/workspace.yaml`, the same canonical file that drives every other
//! plugin. `.noura` is never synchronized, so the switch is per replica.

use serde::{Deserialize, Serialize};

use super::super::*;

/// Plugin identifier that turns native sync and collaboration on for a replica.
pub const SYNC_PLUGIN_ID: &str = "sync";

/// Local marker recording that this replica's sync plugin state was set once.
/// Its presence stops the open-time migration from re-enabling a plugin the
/// user later turned off.
const SYNC_PLUGIN_MARKER_PATH: &str = ".noura/sync/plugin.json";
const SYNC_CONFIG_PATH: &str = ".noura/sync/config.json";

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncPluginMarker {
    version: u8,
}

#[cfg(test)]
thread_local! {
    static MIGRATION_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

impl WorkspaceEngine {
    /// Whether the in-memory manifest enables the `sync` plugin. External
    /// manifest edits update this snapshot when the engine adopts them.
    pub fn sync_plugin_enabled(&self) -> bool {
        self.manifest
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .enabled_plugins
            .iter()
            .any(|id| id == SYNC_PLUGIN_ID)
    }

    /// Fails with `sync_plugin_disabled` unless the `sync` plugin is enabled.
    pub fn require_sync_plugin(&self, operation: &str) -> Result<()> {
        if self.sync_plugin_enabled() {
            return Ok(());
        }
        Err(sync_plugin_disabled(operation))
    }

    /// Turn the `sync` plugin on for this replica, keeping every other
    /// manifest entry, including unknown plugin IDs.
    pub fn enable_sync_plugin(&self) -> Result<()> {
        let manifest = self.read_manifest()?;
        if manifest
            .enabled_plugins
            .iter()
            .any(|id| id == SYNC_PLUGIN_ID)
        {
            return Ok(());
        }
        let mut enabled_plugins = manifest.enabled_plugins;
        enabled_plugins.push(SYNC_PLUGIN_ID.into());
        self.manifest_update(ManifestUpdateInput {
            enabled_plugins: Some(enabled_plugins),
            expected_updated: Some(manifest.updated),
            ..Default::default()
        })?;
        Ok(())
    }

    /// Write the migration marker unless it exists. Takes the write lock.
    pub(crate) fn sync_plugin_record_marker(&self) -> Result<()> {
        let _lock = self.write_lock("sync_plugin_marker")?;
        self.sync_plugin_record_marker_locked()
    }

    /// Write the migration marker unless it exists. The caller holds the write lock.
    pub(crate) fn sync_plugin_record_marker_locked(&self) -> Result<()> {
        if self.sync_path(SYNC_PLUGIN_MARKER_PATH)?.is_file() {
            return Ok(());
        }
        self.sync_write(SYNC_PLUGIN_MARKER_PATH, &SyncPluginMarker { version: 1 })
    }

    /// Workspaces that set up sync before the plugin existed keep syncing:
    /// the first open adds `sync` to `enabled_plugins` and records a marker
    /// so a later "off" is never undone. A failure is logged and never
    /// blocks opening; the next open retries.
    pub(in crate::engine) fn migrate_sync_plugin(&self) {
        if let Err(error) = self.migrate_sync_plugin_inner() {
            tracing::warn!(
                code = %error.code,
                operation = %error.operation,
                "sync plugin migration failed; the workspace opens without it"
            );
        }
    }

    fn migrate_sync_plugin_inner(&self) -> Result<()> {
        if self.sync_path(SYNC_PLUGIN_MARKER_PATH)?.is_file()
            || !self.sync_path(SYNC_CONFIG_PATH)?.is_file()
        {
            return Ok(());
        }
        #[cfg(test)]
        if MIGRATION_FAULT.with(std::cell::Cell::get) {
            return Err(CoreError::new(
                "sync_plugin_migration_fault",
                ErrorCategory::Filesystem,
                "Injected migration failure",
                "sync_plugin_migration",
            ));
        }
        // A crash after the manifest write leaves `sync` enabled without a
        // marker. `enable_sync_plugin` is then a no-op and only the marker
        // is written.
        self.enable_sync_plugin()?;
        self.sync_plugin_record_marker()
    }

    /// Close every collaboration session and forget remote presence. Used
    /// when the sync plugin turns off so no editor keeps a live session.
    pub fn collaboration_close_all(&self) -> Result<()> {
        self.collaboration_sessions
            .lock()
            .map_err(|_| crate::sync::invalid("collaboration_unavailable"))?
            .clear();
        self.collaboration_clear_presence()
    }
}

pub(crate) fn sync_plugin_disabled(operation: &str) -> CoreError {
    let mut error = CoreError::validation(
        "sync_plugin_disabled",
        "Turn on the Sync plugin for this workspace to use sync",
        operation,
    );
    error.details = Some(serde_json::json!({ "pluginId": SYNC_PLUGIN_ID }));
    error
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    struct Workspace {
        _directory: TempDir,
        root: PathBuf,
        app: PathBuf,
    }

    impl Workspace {
        fn new() -> Self {
            let directory = TempDir::new().unwrap();
            let root = directory.path().join("workspace");
            let app = directory.path().join("app");
            WorkspaceEngine::create_with_app_data(&root, "Plugin", &app).unwrap();
            let root = root.canonicalize().unwrap();
            Self {
                _directory: directory,
                root,
                app,
            }
        }

        fn open(&self) -> WorkspaceEngine {
            WorkspaceEngine::open_with_app_data(&self.root, &self.app).unwrap()
        }

        /// A sync configuration written by a build that predates the plugin.
        fn write_legacy_config(&self, enabled: bool) {
            let engine = self.open();
            std::fs::create_dir_all(self.root.join(".noura/sync")).unwrap();
            let credentials = Memory::default();
            let device = crate::sync::DeviceKeys::create(&credentials).unwrap();
            let config = crate::sync::WorkspaceSyncConfig {
                version: 1,
                workspace_id: engine.manifest().id,
                origin: "https://sync.example.com".into(),
                device_id: device.device_id().into(),
                enabled,
                trusted_devices: BTreeMap::from([(
                    device.device_id().into(),
                    device.signer().public_key(),
                )]),
                approved_recipients: BTreeMap::from([(
                    device.device_id().into(),
                    device.recipient(),
                )]),
                approved_accounts: BTreeMap::from([(device.device_id().into(), "account".into())]),
            };
            std::fs::write(
                self.root.join(SYNC_CONFIG_PATH),
                serde_json::to_vec(&config).unwrap(),
            )
            .unwrap();
        }

        fn marker(&self) -> bool {
            self.root.join(SYNC_PLUGIN_MARKER_PATH).is_file()
        }
    }

    #[derive(Default)]
    struct Memory(std::cell::RefCell<BTreeMap<String, String>>);
    impl crate::sync::SyncCredentials for Memory {
        fn read(&self, key: &str) -> Result<zeroize::Zeroizing<String>> {
            self.0
                .borrow()
                .get(key)
                .cloned()
                .map(zeroize::Zeroizing::new)
                .ok_or_else(|| crate::sync::invalid("test_missing"))
        }
        fn write(&self, key: &str, value: &str) -> Result<()> {
            self.0.borrow_mut().insert(key.into(), value.into());
            Ok(())
        }
    }

    fn set_plugins(engine: &WorkspaceEngine, plugins: &[&str]) {
        engine
            .manifest_update(ManifestUpdateInput {
                enabled_plugins: Some(plugins.iter().map(|id| (*id).to_owned()).collect()),
                ..Default::default()
            })
            .unwrap();
    }

    #[test]
    fn new_workspace_does_not_enable_sync_plugin() {
        let workspace = Workspace::new();
        let engine = workspace.open();
        assert!(!engine.sync_plugin_enabled());
        assert!(
            !engine
                .read_manifest()
                .unwrap()
                .enabled_plugins
                .contains(&SYNC_PLUGIN_ID.to_owned())
        );
        assert!(!workspace.marker());
    }

    #[test]
    fn opening_a_synced_workspace_enables_sync_plugin_once() {
        for enabled in [true, false] {
            let workspace = Workspace::new();
            workspace.write_legacy_config(enabled);
            let engine = workspace.open();
            assert!(engine.sync_plugin_enabled(), "paused={}", !enabled);
            assert!(
                engine
                    .read_manifest()
                    .unwrap()
                    .enabled_plugins
                    .contains(&SYNC_PLUGIN_ID.to_owned())
            );
            assert!(workspace.marker());
            let updated = engine.read_manifest().unwrap().updated;
            drop(engine);
            let engine = workspace.open();
            assert!(engine.sync_plugin_enabled());
            assert_eq!(engine.read_manifest().unwrap().updated, updated);
        }
    }

    #[test]
    fn later_disable_survives_reopen() {
        let workspace = Workspace::new();
        workspace.write_legacy_config(true);
        let engine = workspace.open();
        assert!(engine.sync_plugin_enabled());
        set_plugins(&engine, &["notes", "tasks"]);
        assert!(!engine.sync_plugin_enabled());
        drop(engine);
        let engine = workspace.open();
        assert!(!engine.sync_plugin_enabled());
        assert_eq!(
            engine.read_manifest().unwrap().enabled_plugins,
            vec!["notes".to_owned(), "tasks".to_owned()]
        );
    }

    #[test]
    fn interrupted_migration_only_writes_marker() {
        let workspace = Workspace::new();
        workspace.write_legacy_config(true);
        {
            // A crash after the manifest write and before the marker.
            let engine = workspace.open();
            assert!(workspace.marker());
            std::fs::remove_file(workspace.root.join(SYNC_PLUGIN_MARKER_PATH)).unwrap();
            assert!(engine.sync_plugin_enabled());
        }
        let manifest_bytes = std::fs::read(workspace.root.join(WORKSPACE_MANIFEST_PATH)).unwrap();
        let engine = workspace.open();
        assert!(workspace.marker());
        assert!(engine.sync_plugin_enabled());
        assert_eq!(
            std::fs::read(workspace.root.join(WORKSPACE_MANIFEST_PATH)).unwrap(),
            manifest_bytes
        );
    }

    #[test]
    fn migration_preserves_unknown_plugin_ids() {
        let workspace = Workspace::new();
        {
            let engine = workspace.open();
            set_plugins(&engine, &["crm-future", "notes"]);
        }
        workspace.write_legacy_config(true);
        let engine = workspace.open();
        assert_eq!(
            engine.read_manifest().unwrap().enabled_plugins,
            vec![
                "crm-future".to_owned(),
                "notes".to_owned(),
                SYNC_PLUGIN_ID.to_owned()
            ]
        );
    }

    #[test]
    fn migration_failure_does_not_block_open() {
        let workspace = Workspace::new();
        workspace.write_legacy_config(true);
        MIGRATION_FAULT.with(|fault| fault.set(true));
        let engine = WorkspaceEngine::open_with_app_data(&workspace.root, &workspace.app);
        MIGRATION_FAULT.with(|fault| fault.set(false));
        let engine = engine.unwrap();
        assert!(!engine.sync_plugin_enabled());
        assert!(!workspace.marker());
        drop(engine);
        // The next open retries.
        let engine = workspace.open();
        assert!(engine.sync_plugin_enabled());
        assert!(workspace.marker());
    }

    #[test]
    fn saving_a_configuration_records_the_marker() {
        let workspace = Workspace::new();
        workspace.write_legacy_config(true);
        let engine = workspace.open();
        std::fs::remove_file(workspace.root.join(SYNC_PLUGIN_MARKER_PATH)).unwrap();
        let config = engine.sync_configuration().unwrap().unwrap();
        engine.sync_save_configuration(&config).unwrap();
        assert!(workspace.marker());
    }

    #[test]
    fn joined_replica_enables_sync_plugin() {
        let directory = TempDir::new().unwrap();
        let engine = WorkspaceEngine::create_sync_replica(
            directory.path().join("replica"),
            "Replica",
            "workspace_01j00000000000000000000000",
            directory.path().join("app"),
        )
        .unwrap();
        assert!(engine.sync_plugin_enabled());
        let manifest = engine.read_manifest().unwrap();
        assert!(
            manifest
                .enabled_plugins
                .contains(&SYNC_PLUGIN_ID.to_owned())
        );
        assert!(manifest.enabled_plugins.contains(&"notes".to_owned()));
    }

    #[test]
    fn require_sync_plugin_reports_the_plugin() {
        let workspace = Workspace::new();
        let engine = workspace.open();
        let error = engine.require_sync_plugin("sync_status").unwrap_err();
        assert_eq!(error.code, "sync_plugin_disabled");
        assert!(matches!(error.category, ErrorCategory::Validation));
        assert!(!error.retryable);
        assert_eq!(error.operation, "sync_status");
        assert_eq!(
            error.details,
            Some(serde_json::json!({ "pluginId": "sync" }))
        );
        engine.enable_sync_plugin().unwrap();
        engine.require_sync_plugin("sync_status").unwrap();
    }
}
