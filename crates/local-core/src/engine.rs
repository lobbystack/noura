use std::{
    collections::{BTreeMap, HashMap},
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::UNIX_EPOCH,
};

use fs2::FileExt;
use ignore::{WalkBuilder, gitignore::GitignoreBuilder};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{
    AppendChatContextSummaryInput, AppendChatToolResultInput, AppendChatUserMessageInput,
    BeginChatAssistantInput, BeginChatToolCallInput, ChangeChatRetentionInput, Chat, ChatMessage,
    ChatMessageKind, ChatMessageStatus, ChatRead, ChatRetention, CoreError, CoreEvent, CoreWarning,
    CreateChatInput, ErrorCategory, FinishChatAssistantInput, FinishChatToolCallInput, IndexStatus,
    IndexStore, MutationResult, ParseStatus, ParsedMarkdown, RenameChatInput, Result, SearchInput,
    SearchResult, UnmanagedFile, WatchCoordinator, WorkspaceEntry, WorkspaceEntryKind,
    WorkspaceManifest, WorkspaceObject, WorkspacePhase, WorkspaceState, index::CalendarEntry,
    markdown, new_object_id, now_rfc3339, parse_chat, parse_chat_message, path::resolve_for_write,
    serialize_chat, serialize_chat_message, valid_object_id, valid_object_type, validate_retention,
};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CreateObjectInput {
    #[serde(rename = "type")]
    pub object_type: String,
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub relative_path: Option<String>,
    #[serde(default)]
    #[ts(type = "Record<string, unknown>")]
    pub properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ObjectPatch {
    pub title: Option<String>,
    pub body: Option<String>,
    #[serde(default)]
    #[ts(type = "Record<string, unknown>")]
    pub properties: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    pub remove_properties: Vec<String>,
    pub expected_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct DraftReconcileInput {
    pub id: String,
    pub base_revision: String,
    pub base_body: String,
    pub local_body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum DraftReconcileResult {
    Unchanged {
        current: WorkspaceObject,
        body: String,
    },
    Merged {
        current: WorkspaceObject,
        body: String,
    },
    Conflict {
        current: WorkspaceObject,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictResolution {
    UseExternal,
    ReplaceExternal,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ResolveConflictInput {
    pub id: String,
    pub current_revision: String,
    pub local_body: String,
    pub resolution: ConflictResolution,
}

/// Draft state captured by a client before a managed object was modified.
/// Title, body, and properties are reconciled independently during merges;
/// stable identity and canonical metadata always come from the file.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDraftInput {
    pub id: String,
    pub base_revision: String,
    pub base_title: String,
    pub base_body: String,
    #[ts(type = "Record<string, unknown>")]
    pub base_properties: BTreeMap<String, serde_json::Value>,
    pub local_title: String,
    pub local_body: String,
    #[ts(type = "Record<string, unknown>")]
    pub local_properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ManagedDraftResult {
    Unchanged {
        current: WorkspaceObject,
    },
    Merged {
        current: WorkspaceObject,
        title: String,
        body: String,
        #[ts(type = "Record<string, unknown>")]
        properties: BTreeMap<String, serde_json::Value>,
    },
    Conflict {
        current: WorkspaceObject,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ManagedConflictResolution {
    UseExternal,
    ReplaceExternal,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ManagedConflictResolveInput {
    pub id: String,
    pub current_revision: String,
    pub relative_path: String,
    pub created: Option<String>,
    pub local_title: String,
    pub local_body: String,
    #[ts(type = "Record<string, unknown>")]
    pub local_properties: BTreeMap<String, serde_json::Value>,
    pub resolution: ManagedConflictResolution,
}

/// Complete current contents of one Markdown file addressed by relative path.
/// Raw files expose their full bytes as UTF-8 text with CRLF normalized to LF;
/// uses-crlf and has-bom flags are preserved for faithful writes.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RawMarkdownRead {
    pub relative_path: String,
    pub body: String,
    pub revision: String,
    pub uses_crlf: bool,
    pub has_bom: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RawReconcileInput {
    pub relative_path: String,
    pub base_revision: String,
    pub base_body: String,
    pub local_body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum RawReconcileResult {
    Unchanged { current: RawMarkdownRead },
    Merged { current: RawMarkdownRead },
    Conflict { current: RawMarkdownRead },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RawSaveInput {
    pub relative_path: String,
    pub base_revision: String,
    pub base_body: String,
    pub local_body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum RawSaveResult {
    Saved {
        current: RawMarkdownRead,
        #[ts(rename = "managedObject")]
        managed_object: Option<WorkspaceObject>,
    },
    Merged {
        current: RawMarkdownRead,
        #[ts(rename = "managedObject")]
        managed_object: Option<WorkspaceObject>,
    },
    Conflict {
        current: RawMarkdownRead,
        #[ts(rename = "managedObject")]
        managed_object: Option<WorkspaceObject>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RawConflictResolveInput {
    pub relative_path: String,
    pub current_revision: String,
    pub local_body: String,
    pub resolution: ConflictResolution,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RawConflictResolveResult {
    pub current: RawMarkdownRead,
    #[ts(rename = "managedObject")]
    pub managed_object: Option<WorkspaceObject>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum MarkdownLinkTarget {
    Managed {
        object: WorkspaceObject,
    },
    Markdown {
        document: RawMarkdownRead,
    },
    Asset {
        #[ts(rename = "relativePath")]
        relative_path: String,
    },
    Unresolved,
}

type MarkdownIndexEntry = (String, Vec<u8>, i64, ParsedMarkdown);

const CHAT_MUTATION_DIR: &str = ".noura/chat-mutations";

#[derive(Debug, Serialize, Deserialize)]
struct ChatMutationIntent {
    version: u8,
    chat: ChatMutationTarget,
    message: ChatMutationTarget,
}

#[derive(Debug, Serialize, Deserialize)]
struct ChatMutationTarget {
    relative_path: String,
    expected_revision: Option<String>,
    bytes: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChatMutationWriteTarget {
    Chat,
    Message,
}

#[derive(Debug, Clone, Copy)]
struct ChatMutationFault {
    target: ChatMutationWriteTarget,
    remaining: usize,
}

/// Patch for selected `workspace.yaml` fields. Omitted fields keep their
/// current value; the manifest `updated` timestamp always refreshes.
#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", default)]
pub struct ManifestUpdateInput {
    pub name: Option<String>,
    pub enabled_plugins: Option<Vec<String>>,
    pub ignore: Option<Vec<String>>,
    /// Reject the update unless the on-disk manifest still has this
    /// `updated` value, preventing silent overwrite of external edits.
    pub expected_updated: Option<String>,
}

struct WorkspaceScan {
    changed: Vec<MarkdownIndexEntry>,
    seen: std::collections::HashSet<String>,
}

mod sync;

pub struct WorkspaceEngine {
    root: PathBuf,
    manifest: std::sync::RwLock<WorkspaceManifest>,
    index_path: PathBuf,
    index: Arc<Mutex<IndexStore>>,
    lock_path: PathBuf,
    event_sender: tokio::sync::broadcast::Sender<CoreEvent>,
    watcher: WatchCoordinator,
    self_writes: Mutex<HashMap<String, String>>,
    chat_mutation_fault: Mutex<Option<ChatMutationFault>>,
}

impl WorkspaceEngine {
    pub fn create(root: impl AsRef<Path>, name: &str) -> Result<Self> {
        let app_data = directories::ProjectDirs::from("org", "noura", "Noura")
            .ok_or_else(|| {
                CoreError::new(
                    "app_data_unavailable",
                    ErrorCategory::Filesystem,
                    "The operating system application-data directory is unavailable",
                    "workspace_create",
                )
            })?
            .data_local_dir()
            .to_owned();
        Self::create_with_app_data(root, name, app_data)
    }

    pub fn create_with_app_data(
        root: impl AsRef<Path>,
        name: &str,
        app_data: impl AsRef<Path>,
    ) -> Result<Self> {
        Self::create_with_identity(root, name, app_data, None)
    }

    fn create_with_identity(
        root: impl AsRef<Path>,
        name: &str,
        app_data: impl AsRef<Path>,
        workspace_id: Option<&str>,
    ) -> Result<Self> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)
            .map_err(|error| CoreError::io(error, "workspace_create", root.to_str()))?;
        let canonical = root
            .canonicalize()
            .map_err(|error| CoreError::io(error, "workspace_create", root.to_str()))?;
        if canonical.join("workspace.yaml").exists() {
            return Err(CoreError::new(
                "workspace_exists",
                ErrorCategory::Conflict,
                "A workspace already exists at this location",
                "workspace_create",
            ));
        }
        if name.trim().is_empty() {
            return Err(CoreError::validation(
                "workspace_name_required",
                "A workspace name is required",
                "workspace_create",
            ));
        }
        let now = now_rfc3339();
        let manifest = WorkspaceManifest {
            id: workspace_id.map(str::to_owned).unwrap_or_else(|| {
                format!("workspace_{}", ulid::Ulid::new().to_string().to_lowercase())
            }),
            format_version: 1,
            name: name.trim().to_owned(),
            created: now.clone(),
            updated: now,
            enabled_plugins: vec![
                "folders".into(),
                "notes".into(),
                "tasks".into(),
                "calendar".into(),
                "projects".into(),
            ],
            ignore: Vec::new(),
        };
        let bytes = serde_yaml_ng::to_string(&manifest).map_err(|_| {
            CoreError::new(
                "manifest_serialize_failed",
                ErrorCategory::Parse,
                "workspace.yaml could not be serialized",
                "workspace_create",
            )
        })?;
        atomic_write(
            &canonical,
            Path::new("workspace.yaml"),
            bytes.as_bytes(),
            "workspace_create",
        )?;
        let trash = resolve_for_write(&canonical, ".noura/trash", "workspace_create")?;
        std::fs::create_dir_all(&trash)
            .map_err(|error| CoreError::io(error, "workspace_create", canonical.to_str()))?;
        Self::open_with_app_data(canonical, app_data)
    }

    pub fn open(root: impl AsRef<Path>) -> Result<Self> {
        let app_data = directories::ProjectDirs::from("org", "noura", "Noura")
            .ok_or_else(|| {
                CoreError::new(
                    "app_data_unavailable",
                    ErrorCategory::Filesystem,
                    "The operating system application-data directory is unavailable",
                    "workspace_open",
                )
            })?
            .data_local_dir()
            .to_owned();
        Self::open_with_app_data(root, app_data)
    }

    pub fn open_with_app_data(root: impl AsRef<Path>, app_data: impl AsRef<Path>) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|error| CoreError::io(error, "workspace_open", root.as_ref().to_str()))?;
        let manifest_bytes = std::fs::read(root.join("workspace.yaml"))
            .map_err(|error| CoreError::io(error, "workspace_open", Some("workspace.yaml")))?;
        let manifest = parse_workspace_manifest(&manifest_bytes, "workspace_open")?;
        let local_dir = app_data.as_ref().join("workspaces").join(&manifest.id);
        std::fs::create_dir_all(&local_dir)
            .map_err(|error| CoreError::io(error, "workspace_open", local_dir.to_str()))?;
        let index_path = local_dir.join("index.sqlite");
        let index = IndexStore::open(&index_path)?;
        let (event_sender, _) = tokio::sync::broadcast::channel(256);
        let watcher = WatchCoordinator::new(&root)?;
        let engine = Self {
            root,
            manifest: std::sync::RwLock::new(manifest),
            index_path,
            index: Arc::new(Mutex::new(index)),
            lock_path: local_dir.join("workspace.lock"),
            event_sender,
            watcher,
            self_writes: Mutex::new(HashMap::new()),
            chat_mutation_fault: Mutex::new(None),
        };
        engine.recover_pending_chat_mutations()?;
        engine.reconcile()?;
        engine.emit("workspace:ready", "reconciliation", serde_json::json!({}));
        Ok(engine)
    }

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
    fn current_ignore(&self) -> Vec<String> {
        self.manifest
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .ignore
            .clone()
    }
    fn current_workspace_id(&self) -> String {
        self.manifest
            .read()
            .unwrap_or_else(|error| error.into_inner())
            .id
            .clone()
    }
    pub fn root(&self) -> &Path {
        &self.root
    }
    /// Resolve a workspace-managed relative path for a native desktop action.
    ///
    /// Uses the same traversal/symlink validation as mutations; only the
    /// operation label differs. Returns the canonical absolute destination.
    pub fn resolve_managed_path(&self, relative: &str, operation: &str) -> Result<PathBuf> {
        resolve_for_write(&self.root, relative, operation)
    }
    pub fn index_path(&self) -> &Path {
        &self.index_path
    }
    pub fn subscribe(&self) -> tokio::sync::broadcast::Receiver<CoreEvent> {
        self.event_sender.subscribe()
    }
    pub fn state(&self) -> WorkspaceState {
        let (indexed_files, diagnostics) =
            self.index.lock().ok().map_or((0, Vec::new()), |index| {
                (
                    index.file_count().unwrap_or(0),
                    index.diagnostics().unwrap_or_default(),
                )
            });
        WorkspaceState {
            phase: WorkspacePhase::Ready,
            workspace_id: Some(self.current_workspace_id()),
            root_path: self.root.to_str().map(str::to_owned),
            indexed_files,
            diagnostics,
        }
    }

    /// Canonical `workspace.yaml` contents, freshly read from disk. The file
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
                "workspace.yaml changed on disk since it was last read",
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
        let bytes = serde_yaml_ng::to_string(&manifest).map_err(|_| {
            CoreError::new(
                "manifest_serialize_failed",
                ErrorCategory::Parse,
                "workspace.yaml could not be serialized",
                "manifest_update",
            )
        })?;
        atomic_write_checked(
            &self.root,
            Path::new("workspace.yaml"),
            bytes.as_bytes(),
            Some(&markdown::revision(&current_bytes)),
            "manifest_update",
        )?;
        // Journal like every other write site so the watcher poll does not
        // resurface this engine's own atomic manifest write as external.
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(
                "workspace.yaml".to_owned(),
                markdown::revision(bytes.as_bytes()),
            );
        }
        *self
            .manifest
            .write()
            .unwrap_or_else(|error| error.into_inner()) = manifest.clone();
        self.emit(
            "workspace:manifest-updated",
            "application",
            serde_json::json!({
                "enabledPlugins": manifest.enabled_plugins,
                "name": manifest.name,
            }),
        );
        Ok(manifest)
    }

    fn read_manifest_bytes(&self) -> Result<Vec<u8>> {
        std::fs::read(self.root.join("workspace.yaml"))
            .map_err(|error| CoreError::io(error, "manifest_read", Some("workspace.yaml")))
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

    pub fn rebuild_index(&self) -> Result<()> {
        self.emit("workspace:rebuilding", "application", serde_json::json!({}));
        let next = self.index_path.with_extension("sqlite.next");
        if next.exists() {
            std::fs::remove_file(&next)
                .map_err(|error| CoreError::io(error, "index_rebuild", next.to_str()))?;
        }
        let mut replacement = IndexStore::open(&next)?;
        scan_into(&self.root, &self.current_ignore(), &mut replacement)?;
        drop(replacement);
        let previous = self.index_path.with_extension("sqlite.previous");
        if previous.exists() {
            std::fs::remove_file(&previous)
                .map_err(|error| CoreError::io(error, "index_rebuild", previous.to_str()))?;
        }
        // Close the active connection before replacing the database. Windows does not allow an
        // open SQLite file to be renamed, while Unix happens to tolerate it.
        let mut index = self.index.lock().map_err(|_| lock_error("index_rebuild"))?;
        let active = std::mem::replace(&mut *index, IndexStore::in_memory()?);
        drop(active);
        if self.index_path.exists()
            && let Err(error) = std::fs::rename(&self.index_path, &previous)
        {
            *index = IndexStore::open(&self.index_path)?;
            return Err(CoreError::io(
                error,
                "index_rebuild",
                self.index_path.to_str(),
            ));
        }
        if let Err(error) = std::fs::rename(&next, &self.index_path) {
            if previous.exists() {
                let _ = std::fs::rename(&previous, &self.index_path);
            }
            *index = IndexStore::open(&self.index_path)?;
            return Err(CoreError::io(error, "index_rebuild", next.to_str()));
        }
        *index = IndexStore::open(&self.index_path)?;
        drop(index);
        let _ = std::fs::remove_file(previous);
        self.emit(
            "workspace:ready",
            "application",
            serde_json::json!({ "rebuilt": true }),
        );
        Ok(())
    }

    pub fn reconcile(&self) -> Result<()> {
        self.reconcile_forced_with_source(&std::collections::HashSet::new(), "reconciliation")
    }

    fn reconcile_forced(&self, forced: &std::collections::HashSet<String>) -> Result<()> {
        self.reconcile_forced_with_source(forced, "reconciliation")
    }

    fn reconcile_forced_with_source(
        &self,
        forced: &std::collections::HashSet<String>,
        source: &str,
    ) -> Result<()> {
        self.recover_pending_chat_mutations()?;
        let before = self
            .index
            .lock()
            .map_err(|_| lock_error("workspace_reconcile"))?
            .query_objects(None)?
            .into_iter()
            .map(|object| (object.id.clone(), object))
            .collect::<HashMap<_, _>>();
        let metadata = self
            .index
            .lock()
            .map_err(|_| lock_error("workspace_reconcile"))?
            .file_metadata()?;
        let scan = scan_changes(&self.root, &self.current_ignore(), &metadata, forced)?;
        let removed = metadata
            .keys()
            .filter(|path| !scan.seen.contains(*path))
            .cloned()
            .collect::<Vec<_>>();
        if scan.changed.is_empty() && removed.is_empty() {
            return Ok(());
        }
        let mut index = self
            .index
            .lock()
            .map_err(|_| lock_error("workspace_reconcile"))?;
        index.reconcile_markdown(&scan.changed, &removed)?;
        drop(index);
        let after = self
            .index
            .lock()
            .map_err(|_| lock_error("workspace_reconcile"))?
            .query_objects(None)?
            .into_iter()
            .map(|object| (object.id.clone(), object))
            .collect::<HashMap<_, _>>();
        self.emit_reconciled_object_events(&before, &after, source);
        self.emit("search:index-updated", source, serde_json::json!({}));
        Ok(())
    }

    pub fn poll_external_changes(&self, wait: std::time::Duration) -> Result<Vec<String>> {
        let paths = self.watcher.drain_coalesced(wait)?;
        self.process_external_changes(paths)
    }

    fn process_external_changes(&self, paths: Vec<PathBuf>) -> Result<Vec<String>> {
        // The manifest sits outside the indexed workspace (it is never a
        // workspace object), so watcher events for `workspace.yaml` are
        // reconciled directly against the in-memory snapshot instead of
        // `file:changed`. This must run before the ignore set is compiled:
        // an external edit to `ignore` scopes the very scan below.
        self.sync_external_manifest(&paths)?;
        let ignores = compile_workspace_ignores(&self.root, &self.current_ignore())?;
        let mut external = Vec::new();
        let mut journal = self
            .self_writes
            .lock()
            .map_err(|_| lock_error("watcher_poll"))?;
        for path in paths {
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            if !is_visible_workspace_path(relative, path.is_dir(), &ignores) {
                continue;
            }
            let relative = relative
                .to_str()
                .map(|value| value.replace('\\', "/"))
                .ok_or_else(|| {
                    CoreError::validation(
                        "non_utf8_path",
                        "A changed workspace path is not UTF-8",
                        "watcher_poll",
                    )
                })?;
            if let Some(expected) = journal.get(&relative).cloned() {
                let matches = if expected == "<deleted>" {
                    !path.exists()
                } else {
                    std::fs::read(&path)
                        .ok()
                        .is_some_and(|bytes| markdown::revision(&bytes) == expected)
                };
                journal.remove(&relative);
                if matches {
                    continue;
                }
            }
            external.push(relative);
        }
        drop(journal);
        external.sort();
        external.dedup();
        if !external.is_empty() {
            self.reconcile_forced_with_source(&external.iter().cloned().collect(), "external")?;
            self.emit(
                "file:changed",
                "external",
                serde_json::json!({ "paths": external }),
            );
        }
        Ok(external)
    }

    /// Applies watcher events for `workspace.yaml`: journal-suppress the
    /// engine's own atomic write, then adopt any external change. The
    /// journal guard is released before the adopt so it cannot interleave
    /// with the write lock taken by `manifest_update`.
    fn sync_external_manifest(&self, paths: &[PathBuf]) -> Result<()> {
        let touched = paths.iter().any(|path| {
            path.strip_prefix(&self.root)
                .is_ok_and(|relative| relative == Path::new("workspace.yaml"))
        });
        if !touched {
            return Ok(());
        }
        let journaled = self
            .self_writes
            .lock()
            .map_err(|_| lock_error("watcher_poll"))?
            .remove("workspace.yaml");
        if let Some(expected) = journaled {
            let unchanged = std::fs::read(self.root.join("workspace.yaml"))
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
            "external",
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

    pub fn create_object(
        &self,
        input: CreateObjectInput,
    ) -> Result<MutationResult<WorkspaceObject>> {
        validate_object_type(&input.object_type)?;
        if input.title.trim().is_empty() {
            return Err(CoreError::validation(
                "title_required",
                "A title is required",
                "object_create",
            ));
        }
        let now = now_rfc3339();
        let mut properties = input.properties;
        properties.remove("id");
        properties.remove("type");
        properties.remove("created");
        properties.remove("updated");
        normalize_domain_properties(&input.object_type, &mut properties)?;
        let id = new_object_id(&input.object_type);
        let relative_path = input
            .relative_path
            .unwrap_or_else(|| default_object_path(&input.object_type, &input.title, &id));
        let object = WorkspaceObject {
            id,
            object_type: input.object_type,
            title: input.title.trim().into(),
            body: input.body,
            relative_path,
            revision: String::new(),
            created: Some(now.clone()),
            updated: Some(now),
            properties,
        };
        if self.root.join(&object.relative_path).exists() {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A file already exists at the requested path",
                "object_create",
            ));
        }
        self.commit_object(object, None, "object:created", "object_create")
    }

    pub fn adopt_markdown(
        &self,
        relative_path: &str,
        object_type: &str,
        expected_revision: &str,
    ) -> Result<MutationResult<WorkspaceObject>> {
        validate_object_type(object_type)?;
        let path = resolve_for_write(&self.root, relative_path, "object_adopt")?;
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, "object_adopt", Some(relative_path)))?;
        check_revision(&bytes, expected_revision, "object_adopt")?;
        let ParsedMarkdown::Unmanaged {
            title,
            body,
            frontmatter,
        } = markdown::parse_markdown(relative_path, &bytes)
        else {
            return Err(CoreError::validation(
                "not_adoptable",
                "Only idless Markdown can be adopted",
                "object_adopt",
            ));
        };
        let now = now_rfc3339();
        let mut properties = frontmatter.unwrap_or_default();
        properties.remove("id");
        properties.remove("type");
        let created = take_optional_timestamp(&mut properties, "created", "object_adopt")?
            .unwrap_or_else(|| now.clone());
        let updated = take_optional_timestamp(&mut properties, "updated", "object_adopt")?
            .unwrap_or_else(|| now.clone());
        let object = WorkspaceObject {
            id: new_object_id(object_type),
            object_type: object_type.into(),
            title: if title.is_empty() {
                file_stem(relative_path)
            } else {
                title
            },
            body,
            relative_path: relative_path.into(),
            revision: String::new(),
            created: Some(created),
            updated: Some(updated),
            properties,
        };
        self.commit_object(
            object,
            Some(expected_revision),
            "object:created",
            "object_adopt",
        )
    }

    pub fn get_object(&self, id: &str) -> Result<Option<WorkspaceObject>> {
        self.index
            .lock()
            .map_err(|_| lock_error("object_get"))?
            .get_object(id)
    }

    pub fn reconcile_note_draft(&self, input: DraftReconcileInput) -> Result<DraftReconcileResult> {
        let (current, current_bytes) = self.read_canonical_object(&input.id, "note_reconcile")?;
        if current.object_type != "note" {
            return Err(CoreError::validation(
                "object_type_mismatch",
                "Only note drafts can be reconciled",
                "note_reconcile",
            ));
        }
        if current.revision == input.base_revision {
            return Ok(DraftReconcileResult::Unchanged {
                current,
                body: input.local_body,
            });
        }
        match merge_markdown_body(&input.base_body, &input.local_body, &current.body) {
            Some(body) => {
                self.snapshot_bytes(&current.id, "external", &current_bytes)?;
                Ok(DraftReconcileResult::Merged { current, body })
            }
            None => Ok(DraftReconcileResult::Conflict { current }),
        }
    }

    pub fn resolve_note_conflict(
        &self,
        input: ResolveConflictInput,
    ) -> Result<MutationResult<WorkspaceObject>> {
        let (mut current, current_bytes) =
            self.read_canonical_object(&input.id, "note_conflict_resolve")?;
        if current.object_type != "note" {
            return Err(CoreError::validation(
                "object_type_mismatch",
                "Only note conflicts can be resolved",
                "note_conflict_resolve",
            ));
        }
        if current.revision != input.current_revision {
            let mut error = CoreError::new(
                "revision_conflict",
                ErrorCategory::Conflict,
                "The file changed again while the conflict was being reviewed",
                "note_conflict_resolve",
            );
            error.details = Some(serde_json::json!({"currentRevision": current.revision}));
            return Err(error);
        }
        match input.resolution {
            ConflictResolution::UseExternal => {
                let mut local = current.clone();
                local.body = input.local_body;
                let local_bytes = markdown::serialize_object(&local)?;
                self.snapshot_bytes(&current.id, "local", &local_bytes)?;
                Ok(MutationResult {
                    value: current.clone(),
                    revision: current.revision.clone(),
                    durability: "committed".into(),
                    index_status: IndexStatus::Updated,
                    warnings: Vec::new(),
                })
            }
            ConflictResolution::ReplaceExternal => {
                self.snapshot_bytes(&current.id, "external", &current_bytes)?;
                current.body = input.local_body;
                current.updated = Some(now_rfc3339());
                self.commit_object(
                    current,
                    Some(&input.current_revision),
                    "object:updated",
                    "note_conflict_resolve",
                )
            }
        }
    }

    // --- Managed draft reconciliation (notes, tasks, and future types) ---

    /// Reconcile a managed draft against the canonical file without writing.
    /// Returns the merged draft content for clients that still need to render
    /// the combination, or an explicit conflict marker.
    pub fn reconcile_managed_draft(&self, input: ManagedDraftInput) -> Result<ManagedDraftResult> {
        let canonical =
            self.read_canonical_workspace_object(&input.id, "managed_draft_reconcile")?;
        if canonical.revision == input.base_revision
            && canonical.title == input.base_title
            && canonical.body == input.base_body
            && normalized_properties(&canonical.properties)
                == normalized_properties(&input.base_properties)
        {
            return Ok(ManagedDraftResult::Unchanged { current: canonical });
        }
        match merge_managed_fields(&input, &canonical) {
            Ok(merged) => Ok(ManagedDraftResult::Merged {
                current: canonical.clone(),
                title: merged.title,
                body: merged.body,
                properties: merged.properties.clone(),
            }),
            Err(_) => Ok(ManagedDraftResult::Conflict { current: canonical }),
        }
    }

    /// Reconcile and durably commit a managed draft in one operation. A clean
    /// base writes the local draft; a changed base merges field-by-field and
    /// snapshots every displaced version before replacing bytes atomically.
    pub fn save_managed_draft(&self, input: ManagedDraftInput) -> Result<ManagedDraftResult> {
        let canonical = self.read_canonical_workspace_object(&input.id, "managed_draft_save")?;
        if std::env::var("NOURA_DEBUG_MERGE").is_ok() {
            eprintln!("canonical body: {:?}", canonical.body);
            eprintln!("base body: {:?}", input.base_body);
        }
        let unchanged = canonical.revision == input.base_revision
            && canonical.title == input.base_title
            && canonical.body == input.base_body
            && normalized_properties(&canonical.properties)
                == normalized_properties(&input.base_properties);
        if unchanged {
            let mut object = canonical;
            let expected_revision = object.revision.clone();
            object.title = input.local_title;
            object.body = input.local_body;
            object.properties = normalized_properties(&input.local_properties);
            let result =
                self.apply_managed_object(object, Some(&expected_revision), "managed_draft_save")?;
            return Ok(ManagedDraftResult::Unchanged {
                current: result.value,
            });
        }
        let merged = match merge_managed_fields(&input, &canonical) {
            Ok(value) => value,
            Err(_) => return Ok(ManagedDraftResult::Conflict { current: canonical }),
        };
        let canonical_path =
            resolve_for_write(&self.root, &canonical.relative_path, "managed_draft_save")?;
        let canonical_bytes = std::fs::read(&canonical_path).map_err(|error| {
            CoreError::io(error, "managed_draft_save", Some(&canonical.relative_path))
        })?;
        self.snapshot_bytes(&canonical.id, "external", &canonical_bytes)?;
        let mut object = merged;
        object.properties = normalized_properties(&object.properties);
        let result =
            self.apply_managed_object(object, Some(&canonical.revision), "managed_draft_save")?;
        Ok(ManagedDraftResult::Merged {
            current: result.value.clone(),
            title: result.value.title.clone(),
            body: result.value.body.clone(),
            properties: result.value.properties.clone(),
        })
    }

    /// Adopt the external file version or replace it with the reviewed local
    /// draft. Both directions snapshot the version they displace before any
    /// durable write, and both return the object the client should display.
    pub fn resolve_managed_conflict(
        &self,
        input: ManagedConflictResolveInput,
    ) -> Result<WorkspaceObject> {
        let (current, current_bytes) =
            match self.read_canonical_object(&input.id, "managed_conflict_resolve") {
                Ok(value) => value,
                Err(error)
                    if error.code == "object_not_found"
                        && input.resolution == ManagedConflictResolution::ReplaceExternal =>
                {
                    return self.restore_deleted_managed_object(input);
                }
                Err(error) => return Err(error),
            };
        if current.revision != input.current_revision {
            let mut error = CoreError::new(
                "revision_conflict",
                ErrorCategory::Conflict,
                "The file changed again while the conflict was being reviewed",
                "managed_conflict_resolve",
            );
            error.details = Some(serde_json::json!({"currentRevision": current.revision}));
            return Err(error);
        }
        match input.resolution {
            ManagedConflictResolution::UseExternal => {
                let mut local = current.clone();
                local.title = input.local_title;
                local.body = input.local_body;
                local.properties = normalized_properties(&input.local_properties);
                let local_bytes = markdown::serialize_object(&local)?;
                self.snapshot_bytes(&current.id, "local", &local_bytes)?;
                Ok(current)
            }
            ManagedConflictResolution::ReplaceExternal => {
                self.snapshot_bytes(&current.id, "external", &current_bytes)?;
                let mut object = current;
                object.title = input.local_title;
                object.body = input.local_body;
                object.properties = normalized_properties(&input.local_properties);
                let result = self.apply_managed_object(
                    object,
                    Some(&input.current_revision),
                    "managed_conflict_resolve",
                )?;
                Ok(result.value)
            }
        }
    }

    fn restore_deleted_managed_object(
        &self,
        input: ManagedConflictResolveInput,
    ) -> Result<WorkspaceObject> {
        crate::path::validate_relative(&input.relative_path, "managed_object_restore")?;
        let object_type = input.id.split('_').next().unwrap_or_default().to_owned();
        if object_type.is_empty() || !valid_object_id(&input.id, &object_type) {
            return Err(CoreError::validation(
                "invalid_object_id",
                "The object ID is invalid",
                "managed_conflict_resolve",
            ));
        }
        let timestamp = now_rfc3339();
        let created = match input.created {
            Some(created) => {
                ensure_rfc3339_timestamp(
                    &created,
                    "created must be an RFC 3339 timestamp".into(),
                    "managed_conflict_resolve",
                )?;
                Some(created)
            }
            None => Some(timestamp.clone()),
        };
        let object = WorkspaceObject {
            id: input.id,
            object_type,
            title: input.local_title,
            body: input.local_body,
            relative_path: input.relative_path,
            revision: String::new(),
            created,
            updated: Some(timestamp),
            properties: normalized_properties(&input.local_properties),
        };
        let result = self.apply_managed_object(object, None, "managed_conflict_resolve")?;
        Ok(result.value)
    }

    fn read_canonical_workspace_object(
        &self,
        id: &str,
        operation: &str,
    ) -> Result<WorkspaceObject> {
        let (object, _) = self.read_canonical_object(id, operation)?;
        Ok(object)
    }

    fn apply_managed_object(
        &self,
        mut object: WorkspaceObject,
        expected: Option<&str>,
        operation: &str,
    ) -> Result<MutationResult<WorkspaceObject>> {
        normalize_domain_properties(&object.object_type, &mut object.properties)?;
        object.updated = Some(now_rfc3339());
        self.commit_object(object, expected, "object:updated", operation)
    }

    // --- Raw Markdown (unmanaged and malformed files) ---

    /// Read one Markdown file addressed by relative path. Managed frontmatter
    /// is returned as-is; the editor owns complete raw contents.
    pub fn read_raw_markdown(&self, relative_path: &str) -> Result<RawMarkdownRead> {
        let path = validate_raw_markdown_path(&self.root, relative_path)?;
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, "raw_markdown_read", Some(relative_path)))?;
        let (body, uses_crlf, has_bom) = split_raw_bytes(&bytes)?;
        Ok(RawMarkdownRead {
            relative_path: relative_path.to_owned(),
            body,
            revision: markdown::revision(&bytes),
            uses_crlf,
            has_bom,
        })
    }

    /// Read one non-Markdown file (image or other asset) for inline preview.
    /// Workspace containment is validated; total size is capped by the caller.
    pub fn read_local_asset(
        &self,
        source_relative_path: &str,
        target: &str,
        max_bytes: i64,
    ) -> Result<(String, Vec<u8>)> {
        let relative_path = resolve_markdown_target(source_relative_path, target, false)?;
        if relative_path.to_ascii_lowercase().ends_with(".md") {
            return Err(CoreError::validation(
                "invalid_asset_path",
                "Markdown content is read through raw Markdown operations",
                "raw_asset_read",
            ));
        }
        let path = resolve_for_write(&self.root, &relative_path, "raw_asset_read")?;
        let metadata = std::fs::metadata(&path)
            .map_err(|error| CoreError::io(error, "raw_asset_read", Some(&relative_path)))?;
        if metadata.len() as i64 > max_bytes {
            return Err(CoreError::validation(
                "asset_too_large",
                "The local asset exceeds the preview size limit",
                "raw_asset_read",
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, "raw_asset_read", Some(&relative_path)))?;
        Ok((relative_path, bytes))
    }

    pub fn resolve_markdown_link(
        &self,
        source_relative_path: &str,
        target: &str,
    ) -> Result<MarkdownLinkTarget> {
        if target.starts_with("http://") || target.starts_with("https://") {
            return Ok(MarkdownLinkTarget::Unresolved);
        }
        // A target that only names an alias or heading has no file behind it
        // to resolve; callers represent it as an ordinary unresolved link.
        if markdown_target_path(target, true).is_empty() {
            return Ok(MarkdownLinkTarget::Unresolved);
        }
        let mut relative = resolve_markdown_target(source_relative_path, target, true)?;
        let mut path = resolve_for_write(&self.root, &relative, "markdown_link_resolve")?;
        if !path.exists() && Path::new(&relative).extension().is_none() {
            relative.push_str(".md");
            path = resolve_for_write(&self.root, &relative, "markdown_link_resolve")?;
        }
        if !path.exists() || !path.is_file() {
            return Ok(MarkdownLinkTarget::Unresolved);
        }
        if relative.to_ascii_lowercase().ends_with(".md") {
            let bytes = std::fs::read(&path)
                .map_err(|error| CoreError::io(error, "markdown_link_resolve", Some(&relative)))?;
            if let ParsedMarkdown::Managed(object) = markdown::parse_markdown(&relative, &bytes) {
                return Ok(MarkdownLinkTarget::Managed { object });
            }
            return self
                .read_raw_markdown(&relative)
                .map(|document| MarkdownLinkTarget::Markdown { document });
        }
        Ok(MarkdownLinkTarget::Asset {
            relative_path: relative,
        })
    }

    fn reindex_raw_markdown(
        &self,
        relative: &str,
        bytes: &[u8],
        operation: &str,
    ) -> Result<IndexStatus> {
        let destination = resolve_for_write(&self.root, relative, operation)?;
        let parsed = markdown::parse_markdown(relative, bytes);
        let result = self
            .index
            .lock()
            .map_err(|_| lock_error(operation))
            .and_then(|mut index| {
                index.upsert_markdown(relative, bytes, mtime_ns(&destination), &parsed)
            });
        let (index_status, _) = self.index_outcome(result);
        Ok(index_status)
    }

    /// Reconcile a raw Markdown draft against the canonical file without
    /// writing. Bodies merge line-by-line; every overlap requires review.
    pub fn reconcile_raw_markdown(&self, input: RawReconcileInput) -> Result<RawReconcileResult> {
        let path = validate_raw_markdown_path(&self.root, &input.relative_path)?;
        if !path.exists() {
            return Err(CoreError::validation(
                "raw_markdown_missing",
                "The Markdown file no longer exists",
                "raw_markdown_reconcile",
            ));
        }
        let bytes = std::fs::read(&path).map_err(|error| {
            CoreError::io(error, "raw_markdown_reconcile", Some(&input.relative_path))
        })?;
        let (external_body, uses_crlf, has_bom) = split_raw_bytes(&bytes)?;
        let current = RawMarkdownRead {
            relative_path: input.relative_path.clone(),
            body: external_body.clone(),
            revision: markdown::revision(&bytes),
            uses_crlf,
            has_bom,
        };
        if current.revision == input.base_revision {
            return Ok(RawReconcileResult::Unchanged { current });
        }
        match Self::merge_raw_body(&input, &external_body) {
            Some(merged) => Ok(RawReconcileResult::Merged {
                current: RawMarkdownRead {
                    body: merged,
                    ..current
                },
            }),
            None => Ok(RawReconcileResult::Conflict { current }),
        }
    }

    fn merge_raw_body(input: &RawReconcileInput, external_body: &str) -> Option<String> {
        merge_markdown_text(&input.base_body, &input.local_body, external_body)
    }

    /// Reconcile and durably commit a raw Markdown draft. A clean base writes
    /// the local body; a changed base merges line-by-line and snapshots the
    /// displaced external version. CRLF and BOM conventions are preserved.
    pub fn save_raw_markdown(&self, input: RawSaveInput) -> Result<RawSaveResult> {
        let relative = crate::path::validate_relative(&input.relative_path, "raw_markdown_save")?
            .to_str()
            .ok_or_else(|| {
                CoreError::validation(
                    "non_utf8_path",
                    "The raw Markdown path is not UTF-8",
                    "raw_markdown_save",
                )
            })?
            .replace('\\', "/");
        let path = validate_raw_markdown_path(&self.root, &relative)?;
        let _guard = self.write_lock("raw_markdown_save")?;
        if !path.exists() {
            return Err(CoreError::validation(
                "raw_markdown_missing",
                "The Markdown file no longer exists",
                "raw_markdown_save",
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, "raw_markdown_save", Some(&relative)))?;
        let (external_body, uses_crlf, has_bom) = split_raw_bytes(&bytes)?;
        let current = RawMarkdownRead {
            relative_path: relative.clone(),
            body: external_body.clone(),
            revision: markdown::revision(&bytes),
            uses_crlf,
            has_bom,
        };
        let merged_body = if current.revision == input.base_revision {
            input.local_body.clone()
        } else {
            match merge_markdown_text(&input.base_body, &input.local_body, &external_body) {
                Some(merged) => {
                    let segment = raw_history_dir(&relative);
                    write_snapshot(
                        &self.root,
                        "raw_markdown_save",
                        &segment,
                        "external",
                        &bytes,
                    )?;
                    merged
                }
                None => {
                    return Ok(RawSaveResult::Conflict {
                        current,
                        managed_object: None,
                    });
                }
            }
        };
        let next_bytes = compose_raw_bytes(&merged_body, uses_crlf, has_bom);
        let next_relative = PathBuf::from(&relative);
        atomic_write_checked(
            &self.root,
            &next_relative,
            &next_bytes,
            None,
            "raw_markdown_save",
        )?;
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(relative.clone(), markdown::revision(&next_bytes));
        }
        self.reindex_raw_markdown(&relative, &next_bytes, "raw_markdown_save")?;
        self.emit(
            "file:changed",
            "application",
            serde_json::json!({ "paths": [relative] }),
        );
        self.emit("search:index-updated", "application", serde_json::json!({}));
        let managed_object = match markdown::parse_markdown(&relative, &next_bytes) {
            ParsedMarkdown::Managed(object) => Some(object),
            _ => None,
        };
        Ok(RawSaveResult::Saved {
            current: RawMarkdownRead {
                relative_path: relative,
                body: merged_body,
                revision: markdown::revision(&next_bytes),
                uses_crlf,
                has_bom,
            },
            managed_object,
        })
    }

    /// Adopt the external file version or replace it with the reviewed local
    /// draft. Both directions snapshot the version they displace first.
    pub fn resolve_raw_conflict(
        &self,
        input: RawConflictResolveInput,
    ) -> Result<RawConflictResolveResult> {
        let relative =
            crate::path::validate_relative(&input.relative_path, "raw_markdown_resolve")?
                .to_str()
                .ok_or_else(|| {
                    CoreError::validation(
                        "non_utf8_path",
                        "The raw Markdown path is not UTF-8",
                        "raw_markdown_resolve",
                    )
                })?
                .replace('\\', "/");
        let path = validate_raw_markdown_path(&self.root, &relative)?;
        let _guard = self.write_lock("raw_markdown_resolve")?;
        if !path.exists() {
            return Err(CoreError::validation(
                "raw_markdown_missing",
                "The Markdown file no longer exists",
                "raw_markdown_resolve",
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, "raw_markdown_resolve", Some(&relative)))?;
        if markdown::revision(&bytes) != input.current_revision {
            let mut error = CoreError::new(
                "revision_conflict",
                ErrorCategory::Conflict,
                "The file changed again while the conflict was being reviewed",
                "raw_markdown_resolve",
            );
            error.details = Some(serde_json::json!({
                "currentRevision": markdown::revision(&bytes),
            }));
            return Err(error);
        }
        let (external_body, uses_crlf, has_bom) = split_raw_bytes(&bytes)?;
        match input.resolution {
            ConflictResolution::UseExternal => {
                let segment = raw_history_dir(&relative);
                let local_bytes = compose_raw_bytes(&input.local_body, uses_crlf, has_bom);
                write_snapshot(
                    &self.root,
                    "raw_markdown_resolve",
                    &segment,
                    "local",
                    &local_bytes,
                )?;
                let managed_object = match markdown::parse_markdown(&relative, &bytes) {
                    ParsedMarkdown::Managed(object) => Some(object),
                    _ => None,
                };
                Ok(RawConflictResolveResult {
                    current: RawMarkdownRead {
                        relative_path: relative,
                        body: external_body,
                        revision: markdown::revision(&bytes),
                        uses_crlf,
                        has_bom,
                    },
                    managed_object,
                })
            }
            ConflictResolution::ReplaceExternal => {
                let segment = raw_history_dir(&relative);
                write_snapshot(
                    &self.root,
                    "raw_markdown_resolve",
                    &segment,
                    "external",
                    &bytes,
                )?;
                let next_bytes = compose_raw_bytes(&input.local_body, uses_crlf, has_bom);
                let next_relative = PathBuf::from(&relative);
                atomic_write_checked(
                    &self.root,
                    &next_relative,
                    &next_bytes,
                    None,
                    "raw_markdown_resolve",
                )?;
                if let Ok(mut journal) = self.self_writes.lock() {
                    journal.insert(relative.clone(), markdown::revision(&next_bytes));
                }
                self.reindex_raw_markdown(&relative, &next_bytes, "raw_markdown_resolve")?;
                self.emit(
                    "file:changed",
                    "application",
                    serde_json::json!({ "paths": [relative] }),
                );
                self.emit("search:index-updated", "application", serde_json::json!({}));
                let managed_object = match markdown::parse_markdown(&relative, &next_bytes) {
                    ParsedMarkdown::Managed(object) => Some(object),
                    _ => None,
                };
                Ok(RawConflictResolveResult {
                    current: RawMarkdownRead {
                        relative_path: relative,
                        body: input.local_body,
                        revision: markdown::revision(&next_bytes),
                        uses_crlf,
                        has_bom,
                    },
                    managed_object,
                })
            }
        }
    }

    pub fn create_chat(&self, input: CreateChatInput) -> Result<MutationResult<Chat>> {
        let title = validate_chat_title(&input.title, "chat_create")?;
        let retention = input.retention.unwrap_or(ChatRetention::Permanent);
        validate_retention(retention, input.retention_days, "chat_create")?;
        let now = now_rfc3339();
        let id = new_object_id("chat");
        let slug = slug::slugify(title);
        let slug = if slug.is_empty() { "untitled" } else { &slug };
        let short_id = id[id.len() - 6..].to_owned();
        let mut chat = Chat {
            id,
            title: title.into(),
            relative_path: format!("chats/{slug}--{short_id}/chat.md"),
            revision: String::new(),
            created: now.clone(),
            updated: now,
            retention,
            retention_days: input.retention_days,
            properties: BTreeMap::new(),
        };
        let _guard = self.write_lock("chat_create")?;
        let (index_status, warnings) = self.write_chat_unlocked(&mut chat, None, "chat_create")?;
        self.emit_chat_event("chat:created", &chat, "application", serde_json::json!({}));
        Ok(MutationResult {
            value: chat.clone(),
            revision: chat.revision,
            durability: "committed".into(),
            index_status,
            warnings,
        })
    }

    pub fn change_chat_retention(
        &self,
        input: ChangeChatRetentionInput,
    ) -> Result<MutationResult<Chat>> {
        validate_retention(
            input.retention,
            input.retention_days,
            "chat_change_retention",
        )?;
        let mut chat = self.read_canonical_chat(&input.chat_id, "chat_change_retention")?;
        let _guard = self.write_lock("chat_change_retention")?;
        self.check_chat_revision_unlocked(
            &chat,
            &input.expected_chat_revision,
            "chat_change_retention",
        )?;
        chat.retention = input.retention;
        chat.retention_days = input.retention_days;
        chat.updated = now_rfc3339();
        let (index_status, warnings) = self.write_chat_unlocked(
            &mut chat,
            Some(&input.expected_chat_revision),
            "chat_change_retention",
        )?;
        self.emit_chat_event(
            "chat:retention-changed",
            &chat,
            "application",
            serde_json::json!({}),
        );
        Ok(MutationResult {
            value: chat.clone(),
            revision: chat.revision,
            durability: "committed".into(),
            index_status,
            warnings,
        })
    }

    pub fn rename_chat(&self, input: RenameChatInput) -> Result<MutationResult<Chat>> {
        let title = validate_chat_title(&input.title, "chat_rename")?;
        let mut chat = self.read_canonical_chat(&input.chat_id, "chat_rename")?;
        let _guard = self.write_lock("chat_rename")?;
        self.check_chat_revision_unlocked(&chat, &input.expected_chat_revision, "chat_rename")?;
        chat.title = title.into();
        chat.updated = now_rfc3339();
        let (index_status, warnings) = self.write_chat_unlocked(
            &mut chat,
            Some(&input.expected_chat_revision),
            "chat_rename",
        )?;
        self.emit_chat_event("chat:renamed", &chat, "application", serde_json::json!({}));
        Ok(MutationResult {
            value: chat.clone(),
            revision: chat.revision,
            durability: "committed".into(),
            index_status,
            warnings,
        })
    }

    pub fn list_chats(&self) -> Result<Vec<Chat>> {
        self.reconcile()?;
        let objects = self.query_objects(Some("chat"))?;
        let mut chats = Vec::with_capacity(objects.len());
        for object in objects {
            let path = resolve_for_write(&self.root, &object.relative_path, "chat_list")?;
            let bytes = std::fs::read(&path)
                .map_err(|error| CoreError::io(error, "chat_list", Some(&object.relative_path)))?;
            let chat = parse_chat(&object.relative_path, &bytes)?;
            chats.push((parse_chat_timestamp(&chat.updated, "chat_list")?, chat));
        }
        chats.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| left.1.id.cmp(&right.1.id))
        });
        Ok(chats.into_iter().map(|(_, chat)| chat).collect())
    }

    pub fn read_chat(&self, id: &str) -> Result<ChatRead> {
        let chat = self.read_canonical_chat(id, "chat_read")?;
        let mut messages = self
            .query_objects(Some("chat-message"))?
            .into_iter()
            .filter(|object| {
                object
                    .properties
                    .get("chat_id")
                    .and_then(serde_json::Value::as_str)
                    == Some(id)
            })
            .map(|object| {
                let path = resolve_for_write(&self.root, &object.relative_path, "chat_read")?;
                let bytes = std::fs::read(path).map_err(|error| {
                    CoreError::io(error, "chat_read", Some(&object.relative_path))
                })?;
                let message = parse_chat_message(&object.relative_path, &bytes)?;
                Ok((
                    parse_chat_timestamp(&message.created, "chat_read")?,
                    message,
                ))
            })
            .collect::<Result<Vec<_>>>()?;
        messages.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| left.1.id.cmp(&right.1.id))
        });
        Ok(ChatRead {
            chat,
            messages: messages.into_iter().map(|(_, message)| message).collect(),
        })
    }

    pub fn append_chat_user_message(
        &self,
        input: AppendChatUserMessageInput,
    ) -> Result<MutationResult<ChatMessage>> {
        self.append_chat_message(
            &input.chat_id,
            &input.expected_chat_revision,
            ChatMessageKind::User,
            ChatMessageStatus::Completed,
            input.content,
            input.run_id,
            None,
            None,
            None,
            None,
            None,
            "chat:message-appended",
            "chat_append_user_message",
        )
    }

    pub fn begin_chat_assistant(
        &self,
        input: BeginChatAssistantInput,
    ) -> Result<MutationResult<ChatMessage>> {
        self.append_chat_message(
            &input.chat_id,
            &input.expected_chat_revision,
            ChatMessageKind::Assistant,
            ChatMessageStatus::InProgress,
            String::new(),
            input.run_id,
            Some(input.provider_id),
            Some(input.model_id),
            None,
            None,
            None,
            "chat:assistant-began",
            "chat_begin_assistant",
        )
    }

    pub fn finish_chat_assistant(
        &self,
        input: FinishChatAssistantInput,
    ) -> Result<MutationResult<ChatMessage>> {
        self.finish_chat_message(
            &input.chat_id,
            &input.message_id,
            &input.expected_chat_revision,
            &input.expected_message_revision,
            ChatMessageKind::Assistant,
            input.content,
            input.status,
            input.error_code,
            "chat:assistant-finished",
            "chat_finish_assistant",
        )
    }

    pub fn begin_chat_tool_call(
        &self,
        input: BeginChatToolCallInput,
    ) -> Result<MutationResult<ChatMessage>> {
        validate_tool_fields(
            &input.tool_call_id,
            &input.tool_name,
            "chat_begin_tool_call",
        )?;
        self.append_chat_message(
            &input.chat_id,
            &input.expected_chat_revision,
            ChatMessageKind::ToolCall,
            ChatMessageStatus::InProgress,
            input.content,
            input.run_id,
            None,
            None,
            Some(input.tool_call_id),
            Some(input.tool_name),
            None,
            "chat:tool-call-began",
            "chat_begin_tool_call",
        )
    }

    pub fn finish_chat_tool_call(
        &self,
        input: FinishChatToolCallInput,
    ) -> Result<MutationResult<ChatMessage>> {
        self.finish_chat_message(
            &input.chat_id,
            &input.message_id,
            &input.expected_chat_revision,
            &input.expected_message_revision,
            ChatMessageKind::ToolCall,
            input.content,
            input.status,
            input.error_code,
            "chat:tool-call-finished",
            "chat_finish_tool_call",
        )
    }

    pub fn append_chat_tool_result(
        &self,
        input: AppendChatToolResultInput,
    ) -> Result<MutationResult<ChatMessage>> {
        validate_tool_fields(
            &input.tool_call_id,
            &input.tool_name,
            "chat_append_tool_result",
        )?;
        self.append_chat_message(
            &input.chat_id,
            &input.expected_chat_revision,
            ChatMessageKind::ToolResult,
            ChatMessageStatus::Completed,
            input.content,
            input.run_id,
            None,
            None,
            Some(input.tool_call_id),
            Some(input.tool_name),
            None,
            "chat:tool-result-appended",
            "chat_append_tool_result",
        )
    }

    pub fn append_chat_context_summary(
        &self,
        input: AppendChatContextSummaryInput,
    ) -> Result<MutationResult<ChatMessage>> {
        self.append_chat_message(
            &input.chat_id,
            &input.expected_chat_revision,
            ChatMessageKind::ContextSummary,
            ChatMessageStatus::Completed,
            input.content,
            input.run_id,
            None,
            None,
            None,
            None,
            Some(input.summarizes_through_message_id),
            "chat:context-summary-appended",
            "chat_append_context_summary",
        )
    }

    pub fn recover_interrupted_chat(&self, id: &str) -> Result<ChatRead> {
        let mut read = self.read_chat(id)?;
        let completed_tool_results = read
            .messages
            .iter()
            .filter(|message| {
                message.kind == ChatMessageKind::ToolResult
                    && message.status == ChatMessageStatus::Completed
            })
            .cloned()
            .collect::<Vec<_>>();
        let interrupted = read
            .messages
            .iter_mut()
            .filter(|message| message.status == ChatMessageStatus::InProgress)
            .collect::<Vec<_>>();
        if interrupted.is_empty() {
            return Ok(read);
        }
        let _guard = self.write_lock("chat_recover_interrupted")?;
        let mut recovered_count = 0;
        for message in interrupted {
            // The write lock is already held. Reconciliation here would try to
            // acquire it again while recovering a pending mutation, so read the
            // canonical path captured in the preceding reconciled ChatRead.
            let path = resolve_for_write(
                &self.root,
                &message.relative_path,
                "chat_recover_interrupted",
            )?;
            let bytes = std::fs::read(&path).map_err(|error| {
                CoreError::io(
                    error,
                    "chat_recover_interrupted",
                    Some(&message.relative_path),
                )
            })?;
            let current = parse_chat_message(&message.relative_path, &bytes)?;
            if current.status != ChatMessageStatus::InProgress {
                *message = current;
                continue;
            }
            let mut recovered = current;
            recovered.status = if completed_tool_results
                .iter()
                .any(|result| tool_result_completes_call(result, &recovered))
            {
                ChatMessageStatus::Completed
            } else {
                ChatMessageStatus::Interrupted
            };
            recovered.updated = now_rfc3339();
            let expected_chat_revision = read.chat.revision.clone();
            read.chat.updated = now_rfc3339();
            self.commit_chat_mutation_unlocked(
                &mut read.chat,
                Some(&expected_chat_revision),
                &mut recovered,
                Some(&message.revision),
                "chat_recover_interrupted",
            )?;
            *message = recovered;
            recovered_count += 1;
        }
        if recovered_count == 0 {
            return Ok(read);
        }
        self.emit_chat_event(
            "chat:recovered",
            &read.chat,
            "application",
            serde_json::json!({ "interruptedMessages": read.messages.iter().filter(|message| message.status == ChatMessageStatus::Interrupted).count() }),
        );
        Ok(read)
    }

    pub fn expire_chats(&self, now: &str) -> Result<Vec<String>> {
        let now = now.parse::<jiff::Timestamp>().map_err(|_| {
            CoreError::validation(
                "invalid_timestamp",
                "Expiry time must be an RFC 3339 timestamp",
                "chat_expire",
            )
        })?;
        let chats = self.list_chats()?;
        let _guard = self.write_lock("chat_expire")?;
        let mut expired = Vec::new();
        for chat in chats {
            let Some(days) = chat.retention_days else {
                continue;
            };
            let created = chat.created.parse::<jiff::Timestamp>().map_err(|_| {
                CoreError::validation(
                    "invalid_timestamp",
                    "Chat creation time must be an RFC 3339 timestamp",
                    "chat_expire",
                )
            })?;
            let expires_at = created
                .checked_add(jiff::SignedDuration::from_hours(i64::from(days) * 24))
                .map_err(|_| {
                    CoreError::validation(
                        "invalid_timestamp",
                        "Chat retention period is invalid",
                        "chat_expire",
                    )
                })?;
            if expires_at > now || !self.expire_chat_directory(&chat, expires_at)? {
                continue;
            }
            expired.push(chat.id.clone());
            self.emit_chat_event("chat:expired", &chat, "application", serde_json::json!({}));
        }
        drop(_guard);
        if !expired.is_empty() {
            self.reconcile()?;
        }
        Ok(expired)
    }

    pub fn query_objects(&self, object_type: Option<&str>) -> Result<Vec<WorkspaceObject>> {
        self.index
            .lock()
            .map_err(|_| lock_error("object_query"))?
            .query_objects(object_type)
    }
    pub fn search(&self, input: &SearchInput) -> Result<Vec<SearchResult>> {
        self.index
            .lock()
            .map_err(|_| lock_error("search_query"))?
            .search(input)
    }
    pub fn calendar(&self, start: &str, end: &str) -> Result<Vec<CalendarEntry>> {
        self.index
            .lock()
            .map_err(|_| lock_error("calendar_query"))?
            .calendar(start, end)
    }

    pub fn update_object(
        &self,
        id: &str,
        patch: ObjectPatch,
    ) -> Result<MutationResult<WorkspaceObject>> {
        let mut object = self.get_object(id)?.ok_or_else(|| {
            CoreError::new(
                "object_not_found",
                ErrorCategory::Validation,
                "The object does not exist",
                "object_update",
            )
        })?;
        let bytes = std::fs::read(self.root.join(&object.relative_path))
            .map_err(|error| CoreError::io(error, "object_update", Some(&object.relative_path)))?;
        check_revision(&bytes, &patch.expected_revision, "object_update")?;
        if let Some(title) = patch.title {
            if title.trim().is_empty() {
                return Err(CoreError::validation(
                    "title_required",
                    "A title is required",
                    "object_update",
                ));
            }
            object.title = title.trim().into();
        }
        if let Some(body) = patch.body {
            object.body = body;
        }
        for key in patch.remove_properties {
            if !matches!(key.as_str(), "id" | "type" | "created" | "updated") {
                object.properties.remove(&key);
            }
        }
        for (key, value) in patch.properties {
            if !matches!(key.as_str(), "id" | "type" | "created" | "updated") {
                object.properties.insert(key, value);
            }
        }
        normalize_domain_properties(&object.object_type, &mut object.properties)?;
        object.updated = Some(now_rfc3339());
        self.commit_object(
            object,
            Some(&patch.expected_revision),
            "object:updated",
            "object_update",
        )
    }

    pub fn update_object_typed(
        &self,
        id: &str,
        expected_type: &str,
        patch: ObjectPatch,
    ) -> Result<MutationResult<WorkspaceObject>> {
        let object = self.get_object(id)?.ok_or_else(|| {
            CoreError::validation(
                "object_not_found",
                "The object does not exist",
                "object_update",
            )
        })?;
        if object.object_type != expected_type {
            return Err(CoreError::validation(
                "object_type_mismatch",
                "The object does not match this operation",
                "object_update",
            ));
        }
        self.update_object(id, patch)
    }

    pub fn move_object(
        &self,
        id: &str,
        destination: &str,
        expected_revision: &str,
    ) -> Result<MutationResult<WorkspaceObject>> {
        let mut object = self.get_object(id)?.ok_or_else(|| {
            CoreError::new(
                "object_not_found",
                ErrorCategory::Validation,
                "The object does not exist",
                "object_move",
            )
        })?;
        let source = resolve_for_write(&self.root, &object.relative_path, "object_move")?;
        let destination_path = resolve_for_write(&self.root, destination, "object_move")?;
        let _guard = self.write_lock("object_move")?;
        let bytes = std::fs::read(&source)
            .map_err(|error| CoreError::io(error, "object_move", Some(&object.relative_path)))?;
        check_revision(&bytes, expected_revision, "object_move")?;
        if destination_path.exists() {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A file already exists at the destination",
                "object_move",
            ));
        }
        if let Some(parent) = destination_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CoreError::io(error, "object_move", Some(destination)))?;
        }
        let old = object.relative_path.clone();
        std::fs::rename(&source, &destination_path)
            .map_err(|error| CoreError::io(error, "object_move", Some(destination)))?;
        sync_rename_parents(&source, &destination_path, "object_move")?;
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(old.clone(), "<deleted>".into());
            journal.insert(destination.into(), markdown::revision(&bytes));
        }
        object.relative_path = destination.into();
        let index_result = (|| {
            let mut index = self.index.lock().map_err(|_| lock_error("object_move"))?;
            index.remove_path(&old)?;
            let parsed = markdown::parse_markdown(destination, &bytes);
            index.upsert_markdown(destination, &bytes, mtime_ns(&destination_path), &parsed)
        })();
        let (index_status, warnings) = self.index_outcome(index_result);
        self.emit(
            "object:moved",
            "application",
            serde_json::json!({"id":id,"from":old,"to":destination}),
        );
        Ok(MutationResult {
            value: object,
            revision: markdown::revision(&bytes),
            durability: "committed".into(),
            index_status,
            warnings,
        })
    }

    pub fn delete_object(
        &self,
        id: &str,
        expected_revision: &str,
    ) -> Result<MutationResult<WorkspaceObject>> {
        let object = self.get_object(id)?.ok_or_else(|| {
            CoreError::new(
                "object_not_found",
                ErrorCategory::Validation,
                "The object does not exist",
                "object_delete",
            )
        })?;
        let source = resolve_for_write(&self.root, &object.relative_path, "object_delete")?;
        let _guard = self.write_lock("object_delete")?;
        let bytes = std::fs::read(&source)
            .map_err(|error| CoreError::io(error, "object_delete", Some(&object.relative_path)))?;
        check_revision(&bytes, expected_revision, "object_delete")?;
        let timestamp = now_rfc3339().replace([':', '.'], "-");
        let trash_relative = Path::new(".noura/trash")
            .join(timestamp)
            .join(&object.relative_path);
        let trash_value = trash_relative.to_string_lossy();
        let trash = resolve_for_write(&self.root, &trash_value, "trash_write")?;
        if let Some(parent) = trash.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CoreError::io(error, "object_delete", trash.to_str()))?;
        }
        std::fs::rename(&source, &trash)
            .map_err(|error| CoreError::io(error, "object_delete", Some(&object.relative_path)))?;
        sync_rename_parents(&source, &trash, "object_delete")?;
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(object.relative_path.clone(), "<deleted>".into());
        }
        let result = self
            .index
            .lock()
            .map_err(|_| lock_error("object_delete"))
            .and_then(|mut index| index.remove_path(&object.relative_path));
        let (index_status, warnings) = self.index_outcome(result);
        self.emit("object:deleted","application",serde_json::json!({"id":id,"path":object.relative_path,"trashPath":trash_relative.to_string_lossy()}));
        Ok(MutationResult {
            value: object,
            revision: markdown::revision(&bytes),
            durability: "committed".into(),
            index_status,
            warnings,
        })
    }

    pub fn create_folder(&self, relative_path: &str) -> Result<()> {
        let path = resolve_for_write(&self.root, relative_path, "folder_create")?;
        std::fs::create_dir_all(path)
            .map_err(|error| CoreError::io(error, "folder_create", Some(relative_path)))
    }

    pub fn list_folders(&self) -> Result<Vec<crate::FolderEntry>> {
        let mut folders = Vec::new();
        for entry in workspace_walker(&self.root, &self.current_ignore())? {
            let entry = entry.map_err(|error| {
                CoreError::new(
                    "scan_error",
                    ErrorCategory::Filesystem,
                    error.to_string(),
                    "folders_list",
                )
            })?;
            if entry.path() == self.root || !entry.file_type().is_some_and(|kind| kind.is_dir()) {
                continue;
            }
            folders.push(crate::FolderEntry {
                relative_path: normalized_relative_path(&self.root, entry.path(), "folders_list")?,
                name: entry
                    .file_name()
                    .to_str()
                    .map(str::to_owned)
                    .ok_or_else(|| {
                        CoreError::validation(
                            "non_utf8_path",
                            "A workspace folder name is not UTF-8",
                            "folders_list",
                        )
                    })?,
            });
        }
        folders.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(folders)
    }

    pub fn list_workspace_entries(&self) -> Result<Vec<WorkspaceEntry>> {
        self.reconcile()?;
        let metadata = self
            .index
            .lock()
            .map_err(|_| lock_error("files_list"))?
            .workspace_entry_metadata()?;
        let mut entries = Vec::new();
        for entry in workspace_walker(&self.root, &self.current_ignore())? {
            let entry = entry.map_err(|error| {
                CoreError::new(
                    "scan_error",
                    ErrorCategory::Filesystem,
                    error.to_string(),
                    "files_list",
                )
            })?;
            if entry.path() == self.root {
                continue;
            }
            let file_type = entry.file_type();
            if !file_type.is_some_and(|kind| kind.is_dir() || kind.is_file()) {
                continue;
            }
            let relative_path = normalized_relative_path(&self.root, entry.path(), "files_list")?;
            let name = entry
                .file_name()
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| {
                    CoreError::validation(
                        "non_utf8_path",
                        "A workspace entry name is not UTF-8",
                        "files_list",
                    )
                })?;
            if file_type.is_some_and(|kind| kind.is_dir()) {
                entries.push(WorkspaceEntry {
                    relative_path,
                    name,
                    kind: WorkspaceEntryKind::Folder,
                    parse_status: None,
                    object_id: None,
                    object_type: None,
                    revision: None,
                });
                continue;
            }
            let markdown = entry.path().extension().and_then(|value| value.to_str()) == Some("md");
            let indexed = if markdown {
                metadata.get(&relative_path)
            } else {
                None
            };
            if markdown && indexed.is_none() {
                return Err(CoreError::new(
                    "index_entry_missing",
                    ErrorCategory::Index,
                    "A Markdown file is missing from the local index",
                    "files_list",
                ));
            }
            entries.push(WorkspaceEntry {
                relative_path,
                name,
                kind: WorkspaceEntryKind::File,
                parse_status: indexed
                    .map(|value| value.parse_status)
                    .or(Some(ParseStatus::Binary)),
                object_id: indexed.and_then(|value| value.object_id.clone()),
                object_type: indexed.and_then(|value| value.object_type.clone()),
                revision: indexed.map(|value| value.revision.clone()),
            });
        }
        entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(entries)
    }

    pub fn list_non_managed_markdown(&self) -> Result<Vec<UnmanagedFile>> {
        self.reconcile()?;
        self.index
            .lock()
            .map_err(|_| lock_error("files_list_non_managed"))?
            .query_non_managed_markdown()
    }

    pub fn move_folder(&self, from: &str, to: &str) -> Result<()> {
        let source = resolve_for_write(&self.root, from, "folder_move")?;
        let destination = resolve_for_write(&self.root, to, "folder_move")?;
        let _guard = self.write_lock("folder_move")?;
        if !source.is_dir() {
            return Err(CoreError::validation(
                "folder_not_found",
                "The source folder does not exist",
                "folder_move",
            ));
        }
        if destination.exists() {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A folder already exists at the destination",
                "folder_move",
            ));
        }
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CoreError::io(error, "folder_move", Some(to)))?;
        }
        std::fs::rename(source, destination)
            .map_err(|error| CoreError::io(error, "folder_move", Some(to)))?;
        drop(_guard);
        self.reconcile()?;
        Ok(())
    }

    pub fn remove_empty_folder(&self, relative_path: &str) -> Result<()> {
        let path = resolve_for_write(&self.root, relative_path, "folder_remove")?;
        let _guard = self.write_lock("folder_remove")?;
        std::fs::remove_dir(path)
            .map_err(|error| CoreError::io(error, "folder_remove", Some(relative_path)))
    }

    fn commit_object(
        &self,
        mut object: WorkspaceObject,
        expected: Option<&str>,
        event: &str,
        operation: &str,
    ) -> Result<MutationResult<WorkspaceObject>> {
        let relative = crate::path::validate_relative(&object.relative_path, operation)?;
        let destination = resolve_for_write(&self.root, &object.relative_path, operation)?;
        let _guard = self.write_lock(operation)?;
        if let Some(expected) = expected {
            let current = std::fs::read(&destination)
                .map_err(|error| CoreError::io(error, operation, Some(&object.relative_path)))?;
            check_revision(&current, expected, operation)?;
        } else if destination.exists() {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A file already exists at the requested path",
                operation,
            ));
        }
        let bytes = markdown::serialize_object(&object)?;
        let revision = markdown::revision(&bytes);
        atomic_write_checked(&self.root, &relative, &bytes, expected, operation)?;
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(object.relative_path.clone(), revision.clone());
        }
        object.revision = revision.clone();
        let parsed = ParsedMarkdown::Managed(object.clone());
        let result = self
            .index
            .lock()
            .map_err(|_| lock_error(operation))
            .and_then(|mut index| {
                index.upsert_markdown(
                    &object.relative_path,
                    &bytes,
                    mtime_ns(&destination),
                    &parsed,
                )
            });
        let (index_status, warnings) = self.index_outcome(result);
        self.emit(event,"application",serde_json::json!({"id":object.id,"type":object.object_type,"path":object.relative_path,"revision":revision}));
        Ok(MutationResult {
            value: object,
            revision,
            durability: "committed".into(),
            index_status,
            warnings,
        })
    }

    fn read_canonical_chat(&self, id: &str, operation: &str) -> Result<Chat> {
        if !valid_object_id(id, "chat") {
            return Err(CoreError::validation(
                "invalid_chat_id",
                "The chat ID is invalid",
                operation,
            ));
        }
        self.reconcile()?;
        let object = self.get_object(id)?.ok_or_else(|| {
            CoreError::validation("chat_not_found", "The chat does not exist", operation)
        })?;
        if object.object_type != "chat" {
            return Err(CoreError::validation(
                "chat_not_found",
                "The chat does not exist",
                operation,
            ));
        }
        let path = resolve_for_write(&self.root, &object.relative_path, operation)?;
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, operation, Some(&object.relative_path)))?;
        parse_chat(&object.relative_path, &bytes)
    }

    fn read_canonical_chat_message(&self, id: &str, operation: &str) -> Result<ChatMessage> {
        if !valid_object_id(id, "chat-message") {
            return Err(CoreError::validation(
                "invalid_chat_message_id",
                "The chat message ID is invalid",
                operation,
            ));
        }
        self.reconcile()?;
        let object = self.get_object(id)?.ok_or_else(|| {
            CoreError::validation(
                "chat_message_not_found",
                "The chat message does not exist",
                operation,
            )
        })?;
        if object.object_type != "chat-message" {
            return Err(CoreError::validation(
                "chat_message_not_found",
                "The chat message does not exist",
                operation,
            ));
        }
        let path = resolve_for_write(&self.root, &object.relative_path, operation)?;
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, operation, Some(&object.relative_path)))?;
        parse_chat_message(&object.relative_path, &bytes)
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "The durable message append contract carries all canonical message fields without a second transient input type"
    )]
    fn append_chat_message(
        &self,
        chat_id: &str,
        expected_chat_revision: &str,
        kind: ChatMessageKind,
        status: ChatMessageStatus,
        mut content: String,
        run_id: String,
        provider_id: Option<String>,
        model_id: Option<String>,
        tool_call_id: Option<String>,
        tool_name: Option<String>,
        summarizes_through_message_id: Option<String>,
        event: &str,
        operation: &str,
    ) -> Result<MutationResult<ChatMessage>> {
        if matches!(
            &kind,
            ChatMessageKind::ToolCall | ChatMessageKind::ToolResult
        ) {
            content = crate::chat::canonical_json(&content, operation)?;
        }
        let mut chat = self.read_canonical_chat(chat_id, operation)?;
        if let Some(summary_id) = &summarizes_through_message_id {
            let summarized = self.read_canonical_chat_message(summary_id, operation)?;
            if summarized.chat_id != chat.id {
                return Err(CoreError::validation(
                    "chat_message_mismatch",
                    "The summarized message belongs to another chat",
                    operation,
                ));
            }
        }
        let now = now_rfc3339();
        let id = new_object_id("chat-message");
        let date = now.get(..10).ok_or_else(|| {
            CoreError::validation(
                "invalid_timestamp",
                "The current timestamp is invalid",
                operation,
            )
        })?;
        let parent = Path::new(&chat.relative_path).parent().ok_or_else(|| {
            CoreError::validation("invalid_chat_path", "The chat path is invalid", operation)
        })?;
        let relative_path = parent
            .join("messages")
            .join(date)
            .join(format!("{id}.md"))
            .to_str()
            .map(|value| value.replace(std::path::MAIN_SEPARATOR, "/"))
            .ok_or_else(|| {
                CoreError::validation(
                    "non_utf8_path",
                    "The chat message path is not UTF-8",
                    operation,
                )
            })?;
        let content_type = if matches!(
            &kind,
            ChatMessageKind::ToolCall | ChatMessageKind::ToolResult
        ) {
            "application/json".into()
        } else {
            "text/markdown".into()
        };
        let mut message = ChatMessage {
            id,
            chat_id: chat_id.into(),
            run_id,
            kind,
            status,
            content_type,
            content,
            relative_path,
            revision: String::new(),
            created: now.clone(),
            updated: now.clone(),
            provider_id,
            model_id,
            tool_call_id,
            tool_name,
            error_code: None,
            summarizes_through_message_id,
            properties: BTreeMap::new(),
        };
        crate::chat::validate_message_shape(&message, operation)?;
        let _guard = self.write_lock(operation)?;
        self.check_chat_revision_unlocked(&chat, expected_chat_revision, operation)?;
        chat.updated = now;
        let (chat_index_status, message_index_status, warnings) = self
            .commit_chat_mutation_unlocked(
                &mut chat,
                Some(expected_chat_revision),
                &mut message,
                None,
                operation,
            )?;
        let index_status = if matches!(message_index_status, IndexStatus::RepairPending)
            || matches!(chat_index_status, IndexStatus::RepairPending)
        {
            IndexStatus::RepairPending
        } else {
            IndexStatus::Updated
        };
        self.emit_chat_event(
            event,
            &chat,
            "application",
            serde_json::json!({
                "messageId": message.id,
                "messagePath": message.relative_path,
                "messageRevision": message.revision,
            }),
        );
        Ok(MutationResult {
            value: message.clone(),
            revision: message.revision,
            durability: "committed".into(),
            index_status,
            warnings,
        })
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "The durable message finish contract checks both canonical revisions and preserves explicit lifecycle context"
    )]
    fn finish_chat_message(
        &self,
        chat_id: &str,
        message_id: &str,
        expected_chat_revision: &str,
        expected_message_revision: &str,
        expected_kind: ChatMessageKind,
        content: String,
        status: ChatMessageStatus,
        error_code: Option<String>,
        event: &str,
        operation: &str,
    ) -> Result<MutationResult<ChatMessage>> {
        let mut chat = self.read_canonical_chat(chat_id, operation)?;
        let mut message = self.read_canonical_chat_message(message_id, operation)?;
        if message.chat_id != chat.id || message.kind != expected_kind {
            return Err(CoreError::validation(
                "chat_message_mismatch",
                "The chat message does not match this operation",
                operation,
            ));
        }
        if message.status != ChatMessageStatus::InProgress {
            return Err(CoreError::validation(
                "chat_message_not_in_progress",
                "Only in-progress chat messages can be finished",
                operation,
            ));
        }
        if !matches!(
            &status,
            ChatMessageStatus::Completed
                | ChatMessageStatus::Cancelled
                | ChatMessageStatus::Failed
                | ChatMessageStatus::Interrupted
        ) {
            return Err(CoreError::validation(
                "chat_message_not_terminal",
                "Finished chat messages require a terminal status",
                operation,
            ));
        }
        message.content = if matches!(
            &message.kind,
            ChatMessageKind::ToolCall | ChatMessageKind::ToolResult
        ) {
            crate::chat::canonical_json(&content, operation)?
        } else {
            content
        };
        message.status = status;
        message.error_code = error_code;
        crate::chat::validate_message_shape(&message, operation)?;
        let _guard = self.write_lock(operation)?;
        self.check_chat_revision_unlocked(&chat, expected_chat_revision, operation)?;
        self.check_chat_message_revision_unlocked(&message, expected_message_revision, operation)?;
        chat.updated = now_rfc3339();
        message.updated = now_rfc3339();
        let (chat_index_status, message_index_status, warnings) = self
            .commit_chat_mutation_unlocked(
                &mut chat,
                Some(expected_chat_revision),
                &mut message,
                Some(expected_message_revision),
                operation,
            )?;
        let index_status = if matches!(message_index_status, IndexStatus::RepairPending)
            || matches!(chat_index_status, IndexStatus::RepairPending)
        {
            IndexStatus::RepairPending
        } else {
            IndexStatus::Updated
        };
        self.emit_chat_event(
            event,
            &chat,
            "application",
            serde_json::json!({
                "messageId": message.id,
                "messagePath": message.relative_path,
                "messageRevision": message.revision,
            }),
        );
        Ok(MutationResult {
            value: message.clone(),
            revision: message.revision,
            durability: "committed".into(),
            index_status,
            warnings,
        })
    }

    /// Test hook for exercising recovery after either replacement in the
    /// multi-file chat commit. The fault is consumed before the selected write.
    #[doc(hidden)]
    pub fn fail_chat_mutation_write_for_testing(&self, target: &str, times: usize) {
        let target = match target {
            "chat" => ChatMutationWriteTarget::Chat,
            "message" => ChatMutationWriteTarget::Message,
            _ => panic!("unknown chat mutation write target: {target}"),
        };
        *self
            .chat_mutation_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            (times > 0).then_some(ChatMutationFault {
                target,
                remaining: times,
            });
    }

    /// Stage both canonical payloads in a durable workspace intent before
    /// replacing either file. A crash or write error leaves the intent as the
    /// sole authority for replaying the other replacement without guessing.
    fn commit_chat_mutation_unlocked(
        &self,
        chat: &mut Chat,
        expected_chat_revision: Option<&str>,
        message: &mut ChatMessage,
        expected_message_revision: Option<&str>,
        operation: &str,
    ) -> Result<(IndexStatus, IndexStatus, Vec<CoreWarning>)> {
        let chat_bytes = serialize_chat(chat)?;
        let message_bytes = serialize_chat_message(message)?;
        let intent = ChatMutationIntent {
            version: 1,
            chat: ChatMutationTarget {
                relative_path: chat.relative_path.clone(),
                expected_revision: expected_chat_revision.map(str::to_owned),
                bytes: String::from_utf8(chat_bytes.clone()).map_err(|_| {
                    CoreError::new(
                        "chat_mutation_serialize_failed",
                        ErrorCategory::Parse,
                        "Chat Markdown could not be staged for commit",
                        operation,
                    )
                })?,
            },
            message: ChatMutationTarget {
                relative_path: message.relative_path.clone(),
                expected_revision: expected_message_revision.map(str::to_owned),
                bytes: String::from_utf8(message_bytes.clone()).map_err(|_| {
                    CoreError::new(
                        "chat_mutation_serialize_failed",
                        ErrorCategory::Parse,
                        "Chat message Markdown could not be staged for commit",
                        operation,
                    )
                })?,
            },
        };
        self.validate_chat_mutation_intent(&intent, operation)?;
        let intent_path = self.write_chat_mutation_intent(&intent, operation)?;
        self.journal_chat_mutation(&intent);

        // Write the child first. Neither replacement is a successful mutation
        // until recovery has observed both recorded target revisions.
        self.apply_chat_mutation_target(
            &intent.message,
            ChatMutationWriteTarget::Message,
            true,
            operation,
        )?;
        self.apply_chat_mutation_target(
            &intent.chat,
            ChatMutationWriteTarget::Chat,
            true,
            operation,
        )?;
        let _ = self.clear_chat_mutation_intent(&intent_path, operation);

        chat.revision = markdown::revision(&chat_bytes);
        message.revision = markdown::revision(&message_bytes);
        let chat_parsed = ParsedMarkdown::Managed(chat.workspace_object());
        let chat_destination = resolve_for_write(&self.root, &chat.relative_path, operation)?;
        let chat_result = self
            .index
            .lock()
            .map_err(|_| lock_error(operation))
            .and_then(|mut index| {
                index.upsert_markdown(
                    &chat.relative_path,
                    &chat_bytes,
                    mtime_ns(&chat_destination),
                    &chat_parsed,
                )
            });
        let (chat_index_status, mut warnings) = self.index_outcome(chat_result);
        let message_parsed = ParsedMarkdown::Managed(message.workspace_object());
        let message_destination = resolve_for_write(&self.root, &message.relative_path, operation)?;
        let message_result = self
            .index
            .lock()
            .map_err(|_| lock_error(operation))
            .and_then(|mut index| {
                index.upsert_markdown(
                    &message.relative_path,
                    &message_bytes,
                    mtime_ns(&message_destination),
                    &message_parsed,
                )
            });
        let (message_index_status, message_warnings) = self.index_outcome(message_result);
        warnings.extend(message_warnings);
        Ok((chat_index_status, message_index_status, warnings))
    }

    fn recover_pending_chat_mutations(&self) -> Result<()> {
        let _guard = self.write_lock("chat_mutation_recover")?;
        let directory = resolve_for_write(&self.root, CHAT_MUTATION_DIR, "chat_mutation_recover")?;
        if !directory.exists() {
            return Ok(());
        }
        for entry in std::fs::read_dir(&directory)
            .map_err(|error| CoreError::io(error, "chat_mutation_recover", directory.to_str()))?
        {
            let entry = entry.map_err(|error| {
                CoreError::io(error, "chat_mutation_recover", directory.to_str())
            })?;
            if !entry
                .file_type()
                .map_err(|error| CoreError::io(error, "chat_mutation_recover", directory.to_str()))?
                .is_file()
                || entry.path().extension().and_then(|value| value.to_str()) != Some("json")
            {
                continue;
            }
            let path = entry.path();
            let bytes = std::fs::read(&path)
                .map_err(|error| CoreError::io(error, "chat_mutation_recover", path.to_str()))?;
            let intent = serde_json::from_slice::<ChatMutationIntent>(&bytes).map_err(|_| {
                CoreError::new(
                    "chat_mutation_recovery_invalid",
                    ErrorCategory::Parse,
                    "A pending chat mutation intent is invalid",
                    "chat_mutation_recover",
                )
            })?;
            self.validate_chat_mutation_intent(&intent, "chat_mutation_recover")?;
            self.apply_chat_mutation_target(
                &intent.message,
                ChatMutationWriteTarget::Message,
                false,
                "chat_mutation_recover",
            )?;
            self.apply_chat_mutation_target(
                &intent.chat,
                ChatMutationWriteTarget::Chat,
                false,
                "chat_mutation_recover",
            )?;
            let _ = self.clear_chat_mutation_intent(&path, "chat_mutation_recover");
        }
        Ok(())
    }

    fn validate_chat_mutation_intent(
        &self,
        intent: &ChatMutationIntent,
        operation: &str,
    ) -> Result<()> {
        if intent.version != 1 {
            return Err(CoreError::new(
                "chat_mutation_recovery_invalid",
                ErrorCategory::Parse,
                "A pending chat mutation intent has an unsupported version",
                operation,
            ));
        }
        crate::path::validate_relative(&intent.chat.relative_path, operation)?;
        crate::path::validate_relative(&intent.message.relative_path, operation)?;
        let chat = parse_chat(&intent.chat.relative_path, intent.chat.bytes.as_bytes())?;
        let message = parse_chat_message(
            &intent.message.relative_path,
            intent.message.bytes.as_bytes(),
        )?;
        if message.chat_id != chat.id {
            return Err(CoreError::new(
                "chat_mutation_recovery_invalid",
                ErrorCategory::Parse,
                "A pending chat mutation does not join its chat and message",
                operation,
            ));
        }
        Ok(())
    }

    fn write_chat_mutation_intent(
        &self,
        intent: &ChatMutationIntent,
        operation: &str,
    ) -> Result<PathBuf> {
        let relative =
            PathBuf::from(CHAT_MUTATION_DIR).join(format!("{}.json", uuid::Uuid::new_v4()));
        let bytes = serde_json::to_vec(intent).map_err(|_| {
            CoreError::new(
                "chat_mutation_serialize_failed",
                ErrorCategory::Parse,
                "Chat mutation intent could not be serialized",
                operation,
            )
        })?;
        atomic_write(&self.root, &relative, &bytes, operation)?;
        Ok(self.root.join(relative))
    }

    fn apply_chat_mutation_target(
        &self,
        target: &ChatMutationTarget,
        write_target: ChatMutationWriteTarget,
        inject_fault: bool,
        operation: &str,
    ) -> Result<()> {
        let relative = crate::path::validate_relative(&target.relative_path, operation)?;
        let destination = resolve_for_write(&self.root, &target.relative_path, operation)?;
        let target_revision = markdown::revision(target.bytes.as_bytes());
        match std::fs::read(&destination) {
            Ok(current) if markdown::revision(&current) == target_revision => return Ok(()),
            Ok(current)
                if target
                    .expected_revision
                    .as_deref()
                    .is_some_and(|expected| markdown::revision(&current) == expected) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::NotFound
                    && target.expected_revision.is_none() => {}
            Ok(_) | Err(_) => {
                return Err(CoreError::new(
                    "chat_mutation_recovery_conflict",
                    ErrorCategory::Conflict,
                    "A pending chat mutation would overwrite an external file change",
                    operation,
                ));
            }
        }
        if inject_fault && self.consume_chat_mutation_fault(write_target) {
            return Err(CoreError::new(
                "chat_mutation_write_failed",
                ErrorCategory::Filesystem,
                "A test fault interrupted the chat mutation before this file was written",
                operation,
            ));
        }
        atomic_write_checked(
            &self.root,
            &relative,
            target.bytes.as_bytes(),
            target.expected_revision.as_deref(),
            operation,
        )
    }

    fn journal_chat_mutation(&self, intent: &ChatMutationIntent) {
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(
                intent.chat.relative_path.clone(),
                markdown::revision(intent.chat.bytes.as_bytes()),
            );
            journal.insert(
                intent.message.relative_path.clone(),
                markdown::revision(intent.message.bytes.as_bytes()),
            );
        }
    }

    fn clear_chat_mutation_intent(&self, path: &Path, operation: &str) -> Result<()> {
        std::fs::remove_file(path)
            .map_err(|error| CoreError::io(error, operation, path.to_str()))?;
        sync_parent(path, operation)
    }

    fn consume_chat_mutation_fault(&self, target: ChatMutationWriteTarget) -> bool {
        let mut fault = self
            .chat_mutation_fault
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(current) = fault.as_mut() else {
            return false;
        };
        if current.target != target {
            return false;
        }
        current.remaining -= 1;
        if current.remaining == 0 {
            *fault = None;
        }
        true
    }

    fn write_chat_unlocked(
        &self,
        chat: &mut Chat,
        expected_revision: Option<&str>,
        operation: &str,
    ) -> Result<(IndexStatus, Vec<CoreWarning>)> {
        let relative = crate::path::validate_relative(&chat.relative_path, operation)?;
        let destination = resolve_for_write(&self.root, &chat.relative_path, operation)?;
        if let Some(expected) = expected_revision {
            let current = std::fs::read(&destination)
                .map_err(|error| CoreError::io(error, operation, Some(&chat.relative_path)))?;
            check_revision(&current, expected, operation)?;
        } else if destination.exists() {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A file already exists at the requested path",
                operation,
            ));
        }
        let bytes = serialize_chat(chat)?;
        let revision = markdown::revision(&bytes);
        atomic_write_checked(&self.root, &relative, &bytes, expected_revision, operation)?;
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(chat.relative_path.clone(), revision.clone());
        }
        chat.revision = revision;
        let parsed = ParsedMarkdown::Managed(chat.workspace_object());
        let result = self
            .index
            .lock()
            .map_err(|_| lock_error(operation))
            .and_then(|mut index| {
                index.upsert_markdown(&chat.relative_path, &bytes, mtime_ns(&destination), &parsed)
            });
        Ok(self.index_outcome(result))
    }

    fn check_chat_revision_unlocked(
        &self,
        chat: &Chat,
        expected_revision: &str,
        operation: &str,
    ) -> Result<()> {
        let path = resolve_for_write(&self.root, &chat.relative_path, operation)?;
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, operation, Some(&chat.relative_path)))?;
        check_revision(&bytes, expected_revision, operation)
    }

    fn check_chat_message_revision_unlocked(
        &self,
        message: &ChatMessage,
        expected_revision: &str,
        operation: &str,
    ) -> Result<()> {
        let path = resolve_for_write(&self.root, &message.relative_path, operation)?;
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, operation, Some(&message.relative_path)))?;
        check_revision(&bytes, expected_revision, operation)
    }

    fn expire_chat_directory(&self, chat: &Chat, expires_at: jiff::Timestamp) -> Result<bool> {
        let relative = Path::new(&chat.relative_path);
        let Some(directory) = relative.parent() else {
            return Ok(false);
        };
        if relative.file_name().and_then(|value| value.to_str()) != Some("chat.md")
            || directory
                .parent()
                .and_then(|value| value.file_name())
                .and_then(|value| value.to_str())
                != Some("chats")
        {
            return Ok(false);
        }
        let source = resolve_for_write(&self.root, &directory.to_string_lossy(), "chat_expire")?;
        let mut files = Vec::new();
        if collect_expiring_chat_files(&source, &mut files, "chat_expire").is_err() {
            return Ok(false);
        }
        if files.is_empty() || !files.iter().any(|path| path == &source.join("chat.md")) {
            return Ok(false);
        }
        for path in &files {
            let relative = match path.strip_prefix(&self.root).ok().and_then(Path::to_str) {
                Some(relative) => relative.replace('\\', "/"),
                None => return Ok(false),
            };
            let bytes = match std::fs::read(path) {
                Ok(bytes) => bytes,
                Err(_) => return Ok(false),
            };
            if path.file_name().and_then(|value| value.to_str()) == Some("chat.md") {
                if parse_chat(&relative, &bytes)
                    .ok()
                    .as_ref()
                    .map(|value| &value.id)
                    != Some(&chat.id)
                {
                    return Ok(false);
                }
            } else {
                let Some(message) = parse_chat_message(&relative, &bytes).ok() else {
                    return Ok(false);
                };
                if message.chat_id != chat.id
                    || path.file_name().and_then(|value| value.to_str())
                        != Some(format!("{}.md", message.id).as_str())
                {
                    return Ok(false);
                }
            }
            let modified = path
                .metadata()
                .and_then(|metadata| metadata.modified())
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| {
                    jiff::Timestamp::new(duration.as_secs() as i64, duration.subsec_nanos() as i32)
                })
                .transpose()
                .map_err(|_| {
                    CoreError::validation(
                        "invalid_timestamp",
                        "A chat file timestamp is invalid",
                        "chat_expire",
                    )
                })?;
            if modified.is_some_and(|modified| modified > expires_at) {
                return Ok(false);
            }
        }
        let timestamp = now_rfc3339().replace([':', '.'], "-");
        let trash_relative = Path::new(".noura/trash").join(timestamp).join(directory);
        let trash =
            resolve_for_write(&self.root, &trash_relative.to_string_lossy(), "chat_expire")?;
        if trash.exists() {
            return Ok(false);
        }
        if let Some(parent) = trash.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CoreError::io(error, "chat_expire", parent.to_str()))?;
        }
        std::fs::rename(&source, &trash)
            .map_err(|error| CoreError::io(error, "chat_expire", source.to_str()))?;
        sync_rename_parents(&source, &trash, "chat_expire")?;
        if let Ok(mut journal) = self.self_writes.lock() {
            for file in files {
                if let Some(relative) = file.strip_prefix(&self.root).ok().and_then(Path::to_str) {
                    journal.insert(relative.replace('\\', "/"), "<deleted>".into());
                }
            }
        }
        Ok(true)
    }

    fn emit_chat_event(
        &self,
        event_type: &str,
        chat: &Chat,
        source: &str,
        payload: serde_json::Value,
    ) {
        self.emit(
            event_type,
            source,
            serde_json::json!({
                "id": chat.id,
                "path": chat.relative_path,
                "revision": chat.revision,
                "payload": payload,
            }),
        );
    }

    fn write_lock(&self, operation: &str) -> Result<WorkspaceLock> {
        if let Some(parent) = self.lock_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CoreError::io(error, operation, parent.to_str()))?;
        }
        let file = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&self.lock_path)
            .map_err(|error| CoreError::io(error, operation, self.lock_path.to_str()))?;
        file.lock_exclusive()
            .map_err(|error| CoreError::io(error, operation, self.lock_path.to_str()))?;
        Ok(WorkspaceLock(file))
    }

    fn read_canonical_object(
        &self,
        id: &str,
        operation: &str,
    ) -> Result<(WorkspaceObject, Vec<u8>)> {
        let object_type = id.split('_').next().unwrap_or_default();
        if object_type.is_empty() || !valid_object_id(id, object_type) {
            return Err(CoreError::validation(
                "invalid_object_id",
                "The object ID is invalid",
                operation,
            ));
        }
        let mut forced = std::collections::HashSet::new();
        if let Some(indexed) = self.get_object(id)? {
            forced.insert(indexed.relative_path);
        }
        self.reconcile_forced(&forced)?;
        let indexed = self.get_object(id)?.ok_or_else(|| {
            CoreError::validation("object_not_found", "The object does not exist", operation)
        })?;
        let destination = resolve_for_write(&self.root, &indexed.relative_path, operation)?;
        let bytes = std::fs::read(&destination)
            .map_err(|error| CoreError::io(error, operation, Some(&indexed.relative_path)))?;
        let ParsedMarkdown::Managed(object) =
            markdown::parse_markdown(&indexed.relative_path, &bytes)
        else {
            return Err(CoreError::new(
                "object_parse_failed",
                ErrorCategory::Parse,
                "The canonical Markdown file cannot be reconciled safely",
                operation,
            ));
        };
        if object.id != id {
            return Err(CoreError::new(
                "object_identity_changed",
                ErrorCategory::Identity,
                "The canonical file no longer has the expected stable ID",
                operation,
            ));
        }
        Ok((object, bytes))
    }

    fn snapshot_bytes(&self, id: &str, kind: &str, bytes: &[u8]) -> Result<()> {
        let object_type = id.split('_').next().unwrap_or_default();
        if object_type.is_empty()
            || !valid_object_id(id, object_type)
            || !matches!(kind, "local" | "external")
        {
            return Err(CoreError::validation(
                "invalid_history_target",
                "The recovery snapshot target is invalid",
                "history_snapshot",
            ));
        }
        let revision = markdown::revision(bytes);
        let relative = PathBuf::from(".noura")
            .join("history")
            .join(id)
            .join(format!("{revision}-{kind}.md"));
        let relative_text = relative.to_str().ok_or_else(|| {
            CoreError::validation(
                "non_utf8_path",
                "The recovery snapshot path is not UTF-8",
                "history_snapshot",
            )
        })?;
        let destination = resolve_for_write(&self.root, relative_text, "history_snapshot")?;
        if destination.exists() {
            return Ok(());
        }
        atomic_write(&self.root, &relative, bytes, "history_snapshot")
    }
    fn emit(&self, event_type: &str, source: &str, payload: serde_json::Value) {
        let _ = self.event_sender.send(CoreEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: event_type.into(),
            workspace_id: self.current_workspace_id(),
            occurred_at: now_rfc3339(),
            source: source.into(),
            payload,
        });
    }

    fn emit_reconciled_object_events(
        &self,
        before: &HashMap<String, WorkspaceObject>,
        after: &HashMap<String, WorkspaceObject>,
        source: &str,
    ) {
        for (id, object) in after {
            let Some(previous) = before.get(id) else {
                self.emit_object_event("object:created", object, source, None);
                continue;
            };
            if previous.relative_path != object.relative_path {
                self.emit_object_event(
                    "object:moved",
                    object,
                    source,
                    Some(&previous.relative_path),
                );
            } else if previous.revision != object.revision {
                self.emit_object_event("object:updated", object, source, None);
            }
        }
        for (id, object) in before {
            if !after.contains_key(id) {
                self.emit_object_event("object:deleted", object, source, None);
            }
        }
    }

    fn emit_object_event(
        &self,
        event_type: &str,
        object: &WorkspaceObject,
        source: &str,
        previous_path: Option<&str>,
    ) {
        self.emit(
            event_type,
            source,
            serde_json::json!({
                "id": object.id,
                "type": object.object_type,
                "path": object.relative_path,
                "previousPath": previous_path,
                "revision": object.revision,
            }),
        );
    }

    fn index_outcome(&self, result: Result<()>) -> (IndexStatus, Vec<CoreWarning>) {
        match result {
            Ok(()) => (IndexStatus::Updated, Vec::new()),
            Err(_) => {
                self.emit(
                    "workspace:index-stale",
                    "application",
                    serde_json::json!({ "repairScheduled": true }),
                );
                (
                    IndexStatus::RepairPending,
                    vec![CoreWarning {
                        code: "index_repair_pending".into(),
                        message: "The file was committed, but the local index needs reconciliation"
                            .into(),
                    }],
                )
            }
        }
    }
}

