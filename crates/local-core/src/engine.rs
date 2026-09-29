use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    fs::{File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
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
    CreateChatInput, ErrorCategory, EventSource, FinishChatAssistantInput, FinishChatToolCallInput,
    IndexStatus, IndexStore, MutationResult, ParseStatus, ParsedMarkdown, RenameChatInput, Result,
    SearchInput, SearchResult, UnmanagedFile, WORKSPACE_MANIFEST_PATH, WatchCoordinator,
    WorkspaceEntry, WorkspaceEntryKind, WorkspaceManifest, WorkspaceObject, WorkspacePhase,
    WorkspaceState, index::CalendarEntry, markdown, new_object_id, now_rfc3339, parse_chat,
    parse_chat_message, path::resolve_for_write, serialize_chat, serialize_chat_message,
    valid_object_id, valid_object_type, validate_retention,
};

struct CollaborationPresenceCacheEntry {
    object_id: String,
    generation: String,
    session_id: String,
    sequence: u64,
    expires_at: std::time::Instant,
    member: crate::sync::CollaborationPresenceMember,
}

mod chat_journal;
mod chats;
mod copy;
mod drafts;
mod durable;
mod files;
mod manifest;
mod objects;
mod raw_markdown;
mod reconcile;
mod scan;
mod search;
mod sync;
mod trash;

use chat_journal::*;
use drafts::*;
use durable::*;
use manifest::*;
use objects::*;
use raw_markdown::*;
use scan::*;

pub use drafts::{
    ConflictResolution, DraftReconcileInput, DraftReconcileResult, ManagedConflictResolution,
    ManagedConflictResolveInput, ManagedDraftInput, ManagedDraftResult, ResolveConflictInput,
};
pub use manifest::ManifestUpdateInput;
pub use objects::{CreateObjectInput, ObjectPatch};
pub use raw_markdown::{
    MarkdownLinkTarget, RawConflictResolveInput, RawConflictResolveResult, RawMarkdownRead,
    RawReconcileInput, RawReconcileResult, RawSaveInput, RawSaveResult,
};
pub use sync::SYNC_PLUGIN_ID;
pub use trash::{SystemTrash, os_trash};

