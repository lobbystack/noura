//! Read and restore files that deletions and chat expiry moved to
//! `.noura/trash/<timestamp>/<original relative path>`.

use super::*;

const TRASH_ROOT: &str = ".noura/trash";
/// A trashed directory holding this file is one chat, restored as a unit.
const CHAT_MARKER: &str = "chat.md";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub enum TrashEntryKind {
    File,
    Chat,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct TrashEntry {
    /// Workspace-relative path inside `.noura/trash`, used to restore it.
    pub trash_path: String,
    /// Where the entry lived before it was trashed.
    pub original_path: String,
    /// RFC 3339 time the entry was trashed, when the folder name records it.
    pub deleted_at: Option<String>,
    pub kind: TrashEntryKind,
    /// Total size in bytes, including every file in a chat.
    #[ts(type = "number")]
    pub size: u64,
}

impl WorkspaceEngine {
    /// List trashed entries, newest first. Symlinks are skipped, never followed.
    pub fn trash_list(&self) -> Result<Vec<TrashEntry>> {
        let root = self.root.join(TRASH_ROOT);
        let mut entries = Vec::new();
        let batches = match std::fs::read_dir(&root) {
            Ok(batches) => batches,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(entries),
            Err(error) => return Err(CoreError::io(error, "trash_list", Some(TRASH_ROOT))),
        };
        for batch in batches {
            let batch = batch.map_err(|error| CoreError::io(error, "trash_list", None))?;
            let Some(stamp) = batch.file_name().to_str().map(str::to_owned) else {
                continue;
            };
            if !is_real_directory(&batch.path()) {
                continue;
            }
            collect_entries(
                &batch.path(),
                &stamp,
                Path::new(""),
                deleted_at(&stamp),
                &mut entries,
            )?;
        }
        entries.sort_by(|a, b| {
            b.deleted_at
                .cmp(&a.deleted_at)
                .then_with(|| a.original_path.cmp(&b.original_path))
        });
        Ok(entries)
    }

    /// Move a trashed entry back to its original path. Refuses to overwrite a
    /// file that now exists there, then reconciles so the index picks it up.
    pub fn trash_restore(&self, trash_path: &str) -> Result<TrashEntry> {
        let operation = "trash_restore";
        let entry = self
            .trash_list()?
            .into_iter()
            .find(|entry| entry.trash_path == trash_path)
            .ok_or_else(|| {
                CoreError::validation(
                    "trash_entry_not_found",
                    "The trashed item no longer exists",
                    operation,
                )
            })?;
        let source = resolve_for_write(&self.root, &entry.trash_path, "trash_write")?;
        // The original path must be an ordinary workspace path: restoring into
        // `.noura` or through a symlink is rejected like any other write.
        let destination = resolve_for_write(&self.root, &entry.original_path, operation)?;
        {
            let _guard = self.write_lock(operation)?;
            self.collaboration_guard_file_mutation(&entry.original_path)?;
            if std::fs::symlink_metadata(&destination).is_ok() {
                return Err(CoreError::validation(
                    "trash_restore_target_exists",
                    "A file already exists at the original location. Move or rename it, then try again.",
                    operation,
                ));
            }
            if let Some(parent) = destination.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|error| CoreError::io(error, operation, parent.to_str()))?;
            }
            std::fs::rename(&source, &destination).map_err(|error| {
                CoreError::io(error, operation, Some(entry.original_path.as_str()))
            })?;
            sync_rename_parents(&source, &destination, operation)?;
            remove_empty_parents(&source, &self.root.join(TRASH_ROOT));
        }
        // The move is durable at this point. A failed index update surfaces as
        // a diagnostic on the next reconciliation rather than undoing the file.
        let _ = self.reconcile();
        self.emit(
            "trash:restored",
            "application",
            serde_json::json!({ "path": entry.original_path, "trashPath": entry.trash_path }),
        );
        Ok(entry)
    }
}