struct WorkspaceLock(File);
impl Drop for WorkspaceLock {
    fn drop(&mut self) {
        let _ = self.0.unlock();
    }
}

fn scan_into(root: &Path, ignore_patterns: &[String], index: &mut IndexStore) -> Result<()> {
    let scan = scan_changes(
        root,
        ignore_patterns,
        &HashMap::new(),
        &std::collections::HashSet::new(),
    )?;
    index.replace_markdown(&scan.changed)
}

fn scan_changes(
    root: &Path,
    ignore_patterns: &[String],
    indexed: &HashMap<String, (i64, i64)>,
    forced: &std::collections::HashSet<String>,
) -> Result<WorkspaceScan> {
    let walker = workspace_walker(root, ignore_patterns)?;
    let mut changed = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for entry in walker {
        let entry = entry.map_err(|error| {
            CoreError::new(
                "scan_error",
                ErrorCategory::Filesystem,
                error.to_string(),
                "workspace_scan",
            )
        })?;
        if !entry.file_type().is_some_and(|kind| kind.is_file())
            || entry.path().extension().and_then(|value| value.to_str()) != Some("md")
        {
            continue;
        }
        let relative = normalized_relative_path(root, entry.path(), "workspace_scan")?;
        seen.insert(relative.clone());
        let size = entry
            .metadata()
            .map_err(|error| {
                CoreError::new(
                    "scan_error",
                    ErrorCategory::Filesystem,
                    error.to_string(),
                    "workspace_scan",
                )
            })?
            .len()
            .min(i64::MAX as u64) as i64;
        let modified = mtime_ns(entry.path());
        if !forced.contains(&relative) && indexed.get(&relative) == Some(&(size, modified)) {
            continue;
        }
        let bytes = std::fs::read(entry.path())
            .map_err(|error| CoreError::io(error, "workspace_scan", Some(&relative)))?;
        let parsed = markdown::parse_markdown(&relative, &bytes);
        changed.push((relative, bytes, modified, parsed));
    }
    Ok(WorkspaceScan { changed, seen })
}

