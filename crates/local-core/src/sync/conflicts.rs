use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
pub struct SyncConflict {
    pub operation_id: String,
    pub path: String,
    pub current_revision: Option<String>,
    pub local_preview: Option<String>,
    pub remote_preview: Option<String>,
    pub local_deleted: bool,
    pub remote_deleted: bool,
    pub can_resolve: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "lowercase")]
pub enum SyncResolutionChoice {
    Local,
    Remote,
}

#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResolveSyncConflict {
    pub operation_id: String,
    pub current_revision: Option<String>,
    pub choice: SyncResolutionChoice,
}
