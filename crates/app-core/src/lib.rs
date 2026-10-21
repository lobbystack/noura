//! Product behavior and view state shared by every Noura client.
//!
//! This crate depends on no UI crate, so a mobile client can reuse it later.
//! Clients draw [`App`]'s view state and send it what people do. The
//! workspace service opens workspaces and keeps the recent list, the
//! files service builds the file tree, and the update service keeps the
//! app current.

mod app;
mod error;
mod files;
mod recents;
mod updates;
mod workspace;
mod writes;

pub use app::{App, Change, Notice, Screen};
pub use error::AppError;
pub use files::{
    EntryKind, FileTree, OpenedFile, TYPE_AHEAD_TIMEOUT, TreeAction, TreeKey, TreeRow,
};
pub use recents::{RECENT_LIMIT, RecentWorkspace};
pub use updates::{
    FIRST_UPDATE_CHECK, PendingUpdate, UPDATE_CHECK_INTERVAL, UpdateError, UpdateErrorKind,
    UpdateSource, UpdateStatus, Updates,
};
pub use workspace::{Workspace, WorkspaceService};
pub use writes::{PendingWrite, PendingWrites, WriteFailure};

/// Where Noura keeps per-device state, such as indexes and the recent
/// list: the operating system's local app data folder.
pub fn default_app_data_dir() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("org", "noura", "Noura")
        .map(|dirs| dirs.data_local_dir().to_owned())
}