fn compile_workspace_ignores(
    root: &Path,
    patterns: &[String],
) -> Result<ignore::gitignore::Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    for pattern in patterns {
        builder.add_line(None, pattern).map_err(|error| {
            CoreError::validation(
                "invalid_ignore_pattern",
                format!("Workspace ignore pattern is invalid: {error}"),
                "workspace_open",
            )
        })?;
    }
    builder.build().map_err(|error| {
        CoreError::validation(
            "invalid_ignore_pattern",
            format!("Workspace ignore patterns are invalid: {error}"),
            "workspace_open",
        )
    })
}

fn workspace_walker(root: &Path, ignore_patterns: &[String]) -> Result<ignore::Walk> {
    let ignores = compile_workspace_ignores(root, ignore_patterns)?;
    let filter_root = root.to_owned();
    Ok(WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .filter_entry(move |entry| {
            let relative = entry
                .path()
                .strip_prefix(&filter_root)
                .unwrap_or(entry.path());
            is_visible_workspace_path(
                relative,
                entry.file_type().is_some_and(|kind| kind.is_dir()),
                &ignores,
            )
        })
        .build())
}

fn is_visible_workspace_path(
    relative: &Path,
    is_directory: bool,
    ignores: &ignore::gitignore::Gitignore,
) -> bool {
    if relative.as_os_str().is_empty() {
        return true;
    }
    if relative == Path::new("workspace.yaml")
        || relative.starts_with(".noura")
        || relative.starts_with(".git")
        || relative.starts_with("node_modules")
        || relative.starts_with("target")
    {
        return false;
    }
    !ignores
        .matched_path_or_any_parents(relative, is_directory)
        .is_ignore()
}

