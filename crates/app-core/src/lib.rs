//! Product behavior and view state shared by every Noura client.
//!
//! This crate depends on no UI crate, so a mobile client can reuse it later.
//! Clients draw [`App`]'s view state and send it what people do. The
//! workspace service opens workspaces and keeps the recent list, and the
//! files service builds the file tree.

mod app;
mod error;
mod files;
mod objects;
mod problems;
mod recents;
mod watch;
mod workspace;

pub use app::{App, Change, Notice, Origin, Screen};
pub use error::AppError;
pub use files::{
    EntryKind, FileTree, OpenedFile, TYPE_AHEAD_TIMEOUT, TreeAction, TreeKey, TreeRow,
};
#[cfg(feature = "test-hooks")]
pub use local_core::WatchFault;
pub use objects::{ObjectRef, SearchHit};
pub use problems::Problem;
pub use recents::{RECENT_LIMIT, RecentWorkspace};
pub use watch::Rescans;
pub use workspace::{Workspace, WorkspaceService};

/// Where Noura keeps per-device state, such as indexes and the recent
/// list: the operating system's local app data folder.
pub fn default_app_data_dir() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("org", "noura", "Noura")
        .map(|dirs| dirs.data_local_dir().to_owned())
}
