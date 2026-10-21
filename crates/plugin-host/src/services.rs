//! The services a plugin's capability interfaces call. The app provides
//! them; the host checks each call against the plugin's capabilities first
//! and converts what crosses the WIT boundary.

use std::fmt;

/// A workspace-relative path that crossed the plugin boundary: not empty,
/// with `/` between its parts, and no `.`, `..`, `\` or control
/// characters. The service still resolves it inside the workspace and
/// refuses links that leave it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspacePath(String);

impl WorkspacePath {
    /// Check `path`, or say why it isn't a workspace-relative path.
    pub fn new(path: &str) -> Result<Self, ServiceError> {
        let valid = !path.is_empty()
            && path.len() <= 4096
            && !path.starts_with('/')
            && !path.contains('\\')
            && path.split('/').all(|part| {
                !part.is_empty()
                    && part != "."
                    && part != ".."
                    && !part.chars().any(char::is_control)
            });
        if valid {
            Ok(Self(path.to_owned()))
        } else {
            Err(ServiceError::InvalidPath(path.to_owned()))
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for WorkspacePath {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// Why a service refused or failed a call, as the plugin sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServiceError {
    /// A name a WIT string can't hold, such as one that isn't UTF-8.
    UnsupportedPath,
    /// Not a workspace-relative path, or one inside a folder Noura keeps to
    /// itself.
    InvalidPath(String),
    /// Nothing has this path or ID.
    NotFound(String),
    /// The call failed. The message is for the plugin's author and holds no
    /// raw system details.
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Folder,
}

/// One file or folder in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub path: WorkspacePath,
    pub kind: EntryKind,
    pub object_id: Option<String>,
    pub object_type: Option<String>,
    pub revision: Option<String>,
}

/// An item of a workspace listing: an entry, or a name that can't cross
/// the boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Listed {
    Entry(Entry),
    Unsupported,
}

/// A file's bytes and their revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileRead {
    pub path: WorkspacePath,
    pub bytes: Vec<u8>,
    pub revision: String,
}

/// An object read by its stable ID.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectRead {
    pub id: String,
    pub object_type: String,
    pub title: String,
    pub path: WorkspacePath,
    pub body: String,
    /// The frontmatter's other properties, as a JSON object.
    pub properties: String,
    pub revision: String,
}

/// Reads the open workspace for plugins that hold the `workspace-read`
/// capability.
pub trait WorkspaceReads: Send + Sync {
    /// Every file and folder in the workspace.
    fn list_entries(&self) -> Result<Vec<Listed>, ServiceError>;

    /// The file at `path`.
    fn read_file(&self, path: &WorkspacePath) -> Result<FileRead, ServiceError>;

    /// The object with the stable ID `id`.
    fn read_object(&self, id: &str) -> Result<ObjectRead, ServiceError>;
}
