//! Product behavior and view state shared by every Noura client.
//!
//! This crate depends on no UI crate, so a mobile client can reuse it later.
//! Clients draw [`App`]'s view state and send it what people do. The
//! workspace service opens workspaces and keeps the recent list, and the
//! files service builds the file tree.

mod app;
mod error;
mod files;
mod recents;
mod workspace;

pub use app::{App, Change, Notice, Screen};
pub use error::AppError;
pub use files::{
    EntryKind, FileTree, OpenedFile, TYPE_AHEAD_TIMEOUT, TreeAction, TreeKey, TreeRow,
};
pub use recents::{RECENT_LIMIT, RecentWorkspace};
pub use workspace::{Workspace, WorkspaceService};

/// Where Noura keeps per-device state, such as indexes and the recent
/// list: the operating system's local app data folder.
pub fn default_app_data_dir() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("org", "noura", "Noura")
        .map(|dirs| dirs.data_local_dir().to_owned())
}
