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

pub mod calendar;
pub mod merge;
mod object_type;
pub mod raw_text;

pub use object_type::{
    ObjectTypeRegistry, PropertyDeclaration, PropertyError, PropertyKind, PropertySchema,
    RegistrationError,
};

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
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CreateNoteInput {
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub body: String,
    #[ts(optional = nullable)]
    pub relative_path: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[ts(type = "Record<string, unknown>")]
    pub properties: BTreeMap<String, serde_json::Value>,
    pub now: String,
}

/// The portable edit shape for a managed note.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UpdateNoteInput {
    #[ts(optional = nullable)]
    pub title: Option<String>,
    #[ts(optional = nullable)]
    pub body: Option<String>,
    #[ts(optional, type = "Record<string, unknown> | null")]
    pub properties: Option<BTreeMap<String, serde_json::Value>>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
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
#[derive(Debug, Clone, Serialize, PartialEq, TS)]
#[ts(export)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ParsedMarkdown {
    Managed(WorkspaceObject),
    Unmanaged {
        title: String,
        body: String,
        #[ts(type = "Record<string, unknown> | null")]
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
        .unwrap_or_else(|| default_managed_object_path(object_type, &input.title));
    Ok(WorkspaceObject {
        id,
        object_type: object_type.into(),
        title: title_from_path(&relative_path),
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
    // The title is the file name, so a new title moves the file to a new
    // name in the same folder. The filesystem adapter carries out the move.
    if let Some(title) = input.title {
        let name = normalize_file_name(&title);
        if name.is_empty() {
            return Err(FormatError::TitleRequired);
        }
        object.relative_path = renamed_path(&object.relative_path, &name);
        object.title = name;
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
    if valid_date_value(value) {
        Ok(())
    } else {
        Err(FormatError::InvalidTaskDue)
    }
}

/// Checks for a civil date (`YYYY-MM-DD`) or an RFC 3339 instant with an
/// explicit offset.
fn valid_date_value(value: &str) -> bool {
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
    date_only || timed
}

/// The title of the file at `relative_path`: its file name without the
/// extension. A note, task or project is named by its file, and a heading
/// inside the file is ordinary text.
pub fn title_from_path(relative_path: &str) -> String {
    let name = relative_path
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(relative_path);
    match name.rfind('.') {
        Some(dot) if dot > 0 => name[..dot].to_owned(),
        _ => name.to_owned(),
    }
}

/// The longest file name, in characters, that [`normalize_file_name`]
/// keeps.
pub const MAX_FILE_NAME_CHARS: usize = 200;

/// Turns a typed name into a file name that works on macOS, Windows and
/// Linux: no path separators, no characters macOS or Windows reject, no
/// leading dots (which would hide the file), no surrounding spaces, no
/// trailing dots (which Windows drops), and at most [`MAX_FILE_NAME_CHARS`]
/// characters. A name Windows keeps for a device, such as `CON` or `com1`,
/// gets an underscore after it. An empty result means the name had
/// nothing usable.
pub fn normalize_file_name(typed: &str) -> String {
    let kept: String = typed
        .chars()
        .filter(|character| {
            !character.is_control()
                && !matches!(
                    character,
                    '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|'
                )
        })
        .collect();
    let dot_or_space = |character: char| character == '.' || character.is_whitespace();
    let name = kept.trim_start_matches(dot_or_space).trim_end();
    let cut: String = name.chars().take(MAX_FILE_NAME_CHARS).collect();
    let name = cut.trim_end_matches(dot_or_space);
    // Windows reads `CON` as the device even with an extension after it.
    let stem = name.split('.').next().unwrap_or(name).trim_end();
    if is_windows_device_name(stem) {
        let (stem, rest) = name.split_at(stem.len());
        return format!("{stem}_{rest}");
    }
    name.to_owned()
}

/// Whether Windows keeps `name` for a device: `CON`, `PRN`, `AUX`, `NUL`,
/// or `COM` or `LPT` followed by a digit from 1 to 9, in any letter case.
fn is_windows_device_name(name: &str) -> bool {
    let upper = name.to_ascii_uppercase();
    match upper.as_str() {
        "CON" | "PRN" | "AUX" | "NUL" => true,
        _ => {
            upper.len() == 4
                && (upper.starts_with("COM") || upper.starts_with("LPT"))
                && matches!(upper.as_bytes()[3], b'1'..=b'9')
        }
    }
}

/// The path of the file at `relative_path` once it's named `name`, in the
/// same folder and with the same extension.
pub fn renamed_path(relative_path: &str, name: &str) -> String {
    let (folder, file) = match relative_path.rfind('/') {
        Some(slash) => (&relative_path[..=slash], &relative_path[slash + 1..]),
        None => ("", relative_path),
    };
    let extension = match file.rfind('.') {
        Some(dot) if dot > 0 => &file[dot..],
        _ => "",
    };
    format!("{folder}{name}{extension}")
}

/// Where the frontmatter of LF `text` ends: the byte offset just past its
/// closing `---` line, or `None` when the text has no complete frontmatter.
/// A leading byte-order mark must already be gone.
pub fn frontmatter_end(text: &str) -> Option<usize> {
    if !text.starts_with("---\n") {
        return None;
    }
    text[4..]
        .find("\n---\n")
        .map(|index| index + 4 + "\n---\n".len())
}

/// Reads a run of carriage returns right before a line feed as one line
/// break, for the body. Reading only CRLF as LF would leave `\r\n` of a
/// `\r\r\n` in the body. The serializer writes that back as it is, and the
/// next read would drop one more carriage return, so the bytes would change
/// on every write. The frontmatter still reads only CRLF as LF, so no file
/// moves between managed, malformed and unmanaged.
fn single_line_breaks(text: &str) -> Cow<'_, str> {
    if !text.contains("\r\n") {
        return Cow::Borrowed(text);
    }
    let mut normalized = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        match line.strip_suffix('\n') {
            Some(content) => {
                normalized.push_str(content.trim_end_matches('\r'));
                normalized.push('\n');
            }
            None => normalized.push_str(line),
        }
    }
    Cow::Owned(normalized)
}

/// Parses a Markdown file without relying on a local filesystem or runtime.
pub fn parse_markdown(relative_path: &str, bytes: &[u8]) -> ParsedMarkdown {
    let title = title_from_path(relative_path);
    let Ok(text) = std::str::from_utf8(bytes) else {
        return ParsedMarkdown::Malformed {
            title,
            body: String::new(),
            error: "The file is not UTF-8".into(),
        };
    };
    // A byte-order mark isn't part of the text. Writers keep it.
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let text = if text.contains("\r\n") {
        Cow::Owned(text.replace("\r\n", "\n"))
    } else {
        Cow::Borrowed(text)
    };
    let text = text.as_ref();
    if !text.starts_with("---\n") {
        return ParsedMarkdown::Unmanaged {
            title,
            body: single_line_breaks(text).into_owned(),
            frontmatter: None,
        };
    }
    let Some(end) = frontmatter_end(text) else {
        return ParsedMarkdown::Malformed {
            title,
            body: single_line_breaks(text).into_owned(),
            error: "Frontmatter has no closing delimiter".into(),
        };
    };
    // The line break before the closing delimiter ends the last YAML line.
    // Without it, a block scalar written last would lose its final line break.
    let yaml = &text[4..end - "---\n".len()];
    let body = single_line_breaks(&text[end..]).into_owned();
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

/// The properties `serialize_object` writes first, in this order, before the
/// sorted unknown properties. It applies to every type without a registered
/// schema; schemas take over once the Tasks, Projects and Calendar plugins
/// declare their types.
const BUILT_IN_PROPERTY_ORDER: [&str; 8] = [
    "status",
    "priority",
    "project",
    "due",
    "date",
    "start",
    "end",
    "kanban_order",
];

/// Serializes a managed object using the deterministic workspace representation.
pub fn serialize_object(object: &WorkspaceObject) -> Result<Vec<u8>, FormatError> {
    serialize_envelope(object, &BUILT_IN_PROPERTY_ORDER)
}

/// Serializes `object` over `existing`, the bytes its file holds now. The
/// frontmatter and the body each keep their bytes when they didn't change,
/// and the file keeps its byte-order mark, the ending of each line nobody
/// changed, and its final line break or the lack of one. A file that isn't
/// managed yet gets [`serialize_object`]'s text with its own byte-order mark
/// and line endings.
pub fn serialize_object_over(
    existing: &[u8],
    object: &WorkspaceObject,
) -> Result<Vec<u8>, FormatError> {
    rewrite_envelope(existing, object, &BUILT_IN_PROPERTY_ORDER)
}

pub(crate) fn rewrite_envelope(
    existing: &[u8],
    object: &WorkspaceObject,
    leading: &[&str],
) -> Result<Vec<u8>, FormatError> {
    let Ok((text, layout)) = raw_text::split_raw_bytes(existing) else {
        return serialize_envelope(object, leading);
    };
    let (ParsedMarkdown::Managed(current), Some(end)) = (
        parse_markdown(&object.relative_path, existing),
        frontmatter_end(&text),
    ) else {
        // A file becoming managed, as when it's adopted, keeps its
        // byte-order mark and the endings of the lines it already had.
        let written = serialize_envelope(object, leading)?;
        let written = String::from_utf8(written).map_err(|_| FormatError::ObjectSerialization)?;
        return Ok(raw_text::compose_raw_bytes(&written, &layout));
    };
    let same_envelope = current.id == object.id
        && current.object_type == object.object_type
        && current.created == object.created
        && current.updated == object.updated
        && current.properties == object.properties;
    let mut next = if same_envelope {
        text[..end].to_owned()
    } else {
        envelope_frontmatter(object, leading)?
    };
    if current.body == object.body {
        next.push_str(&text[end..]);
    } else {
        next.push_str(&object.body);
    }
    Ok(raw_text::compose_raw_bytes(&next, &layout))
}

/// Writes the managed-object envelope: `id` and `type`, the `leading`
/// properties in that order, the other properties sorted by key, `created`
/// and `updated`, then the body as it is. The title is the file's name, so
/// no heading is written.
fn serialize_envelope(object: &WorkspaceObject, leading: &[&str]) -> Result<Vec<u8>, FormatError> {
    let mut result = envelope_frontmatter(object, leading)?;
    result.push_str(&object.body);
    Ok(result.into_bytes())
}

/// The frontmatter block of the managed-object envelope, closing `---`
/// line included.
fn envelope_frontmatter(object: &WorkspaceObject, leading: &[&str]) -> Result<String, FormatError> {
    if !valid_object_id(&object.id, &object.object_type) {
        return Err(FormatError::InvalidObjectId);
    }
    let mut entries = vec![
        ("id", serde_json::Value::String(object.id.clone())),
        (
            "type",
            serde_json::Value::String(object.object_type.clone()),
        ),
    ];
    for key in leading {
        if let Some(value) = object.properties.get(*key) {
            entries.push((key, value.clone()));
        }
    }
    for (key, value) in &object.properties {
        if !is_managed_property(key) && !leading.contains(&key.as_str()) {
            entries.push((key, value.clone()));
        }
    }
    if let Some(created) = &object.created {
        entries.push(("created", serde_json::Value::String(created.clone())));
    }
    if let Some(updated) = &object.updated {
        entries.push(("updated", serde_json::Value::String(updated.clone())));
    }
    let yaml = frontmatter_yaml(&entries)?;
    Ok(format!("---\n{yaml}---\n"))
}

/// Writes frontmatter entries as a YAML mapping, in order, as libyaml writes
/// it, with one exception.
///
/// YAML 1.1 readers such as libyaml read U+2028 and U+2029 as line breaks.
/// libyaml writes them raw, and it writes a multi-line string as a literal
/// block. When the last value it writes is a literal block that ends in one
/// of them, its output ends without a line feed and the closing `---` loses
/// its line. Only then is that last string written double-quoted. Every
/// other value keeps libyaml's bytes. libyaml doesn't let a caller pick a
/// scalar style, so it writes the mapping again with an empty string in that
/// place, which ends its output as `''`, and the quoted string replaces it.
fn frontmatter_yaml(entries: &[(&str, serde_json::Value)]) -> Result<String, FormatError> {
    let mut mapping = serde_yaml_ng::Mapping::with_capacity(entries.len());
    for (key, value) in entries {
        let value = serde_yaml_ng::to_value(value).map_err(|_| FormatError::ObjectSerialization)?;
        mapping.insert(serde_yaml_ng::Value::String((*key).to_owned()), value);
    }
    let mut mapping = serde_yaml_ng::Value::Mapping(mapping);
    let yaml = serde_yaml_ng::to_string(&mapping).map_err(|_| FormatError::ObjectSerialization)?;
    if yaml.ends_with('\n') {
        return Ok(yaml);
    }
    let last = last_string(&mut mapping).ok_or(FormatError::ObjectSerialization)?;
    let text = std::mem::take(last);
    let yaml = serde_yaml_ng::to_string(&mapping).map_err(|_| FormatError::ObjectSerialization)?;
    let before = yaml
        .strip_suffix("''\n")
        .ok_or(FormatError::ObjectSerialization)?;
    Ok(format!("{before}{}\n", double_quoted(&text)))
}

/// The last scalar libyaml writes for `value`, when it's a string. A mapping
/// writes each key before its value, so its last scalar is a value.
fn last_string(value: &mut serde_yaml_ng::Value) -> Option<&mut String> {
    match value {
        serde_yaml_ng::Value::String(text) => Some(text),
        serde_yaml_ng::Value::Sequence(items) => items.last_mut().and_then(last_string),
        serde_yaml_ng::Value::Mapping(map) => map.values_mut().last().and_then(last_string),
        _ => None,
    }
}

/// Writes `text` as a double-quoted YAML scalar, escaping exactly what
/// libyaml escapes: characters outside YAML's printable set, the byte order
/// mark, line breaks, `"` and `\`.
fn double_quoted(text: &str) -> String {
    use std::fmt::Write as _;

    let mut quoted = String::with_capacity(text.len() + 2);
    quoted.push('"');
    for character in text.chars() {
        match character {
            '\0' => quoted.push_str("\\0"),
            '\u{7}' => quoted.push_str("\\a"),
            '\u{8}' => quoted.push_str("\\b"),
            '\t' => quoted.push_str("\\t"),
            '\n' => quoted.push_str("\\n"),
            '\u{b}' => quoted.push_str("\\v"),
            '\u{c}' => quoted.push_str("\\f"),
            '\r' => quoted.push_str("\\r"),
            '\u{1b}' => quoted.push_str("\\e"),
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\u{85}' => quoted.push_str("\\N"),
            '\u{2028}' => quoted.push_str("\\L"),
            '\u{2029}' => quoted.push_str("\\P"),
            '\u{feff}' => quoted.push_str("\\uFEFF"),
            ' '..='~' | '\u{a0}'..='\u{d7ff}' | '\u{e000}'..='\u{fffd}' | '\u{10000}'.. => {
                quoted.push(character);
            }
            _ => {
                let code = u32::from(character);
                // Writing to a String can't fail.
                let _ = match code {
                    ..=0xff => write!(quoted, "\\x{code:02X}"),
                    0x100..=0xffff => write!(quoted, "\\u{code:04X}"),
                    _ => write!(quoted, "\\U{code:08X}"),
                };
            }
        }
    }
    quoted.push('"');
    quoted
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

/// Returns the canonical initial file location for a managed object, named
/// after its title, since the title is the file name. Project files are
/// folder notes, named like their folder, so the folder can hold related
/// files. The filesystem adapter picks another name when this one is taken.
pub fn default_managed_object_path(object_type: &str, title: &str) -> String {
    let name = normalize_file_name(title);
    let name = if name.is_empty() { "Untitled" } else { &name };
    if object_type == "project" {
        format!("projects/{name}/{name}.md")
    } else {
        format!("{object_type}s/{name}.md")
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
    fn managed_bodies_are_written_as_they_are() {
        let object = |body: &str| WorkspaceObject {
            id: "note_01j00000000000000000000000".into(),
            object_type: "note".into(),
            title: "Example".into(),
            body: body.into(),
            relative_path: "notes/example.md".into(),
            revision: String::new(),
            created: Some("2026-08-27T12:00:00Z".into()),
            updated: Some("2026-08-27T12:00:00Z".into()),
            properties: BTreeMap::new(),
        };
        let body_after_round_trip = |body: &str| {
            let bytes = serialize_object(&object(body)).unwrap();
            let ParsedMarkdown::Managed(parsed) = parse_markdown("notes/example.md", &bytes) else {
                panic!("managed object expected")
            };
            parsed.body
        };
        // A trailing space, or two for a Markdown line break, survives a save.
        assert_eq!(body_after_round_trip("Typing "), "Typing ");
        assert_eq!(body_after_round_trip("Line  \nnext"), "Line  \nnext");
        // Line breaks at the end, or none, stay as they are.
        assert_eq!(body_after_round_trip("Text\n\n"), "Text\n\n");
        let bytes = serialize_object(&object("Text")).unwrap();
        assert!(bytes.ends_with(b"---\nText"));
        assert_eq!(body_after_round_trip("  \n"), "  \n");
    }

    fn note(relative_path: &str, body: &str) -> WorkspaceObject {
        WorkspaceObject {
            id: "note_01j00000000000000000000000".into(),
            object_type: "note".into(),
            title: String::new(),
            body: body.into(),
            relative_path: relative_path.into(),
            revision: String::new(),
            created: Some("2026-08-27T12:00:00Z".into()),
            updated: Some("2026-08-27T12:00:00Z".into()),
            properties: BTreeMap::from([("custom".into(), serde_json::json!("kept"))]),
        }
    }

    #[test]
    fn a_title_is_the_file_name_and_a_heading_is_body_text() {
        let bytes = b"---\nid: note_01j00000000000000000000000\ntype: note\n---\n# Agenda\n\nBody";
        let ParsedMarkdown::Managed(object) = parse_markdown("notes/Team sync.v2.md", bytes) else {
            panic!("managed object expected")
        };
        assert_eq!(object.title, "Team sync.v2");
        assert_eq!(object.body, "# Agenda\n\nBody");
        let ParsedMarkdown::Unmanaged { title, body, .. } =
            parse_markdown("Plain.md", b"# Heading\nText")
        else {
            panic!("unmanaged file expected")
        };
        assert_eq!(
            (title.as_str(), body.as_str()),
            ("Plain", "# Heading\nText")
        );
    }

    #[test]
    fn serialization_writes_no_heading_and_the_body_as_given() {
        let mut object = note("notes/Plan.md", "Body without a final newline");
        object.title = "Ignored title".into();
        let bytes = String::from_utf8(serialize_object(&object).unwrap()).unwrap();
        assert_eq!(
            bytes,
            "---\nid: note_01j00000000000000000000000\ntype: note\ncustom: kept\n\
             created: 2026-08-27T12:00:00Z\nupdated: 2026-08-27T12:00:00Z\n---\n\
             Body without a final newline"
        );
        assert!(!bytes.contains("# "));
    }

    #[test]
    fn a_byte_order_mark_before_the_frontmatter_keeps_a_file_managed() {
        let bytes = b"\xEF\xBB\xBF---\r\nid: note_01j00000000000000000000000\r\ntype: note\r\n---\r\nBody\r\n";
        let ParsedMarkdown::Managed(object) = parse_markdown("Note.md", bytes) else {
            panic!("managed object expected")
        };
        assert_eq!(object.body, "Body\n");
        assert_eq!(object.revision, revision(bytes));
    }

    #[test]
    fn a_rewrite_keeps_the_body_bytes_line_endings_bom_and_final_newline() {
        let existing = "\u{feff}---\r\nid: note_01j00000000000000000000000\r\ntype: note\r\n\
            custom: kept\r\ncreated: 2026-08-27T12:00:00Z\r\nupdated: 2026-08-27T12:00:00Z\r\n\
            ---\r\n\r\n# Heading\r\nLine\nmixed\r\nno final newline";
        let ParsedMarkdown::Managed(mut object) = parse_markdown("Note.md", existing.as_bytes())
        else {
            panic!("managed object expected")
        };
        object
            .properties
            .insert("status".into(), serde_json::json!("done"));
        let written =
            String::from_utf8(serialize_object_over(existing.as_bytes(), &object).unwrap())
                .unwrap();
        assert_eq!(
            written,
            "\u{feff}---\r\nid: note_01j00000000000000000000000\r\ntype: note\r\n\
             status: done\r\ncustom: kept\r\ncreated: 2026-08-27T12:00:00Z\r\n\
             updated: 2026-08-27T12:00:00Z\r\n---\r\n\r\n# Heading\r\nLine\nmixed\r\n\
             no final newline"
        );
    }

    #[test]
    fn a_rewrite_keeps_unchanged_frontmatter_bytes_and_changes_only_the_body() {
        let existing = "---\nid: note_01j00000000000000000000000\ntype: note\n\
            custom:   'hand written'   # a comment\n---\nOld body\r\nkept line\r\n";
        let ParsedMarkdown::Managed(mut object) = parse_markdown("Note.md", existing.as_bytes())
        else {
            panic!("managed object expected")
        };
        object.body = "New body\nkept line\n".into();
        let written =
            String::from_utf8(serialize_object_over(existing.as_bytes(), &object).unwrap())
                .unwrap();
        assert_eq!(
            written,
            "---\nid: note_01j00000000000000000000000\ntype: note\n\
             custom:   'hand written'   # a comment\n---\nNew body\r\nkept line\r\n"
        );
        // Nothing changed: the bytes stay as they are.
        let ParsedMarkdown::Managed(unchanged) = parse_markdown("Note.md", existing.as_bytes())
        else {
            panic!("managed object expected")
        };
        assert_eq!(
            serialize_object_over(existing.as_bytes(), &unchanged).unwrap(),
            existing.as_bytes()
        );
    }

    #[test]
    fn file_names_lose_separators_reserved_characters_leading_dots_and_spaces() {
        assert_eq!(normalize_file_name("  ..My/plan: v2?  "), "Myplan v2");
        assert_eq!(
            normalize_file_name("a\\b*c\"d<e>f|g\u{7}h\u{7f}"),
            "abcdefgh"
        );
        assert_eq!(normalize_file_name(" . . "), "");
        let long = "é".repeat(250);
        assert_eq!(normalize_file_name(&long).chars().count(), 200);
        assert_eq!(
            normalize_file_name(&format!("{} b", "a".repeat(199))),
            "a".repeat(199)
        );
        assert_eq!(
            normalize_file_name(&format!("{}. b", "a".repeat(198))),
            "a".repeat(198)
        );
    }

    #[test]
    fn file_names_lose_trailing_dots_and_dodge_windows_device_names() {
        assert_eq!(normalize_file_name("Plan."), "Plan");
        assert_eq!(normalize_file_name("Plan . . "), "Plan");
        assert_eq!(normalize_file_name("v1.2"), "v1.2");
        assert_eq!(normalize_file_name("CON"), "CON_");
        assert_eq!(normalize_file_name("nul"), "nul_");
        assert_eq!(normalize_file_name("Com1"), "Com1_");
        assert_eq!(normalize_file_name("lpt9.backup"), "lpt9_.backup");
        assert_eq!(normalize_file_name("con ."), "con_");
        assert_eq!(normalize_file_name("COM0"), "COM0");
        assert_eq!(normalize_file_name("CONSOLE"), "CONSOLE");
        assert_eq!(normalize_file_name("Auxiliary"), "Auxiliary");
        assert_eq!(
            default_managed_object_path("project", "Plan."),
            "projects/Plan/Plan.md"
        );
    }

    #[test]
    fn new_objects_are_named_after_their_title() {
        assert_eq!(
            default_managed_object_path("note", " Team: sync "),
            "notes/Team sync.md"
        );
        assert_eq!(
            default_managed_object_path("project", "Launch"),
            "projects/Launch/Launch.md"
        );
        assert_eq!(
            default_managed_object_path("task", "///"),
            "tasks/Untitled.md"
        );
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
        assert_eq!(object.title, "note");
        assert_eq!(object.body, "# Title\n\nBody\n");
    }

    #[derive(Deserialize)]
    struct Fixtures {
        manifest: Vec<Fixture>,
        object_id: Vec<ObjectIdFixture>,
        task_properties: Vec<TaskPropertiesFixture>,
        project_properties: Vec<TaskPropertiesFixture>,
        object_body: Vec<ObjectBodyFixture>,
        titles: Vec<TitleFixture>,
        file_names: Vec<FileNameFixture>,
        rewrites: Vec<RewriteFixture>,
    }

    #[derive(Deserialize)]
    struct TitleFixture {
        name: String,
        path: String,
        markdown: String,
        title: String,
        body: String,
    }

    #[derive(Deserialize)]
    struct FileNameFixture {
        typed: String,
        name: String,
    }

    #[derive(Deserialize)]
    struct RewriteFixture {
        name: String,
        path: String,
        existing: String,
        #[serde(default)]
        set_properties: BTreeMap<String, serde_json::Value>,
        body: Option<String>,
        expected: String,
    }

    fn conformance() -> Fixtures {
        serde_json::from_str(include_str!(
            "../../../docs/workspace-format/fixtures/conformance-v1.json"
        ))
        .unwrap()
    }

    #[test]
    fn titles_come_from_file_names_as_the_shared_fixtures_say() {
        for fixture in conformance().titles {
            let (title, body) = match parse_markdown(&fixture.path, fixture.markdown.as_bytes()) {
                ParsedMarkdown::Managed(object) => (object.title, object.body),
                ParsedMarkdown::Unmanaged { title, body, .. } => (title, body),
                ParsedMarkdown::Malformed { error, .. } => panic!("{}: {error}", fixture.name),
            };
            assert_eq!(
                (title, body),
                (fixture.title, fixture.body),
                "{}",
                fixture.name
            );
        }
    }

    #[test]
    fn typed_names_become_file_names_as_the_shared_fixtures_say() {
        for fixture in conformance().file_names {
            assert_eq!(
                normalize_file_name(&fixture.typed),
                fixture.name,
                "{:?}",
                fixture.typed
            );
        }
    }

    #[test]
    fn rewrites_keep_bytes_as_the_shared_fixtures_say() {
        for fixture in conformance().rewrites {
            let ParsedMarkdown::Managed(mut object) =
                parse_markdown(&fixture.path, fixture.existing.as_bytes())
            else {
                panic!("{}: managed object expected", fixture.name)
            };
            object.properties.extend(fixture.set_properties);
            if let Some(body) = fixture.body {
                object.body = body;
            }
            let written = serialize_object_over(fixture.existing.as_bytes(), &object).unwrap();
            assert_eq!(
                String::from_utf8(written).unwrap(),
                fixture.expected,
                "{}",
                fixture.name
            );
        }
    }

    #[derive(Deserialize)]
    struct ObjectBodyFixture {
        name: String,
        body: String,
        expected_body: String,
        file_ends_with: String,
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
    fn object_bodies_round_trip_as_the_shared_fixtures_say() {
        let fixtures: Fixtures = serde_json::from_str(include_str!(
            "../../../docs/workspace-format/fixtures/conformance-v1.json"
        ))
        .unwrap();
        for fixture in fixtures.object_body {
            let object = WorkspaceObject {
                id: "note_01j00000000000000000000000".into(),
                object_type: "note".into(),
                title: "Example".into(),
                body: fixture.body,
                relative_path: "notes/example.md".into(),
                revision: String::new(),
                created: Some("2026-08-27T12:00:00Z".into()),
                updated: Some("2026-08-27T12:00:00Z".into()),
                properties: BTreeMap::new(),
            };
            let bytes = serialize_object(&object).unwrap();
            let text = String::from_utf8(bytes.clone()).unwrap();
            assert!(text.ends_with(&fixture.file_ends_with), "{}", fixture.name);
            let ParsedMarkdown::Managed(parsed) = parse_markdown(&object.relative_path, &bytes)
            else {
                panic!("{}: managed object expected", fixture.name)
            };
            assert_eq!(parsed.body, fixture.expected_body, "{}", fixture.name);
        }
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
        assert_eq!(note.relative_path, "notes/Browser note.md");
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
        assert_eq!(task.relative_path, "tasks/Browser task.md");
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
        assert_eq!(
            project.relative_path,
            "projects/Browser project/Browser project.md"
        );
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

    proptest::proptest! {
        /// libyaml double-quotes any string that holds U+0085, so its output
        /// for such strings shows the escapes `double_quoted` has to match.
        #[test]
        fn double_quoted_strings_match_libyaml(
            before in proptest::collection::vec(proptest::char::any(), 0..16),
            after in proptest::collection::vec(proptest::char::any(), 0..16),
        ) {
            let text = format!(
                "{}\u{85}{}",
                String::from_iter(before),
                String::from_iter(after)
            );
            let libyaml = serde_yaml_ng::to_string(&text).unwrap();
            proptest::prop_assert_eq!(libyaml, format!("{}\n", double_quoted(&text)));
        }
    }

    #[test]
    fn double_quoted_strings_read_back_unchanged() {
        let text = "\0\u{7}\u{8}\t\n\u{b}\u{c}\r\u{1b}\"\\\u{7f}\u{85}\u{9f}\u{a0}\u{2028}\u{2029}\u{feff}\u{fffe}\u{e000}\u{1f600}'#: -";
        let quoted = double_quoted(text);
        assert_eq!(serde_yaml_ng::from_str::<String>(&quoted).unwrap(), text);
    }

    /// Text heavy on the line breaks libyaml writes raw or escapes.
    fn line_break_text() -> impl proptest::strategy::Strategy<Value = String> {
        use proptest::strategy::Strategy as _;
        proptest::collection::vec(
            proptest::sample::select(
                &[
                    "\u{2028}", "\u{2029}", "\n", "\r", "\u{85}", " ", "a", "'", "- ", "---",
                ][..],
            ),
            0..8,
        )
        .prop_map(|pieces| pieces.concat())
    }

    fn line_break_value() -> impl proptest::strategy::Strategy<Value = serde_json::Value> {
        use proptest::strategy::Strategy as _;
        let leaf = proptest::prop_oneof![
            1 => proptest::strategy::Just(serde_json::Value::Null),
            4 => line_break_text().prop_map(serde_json::Value::String),
        ];
        leaf.prop_recursive(3, 12, 3, |inner| {
            proptest::prop_oneof![
                proptest::collection::vec(inner.clone(), 0..3).prop_map(serde_json::Value::Array),
                proptest::collection::btree_map(line_break_text(), inner, 0..3)
                    .prop_map(|map| serde_json::Value::Object(map.into_iter().collect())),
            ]
        })
    }

    proptest::proptest! {
        /// The frontmatter is libyaml's output unless that output lacks a
        /// final line feed. Then it still reads back as the same mapping.
        #[test]
        fn frontmatter_keeps_libyaml_bytes_unless_they_lack_a_final_line_feed(
            entries in proptest::collection::vec((line_break_text(), line_break_value()), 0..4),
        ) {
            let entries = entries
                .iter()
                .map(|(key, value)| (key.as_str(), value.clone()))
                .collect::<Vec<_>>();
            let mut mapping = serde_yaml_ng::Mapping::new();
            for (key, value) in &entries {
                mapping.insert(
                    serde_yaml_ng::Value::String((*key).to_owned()),
                    serde_yaml_ng::to_value(value).unwrap(),
                );
            }
            let libyaml = serde_yaml_ng::to_string(&mapping).unwrap();
            let written = frontmatter_yaml(&entries).unwrap();
            if libyaml.ends_with('\n') {
                proptest::prop_assert_eq!(written, libyaml);
            } else {
                proptest::prop_assert!(written.ends_with("\"\n"), "{:?}", written);
                let read: serde_yaml_ng::Mapping = serde_yaml_ng::from_str(&written).unwrap();
                proptest::prop_assert_eq!(read, mapping);
            }
        }
    }

    #[test]
    fn only_a_last_literal_block_ending_in_a_line_separator_is_double_quoted() {
        let mut object = WorkspaceObject {
            id: "note_01j00000000000000000000000".into(),
            object_type: "note".into(),
            title: "example".into(),
            body: String::new(),
            relative_path: "notes/example.md".into(),
            revision: String::new(),
            created: None,
            updated: Some("\na\u{2028}".into()),
            properties: BTreeMap::from([
                ("a".into(), serde_json::json!("Boil.\nServe.\u{2028}")),
                (
                    "b".into(),
                    serde_json::json!(["x\u{2029}", {"k": "y\n\u{2029}"}]),
                ),
            ]),
        };
        let bytes = String::from_utf8(serialize_object(&object).unwrap()).unwrap();
        // libyaml's own forms, with raw separators, except for the last value.
        assert_eq!(
            bytes,
            "---\nid: note_01j00000000000000000000000\ntype: note\na: |\n  Boil.\n  Serve.\u{2028}b:\n- 'x\u{2029}  '\n- k: |+\n    y\n\u{2029}updated: \"\\na\\L\"\n---\n"
        );
        let ParsedMarkdown::Managed(parsed) = parse_markdown("notes/example.md", bytes.as_bytes())
        else {
            panic!("the canonical bytes must stay managed");
        };
        object.revision = parsed.revision.clone();
        assert_eq!(parsed, object);
    }
}
