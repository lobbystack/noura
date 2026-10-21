//! Copying files and folders from elsewhere on the computer into the
//! workspace, as when they are dropped on the file tree. The sources stay
//! where they are.

use std::io::Read;

use super::*;

/// What an import copied into the workspace.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportResult {
    /// Workspace paths of the dropped files and folders, in drop order. A
    /// name already taken gets a number, as `Plan 1.md`.
    pub imported: Vec<String>,
    /// Entries left out: symbolic links, hidden files, names that aren't
    /// UTF-8, and anything that vanished before it could be copied.
    pub skipped: u32,
}

impl WorkspaceEngine {
    /// Copy files and folders at absolute `sources` into the workspace folder
    /// `folder` (`""` is the top of the workspace).
    ///
    /// Folders are copied with everything in them. Existing names are never
    /// overwritten: a clash gets the next free number. A managed object whose
    /// ID the workspace already uses gets a new ID, so the copy keeps its own
    /// identity; any other file is copied byte for byte. Every copied file is
    /// durable before this returns.
    pub fn import_external(&self, sources: &[PathBuf], folder: &str) -> Result<ImportResult> {
        let operation = "files_import";
        let target = if folder.is_empty() {
            self.root.clone()
        } else {
            resolve_for_write(&self.root, folder, operation)?
        };
        let is_folder = std::fs::symlink_metadata(&target)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false);
        if !is_folder {
            return Err(CoreError::validation(
                "folder_not_found",
                "The destination is not a folder in this workspace",
                operation,
            ));
        }
        let root = std::fs::canonicalize(&self.root)
            .map_err(|error| CoreError::io(error, operation, None))?;
        let destination = std::fs::canonicalize(&target)
            .map_err(|error| CoreError::io(error, operation, None))?;
        // Held until the walk below has indexed every copy, so no walk in
        // between finds one unindexed and takes it for another app's.
        let _changes = self.hold_changes();
        let mut import = Import {
            engine: self,
            operation,
            ids: HashSet::new(),
            skipped: 0,
        };
        // Every source passes the checks before anything is copied, so a
        // refused import copies nothing.
        for source in sources {
            if !source.is_absolute() {
                return Err(CoreError::validation(
                    "absolute_path_required",
                    "Dropped files must be given by absolute path",
                    operation,
                ));
            }
            // Copying a folder that holds the workspace would copy the copy.
            if std::fs::canonicalize(source).is_ok_and(|source| root.starts_with(source)) {
                return Err(CoreError::validation(
                    "import_contains_workspace",
                    "A folder can't be copied into a workspace inside it",
                    operation,
                ));
            }
            // Copying a folder into itself would find each new copy and recurse.
            if std::fs::canonicalize(source).is_ok_and(|source| destination.starts_with(source)) {
                return Err(CoreError::validation(
                    "import_into_itself",
                    "A folder can't be copied into itself",
                    operation,
                ));
            }
        }
        let mut imported = Vec::new();
        let copied = sources.iter().try_for_each(|source| {
            if let Some(path) = import.entry(source, folder)? {
                imported.push(path);
            }
            Ok(())
        });
        // Whatever landed before a failure is on disk; index it and
        // announce it either way.
        self.reconcile_after_change(operation);
        self.announce_own_change(&imported.iter().map(String::as_str).collect::<Vec<_>>());
        copied?;
        Ok(ImportResult {
            imported,
            skipped: import.skipped,
        })
    }
}

struct Import<'a> {
    engine: &'a WorkspaceEngine,
    operation: &'a str,
    /// Object IDs this import has written, so two copies of one note in a
    /// dropped folder don't share an ID before the index sees either.
    ids: HashSet<String>,
    skipped: u32,
}

