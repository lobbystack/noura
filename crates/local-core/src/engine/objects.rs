//! Managed objects: create, adopt, read, update, move, and delete the
//! Markdown files that carry a stable ID.

use super::*;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CreateObjectInput {
    #[serde(rename = "type")]
    pub object_type: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[ts(optional = nullable)]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[ts(type = "Record<string, unknown>")]
    pub properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ObjectPatch {
    #[ts(optional = nullable)]
    pub title: Option<String>,
    #[ts(optional = nullable)]
    pub body: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[ts(type = "Record<string, unknown>")]
    pub properties: BTreeMap<String, serde_json::Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub remove_properties: Vec<String>,
    pub expected_revision: String,
}

impl WorkspaceEngine {
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
        let relative_path = input.relative_path.unwrap_or_else(|| {
            workspace_format::default_managed_object_path(&input.object_type, &input.title, &id)
        });
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
        self.collaboration_guard_file_mutation(&object.relative_path)?;
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
            EventSource::Application,
            serde_json::json!({"id":id,"from":old,"to":destination}),
        );
        Ok(MutationResult {
            value: object,
            revision: markdown::revision(&bytes),
            durability: "committed".into(),
            index_status,
            warnings,
            chat_revision: None,
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
        self.collaboration_guard_file_mutation(&object.relative_path)?;
        let bytes = std::fs::read(&source)
            .map_err(|error| CoreError::io(error, "object_delete", Some(&object.relative_path)))?;
        check_revision(&bytes, expected_revision, "object_delete")?;
        let trash_path = self.discard(&source, &object.relative_path, "object_delete")?;
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(object.relative_path.clone(), "<deleted>".into());
        }
        let result = self
            .index
            .lock()
            .map_err(|_| lock_error("object_delete"))
            .and_then(|mut index| index.remove_path(&object.relative_path));
        let (index_status, warnings) = self.index_outcome(result);
        self.emit(
            "object:deleted",
            EventSource::Application,
            serde_json::json!({"id":id,"path":object.relative_path,"trashPath":trash_path}),
        );
        Ok(MutationResult {
            value: object,
            revision: markdown::revision(&bytes),
            durability: "committed".into(),
            index_status,
            warnings,
            chat_revision: None,
        })
    }

    pub(super) fn commit_object(
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
            self.collaboration_guard_file_mutation(&object.relative_path)?;
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
        atomic_write_checked(&self.root, &relative, &bytes, expected, operation)?;
        if let Ok(mut journal) = self.self_writes.lock() {
            journal.insert(object.relative_path.clone(), revision.clone());
        }
        // Serialization normalizes the title and body (for example it trims
        // trailing blank lines). Index and return what the file now says, so
        // a client's next save starts from the canonical text instead of
        // conflicting with it.
        if let ParsedMarkdown::Managed(mut canonical) =
            markdown::parse_markdown(&object.relative_path, &bytes)
        {
            canonical.relative_path = object.relative_path.clone();
            object = canonical;
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
        self.emit(event,EventSource::Application,serde_json::json!({"id":object.id,"type":object.object_type,"path":object.relative_path,"revision":revision}));
        Ok(MutationResult {
            value: object,
            revision,
            durability: "committed".into(),
            index_status,
            warnings,
            chat_revision: None,
        })
    }

    pub(super) fn read_canonical_object(
        &self,
        id: &str,
        operation: &str,
    ) -> Result<(WorkspaceObject, Vec<u8>)> {
        let object_type = id.split('_').next().unwrap_or_default();
        if object_type.is_empty() || !valid_object_id(id, object_type) {
            return Err(CoreError::new(
                "invalid_object_id",
                ErrorCategory::Identity,
                "The object ID is invalid",
                operation,
            ));
        }
        self.read_canonical_file(id, operation)?.ok_or_else(|| {
            CoreError::validation("object_not_found", "The object does not exist", operation)
        })
    }

    /// Read the file that holds `id` and return its parsed object and bytes.
    ///
    /// Autosaves and chat steps call this on every write, so it checks only
    /// the one target file: when its bytes differ from the index, it
    /// re-indexes that file alone. It walks the workspace only when the
    /// watcher reported unreconciled changes, or when the indexed path no
    /// longer holds the object (an external move, delete, or ID edit).
    pub(super) fn read_canonical_file(
        &self,
        id: &str,
        operation: &str,
    ) -> Result<Option<(WorkspaceObject, Vec<u8>)>> {
        self.reconcile_if_needed()?;
        let mut walked = false;
        loop {
            let location = self
                .index
                .lock()
                .map_err(|_| lock_error(operation))?
                .object_location(id)?;
            let Some((relative, indexed_revision)) = location else {
                if walked {
                    return Ok(None);
                }
                self.reconcile()?;
                walked = true;
                continue;
            };
            let destination = resolve_for_write(&self.root, &relative, operation)?;
            let bytes = match std::fs::read(&destination) {
                Ok(bytes) => bytes,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && !walked => {
                    self.reconcile()?;
                    walked = true;
                    continue;
                }
                Err(error) => return Err(CoreError::io(error, operation, Some(&relative))),
            };
            let parsed = markdown::parse_markdown(&relative, &bytes);
            let object = match parsed {
                ParsedMarkdown::Managed(ref object) if object.id == id => object.clone(),
                _ if !walked => {
                    self.reconcile()?;
                    walked = true;
                    continue;
                }
                ParsedMarkdown::Managed(_) => {
                    return Err(CoreError::new(
                        "object_identity_changed",
                        ErrorCategory::Identity,
                        "The canonical file no longer has the expected stable ID",
                        operation,
                    ));
                }
                _ => {
                    return Err(CoreError::new(
                        "object_parse_failed",
                        ErrorCategory::Parse,
                        "The canonical Markdown file cannot be reconciled safely",
                        operation,
                    ));
                }
            };
            if markdown::revision(&bytes) != indexed_revision {
                let modified = std::fs::metadata(&destination)
                    .map(|metadata| modified_ns(&metadata))
                    .unwrap_or(0);
                self.apply_index_changes(
                    vec![(relative, bytes.clone(), modified, parsed)],
                    Vec::new(),
                    EventSource::Reconciliation,
                )?;
            }
            return Ok(Some((object, bytes)));
        }
    }
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
    ensure_rfc3339_timestamp(
        value,
        format!("{key} must be an RFC 3339 timestamp"),
        operation,
    )?;
    Ok(Some(value.to_owned()))
}

/// Client-supplied durable timestamps keep working files consistent; a
/// malformed value must fail the mutation instead of becoming frontmatter.
pub(super) fn ensure_rfc3339_timestamp(
    value: &str,
    message: String,
    operation: &str,
) -> Result<()> {
    value
        .parse::<jiff::Timestamp>()
        .map(|_| ())
        .map_err(|_| CoreError::validation("invalid_timestamp", message, operation))
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

pub(super) fn file_stem(path: &str) -> String {
    Path::new(path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Untitled")
        .replace(['-', '_'], " ")
}

pub(super) fn normalize_domain_properties(
    object_type: &str,
    properties: &mut BTreeMap<String, serde_json::Value>,
) -> Result<()> {
    if object_type == "task" {
        workspace_format::normalize_task_properties(properties).map_err(task_format_error)?;
    } else if object_type == "project" {
        workspace_format::normalize_project_properties(properties).map_err(task_format_error)?;
    }
    Ok(())
}

fn task_format_error(error: workspace_format::FormatError) -> CoreError {
    match error {
        workspace_format::FormatError::InvalidTaskStatus => CoreError::validation(
            "invalid_field",
            "Unsupported status value",
            "object_validate",
        ),
        workspace_format::FormatError::InvalidTaskStatusType => CoreError::validation(
            "invalid_field",
            "status must be a string",
            "object_validate",
        ),
        workspace_format::FormatError::InvalidTaskPriority => CoreError::validation(
            "invalid_field",
            "Unsupported priority value",
            "object_validate",
        ),
        workspace_format::FormatError::InvalidTaskPriorityType => CoreError::validation(
            "invalid_field",
            "priority must be a string",
            "object_validate",
        ),
        workspace_format::FormatError::InvalidTaskDue => CoreError::validation(
            "invalid_date",
            "due must use YYYY-MM-DD or RFC 3339 with an explicit offset",
            "object_validate",
        ),
        workspace_format::FormatError::InvalidTaskProject => CoreError::validation(
            "invalid_project_id",
            "Task project references use a stable project ID",
            "object_validate",
        ),
        workspace_format::FormatError::InvalidProjectStatus => CoreError::validation(
            "invalid_field",
            "Unsupported project status value",
            "object_validate",
        ),
        workspace_format::FormatError::InvalidProjectStatusType => CoreError::validation(
            "invalid_field",
            "project status must be a string",
            "object_validate",
        ),
        _ => CoreError::validation(
            "invalid_field",
            "Task metadata is invalid",
            "object_validate",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn task_mutations_use_portable_defaults_validation_and_canonical_bytes() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "task".into(),
                title: "Portable task".into(),
                body: String::new(),
                relative_path: Some("tasks/portable.md".into()),
                properties: BTreeMap::from([("custom".into(), serde_json::json!("kept"))]),
            })
            .unwrap();
        assert_eq!(created.value.properties["status"], "todo");
        assert_eq!(created.value.properties["priority"], "medium");

        let updated = engine
            .update_object(
                &created.value.id,
                ObjectPatch {
                    title: None,
                    body: None,
                    properties: BTreeMap::from([("status".into(), serde_json::json!("done"))]),
                    remove_properties: Vec::new(),
                    expected_revision: created.revision,
                },
            )
            .unwrap();
        assert_eq!(updated.value.properties["status"], "done");
        assert_eq!(updated.value.properties["custom"], "kept");
        assert!(
            std::fs::read_to_string(workspace.path().join("tasks/portable.md"))
                .unwrap()
                .contains("status: done")
        );

        let removed = engine
            .update_object(
                &created.value.id,
                ObjectPatch {
                    title: None,
                    body: None,
                    properties: BTreeMap::new(),
                    remove_properties: vec!["custom".into(), "status".into()],
                    expected_revision: updated.revision,
                },
            )
            .unwrap();
        assert_eq!(removed.value.properties["status"], "todo");
        assert!(!removed.value.properties.contains_key("custom"));
        assert!(
            !std::fs::read_to_string(workspace.path().join("tasks/portable.md"))
                .unwrap()
                .contains("custom:")
        );
    }

    #[test]
    fn project_creation_keeps_native_defaults_for_omitted_optional_values() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let input = serde_json::from_value(serde_json::json!({
            "type": "project",
            "title": "Portable project"
        }))
        .unwrap();

        let created = engine.create_object(input).unwrap();

        assert_eq!(created.value.body, "");
        assert_eq!(created.value.properties["status"], "planned");
        assert_eq!(created.value.properties.len(), 1);
        assert!(
            created
                .value
                .relative_path
                .starts_with("projects/portable-project--")
        );
        assert!(created.value.relative_path.ends_with("/project.md"));
        assert!(workspace.path().join(created.value.relative_path).is_file());
    }

    #[test]
    fn full_storage_lifecycle_survives_index_deletion() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let project = engine
            .create_object(CreateObjectInput {
                object_type: "project".into(),
                title: "Alpha proof project".into(),
                body: "projectquartz durable body".into(),
                relative_path: Some("projects/alpha/project.md".into()),
                properties: BTreeMap::from([
                    ("status".into(), serde_json::json!("active")),
                    ("start".into(), serde_json::json!("2026-09-01")),
                    ("end".into(), serde_json::json!("2026-09-04")),
                ]),
            })
            .unwrap();
        let task = engine
            .create_object(CreateObjectInput {
                object_type: "task".into(),
                title: "Alpha proof task".into(),
                body: "taskcobalt durable body".into(),
                relative_path: Some("projects/alpha/tasks/proof.md".into()),
                properties: BTreeMap::from([
                    ("status".into(), serde_json::json!("todo")),
                    ("priority".into(), serde_json::json!("high")),
                    ("due".into(), serde_json::json!("2026-09-02")),
                    (
                        "project".into(),
                        serde_json::json!(project.value.id.clone()),
                    ),
                ]),
            })
            .unwrap();
        let note = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Alpha proof note".into(),
                body: "Original noteamber body".into(),
                relative_path: Some("notes/proof.md".into()),
                properties: BTreeMap::from([("date".into(), serde_json::json!("2026-09-03"))]),
            })
            .unwrap();
        let path = workspace.path().join("notes/proof.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Original", "Externally edited");
        std::fs::write(&path, external).unwrap();
        engine.reconcile().unwrap();
        let current = engine.get_object(&note.value.id).unwrap().unwrap();
        assert_eq!(current.body, "Externally edited noteamber body");
        let moved = engine
            .move_object(&current.id, "archive/proof.md", &current.revision)
            .unwrap();
        assert_eq!(moved.value.id, note.value.id);

        let before_objects = engine
            .query_objects(None)
            .unwrap()
            .into_iter()
            .map(|object| {
                (
                    object.id,
                    object.object_type,
                    object.title,
                    object.relative_path,
                    object.body,
                    object.properties,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(before_objects.len(), 3);
        for query in ["projectquartz", "taskcobalt", "noteamber"] {
            assert_eq!(
                engine
                    .search(&SearchInput {
                        query: query.into(),
                        ..Default::default()
                    })
                    .unwrap()
                    .len(),
                1
            );
        }
        let before_calendar = engine.calendar("2026-09-01", "2026-09-05").unwrap();
        assert_eq!(before_calendar.len(), 3);
        assert!(
            before_calendar
                .iter()
                .any(|entry| entry.source_id == project.value.id && entry.end.is_some())
        );
        assert!(
            before_calendar
                .iter()
                .any(|entry| entry.source_id == task.value.id)
        );
        assert!(
            before_calendar
                .iter()
                .any(|entry| entry.source_id == note.value.id)
        );

        let index_path = engine.index_path().to_owned();
        drop(engine);
        std::fs::remove_file(index_path).unwrap();
        let rebuilt =
            WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
        let after_objects = rebuilt
            .query_objects(None)
            .unwrap()
            .into_iter()
            .map(|object| {
                (
                    object.id,
                    object.object_type,
                    object.title,
                    object.relative_path,
                    object.body,
                    object.properties,
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(after_objects, before_objects);
        assert_eq!(
            rebuilt
                .get_object(&note.value.id)
                .unwrap()
                .unwrap()
                .relative_path,
            "archive/proof.md"
        );
        for query in ["projectquartz", "taskcobalt", "noteamber"] {
            assert_eq!(
                rebuilt
                    .search(&SearchInput {
                        query: query.into(),
                        ..Default::default()
                    })
                    .unwrap()
                    .len(),
                1
            );
        }
        let after_calendar = rebuilt.calendar("2026-09-01", "2026-09-05").unwrap();
        assert_eq!(
            after_calendar
                .iter()
                .map(|entry| (
                    entry.source_id.as_str(),
                    entry.property.as_str(),
                    entry.start.as_str(),
                    entry.end.as_deref(),
                ))
                .collect::<Vec<_>>(),
            before_calendar
                .iter()
                .map(|entry| (
                    entry.source_id.as_str(),
                    entry.property.as_str(),
                    entry.start.as_str(),
                    entry.end.as_deref(),
                ))
                .collect::<Vec<_>>()
        );
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
}
