//! Platform-independent parsing, validation, and canonical serialization for
//! Noura workspace files.
//!
//! This crate deliberately has no filesystem, database, runtime, or native
//! credential dependencies so the same durable format logic can run on native
//! clients and WebAssembly clients.

use std::{
    borrow::Cow,
    collections::BTreeMap,
    path::{Component, Path},
};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

/// Canonical workspace-relative location of Noura's durable manifest.
pub const WORKSPACE_MANIFEST_PATH: &str = ".noura/workspace.yaml";

/// Errors produced while handling canonical workspace-format data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum FormatError {
    #[error("The stable ID does not match the object type")]
    InvalidObjectId,
    #[error("Frontmatter could not be serialized")]
    ObjectSerialization,
    #[error(".noura/workspace.yaml is invalid")]
    InvalidManifest,
    #[error("The workspace ID must be a lowercase stable workspace ID")]
    InvalidWorkspaceId,
    #[error("A workspace name is required")]
    WorkspaceNameRequired,
    #[error("Plugin identifiers use lowercase letters, digits, and hyphens")]
    InvalidPluginId,
    #[error("This workspace format version is not supported")]
    UnsupportedWorkspaceVersion,
    #[error(".noura/workspace.yaml could not be serialized")]
    ManifestSerialization,
    #[error("A title is required")]
    TitleRequired,
    #[error("A timestamp must be an RFC 3339 instant")]
    InvalidTimestamp,
    #[error("status must be todo, in-progress, done, or cancelled")]
    InvalidTaskStatus,
    #[error("status must be a string")]
    InvalidTaskStatusType,
    #[error("priority must be low, medium, high, or urgent")]
    InvalidTaskPriority,
    #[error("priority must be a string")]
    InvalidTaskPriorityType,
    #[error("due must use YYYY-MM-DD or RFC 3339 with an explicit offset")]
    InvalidTaskDue,
    #[error("Task project references use a stable project ID")]
    InvalidTaskProject,
    #[error("status must be planned, active, on-hold, completed, or cancelled")]
    InvalidProjectStatus,
    #[error("project status must be a string")]
    InvalidProjectStatusType,
}

/// Portable failures for a managed object destination path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManagedObjectPathError {
    Empty,
    Absolute,
    Unsafe,
    Reserved,
    UnsupportedExtension,
}

/// The canonical workspace manifest stored in `.noura/workspace.yaml`.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub struct WorkspaceManifest {
    pub id: String,
    pub format_version: u32,
    pub name: String,
    pub created: String,
    pub updated: String,
    pub enabled_plugins: Vec<String>,
    pub ignore: Vec<String>,
}

/// A managed Markdown object and its derived local metadata.
#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceObject {
    pub id: String,
    #[serde(rename = "type")]
    pub object_type: String,
    pub title: String,
    pub body: String,
    pub relative_path: String,
    pub revision: String,
    pub created: Option<String>,
    pub updated: Option<String>,
    #[ts(type = "Record<string, unknown>")]
    pub properties: BTreeMap<String, serde_json::Value>,
}

/// The portable input used to create a note without depending on a filesystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateNoteInput {
    pub title: String,
    #[serde(default)]
    pub body: String,
    pub relative_path: Option<String>,
    #[serde(default)]
    pub properties: BTreeMap<String, serde_json::Value>,
    pub now: String,
}

/// The portable edit shape for a managed note.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNoteInput {
    pub title: Option<String>,
    pub body: Option<String>,
    pub properties: Option<BTreeMap<String, serde_json::Value>>,
    #[serde(default)]
    pub remove_properties: Vec<String>,
    pub now: String,
}

/// The portable input used to create a task without depending on a filesystem.
pub type CreateTaskInput = CreateNoteInput;

/// The portable edit shape for a managed task.
pub type UpdateTaskInput = UpdateNoteInput;

/// The portable input used to create a project without depending on a filesystem.
pub type CreateProjectInput = CreateNoteInput;

/// The portable edit shape for a managed project.
pub type UpdateProjectInput = UpdateNoteInput;