impl Import<'_> {
    /// Copy one file or folder into `folder`. Returns its workspace path, or
    /// None when it was skipped.
    fn entry(&mut self, source: &Path, folder: &str) -> Result<Option<String>> {
        let Some(name) = source
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.starts_with('.'))
        else {
            self.skipped += 1;
            return Ok(None);
        };
        let Ok(metadata) = std::fs::symlink_metadata(source) else {
            self.skipped += 1;
            return Ok(None);
        };
        let path = match self.free_path(folder, name, metadata.is_dir()) {
            Ok(path) => path,
            // `node_modules` or `target` at the top, which noura never writes.
            Err(error) if error.code == "reserved_path" => {
                self.skipped += 1;
                return Ok(None);
            }
            Err(error) => return Err(error),
        };
        if metadata.is_dir() {
            self.create_folder(&path)?;
            let mut children = std::fs::read_dir(source)
                .map_err(|error| CoreError::io(error, self.operation, None))?
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .collect::<Vec<_>>();
            children.sort();
            for child in children {
                self.entry(&child, &path)?;
            }
            Ok(Some(path))
        } else if metadata.is_file() {
            self.file(source, &path)?;
            Ok(Some(path))
        } else {
            self.skipped += 1;
            Ok(None)
        }
    }

    /// `folder/name`, or `folder/name 1.ext` and so on when that is taken.
    fn free_path(&self, folder: &str, name: &str, is_folder: bool) -> Result<String> {
        let (stem, suffix) = match name.rfind('.') {
            Some(dot) if dot > 0 && !is_folder => name.split_at(dot),
            _ => (name, ""),
        };
        let join = |name: String| {
            if folder.is_empty() {
                name
            } else {
                format!("{folder}/{name}")
            }
        };
        for number in 0u32.. {
            let candidate = if number == 0 {
                join(name.to_owned())
            } else {
                join(format!("{stem} {number}{suffix}"))
            };
            let path = resolve_for_write(&self.engine.root, &candidate, self.operation)?;
            if std::fs::symlink_metadata(&path).is_err() {
                return Ok(candidate);
            }
        }
        unreachable!("a free name exists")
    }

    fn create_folder(&self, relative: &str) -> Result<()> {
        let path = resolve_for_write(&self.engine.root, relative, self.operation)?;
        self.engine.expect_own_folder(relative);
        std::fs::create_dir(&path)
            .map_err(|error| CoreError::io(error, self.operation, Some(relative)))
            .inspect_err(|_| self.engine.forget_own_write(relative))?;
        sync_parent(&path, self.operation)
    }

    fn file(&mut self, source: &Path, relative: &str) -> Result<()> {
        let operation = self.operation;
        if is_markdown_path(relative) {
            let bytes = std::fs::read(source)
                .map_err(|error| CoreError::io(error, operation, Some(relative)))?;
            if let ParsedMarkdown::Managed(object) = markdown::parse_markdown(relative, &bytes) {
                let taken =
                    self.ids.contains(&object.id) || self.engine.get_object(&object.id)?.is_some();
                if taken {
                    let title = file_stem(relative);
                    let created = self.engine.create_object(CreateObjectInput {
                        object_type: object.object_type,
                        title: if title.trim().is_empty() {
                            object.title
                        } else {
                            title
                        },
                        body: object.body,
                        relative_path: Some(relative.to_owned()),
                        properties: object.properties,
                    })?;
                    self.ids.insert(created.value.id);
                    return Ok(());
                }
                self.ids.insert(object.id);
            }
            return self.write(relative, &mut bytes.as_slice());
        }
        let mut file =
            File::open(source).map_err(|error| CoreError::io(error, operation, Some(relative)))?;
        self.write(relative, &mut file)
    }

    /// Stream `reader` to a new file at `relative`, durably and without
    /// replacing anything already there.
    fn write(&self, relative: &str, reader: &mut dyn Read) -> Result<()> {
        let operation = self.operation;
        let destination = resolve_for_write(&self.engine.root, relative, operation)?;
        let _guard = self.engine.write_lock(operation)?;
        self.engine.collaboration_guard_file_mutation(relative)?;
        let mut file = atomic_write_file::AtomicWriteFile::open(&destination)
            .map_err(|error| CoreError::io(error, operation, Some(relative)))?;
        std::io::copy(reader, &mut file)
            .and_then(|_| file.sync_all())
            .map_err(|error| CoreError::io(error, operation, Some(relative)))?;
        // Checked under the lock, just before the rename, so a file that
        // appeared meanwhile is never replaced.
        if std::fs::symlink_metadata(&destination).is_ok() {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A file already exists at the destination",
                operation,
            ));
        }
        file.commit()
            .map_err(|error| CoreError::io(error, operation, Some(relative)))?;
        // The import holds the change gate, so the entry is in place before
        // the watcher thread checks any report of the file.
        self.engine.expect_own_file(relative);
        sync_parent(&destination, operation)
    }
}
