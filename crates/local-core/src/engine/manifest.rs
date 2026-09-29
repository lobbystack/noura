//! The workspace manifest at `.noura/workspace.yaml` and the plugin state
//! stored beside it.

use super::*;

/// Patch for selected `.noura/workspace.yaml` fields. Omitted fields keep their
/// current value; the manifest `updated` timestamp always refreshes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export, optional_fields = nullable)]
#[serde(rename_all = "camelCase", default)]
pub struct ManifestUpdateInput {
    pub name: Option<String>,
    pub enabled_plugins: Option<Vec<String>>,
    pub ignore: Option<Vec<String>>,
    /// Reject the update unless the on-disk manifest still has this
    /// `updated` value, preventing silent overwrite of external edits.
    pub expected_updated: Option<String>,
}

impl WorkspaceEngine {
    /// Owned snapshot of the current manifest. Returning a clone (instead
    /// of the live lock guard) means callers can hold the value across
    /// engine writes without deadlocking `manifest_update` or an external
    /// adopt on the same thread.
    pub fn manifest(&self) -> WorkspaceManifest {
        self.manifest
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub(super) fn current_ignore(&self) -> Vec<String> {
        self.manifest
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .ignore
            .clone()
    }

    pub(super) fn current_workspace_id(&self) -> String {
        self.manifest
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .id
            .clone()
    }

    /// Canonical `.noura/workspace.yaml` contents, freshly read from disk. The file
    /// wins over the in-memory snapshot, which only exists to avoid re-reading
    /// the manifest on every write.
    pub fn read_manifest(&self) -> Result<WorkspaceManifest> {
        parse_workspace_manifest(&self.read_manifest_bytes()?, "manifest_read")
    }

    pub fn manifest_update(&self, input: ManifestUpdateInput) -> Result<WorkspaceManifest> {
        let update_name = input.name.is_some();
        let update_enabled = input.enabled_plugins.is_some();
        let update_ignore = input.ignore.is_some();
        if !update_name && !update_enabled && !update_ignore {
            return self.read_manifest();
        }
        let name = match &input.name {
            Some(value) => {
                let value = value.trim();
                if value.is_empty() {
                    return Err(CoreError::validation(
                        "workspace_name_required",
                        "A workspace name is required",
                        "manifest_update",
                    ));
                }
                value.to_owned()
            }
            None => String::new(),
        };
        let mut enabled_plugins = match input.enabled_plugins {
            Some(values) => {
                let mut values = values;
                for id in &values {
                    validate_plugin_id(id, "manifest_update")?;
                }
                values.sort();
                values.dedup();
                values
            }
            None => Vec::new(),
        };
        let ignore = match input.ignore {
            Some(values) => {
                for pattern in &values {
                    if pattern.trim().is_empty() {
                        return Err(CoreError::validation(
                            "invalid_ignore_pattern",
                            "Workspace ignore patterns must not be empty",
                            "manifest_update",
                        ));
                    }
                }
                values
            }
            None => Vec::new(),
        };
        let _guard = self.write_lock("manifest_update")?;
        let current_bytes = self.read_manifest_bytes()?;
        let mut manifest = parse_workspace_manifest(&current_bytes, "manifest_update")?;
        if let Some(expected) = &input.expected_updated
            && manifest.updated != *expected
        {
            return Err(CoreError::new(
                "manifest_conflict",
                ErrorCategory::Conflict,
                ".noura/workspace.yaml changed on disk since it was last read",
                "manifest_update",
            ));
        }
        if update_name {
            manifest.name = name;
        }
        if update_enabled {
            manifest.enabled_plugins = std::mem::take(&mut enabled_plugins);
        }
        if update_ignore {
            manifest.ignore = ignore;
        }
        manifest.updated = now_rfc3339();
        validate_manifest(&manifest, "manifest_update")?;
        let bytes = serialize_workspace_manifest(&manifest, "manifest_update")?;
        atomic_write_checked(
            &self.root,
            Path::new(WORKSPACE_MANIFEST_PATH),
            bytes.as_bytes(),
            Some(&markdown::revision(&current_bytes)),
            "manifest_update",
        )?;
        // Journal like every other write site so the watcher poll does not
        // resurface this engine's own atomic manifest write as external.
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(
                WORKSPACE_MANIFEST_PATH.to_owned(),
                markdown::revision(bytes.as_bytes()),
            );
        }
        *self
            .manifest
            .write()
            .unwrap_or_else(|error| error.into_inner()) = manifest.clone();
        self.emit(
            "workspace:manifest-updated",
            EventSource::Application,
            serde_json::json!({
                "enabledPlugins": manifest.enabled_plugins,
                "name": manifest.name,
            }),
        );
        Ok(manifest)
    }

