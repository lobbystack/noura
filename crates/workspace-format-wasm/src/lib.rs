//! WebAssembly bindings for Noura's canonical workspace-format implementation.
//!
//! This crate intentionally contains no format rules. It only translates the
//! JavaScript boundary to the platform-independent `workspace-format` crate.

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use wasm_bindgen::prelude::*;
use workspace_format::{
    CreateNoteInput, CreateProjectInput, CreateTaskInput, FormatError, UpdateNoteInput,
    UpdateProjectInput, UpdateTaskInput, WorkspaceManifest, WorkspaceObject,
    create_note as create_canonical_note, create_project as create_canonical_project,
    create_task as create_canonical_task,
    create_workspace_manifest as create_canonical_workspace_manifest, normalize_workspace_manifest,
    parse_markdown as parse_canonical_markdown,
    parse_workspace_manifest as parse_canonical_workspace_manifest, revision,
    serialize_object as serialize_canonical_object,
    serialize_workspace_manifest as serialize_canonical_workspace_manifest,
    update_note as update_canonical_note, update_project as update_canonical_project,
    update_task as update_canonical_task, valid_object_id, validate_managed_object_path,
    validate_workspace_manifest,
};

fn public_error(code: &'static str, message: &'static str) -> JsValue {
    let json = format!(r#"{{"code":"{code}","message":"{message}"}}"#);
    js_sys::JSON::parse(&json).unwrap_or_else(|_| JsValue::from_str("workspace format error"))
}

#[wasm_bindgen(
    inline_js = "export function secure_random_bytes() { const bytes = new Uint8Array(10); globalThis.crypto.getRandomValues(bytes); return bytes; }"
)]
extern "C" {
    fn secure_random_bytes() -> Vec<u8>;
}

fn generated_id(object_type: &str) -> String {
    // IDs describe the local mutation time, as native ULIDs do. `now` belongs
    // to durable object metadata and may legitimately predate the Unix epoch.
    let time = js_sys::Date::now() as u64;
    let bytes = secure_random_bytes();
    let random = bytes
        .into_iter()
        .fold(0u128, |value, byte| (value << 8) | u128::from(byte));
    format!(
        "{object_type}_{}",
        ulid::Ulid::from_parts(time, random)
            .to_string()
            .to_lowercase()
    )
}

fn format_error(error: FormatError) -> JsValue {
    match error {
        FormatError::InvalidObjectId => public_error(
            "invalid_object_id",
            "The stable ID does not match the object type",
        ),
        FormatError::ObjectSerialization => public_error(
            "object_serialization_failed",
            "Frontmatter could not be serialized",
        ),
        FormatError::InvalidManifest => {
            public_error("invalid_manifest", ".noura/workspace.yaml is invalid")
        }
        FormatError::InvalidWorkspaceId => public_error(
            "invalid_workspace_id",
            "The workspace ID must be a lowercase stable workspace ID",
        ),
        FormatError::WorkspaceNameRequired => {
            public_error("workspace_name_required", "A workspace name is required")
        }
        FormatError::InvalidPluginId => public_error(
            "invalid_plugin_id",
            "Plugin identifiers use lowercase letters, digits, and hyphens",
        ),
        FormatError::UnsupportedWorkspaceVersion => public_error(
            "unsupported_workspace_version",
            "This workspace format version is not supported",
        ),
        FormatError::ManifestSerialization => public_error(
            "manifest_serialization_failed",
            ".noura/workspace.yaml could not be serialized",
        ),
        FormatError::TitleRequired => public_error("title_required", "A title is required"),
        FormatError::InvalidTimestamp => public_error(
            "invalid_timestamp",
            "A timestamp must be an RFC 3339 instant",
        ),
        FormatError::InvalidTaskStatus => public_error("invalid_field", "Unsupported status value"),
        FormatError::InvalidTaskStatusType => {
            public_error("invalid_field", "status must be a string")
        }
        FormatError::InvalidTaskPriority => {
            public_error("invalid_field", "Unsupported priority value")
        }
        FormatError::InvalidTaskPriorityType => {
            public_error("invalid_field", "priority must be a string")
        }
        FormatError::InvalidTaskDue => public_error(
            "invalid_date",
            "due must use YYYY-MM-DD or RFC 3339 with an explicit offset",
        ),
        FormatError::InvalidTaskProject => public_error(
            "invalid_project_id",
            "Task project references use a stable project ID",
        ),
        FormatError::InvalidProjectStatus => {
            public_error("invalid_field", "Unsupported project status value")
        }
        FormatError::InvalidProjectStatusType => {
            public_error("invalid_field", "project status must be a string")
        }
    }
}