fn normalized_relative_path(root: &Path, path: &Path, operation: &str) -> Result<String> {
    path.strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| {
            CoreError::validation(
                "non_utf8_path",
                "The workspace contains a path that is not UTF-8",
                operation,
            )
        })
}
fn atomic_write(root: &Path, relative: &Path, bytes: &[u8], operation: &str) -> Result<()> {
    atomic_write_checked(root, relative, bytes, None, operation)
}

fn atomic_write_checked(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    expected_revision: Option<&str>,
    operation: &str,
) -> Result<()> {
    let destination = root.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    }
    let mut file = atomic_write_file::AtomicWriteFile::open(&destination)
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    if let Some(expected) = expected_revision {
        let current = std::fs::read(&destination)
            .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
        check_revision(&current, expected, operation)?;
    }
    file.commit()
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    sync_parent(&destination, operation)?;
    Ok(())
}

fn markdown_requires_manual_review(body: &str) -> bool {
    body.lines().any(|line| {
        let trimmed = line.trim_start();
        trimmed.starts_with("<")
            || trimmed.starts_with(":::")
            || (trimmed.starts_with('[') && trimmed.contains("]:"))
    })
}

fn merge_markdown_body(base: &str, local: &str, external: &str) -> Option<String> {
    if [base, local, external]
        .into_iter()
        .any(markdown_requires_manual_review)
    {
        return None;
    }
    diffy::merge(base, local, external).ok()
}