    fn read_manifest_bytes(&self) -> Result<Vec<u8>> {
        std::fs::read(self.root.join(WORKSPACE_MANIFEST_PATH))
            .map_err(|error| CoreError::io(error, "manifest_read", Some(WORKSPACE_MANIFEST_PATH)))
    }

    /// Read one plugin-local cache value. The state lives in the disposable
    /// index: durable plugin data belongs in workspace files.
    pub fn plugin_state_get(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Result<Option<serde_json::Value>> {
        validate_plugin_id(plugin_id, "plugin_state_get")?;
        validate_plugin_key(key, "plugin_state_get")?;
        let index = self
            .index
            .lock()
            .map_err(|_| lock_error("plugin_state_get"))?;
        index.plugin_state_get(plugin_id, key)
    }

    /// Write one plugin-local cache value. Index rebuilds discard it by design.
    pub fn plugin_state_set(
        &self,
        plugin_id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        validate_plugin_id(plugin_id, "plugin_state_set")?;
        validate_plugin_key(key, "plugin_state_set")?;
        let mut index = self
            .index
            .lock()
            .map_err(|_| lock_error("plugin_state_set"))?;
        index.plugin_state_set(plugin_id, key, &value)
    }

    /// Remove one plugin-local cache value. Returns whether one existed.
    pub fn plugin_state_delete(&self, plugin_id: &str, key: &str) -> Result<bool> {
        validate_plugin_id(plugin_id, "plugin_state_delete")?;
        validate_plugin_key(key, "plugin_state_delete")?;
        let mut index = self
            .index
            .lock()
            .map_err(|_| lock_error("plugin_state_delete"))?;
        index.plugin_state_delete(plugin_id, key)
    }

    /// Applies watcher events for `.noura/workspace.yaml`: journal-suppress the
    /// engine's own atomic write, then adopt any external change. The
    /// journal guard is released before the adopt so it cannot interleave
    /// with the write lock taken by `manifest_update`.
    pub(super) fn sync_external_manifest(&self, paths: &[PathBuf]) -> Result<()> {
        let touched = paths.iter().any(|path| {
            path.strip_prefix(&self.root)
                .is_ok_and(|relative| relative == Path::new(WORKSPACE_MANIFEST_PATH))
        });
        if !touched {
            return Ok(());
        }
        let journaled = self
            .self_writes
            .lock()
            .map_err(|_| lock_error("watcher_poll"))?
            .remove(WORKSPACE_MANIFEST_PATH);
        if let Some(expected) = journaled {
            let unchanged = std::fs::read(self.root.join(WORKSPACE_MANIFEST_PATH))
                .ok()
                .is_some_and(|bytes| markdown::revision(&bytes) == expected);
            if unchanged {
                return Ok(());
            }
        }
        self.apply_external_manifest()
    }

    /// The file wins: adopt the on-disk manifest into the engine snapshot
    /// and notify listeners with the same event an application mutation
    /// emits, so runtimes re-sync from the authoritative file. An
    /// unreadable or invalid file keeps the last known-good snapshot — a
    /// hand edit can be caught mid-save — until a later event resyncs.
    fn apply_external_manifest(&self) -> Result<()> {
        // Serialize with manifest_update: the file must not change under a
        // read-modify-write while the watcher is adopting it.
        let guard = self.write_lock("manifest_external_sync")?;
        let Ok(manifest) = self.read_manifest() else {
            return Ok(());
        };
        let (name, enabled_plugins) = (manifest.name.clone(), manifest.enabled_plugins.clone());
        let ignore_changed = {
            let mut current = self
                .manifest
                .write()
                .unwrap_or_else(|error| error.into_inner());
            if *current == manifest {
                return Ok(());
            }
            let ignore_changed = current.ignore != manifest.ignore;
            *current = manifest;
            ignore_changed
        };
        drop(guard);
        self.emit(
            "workspace:manifest-updated",
            EventSource::External,
            serde_json::json!({
                "enabledPlugins": enabled_plugins,
                "name": name,
            }),
        );
        if ignore_changed {
            // The visible scope of the workspace changed; realign the
            // index now instead of waiting for periodic reconciliation.
            self.reconcile()?;
        }
        Ok(())
    }
}

pub(super) fn validate_manifest(manifest: &WorkspaceManifest, operation: &str) -> Result<()> {
    workspace_format::validate_workspace_manifest(manifest)
        .map_err(|error| map_format_error(error, operation))?;
    compile_workspace_ignores(Path::new("."), &manifest.ignore)?;
    Ok(())
}

pub(super) fn serialize_workspace_manifest(
    manifest: &WorkspaceManifest,
    operation: &str,
) -> Result<String> {
    workspace_format::serialize_workspace_manifest(manifest)
        .map_err(|error| map_format_error(error, operation))
}

fn map_format_error(error: workspace_format::FormatError, operation: &str) -> CoreError {
    match error {
        workspace_format::FormatError::InvalidObjectId => CoreError::new(
            "invalid_object_id",
            ErrorCategory::Identity,
            "The stable ID does not match the object type",
            operation,
        ),
        workspace_format::FormatError::ObjectSerialization => CoreError::new(
            "serialize_failed",
            ErrorCategory::Parse,
            "Frontmatter could not be serialized",
            operation,
        ),
        workspace_format::FormatError::InvalidManifest => CoreError::new(
            "invalid_workspace_manifest",
            ErrorCategory::Parse,
            ".noura/workspace.yaml is invalid",
            operation,
        ),
        workspace_format::FormatError::InvalidWorkspaceId => CoreError::validation(
            "invalid_workspace_id",
            "The workspace ID must be a lowercase stable workspace ID",
            operation,
        ),
        workspace_format::FormatError::WorkspaceNameRequired => CoreError::validation(
            "workspace_name_required",
            "A workspace name is required",
            operation,
        ),
        workspace_format::FormatError::InvalidPluginId => CoreError::validation(
            "invalid_plugin_id",
            "Plugin identifiers use lowercase letters, digits, and hyphens",
            operation,
        ),
        workspace_format::FormatError::UnsupportedWorkspaceVersion => CoreError::validation(
            "unsupported_workspace_version",
            "This workspace format version is not supported",
            operation,
        ),
        workspace_format::FormatError::ManifestSerialization => CoreError::new(
            "manifest_serialize_failed",
            ErrorCategory::Parse,
            ".noura/workspace.yaml could not be serialized",
            operation,
        ),
        workspace_format::FormatError::TitleRequired => {
            CoreError::validation("title_required", "A title is required", operation)
        }
        workspace_format::FormatError::InvalidTimestamp => CoreError::validation(
            "invalid_timestamp",
            "A timestamp must be an RFC 3339 instant",
            operation,
        ),
        workspace_format::FormatError::InvalidTaskStatus => {
            CoreError::validation("invalid_field", "Unsupported status value", operation)
        }
        workspace_format::FormatError::InvalidTaskStatusType => {
            CoreError::validation("invalid_field", "status must be a string", operation)
        }
        workspace_format::FormatError::InvalidTaskPriority => {
            CoreError::validation("invalid_field", "Unsupported priority value", operation)
        }
        workspace_format::FormatError::InvalidTaskPriorityType => {
            CoreError::validation("invalid_field", "priority must be a string", operation)
        }
        workspace_format::FormatError::InvalidTaskDue => CoreError::validation(
            "invalid_date",
            "due must use YYYY-MM-DD or RFC 3339 with an explicit offset",
            operation,
        ),
        workspace_format::FormatError::InvalidTaskProject => CoreError::validation(
            "invalid_project_id",
            "Task project references use a stable project ID",
            operation,
        ),
        workspace_format::FormatError::InvalidProjectStatus => CoreError::validation(
            "invalid_field",
            "Unsupported project status value",
            operation,
        ),
        workspace_format::FormatError::InvalidProjectStatusType => CoreError::validation(
            "invalid_field",
            "project status must be a string",
            operation,
        ),
    }
}

/// Plugin identifiers match the plugin-sdk manifest pattern: a lowercase
/// letter, then lowercase letters, digits, or hyphens. Unknown plugin IDs are
/// tolerated so future ecosystem plugins do not break older builds.
fn validate_plugin_id(value: &str, operation: &str) -> Result<()> {
    if value.is_empty() || value.len() > 64 || !is_valid_plugin_id(value) {
        return Err(CoreError::validation(
            "invalid_plugin_id",
            "Plugin identifiers use lowercase letters, digits, and hyphens",
            operation,
        ));
    }
    Ok(())
}

fn is_valid_plugin_id(value: &str) -> bool {
    let mut chars = value.chars();
    let starts_lowercase = chars.next().is_some_and(|c| c.is_ascii_lowercase());
    starts_lowercase
        && value
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

fn validate_plugin_key(value: &str, operation: &str) -> Result<()> {
    if value.is_empty() || value.len() > 256 || value.chars().any(|c| c.is_control() || c == '\0') {
        return Err(CoreError::validation(
            "invalid_plugin_state_key",
            "Plugin state keys must be 1..=256 characters without control characters",
            operation,
        ));
    }
    Ok(())
}

pub(super) fn parse_workspace_manifest(bytes: &[u8], operation: &str) -> Result<WorkspaceManifest> {
    let mut manifest = workspace_format::decode_workspace_manifest(bytes)
        .map_err(|error| map_format_error(error, operation))?;
    validate_manifest(&manifest, operation)?;
    if manifest.format_version != 1 {
        return Err(map_format_error(
            workspace_format::FormatError::UnsupportedWorkspaceVersion,
            operation,
        ));
    }
    // `enabled_plugins` is deduplicated with insignificant order in the
    // format; readers canonicalize so consumers never observe a raw hand
    // edit's duplicates, matching the writer's normalization.
    workspace_format::normalize_workspace_manifest(&mut manifest);
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;
    use tempfile::tempdir;

    #[derive(Deserialize)]
    struct ConformanceFixture {
        manifest: Vec<FixtureCase>,
    }

    #[derive(Deserialize)]
    struct FixtureCase {
        valid: bool,
        value: serde_json::Value,
    }

    #[test]
    fn rust_manifest_validation_matches_shared_conformance_fixtures() {
        let fixtures: ConformanceFixture = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/conformance-v1.json"
        ))
        .unwrap();
        for fixture in fixtures.manifest {
            let accepted = serde_json::from_value::<WorkspaceManifest>(fixture.value)
                .ok()
                .is_some_and(|manifest| validate_manifest(&manifest, "manifest_validate").is_ok());
            assert_eq!(accepted, fixture.valid);
        }
    }

