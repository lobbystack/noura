//! Managed objects: create, adopt, read, update, move, and delete the
//! Markdown files that carry a stable ID.

use super::*;
use workspace_format::ObjectTypeRegistry;

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
        let id = new_object_id(&input.object_type);
        let relative_path = match input.relative_path {
            Some(relative_path) => relative_path,
            None => self.free_object_path(&input.object_type, &input.title),
        };
        let object = WorkspaceObject {
            id,
            object_type: input.object_type,
            title: workspace_format::title_from_path(&relative_path),
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

    /// Create an object whose type `schemas` may hold a schema for, as a
    /// plugin does for its own types. A registered type's properties pass
    /// its schema, which adds the defaults they lack, and its file takes
    /// the type's canonical order. A property that fails the schema stops
    /// the create with `invalid_property`, naming the property, and nothing
    /// is written. Other types keep their properties as given. The
    /// title names the file only when `input` gives no path: with one, the
    /// path's file name is the title, and `input.title` may be empty. The
    /// call returns once the file is durably on disk.
    pub fn create_object_checked(
        &self,
        input: CreateObjectInput,
        schemas: &ObjectTypeRegistry,
    ) -> Result<MutationResult<WorkspaceObject>> {
        const OPERATION: &str = "object_create";
        validate_object_type(&input.object_type)?;
        if input.relative_path.is_none() && input.title.trim().is_empty() {
            return Err(CoreError::validation(
                "title_required",
                "A title is required",
                OPERATION,
            ));
        }
        let now = now_rfc3339();
        let mut properties = input.properties;
        for key in ["id", "type", "created", "updated"] {
            properties.remove(key);
        }
        let relative_path = match input.relative_path {
            Some(relative_path) => relative_path,
            None => self.free_object_path(&input.object_type, &input.title),
        };
        let mut object = WorkspaceObject {
            id: new_object_id(&input.object_type),
            object_type: input.object_type,
            title: workspace_format::title_from_path(&relative_path),
            body: input.body,
            relative_path,
            revision: String::new(),
            created: Some(now.clone()),
            updated: Some(now),
            properties,
        };
        check_schema(&mut object, schemas)?;
        self.commit_object_with(object, None, "object:created", OPERATION, schemas)
    }

    /// Change the body or properties of the object with the stable ID
    /// `id`, as a plugin does for its own types. The file says what the
    /// object holds, whatever the index says, and it must still hold
    /// `expected_revision`. A registered type's properties pass its schema
    /// afterwards: a property that fails stops the update with
    /// `invalid_property` and leaves the file as it is. Each part nobody
    /// changed keeps its bytes, and a changed frontmatter takes the type's
    /// canonical order. A new title is refused; moving the file renames
    /// it.
    pub fn update_object_checked(
        &self,
        id: &str,
        patch: ObjectPatch,
        schemas: &ObjectTypeRegistry,
    ) -> Result<MutationResult<WorkspaceObject>> {
        const OPERATION: &str = "object_update";
        if patch.title.is_some() {
            return Err(CoreError::validation(
                "title_not_supported",
                "Move the file to change its title",
                OPERATION,
            ));
        }
        let (mut object, bytes) = self.read_canonical_object(id, OPERATION)?;
        check_revision(&bytes, &patch.expected_revision, OPERATION)?;
        let expected_revision = patch.expected_revision.clone();
        apply_patch(&mut object, patch);
        check_schema(&mut object, schemas)?;
        object.updated = Some(now_rfc3339());
        self.commit_object_with(
            object,
            Some(&expected_revision),
            "object:updated",
            OPERATION,
            schemas,
        )
    }

    /// The object with the stable ID `id`, read from its file: an outside
    /// edit or move the index hasn't caught up with yet still shows.
    pub fn read_object(&self, id: &str) -> Result<WorkspaceObject> {
        self.read_canonical_object(id, "object_get")
            .map(|(object, _)| object)
    }

    /// Every object of `object_type`, each as its file holds it now. The
    /// index finds the files, and a file that changed since the index read
    /// it is read again; one that no longer holds an object of this type is
    /// left out.
    pub fn list_objects(&self, object_type: &str) -> Result<Vec<WorkspaceObject>> {
        const OPERATION: &str = "object_list";
        self.reconcile_if_needed()?;
        let indexed = self
            .index
            .lock()
            .map_err(|_| lock_error(OPERATION))?
            .query_objects(Some(object_type))?;
        let mut objects = Vec::with_capacity(indexed.len());
        for object in indexed {
            let Ok(path) = resolve_for_write(&self.root, &object.relative_path, OPERATION) else {
                continue;
            };
            let Ok(bytes) = read_downloaded(&path, &object.relative_path, OPERATION) else {
                continue;
            };
            if markdown::revision(&bytes) == object.revision {
                objects.push(object);
                continue;
            }
            if let ParsedMarkdown::Managed(current) =
                markdown::parse_markdown(&object.relative_path, &bytes)
                && current.object_type == object_type
            {
                objects.push(current);
            }
        }
        Ok(objects)
    }

    pub fn adopt_markdown(
        &self,
        relative_path: &str,
        object_type: &str,
        expected_revision: &str,
    ) -> Result<MutationResult<WorkspaceObject>> {
        validate_object_type(object_type)?;
        let path = resolve_for_write(&self.root, relative_path, "object_adopt")?;
        let bytes = read_downloaded(&path, relative_path, "object_adopt")?;
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

    /// The default path for a new object titled `title`, or the first of
    /// `title 1`, `title 2` and so on whose file doesn't exist yet.
    fn free_object_path(&self, object_type: &str, title: &str) -> String {
        let base = workspace_format::normalize_file_name(title);
        let base = if base.is_empty() { "Untitled" } else { &base };
        let mut candidate = String::new();
        for number in 0u32..10_000 {
            let suffix = match number {
                0 => String::new(),
                _ => format!(" {number}"),
            };
            let room = workspace_format::MAX_FILE_NAME_CHARS - suffix.chars().count();
            let stem: String = base.chars().take(room).collect();
            let name = format!("{}{suffix}", stem.trim_end());
            candidate = workspace_format::default_managed_object_path(object_type, &name);
            if std::fs::symlink_metadata(self.root.join(&candidate)).is_err() {
                break;
            }
        }
        candidate
    }

    /// Change an object's body or properties. A new title renames its file
    /// in the same folder, since the title is the file's name. The file
    /// must still hold `expected_revision`, and each part nobody changed
    /// keeps its bytes.
    pub fn update_object(
        &self,
        id: &str,
        patch: ObjectPatch,
    ) -> Result<MutationResult<WorkspaceObject>> {
        // The index finds the file; the file says what the object holds, so
        // a stale index can't put old text back.
        let located = match self.get_object(id)? {
            Some(object) => object.relative_path,
            None => {
                self.read_canonical_object(id, "object_update")?
                    .0
                    .relative_path
            }
        };
        let path = resolve_for_write(&self.root, &located, "object_update")?;
        let bytes = read_downloaded(&path, &located, "object_update")?;
        check_revision(&bytes, &patch.expected_revision, "object_update")?;
        let mut object = match markdown::parse_markdown(&located, &bytes) {
            ParsedMarkdown::Managed(object) if object.id == id => object,
            _ => {
                return Err(CoreError::new(
                    "object_parse_failed",
                    ErrorCategory::Parse,
                    "The canonical Markdown file cannot be reconciled safely",
                    "object_update",
                ));
            }
        };
        let renamed = match &patch.title {
            Some(title) => {
                let name = workspace_format::normalize_file_name(title);
                if name.is_empty() {
                    return Err(CoreError::validation(
                        "title_required",
                        "A title is required",
                        "object_update",
                    ));
                }
                let destination = workspace_format::renamed_path(&object.relative_path, &name);
                (destination != object.relative_path).then_some(destination)
            }
            None => None,
        };
        if let Some(destination) = &renamed {
            let source = resolve_for_write(&self.root, &object.relative_path, "object_update")?;
            let target = resolve_for_write(&self.root, destination, "object_update")?;
            if std::fs::symlink_metadata(&target).is_ok() && !same_file(&source, &target) {
                return Err(CoreError::new(
                    "path_exists",
                    ErrorCategory::Conflict,
                    "A file already exists at the destination",
                    "object_update",
                ));
            }
        }
        let content_changed = patch.body.is_some()
            || !patch.properties.is_empty()
            || !patch.remove_properties.is_empty();
        if !content_changed {
            return match renamed {
                Some(destination) => self.move_object(id, &destination, &patch.expected_revision),
                None => Ok(MutationResult {
                    revision: markdown::revision(&bytes),
                    value: object,
                    durability: "committed".into(),
                    index_status: IndexStatus::Updated,
                    warnings: Vec::new(),
                    chat_revision: None,
                }),
            };
        }
        let expected_revision = patch.expected_revision.clone();
        apply_content(&mut object, patch);
        let committed = self.commit_object(
            object,
            Some(&expected_revision),
            "object:updated",
            "object_update",
        )?;
        match renamed {
            Some(destination) => self.move_object(id, &destination, &committed.revision),
            None => Ok(committed),
        }
    }

    /// Change the body or properties of the managed object in the file at
    /// `relative_path`, as a note's properties panel does. The file says
    /// which object it holds, not the index, so two files that share an
    /// ID, as after a copy in the file manager, each take their own edits.
    /// The file must still hold `expected_revision`, each part nobody
    /// changed keeps its bytes, and `updated` takes the time of the edit.
    /// A new title is refused: renaming the file changes it.
    pub fn update_object_at(
        &self,
        relative_path: &str,
        patch: ObjectPatch,
    ) -> Result<MutationResult<WorkspaceObject>> {
        const OPERATION: &str = "object_update";
        if patch.title.is_some() {
            return Err(CoreError::validation(
                "title_not_supported",
                "Rename the file to change its title",
                OPERATION,
            ));
        }
        crate::path::validate_relative(relative_path, OPERATION)?;
        let path = resolve_for_write(&self.root, relative_path, OPERATION)?;
        if !path.exists() && !icloud_placeholder(&path).exists() {
            return Err(CoreError::validation(
                "object_not_found",
                "The object does not exist",
                OPERATION,
            ));
        }
        let bytes = read_downloaded(&path, relative_path, OPERATION)?;
        check_revision(&bytes, &patch.expected_revision, OPERATION)?;
        let mut object = match markdown::parse_markdown(relative_path, &bytes) {
            ParsedMarkdown::Managed(object) => object,
            _ => {
                return Err(CoreError::new(
                    "object_parse_failed",
                    ErrorCategory::Parse,
                    "The file no longer holds a managed object",
                    OPERATION,
                ));
            }
        };
        let expected_revision = patch.expected_revision.clone();
        apply_content(&mut object, patch);
        self.commit_object(
            object,
            Some(&expected_revision),
            "object:updated",
            OPERATION,
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
        let bytes = read_downloaded(&source, &object.relative_path, "object_move")?;
        check_revision(&bytes, expected_revision, "object_move")?;
        if destination_path.exists() {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A file already exists at the destination",
                "object_move",
            ));
        }
        let old = object.relative_path.clone();
        // Before the folders exist, so the watcher's reports of them read
        // as ours too.
        self.expect_own_write(&old, None);
        self.expect_own_write(destination, Some(&markdown::revision(&bytes)));
        let moved = destination_path
            .parent()
            .map_or(Ok(()), |parent| create_folders(parent, "object_move"))
            .and_then(|()| {
                std::fs::rename(&source, &destination_path)
                    .map_err(|error| CoreError::io(error, "object_move", Some(destination)))
            });
        if let Err(error) = moved {
            self.forget_own_write(&old);
            self.forget_own_write(destination);
            return Err(error);
        }
        sync_rename_parents(&source, &destination_path, "object_move")?;
        object.relative_path = destination.into();
        object.title = workspace_format::title_from_path(destination);
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
        let bytes = read_downloaded(&source, &object.relative_path, "object_delete")?;
        check_revision(&bytes, expected_revision, "object_delete")?;
        self.expect_own_write(&object.relative_path, None);
        let trash_path = self
            .discard(&source, &object.relative_path, "object_delete")
            .inspect_err(|_| self.forget_own_write(&object.relative_path))?;
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
        object: WorkspaceObject,
        expected: Option<&str>,
        event: &str,
        operation: &str,
    ) -> Result<MutationResult<WorkspaceObject>> {
        self.commit_object_with(
            object,
            expected,
            event,
            operation,
            &ObjectTypeRegistry::new(),
        )
    }

    /// [`Self::commit_object`], with a registered type's frontmatter in the
    /// canonical order its schema in `schemas` gives.
    fn commit_object_with(
        &self,
        mut object: WorkspaceObject,
        expected: Option<&str>,
        event: &str,
        operation: &str,
        schemas: &ObjectTypeRegistry,
    ) -> Result<MutationResult<WorkspaceObject>> {
        let relative = crate::path::validate_relative(&object.relative_path, operation)?;
        let destination = resolve_for_write(&self.root, &object.relative_path, operation)?;
        let _guard = self.write_lock(operation)?;
        let bytes = if let Some(expected) = expected {
            self.collaboration_guard_file_mutation(&object.relative_path)?;
            let current = read_downloaded(&destination, &object.relative_path, operation)?;
            check_revision(&current, expected, operation)?;
            // Only what changed gets new bytes: the rest of the file, its
            // byte-order mark and line endings stay as they are.
            schemas
                .serialize_over(&current, &object)
                .map_err(markdown::serialize_error)?
        } else if destination.exists() {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A file already exists at the requested path",
                operation,
            ));
        } else {
            schemas
                .serialize(&object)
                .map_err(markdown::serialize_error)?
        };
        let revision = markdown::revision(&bytes);
        self.expect_own_write(&object.relative_path, Some(&revision));
        atomic_write_checked(&self.root, &relative, &bytes, expected, operation)
            .inspect_err(|_| self.forget_own_write(&object.relative_path))?;
        // The file names the object and the parser reads the body's line
        // breaks as LF. Index and return what the file now says, so a
        // client's next save starts from it instead of conflicting with it.
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
            ensure_downloaded(&destination, &relative, operation)?;
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