/// The result of parsing a Markdown file without reading it from a filesystem.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParsedMarkdown {
    Managed(WorkspaceObject),
    Unmanaged {
        title: String,
        body: String,
        frontmatter: Option<BTreeMap<String, serde_json::Value>>,
    },
    Malformed {
        title: String,
        body: String,
        error: String,
    },
}

/// Returns the content revision used for external-edit detection.
pub fn revision(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

/// Validates the path policy shared by native and browser managed-object moves.
/// Filesystem-specific checks such as symlink traversal remain with the adapter.
pub fn validate_managed_object_path(path: &str) -> Result<(), ManagedObjectPathError> {
    if path.is_empty() {
        return Err(ManagedObjectPathError::Empty);
    }
    let value = Path::new(path);
    if value.is_absolute() {
        return Err(ManagedObjectPathError::Absolute);
    }
    for (index, component) in value.components().enumerate() {
        if !matches!(component, Component::Normal(_)) {
            return Err(ManagedObjectPathError::Unsafe);
        }
        if index == 0
            && matches!(
                component.as_os_str().to_str(),
                Some(".noura" | ".git" | "node_modules" | "target")
            )
        {
            return Err(ManagedObjectPathError::Reserved);
        }
    }
    if value.extension().and_then(|value| value.to_str()) != Some("md") {
        return Err(ManagedObjectPathError::UnsupportedExtension);
    }
    Ok(())
}

/// Constructs a canonical workspace manifest while retaining all durable format
/// decisions in portable Rust. The caller supplies the current clock instant.
pub fn create_workspace_manifest(
    id: String,
    name: String,
    now: String,
) -> Result<WorkspaceManifest, FormatError> {
    if name.trim().is_empty() {
        return Err(FormatError::WorkspaceNameRequired);
    }
    validate_timestamp(&now)?;
    if !valid_object_id(&id, "workspace") {
        return Err(FormatError::InvalidWorkspaceId);
    }
    Ok(WorkspaceManifest {
        id,
        format_version: 1,
        name: name.trim().into(),
        created: now.clone(),
        updated: now,
        enabled_plugins: Vec::new(),
        ignore: Vec::new(),
    })
}

/// Constructs a note before canonical serialization. Path safety and collision
/// checks stay with the filesystem adapter because they depend on its storage.
pub fn create_note(id: String, input: CreateNoteInput) -> Result<WorkspaceObject, FormatError> {
    create_managed_object(id, "note", input)
}

/// Constructs a task with its canonical default fields and validated task metadata.
pub fn create_task(id: String, input: CreateTaskInput) -> Result<WorkspaceObject, FormatError> {
    let mut task = create_managed_object(id, "task", input)?;
    normalize_task_properties(&mut task.properties)?;
    Ok(task)
}

/// Constructs a project with canonical default fields and validated project metadata.
pub fn create_project(
    id: String,
    input: CreateProjectInput,
) -> Result<WorkspaceObject, FormatError> {
    let mut project = create_managed_object(id, "project", input)?;
    normalize_project_properties(&mut project.properties)?;
    Ok(project)
}

fn create_managed_object(
    id: String,
    object_type: &str,
    mut input: CreateNoteInput,
) -> Result<WorkspaceObject, FormatError> {
    if input.title.trim().is_empty() {
        return Err(FormatError::TitleRequired);
    }
    validate_timestamp(&input.now)?;
    for key in ["id", "type", "created", "updated"] {
        input.properties.remove(key);
    }
    if !valid_object_id(&id, object_type) {
        return Err(FormatError::InvalidObjectId);
    }
    let relative_path = input
        .relative_path
        .unwrap_or_else(|| default_managed_object_path(object_type, &input.title, &id));
    Ok(WorkspaceObject {
        id,
        object_type: object_type.into(),
        title: input.title.trim().into(),
        body: input.body,
        relative_path,
        revision: String::new(),
        created: Some(input.now.clone()),
        updated: Some(input.now),
        properties: input.properties,
    })
}

/// Applies a note edit without allowing managed metadata to be overwritten.
pub fn update_note(
    note: WorkspaceObject,
    input: UpdateNoteInput,
) -> Result<WorkspaceObject, FormatError> {
    update_managed_object(note, input, "note")
}

/// Applies a task edit while retaining canonical task metadata validation.
pub fn update_task(
    task: WorkspaceObject,
    input: UpdateTaskInput,
) -> Result<WorkspaceObject, FormatError> {
    let mut task = update_managed_object(task, input, "task")?;
    normalize_task_properties(&mut task.properties)?;
    Ok(task)
}

/// Applies a project edit while retaining canonical project metadata validation.
pub fn update_project(
    project: WorkspaceObject,
    input: UpdateProjectInput,
) -> Result<WorkspaceObject, FormatError> {
    let mut project = update_managed_object(project, input, "project")?;
    normalize_project_properties(&mut project.properties)?;
    Ok(project)
}

fn update_managed_object(
    mut object: WorkspaceObject,
    input: UpdateNoteInput,
    object_type: &str,
) -> Result<WorkspaceObject, FormatError> {
    if object.object_type != object_type || !valid_object_id(&object.id, object_type) {
        return Err(FormatError::InvalidObjectId);
    }
    validate_timestamp(&input.now)?;
    if let Some(title) = input.title {
        if title.trim().is_empty() {
            return Err(FormatError::TitleRequired);
        }
        object.title = title.trim().into();
    }
    if let Some(body) = input.body {
        object.body = body;
    }
    for key in input.remove_properties {
        if !is_managed_property(&key) {
            object.properties.remove(&key);
        }
    }
    if let Some(properties) = input.properties {
        for (key, value) in properties {
            if !is_managed_property(&key) {
                object.properties.insert(key, value);
            }
        }
    }
    object.updated = Some(input.now);
    Ok(object)
}

/// Applies the task defaults and validates task-specific frontmatter values.
/// Unknown properties are deliberately retained as user-owned metadata.
pub fn normalize_task_properties(
    properties: &mut BTreeMap<String, serde_json::Value>,
) -> Result<(), FormatError> {
    properties
        .entry("status".into())
        .or_insert_with(|| serde_json::json!("todo"));
    properties
        .entry("priority".into())
        .or_insert_with(|| serde_json::json!("medium"));
    match properties.get("status") {
        Some(serde_json::Value::String(value))
            if matches!(
                value.as_str(),
                "todo" | "in-progress" | "done" | "cancelled"
            ) => {}
        Some(serde_json::Value::String(_)) => return Err(FormatError::InvalidTaskStatus),
        _ => return Err(FormatError::InvalidTaskStatusType),
    }
    match properties.get("priority") {
        Some(serde_json::Value::String(value))
            if matches!(value.as_str(), "low" | "medium" | "high" | "urgent") => {}
        Some(serde_json::Value::String(_)) => return Err(FormatError::InvalidTaskPriority),
        _ => return Err(FormatError::InvalidTaskPriorityType),
    }
    if let Some(value) = properties.get("due").and_then(serde_json::Value::as_str) {
        validate_task_due(value)?;
    }
    if let Some(project) = properties
        .get("project")
        .and_then(serde_json::Value::as_str)
        && !valid_object_id(project, "project")
    {
        return Err(FormatError::InvalidTaskProject);
    }
    Ok(())
}

/// Applies project defaults and validates project-specific frontmatter values.
/// Unknown properties are deliberately retained as user-owned metadata.
pub fn normalize_project_properties(
    properties: &mut BTreeMap<String, serde_json::Value>,
) -> Result<(), FormatError> {
    properties
        .entry("status".into())
        .or_insert_with(|| serde_json::json!("planned"));
    match properties.get("status") {
        Some(serde_json::Value::String(value))
            if matches!(
                value.as_str(),
                "planned" | "active" | "on-hold" | "completed" | "cancelled"
            ) =>
        {
            Ok(())
        }
        Some(serde_json::Value::String(_)) => Err(FormatError::InvalidProjectStatus),
        _ => Err(FormatError::InvalidProjectStatusType),
    }
}

fn validate_task_due(value: &str) -> Result<(), FormatError> {
    let date_only = value.len() == 10
        && value.as_bytes().get(4) == Some(&b'-')
        && value.as_bytes().get(7) == Some(&b'-')
        && value.parse::<jiff::civil::Date>().is_ok();
    let timed = (value.ends_with('Z')
        || value
            .as_bytes()
            .iter()
            .skip(10)
            .any(|byte| matches!(byte, b'+' | b'-')))
        && value.parse::<jiff::Timestamp>().is_ok();
    if date_only || timed {
        Ok(())
    } else {
        Err(FormatError::InvalidTaskDue)
    }
}

fn split_title(content: &str) -> (String, String) {
    let mut lines = content.lines();
    match lines.next() {
        Some(first) if first.starts_with("# ") => {
            let title = first[2..].trim().to_owned();
            let body = lines
                .collect::<Vec<_>>()
                .join("\n")
                .trim_start_matches('\n')
                .to_owned();
            (title, body)
        }
        _ => (String::new(), content.to_owned()),
    }
}

/// Parses a Markdown file without relying on a local filesystem or runtime.
pub fn parse_markdown(relative_path: &str, bytes: &[u8]) -> ParsedMarkdown {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return ParsedMarkdown::Malformed {
            title: String::new(),
            body: String::new(),
            error: "The file is not UTF-8".into(),
        };
    };
    let text = if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    };
    let text = text.as_ref();
    if !text.starts_with("---\n") {
        let (title, body) = split_title(text);
        return ParsedMarkdown::Unmanaged {
            title,
            body,
            frontmatter: None,
        };
    }
    let Some(end) = text[4..].find("\n---\n").map(|index| index + 4) else {
        let (title, body) = split_title(text);
        return ParsedMarkdown::Malformed {
            title,
            body,
            error: "Frontmatter has no closing delimiter".into(),
        };
    };
    let yaml = &text[4..end];
    let markdown = text[end + 5..].trim_start_matches('\n');
    let (title, body) = split_title(markdown);
    let properties: BTreeMap<String, serde_json::Value> = match serde_yaml_ng::from_str(yaml) {
        Ok(properties) => properties,
        Err(error) => {
            return ParsedMarkdown::Malformed {
                title,
                body,
                error: format!("Invalid YAML frontmatter: {error}"),
            };
        }
    };
    let Some(id) = properties
        .get("id")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        return ParsedMarkdown::Unmanaged {
            title,
            body,
            frontmatter: Some(properties),
        };
    };
    let Some(object_type) = properties
        .get("type")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
    else {
        return ParsedMarkdown::Malformed {
            title,
            body,
            error: "Managed frontmatter requires a string type".into(),
        };
    };
    if !valid_object_id(&id, &object_type) {
        return ParsedMarkdown::Malformed {
            title,
            body,
            error: "The stable ID does not match the object type".into(),
        };
    }
    let created = properties
        .get("created")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let updated = properties
        .get("updated")
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned);
    let mut custom_properties = properties;
    for key in ["id", "type", "created", "updated"] {
        custom_properties.remove(key);
    }
    ParsedMarkdown::Managed(WorkspaceObject {
        id,
        object_type,
        title,
        body,
        relative_path: relative_path.to_owned(),
        revision: revision(bytes),
        created,
        updated,
        properties: custom_properties,
    })
}

