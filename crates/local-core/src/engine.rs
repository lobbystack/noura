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
    CoreError, CoreEvent, CoreWarning, ErrorCategory, IndexStatus, IndexStore, MutationResult,
    ParsedMarkdown, Result, SearchInput, SearchResult, WatchCoordinator, WorkspaceManifest,
    WorkspaceObject, WorkspacePhase, WorkspaceState, index::CalendarEntry, markdown, new_object_id,
    now_rfc3339, path::resolve_for_write, valid_object_id, valid_object_type,
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

type MarkdownIndexEntry = (String, Vec<u8>, i64, ParsedMarkdown);

struct WorkspaceScan {
    changed: Vec<MarkdownIndexEntry>,
    seen: std::collections::HashSet<String>,
}

pub struct WorkspaceEngine {
    root: PathBuf,
    manifest: WorkspaceManifest,
    index_path: PathBuf,
    index: Arc<Mutex<IndexStore>>,
    lock_path: PathBuf,
    event_sender: tokio::sync::broadcast::Sender<CoreEvent>,
    watcher: WatchCoordinator,
    self_writes: Mutex<HashMap<String, String>>,
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
            id: format!("workspace_{}", ulid::Ulid::new().to_string().to_lowercase()),
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
        let manifest: WorkspaceManifest =
            serde_yaml_ng::from_slice(&manifest_bytes).map_err(|_| {
                CoreError::new(
                    "invalid_workspace_manifest",
                    ErrorCategory::Parse,
                    "workspace.yaml is invalid",
                    "workspace_open",
                )
            })?;
        validate_manifest(&manifest)?;
        if manifest.format_version != 1 {
            return Err(CoreError::validation(
                "unsupported_workspace_version",
                "This workspace format version is not supported",
                "workspace_open",
            ));
        }
        let local_dir = app_data.as_ref().join("workspaces").join(&manifest.id);
        std::fs::create_dir_all(&local_dir)
            .map_err(|error| CoreError::io(error, "workspace_open", local_dir.to_str()))?;
        let index_path = local_dir.join("index.sqlite");
        let index = IndexStore::open(&index_path)?;
        let (event_sender, _) = tokio::sync::broadcast::channel(256);
        let watcher = WatchCoordinator::new(&root)?;
        let engine = Self {
            root,
            manifest,
            index_path,
            index: Arc::new(Mutex::new(index)),
            lock_path: local_dir.join("workspace.lock"),
            event_sender,
            watcher,
            self_writes: Mutex::new(HashMap::new()),
        };
        engine.reconcile()?;
        engine.emit("workspace:ready", "reconciliation", serde_json::json!({}));
        Ok(engine)
    }

    pub fn manifest(&self) -> &WorkspaceManifest {
        &self.manifest
    }
    pub fn root(&self) -> &Path {
        &self.root
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
            workspace_id: Some(self.manifest.id.clone()),
            root_path: self.root.to_str().map(str::to_owned),
            indexed_files,
            diagnostics,
        }
    }

    pub fn rebuild_index(&self) -> Result<()> {
        self.emit("workspace:rebuilding", "application", serde_json::json!({}));
        let next = self.index_path.with_extension("sqlite.next");
        if next.exists() {
            std::fs::remove_file(&next)
                .map_err(|error| CoreError::io(error, "index_rebuild", next.to_str()))?;
        }
        let mut replacement = IndexStore::open(&next)?;
        scan_into(&self.root, &self.manifest.ignore, &mut replacement)?;
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
        self.reconcile_forced(&std::collections::HashSet::new())
    }

    fn reconcile_forced(&self, forced: &std::collections::HashSet<String>) -> Result<()> {
        let metadata = self
            .index
            .lock()
            .map_err(|_| lock_error("workspace_reconcile"))?
            .file_metadata()?;
        let scan = scan_changes(&self.root, &self.manifest.ignore, &metadata, forced)?;
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
        self.emit(
            "search:index-updated",
            "reconciliation",
            serde_json::json!({}),
        );
        Ok(())
    }

    pub fn poll_external_changes(&self, wait: std::time::Duration) -> Result<Vec<String>> {
        let paths = self.watcher.drain_coalesced(wait)?;
        let mut external = Vec::new();
        let mut journal = self
            .self_writes
            .lock()
            .map_err(|_| lock_error("watcher_poll"))?;
        for path in paths {
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            if relative.starts_with(".noura") {
                continue;
            }
            let Some(relative) = relative.to_str().map(|value| value.replace('\\', "/")) else {
                continue;
            };
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
            self.reconcile_forced(&external.iter().cloned().collect())?;
            self.emit(
                "file:changed",
                "external",
                serde_json::json!({ "paths": external }),
            );
        }
        Ok(external)
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
        let root = self.root.clone();
        let walker = WalkBuilder::new(&self.root)
            .hidden(false)
            .filter_entry(move |entry| {
                !entry.path().strip_prefix(&root).is_ok_and(|relative| {
                    relative.starts_with(".noura")
                        || relative.starts_with(".git")
                        || relative.starts_with("node_modules")
                        || relative.starts_with("target")
                })
            })
            .build();
        for entry in walker {
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
            let relative = entry
                .path()
                .strip_prefix(&self.root)
                .ok()
                .and_then(|value| value.to_str())
                .ok_or_else(|| {
                    CoreError::validation(
                        "non_utf8_path",
                        "A folder path is not UTF-8",
                        "folders_list",
                    )
                })?;
            folders.push(crate::FolderEntry {
                relative_path: relative.replace('\\', "/"),
                name: entry.file_name().to_string_lossy().into_owned(),
            });
        }
        folders.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(folders)
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
        atomic_write(&self.root, &relative, &bytes, operation)?;
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
    fn emit(&self, event_type: &str, source: &str, payload: serde_json::Value) {
        let _ = self.event_sender.send(CoreEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event_type: event_type.into(),
            workspace_id: self.manifest.id.clone(),
            occurred_at: now_rfc3339(),
            source: source.into(),
            payload,
        });
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
    let ignores = compile_workspace_ignores(root, ignore_patterns)?;
    let filter_root = root.to_owned();
    let filter_ignores = ignores.clone();
    let walker = WalkBuilder::new(root)
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
            !relative.starts_with(".noura")
                && !relative.starts_with(".git")
                && !relative.starts_with("node_modules")
                && !relative.starts_with("target")
                && !filter_ignores
                    .matched_path_or_any_parents(
                        relative,
                        entry.file_type().is_some_and(|kind| kind.is_dir()),
                    )
                    .is_ignore()
        })
        .build();
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
        let relative = entry
            .path()
            .strip_prefix(root)
            .ok()
            .and_then(|path| path.to_str())
            .ok_or_else(|| {
                CoreError::validation(
                    "non_utf8_path",
                    "The scanner found a non-UTF-8 path",
                    "workspace_scan",
                )
            })?;
        let relative = relative.replace('\\', "/");
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
fn atomic_write(root: &Path, relative: &Path, bytes: &[u8], operation: &str) -> Result<()> {
    let destination = root.join(relative);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    }
    let mut file = atomic_write_file::AtomicWriteFile::open(&destination)
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .and_then(|()| file.commit())
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    sync_parent(&destination, operation)?;
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
fn validate_manifest(manifest: &WorkspaceManifest) -> Result<()> {
    if !valid_object_id(&manifest.id, "workspace") {
        return Err(CoreError::validation(
            "invalid_workspace_id",
            "The workspace ID must be a lowercase stable workspace ID",
            "workspace_open",
        ));
    }
    if manifest.name.trim().is_empty() {
        return Err(CoreError::validation(
            "workspace_name_required",
            "A workspace name is required",
            "workspace_open",
        ));
    }
    compile_workspace_ignores(Path::new("."), &manifest.ignore)?;
    Ok(())
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
    value.parse::<jiff::Timestamp>().map_err(|_| {
        CoreError::validation(
            "invalid_timestamp",
            format!("{key} must be an RFC 3339 timestamp"),
            operation,
        )
    })?;
    Ok(Some(value.to_owned()))
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
                .is_some_and(|manifest| validate_manifest(&manifest).is_ok());
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
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "First".into(),
                body: "Original".into(),
                relative_path: Some("notes/first.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        assert!(workspace.path().join("notes/first.md").exists());
        assert_eq!(
            engine
                .search(&SearchInput {
                    query: "Original".into(),
                    ..Default::default()
                })
                .unwrap()
                .len(),
            1
        );
        let path = workspace.path().join("notes/first.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Original", "External");
        std::fs::write(&path, external).unwrap();
        engine.reconcile().unwrap();
        let current = engine.get_object(&created.value.id).unwrap().unwrap();
        assert_eq!(current.body, "External");
        let moved = engine
            .move_object(&current.id, "archive/first.md", &current.revision)
            .unwrap();
        assert_eq!(moved.value.id, created.value.id);
        let index_path = engine.index_path().to_owned();
        drop(engine);
        std::fs::remove_file(index_path).unwrap();
        let rebuilt =
            WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
        let after = rebuilt.get_object(&created.value.id).unwrap().unwrap();
        assert_eq!(after.id, created.value.id);
        assert_eq!(after.relative_path, "archive/first.md");
        assert_eq!(after.body, "External");
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
}
