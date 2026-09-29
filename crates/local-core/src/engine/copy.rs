//! Duplicating a file. The copy of a note, task or project gets a new ID so
//! both files keep their own identity; any other file is copied byte for byte.

use super::*;

impl WorkspaceEngine {
    /// Copy the file at `from` to `to`, which must not exist yet.
    ///
    /// A managed object's copy is a new object with a fresh ID, the same
    /// properties and body, and a title that follows the new file name. Any
    /// other file, including idless Markdown, is copied byte for byte. The copy
    /// is durable before this returns.
    pub fn copy_file(&self, from: &str, to: &str) -> Result<()> {
        let operation = "file_copy";
        let source = resolve_for_write(&self.root, from, operation)?;
        let destination = resolve_for_write(&self.root, to, operation)?;
        let metadata = std::fs::symlink_metadata(&source)
            .map_err(|error| CoreError::io(error, operation, Some(from)))?;
        if !metadata.is_file() {
            return Err(CoreError::validation(
                "file_not_found",
                "The source is not a file in this workspace",
                operation,
            ));
        }
        if std::fs::symlink_metadata(&destination).is_ok() {
            return Err(copy_destination_exists(operation));
        }
        let bytes =
            std::fs::read(&source).map_err(|error| CoreError::io(error, operation, Some(from)))?;
        if let ParsedMarkdown::Managed(object) = markdown::parse_markdown(from, &bytes) {
            let title = file_stem(to);
            self.create_object(CreateObjectInput {
                object_type: object.object_type,
                title: if title.trim().is_empty() {
                    object.title
                } else {
                    title
                },
                body: object.body,
                relative_path: Some(to.to_owned()),
                properties: object.properties,
            })?;
            return Ok(());
        }
        let guard = self.write_lock(operation)?;
        self.collaboration_guard_file_mutation(to)?;
        // Checked again under the lock so a concurrent writer can't be
        // overwritten between the first check and the write.
        if std::fs::symlink_metadata(&destination).is_ok() {
            return Err(copy_destination_exists(operation));
        }
        let relative = destination.strip_prefix(&self.root).map_err(|_| {
            CoreError::validation(
                "unsafe_path",
                "The destination is outside the workspace",
                operation,
            )
        })?;
        atomic_write(&self.root, relative, &bytes, operation)?;
        drop(guard);
        self.reconcile_as_application()
    }
}

fn copy_destination_exists(operation: &str) -> CoreError {
    CoreError::new(
        "path_exists",
        ErrorCategory::Conflict,
        "A file already exists at the destination",
        operation,
    )
}
