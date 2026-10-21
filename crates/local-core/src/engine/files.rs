//! Folders and ordinary files: listings, moves, trash, PDFs, and local
//! assets referenced from notes.

use super::*;

impl WorkspaceEngine {
    /// Read one non-Markdown file (image or other asset) for inline preview.
    /// Workspace containment is validated; total size is capped by the caller.
    pub fn inspect_pdf(&self, relative_path: &str) -> Result<crate::PdfInfo> {
        crate::pdf::inspect(&self.root, &self.manifest().id, relative_path)
    }

    pub fn read_pdf_range(&self, input: &crate::PdfRangeInput) -> Result<Vec<u8>> {
        crate::pdf::read_range(&self.root, &self.manifest().id, input)
    }

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

    /// Create a folder and the folders above it. Every new folder's entry is
    /// durable before this returns.
    pub fn create_folder(&self, relative_path: &str) -> Result<()> {
        let path = resolve_for_write(&self.root, relative_path, "folder_create")?;
        create_folders(&path, "folder_create")
    }

    pub fn list_folders(&self) -> Result<Vec<crate::FolderEntry>> {
        let scan = scan_workspace(&self.root, &self.current_ignore(), None, true)?;
        let mut folders = scan
            .entries
            .into_iter()
            .filter(|entry| entry.is_folder)
            .map(|entry| crate::FolderEntry {
                relative_path: entry.relative_path,
                name: entry.name,
            })
            .collect::<Vec<_>>();
        folders.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(folders)
    }

    /// Every visible file and folder with its index status, from one walk.
    /// The walk also reconciles when the watcher reported changes. The
    /// files are canonical, so when the index can't be updated the listing
    /// still shows them, with what the index last knew about each.
    pub fn list_workspace_entries(&self) -> Result<Vec<WorkspaceEntry>> {
        self.list_workspace_entries_counting_unsupported()
            .map(|(entries, _)| entries)
    }

    /// [`Self::list_workspace_entries`], and how many files and folders it
    /// left out because their names can't be represented as UTF-8.
    pub fn list_workspace_entries_counting_unsupported(
        &self,
    ) -> Result<(Vec<WorkspaceEntry>, usize)> {
        let scan = if self.needs_reconcile() {
            match self.reconcile_walk(&HashSet::new(), EventSource::Reconciliation, true) {
                Ok(scan) => scan,
                Err(error) if matches!(error.category, ErrorCategory::Index) => {
                    tracing::warn!(code = %error.code, "listing files the index couldn't follow");
                    self.dirty.store(true, Ordering::SeqCst);
                    scan_workspace(&self.root, &self.current_ignore(), None, true)?
                }
                Err(error) => return Err(error),
            }
        } else {
            self.recover_pending_chat_mutations()?;
            scan_workspace(&self.root, &self.current_ignore(), None, true)?
        };
        let mut metadata = self
            .index
            .lock()
            .map_err(|_| lock_error("files_list"))?
            .workspace_entry_metadata()?;
        let unindexed = scan
            .entries
            .iter()
            .filter(|entry| {
                !entry.is_folder
                    && !entry.not_downloaded
                    && is_markdown_path(&entry.relative_path)
                    && !metadata.contains_key(&entry.relative_path)
            })
            .map(|entry| entry.relative_path.clone())
            .collect::<Vec<_>>();
        if !unindexed.is_empty() {
            // A file appeared before its watcher event arrived. Index just
            // those files instead of failing the listing.
            match self.reconcile_paths(&unindexed, EventSource::Reconciliation) {
                Ok(()) => {
                    metadata = self
                        .index
                        .lock()
                        .map_err(|_| lock_error("files_list"))?
                        .workspace_entry_metadata()?;
                }
                Err(error) if matches!(error.category, ErrorCategory::Index) => {
                    tracing::warn!(code = %error.code, "listing files the index couldn't add");
                    self.dirty.store(true, Ordering::SeqCst);
                }
                Err(error) => return Err(error),
            }
        }
        let unsupported = scan
            .diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.code == "unsupported_path")
            .count();
        let mut entries = scan
            .entries
            .into_iter()
            .map(|entry| {
                if entry.is_folder {
                    return WorkspaceEntry {
                        relative_path: entry.relative_path,
                        name: entry.name,
                        kind: WorkspaceEntryKind::Folder,
                        parse_status: None,
                        object_id: None,
                        object_type: None,
                        revision: None,
                        not_downloaded: false,
                    };
                }
                let indexed = metadata.get(&entry.relative_path);
                let markdown = is_markdown_path(&entry.relative_path);
                WorkspaceEntry {
                    parse_status: match indexed {
                        Some(value) => Some(value.parse_status),
                        None if markdown => None,
                        None => Some(ParseStatus::Binary),
                    },
                    object_id: indexed.and_then(|value| value.object_id.clone()),
                    object_type: indexed.and_then(|value| value.object_type.clone()),
                    revision: indexed.map(|value| value.revision.clone()),
                    relative_path: entry.relative_path,
                    name: entry.name,
                    kind: WorkspaceEntryKind::File,
                    not_downloaded: entry.not_downloaded,
                }
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        entries.dedup_by(|right, left| left.relative_path == right.relative_path);
        Ok((entries, unsupported))
    }

    /// The bytes of the file at `relative_path` and the revision of those
    /// bytes, for a reader such as a plugin. The path must stay inside the
    /// workspace, out of the folders Noura keeps to itself and off links.
    /// A file larger than `max_bytes` is refused.
    pub fn read_file(&self, relative_path: &str, max_bytes: u64) -> Result<(Vec<u8>, String)> {
        let operation = "file_read";
        let path = resolve_for_write(&self.root, relative_path, operation)?;
        let not_found = || {
            let mut error = CoreError::new(
                "file_not_found",
                ErrorCategory::Filesystem,
                "The file doesn't exist",
                operation,
            );
            error.path = Some(relative_path.to_owned());
            error
        };
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) if metadata.is_file() => metadata,
            Ok(_) => return Err(not_found()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Err(not_found()),
            Err(error) => return Err(CoreError::io(error, operation, Some(relative_path))),
        };
        if metadata.len() > max_bytes {
            return Err(CoreError::validation(
                "file_too_large",
                "The file is larger than the reader accepts",
                operation,
            ));
        }
        let bytes = std::fs::read(&path)
            .map_err(|error| CoreError::io(error, operation, Some(relative_path)))?;
        let revision = markdown::revision(&bytes);
        Ok((bytes, revision))
    }

