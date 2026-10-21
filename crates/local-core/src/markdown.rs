use crate::{CoreError, ErrorCategory, Result, WorkspaceObject};

pub use workspace_format::{ParsedMarkdown, parse_markdown, revision};

/// Compatibility adapter for callers that consume local-core's structured IPC errors.
pub fn serialize_object(object: &WorkspaceObject) -> Result<Vec<u8>> {
    workspace_format::serialize_object(object).map_err(serialize_error)
}

/// Serializes `object` over the bytes its file holds now, keeping every
/// byte nobody changed: see [`workspace_format::serialize_object_over`].
pub fn serialize_object_over(existing: &[u8], object: &WorkspaceObject) -> Result<Vec<u8>> {
    workspace_format::serialize_object_over(existing, object).map_err(serialize_error)
}

fn serialize_error(error: workspace_format::FormatError) -> CoreError {
    match error {
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
    }
}
