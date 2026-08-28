use serde::{Deserialize, Serialize};
use thiserror::Error;
use ts_rs::TS;

pub type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCategory {
    Validation,
    Filesystem,
    Permission,
    Parse,
    Identity,
    Index,
    Conflict,
    Credential,
    Provider,
    Transient,
}

#[derive(Debug, Clone, Serialize, Deserialize, Error, TS)]
#[ts(export)]
#[error("{message}")]
#[serde(rename_all = "camelCase")]
pub struct CoreError {
    pub code: String,
    pub category: ErrorCategory,
    pub message: String,
    pub retryable: bool,
    pub operation: String,
    pub workspace_id: Option<String>,
    pub object_id: Option<String>,
    pub path: Option<String>,
    #[ts(type = "Record<string, unknown> | undefined")]
    pub details: Option<serde_json::Value>,
}

impl CoreError {
    pub fn new(
        code: &str,
        category: ErrorCategory,
        message: impl Into<String>,
        operation: &str,
    ) -> Self {
        Self {
            code: code.into(),
            category,
            message: message.into(),
            retryable: false,
            operation: operation.into(),
            workspace_id: None,
            object_id: None,
            path: None,
            details: None,
        }
    }

    pub fn validation(code: &str, message: impl Into<String>, operation: &str) -> Self {
        Self::new(code, ErrorCategory::Validation, message, operation)
    }

    pub fn io(error: std::io::Error, operation: &str, path: Option<&str>) -> Self {
        let category = if error.kind() == std::io::ErrorKind::PermissionDenied {
            ErrorCategory::Permission
        } else {
            ErrorCategory::Filesystem
        };
        let code = if matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
        ) {
            "filesystem_transient"
        } else {
            "filesystem_error"
        };
        let mut value = Self::new(code, category, "The filesystem operation failed", operation);
        value.retryable = matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
        );
        value.path = path.map(str::to_owned);
        value
    }

    pub fn index(error: rusqlite::Error, operation: &str) -> Self {
        let mut value = Self::new(
            "index_error",
            ErrorCategory::Index,
            "The local index operation failed",
            operation,
        );
        value.details = Some(serde_json::json!({ "diagnostic": error.to_string() }));
        value
    }
}