fn merge_markdown_text(base: &str, local: &str, external: &str) -> Option<String> {
    diffy::merge(base, local, external).ok()
}

/// Merge a managed draft field-by-field against the canonical file. Body text
/// merges line-by-line; title and every top-level property merge
/// independently. Differences to the same field from both sides conflict.
fn merge_managed_fields(
    base: &ManagedDraftInput,
    canonical: &WorkspaceObject,
) -> Result<WorkspaceObject> {
    let merged_body = merge_markdown_text(&base.base_body, &base.local_body, &canonical.body)
        .ok_or_else(|| {
            CoreError::new(
                "draft_conflict",
                ErrorCategory::Conflict,
                "The body changed on both sides and requires manual review",
                "managed_draft_merge",
            )
        })?;
    let merged_title = if base.local_title != base.base_title {
        base.local_title.clone()
    } else {
        canonical.title.clone()
    };
    let mut merged_properties = canonical.properties.clone();
    for (key, local_value) in &base.local_properties {
        let external_value = canonical.properties.get(key);
        let base_value = base.base_properties.get(key);
        let locally_changed = Some(local_value) != base_value;
        let externally_changed = external_value != base_value;
        if locally_changed && externally_changed && external_value != Some(local_value) {
            return Err(CoreError::new(
                "draft_conflict",
                ErrorCategory::Conflict,
                "The same property changed on both sides and requires manual review",
                "managed_draft_merge",
            ));
        }
        if locally_changed {
            merged_properties.insert(key.clone(), local_value.clone());
        } else if externally_changed {
            merged_properties.insert(
                key.clone(),
                external_value.cloned().unwrap_or(serde_json::Value::Null),
            );
        }
    }
    for key in base.base_properties.keys() {
        if !base.local_properties.contains_key(key)
            && !canonical.properties.contains_key(key)
            && !matches!(key.as_str(), "id" | "type" | "created" | "updated")
        {
            merged_properties.remove(key);
        }
    }
    Ok(WorkspaceObject {
        id: canonical.id.clone(),
        object_type: canonical.object_type.clone(),
        title: merged_title,
        body: merged_body,
        relative_path: canonical.relative_path.clone(),
        revision: canonical.revision.clone(),
        created: canonical.created.clone(),
        updated: canonical.updated.clone(),
        properties: merged_properties,
    })
}

