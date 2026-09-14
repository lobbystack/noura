use std::path::{Component, Path, PathBuf};

use crate::{CoreError, Result};
use workspace_format::{ManagedObjectPathError, validate_managed_object_path};

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
    if value.is_absolute() {
        return Err(CoreError::validation(
            "absolute_path",
            "Absolute paths are not accepted",
            operation,
        ));
    }
    for component in value.components() {
        if !matches!(component, Component::Normal(_)) {
            return Err(CoreError::validation(
                "unsafe_path",
                "Path traversal and special path components are not accepted",
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
