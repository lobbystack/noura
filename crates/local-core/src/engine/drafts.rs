//! Editor drafts of managed objects: three-way merges with external edits,
//! conflict resolution, and the recovery snapshots each resolution keeps.

use super::*;
use workspace_format::merge::{ManagedDraftFields, ManagedMergeConflict};
pub(super) use workspace_format::merge::{
    merge_markdown_body, merge_text as merge_markdown_text, normalized_properties,
};

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct DraftReconcileInput {
    pub id: String,
    pub base_revision: String,
    pub base_body: String,
    pub local_body: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum DraftReconcileResult {
    Unchanged {
        current: WorkspaceObject,
        body: String,
    },
    Merged {
        current: WorkspaceObject,
        body: String,
    },
    Conflict {
        current: WorkspaceObject,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ConflictResolution {
    UseExternal,
    ReplaceExternal,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ResolveConflictInput {
    pub id: String,
    pub current_revision: String,
    pub local_body: String,
    pub resolution: ConflictResolution,
}

/// Draft state captured by a client before a managed object was modified.
/// Title, body, and properties are reconciled independently during merges;
/// stable identity and canonical metadata always come from the file.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ManagedDraftInput {
    pub id: String,
    pub base_revision: String,
    pub base_title: String,
    pub base_body: String,
    #[ts(type = "Record<string, unknown>")]
    pub base_properties: BTreeMap<String, serde_json::Value>,
    pub local_title: String,
    pub local_body: String,
    #[ts(type = "Record<string, unknown>")]
    pub local_properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "status", rename_all = "kebab-case")]
pub enum ManagedDraftResult {
    Unchanged {
        current: WorkspaceObject,
    },
    Merged {
        current: WorkspaceObject,
        title: String,
        body: String,
        #[ts(type = "Record<string, unknown>")]
        properties: BTreeMap<String, serde_json::Value>,
    },
    Conflict {
        current: WorkspaceObject,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ManagedConflictResolution {
    UseExternal,
    ReplaceExternal,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ManagedConflictResolveInput {
    pub id: String,
    pub current_revision: String,
    pub relative_path: String,
    pub created: Option<String>,
    pub local_title: String,
    pub local_body: String,
    #[ts(type = "Record<string, unknown>")]
    pub local_properties: BTreeMap<String, serde_json::Value>,
    pub resolution: ManagedConflictResolution,
}

impl WorkspaceEngine {
    pub fn reconcile_note_draft(&self, input: DraftReconcileInput) -> Result<DraftReconcileResult> {
        let (current, current_bytes) = self.read_canonical_object(&input.id, "note_reconcile")?;
        if current.object_type != "note" {
            return Err(CoreError::validation(
                "object_type_mismatch",
                "Only note drafts can be reconciled",
                "note_reconcile",
            ));
        }
        if current.revision == input.base_revision {
            return Ok(DraftReconcileResult::Unchanged {
                current,
                body: input.local_body,
            });
        }
        match merge_markdown_body(&input.base_body, &input.local_body, &current.body) {
            Some(body) => {
                self.snapshot_bytes(&current.id, "external", &current_bytes)?;
                Ok(DraftReconcileResult::Merged { current, body })
            }
            None => Ok(DraftReconcileResult::Conflict { current }),
        }
    }

    pub fn resolve_note_conflict(
        &self,
        input: ResolveConflictInput,
    ) -> Result<MutationResult<WorkspaceObject>> {
        let (mut current, current_bytes) =
            self.read_canonical_object(&input.id, "note_conflict_resolve")?;
        if current.object_type != "note" {
            return Err(CoreError::validation(
                "object_type_mismatch",
                "Only note conflicts can be resolved",
                "note_conflict_resolve",
            ));
        }
        if current.revision != input.current_revision {
            let mut error = CoreError::new(
                "revision_conflict",
                ErrorCategory::Conflict,
                "The file changed again while the conflict was being reviewed",
                "note_conflict_resolve",
            );
            error.details = Some(serde_json::json!({"currentRevision": current.revision}));
            return Err(error);
        }
        match input.resolution {
            ConflictResolution::UseExternal => {
                let mut local = current.clone();
                local.body = input.local_body;
                let local_bytes = markdown::serialize_object(&local)?;
                self.snapshot_bytes(&current.id, "local", &local_bytes)?;
                Ok(MutationResult {
                    value: current.clone(),
                    revision: current.revision.clone(),
                    durability: "committed".into(),
                    index_status: IndexStatus::Updated,
                    warnings: Vec::new(),
                    chat_revision: None,
                })
            }
            ConflictResolution::ReplaceExternal => {
                self.snapshot_bytes(&current.id, "external", &current_bytes)?;
                current.body = input.local_body;
                current.updated = Some(now_rfc3339());
                self.commit_object(
                    current,
                    Some(&input.current_revision),
                    "object:updated",
                    "note_conflict_resolve",
                )
            }
        }
    }

    // --- Managed draft reconciliation (notes, tasks, and future types) ---

    /// Reconcile a managed draft against the canonical file without writing.
    /// Returns the merged draft content for clients that still need to render
    /// the combination, or an explicit conflict marker.
    pub fn reconcile_managed_draft(&self, input: ManagedDraftInput) -> Result<ManagedDraftResult> {
        let canonical =
            self.read_canonical_workspace_object(&input.id, "managed_draft_reconcile")?;
        if canonical.revision == input.base_revision
            && canonical.title == input.base_title
            && canonical.body == input.base_body
            && normalized_properties(&canonical.properties)
                == normalized_properties(&input.base_properties)
        {
            return Ok(ManagedDraftResult::Unchanged { current: canonical });
        }
        match merge_managed_fields(&input, &canonical) {
            Ok(merged) => Ok(ManagedDraftResult::Merged {
                current: canonical.clone(),
                title: merged.title,
                body: merged.body,
                properties: merged.properties.clone(),
            }),
            Err(_) => Ok(ManagedDraftResult::Conflict { current: canonical }),
        }
    }

    /// Reconcile and durably commit a managed draft in one operation. A clean
    /// base writes the local draft; a changed base merges field-by-field and
    /// snapshots every displaced version before replacing bytes atomically.
    pub fn save_managed_draft(&self, input: ManagedDraftInput) -> Result<ManagedDraftResult> {
        let canonical = self.read_canonical_workspace_object(&input.id, "managed_draft_save")?;
        let unchanged = canonical.revision == input.base_revision
            && canonical.title == input.base_title
            && canonical.body == input.base_body
            && normalized_properties(&canonical.properties)
                == normalized_properties(&input.base_properties);
        if unchanged {
            let mut object = canonical;
            let expected_revision = object.revision.clone();
            object.title = input.local_title;
            object.body = input.local_body;
            object.properties = normalized_properties(&input.local_properties);
            let result =
                self.apply_managed_object(object, Some(&expected_revision), "managed_draft_save")?;
            return Ok(ManagedDraftResult::Unchanged {
                current: result.value,
            });
        }
        let merged = match merge_managed_fields(&input, &canonical) {
            Ok(value) => value,
            Err(_) => return Ok(ManagedDraftResult::Conflict { current: canonical }),
        };
        let canonical_path =
            resolve_for_write(&self.root, &canonical.relative_path, "managed_draft_save")?;
        let canonical_bytes = std::fs::read(&canonical_path).map_err(|error| {
            CoreError::io(error, "managed_draft_save", Some(&canonical.relative_path))
        })?;
        self.snapshot_bytes(&canonical.id, "external", &canonical_bytes)?;
        let mut object = merged;
        object.properties = normalized_properties(&object.properties);
        let result =
            self.apply_managed_object(object, Some(&canonical.revision), "managed_draft_save")?;
        Ok(ManagedDraftResult::Merged {
            current: result.value.clone(),
            title: result.value.title.clone(),
            body: result.value.body.clone(),
            properties: result.value.properties.clone(),
        })
    }

    /// Adopt the external file version or replace it with the reviewed local
    /// draft. Both directions snapshot the version they displace before any
    /// durable write, and both return the object the client should display.
    pub fn resolve_managed_conflict(
        &self,
        input: ManagedConflictResolveInput,
    ) -> Result<WorkspaceObject> {
        let (current, current_bytes) =
            match self.read_canonical_object(&input.id, "managed_conflict_resolve") {
                Ok(value) => value,
                Err(error)
                    if error.code == "object_not_found"
                        && input.resolution == ManagedConflictResolution::ReplaceExternal =>
                {
                    return self.restore_deleted_managed_object(input);
                }
                Err(error) => return Err(error),
            };
        if current.revision != input.current_revision {
            let mut error = CoreError::new(
                "revision_conflict",
                ErrorCategory::Conflict,
                "The file changed again while the conflict was being reviewed",
                "managed_conflict_resolve",
            );
            error.details = Some(serde_json::json!({"currentRevision": current.revision}));
            return Err(error);
        }
        match input.resolution {
            ManagedConflictResolution::UseExternal => {
                let mut local = current.clone();
                local.title = input.local_title;
                local.body = input.local_body;
                local.properties = normalized_properties(&input.local_properties);
                let local_bytes = markdown::serialize_object(&local)?;
                self.snapshot_bytes(&current.id, "local", &local_bytes)?;
                Ok(current)
            }
            ManagedConflictResolution::ReplaceExternal => {
                self.snapshot_bytes(&current.id, "external", &current_bytes)?;
                let mut object = current;
                object.title = input.local_title;
                object.body = input.local_body;
                object.properties = normalized_properties(&input.local_properties);
                let result = self.apply_managed_object(
                    object,
                    Some(&input.current_revision),
                    "managed_conflict_resolve",
                )?;
                Ok(result.value)
            }
        }
    }

    fn restore_deleted_managed_object(
        &self,
        input: ManagedConflictResolveInput,
    ) -> Result<WorkspaceObject> {
        crate::path::validate_relative(&input.relative_path, "managed_object_restore")?;
        let object_type = input.id.split('_').next().unwrap_or_default().to_owned();
        if object_type.is_empty() || !valid_object_id(&input.id, &object_type) {
            return Err(CoreError::new(
                "invalid_object_id",
                ErrorCategory::Identity,
                "The object ID is invalid",
                "managed_conflict_resolve",
            ));
        }
        let timestamp = now_rfc3339();
        let created = match input.created {
            Some(created) => {
                ensure_rfc3339_timestamp(
                    &created,
                    "created must be an RFC 3339 timestamp".into(),
                    "managed_conflict_resolve",
                )?;
                Some(created)
            }
            None => Some(timestamp.clone()),
        };
        let object = WorkspaceObject {
            id: input.id,
            object_type,
            title: input.local_title,
            body: input.local_body,
            relative_path: input.relative_path,
            revision: String::new(),
            created,
            updated: Some(timestamp),
            properties: normalized_properties(&input.local_properties),
        };
        let result = self.apply_managed_object(object, None, "managed_conflict_resolve")?;
        Ok(result.value)
    }

    fn read_canonical_workspace_object(
        &self,
        id: &str,
        operation: &str,
    ) -> Result<WorkspaceObject> {
        let (object, _) = self.read_canonical_object(id, operation)?;
        Ok(object)
    }

    fn apply_managed_object(
        &self,
        mut object: WorkspaceObject,
        expected: Option<&str>,
        operation: &str,
    ) -> Result<MutationResult<WorkspaceObject>> {
        normalize_domain_properties(&object.object_type, &mut object.properties)?;
        object.updated = Some(now_rfc3339());
        self.commit_object(object, expected, "object:updated", operation)
    }

    fn snapshot_bytes(&self, id: &str, kind: &str, bytes: &[u8]) -> Result<()> {
        let object_type = id.split('_').next().unwrap_or_default();
        if object_type.is_empty()
            || !valid_object_id(id, object_type)
            || !matches!(kind, "local" | "external")
        {
            return Err(CoreError::validation(
                "invalid_history_target",
                "The recovery snapshot target is invalid",
                "history_snapshot",
            ));
        }
        let revision = markdown::revision(bytes);
        let relative = PathBuf::from(".noura")
            .join("history")
            .join(id)
            .join(format!("{revision}-{kind}.md"));
        let relative_text = relative.to_str().ok_or_else(|| {
            CoreError::validation(
                "non_utf8_path",
                "The recovery snapshot path is not UTF-8",
                "history_snapshot",
            )
        })?;
        let destination = resolve_for_write(&self.root, relative_text, "history_snapshot")?;
        if destination.exists() {
            return Ok(());
        }
        atomic_write(&self.root, &relative, bytes, "history_snapshot")
    }
}

/// Merge a managed draft field-by-field against the canonical file, using the
/// shared workspace-format rules.
fn merge_managed_fields(
    base: &ManagedDraftInput,
    canonical: &WorkspaceObject,
) -> Result<WorkspaceObject> {
    let draft = ManagedDraftFields {
        base_title: &base.base_title,
        local_title: &base.local_title,
        base_body: &base.base_body,
        local_body: &base.local_body,
        base_properties: &base.base_properties,
        local_properties: &base.local_properties,
    };
    workspace_format::merge::merge_managed_fields(draft, canonical).map_err(|conflict| {
        let message = match conflict {
            ManagedMergeConflict::Body => {
                "The body changed on both sides and requires manual review"
            }
            ManagedMergeConflict::Property => {
                "The same property changed on both sides and requires manual review"
            }
        };
        CoreError::new(
            "draft_conflict",
            ErrorCategory::Conflict,
            message,
            "managed_draft_merge",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn draft_reconciliation_merges_independent_markdown_edits_and_snapshots_external() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Merge".into(),
                body: "first\n\nsecond\n".into(),
                relative_path: Some("merge.md".into()),
                properties: BTreeMap::from([("tag".into(), serde_json::json!("base"))]),
            })
            .unwrap();
        let path = workspace.path().join("merge.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("tag: base", "tag: external")
            .replace("second", "second from file");
        std::fs::write(&path, external).unwrap();

        let result = engine
            .reconcile_note_draft(DraftReconcileInput {
                id: created.value.id.clone(),
                base_revision: created.revision,
                base_body: "first\n\nsecond\n".into(),
                local_body: "first in app\n\nsecond\n".into(),
            })
            .unwrap();

        let DraftReconcileResult::Merged { current, body } = result else {
            panic!("expected a clean merge");
        };
        assert_eq!(body, "first in app\n\nsecond from file");
        assert_eq!(current.properties["tag"], "external");
        let history = workspace
            .path()
            .join(".noura/history")
            .join(&created.value.id);
        assert_eq!(std::fs::read_dir(history).unwrap().count(), 1);
        assert!(
            engine
                .list_workspace_entries()
                .unwrap()
                .iter()
                .all(|entry| !entry.relative_path.starts_with(".noura"))
        );
        assert!(
            std::fs::read_to_string(path)
                .unwrap()
                .contains("second from file")
        );
    }

    #[test]
    fn overlapping_and_unsupported_drafts_never_write_merge_markers() {
        for local_body in ["local\n", "<aside>local</aside>\n"] {
            let workspace = tempdir().unwrap();
            let app_data = tempdir().unwrap();
            let engine =
                WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                    .unwrap();
            let created = engine
                .create_object(CreateObjectInput {
                    object_type: "note".into(),
                    title: "Conflict".into(),
                    body: "base\n".into(),
                    relative_path: Some("conflict.md".into()),
                    properties: BTreeMap::new(),
                })
                .unwrap();
            let path = workspace.path().join("conflict.md");
            let external = std::fs::read_to_string(&path)
                .unwrap()
                .replace("base", "external");
            std::fs::write(&path, external).unwrap();
            let result = engine
                .reconcile_note_draft(DraftReconcileInput {
                    id: created.value.id,
                    base_revision: created.revision,
                    base_body: "base\n".into(),
                    local_body: local_body.into(),
                })
                .unwrap();
            assert!(matches!(result, DraftReconcileResult::Conflict { .. }));
            let canonical = std::fs::read_to_string(path).unwrap();
            assert!(!canonical.contains("<<<<<<<"));
            assert!(canonical.contains("external"));
        }
    }

    #[test]
    fn conflict_resolution_snapshots_each_displaced_version() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Resolve".into(),
                body: "base\n".into(),
                relative_path: Some("resolve.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        let path = workspace.path().join("resolve.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("base", "external");
        std::fs::write(&path, external).unwrap();
        let reviewed = engine
            .reconcile_note_draft(DraftReconcileInput {
                id: created.value.id.clone(),
                base_revision: created.revision,
                base_body: "base\n".into(),
                local_body: "local\n".into(),
            })
            .unwrap();
        let DraftReconcileResult::Conflict { current } = reviewed else {
            panic!("expected a conflict");
        };
        let replaced = engine
            .resolve_note_conflict(ResolveConflictInput {
                id: created.value.id.clone(),
                current_revision: current.revision,
                local_body: "local\n".into(),
                resolution: ConflictResolution::ReplaceExternal,
            })
            .unwrap();
        // Mutations return the canonical parse of the written file.
        assert_eq!(replaced.value.body, "local");
        let snapshots = std::fs::read_dir(
            workspace
                .path()
                .join(".noura/history")
                .join(created.value.id),
        )
        .unwrap()
        .count();
        assert_eq!(snapshots, 1);
    }

    #[test]
    fn using_external_version_snapshots_the_local_draft() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Use external".into(),
                body: "base\n".into(),
                relative_path: Some("use-external.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        let path = workspace.path().join("use-external.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("base", "external");
        std::fs::write(&path, external).unwrap();
        let DraftReconcileResult::Conflict { current } = engine
            .reconcile_note_draft(DraftReconcileInput {
                id: created.value.id.clone(),
                base_revision: created.revision,
                base_body: "base\n".into(),
                local_body: "local\n".into(),
            })
            .unwrap()
        else {
            panic!("expected a conflict");
        };
        let resolved = engine
            .resolve_note_conflict(ResolveConflictInput {
                id: created.value.id.clone(),
                current_revision: current.revision,
                local_body: "local\n".into(),
                resolution: ConflictResolution::UseExternal,
            })
            .unwrap();
        assert_eq!(resolved.value.body, "external");
        let history = workspace
            .path()
            .join(".noura/history")
            .join(created.value.id);
        let snapshot = std::fs::read_dir(history)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        assert!(std::fs::read_to_string(snapshot).unwrap().contains("local"));
    }
}