/// Serializes a managed object using the deterministic workspace representation.
pub fn serialize_object(object: &WorkspaceObject) -> Result<Vec<u8>, FormatError> {
    if !valid_object_id(&object.id, &object.object_type) {
        return Err(FormatError::InvalidObjectId);
    }
    let mut ordered = serde_yaml_ng::Mapping::new();
    let mut insert = |key: &str, value: serde_json::Value| -> Result<(), FormatError> {
        let value = serde_yaml_ng::to_value(value).map_err(|_| FormatError::ObjectSerialization)?;
        ordered.insert(serde_yaml_ng::Value::String(key.into()), value);
        Ok(())
    };
    insert("id", serde_json::Value::String(object.id.clone()))?;
    insert(
        "type",
        serde_json::Value::String(object.object_type.clone()),
    )?;
    for key in [
        "status",
        "priority",
        "project",
        "due",
        "date",
        "start",
        "end",
        "kanban_order",
    ] {
        if let Some(value) = object.properties.get(key) {
            insert(key, value.clone())?;
        }
    }
    for (key, value) in &object.properties {
        if !matches!(
            key.as_str(),
            "id" | "type"
                | "status"
                | "priority"
                | "project"
                | "due"
                | "date"
                | "start"
                | "end"
                | "kanban_order"
                | "created"
                | "updated"
        ) {
            insert(key, value.clone())?;
        }
    }
    if let Some(created) = &object.created {
        insert("created", serde_json::Value::String(created.clone()))?;
    }
    if let Some(updated) = &object.updated {
        insert("updated", serde_json::Value::String(updated.clone()))?;
    }
    let yaml = serde_yaml_ng::to_string(&ordered).map_err(|_| FormatError::ObjectSerialization)?;
    let mut result = format!("---\n{}---\n\n# {}\n", yaml, object.title.trim());
    if !object.body.trim().is_empty() {
        result.push('\n');
        result.push_str(object.body.trim_end());
        result.push('\n');
    }
    Ok(result.into_bytes())
}

