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
    /// `path` is the folder or file that `access` names.
    pub(crate) fn from_core(error: CoreError, path: &str, access: Access) -> Self {
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
            "symlink_escape" => match (access, error.operation.as_str()) {
                (Access::ReadFile, _) => {
                    format!("{path} is a link or sits in a linked folder, so Noura won't use it.")
                }
                (_, "workspace_create" | "workspace_open") => format!(
                    "The .noura folder or its settings file in {path} is a link, so Noura \
                     won't use it."
                ),
                _ => format!("Something inside {path} is a link, and Noura won't follow it."),
            },
            _ => match (error.category, access) {
                (ErrorCategory::Permission, Access::SetUpWorkspace) => format!(
                    "Noura doesn't have permission to add its .noura settings folder to {path}."
                ),
                (ErrorCategory::Permission, _) => {
                    format!("Noura doesn't have permission to read {path}.")
                }
                (_, Access::SetUpWorkspace) => {
                    format!("Noura couldn't set up a workspace in {path}.")
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

impl AppError {
    /// Translate a `local-core` error from a change to the file or folder
    /// at `path`, logging the details.
    pub(crate) fn from_change(error: CoreError, path: &str) -> Self {
        tracing::warn!(
            code = %error.code,
            operation = %error.operation,
            details = ?error.details,
            "a file operation failed"
        );
        let message = match error.code.as_str() {
            "unsupported_path" | "non_utf8_path" => format!(
                "{path} has characters in its name Noura can't represent, so Noura left it as \
                 it was."
            ),
            "symlink_escape" => {
                format!("{path} is a link or sits in a linked folder, so Noura won't change it.")
            }
            "empty_path" | "absolute_path" | "unsafe_path" => {
                "Noura only changes files inside this workspace.".to_owned()
            }
            "reserved_path" => "Noura doesn't keep files in .noura, .git, node_modules or \
                 target at the top of the workspace."
                .to_owned(),
            "path_exists" => format!("Something named {path} is already there."),
            "file_not_found" | "folder_not_found" | "object_not_found" => {
                format!("{path} isn't in the workspace anymore.")
            }
            "collaboration_transaction_required" => "Noura can't rename, move or delete a \
                 file while it's open for live editing."
                .to_owned(),
            "import_contains_workspace" => {
                "That folder holds this workspace, so Noura can't copy it here.".to_owned()
            }
            "import_into_itself" => "Noura can't copy a folder into itself.".to_owned(),
            "manifest_conflict" => "The settings file .noura/workspace.yaml changed on disk \
                 since Noura read it. Try again."
                .to_owned(),
            "invalid_ignore_pattern" => match error
                .details
                .as_ref()
                .and_then(|details| details.get("pattern"))
                .and_then(|pattern| pattern.as_str())
            {
                Some(pattern) => format!("\u{201c}{pattern}\u{201d} isn't a valid pattern."),
                None => "One of the patterns isn't valid.".to_owned(),
            },
            _ => match error.category {
                ErrorCategory::Permission => {
                    format!("Noura doesn't have permission to change {path}.")
                }
                _ => format!("Noura couldn't change {path}."),
            },
        };
        Self {
            code: error.code,
            message,
        }
    }
}

/// What the failed `local-core` call did with the path it names, so a
/// permission or link error says the right thing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Access {
    /// Opening or listing a workspace whose settings file exists. Noura
    /// only reads the folder.
    ReadWorkspace,
    /// Creating a workspace, or adding settings to a plain folder. Noura
    /// writes `.noura` into the folder.
    SetUpWorkspace,
    /// Reading the file at a workspace-relative path.
    ReadFile,
}

impl fmt::Display for AppError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for AppError {}
