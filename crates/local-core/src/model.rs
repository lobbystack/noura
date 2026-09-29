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

pub fn now_rfc3339() -> String {
    jiff::Timestamp::now().to_string()
}
