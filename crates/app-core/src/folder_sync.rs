//! The Workspace sync section's state (docs/workspace-format/sync-v2.md
//! §15, §16): whether the open workspace syncs, whether sync runs or you
//! paused it, how many changes wait to upload, the conflicts that wait for
//! you, the last error and the last successful sync.
//!
//! [`FolderSyncScreen`] is plain view state that `app-ui` draws; what you do
//! there arrives as a [`FolderSyncAction`] at the [`FolderSyncBackend`] the
//! app runs with. The desktop's [`LiveAccount`](crate::account::live) runs
//! the open workspace's sync rounds next to the account; tests supply a
//! fake.

use std::sync::{Arc, Mutex, PoisonError};

pub use local_core::WorkspaceEngine;
pub use sync_protocol::types::Id;

pub use crate::sync::workspace::state::OpKey;
use crate::sync::workspace::{SkipWhy, Status};

/// Where the open workspace's sync stands.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FolderSyncPhase {
    /// No workspace is open, or this computer has no sync account yet.
    #[default]
    Unavailable,
    /// The workspace doesn't sync; its files stay on this computer.
    Off,
    /// Sync runs.
    On,
    /// You paused sync.
    Paused,
}

/// One version of a conflicted file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchEntry {
    /// What the button that keeps it says.
    pub label: String,
    /// Its position, which choosing names.
    pub key: OpKey,
}

/// A file whose devices disagree, waiting for you to choose (§16).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConflictEntry {
    /// The object.
    pub rid: Id,
    /// The path this device shows.
    pub path: String,
    /// Its versions.
    pub branches: Vec<BranchEntry>,
}

/// What the Workspace sync section shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FolderSyncScreen {
    /// Where sync stands.
    pub phase: FolderSyncPhase,
    /// The changes waiting to upload.
    pub pending: usize,
    /// The conflicts waiting for you.
    pub conflicts: Vec<ConflictEntry>,
    /// The files sync leaves out, one line each.
    pub skipped: Vec<String>,
    /// What went wrong last, for you to read.
    pub last_error: Option<String>,
    /// When the last round finished without an error, as local time.
    pub last_success: Option<String>,
    /// Whether an action is running.
    pub busy: bool,
}

impl FolderSyncScreen {
    /// The screen of a folder whose sync `status` describes.
    pub fn of(status: &Status, paused: bool) -> Self {
        Self {
            phase: if paused {
                FolderSyncPhase::Paused
            } else {
                FolderSyncPhase::On
            },
            pending: status.pending,
            conflicts: Vec::new(),
            skipped: status.skipped.iter().map(skipped_line).collect(),
            last_error: None,
            last_success: status.last_success.map(local_time),
            busy: false,
        }
    }
}

/// One line of the list of files sync leaves out.
pub fn skipped_line(skipped: &crate::sync::workspace::Skipped) -> String {
    let why = match skipped.why {
        SkipWhy::Symlink => "a link to another place",
        SkipWhy::UnsupportedName => "a name here has characters sync can't carry",
        SkipWhy::PathRules => "its name doesn't work on every computer",
        SkipWhy::Unreadable => "Noura can't read it",
        SkipWhy::NotDownloaded => "it isn't downloaded from the cloud yet",
        SkipWhy::IdentityConflict => "another file has the same ID",
        SkipWhy::TooLarge => "files above 256 KB sync once attachments arrive",
    };
    let path = if skipped.path.is_empty() {
        "The workspace folder"
    } else {
        skipped.path.as_str()
    };
    format!("{path}: {why}")
}

/// Unix seconds as local date and time, such as `2026-10-24 14:05`.
pub fn local_time(seconds: u64) -> String {
    let Ok(timestamp) = jiff::Timestamp::from_second(seconds as i64) else {
        return String::new();
    };
    let zoned = timestamp.to_zoned(jiff::tz::TimeZone::system());
    zoned.strftime("%Y-%m-%d %H:%M").to_string()
}

/// What you do in the Workspace sync section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FolderSyncAction {
    /// Sync the open workspace, as a new remote workspace or again.
    TurnOn,
    /// Stop syncing it; the files stay.
    TurnOff,
    /// Hold changes on this computer.
    Pause,
    /// Upload what queued and sync again.
    Resume,
    /// Keep one version of a conflicted file.
    Choose {
        /// The object.
        rid: Id,
        /// The version.
        key: OpKey,
    },
}

/// Carries out what you do in the Workspace sync section and reports where
/// sync stands.
pub trait FolderSyncBackend: Send + Sync {
    /// What the section shows now.
    fn screen(&self) -> FolderSyncScreen;
    /// Starts `action`; the screen changes once it runs.
    fn act(&self, action: FolderSyncAction);
    /// Calls `wake` whenever the screen changes.
    fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>);
    /// The workspace the windows show, or `None` once none is open.
    fn attach(&self, engine: Option<Arc<WorkspaceEngine>>);
}

/// The backend of an app that runs without sync: the workspace never
/// syncs.
#[derive(Default)]
pub struct NoFolderSync {
    screen: Mutex<FolderSyncScreen>,
}

impl FolderSyncBackend for NoFolderSync {
    fn screen(&self) -> FolderSyncScreen {
        self.screen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn act(&self, _action: FolderSyncAction) {}

    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}

    fn attach(&self, _engine: Option<Arc<WorkspaceEngine>>) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skipped_files_read_as_one_line_each() {
        let line = skipped_line(&crate::sync::workspace::Skipped {
            path: "Notes/CON".into(),
            why: SkipWhy::PathRules,
        });
        assert_eq!(line, "Notes/CON: its name doesn't work on every computer");
        assert!(local_time(0).starts_with("19"));
    }
}
