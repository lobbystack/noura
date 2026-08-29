use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum TaskStatus {
    Todo,
    InProgress,
    Done,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum TaskPriority {
    Low,
    Medium,
    High,
    Urgent,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectStatus {
    Planned,
    Active,
    OnHold,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, JsonSchema, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ParseStatus {
    Managed,
    Unmanaged,
    Malformed,
    Binary,
}

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

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct UnmanagedFile {
    pub relative_path: String,
    pub title: String,
    pub body: String,
    pub revision: String,
    pub parse_status: ParseStatus,
    pub parse_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspaceEntryKind {
    File,
    Folder,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEntry {
    pub relative_path: String,
    pub name: String,
    pub kind: WorkspaceEntryKind,
    pub parse_status: Option<ParseStatus>,
    pub object_id: Option<String>,
    pub object_type: Option<String>,
    pub revision: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum WorkspacePhase {
    Idle,
    Opening,
    Scanning,
    Indexing,
    Ready,
    Rebuilding,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceState {
    pub phase: WorkspacePhase,
    pub workspace_id: Option<String>,
    pub root_path: Option<String>,
    pub indexed_files: u64,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostic {
    pub code: String,
    pub message: String,
    pub relative_path: Option<String>,
    pub object_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum IndexStatus {
    Updated,
    RepairPending,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct MutationResult<T> {
    pub value: T,
    pub revision: String,
    pub durability: String,
    pub index_status: IndexStatus,
    pub warnings: Vec<CoreWarning>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CoreWarning {
    pub code: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FolderEntry {
    pub relative_path: String,
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CoreEvent {
    pub event_id: String,
    #[serde(rename = "type")]
    pub event_type: String,
    pub workspace_id: String,
    pub occurred_at: String,
    pub source: String,
    #[ts(type = "unknown")]
    pub payload: serde_json::Value,
}

pub fn new_object_id(object_type: &str) -> String {
    format!(
        "{}_{}",
        object_type,
        ulid::Ulid::new().to_string().to_lowercase()
    )
}

pub fn valid_object_id(id: &str, object_type: &str) -> bool {
    if !valid_object_type(object_type) {
        return false;
    }
    let Some(value) = id.strip_prefix(&format!("{object_type}_")) else {
        return false;
    };
    value.len() == 26 && value == value.to_ascii_lowercase() && value.parse::<ulid::Ulid>().is_ok()
}

pub fn valid_object_type(value: &str) -> bool {
    value.as_bytes().first().is_some_and(u8::is_ascii_lowercase)
        && value
            .bytes()
            .skip(1)
            .all(|byte| byte.is_ascii_lowercase() || byte == b'-')
}

pub fn now_rfc3339() -> String {
    jiff::Timestamp::now().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Fixtures {
        object_id: Vec<Fixture>,
    }
    #[derive(Deserialize)]
    struct Fixture {
        valid: bool,
        value: String,
    }

    #[test]
    fn rust_object_id_validation_matches_shared_conformance_fixtures() {
        let fixtures: Fixtures = serde_json::from_str(include_str!(
            "../../../docs/workspace-format/fixtures/conformance-v1.json"
        ))
        .unwrap();
        for fixture in fixtures.object_id {
            assert_eq!(valid_object_id(&fixture.value, "note"), fixture.valid);
        }
    }
}