/// Put a patch's body and properties into `object`, leaving the properties
/// Noura keeps for itself alone, and stamp `updated`.
fn apply_content(object: &mut WorkspaceObject, patch: ObjectPatch) {
    apply_patch(object, patch);
    object.updated = Some(now_rfc3339());
}

/// Put a patch's body and properties into `object`, leaving the properties
/// Noura keeps for itself alone.
fn apply_patch(object: &mut WorkspaceObject, patch: ObjectPatch) {
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
}

/// Check `object`'s properties against its type's schema in `schemas`,
/// adding the defaults it lacks. A type without a schema keeps its
/// properties as they are. A failing property comes back as
/// `invalid_property`, naming it and the workspace format's code for what
/// is wrong, and leaves `object` as it was.
fn check_schema(object: &mut WorkspaceObject, schemas: &ObjectTypeRegistry) -> Result<()> {
    schemas.normalize(object).map_err(|error| {
        let details = serde_json::to_value(&error).unwrap_or_default();
        let property = details["property"].as_str().unwrap_or_default().to_owned();
        let mut core = CoreError::validation(
            "invalid_property",
            format!("{property} doesn't fit the object type's schema: {error}"),
            "object_validate",
        );
        core.details = Some(serde_json::json!({
            "property": property,
            "code": details["code"],
        }));
        core
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_task_keeps_its_properties_as_written_without_a_plugins_schema() {
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
        // The kernel adds no task defaults: the Tasks plugin's schema does.
        assert_eq!(
            created.value.properties,
            BTreeMap::from([("custom".into(), serde_json::json!("kept"))])
        );

        // Nor does it check task values.
        let updated = engine
            .update_object(
                &created.value.id,
                ObjectPatch {
                    title: None,
                    body: None,
                    properties: BTreeMap::from([("status".into(), serde_json::json!("later"))]),
                    remove_properties: Vec::new(),
                    expected_revision: created.revision,
                },
            )
            .unwrap();
        assert_eq!(updated.value.properties["status"], "later");
        assert_eq!(updated.value.properties["custom"], "kept");
        let written = std::fs::read_to_string(workspace.path().join("tasks/portable.md")).unwrap();
        assert!(
            written.contains("custom: kept\nstatus: later\n"),
            "{written}"
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
        assert!(removed.value.properties.is_empty());
        let written = std::fs::read_to_string(workspace.path().join("tasks/portable.md")).unwrap();
        assert!(!written.contains("custom:") && !written.contains("status:"));
    }

    #[test]
    fn a_project_without_a_path_goes_in_the_folder_named_after_its_type() {
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
        assert!(created.value.properties.is_empty());
        assert_eq!(created.value.relative_path, "projects/Portable project.md");
        assert_eq!(created.value.title, "Portable project");
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
        let in_project = crate::ObjectFilter {
            property: Some(crate::PropertyValue {
                name: "project".into(),
                value: serde_json::json!(project.value.id.clone()),
            }),
            ..crate::ObjectFilter::default()
        };
        let ids = |objects: Vec<WorkspaceObject>| {
            objects
                .into_iter()
                .map(|object| object.id)
                .collect::<Vec<_>>()
        };
        let before_in_project = ids(engine.query_objects_filtered(&in_project).unwrap());
        assert_eq!(before_in_project, [task.value.id.as_str()]);

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
        assert_eq!(
            ids(rebuilt.query_objects_filtered(&in_project).unwrap()),
            before_in_project
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

    fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        (workspace, app_data, engine)
    }

    #[test]
    fn new_objects_are_named_after_their_title_and_skip_taken_names() {
        let (workspace, _app_data, engine) = engine();
        let create = |title: &str| {
            engine
                .create_object(CreateObjectInput {
                    object_type: "note".into(),
                    title: title.into(),
                    body: "Body\n".into(),
                    relative_path: None,
                    properties: BTreeMap::new(),
                })
                .unwrap()
                .value
        };
        let first = create("Team: sync");
        let second = create("Team sync");
        assert_eq!(
            (first.relative_path.as_str(), first.title.as_str()),
            ("notes/Team sync.md", "Team sync")
        );
        assert_eq!(
            (second.relative_path.as_str(), second.title.as_str()),
            ("notes/Team sync 1.md", "Team sync 1")
        );
        let bytes = std::fs::read_to_string(workspace.path().join("notes/Team sync.md")).unwrap();
        assert!(bytes.ends_with("---\nBody\n") && !bytes.contains("# "));
    }

    #[test]
    fn a_new_title_renames_the_file_and_keeps_its_id_and_bytes() {
        let (workspace, _app_data, engine) = engine();
        let bytes = b"---\r\nid: note_01j00000000000000000000000\r\ntype: note\r\n---\r\nKept\r\n";
        std::fs::write(workspace.path().join("Old.md"), bytes).unwrap();
        engine.reconcile().unwrap();
        let renamed = engine
            .update_object(
                "note_01j00000000000000000000000",
                ObjectPatch {
                    title: Some(" New: name ".into()),
                    body: None,
                    properties: BTreeMap::new(),
                    remove_properties: Vec::new(),
                    expected_revision: markdown::revision(bytes),
                },
            )
            .unwrap();
        assert_eq!(renamed.value.relative_path, "New name.md");
        assert_eq!(renamed.value.title, "New name");
        assert_eq!(renamed.value.id, "note_01j00000000000000000000000");
        assert!(!workspace.path().join("Old.md").exists());
        assert_eq!(
            std::fs::read(workspace.path().join("New name.md")).unwrap(),
            bytes
        );
    }

    #[test]
    fn a_title_never_moves_the_file_out_of_its_folder() {
        let (workspace, _app_data, engine) = engine();
        let bytes = b"---\nid: note_01j00000000000000000000000\ntype: note\n---\nBody";
        std::fs::create_dir(workspace.path().join("Docs")).unwrap();
        std::fs::write(workspace.path().join("Docs/Old.md"), bytes).unwrap();
        engine.reconcile().unwrap();
        let renamed = engine
            .update_object(
                "note_01j00000000000000000000000",
                ObjectPatch {
                    title: Some("../../outside".into()),
                    body: None,
                    properties: BTreeMap::new(),
                    remove_properties: Vec::new(),
                    expected_revision: markdown::revision(bytes),
                },
            )
            .unwrap();
        assert_eq!(renamed.value.relative_path, "Docs/outside.md");
        assert_eq!(
            std::fs::read(workspace.path().join("Docs/outside.md")).unwrap(),
            bytes
        );
    }

    #[test]
    fn a_title_taken_in_the_folder_changes_nothing() {
        let (workspace, _app_data, engine) = engine();
        let bytes = b"---\nid: note_01j00000000000000000000000\ntype: note\n---\nBody";
        std::fs::write(workspace.path().join("Old.md"), bytes).unwrap();
        std::fs::write(workspace.path().join("Taken.md"), b"other").unwrap();
        engine.reconcile().unwrap();
        let error = engine
            .update_object(
                "note_01j00000000000000000000000",
                ObjectPatch {
                    title: Some("Taken".into()),
                    body: Some("Changed".into()),
                    properties: BTreeMap::new(),
                    remove_properties: Vec::new(),
                    expected_revision: markdown::revision(bytes),
                },
            )
            .unwrap_err();
        assert_eq!(error.code, "path_exists");
        assert_eq!(
            std::fs::read(workspace.path().join("Old.md")).unwrap(),
            bytes
        );
        assert_eq!(
            std::fs::read(workspace.path().join("Taken.md")).unwrap(),
            b"other"
        );
    }

    #[test]
    fn a_property_edit_keeps_the_body_line_endings_and_byte_order_mark() {
        let (workspace, _app_data, engine) = engine();
        let bytes = "\u{feff}---\r\nid: note_01j00000000000000000000000\r\ntype: note\r\n---\r\n\r\n# Heading\r\nNo final newline";
        std::fs::write(workspace.path().join("Note.md"), bytes).unwrap();
        engine.reconcile().unwrap();
        let updated = engine
            .update_object(
                "note_01j00000000000000000000000",
                ObjectPatch {
                    title: None,
                    body: None,
                    properties: BTreeMap::from([("status".into(), serde_json::json!("done"))]),
                    remove_properties: Vec::new(),
                    expected_revision: markdown::revision(bytes.as_bytes()),
                },
            )
            .unwrap();
        let written = std::fs::read_to_string(workspace.path().join("Note.md")).unwrap();
        assert!(written.starts_with("\u{feff}---\r\nid: note_01j00000000000000000000000\r\ntype: note\r\nstatus: done\r\nupdated: "));
        assert!(written.ends_with("\r\n---\r\n\r\n# Heading\r\nNo final newline"));
        assert_eq!(updated.revision, markdown::revision(written.as_bytes()));
    }

    #[test]
    fn a_stale_index_never_writes_old_text_back() {
        let (workspace, _app_data, engine) = engine();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Note".into(),
                body: "Indexed\n".into(),
                relative_path: Some("Note.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        // Another app changes the body and the index hasn't seen it yet; a
        // property edit made against the file's new bytes keeps that body.
        let path = workspace.path().join("Note.md");
        let changed = std::fs::read_to_string(&path)
            .unwrap()
            .replace("Indexed", "Changed outside");
        std::fs::write(&path, &changed).unwrap();
        engine
            .dirty
            .store(false, std::sync::atomic::Ordering::SeqCst);
        engine
            .update_object(
                &created.value.id,
                ObjectPatch {
                    title: None,
                    body: None,
                    properties: BTreeMap::from([("tag".into(), serde_json::json!("x"))]),
                    remove_properties: Vec::new(),
                    expected_revision: markdown::revision(changed.as_bytes()),
                },
            )
            .unwrap();
        assert!(
            std::fs::read_to_string(&path)
                .unwrap()
                .ends_with("---\nChanged outside\n")
        );
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