fn normalized_properties(
    properties: &BTreeMap<String, serde_json::Value>,
) -> BTreeMap<String, serde_json::Value> {
    properties
        .iter()
        .filter(|(key, _)| !matches!(key.as_str(), "id" | "type" | "created" | "updated"))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn compose_raw_bytes(body: &str, uses_crlf: bool, has_bom: bool) -> Vec<u8> {
    let text = if uses_crlf {
        body.replace('\n', "\r\n")
    } else {
        body.to_owned()
    };
    let mut bytes = if has_bom {
        b"\xEF\xBB\xBF".to_vec()
    } else {
        Vec::new()
    };
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

fn split_raw_bytes(bytes: &[u8]) -> Result<(String, bool, bool)> {
    let (has_bom, text_bytes) = if bytes.starts_with(b"\xEF\xBB\xBF") {
        (true, &bytes[3..])
    } else {
        (false, bytes)
    };
    let text = std::str::from_utf8(text_bytes).map_err(|_| {
        CoreError::new(
            "invalid_utf8",
            ErrorCategory::Parse,
            "The raw Markdown file is not UTF-8",
            "raw_markdown_read",
        )
    })?;
    let uses_crlf = text.contains("\r\n");
    let body = text.replace("\r\n", "\n");
    Ok((body, uses_crlf, has_bom))
}

fn validate_raw_markdown_path(root: &Path, relative: &str) -> Result<PathBuf> {
    if !relative.to_ascii_lowercase().ends_with(".md") {
        return Err(CoreError::validation(
            "invalid_raw_markdown_path",
            "Raw edits are limited to Markdown files",
            "raw_markdown",
        ));
    }
    resolve_for_write(root, relative, "raw_markdown")
}

/// Strip the alias (`|`) and — when fragments are meaningful — the heading
/// fragment (`#`) from an Obsidian-style target, leaving the resolvable
/// path. An empty result means the target named only an alias or fragment.
fn markdown_target_path(target: &str, allow_fragment: bool) -> &str {
    let path = target.split('|').next().unwrap_or_default().trim();
    if allow_fragment {
        path.split('#').next().unwrap_or_default()
    } else {
        path
    }
}

fn resolve_markdown_target(
    source_relative_path: &str,
    target: &str,
    allow_fragment: bool,
) -> Result<String> {
    let target = markdown_target_path(target, allow_fragment);
    if target.is_empty() || target.contains('\0') {
        return Err(CoreError::validation(
            "invalid_markdown_target",
            "The Markdown target is empty or invalid",
            "markdown_target_resolve",
        ));
    }
    let source = crate::path::validate_relative(source_relative_path, "markdown_target_resolve")?;
    let parent = source.parent().unwrap_or_else(|| Path::new(""));
    let joined = parent.join(target);
    let mut normalized = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::Normal(value) => normalized.push(value),
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                if !normalized.pop() {
                    return Err(CoreError::validation(
                        "path_traversal",
                        "The Markdown target escapes the workspace",
                        "markdown_target_resolve",
                    ));
                }
            }
            _ => {
                return Err(CoreError::validation(
                    "invalid_markdown_target",
                    "The Markdown target must be a relative workspace path",
                    "markdown_target_resolve",
                ));
            }
        }
    }
    normalized
        .to_str()
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| {
            CoreError::validation(
                "non_utf8_path",
                "The Markdown target path is not UTF-8",
                "markdown_target_resolve",
            )
        })
}