/// Decodes a manifest document without applying semantic validation.
pub fn decode_workspace_manifest(bytes: &[u8]) -> Result<WorkspaceManifest, FormatError> {
    serde_yaml_ng::from_slice(bytes).map_err(|_| FormatError::InvalidManifest)
}

/// Checks the platform-independent invariants of a workspace manifest.
pub fn validate_workspace_manifest(manifest: &WorkspaceManifest) -> Result<(), FormatError> {
    if !valid_object_id(&manifest.id, "workspace") {
        return Err(FormatError::InvalidWorkspaceId);
    }
    if manifest.name.trim().is_empty() {
        return Err(FormatError::WorkspaceNameRequired);
    }
    if manifest
        .enabled_plugins
        .iter()
        .any(|id| !valid_plugin_id(id))
    {
        return Err(FormatError::InvalidPluginId);
    }
    Ok(())
}

/// Parses and canonicalizes a manifest for a portable workspace client.
pub fn parse_workspace_manifest(bytes: &[u8]) -> Result<WorkspaceManifest, FormatError> {
    let mut manifest = decode_workspace_manifest(bytes)?;
    validate_workspace_manifest(&manifest)?;
    if manifest.format_version != 1 {
        return Err(FormatError::UnsupportedWorkspaceVersion);
    }
    normalize_workspace_manifest(&mut manifest);
    Ok(manifest)
}

