//! Platform-independent parsing, validation, and canonical serialization for
//! Noura workspace files.
//!
//! This crate deliberately has no filesystem, database, runtime, or native
//! credential dependencies so the same durable format logic can run on native
//! clients and WebAssembly clients.

use std::{borrow::Cow, collections::BTreeMap};

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
        assert!(manifests_match && object_ids_match);
    }

    #[test]
    fn manifest_parser_sorts_and_deduplicates_plugins() {
        let manifest = parse_workspace_manifest(
            b"id: workspace_01j00000000000000000000000\nformat_version: 1\nname: Example\ncreated: 2026-08-27T12:00:00Z\nupdated: 2026-08-27T12:00:00Z\nenabled_plugins: [tasks, notes, tasks]\nignore: []\n",
        )
        .unwrap();
        assert_eq!(manifest.enabled_plugins, ["notes", "tasks"]);
    }
}
