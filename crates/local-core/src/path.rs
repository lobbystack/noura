use std::path::{Component, Path, PathBuf};

use crate::{CoreError, Result};

pub fn validate_relative(path: &str, operation: &str) -> Result<PathBuf> {
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
    for (index, component) in value.components().enumerate() {
        if !matches!(component, Component::Normal(_)) {
            return Err(CoreError::validation(
                "unsafe_path",
                "Path traversal and special path components are not accepted",
                operation,
            ));
        }
        if operation.contains("object")
            && index == 0
            && matches!(
                component.as_os_str().to_str(),
                Some(".noura" | ".git" | "node_modules" | "target")
            )
        {
            return Err(CoreError::validation(
                "reserved_path",
                "Managed objects cannot be stored in an internal or generated directory",
                operation,
            ));
        }
    }
    if operation.contains("object")
        && value.extension().and_then(|value| value.to_str()) != Some("md")
    {
        return Err(CoreError::validation(
            "unsupported_extension",
            "Managed objects must use the .md extension",
            operation,
        ));
    }
    Ok(value.to_owned())
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