fn invalid_input() -> JsValue {
    public_error(
        "invalid_input",
        "The workspace format input has an invalid shape",
    )
}

fn internal_error() -> JsValue {
    public_error(
        "internal_error",
        "The workspace format operation could not be completed",
    )
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct ManifestUpdateInput {
    name: Option<String>,
    enabled_plugins: Option<Vec<String>>,
    ignore: Option<Vec<String>>,
}

fn from_js<T: DeserializeOwned>(value: JsValue) -> Result<T, JsValue> {
    let json = js_sys::JSON::stringify(&value).map_err(|_| invalid_input())?;
    let Some(json) = json.as_string() else {
        return Err(invalid_input());
    };
    serde_json::from_str(&json).map_err(|_| invalid_input())
}

fn to_js<T: Serialize>(value: &T) -> Result<JsValue, JsValue> {
    let json = serde_json::to_string(value).map_err(|_| internal_error())?;
    js_sys::JSON::parse(&json).map_err(|_| internal_error())
}

/// Parses Markdown with the canonical workspace-format parser.
#[wasm_bindgen]
pub fn parse_markdown(relative_path: &str, bytes: &[u8]) -> Result<JsValue, JsValue> {
    to_js(&parse_canonical_markdown(relative_path, bytes))
}

/// Serializes a managed object with the canonical workspace-format serializer.
#[wasm_bindgen]
pub fn serialize_object(object: JsValue) -> Result<Vec<u8>, JsValue> {
    let object = from_js::<WorkspaceObject>(object)?;
    serialize_canonical_object(&object).map_err(format_error)
}

/// Parses and normalizes a workspace manifest with the canonical parser.
#[wasm_bindgen]
pub fn parse_workspace_manifest(bytes: &[u8]) -> Result<JsValue, JsValue> {
    let manifest = parse_canonical_workspace_manifest(bytes).map_err(format_error)?;
    to_js(&manifest)
}

/// Validates, normalizes, and serializes a workspace manifest canonically.
#[wasm_bindgen]
pub fn serialize_workspace_manifest(manifest: JsValue) -> Result<String, JsValue> {
    let mut manifest = from_js::<WorkspaceManifest>(manifest)?;
    validate_workspace_manifest(&manifest).map_err(format_error)?;
    if manifest.format_version != 1 {
        return Err(format_error(FormatError::UnsupportedWorkspaceVersion));
    }
    normalize_workspace_manifest(&mut manifest);
    serialize_canonical_workspace_manifest(&manifest).map_err(format_error)
}

/// Applies the native manifest update rules before browser storage writes its
/// canonical bytes. Revision comparison remains at the storage boundary.
#[wasm_bindgen]
pub fn update_workspace_manifest(
    manifest: JsValue,
    input: JsValue,
    now: String,
) -> Result<JsValue, JsValue> {
    let mut manifest = from_js::<WorkspaceManifest>(manifest)?;
    let input = from_js::<ManifestUpdateInput>(input)?;
    if input.name.is_none() && input.enabled_plugins.is_none() && input.ignore.is_none() {
        return to_js(&manifest);
    }
    if let Some(name) = input.name {
        let name = name.trim();
        if name.is_empty() {
            return Err(format_error(FormatError::WorkspaceNameRequired));
        }
        manifest.name = name.to_owned();
    }
    if let Some(enabled_plugins) = input.enabled_plugins {
        manifest.enabled_plugins = enabled_plugins;
    }
    if let Some(ignore) = input.ignore {
        if ignore.iter().any(|pattern| pattern.trim().is_empty()) {
            return Err(public_error(
                "invalid_ignore_pattern",
                "Workspace ignore patterns must not be empty",
            ));
        }
        manifest.ignore = ignore;
    }
    manifest.updated = now;
    validate_workspace_manifest(&manifest).map_err(format_error)?;
    if manifest.format_version != 1 {
        return Err(format_error(FormatError::UnsupportedWorkspaceVersion));
    }
    normalize_workspace_manifest(&mut manifest);
    to_js(&manifest)
}

/// Returns the revision used for external-edit detection.
#[wasm_bindgen]
pub fn content_revision(bytes: &[u8]) -> String {
    revision(bytes)
}

/// Checks a stable object ID against its object type.
#[wasm_bindgen]
pub fn is_valid_object_id(id: &str, object_type: &str) -> bool {
    valid_object_id(id, object_type)
}

/// Checks the shared destination policy for a managed object path.
#[wasm_bindgen]
pub fn is_valid_managed_object_path(path: &str) -> bool {
    validate_managed_object_path(path).is_ok()
}

/// Constructs a new workspace manifest through portable Rust domain rules.
#[wasm_bindgen]
pub fn create_workspace_manifest(name: String, now: String) -> Result<JsValue, JsValue> {
    let id = generated_id("workspace");
    to_js(&create_canonical_workspace_manifest(id, name, now).map_err(format_error)?)
}

/// Constructs a new note through portable Rust domain rules.
#[wasm_bindgen]
pub fn create_note(input: JsValue) -> Result<JsValue, JsValue> {
    let input = from_js::<CreateNoteInput>(input)?;
    let id = generated_id("note");
    to_js(&create_canonical_note(id, input).map_err(format_error)?)
}

/// Applies a note edit through portable Rust domain rules.
#[wasm_bindgen]
pub fn update_note(note: JsValue, input: JsValue) -> Result<JsValue, JsValue> {
    let note = from_js::<WorkspaceObject>(note)?;
    let input = from_js::<UpdateNoteInput>(input)?;
    to_js(&update_canonical_note(note, input).map_err(format_error)?)
}

/// Constructs a new task through portable Rust domain rules.
#[wasm_bindgen]
pub fn create_task(input: JsValue) -> Result<JsValue, JsValue> {
    let input = from_js::<CreateTaskInput>(input)?;
    let id = generated_id("task");
    to_js(&create_canonical_task(id, input).map_err(format_error)?)
}

/// Applies a task edit through portable Rust domain rules.
#[wasm_bindgen]
pub fn update_task(task: JsValue, input: JsValue) -> Result<JsValue, JsValue> {
    let task = from_js::<WorkspaceObject>(task)?;
    let input = from_js::<UpdateTaskInput>(input)?;
    to_js(&update_canonical_task(task, input).map_err(format_error)?)
}

/// Constructs a new project through portable Rust domain rules.
#[wasm_bindgen]
pub fn create_project(input: JsValue) -> Result<JsValue, JsValue> {
    let input = from_js::<CreateProjectInput>(input)?;
    let id = generated_id("project");
    to_js(&create_canonical_project(id, input).map_err(format_error)?)
}

/// Applies a project edit through portable Rust domain rules.
#[wasm_bindgen]
pub fn update_project(project: JsValue, input: JsValue) -> Result<JsValue, JsValue> {
    let project = from_js::<WorkspaceObject>(project)?;
    let input = from_js::<UpdateProjectInput>(input)?;
    to_js(&update_canonical_project(project, input).map_err(format_error)?)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;
    use workspace_format::{
        WorkspaceManifest, normalize_project_properties, valid_object_id,
        validate_workspace_manifest,
    };

    #[derive(Deserialize)]
    struct Fixtures {
        manifest: Vec<ManifestFixture>,
        object_id: Vec<ObjectIdFixture>,
        project_properties: Vec<PropertiesFixture>,
    }

    #[derive(Deserialize)]
    struct ManifestFixture {
        valid: bool,
        value: serde_json::Value,
    }

    #[derive(Deserialize)]
    struct ObjectIdFixture {
        valid: bool,
        value: String,
    }

    #[derive(Deserialize)]
    struct PropertiesFixture {
        valid: bool,
        value: std::collections::BTreeMap<String, serde_json::Value>,
    }

    #[test]
    fn binding_uses_shared_conformance_rules() {
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
        let project_properties_match = fixtures.project_properties.into_iter().all(|fixture| {
            let mut properties = fixture.value;
            normalize_project_properties(&mut properties).is_ok() == fixture.valid
        });

        assert!(manifests_match && object_ids_match && project_properties_match);
    }
}
