use std::path::{Component, Path, PathBuf};

use crate::{CoreError, Result};
use workspace_format::{ManagedObjectPathError, validate_managed_object_path};

/// Internal operations are the only callers allowed to name reserved
/// directories. User- and plugin-facing mutations must stay out of them.
fn may_use_internal_paths(operation: &str) -> bool {
    matches!(
        operation,
        "workspace_create"
            | "trash_write"
            | "chat_mutation_recover"
            | "chat_expire"
            | "history_snapshot"
            | "sync"
    )
}

pub fn validate_relative(path: &str, operation: &str) -> Result<PathBuf> {
    if operation.contains("object") {
        validate_managed_object_path(path)
            .map_err(|error| managed_object_path_error(error, operation))?;
    }
    if path.is_empty() {
        return Err(CoreError::validation(
            "empty_path",
            "A relative path is required",
            operation,
        ));
    }
    let value = Path::new(path);
    // Windows doesn't call `/etc/hosts` absolute, since it names no drive.
    if value.is_absolute() || value.has_root() {
        return Err(CoreError::validation(
            "absolute_path",
            "Absolute paths are not accepted",
            operation,
        ));
    }
    for (index, component) in value.components().enumerate() {
        if !matches!(component, Component::Normal(_)) {
            return Err(CoreError::validation(
                "unsafe_path",
                "Path traversal and special path components are not accepted",
                operation,
            ));
        }
        if !may_use_internal_paths(operation)
            && index == 0
            && matches!(
                component.as_os_str().to_str(),
                Some(".noura" | ".git" | "node_modules" | "target")
            )
        {
            return Err(CoreError::validation(
                "reserved_path",
                "Files cannot be stored in an internal or generated directory",
                operation,
            ));
        }
    }
    Ok(value.to_owned())
}

fn managed_object_path_error(error: ManagedObjectPathError, operation: &str) -> CoreError {
    match error {
        ManagedObjectPathError::Empty => {
            CoreError::validation("empty_path", "A relative path is required", operation)
        }
        ManagedObjectPathError::Absolute => CoreError::validation(
            "absolute_path",
            "Absolute paths are not accepted",
            operation,
        ),
        ManagedObjectPathError::Unsafe => CoreError::validation(
            "unsafe_path",
            "Path traversal and special path components are not accepted",
            operation,
        ),
        ManagedObjectPathError::Reserved => CoreError::validation(
            "reserved_path",
            "Managed objects cannot be stored in an internal or generated directory",
            operation,
        ),
        ManagedObjectPathError::UnsupportedExtension => CoreError::validation(
            "unsupported_extension",
            "Managed objects must use the .md extension",
            operation,
        ),
    }
}

pub fn resolve_for_write(root: &Path, relative: &str, operation: &str) -> Result<PathBuf> {
    let relative = validate_relative(relative, operation)?;
    let mut cursor = root.to_owned();
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            cursor.push(component);
            if cursor.exists() {
                let metadata = std::fs::symlink_metadata(&cursor).map_err(|error| {
                    CoreError::io(error, operation, Some(relative.to_string_lossy().as_ref()))
                })?;
                if metadata.file_type().is_symlink() {
                    return Err(CoreError::validation(
                        "symlink_escape",
                        "Mutations cannot traverse symlinks",
                        operation,
                    ));
                }
            }
        }
    }
    let destination = root.join(relative);
    if destination.exists() {
        let metadata = std::fs::symlink_metadata(&destination).map_err(|error| {
            CoreError::io(
                error,
                operation,
                Some(destination.to_string_lossy().as_ref()),
            )
        })?;
        if metadata.file_type().is_symlink() {
            return Err(CoreError::validation(
                "symlink_escape",
                "Mutations cannot target symlinks",
                operation,
            ));
        }
    }
    Ok(destination)
}

/// `relative` with each folder it names spelled as the folder already on
/// disk when only the letter case differs, so a new `Notes/Plan.md` lands
/// in an existing `notes/` folder instead of a second folder beside it, on
/// any file system. A folder that exists with the exact name wins, and a
/// folder that doesn't exist yet keeps the name it was given. The file
/// name itself keeps its case.
pub fn match_existing_folder_case(root: &Path, relative: &str) -> String {
    let mut parts: Vec<String> = relative.split('/').map(str::to_owned).collect();
    let Some((_, folders)) = parts.split_last_mut() else {
        return relative.to_owned();
    };
    let mut cursor = root.to_owned();
    for folder in folders.iter_mut() {
        let Ok(entries) = std::fs::read_dir(&cursor) else {
            break;
        };
        let mut folded = None;
        let mut exact = false;
        for entry in entries.flatten() {
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            if name == *folder {
                exact = true;
                break;
            }
            if folded.is_none() && name.to_lowercase() == folder.to_lowercase() {
                folded = Some(name);
            }
        }
        if !exact {
            match folded {
                Some(name) => *folder = name,
                None => break,
            }
        }
        cursor.push(&*folder);
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_files_join_a_folder_that_differs_only_in_case() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("notes/plans")).unwrap();
        assert_eq!(
            match_existing_folder_case(root.path(), "Notes/Plans/Plan.md"),
            "notes/plans/Plan.md"
        );
        assert_eq!(
            match_existing_folder_case(root.path(), "Tasks/Plan.md"),
            "Tasks/Plan.md"
        );
        assert_eq!(
            match_existing_folder_case(root.path(), "Plan.md"),
            "Plan.md"
        );
        assert_eq!(
            match_existing_folder_case(root.path(), "Notes/New/Plan.md"),
            "notes/New/Plan.md"
        );
    }

    #[test]
    fn managed_objects_require_markdown_outside_reserved_directories() {
        assert_eq!(
            validate_relative("note", "object_create").unwrap_err().code,
            "unsupported_extension"
        );
        assert_eq!(
            validate_relative(".noura/note.md", "object_create")
                .unwrap_err()
                .code,
            "reserved_path"
        );
        assert!(validate_relative("notes/note.md", "object_create").is_ok());
    }

    #[test]
    fn user_facing_mutations_reject_reserved_directories() {
        for operation in [
            "folder_create",
            "folder_move",
            "folder_remove",
            "raw_markdown_save",
            "managed_draft_save",
            "collaboration_move",
        ] {
            for path in [
                ".noura/note.md",
                ".git/config",
                "node_modules/pkg/index.md",
                "target/debug/note.md",
            ] {
                assert_eq!(
                    validate_relative(path, operation).unwrap_err().code,
                    "reserved_path",
                    "{operation} {path}"
                );
            }
        }
    }

    #[test]
    fn internal_operations_may_write_reserved_directories() {
        for (operation, path) in [
            ("workspace_create", ".noura/workspace.yaml"),
            ("trash_write", ".noura/trash/note.md"),
            ("chat_mutation_recover", ".noura/chat-mutations"),
            ("history_snapshot", ".noura/history/note/revision-local.md"),
            ("sync", ".noura/sync/state.json"),
        ] {
            assert!(
                validate_relative(path, operation).is_ok(),
                "{operation} {path}"
            );
        }
    }
}