/// Canonicalizes fields whose order and duplicate values are insignificant.
pub fn normalize_workspace_manifest(manifest: &mut WorkspaceManifest) {
    manifest.enabled_plugins.sort();
    manifest.enabled_plugins.dedup();
}

/// Serializes a manifest with the existing canonical YAML serializer.
pub fn serialize_workspace_manifest(manifest: &WorkspaceManifest) -> Result<String, FormatError> {
    serde_yaml_ng::to_string(manifest).map_err(|_| FormatError::ManifestSerialization)
}

/// Checks whether an ID has the stable prefix and lowercase ULID required by the format.
pub fn valid_object_id(id: &str, object_type: &str) -> bool {
    if !valid_object_type(object_type) {
        return false;
    }
    let Some(value) = id.strip_prefix(&format!("{object_type}_")) else {
        return false;
    };
    value.len() == 26 && value == value.to_ascii_lowercase() && value.parse::<ulid::Ulid>().is_ok()
}

/// Returns the canonical initial file location for a managed object. Project
/// files are folder notes so their enclosing folder can hold related files.
pub fn default_managed_object_path(object_type: &str, title: &str, id: &str) -> String {
    let value = slug::slugify(title);
    let slug = if value.is_empty() { "untitled" } else { &value };
    let short = &id[id.len().saturating_sub(6)..];
    if object_type == "project" {
        format!("projects/{slug}--{short}/project.md")
    } else {
        format!("{object_type}s/{slug}--{short}.md")
    }
}

