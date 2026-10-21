//! Product behavior and view state shared by every Noura client.
//!
//! This crate depends on no UI crate, so a mobile client can reuse it later.
//! Clients draw [`App`]'s view state and send it what people do. Each `App`
//! is one window, and the windows of one launch share the open workspace
//! and the update service. The workspace service opens workspaces and
//! keeps the recent list, the files service builds the file tree, the
//! editor sessions service keeps each note open in a tab and saves its
//! edits, and the update service keeps the app current. The app keeps each
//! workspace's windows with their tabs and open folders, its rail order,
//! the sidebar's layout, and the appearance, text size, line width and
//! zoom, in this device's app data. One command registry feeds the key
//! handling, the command palette, the native menus and the shortcuts
//! list. Settings lists its sections from one list that later areas
//! extend. The plugins service installs plugins from folders, keeps this
//! device's approvals, turns plugins on and off per workspace and runs
//! them through `plugin-host`.

mod app;
mod commands;
mod diagnostics;
mod error;
mod files;
mod ignored;
mod inbox;
mod keys;
mod layout;
mod login;
mod menus;
mod notes;
mod objects;
mod operations;
mod plugin_objects;
mod plugins;
mod preferences;
mod problems;
mod quick_open;
mod rail;
mod recents;
mod saves;
mod search;
mod session;
mod settings;
mod store;
mod system;
mod tabs;
mod updates;
mod view_state;
mod watch;
mod workspace;
mod writes;

pub use app::{
    App, Change, CommandPalette, DragOut, IgnoredFiles, Notice, NoticeKind, Origin, Palette,
    PaletteMode, Question, QuickOpen, Rename, Screen, Search, UnsavedPrompt,
};
pub use commands::{
    Area, Command, CommandSource, ShortcutEntry, ShortcutGroup, command_id, default_shortcuts,
};
pub use diagnostics::{LOG_FILE, log_folder};
pub use error::AppError;
pub use files::{
    DropSpot, EntryKind, FileTree, OpenedFile, SPRING_OPEN_DELAY, TYPE_AHEAD_TIMEOUT, TreeAction,
    TreeKey, TreeRow,
};
pub use inbox::{
    Attention, FileLink, Inbox, InboxSection, InboxSectionProvider, LinkAction, RECENT_NOTES,
    RecentNote,
};
pub use keys::{Key, KeyPress, Shortcut};
pub use layout::{FILES_SECTION, Layout, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH};
#[cfg(feature = "test-hooks")]
pub use local_core::WatchFault;
pub use login::{LoginItem, NoLoginItem, OpenAtLogin};
pub use menus::{Menu, MenuItem, MenuName, SystemItem};
pub use notes::{
    ConflictChoice, NoteConflict, NoteSession, NoteSync, Property, PropertyValue,
    SAVE_AT_LEAST_EVERY, SAVE_PAUSE, SaveProblem,
};
pub use objects::ObjectRef;
pub use operations::Imported;
pub use plugin_host::{View, ViewKind, ViewNode};
pub use plugins::{CapabilityLine, PluginInstall, PluginRow, PluginView};
pub use preferences::{ACTUAL_SIZE, Appearance, LineWidth, Preferences, TextSize, ZOOM_LEVELS};
pub use problems::Problem;
pub use quick_open::{QUICK_OPEN_LIMIT, QuickOpenMatch};
pub use rail::{FILES_PAGE, INBOX_PAGE, Page};
pub use recents::{RECENT_LIMIT, RecentWorkspace};
pub use saves::{NoPendingSaves, PendingSaves};
pub use search::{SEARCH_LIMIT, SearchHit, SearchQuery, SearchState};
pub use settings::{
    ABOUT_LINKS, AboutLink, LICENSE, Settings, SettingsGroup, SettingsSection,
    THIRD_PARTY_LICENSES, VERSION, settings_section,
};
pub use system::{NoSystem, Platform, System, move_to_system_trash};
pub use tabs::{OpenAs, Tab, TabContent, TabId, TabPlace, ViewPlace};
pub use updates::{
    FIRST_UPDATE_CHECK, PendingUpdate, UPDATE_CHECK_INTERVAL, UpdateError, UpdateErrorKind,
    UpdateSource, UpdateStatus, Updates,
};
pub use watch::Rescans;
pub use workspace::{Workspace, WorkspaceService};
pub use writes::{PendingWork, PendingWrite, PendingWrites, WriteFailure};

/// Where Noura keeps per-device state, such as indexes and the recent
/// list: the operating system's local app data folder.
pub fn default_app_data_dir() -> Option<std::path::PathBuf> {
    directories::ProjectDirs::from("org", "noura", "Noura")
        .map(|dirs| dirs.data_local_dir().to_owned())
}