fn raw_history_dir(relative: &str) -> String {
    let digest = blake3::hash(relative.as_bytes()).to_hex();
    format!("raw-{}", &digest[..16])
}

fn write_snapshot(
    root: &Path,
    operation: &str,
    segment: &str,
    kind: &str,
    bytes: &[u8],
) -> Result<()> {
    let revision = markdown::revision(bytes);
    let relative_path = PathBuf::from(".noura")
        .join("history")
        .join(segment)
        .join(format!("{revision}-{kind}.md"));
    let relative = relative_path.to_str().ok_or_else(|| {
        CoreError::validation(
            "non_utf8_path",
            "The recovery snapshot path is not UTF-8",
            "history_snapshot",
        )
    })?;
    let destination = resolve_for_write(root, relative, operation)?;
    if destination.exists() {
        return Ok(());
    }
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    }
    atomic_write(root, &relative_path, bytes, operation)
}

fn validate_tool_fields(tool_call_id: &str, tool_name: &str, operation: &str) -> Result<()> {
    if tool_call_id.trim().is_empty()
        || tool_name.trim().is_empty()
        || tool_call_id.chars().any(char::is_control)
        || tool_name.chars().any(char::is_control)
    {
        return Err(CoreError::validation(
            "invalid_tool_message",
            "Tool messages require a tool call ID and tool name",
            operation,
        ));
    }
    Ok(())
}

fn parse_chat_timestamp(value: &str, operation: &str) -> Result<jiff::Timestamp> {
    value.parse::<jiff::Timestamp>().map_err(|_| {
        CoreError::validation(
            "invalid_timestamp",
            "Chat timestamps must be RFC 3339 timestamps",
            operation,
        )
    })
}

fn tool_result_completes_call(result: &ChatMessage, call: &ChatMessage) -> bool {
    call.kind == ChatMessageKind::ToolCall
        && result.run_id == call.run_id
        && result.tool_call_id == call.tool_call_id
        && result.tool_name == call.tool_name
}

/// Retention only moves a directory when it contains the exact canonical chat
/// layout. Unknown files, symlinks, malformed messages, and hand-created
/// folders make the chat ineligible rather than risking unrelated data.
fn collect_expiring_chat_files(
    path: &Path,
    files: &mut Vec<PathBuf>,
    operation: &str,
) -> Result<()> {
    let entries =
        std::fs::read_dir(path).map_err(|error| CoreError::io(error, operation, path.to_str()))?;
    for entry in entries {
        let entry = entry.map_err(|error| CoreError::io(error, operation, path.to_str()))?;
        let file_type = entry
            .file_type()
            .map_err(|error| CoreError::io(error, operation, entry.path().to_str()))?;
        let child = entry.path();
        if file_type.is_symlink() {
            return Err(CoreError::validation(
                "symlink_escape",
                "Chat retention cannot traverse symlinks",
                operation,
            ));
        }
        if file_type.is_file() {
            if child.file_name().and_then(|value| value.to_str()) == Some("chat.md")
                || child
                    .file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|name| name.starts_with("chat-message_") && name.ends_with(".md"))
            {
                files.push(child);
                continue;
            }
            return Err(CoreError::validation(
                "unsafe_chat_expiry",
                "Chat retention found an unexpected file",
                operation,
            ));
        }
        if file_type.is_dir() {
            let name = child
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if name == "messages"
                || (name.len() == 10
                    && name.as_bytes().get(4) == Some(&b'-')
                    && name.as_bytes().get(7) == Some(&b'-'))
            {
                collect_expiring_chat_files(&child, files, operation)?;
                continue;
            }
        }
        return Err(CoreError::validation(
            "unsafe_chat_expiry",
            "Chat retention found an unexpected path",
            operation,
        ));
    }
    Ok(())
}

#[cfg(unix)]
fn sync_parent(path: &Path, operation: &str) -> Result<()> {
    let parent = path.parent().unwrap_or(path);
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| CoreError::io(error, operation, parent.to_str()))
}

#[cfg(not(unix))]
fn sync_parent(_path: &Path, _operation: &str) -> Result<()> {
    Ok(())
}

fn sync_rename_parents(source: &Path, destination: &Path, operation: &str) -> Result<()> {
    sync_parent(source, operation)?;
    if source.parent() != destination.parent() {
        sync_parent(destination, operation)?;
    }
    Ok(())
}
fn mtime_ns(path: &Path) -> i64 {
    path.metadata()
        .and_then(|value| value.modified())
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

fn validate_chat_title<'a>(title: &'a str, operation: &str) -> Result<&'a str> {
    let trimmed = title.trim();
    if trimmed.is_empty() || title.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "chat_title_required",
            "A chat title without control characters is required",
            operation,
        ));
    }
    Ok(trimmed)
}

