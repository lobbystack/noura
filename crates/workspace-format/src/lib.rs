//! Platform-independent parsing, validation, and canonical serialization for
//! Noura workspace files.
//!
//! This crate deliberately has no filesystem, database, runtime, or native
//! credential dependencies so the same durable format logic can run on native
//! clients and WebAssembly clients.

use std::{
    borrow::Cow,
    collections::{BTreeMap, HashSet},
    ops::RangeInclusive,
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

/// Reads a run of carriage returns right before a line feed as one line
/// break, for the title and body. Reading only CRLF as LF would leave `\r\n`
/// of a `\r\r\n` in the body. The serializer writes that back as it is, and
/// the next read would drop one more carriage return, so the bytes would
/// change on every write. The frontmatter still reads only CRLF as LF, so
/// no file moves between managed, malformed and unmanaged.
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
        let (title, body) = split_title(&single_line_breaks(text));
        return ParsedMarkdown::Unmanaged {
            title,
            body,
            frontmatter: None,
        };
    }
    let Some(end) = text[4..].find("\n---\n").map(|index| index + 4) else {
        let (title, body) = split_title(&single_line_breaks(text));
        return ParsedMarkdown::Malformed {
            title,
            body,
            error: "Frontmatter has no closing delimiter".into(),
        };
    };
    // The line break before the closing delimiter ends the last YAML line.
    // Without it, a block scalar written last would lose its final line break.
    let yaml = &text[4..=end];
    let markdown = single_line_breaks(&text[end + 5..]);
    let (title, body) = split_title(markdown.trim_start_matches('\n'));
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

/// Writes the managed-object envelope: `id` and `type`, the `leading`
/// properties in that order, the other properties sorted by key, `created`
/// and `updated`, then the title heading and the body.
fn serialize_envelope(object: &WorkspaceObject, leading: &[&str]) -> Result<Vec<u8>, FormatError> {
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
    let mut result = format!("---\n{}---\n\n# {}\n", yaml, object.title.trim());
    // Only line breaks at the end are normalized. Trailing spaces stay: two
    // of them are a Markdown line break, and the editor may be mid-typing.
    let body = object.body.trim_end_matches(['\n', '\r']);
    if !body.trim().is_empty() {
        result.push('\n');
        result.push_str(body);
        result.push('\n');
    }
    Ok(result.into_bytes())
}

/// U+2028 and U+2029 are line breaks to YAML 1.1 readers such as libyaml and
/// ordinary characters to YAML 1.2 readers. libyaml writes them raw in plain,
/// single-quoted and literal scalars, and a literal scalar that ends in one
/// leaves the YAML without the line break the closing `---` needs. U+0085 is
/// the third such character, but libyaml already double-quotes it.
const LINE_SEPARATORS: [char; 2] = ['\u{2028}', '\u{2029}'];

/// The private-use characters that stand-in markers are built from, in the
/// order they're tried. libyaml writes them raw in plain scalars.
const STAND_IN_CHARACTERS: [RangeInclusive<char>; 3] = [
    '\u{e000}'..='\u{f8ff}',
    '\u{f0000}'..='\u{ffffd}',
    '\u{100000}'..='\u{10fffd}',
];

/// Writes frontmatter entries as a YAML mapping, in order.
///
/// A string value that holds a line separator is written double-quoted with
/// libyaml's escapes. Mapping keys go to libyaml as they are, and libyaml
/// reads its own output back as the same key. libyaml doesn't let a caller
/// pick a scalar style, so each such string goes to it as a plain stand-in,
/// and one pass over its output swaps the quoted strings in.
fn frontmatter_yaml(entries: &[(&str, serde_json::Value)]) -> Result<String, FormatError> {
    let mut stand_ins = StandIns::for_entries(entries)?;
    let mut mapping = serde_yaml_ng::Mapping::with_capacity(entries.len());
    for (key, value) in entries {
        let key = serde_yaml_ng::Value::String((*key).to_owned());
        mapping.insert(key, stand_ins.value(value)?);
    }
    let yaml = serde_yaml_ng::to_string(&mapping).map_err(|_| FormatError::ObjectSerialization)?;
    stand_ins.restore(&yaml)
}

/// Plain scalars that stand in for double-quoted strings while libyaml
/// writes the frontmatter.
///
/// A stand-in is the marker, a decimal index and the marker again. The
/// marker is two private-use characters that never follow each other in any
/// key or string of the frontmatter. libyaml writes those characters raw and
/// adds only ASCII around and inside the scalars it writes, so the marker
/// shows up in its output only inside stand-ins. The marker stays two
/// characters long whatever the input holds, which keeps stand-ins short.
struct StandIns {
    marker: String,
    quoted: Vec<String>,
}

