use crate::{CoreError, ErrorCategory, Result, WorkspaceObject};

pub use workspace_format::{ParsedMarkdown, parse_markdown, revision};

/// Compatibility adapter for callers that consume local-core's structured IPC errors.
pub fn serialize_object(object: &WorkspaceObject) -> Result<Vec<u8>> {
    workspace_format::serialize_object(object).map_err(|error| match error {
        workspace_format::FormatError::InvalidObjectId => CoreError::new(
            "invalid_object_id",
            ErrorCategory::Identity,
            "The stable ID does not match the object type",
            "serialize_object",
        ),
        workspace_format::FormatError::ObjectSerialization => CoreError::new(
            "serialize_failed",
            ErrorCategory::Parse,
            "Frontmatter could not be serialized",
            "serialize_object",
        ),
        _ => CoreError::new(
            "serialize_failed",
            ErrorCategory::Parse,
            "Frontmatter could not be serialized",
            "serialize_object",
        ),
    })
}