fn check_revision(bytes: &[u8], expected: &str, operation: &str) -> Result<()> {
    let current = markdown::revision(bytes);
    if current != expected {
        let mut error = CoreError::new(
            "revision_conflict",
            ErrorCategory::Conflict,
            "The file changed since it was loaded",
            operation,
        );
        error.details = Some(serde_json::json!({"currentRevision":current}));
        return Err(error);
    }
    Ok(())
}
fn validate_manifest(manifest: &WorkspaceManifest, operation: &str) -> Result<()> {
    if !valid_object_id(&manifest.id, "workspace") {
        return Err(CoreError::validation(
            "invalid_workspace_id",
            "The workspace ID must be a lowercase stable workspace ID",
            operation,
        ));
    }
    if manifest.name.trim().is_empty() {
        return Err(CoreError::validation(
            "workspace_name_required",
            "A workspace name is required",
            operation,
        ));
    }
    for id in &manifest.enabled_plugins {
        validate_plugin_id(id, operation)?;
    }
    compile_workspace_ignores(Path::new("."), &manifest.ignore)?;
    Ok(())
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

fn parse_workspace_manifest(bytes: &[u8], operation: &str) -> Result<WorkspaceManifest> {
    let mut manifest: WorkspaceManifest = serde_yaml_ng::from_slice(bytes).map_err(|_| {
        CoreError::new(
            "invalid_workspace_manifest",
            ErrorCategory::Parse,
            "workspace.yaml is invalid",
            operation,
        )
    })?;
    validate_manifest(&manifest, operation)?;
    if manifest.format_version != 1 {
        return Err(CoreError::validation(
            "unsupported_workspace_version",
            "This workspace format version is not supported",
            operation,
        ));
    }
    // `enabled_plugins` is deduplicated with insignificant order in the
    // format; readers canonicalize so consumers never observe a raw hand
    // edit's duplicates, matching the writer's normalization.
    manifest.enabled_plugins.sort();
    manifest.enabled_plugins.dedup();
    Ok(manifest)
}

fn take_optional_timestamp(
    properties: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
    operation: &str,
) -> Result<Option<String>> {
    let Some(value) = properties.remove(key) else {
        return Ok(None);
    };
    let value = value.as_str().ok_or_else(|| {
        CoreError::validation(
            "invalid_timestamp",
            format!("{key} must be an RFC 3339 timestamp"),
            operation,
        )
    })?;
    ensure_rfc3339_timestamp(
        value,
        format!("{key} must be an RFC 3339 timestamp"),
        operation,
    )?;
    Ok(Some(value.to_owned()))
}

/// Client-supplied durable timestamps keep working files consistent; a
/// malformed value must fail the mutation instead of becoming frontmatter.
fn ensure_rfc3339_timestamp(value: &str, message: String, operation: &str) -> Result<()> {
    value
        .parse::<jiff::Timestamp>()
        .map(|_| ())
        .map_err(|_| CoreError::validation("invalid_timestamp", message, operation))
}

fn validate_object_type(value: &str) -> Result<()> {
    if !valid_object_type(value) {
        return Err(CoreError::validation(
            "invalid_object_type",
            "Object types use lowercase letters and hyphens",
            "object_validate",
        ));
    }
    Ok(())
}
fn lock_error(operation: &str) -> CoreError {
    CoreError::new(
        "lock_poisoned",
        ErrorCategory::Transient,
        "A local workspace lock is unavailable",
        operation,
    )
}
fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Untitled")
        .replace(['-', '_'], " ")
}
fn default_object_path(object_type: &str, title: &str, id: &str) -> String {
    let value = slug::slugify(title);
    let slug = if value.is_empty() { "untitled" } else { &value };
    let short = &id[id.len().saturating_sub(6)..];
    if object_type == "project" {
        format!("projects/{slug}--{short}/project.md")
    } else {
        format!("{object_type}s/{slug}--{short}.md")
    }
}
fn normalize_domain_properties(
    object_type: &str,
    properties: &mut BTreeMap<String, serde_json::Value>,
) -> Result<()> {
    if object_type == "task" {
        properties
            .entry("status".into())
            .or_insert_with(|| serde_json::json!("todo"));
        properties
            .entry("priority".into())
            .or_insert_with(|| serde_json::json!("medium"));
        validate_enum(
            properties,
            "status",
            &["todo", "in-progress", "done", "cancelled"],
        )?;
        validate_enum(properties, "priority", &["low", "medium", "high", "urgent"])?;
        if let Some(value) = properties.get("due").and_then(serde_json::Value::as_str) {
            validate_date_value(value, "due")?;
        }
        if let Some(project) = properties
            .get("project")
            .and_then(serde_json::Value::as_str)
            && !crate::valid_object_id(project, "project")
        {
            return Err(CoreError::validation(
                "invalid_project_id",
                "Task project references use a stable project ID",
                "object_validate",
            ));
        }
    } else if object_type == "project" {
        properties
            .entry("status".into())
            .or_insert_with(|| serde_json::json!("planned"));
        validate_enum(
            properties,
            "status",
            &["planned", "active", "on-hold", "completed", "cancelled"],
        )?;
    }
    Ok(())
}
fn validate_enum(
    properties: &BTreeMap<String, serde_json::Value>,
    field: &str,
    values: &[&str],
) -> Result<()> {
    let value = properties
        .get(field)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            CoreError::validation(
                "invalid_field",
                format!("{field} must be a string"),
                "object_validate",
            )
        })?;
    if !values.contains(&value) {
        return Err(CoreError::validation(
            "invalid_field",
            format!("Unsupported {field} value"),
            "object_validate",
        ));
    }
    Ok(())
}
fn validate_date_value(value: &str, field: &str) -> Result<()> {
    let date_only = value.len() == 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && value.parse::<jiff::civil::Date>().is_ok();
    let timed = (value.ends_with('Z')
        || value
            .as_bytes()
            .iter()
            .skip(10)
            .any(|byte| matches!(byte, b'+' | b'-')))
        && value.parse::<jiff::Timestamp>().is_ok();
    if !date_only && !timed {
        return Err(CoreError::validation(
            "invalid_date",
            format!("{field} must use YYYY-MM-DD or RFC 3339 with an explicit offset"),
            "object_validate",
        ));
    }
    Ok(())
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
            "../../../docs/workspace-format/fixtures/conformance-v1.json"
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
    fn full_storage_lifecycle_survives_index_deletion() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let project = engine
            .create_object(CreateObjectInput {
                object_type: "project".into(),
                title: "Alpha proof project".into(),
                body: "projectquartz durable body".into(),
                relative_path: Some("projects/alpha/project.md".into()),
                properties: BTreeMap::from([
                    ("status".into(), serde_json::json!("active")),
                    ("start".into(), serde_json::json!("2026-09-01")),
                    ("end".into(), serde_json::json!("2026-09-04")),
                ]),
            })
            .unwrap();
        let task = engine
            .create_object(CreateObjectInput {
                object_type: "task".into(),
                title: "Alpha proof task".into(),
                body: "taskcobalt durable body".into(),
                relative_path: Some("projects/alpha/tasks/proof.md".into()),
                properties: BTreeMap::from([
                    ("status".into(), serde_json::json!("todo")),
                    ("priority".into(), serde_json::json!("high")),
                    ("due".into(), serde_json::json!("2026-09-02")),
                    (
                        "project".into(),
                        serde_json::json!(project.value.id.clone()),
                    ),
                ]),
            })
            .unwrap();
        let note = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Alpha proof note".into(),
                body: "Original noteamber body".into(),
                relative_path: Some("notes/proof.md".into()),
                properties: BTreeMap::from([("date".into(), serde_json::json!("2026-09-03"))]),
            })
            .unwrap();
        let path = workspace.path().join("notes/proof.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Original", "Externally edited");
        std::fs::write(&path, external).unwrap();
        engine.reconcile().unwrap();
        let current = engine.get_object(&note.value.id).unwrap().unwrap();
        assert_eq!(current.body, "Externally edited noteamber body");
        let moved = engine
            .move_object(&current.id, "archive/proof.md", &current.revision)
            .unwrap();
        assert_eq!(moved.value.id, note.value.id);

        let before_objects = engine
            .query_objects(None)
            .unwrap()
            .into_iter()
            .map(|object| {
                (
                    object.id,
                    object.object_type,
                    object.title,
                    object.relative_path,
                    object.body,
                    object.properties,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(before_objects.len(), 3);
        for query in ["projectquartz", "taskcobalt", "noteamber"] {
            assert_eq!(
                engine
                    .search(&SearchInput {
                        query: query.into(),
                        ..Default::default()
                    })
                    .unwrap()
                    .len(),
                1
            );
        }
        let before_calendar = engine.calendar("2026-09-01", "2026-09-05").unwrap();
        assert_eq!(before_calendar.len(), 3);
        assert!(
            before_calendar
                .iter()
                .any(|entry| entry.source_id == project.value.id && entry.end.is_some())
        );
        assert!(
            before_calendar
                .iter()
                .any(|entry| entry.source_id == task.value.id)
        );
        assert!(
            before_calendar
                .iter()
                .any(|entry| entry.source_id == note.value.id)
        );

        let index_path = engine.index_path().to_owned();
        drop(engine);
        std::fs::remove_file(index_path).unwrap();
        let rebuilt =
            WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
        let after_objects = rebuilt
            .query_objects(None)
            .unwrap()
            .into_iter()
            .map(|object| {
                (
                    object.id,
                    object.object_type,
                    object.title,
                    object.relative_path,
                    object.body,
                    object.properties,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(after_objects, before_objects);
        assert_eq!(
            rebuilt
                .get_object(&note.value.id)
                .unwrap()
                .unwrap()
                .relative_path,
            "archive/proof.md"
        );
        for query in ["projectquartz", "taskcobalt", "noteamber"] {
            assert_eq!(
                rebuilt
                    .search(&SearchInput {
                        query: query.into(),
                        ..Default::default()
                    })
                    .unwrap()
                    .len(),
                1
            );
        }
        let after_calendar = rebuilt.calendar("2026-09-01", "2026-09-05").unwrap();
        assert_eq!(
            after_calendar
                .iter()
                .map(|entry| (
                    entry.source_id.as_str(),
                    entry.property.as_str(),
                    entry.start.as_str(),
                    entry.end.as_deref(),
                ))
                .collect::<Vec<_>>(),
            before_calendar
                .iter()
                .map(|entry| (
                    entry.source_id.as_str(),
                    entry.property.as_str(),
                    entry.start.as_str(),
                    entry.end.as_deref(),
                ))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn expected_revision_protects_external_edits() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "First".into(),
                body: String::new(),
                relative_path: Some("first.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        std::fs::write(workspace.path().join("first.md"), "external").unwrap();
        let error = engine
            .update_object(
                &created.value.id,
                ObjectPatch {
                    title: Some("Changed".into()),
                    body: None,
                    properties: BTreeMap::new(),
                    remove_properties: Vec::new(),
                    expected_revision: created.revision,
                },
            )
            .unwrap_err();
        assert_eq!(error.code, "revision_conflict");
    }

    #[test]
    fn draft_reconciliation_merges_independent_markdown_edits_and_snapshots_external() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Merge".into(),
                body: "first\n\nsecond\n".into(),
                relative_path: Some("merge.md".into()),
                properties: BTreeMap::from([("tag".into(), serde_json::json!("base"))]),
            })
            .unwrap();
        let path = workspace.path().join("merge.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("tag: base", "tag: external")
            .replace("second", "second from file");
        std::fs::write(&path, external).unwrap();

        let result = engine
            .reconcile_note_draft(DraftReconcileInput {
                id: created.value.id.clone(),
                base_revision: created.revision,
                base_body: "first\n\nsecond\n".into(),
                local_body: "first in app\n\nsecond\n".into(),
            })
            .unwrap();

        let DraftReconcileResult::Merged { current, body } = result else {
            panic!("expected a clean merge");
        };
        assert_eq!(body, "first in app\n\nsecond from file");
        assert_eq!(current.properties["tag"], "external");
        let history = workspace
            .path()
            .join(".noura/history")
            .join(&created.value.id);
        assert_eq!(std::fs::read_dir(history).unwrap().count(), 1);
        assert!(
            engine
                .list_workspace_entries()
                .unwrap()
                .iter()
                .all(|entry| !entry.relative_path.starts_with(".noura"))
        );
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("second from file")
        );
    }

    #[test]
    fn overlapping_and_unsupported_drafts_never_write_merge_markers() {
        for local_body in ["local\n", "<aside>local</aside>\n"] {
            let workspace = tempdir().unwrap();
            let app_data = tempdir().unwrap();
            let engine =
                WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                    .unwrap();
            let created = engine
                .create_object(CreateObjectInput {
                    object_type: "note".into(),
                    title: "Conflict".into(),
                    body: "base\n".into(),
                    relative_path: Some("conflict.md".into()),
                    properties: BTreeMap::new(),
                })
                .unwrap();
            let path = workspace.path().join("conflict.md");
            let external = std::fs::read_to_string(&path)
                .unwrap()
                .replace("base", "external");
            std::fs::write(&path, external).unwrap();
            let result = engine
                .reconcile_note_draft(DraftReconcileInput {
                    id: created.value.id,
                    base_revision: created.revision,
                    base_body: "base\n".into(),
                    local_body: local_body.into(),
                })
                .unwrap();
            assert!(matches!(result, DraftReconcileResult::Conflict { .. }));
            let canonical = std::fs::read_to_string(path).unwrap();
            assert!(!canonical.contains("<<<<<<<"));
            assert!(canonical.contains("external"));
        }
    }

    #[test]
    fn markdown_merge_covers_common_line_oriented_content() {
        let cases = [
            (
                "one\n\ntwo\n",
                "one local\n\ntwo\n",
                "one\n\ntwo external\n",
                vec!["one local", "two external"],
            ),
            (
                "same\n\nend\n",
                "same edit\n\nend\n",
                "same edit\n\nend\n",
                vec!["same edit"],
            ),
            (
                "start\n\nneutral one\n\nneutral two\n\nend\n",
                "start\n\ninserted\n\nneutral one\n\nneutral two\n\nend\n",
                "start\n\nneutral one\n\nneutral two\n\nend external\n",
                vec!["inserted", "end external"],
            ),
            (
                "keep\nremove local\nkeep two\nexternal tail\n",
                "keep\nkeep two\nexternal tail\n",
                "keep\nremove local\nkeep two\nchanged tail\n",
                vec!["keep two", "changed tail"],
            ),
            (
                "- alpha\n- beta\n\nparagraph\n",
                "- alpha local\n- beta\n\nparagraph\n",
                "- alpha\n- beta\n\nparagraph external\n",
                vec!["alpha local", "paragraph external"],
            ),
            (
                "```rs\nlet a = 1;\n```\n\nafter\n",
                "```rs\nlet a = 2;\n```\n\nafter\n",
                "```rs\nlet a = 1;\n```\n\nafter external\n",
                vec!["let a = 2", "after external"],
            ),
            (
                "| A | B |\n| - | - |\n| 1 | 2 |\n\nafter\n",
                "| A | B |\n| - | - |\n| 1 | local |\n\nafter\n",
                "| A | B |\n| - | - |\n| 1 | 2 |\n\nafter external\n",
                vec!["local", "after external"],
            ),
            (
                "café\n\n世界\n",
                "café local\n\n世界\n",
                "café\n\n世界 external\n",
                vec!["café local", "世界 external"],
            ),
            (
                "one\r\n\r\ntwo\r\n",
                "one local\r\n\r\ntwo\r\n",
                "one\r\n\r\ntwo external\r\n",
                vec!["one local", "two external"],
            ),
            (
                "one\n\ntwo",
                "one local\n\ntwo",
                "one\n\ntwo external",
                vec!["one local", "two external"],
            ),
        ];
        for (index, (base, local, external, fragments)) in cases.into_iter().enumerate() {
            let merged = merge_markdown_body(base, local, external)
                .unwrap_or_else(|| panic!("case {index} unexpectedly conflicted"));
            for fragment in fragments {
                assert!(
                    merged.contains(fragment),
                    "missing {fragment:?} in {merged:?}"
                );
            }
            assert!(!merged.contains("<<<<<<<"));
        }
        assert!(merge_markdown_body("same\n", "local\n", "external\n").is_none());
    }

    #[test]
    fn conflict_resolution_snapshots_each_displaced_version() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Resolve".into(),
                body: "base\n".into(),
                relative_path: Some("resolve.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        let path = workspace.path().join("resolve.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("base", "external");
        std::fs::write(&path, external).unwrap();
        let reviewed = engine
            .reconcile_note_draft(DraftReconcileInput {
                id: created.value.id.clone(),
                base_revision: created.revision,
                base_body: "base\n".into(),
                local_body: "local\n".into(),
            })
            .unwrap();
        let DraftReconcileResult::Conflict { current } = reviewed else {
            panic!("expected a conflict");
        };
        let replaced = engine
            .resolve_note_conflict(ResolveConflictInput {
                id: created.value.id.clone(),
                current_revision: current.revision,
                local_body: "local\n".into(),
                resolution: ConflictResolution::ReplaceExternal,
            })
            .unwrap();
        assert_eq!(replaced.value.body, "local\n");
        let snapshots = std::fs::read_dir(
            workspace
                .path()
                .join(".noura/history")
                .join(created.value.id),
        )
        .unwrap()
        .count();
        assert_eq!(snapshots, 1);
    }

    #[test]
    fn using_external_version_snapshots_the_local_draft() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Use external".into(),
                body: "base\n".into(),
                relative_path: Some("use-external.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        let path = workspace.path().join("use-external.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("base", "external");
        std::fs::write(&path, external).unwrap();
        let DraftReconcileResult::Conflict { current } = engine
            .reconcile_note_draft(DraftReconcileInput {
                id: created.value.id.clone(),
                base_revision: created.revision,
                base_body: "base\n".into(),
                local_body: "local\n".into(),
            })
            .unwrap()
        else {
            panic!("expected a conflict");
        };
        let resolved = engine
            .resolve_note_conflict(ResolveConflictInput {
                id: created.value.id.clone(),
                current_revision: current.revision,
                local_body: "local\n".into(),
                resolution: ConflictResolution::UseExternal,
            })
            .unwrap();
        assert_eq!(resolved.value.body, "external");
        let history = workspace
            .path()
            .join(".noura/history")
            .join(created.value.id);
        let snapshot = std::fs::read_dir(history)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(std::fs::read_to_string(snapshot).unwrap().contains("local"));
    }

    #[test]
    fn create_does_not_overwrite_an_existing_manifest() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        std::fs::write(workspace.path().join("workspace.yaml"), "sentinel").unwrap();
        let error =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .err()
                .unwrap();
        assert_eq!(error.code, "workspace_exists");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("workspace.yaml")).unwrap(),
            "sentinel"
        );
    }

    #[test]
    fn adoption_preserves_idless_frontmatter() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let bytes = b"---\ncustom: retained\ncreated: 2026-08-01T10:00:00Z\n---\n# Draft\n\nBody\n";
        std::fs::write(workspace.path().join("draft.md"), bytes).unwrap();
        let adopted = engine
            .adopt_markdown("draft.md", "note", &markdown::revision(bytes))
            .unwrap();
        assert_eq!(adopted.value.properties["custom"], "retained");
        assert_eq!(
            adopted.value.created.as_deref(),
            Some("2026-08-01T10:00:00Z")
        );
    }

    #[test]
    fn scanner_uses_manifest_ignore_instead_of_gitignore() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        drop(engine);
        let manifest_path = workspace.path().join("workspace.yaml");
        let manifest = std::fs::read_to_string(&manifest_path)
            .unwrap()
            .replace("ignore: []", "ignore:\n- ignored/**");
        std::fs::write(&manifest_path, manifest).unwrap();
        std::fs::write(workspace.path().join(".gitignore"), "visible.md\n").unwrap();
        std::fs::create_dir(workspace.path().join("ignored")).unwrap();
        std::fs::write(
            workspace.path().join("visible.md"),
            "# Visible\n\nneedle-visible",
        )
        .unwrap();
        std::fs::write(
            workspace.path().join("ignored/hidden.md"),
            "# Hidden\n\nneedle-hidden",
        )
        .unwrap();
        let engine =
            WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
        assert_eq!(
            engine
                .search(&SearchInput {
                    query: "needle-visible".into(),
                    ..Default::default()
                })
                .unwrap()
                .len(),
            1
        );
        assert!(
            engine
                .search(&SearchInput {
                    query: "needle-hidden".into(),
                    ..Default::default()
                })
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn invalid_workspace_id_is_rejected_before_creating_local_state() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        std::fs::write(workspace.path().join("workspace.yaml"), "id: ../../outside\nformat_version: 1\nname: Bad\ncreated: 2026-08-27T12:00:00Z\nupdated: 2026-08-27T12:00:00Z\nenabled_plugins: []\nignore: []\n").unwrap();
        let error = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path())
            .err()
            .unwrap();
        assert_eq!(error.code, "invalid_workspace_id");
        assert!(!app_data.path().join("outside").exists());
    }

    #[cfg(unix)]
    #[test]
    fn create_rejects_a_symlinked_internal_directory() {
        let workspace = tempdir().unwrap();
        let outside = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join(".noura")).unwrap();
        let error =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .err()
                .unwrap();
        assert_eq!(error.code, "symlink_escape");
        assert!(!outside.path().join("trash").exists());
    }

    #[test]
    fn external_change_event_is_emitted_after_markdown_reconciliation() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let path = engine.root().join("watched.md");
        let mut events = engine.subscribe();
        std::fs::write(&path, "# Watched\n\nBody\n").unwrap();

        let changes = engine.process_external_changes(vec![path]).unwrap();
        let received = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let event = received
            .iter()
            .find(|event| event.event_type == "file:changed");

        assert_eq!(
            (
                changes,
                event.map(|value| value.payload["paths"].clone()),
                engine
                    .list_non_managed_markdown()
                    .unwrap()
                    .iter()
                    .any(|file| file.relative_path == "watched.md"),
            ),
            (
                vec!["watched.md".to_owned()],
                Some(serde_json::json!(["watched.md"])),
                true,
            )
        );
    }

    #[test]
    fn external_managed_changes_emit_semantic_object_events() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Watched".into(),
                body: "before".into(),
                relative_path: Some("watched-managed.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        let path = engine.root().join("watched-managed.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("before", "after");
        std::fs::write(&path, external).unwrap();
        let mut events = engine.subscribe();

        engine.process_external_changes(vec![path]).unwrap();

        let received = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let updated = received
            .iter()
            .find(|event| event.event_type == "object:updated")
            .unwrap();
        assert_eq!(updated.source, "external");
        assert_eq!(updated.payload["id"], created.value.id);
    }

    #[test]
    fn external_task_priority_edit_reconciles_before_emitting_the_object_event() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "task".into(),
                title: "Ship beta".into(),
                body: String::new(),
                relative_path: Some("tasks/ship-beta.md".into()),
                properties: BTreeMap::from([
                    ("status".into(), serde_json::json!("todo")),
                    ("priority".into(), serde_json::json!("medium")),
                ]),
            })
            .unwrap();
        let path = engine.root().join(&created.value.relative_path);
        let bytes = std::fs::read_to_string(&path)
            .unwrap()
            .replace("priority: medium", "priority: high");
        std::fs::write(&path, bytes).unwrap();
        let mut events = engine.subscribe();

        engine.process_external_changes(vec![path]).unwrap();

        let current = engine.get_object(&created.value.id).unwrap().unwrap();
        assert_eq!(current.properties["priority"], serde_json::json!("high"));
        let received = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let updated = received
            .iter()
            .find(|event| event.event_type == "object:updated")
            .unwrap();
        assert_eq!(updated.source, "external");
        assert_eq!(updated.payload["id"], created.value.id);
    }

    #[test]
    fn external_manifest_change_adopts_the_file_and_emits_manifest_updated() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let manifest_path = engine.root().join("workspace.yaml");
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
        assert_eq!(updated.source, "external");
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

        let manifest_path = engine.root().join("workspace.yaml");
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
        assert_eq!(manifest_events[0].source, "application");
        assert_ne!(engine.manifest().updated, before.updated);
    }

    #[test]
    fn invalid_external_manifest_keeps_the_last_known_good_snapshot() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let manifest_path = engine.root().join("workspace.yaml");
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
        let manifest_path = engine.root().join("workspace.yaml");
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

    #[cfg(unix)]
    #[test]
    fn relative_path_normalization_rejects_non_utf8_paths() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let root = PathBuf::from("/workspace");
        let invalid = OsString::from_vec(vec![b'i', b'n', b'v', 0xff]);
        let error = normalized_relative_path(&root, &root.join(invalid), "files_list").unwrap_err();

        assert_eq!(error.code, "non_utf8_path");
    }
}