impl StandIns {
    /// Picks a marker when some string value holds a line separator. Keys
    /// can't hold the marker either, since libyaml writes them raw too.
    fn for_entries(entries: &[(&str, serde_json::Value)]) -> Result<Self, FormatError> {
        let mut texts = Vec::new();
        let mut needs_quotes = false;
        for (key, value) in entries {
            texts.push(*key);
            visit_text(value, &mut |text, is_key| {
                needs_quotes |= !is_key && text.contains(LINE_SEPARATORS);
                texts.push(text);
            });
        }
        let marker = if needs_quotes {
            unused_pair(&texts).ok_or(FormatError::ObjectSerialization)?
        } else {
            String::new()
        };
        Ok(Self {
            marker,
            quoted: Vec::new(),
        })
    }

    /// Converts a JSON value to YAML, putting a stand-in in place of each
    /// string value that holds a line separator.
    fn value(&mut self, value: &serde_json::Value) -> Result<serde_yaml_ng::Value, FormatError> {
        Ok(match value {
            serde_json::Value::String(text) => self.string(text),
            serde_json::Value::Array(items) => serde_yaml_ng::Value::Sequence(
                items
                    .iter()
                    .map(|item| self.value(item))
                    .collect::<Result<_, _>>()?,
            ),
            serde_json::Value::Object(map) => {
                let mut mapping = serde_yaml_ng::Mapping::with_capacity(map.len());
                for (key, item) in map {
                    mapping.insert(serde_yaml_ng::Value::String(key.clone()), self.value(item)?);
                }
                serde_yaml_ng::Value::Mapping(mapping)
            }
            other => {
                serde_yaml_ng::to_value(other).map_err(|_| FormatError::ObjectSerialization)?
            }
        })
    }

    fn string(&mut self, text: &str) -> serde_yaml_ng::Value {
        if !text.contains(LINE_SEPARATORS) {
            return serde_yaml_ng::Value::String(text.to_owned());
        }
        let index = self.quoted.len();
        self.quoted.push(double_quoted(text));
        serde_yaml_ng::Value::String(format!("{0}{index}{0}", self.marker))
    }

    /// Swaps each stand-in in libyaml's output for its quoted string, in one
    /// pass.
    fn restore(self, yaml: &str) -> Result<String, FormatError> {
        if self.quoted.is_empty() {
            return Ok(yaml.to_owned());
        }
        let quoted_len = self.quoted.iter().map(String::len).sum::<usize>();
        let mut restored = String::with_capacity(yaml.len() + quoted_len);
        let mut rest = yaml;
        let mut count = 0;
        while let Some(start) = rest.find(self.marker.as_str()) {
            restored.push_str(&rest[..start]);
            let after = &rest[start + self.marker.len()..];
            let digits = after.bytes().take_while(u8::is_ascii_digit).count();
            let quoted = after[..digits]
                .parse::<usize>()
                .ok()
                .and_then(|index| self.quoted.get(index))
                .ok_or(FormatError::ObjectSerialization)?;
            restored.push_str(quoted);
            rest = after[digits..]
                .strip_prefix(self.marker.as_str())
                .ok_or(FormatError::ObjectSerialization)?;
            count += 1;
        }
        if count != self.quoted.len() {
            return Err(FormatError::ObjectSerialization);
        }
        restored.push_str(rest);
        Ok(restored)
    }
}

/// Calls `visit` with every string and mapping key inside `value`, and
/// whether it is a key.
fn visit_text<'a>(value: &'a serde_json::Value, visit: &mut impl FnMut(&'a str, bool)) {
    match value {
        serde_json::Value::String(text) => visit(text, false),
        serde_json::Value::Array(items) => {
            for item in items {
                visit_text(item, visit);
            }
        }
        serde_json::Value::Object(map) => {
            for (key, item) in map {
                visit(key, true);
                visit_text(item, visit);
            }
        }
        _ => {}
    }
}

