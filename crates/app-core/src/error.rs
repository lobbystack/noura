//! Errors that clients show to people.

use std::fmt;

use local_core::{CoreError, ErrorCategory};

/// A failure with a stable code and a message written for the person using
/// Noura. Raw OS and database details stay in the diagnostic log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppError {
    pub code: String,
    pub message: String,
}

impl AppError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_owned(),
            message: message.into(),
        }
    }

    /// The error for a path Noura can't represent without changing it.
    pub fn unsupported_path() -> Self {
        Self::new(
            "unsupported_path",
            "This folder's path has characters Noura can't represent. Rename the folder, \
             then try again.",
        )
    }

    /// Translate a `local-core` error into words, logging the details.
    pub(crate) fn from_core(error: CoreError, path: &str) -> Self {
        tracing::warn!(
            code = %error.code,
            operation = %error.operation,
            details = ?error.details,
            "a workspace operation failed"
        );
        let message = match error.code.as_str() {
            "workspace_exists" => {
                "This folder already has a workspace. Use Open folder to open it.".to_owned()
            }
            "workspace_name_required" => "Enter a name for the workspace.".to_owned(),
            "invalid_workspace_manifest"
            | "invalid_workspace_id"
            | "invalid_plugin_id"
            | "invalid_ignore_pattern" => "Noura can't read the settings file \
                 .noura/workspace.yaml, so it left the file as it was."
                .to_owned(),
            "unsupported_workspace_version" => "This version of Noura can't read this \
                 workspace's format, so it left the files as they were."
                .to_owned(),
            "workspace_identity_changed" => format!("A different workspace is now at {path}."),
            "unsupported_path" | "non_utf8_path" => return Self::unsupported_path(),
            "symlink_escape" => {
                "The folder's .noura folder is a link to somewhere else, so Noura won't use it."
                    .to_owned()
            }
            _ => match error.category {
                ErrorCategory::Permission => {
                    format!("Noura doesn't have permission to read {path}.")
                }
                _ => format!("Noura couldn't open {path}."),
            },
        };
        Self {
            code: error.code,
            message,
        }
    }
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}