pub struct WorkspaceEngine {
    root: PathBuf,
    manifest: std::sync::RwLock<WorkspaceManifest>,
    index_path: PathBuf,
    index: Arc<Mutex<IndexStore>>,
    lock_path: PathBuf,
    event_sender: tokio::sync::broadcast::Sender<CoreEvent>,
    watcher: WatchCoordinator,
    /// Paths this engine wrote, with the revision it wrote. Shared with the
    /// watcher thread so our own writes do not mark the index dirty.
    self_writes: Arc<Mutex<HashMap<String, String>>>,
    /// Set by the watcher thread when a visible path changes and cleared when
    /// a workspace walk starts. Reads skip the walk while it stays clear.
    dirty: Arc<AtomicBool>,
    /// What the last full walk found beyond the index itself.
    scan_state: Mutex<ScanState>,
    chat_mutation_fault: Mutex<Option<ChatMutationFault>>,
    #[cfg(test)]
    collaboration_mutation_fault: Mutex<Option<u8>>,
    collaboration_sessions: Mutex<HashMap<String, (String, String, bool)>>,
    collaboration_presence: Mutex<HashMap<String, CollaborationPresenceCacheEntry>>,
    system_trash: std::sync::RwLock<Option<SystemTrash>>,
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
        Self::create_with_identity(root, name, app_data, None, &[])
    }

    /// `extra_plugins` are enabled in the new manifest in addition to the
    /// default domain plugins.
    fn create_with_identity(
        root: impl AsRef<Path>,
        name: &str,
        app_data: impl AsRef<Path>,
        workspace_id: Option<&str>,
        extra_plugins: &[&str],
    ) -> Result<Self> {
        let root = root.as_ref();
        std::fs::create_dir_all(root)
            .map_err(|error| CoreError::io(error, "workspace_create", root.to_str()))?;
        let canonical = root
            .canonicalize()
            .map_err(|error| CoreError::io(error, "workspace_create", root.to_str()))?;
        if canonical.join(WORKSPACE_MANIFEST_PATH).exists() {
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
        let mut enabled_plugins: Vec<String> =
            ["folders", "notes", "tasks", "calendar", "projects"]
                .into_iter()
                .map(str::to_owned)
                .collect();
        for plugin in extra_plugins {
            if !enabled_plugins.iter().any(|id| id == plugin) {
                enabled_plugins.push((*plugin).to_owned());
            }
        }
        let manifest = WorkspaceManifest {
            id: workspace_id.map(str::to_owned).unwrap_or_else(|| {
                format!("workspace_{}", ulid::Ulid::new().to_string().to_lowercase())
            }),
            format_version: 1,
            name: name.trim().to_owned(),
            created: now.clone(),
            updated: now,
            enabled_plugins,
            ignore: Vec::new(),
        };
        validate_manifest(&manifest, "workspace_create")?;
        let bytes = serialize_workspace_manifest(&manifest, "workspace_create")?;
        resolve_for_write(&canonical, WORKSPACE_MANIFEST_PATH, "workspace_create")?;
        atomic_write(
            &canonical,
            Path::new(WORKSPACE_MANIFEST_PATH),
            bytes.as_bytes(),
            "workspace_create",
        )?;
        let trash = resolve_for_write(&canonical, ".noura/trash", "workspace_create")?;
        std::fs::create_dir_all(&trash)
            .map_err(|error| CoreError::io(error, "workspace_create", canonical.to_str()))?;
        Self::open_with_app_data(canonical, app_data)
    }

    /// Opens a workspace, initializing Noura metadata when an existing
    /// directory has no `.noura/workspace.yaml`. A saved identity can be supplied
    /// when repairing a previously registered workspace.
    pub fn open_or_initialize(
        root: impl AsRef<Path>,
        name: &str,
        workspace_id: Option<&str>,
    ) -> Result<Self> {
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
        Self::open_or_initialize_with_app_data(root, name, workspace_id, app_data)
    }

    pub fn open_or_initialize_with_app_data(
        root: impl AsRef<Path>,
        name: &str,
        workspace_id: Option<&str>,
        app_data: impl AsRef<Path>,
    ) -> Result<Self> {
        let root = root
            .as_ref()
            .canonicalize()
            .map_err(|error| CoreError::io(error, "workspace_open", root.as_ref().to_str()))?;
        if root
            .join(WORKSPACE_MANIFEST_PATH)
            .try_exists()
            .map_err(|error| {
                CoreError::io(error, "workspace_open", Some(WORKSPACE_MANIFEST_PATH))
            })?
        {
            return Self::open_with_app_data(root, app_data);
        }
        Self::create_with_identity(root, name, app_data, workspace_id, &[])
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
        let manifest_bytes =
            std::fs::read(root.join(WORKSPACE_MANIFEST_PATH)).map_err(|error| {
                CoreError::io(error, "workspace_open", Some(WORKSPACE_MANIFEST_PATH))
            })?;
        let manifest = parse_workspace_manifest(&manifest_bytes, "workspace_open")?;
        let local_dir = app_data.as_ref().join("workspaces").join(&manifest.id);
        std::fs::create_dir_all(&local_dir)
            .map_err(|error| CoreError::io(error, "workspace_open", local_dir.to_str()))?;
        let index_path = local_dir.join("index.sqlite");
        let index = IndexStore::open(&index_path)?;
        let (event_sender, _) = tokio::sync::broadcast::channel(256);
        let self_writes = Arc::new(Mutex::new(HashMap::new()));
        let dirty = Arc::new(AtomicBool::new(false));
        let watcher = WatchCoordinator::new(
            &root,
            dirty_marker(root.clone(), self_writes.clone(), dirty.clone()),
        );
        let engine = Self {
            root,
            manifest: std::sync::RwLock::new(manifest),
            index_path,
            index: Arc::new(Mutex::new(index)),
            lock_path: local_dir.join("workspace.lock"),
            event_sender,
            watcher,
            self_writes,
            dirty,
            scan_state: Mutex::new(ScanState::default()),
            chat_mutation_fault: Mutex::new(None),
            #[cfg(test)]
            collaboration_mutation_fault: Mutex::new(None),
            collaboration_sessions: Mutex::new(HashMap::new()),
            collaboration_presence: Mutex::new(HashMap::new()),
            system_trash: std::sync::RwLock::new(None),
        };
        engine.recover_pending_chat_mutations()?;
        engine.reconcile()?;
        engine.migrate_sync_plugin();
        engine.emit(
            "workspace:ready",
            EventSource::Reconciliation,
            serde_json::json!({}),
        );
        Ok(engine)
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
        let (indexed_files, mut diagnostics) =
            self.index.lock().ok().map_or((0, Vec::new()), |index| {
                (
                    index.file_count().unwrap_or(0),
                    index.diagnostics().unwrap_or_default(),
                )
            });
        if let Ok(scan) = self.scan_state.lock() {
            diagnostics.extend(scan.diagnostics.iter().cloned());
        }
        WorkspaceState {
            phase: WorkspacePhase::Ready,
            workspace_id: Some(self.current_workspace_id()),
            root_path: self.root.to_str().map(str::to_owned),
            indexed_files,
            diagnostics,
        }
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

    fn emit(&self, event_type: &str, source: EventSource, payload: serde_json::Value) {
        let _ = self.event_sender.send(CoreEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: event_type.into(),
            workspace_id: self.current_workspace_id(),
            occurred_at: now_rfc3339(),
            source,
            payload,
        });
    }

    fn index_outcome(&self, result: Result<()>) -> (IndexStatus, Vec<CoreWarning>) {
        match result {
            Ok(()) => (IndexStatus::Updated, Vec::new()),
            Err(_) => {
                self.emit(
                    "workspace:index-stale",
                    EventSource::Application,
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

fn lock_error(operation: &str) -> CoreError {
    CoreError::new(
        "lock_poisoned",
        ErrorCategory::Transient,
        "A local workspace lock is unavailable",
        operation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn create_does_not_overwrite_an_existing_manifest() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        std::fs::create_dir(workspace.path().join(".noura")).unwrap();
        std::fs::write(workspace.path().join(WORKSPACE_MANIFEST_PATH), "sentinel").unwrap();
        let error =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .err()
                .unwrap();
        assert_eq!(error.code, "workspace_exists");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join(WORKSPACE_MANIFEST_PATH)).unwrap(),
            "sentinel"
        );
    }

    #[test]
    fn create_treats_root_workspace_yaml_as_an_ordinary_file() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        std::fs::write(workspace.path().join("workspace.yaml"), "user content").unwrap();

        WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path()).unwrap();

        assert_eq!(
            (
                std::fs::read_to_string(workspace.path().join("workspace.yaml")).unwrap(),
                workspace.path().join(WORKSPACE_MANIFEST_PATH).is_file(),
            ),
            ("user content".to_owned(), true)
        );
    }

    #[test]
    fn open_or_initialize_adds_metadata_to_an_existing_folder() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        std::fs::write(workspace.path().join("existing.md"), "# Existing\n").unwrap();

        let engine = WorkspaceEngine::open_or_initialize_with_app_data(
            workspace.path(),
            "Existing notes",
            None,
            app_data.path(),
        )
        .unwrap();

        assert_eq!(engine.manifest().name, "Existing notes");
        assert_eq!(
            std::fs::read_to_string(workspace.path().join("existing.md")).unwrap(),
            "# Existing\n"
        );
        assert!(workspace.path().join(".noura/trash").is_dir());
        assert!(workspace.path().join(WORKSPACE_MANIFEST_PATH).is_file());
    }

    #[test]
    fn open_or_initialize_does_not_create_a_missing_folder() {
        let parent = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let workspace = parent.path().join("missing");

        let error = WorkspaceEngine::open_or_initialize_with_app_data(
            &workspace,
            "Missing",
            None,
            app_data.path(),
        )
        .err()
        .unwrap();

        assert_eq!(error.operation, "workspace_open");
        assert!(!workspace.exists());
    }

    #[test]
    fn open_or_initialize_reuses_a_saved_workspace_identity() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let workspace_id = "workspace_01j00000000000000000000000";

        let engine = WorkspaceEngine::open_or_initialize_with_app_data(
            workspace.path(),
            "Recovered",
            Some(workspace_id),
            app_data.path(),
        )
        .unwrap();

        assert_eq!(engine.manifest().id, workspace_id);
    }

    #[test]
    fn open_or_initialize_rejects_an_invalid_saved_identity_before_writing() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();

        let error = WorkspaceEngine::open_or_initialize_with_app_data(
            workspace.path(),
            "Recovered",
            Some("../../outside"),
            app_data.path(),
        )
        .err()
        .unwrap();

        assert_eq!(error.code, "invalid_workspace_id");
        assert!(!workspace.path().join(WORKSPACE_MANIFEST_PATH).exists());
    }

    #[test]
    fn invalid_workspace_id_is_rejected_before_creating_local_state() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        std::fs::create_dir(workspace.path().join(".noura")).unwrap();
        std::fs::write(workspace.path().join(WORKSPACE_MANIFEST_PATH), "id: ../../outside\nformat_version: 1\nname: Bad\ncreated: 2026-08-27T12:00:00Z\nupdated: 2026-08-27T12:00:00Z\nenabled_plugins: []\nignore: []\n").unwrap();
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
}