/// Returns the first pair of stand-in characters, in order, that never
/// follow each other in `texts`. Every pair tried before it follows each
/// other somewhere in `texts`, so the search makes at most one try per
/// character of `texts`, plus one.
fn unused_pair(texts: &[&str]) -> Option<String> {
    let is_stand_in_character = |character: &char| {
        STAND_IN_CHARACTERS
            .iter()
            .any(|range| range.contains(character))
    };
    let mut used = HashSet::new();
    for text in texts {
        let mut previous = None;
        for character in text.chars() {
            let current = Some(character).filter(is_stand_in_character);
            if let (Some(first), Some(second)) = (previous, current) {
                used.insert((first, second));
            }
            previous = current;
        }
    }
    STAND_IN_CHARACTERS
        .into_iter()
        .flatten()
        .flat_map(|first| {
            STAND_IN_CHARACTERS
                .into_iter()
                .flatten()
                .map(move |second| (first, second))
        })
        .find(|pair| !used.contains(pair))
        .map(|(first, second)| String::from_iter([first, second]))
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
    fn managed_bodies_keep_trailing_spaces_and_drop_trailing_line_breaks() {
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
        // Line breaks at the end are normalized; the file ends with one.
        assert_eq!(body_after_round_trip("Text\n\n"), "Text");
        let bytes = serialize_object(&object("Text\n\n")).unwrap();
        assert!(bytes.ends_with(b"Text\n"));
        // A body of only whitespace is written as no body.
        assert_eq!(body_after_round_trip("  \n"), "");
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
        object_body: Vec<ObjectBodyFixture>,
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

    #[test]
    fn line_separators_in_string_values_are_double_quoted() {
        let mut object = WorkspaceObject {
            id: "note_01j00000000000000000000000".into(),
            object_type: "note".into(),
            title: "Example".into(),
            body: String::new(),
            relative_path: "notes/example.md".into(),
            revision: String::new(),
            created: None,
            updated: Some("\na\u{2028}".into()),
            properties: BTreeMap::from([
                ("a".into(), serde_json::json!("Boil.\nServe.\u{2028}")),
                (
                    "b".into(),
                    serde_json::json!(["x\u{2029}", {"k": "y\u{2029}"}]),
                ),
                (
                    "c".into(),
                    serde_json::json!("\u{e000}0\u{e000} stays plain text"),
                ),
            ]),
        };
        let bytes = String::from_utf8(serialize_object(&object).unwrap()).unwrap();
        assert_eq!(
            bytes,
            "---\nid: note_01j00000000000000000000000\ntype: note\na: \"Boil.\\nServe.\\L\"\nb:\n- \"x\\P\"\n- k: \"y\\P\"\nc: \u{e000}0\u{e000} stays plain text\nupdated: \"\\na\\L\"\n---\n\n# Example\n"
        );
        let ParsedMarkdown::Managed(parsed) = parse_markdown("notes/example.md", bytes.as_bytes())
        else {
            panic!("the canonical bytes must stay managed");
        };
        object.revision = parsed.revision.clone();
        assert_eq!(parsed, object);
    }

    #[test]
    fn stand_in_markers_stay_two_characters_whatever_the_input_holds() {
        assert_eq!(unused_pair(&["plain text"]).unwrap(), "\u{e000}\u{e000}");
        let run = "\u{e000}".repeat(10_000);
        assert_eq!(unused_pair(&[&run]).unwrap(), "\u{e000}\u{e001}");
        // Every pair that starts with U+E000 is taken, so the search walks
        // the whole first row before it moves to U+E001.
        let first_row = STAND_IN_CHARACTERS
            .into_iter()
            .flatten()
            .map(|second| format!("\u{e000}{second} "))
            .collect::<String>();
        assert_eq!(
            unused_pair(&[&first_row, &run]).unwrap(),
            "\u{e001}\u{e000}"
        );
    }

    #[test]
    fn stand_ins_never_match_text_that_holds_marker_characters() {
        let object = WorkspaceObject {
            id: "note_01j00000000000000000000000".into(),
            object_type: "note".into(),
            title: "Example".into(),
            body: String::new(),
            relative_path: "notes/example.md".into(),
            revision: String::new(),
            created: None,
            updated: None,
            properties: BTreeMap::from([
                (
                    "a".into(),
                    serde_json::json!("\u{e000}\u{e000}0\u{e000}\u{e000}"),
                ),
                (
                    "b".into(),
                    serde_json::json!("\u{e000}\u{e001}0\u{e000}\u{e001}"),
                ),
                (
                    "c\u{2028}".into(),
                    serde_json::json!({"\u{e000}\u{e000}1\u{e000}\u{e000}": "x\u{2029}"}),
                ),
            ]),
        };
        let bytes = String::from_utf8(serialize_object(&object).unwrap()).unwrap();
        assert_eq!(
            bytes,
            "---\nid: note_01j00000000000000000000000\ntype: note\na: \u{e000}\u{e000}0\u{e000}\u{e000}\nb: \u{e000}\u{e001}0\u{e000}\u{e001}\n? 'c\u{2028}  '\n: \u{e000}\u{e000}1\u{e000}\u{e000}: \"x\\P\"\n---\n\n# Example\n"
        );
    }
}
