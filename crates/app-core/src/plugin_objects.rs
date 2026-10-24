//! Objects of plugins' own types, written through `local-core` for the
//! plugin host. The host passes a call here only for a type the calling
//! plugin registered. Each write checks the type's schema, carries the
//! revision the plugin read, and returns once the file is durably on disk.
//! Reads come from the files, whatever the index says.

use std::sync::Arc;

use local_core::{CoreError, CreateObjectInput, ObjectPatch, WorkspaceObject};
use plugin_host::{
    NewObject, ObjectChanges, ObjectRecord, ObjectTypes, PropertyProblem, ServiceError,
    WorkspaceObjects, WorkspacePath,
};

use crate::Workspace;

/// The folders Noura keeps to itself. A plugin's path that starts with one
/// in any letter case is refused, since macOS and Windows match names
/// without case.
const RESERVED_FOLDERS: [&str; 4] = [".noura", ".git", "node_modules", "target"];

pub(crate) struct ObjectStore {
    workspace: Arc<Workspace>,
    types: ObjectTypes,
}

impl ObjectStore {
    pub(crate) fn new(workspace: Arc<Workspace>, types: ObjectTypes) -> Self {
        Self { workspace, types }
    }

    fn record(&self, object: WorkspaceObject) -> Result<ObjectRecord, ServiceError> {
        let path =
            WorkspacePath::new(&object.relative_path).map_err(|_| ServiceError::UnsupportedPath)?;
        let problem = self.types.problem(&object);
        Ok(ObjectRecord {
            title: local_core::title_from_path(&object.relative_path),
            properties: serde_json::to_string(&object.properties)
                .unwrap_or_else(|_| "{}".to_owned()),
            id: object.id,
            object_type: object.object_type,
            path,
            body: object.body,
            revision: object.revision,
            created: object.created,
            updated: object.updated,
            problem,
        })
    }

    /// Find the object's file from what it holds, so a stale index can't
    /// send a move or a delete to the wrong place.
    fn locate(&self, id: &str) -> Result<WorkspaceObject, ServiceError> {
        self.workspace
            .engine()
            .read_object(id)
            .map_err(|error| service_error(&error, id))
    }
}

impl WorkspaceObjects for ObjectStore {
    fn create(&self, object: NewObject) -> Result<ObjectRecord, ServiceError> {
        let path = object.path.as_ref().map(checked_path).transpose()?;
        let shown = path.clone().unwrap_or_else(|| object.title.clone());
        let input = CreateObjectInput {
            object_type: object.object_type,
            title: object.title,
            body: object.body,
            relative_path: path,
            properties: object.properties.into_iter().collect(),
        };
        let created = self
            .workspace
            .engine()
            .create_object_checked(input, &self.types.schemas())
            .map_err(|error| service_error(&error, &shown))?;
        self.record(created.value)
    }

    fn get(&self, id: &str) -> Result<ObjectRecord, ServiceError> {
        self.record(self.locate(id)?)
    }

    /// Every object of the type whose path a plugin can hold. One whose
    /// file name a plugin path can't carry, such as a name with a control
    /// character, is left out, so it can't fail the others.
    fn list(&self, object_type: &str) -> Result<Vec<ObjectRecord>, ServiceError> {
        let objects = self
            .workspace
            .engine()
            .list_objects(object_type)
            .map_err(|error| service_error(&error, object_type))?;
        let mut records = Vec::with_capacity(objects.len());
        for object in objects {
            match self.record(object) {
                Ok(record) => records.push(record),
                Err(_) => tracing::warn!(
                    object_type,
                    "a plugin's list left out an object whose path it can't hold"
                ),
            }
        }
        Ok(records)
    }

    fn update(
        &self,
        id: &str,
        expected_revision: &str,
        changes: ObjectChanges,
    ) -> Result<ObjectRecord, ServiceError> {
        let patch = ObjectPatch {
            title: None,
            body: changes.body,
            properties: changes.properties.into_iter().collect(),
            remove_properties: changes.remove,
            expected_revision: expected_revision.to_owned(),
        };
        let updated = self
            .workspace
            .engine()
            .update_object_checked(id, patch, &self.types.schemas())
            .map_err(|error| service_error(&error, id))?;
        self.record(updated.value)
    }

    fn move_to(
        &self,
        id: &str,
        expected_revision: &str,
        path: &WorkspacePath,
    ) -> Result<ObjectRecord, ServiceError> {
        let destination = checked_path(path)?;
        self.locate(id)?;
        let moved = self
            .workspace
            .engine()
            .move_object(id, &destination, expected_revision)
            .map_err(|error| match error.code.as_str() {
                "revision_conflict" | "identity_conflict" | "object_not_found" => {
                    service_error(&error, id)
                }
                _ => service_error(&error, &destination),
            })?;
        let mut object = moved.value;
        object.revision = moved.revision;
        self.record(object)
    }

    fn delete(&self, id: &str, expected_revision: &str) -> Result<(), ServiceError> {
        self.locate(id)?;
        self.workspace
            .engine()
            .delete_object(id, expected_revision)
            .map(|_| ())
            .map_err(|error| service_error(&error, id))
    }
}

/// `path` as a workspace-relative string, refused when it names a folder
/// Noura keeps to itself in any letter case.
fn checked_path(path: &WorkspacePath) -> Result<String, ServiceError> {
    let first = path.as_str().split('/').next().unwrap_or_default();
    if RESERVED_FOLDERS
        .iter()
        .any(|reserved| first.eq_ignore_ascii_case(reserved))
    {
        return Err(ServiceError::InvalidPath(path.as_str().to_owned()));
    }
    Ok(path.as_str().to_owned())
}

/// A `local-core` error as a plugin sees it, with the details logged.
/// `what` is the ID, type or path the call named.
fn service_error(error: &CoreError, what: &str) -> ServiceError {
    match error.code.as_str() {
        "revision_conflict" => ServiceError::RevisionConflict(what.to_owned()),
        "identity_conflict" => ServiceError::IdentityConflict(what.to_owned()),
        "invalid_property" => {
            let details = error.details.clone().unwrap_or_default();
            ServiceError::InvalidProperty(PropertyProblem {
                property: details["property"].as_str().unwrap_or_default().to_owned(),
                code: details["code"].as_str().unwrap_or_default().to_owned(),
            })
        }
        "path_exists" => ServiceError::AlreadyExists(what.to_owned()),
        "empty_path"
        | "absolute_path"
        | "unsafe_path"
        | "reserved_path"
        | "symlink_escape"
        | "unsupported_extension" => ServiceError::InvalidPath(what.to_owned()),
        "unsupported_path" | "non_utf8_path" => ServiceError::UnsupportedPath,
        "object_not_found" | "file_not_found" | "invalid_object_id" => {
            ServiceError::NotFound(what.to_owned())
        }
        "title_required" => ServiceError::Failed("A title is required.".to_owned()),
        "file_not_downloaded" => ServiceError::Failed(error.message.clone()),
        "object_parse_failed" | "object_identity_changed" => ServiceError::Failed(
            "The object's file doesn't hold its properties anymore.".to_owned(),
        ),
        code => {
            tracing::warn!(code, operation = %error.operation, "a plugin's object call failed");
            ServiceError::Failed("Noura couldn't finish the object call.".to_owned())
        }
    }
}