fn collect_entries(
    directory: &Path,
    stamp: &str,
    relative: &Path,
    deleted_at: Option<String>,
    entries: &mut Vec<TrashEntry>,
) -> Result<()> {
    let children = std::fs::read_dir(directory)
        .map_err(|error| CoreError::io(error, "trash_list", directory.to_str()))?;
    for child in children {
        let child = child.map_err(|error| CoreError::io(error, "trash_list", None))?;
        let path = child.path();
        let Some(name) = child.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        let metadata = std::fs::symlink_metadata(&path)
            .map_err(|error| CoreError::io(error, "trash_list", path.to_str()))?;
        let child_relative = relative.join(&name);
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            if is_regular_file(&path.join(CHAT_MARKER)) {
                entries.push(entry(
                    stamp,
                    &child_relative,
                    deleted_at.clone(),
                    TrashEntryKind::Chat,
                    directory_size(&path),
                ));
            } else {
                collect_entries(&path, stamp, &child_relative, deleted_at.clone(), entries)?;
            }
        } else if metadata.is_file() {
            entries.push(entry(
                stamp,
                &child_relative,
                deleted_at.clone(),
                TrashEntryKind::File,
                metadata.len(),
            ));
        }
    }
    Ok(())
}

fn entry(
    stamp: &str,
    original: &Path,
    deleted_at: Option<String>,
    kind: TrashEntryKind,
    size: u64,
) -> TrashEntry {
    let original_path = original
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/");
    TrashEntry {
        trash_path: format!("{TRASH_ROOT}/{stamp}/{original_path}"),
        original_path,
        deleted_at,
        kind,
        size,
    }
}

/// Folder names are RFC 3339 timestamps with `:` and `.` replaced by `-`.
fn deleted_at(stamp: &str) -> Option<String> {
    let (date, time) = stamp.split_once('T')?;
    let time = time.strip_suffix('Z')?;
    let mut parts = time.splitn(4, '-');
    let (hours, minutes, seconds) = (parts.next()?, parts.next()?, parts.next()?);
    let fraction = parts.next();
    let value = match fraction {
        Some(fraction) => format!("{date}T{hours}:{minutes}:{seconds}.{fraction}Z"),
        None => format!("{date}T{hours}:{minutes}:{seconds}Z"),
    };
    value
        .parse::<jiff::Timestamp>()
        .ok()
        .map(|timestamp| timestamp.to_string())
}

fn is_real_directory(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.is_file())
}

fn directory_size(path: &Path) -> u64 {
    let Ok(children) = std::fs::read_dir(path) else {
        return 0;
    };
    children
        .flatten()
        .map(|child| match std::fs::symlink_metadata(child.path()) {
            Ok(metadata) if metadata.is_dir() => directory_size(&child.path()),
            Ok(metadata) if metadata.is_file() => metadata.len(),
            _ => 0,
        })
        .sum()
}

/// Remove directories the restore emptied, stopping at the trash root.
fn remove_empty_parents(restored: &Path, trash_root: &Path) {
    let mut cursor = restored.parent();
    while let Some(directory) = cursor {
        if directory == trash_root || !directory.starts_with(trash_root) {
            break;
        }
        if std::fs::remove_dir(directory).is_err() {
            break;
        }
        cursor = directory.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::deleted_at;

    #[test]
    fn trash_folder_names_convert_back_to_rfc3339() {
        assert_eq!(
            deleted_at("2026-09-28T21-42-50-123456789Z").as_deref(),
            Some("2026-09-28T21:42:50.123456789Z")
        );
        assert_eq!(
            deleted_at("2026-09-28T21-42-50Z").as_deref(),
            Some("2026-09-28T21:42:50Z")
        );
        assert_eq!(deleted_at("not-a-time"), None);
        assert_eq!(deleted_at("2026-13-40T99-99-99Z"), None);
    }
}