fn validate_timestamp(value: &str) -> Result<(), FormatError> {
    value
        .parse::<jiff::Timestamp>()
        .map(|_| ())
        .map_err(|_| FormatError::InvalidTimestamp)
}

fn is_managed_property(key: &str) -> bool {
    matches!(key, "id" | "type" | "created" | "updated")
}

/// Checks whether an object type is valid in a stable ID prefix.
pub fn valid_object_type(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .skip(1)
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
}

fn valid_plugin_id(value: &str) -> bool {
    let mut chars = value.chars();
    chars
        .next()
        .is_some_and(|character| character.is_ascii_lowercase())
        && value.len() <= 64
        && value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[test]
    fn managed_markdown_round_trips_deterministically() {
        let object = WorkspaceObject {
            id: "note_01j00000000000000000000000".into(),
            object_type: "note".into(),
            title: "Example".into(),
            body: "Body".into(),
            relative_path: "notes/example.md".into(),
            revision: String::new(),
            created: Some("2026-08-27T12:00:00Z".into()),
            updated: Some("2026-08-27T12:00:00Z".into()),
            properties: BTreeMap::from([("custom".into(), serde_json::json!("kept"))]),
        };
        let first = serialize_object(&object).unwrap();
        let ParsedMarkdown::Managed(parsed) = parse_markdown(&object.relative_path, &first) else {
            panic!("managed object expected")
        };
        let second = serialize_object(&parsed).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn malformed_frontmatter_is_not_managed() {
        let ParsedMarkdown::Malformed { error, .. } =
            parse_markdown("bad.md", b"---\nid: [\n---\n# Bad")
        else {
            panic!("malformed frontmatter expected");
        };
        assert!(error.starts_with("Invalid YAML frontmatter:"));
    }

    #[test]
    fn crlf_frontmatter_is_parsed_as_managed() {
        let bytes = "---\r\nid: note_01j00000000000000000000000\r\ntype: note\r\n---\r\n# Title\r\n\r\nBody\r\n";
        let ParsedMarkdown::Managed(object) = parse_markdown("note.md", bytes.as_bytes()) else {
            panic!("CRLF frontmatter should be managed");
        };
        assert_eq!(object.body, "Body");
    }

    #[derive(Deserialize)]
    struct Fixtures {
        manifest: Vec<Fixture>,
        object_id: Vec<ObjectIdFixture>,
        task_properties: Vec<TaskPropertiesFixture>,
        project_properties: Vec<TaskPropertiesFixture>,
    }

    #[derive(Deserialize)]
    struct Fixture {
        valid: bool,
        value: serde_json::Value,
    }

    #[derive(Deserialize)]
    struct ObjectIdFixture {
        valid: bool,
        value: String,
    }

    #[derive(Deserialize)]
    struct TaskPropertiesFixture {
        valid: bool,
        value: BTreeMap<String, serde_json::Value>,
    }

    #[test]
    fn portable_validation_matches_shared_conformance_fixtures() {
        let fixtures: Fixtures = serde_json::from_str(include_str!(
            "../../../docs/workspace-format/fixtures/conformance-v1.json"
        ))
        .unwrap();
        let manifests_match = fixtures.manifest.into_iter().all(|fixture| {
            serde_json::from_value::<WorkspaceManifest>(fixture.value)
                .ok()
                .is_some_and(|manifest| validate_workspace_manifest(&manifest).is_ok())
                == fixture.valid
        });
        let object_ids_match = fixtures
            .object_id
            .into_iter()
            .all(|fixture| valid_object_id(&fixture.value, "note") == fixture.valid);
        let task_properties_match = fixtures.task_properties.into_iter().all(|fixture| {
            let mut properties = fixture.value;
            normalize_task_properties(&mut properties).is_ok() == fixture.valid
        });
        let project_properties_match = fixtures.project_properties.into_iter().all(|fixture| {
            let mut properties = fixture.value;
            normalize_project_properties(&mut properties).is_ok() == fixture.valid
        });
        assert!(
            manifests_match
                && object_ids_match
                && task_properties_match
                && project_properties_match
        );
    }

    #[test]
    fn manifest_parser_sorts_and_deduplicates_plugins() {
        let manifest = parse_workspace_manifest(
            b"id: workspace_01j00000000000000000000000\nformat_version: 1\nname: Example\ncreated: 2026-08-27T12:00:00Z\nupdated: 2026-08-27T12:00:00Z\nenabled_plugins: [tasks, notes, tasks]\nignore: []\n",
        )
        .unwrap();
        assert_eq!(manifest.enabled_plugins, ["notes", "tasks"]);
    }

    #[test]
    fn note_creation_uses_native_compatible_defaults() {
        let note = create_note(
            "note_01j00000000000000000000000".into(),
            CreateNoteInput {
                title: " Browser note ".into(),
                body: "Body".into(),
                relative_path: None,
                properties: BTreeMap::from([("id".into(), serde_json::json!("ignored"))]),
                now: "2026-09-12T00:00:00Z".into(),
            },
        )
        .unwrap();
        assert!(valid_object_id(&note.id, "note"));
        assert_eq!(note.title, "Browser note");
        assert!(note.relative_path.starts_with("notes/browser-note--"));
        assert!(!note.properties.contains_key("id"));
    }

    #[test]
    fn task_domain_rules_apply_defaults_and_preserve_unknown_metadata() {
        let task = create_task(
            "task_01j00000000000000000000000".into(),
            CreateTaskInput {
                title: " Browser task ".into(),
                body: "Body".into(),
                relative_path: None,
                properties: BTreeMap::from([("custom".into(), serde_json::json!("kept"))]),
                now: "2026-09-12T00:00:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(task.object_type, "task");
        assert_eq!(task.properties["status"], "todo");
        assert_eq!(task.properties["priority"], "medium");
        assert_eq!(task.properties["custom"], "kept");
        assert!(task.relative_path.starts_with("tasks/browser-task--"));
    }

    #[test]
    fn task_domain_rules_reject_invalid_status_due_and_project() {
        let mut invalid_status = BTreeMap::from([("status".into(), serde_json::json!("later"))]);
        assert_eq!(
            normalize_task_properties(&mut invalid_status),
            Err(FormatError::InvalidTaskStatus)
        );
        let mut invalid_due = BTreeMap::from([("due".into(), serde_json::json!("tomorrow"))]);
        assert_eq!(
            normalize_task_properties(&mut invalid_due),
            Err(FormatError::InvalidTaskDue)
        );
        let mut invalid_project =
            BTreeMap::from([("project".into(), serde_json::json!("project_not-an-id"))]);
        assert_eq!(
            normalize_task_properties(&mut invalid_project),
            Err(FormatError::InvalidTaskProject)
        );
    }

    #[test]
    fn project_domain_rules_apply_defaults_and_preserve_unknown_metadata() {
        let project = create_project(
            "project_01j00000000000000000000000".into(),
            CreateProjectInput {
                title: " Browser project ".into(),
                body: "Body".into(),
                relative_path: None,
                properties: BTreeMap::from([("custom".into(), serde_json::json!("kept"))]),
                now: "2026-09-12T00:00:00Z".into(),
            },
        )
        .unwrap();
        assert_eq!(project.object_type, "project");
        assert_eq!(project.properties["status"], "planned");
        assert_eq!(project.properties["custom"], "kept");
        assert!(
            project
                .relative_path
                .starts_with("projects/browser-project--")
        );
        assert!(project.relative_path.ends_with("/project.md"));
    }

    #[test]
    fn managed_object_paths_reject_internal_and_non_markdown_destinations() {
        assert_eq!(
            validate_managed_object_path(".noura/note.md"),
            Err(ManagedObjectPathError::Reserved)
        );
        assert_eq!(
            validate_managed_object_path("notes/note.txt"),
            Err(ManagedObjectPathError::UnsupportedExtension)
        );
        assert_eq!(
            validate_managed_object_path("notes/../note.md"),
            Err(ManagedObjectPathError::Unsafe)
        );
        assert!(validate_managed_object_path("notes/note.md").is_ok());
    }
}