    pub fn list_non_managed_markdown(&self) -> Result<Vec<UnmanagedFile>> {
        self.reconcile_if_needed()?;
        self.index
            .lock()
            .map_err(|_| lock_error("files_list_non_managed"))?
            .query_non_managed_markdown()
    }

    /// Rename or move a folder with everything in it, as
    /// [`Self::move_file`] does for a file.
    pub fn move_folder(&self, from: &str, to: &str) -> Result<()> {
        let source = resolve_for_write(&self.root, from, "folder_move")?;
        let destination = resolve_for_write(&self.root, to, "folder_move")?;
        let guard = self.write_lock("folder_move")?;
        self.collaboration_guard_tree_mutation(from)?;
        self.collaboration_guard_tree_mutation(to)?;
        if !source.is_dir() {
            return Err(CoreError::validation(
                "folder_not_found",
                "The source folder does not exist",
                "folder_move",
            ));
        }
        // A case-only rename names the same folder on case-insensitive volumes.
        if std::fs::symlink_metadata(&destination).is_ok() && !same_file(&source, &destination) {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A folder already exists at the destination",
                "folder_move",
            ));
        }
        if let Some(parent) = destination.parent() {
            create_folders(parent, "folder_move")?;
        }
        std::fs::rename(&source, &destination)
            .map_err(|error| CoreError::io(error, "folder_move", Some(to)))?;
        sync_rename_parents(&source, &destination, "folder_move")?;
        drop(guard);
        self.reconcile_after_change("folder_move");
        Ok(())
    }

    /// Rename or move one file inside the workspace. A managed object keeps
    /// its stable ID because the ID lives in the file. The move is durable
    /// before this returns; an index that can't follow catches up on the
    /// next read instead of failing the move.
    pub fn move_file(&self, from: &str, to: &str) -> Result<()> {
        let operation = "file_move";
        let source = resolve_for_write(&self.root, from, operation)?;
        let destination = resolve_for_write(&self.root, to, operation)?;
        let guard = self.write_lock(operation)?;
        self.collaboration_guard_file_mutation(from)?;
        self.collaboration_guard_file_mutation(to)?;
        let metadata = std::fs::symlink_metadata(&source)
            .map_err(|error| CoreError::io(error, operation, Some(from)))?;
        if !metadata.is_file() {
            return Err(CoreError::validation(
                "file_not_found",
                "The source is not a file in this workspace",
                operation,
            ));
        }
        if std::fs::symlink_metadata(&destination).is_ok() && !same_file(&source, &destination) {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A file already exists at the destination",
                operation,
            ));
        }
        if let Some(parent) = destination.parent() {
            create_folders(parent, operation)?;
        }
        std::fs::rename(&source, &destination)
            .map_err(|error| CoreError::io(error, operation, Some(to)))?;
        sync_rename_parents(&source, &destination, operation)?;
        drop(guard);
        self.reconcile_after_change(operation);
        Ok(())
    }

    /// Move a file or folder to the trash. Returns the `.noura/trash` path
    /// when it went there instead of the system trash. As with
    /// [`Self::move_file`], an index that can't follow doesn't fail it.
    pub fn trash_path(&self, relative_path: &str) -> Result<Option<String>> {
        let operation = "file_trash";
        let source = resolve_for_write(&self.root, relative_path, operation)?;
        if source == self.root {
            return Err(CoreError::validation(
                "invalid_path",
                "The workspace folder itself can't be deleted",
                operation,
            ));
        }
        let guard = self.write_lock(operation)?;
        self.collaboration_guard_tree_mutation(relative_path)?;
        std::fs::symlink_metadata(&source)
            .map_err(|error| CoreError::io(error, operation, Some(relative_path)))?;
        let trashed = self.discard(&source, relative_path, operation)?;
        drop(guard);
        self.reconcile_after_change(operation);
        Ok(trashed)
    }

    pub fn remove_empty_folder(&self, relative_path: &str) -> Result<()> {
        let path = resolve_for_write(&self.root, relative_path, "folder_remove")?;
        let _guard = self.write_lock("folder_remove")?;
        std::fs::remove_dir(path)
            .map_err(|error| CoreError::io(error, "folder_remove", Some(relative_path)))
    }
}