    #[test]
    fn external_manifest_change_adopts_the_file_and_emits_manifest_updated() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let manifest_path = engine.root().join(WORKSPACE_MANIFEST_PATH);
        let mut events = engine.subscribe();
        let external = std::fs::read_to_string(&manifest_path)
            .unwrap()
            .replace("- calendar\n", "");
        std::fs::write(&manifest_path, external).unwrap();

        let changes = engine
            .process_external_changes(vec![manifest_path])
            .unwrap();

        assert!(changes.is_empty());
        assert!(
            !engine
                .manifest()
                .enabled_plugins
                .iter()
                .any(|id| id == "calendar")
        );
        let received = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let updated = received
            .iter()
            .find(|event| event.event_type == "workspace:manifest-updated")
            .unwrap();
        assert_eq!(updated.source, EventSource::External);
        assert!(
            !updated.payload["enabledPlugins"]
                .as_array()
                .unwrap()
                .iter()
                .any(|id| id == "calendar")
        );
        assert!(
            !received
                .iter()
                .any(|event| event.event_type == "file:changed")
        );
    }

    #[test]
    fn manifest_update_write_is_not_reported_as_an_external_manifest_change() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let mut events = engine.subscribe();
        let before = engine.read_manifest().unwrap();
        engine
            .manifest_update(ManifestUpdateInput {
                enabled_plugins: Some(vec!["notes".into()]),
                ..Default::default()
            })
            .unwrap();

        let manifest_path = engine.root().join(WORKSPACE_MANIFEST_PATH);
        let changes = engine
            .process_external_changes(vec![manifest_path])
            .unwrap();

        assert!(changes.is_empty());
        assert_eq!(engine.manifest().enabled_plugins, vec!["notes".to_owned()]);
        let received = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let manifest_events = received
            .iter()
            .filter(|event| event.event_type == "workspace:manifest-updated")
            .collect::<Vec<_>>();
        assert_eq!(manifest_events.len(), 1);
        assert_eq!(manifest_events[0].source, EventSource::Application);
        assert_ne!(engine.manifest().updated, before.updated);
    }

    #[test]
    fn invalid_external_manifest_keeps_the_last_known_good_snapshot() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let manifest_path = engine.root().join(WORKSPACE_MANIFEST_PATH);
        let before = engine.manifest();
        let mut events = engine.subscribe();
        std::fs::write(&manifest_path, "not: [valid, manifest").unwrap();

        let changes = engine
            .process_external_changes(vec![manifest_path])
            .unwrap();

        assert!(changes.is_empty());
        assert_eq!(engine.manifest().enabled_plugins, before.enabled_plugins);
        let received = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        assert!(
            !received
                .iter()
                .any(|event| event.event_type == "workspace:manifest-updated")
        );
    }

    #[test]
    fn external_ignore_change_realigned_the_scope_immediately() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Scoped".into(),
                body: "needle-scoped".into(),
                relative_path: Some("scope/note.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        let manifest_path = engine.root().join(WORKSPACE_MANIFEST_PATH);
        let external = std::fs::read_to_string(&manifest_path)
            .unwrap()
            .replace("ignore: []", "ignore:\n- scope/**");
        std::fs::write(&manifest_path, external).unwrap();

        engine
            .process_external_changes(vec![manifest_path])
            .unwrap();

        assert!(
            !engine
                .search(&SearchInput {
                    query: "needle-scoped".into(),
                    ..Default::default()
                })
                .unwrap()
                .iter()
                .any(|result| result.relative_path == "scope/note.md")
        );
    }

    #[test]
    fn parsing_normalizes_duplicate_and_unsorted_enabled_plugins() {
        let manifest = "id: workspace_01j00000000000000000000000\nformat_version: 1\nname: Test\ncreated: 2026-08-27T12:00:00Z\nupdated: 2026-08-27T12:00:00Z\nenabled_plugins: [calendar, notes, calendar, tasks]\nignore: []\n";
        let parsed = parse_workspace_manifest(manifest.as_bytes(), "manifest_parse").unwrap();
        assert_eq!(
            parsed.enabled_plugins,
            vec![
                "calendar".to_owned(),
                "notes".to_owned(),
                "tasks".to_owned()
            ]
        );
    }
}
