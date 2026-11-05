//! Plugins' reads of the open workspace, through `local-core`.

use std::{path::Path, sync::Arc};

use local_core::{CoreError, ParsedMarkdown, WorkspaceEntryKind, WorkspaceObject, parse_markdown};
use plugin_host::{
    Entry, EntryKind, FileRead, Listed, ObjectRead, ServiceError, WorkspacePath, WorkspaceReads,
};

use crate::Workspace;

/// The largest file a plugin may read.
const MAX_READ_BYTES: u64 = 16 << 20;

/// Reads the open workspace for plugins through `local-core`, which keeps
/// every path inside the workspace.
pub(super) struct WorkspaceReader(pub(super) Arc<Workspace>);

impl WorkspaceReads for WorkspaceReader {
    fn list_entries(&self) -> Result<Vec<Listed>, ServiceError> {
        let (entries, unsupported) = self
            .0
            .engine()
            .list_workspace_entries_counting_unsupported()
            .map_err(|error| service_error(&error, ""))?;
        let mut listed = Vec::with_capacity(entries.len() + unsupported);
        for entry in entries {
            let Ok(path) = WorkspacePath::new(&entry.relative_path) else {
                listed.push(Listed::Unsupported);
                continue;
            };
            listed.push(Listed::Entry(Entry {
                path,
                kind: match entry.kind {
                    WorkspaceEntryKind::Folder => EntryKind::Folder,
                    _ => EntryKind::File,
                },
                object_id: entry.object_id,
                object_type: entry.object_type,
                revision: entry.revision,
            }));
        }
        listed.extend(std::iter::repeat_n(Listed::Unsupported, unsupported));
        Ok(listed)
    }

    fn read_file(&self, path: &WorkspacePath) -> Result<FileRead, ServiceError> {
        let (bytes, revision) = self
            .0
            .engine()
            .read_file(path.as_str(), MAX_READ_BYTES)
            .map_err(|error| service_error(&error, path.as_str()))?;
        Ok(FileRead {
            path: path.clone(),
            bytes,
            revision,
        })
    }

    fn read_object(&self, id: &str) -> Result<ObjectRead, ServiceError> {
        let object = self
            .0
            .engine()
            .get_object(id)
            .map_err(|error| service_error(&error, id))?
            .ok_or_else(|| ServiceError::NotFound(id.to_owned()))?;
        let path =
            WorkspacePath::new(&object.relative_path).map_err(|_| ServiceError::UnsupportedPath)?;
        Ok(object_read(object, path))
    }

    fn read_object_at(&self, path: &WorkspacePath) -> Result<ObjectRead, ServiceError> {
        let (bytes, revision) = self
            .0
            .engine()
            .read_file(path.as_str(), MAX_READ_BYTES)
            .map_err(|error| service_error(&error, path.as_str()))?;
        match parse_markdown(path.as_str(), &bytes) {
            ParsedMarkdown::Managed(object) => Ok(object_read(
                WorkspaceObject { revision, ..object },
                path.clone(),
            )),
            _ => Err(ServiceError::NotFound(path.to_string())),
        }
    }
}

/// An object as a plugin reads it, from its file at `path`.
fn object_read(object: WorkspaceObject, path: WorkspacePath) -> ObjectRead {
    // An object's title is its file name without the extension.
    let title = Path::new(path.as_str())
        .file_stem()
        .and_then(|stem| stem.to_str())
        .unwrap_or_default()
        .to_owned();
    ObjectRead {
        id: object.id,
        object_type: object.object_type,
        title,
        path,
        body: object.body,
        properties: serde_json::to_string(&object.properties).unwrap_or_else(|_| "{}".to_owned()),
        revision: object.revision,
    }
}

/// A `local-core` error as a plugin sees it, with the details logged.
fn service_error(error: &CoreError, what: &str) -> ServiceError {
    match error.code.as_str() {
        "empty_path" | "absolute_path" | "unsafe_path" | "reserved_path" | "symlink_escape" => {
            ServiceError::InvalidPath(what.to_owned())
        }
        "unsupported_path" | "non_utf8_path" => ServiceError::UnsupportedPath,
        "file_not_found" | "object_not_found" => ServiceError::NotFound(what.to_owned()),
        // More than one file holds the ID `read_object` asked for.
        "identity_conflict" => ServiceError::IdentityConflict(what.to_owned()),
        "file_too_large" => ServiceError::Failed("The file is larger than 16 MB.".to_owned()),
        "file_not_downloaded" => ServiceError::Failed(error.message.clone()),
        code => {
            tracing::warn!(code, operation = %error.operation, "a plugin's workspace read failed");
            ServiceError::Failed("Noura couldn't read the workspace.".to_owned())
        }
    }
}
