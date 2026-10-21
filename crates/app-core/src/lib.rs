//! Product behavior and view state shared by every Noura client.
//!
//! This crate depends on no UI crate, so a mobile client can reuse it later.
//! Clients draw [`App`]'s view state and send it what people do. The
//! workspace service opens workspaces and keeps the recent list, the
//! files service builds the file tree, and the editor sessions service
//! keeps each note open in a tab and saves its edits. The app keeps each workspace's tabs,
//! open folders and rail order, and the sidebar's layout, in this device's
//! app data. One command registry feeds the key handling, the command
//! palette, the native menus and the shortcuts list.

mod app;
mod commands;
mod error;
mod files;
mod ignored;
mod keys;
mod layout;
mod menus;
mod notes;
mod operations;
mod rail;
mod recents;
mod saves;
mod store;
mod system;
mod tabs;
mod view_state;
mod workspace;

pub use app::{
    App, Change, CommandPalette, DragOut, IgnoredFiles, Notice, NoticeKind, Question, Rename,
    Screen, UnsavedPrompt,
};
pub use commands::{
    Area, Command, CommandSource, ShortcutEntry, ShortcutGroup, command_id, default_shortcuts,
};
pub use error::AppError;
pub use files::{
    DropSpot, EntryKind, FileTree, OpenedFile, SPRING_OPEN_DELAY, TYPE_AHEAD_TIMEOUT, TreeAction,
    TreeKey, TreeRow,
};
pub use keys::{Key, KeyPress, Shortcut};
pub use layout::{FILES_SECTION, Layout, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH};
pub use menus::{Menu, MenuItem, MenuName, SystemItem};
pub use notes::{
    LineWidth, NoteSession, NoteSync, Property, PropertyValue, SAVE_AT_LEAST_EVERY, SAVE_PAUSE,
    SaveProblem,
};
pub use operations::Imported;
pub use rail::{FILES_PAGE, INBOX_PAGE, Page};
pub use recents::{RECENT_LIMIT, RecentWorkspace};
pub use saves::{NoPendingSaves, PendingSaves};
pub use system::{NoSystem, Platform, System, move_to_system_trash};
pub use tabs::{OpenAs, Tab, TabContent, TabId, TabPlace, ViewPlace};
pub use workspace::{Workspace, WorkspaceService};

/// Where Noura keeps per-device state, such as indexes and the recent
/// list: the operating system's local app data folder.
pub fn default_app_data_dir() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("org", "noura", "Noura")
        .map(|dirs| dirs.data_local_dir().to_owned())
}
