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

pub use crate::sync::workspace::rebuild::HeldChoice;
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
    /// This computer holds a device of your account, but the sync server
    /// can't be reached yet, and Noura keeps trying.
    Unreachable,
    /// The folder is a copy of a synced folder, or its journal names
    /// another replica: it syncs once you connect it (§15).
    NotConnected,
    /// The server lost the account this folder synced under, and the
    /// restore notice offers the way back (§14).
    AccountLost,
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

/// A local version held after the folder synced without its journal,
/// waiting for you to choose (§15).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldEntry {
    /// The object.
    pub rid: Id,
    /// Where the file sits.
    pub path: String,
}

/// A folder whose files wait until you confirm who may see them (§9).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HeldRootEntry {
    /// The folder.
    pub folder: String,
    /// The files beneath it.
    pub files: Vec<String>,
    /// How many people would see them in the workspace.
    pub people: usize,
    /// The domain whose `.noura-domain` file this computer can write again
    /// to end the wait.
    pub restore: Option<Id>,
}

/// A version an earlier journal kept for review (§15).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArchivedEntry {
    /// Its place in the list, which restoring names.
    pub index: usize,
    /// The path the version had.
    pub path: String,
}

/// A note whose edits stopped merging on this computer, waiting for you
/// (§22).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoppedNoteEntry {
    /// The object.
    pub rid: Id,
    /// The note's path.
    pub path: String,
    /// Whether a rename took the note outside its folder, rather than a
    /// collaborator's edit stopping it from merging.
    pub outside: bool,
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
    /// The notes whose edits stopped merging.
    pub stopped_notes: Vec<StoppedNoteEntry>,
    /// Collaborators' edits that didn't reach a note, one line each,
    /// naming the device that signed each (§22).
    pub text_reviews: Vec<String>,
    /// The local versions held for review.
    pub held: Vec<HeldEntry>,
    /// The folders whose files wait for you.
    pub held_roots: Vec<HeldRootEntry>,
    /// The versions an earlier journal kept.
    pub archived: Vec<ArchivedEntry>,
    /// The versions from other devices that didn't open, one line each,
    /// naming the device that signed each (§13).
    pub unreadable: Vec<String>,
    /// Whether sync holds your changes after a device left your account,
    /// until the workspace records it and changes its key (§9).
    pub removal_hold: bool,
    /// Whether a device left your account but the workspace is too full to
    /// record it, so that device keeps reading new changes (§9).
    pub removal_over_limit: bool,
    /// What ended sync for this computer, when a change took its access
    /// to the workspace (§15).
    pub ended: Option<String>,
    /// The files sync leaves out, one line each.
    pub skipped: Vec<String>,
    /// What waits for you in the Members section, which the window says
    /// once wherever you are (§9).
    pub attention: Option<String>,
    /// The workspace's storage against its quota, one line (§20).
    pub usage: Option<String>,
    /// Whether usage stands above the quota, so changes wait.
    pub over_quota: bool,
    /// Changes the server refused for lack of space, which stay on this
    /// computer until space frees up (§20).
    pub waiting_for_space: usize,
    /// Files deleted on another device while this computer was away, which
    /// wait for you (§18).
    pub deleted_away: Vec<String>,
    /// What a server restore did here, when it needs your attention.
    pub restore: Option<RestoreNotice>,
    /// Whether this device's account owns the workspace, so you can set
    /// it up again even when this device found nothing lost (§14).
    pub can_set_up_again: bool,
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
            stopped_notes: Vec::new(),
            text_reviews: Vec::new(),
            pending: status.pending,
            conflicts: Vec::new(),
            held: Vec::new(),
            held_roots: Vec::new(),
            archived: Vec::new(),
            unreadable: Vec::new(),
            removal_hold: false,
            removal_over_limit: false,
            ended: status.ended.as_ref().map(ended_line),
            skipped: status.skipped.iter().map(skipped_line).collect(),
            attention: None,
            usage: status.usage.map(usage_line),
            over_quota: status
                .usage
                .is_some_and(|u| u.quota.is_some_and(|quota| u.used >= quota)),
            waiting_for_space: status.waiting_for_space,
            deleted_away: Vec::new(),
            restore: restore_notice(status),
            can_set_up_again: status.owner,
            last_error: None,
            last_success: status.last_success.map(local_time),
            busy: false,
        }
    }
}

/// What the app says about a server restore, and the way back it offers.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RestoreNotice {
    /// The banner's sentence.
    pub line: String,
    /// Whether Reset devices is the way back.
    pub reset_devices: bool,
    /// Whether Set up sync again is the way back.
    pub set_up_again: bool,
    /// The objects this computer wrote again.
    pub written_again: Vec<String>,
    /// Conflict branches and newer files that wait for you.
    pub to_review: Vec<String>,
}

/// The banner for a restore, a rollback or a stop (§14), if any.
pub fn restore_notice(status: &Status) -> Option<RestoreNotice> {
    if let Some(lost) = &status.restore_lost {
        return Some(lost_notice(lost, status.owner));
    }
    if status.rollback.is_some() {
        return Some(RestoreNotice {
            line: "The server went back to an older state and reported no restore. Sync stopped here until its operator marks the restore.".to_owned(),
            ..RestoreNotice::default()
        });
    }
    if status.chain_gap {
        return Some(RestoreNotice {
            line:
                "The server skipped part of a device's changes. Sync waits here until they arrive."
                    .to_owned(),
            ..RestoreNotice::default()
        });
    }
    // Rewrites signed after you dismissed the alert bring the notice back.
    let shows = status.restored.is_some()
        || !status.written_again.is_empty()
        || !status.to_review.is_empty();
    shows.then(|| RestoreNotice {
        line: "The server reports a restore from a backup. Noura checked what it holds and kept syncing."
            .to_owned(),
        written_again: status.written_again.clone(),
        to_review: status.to_review.clone(),
        ..RestoreNotice::default()
    })
}

/// The banner for what a restore lost, with the way back it offers.
pub fn lost_notice(lost: &crate::sync::workspace::restore::Lost, owner: bool) -> RestoreNotice {
    use crate::sync::workspace::restore::Lost;
    let short = |id: &Id| id.to_hex().chars().take(8).collect::<String>();
    let (line, reset_devices, set_up_again) = match lost {
        Lost::OwnAccount => (
            "The server's operator restored a backup that lacks your latest devices. Sync stopped here. Reset your devices to go on.".to_owned(),
            true,
            false,
        ),
        Lost::MemberAccount(account) => (
            format!(
                "The server's operator restored a backup that lacks devices of account {}. Sync stopped here until that person resets their devices and an owner or admin accepts the reset.",
                short(account)
            ),
            false,
            false,
        ),
        Lost::Policy | Lost::Epoch | Lost::Envelope | Lost::Checkpoints | Lost::FinalHead(_) => (
            "The server's operator restored a backup that lacks part of this workspace's history. Sync stopped here. An owner can set the workspace up again.".to_owned(),
            false,
            owner,
        ),
        Lost::Server => (
            "This server lost your account. Sync stopped here. You can turn sync on for this folder as a new workspace.".to_owned(),
            false,
            true,
        ),
        Lost::Closed => (
            "The owner closed this workspace. Your files stay on this computer.".to_owned(),
            false,
            owner,
        ),
    };
    RestoreNotice {
        line,
        reset_devices,
        set_up_again,
        ..RestoreNotice::default()
    }
}

/// A number of bytes for people to read.
fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    if value < 1024.0 {
        return format!("{bytes} bytes");
    }
    let mut unit = 0;
    value /= 1024.0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}

/// The workspace's storage, against its quota when it has one.
fn usage_line(usage: crate::sync::workspace::Usage) -> String {
    match usage.quota {
        Some(quota) => format!("{} of {} used.", size(usage.used), size(quota)),
        None => format!("{} used.", size(usage.used)),
    }
}

/// What ended sync for this computer, naming who made the change.
pub fn ended_line(ended: &crate::sync::workspace::Ended) -> String {
    let short = |id: &sync_protocol::types::Id| id.to_hex().chars().take(8).collect::<String>();
    let who = match &ended.by_account {
        Some(account) => format!("Account {}", short(account)),
        None => "Another account".to_owned(),
    };
    let what = if ended.account_removed {
        "removed your account from this workspace"
    } else if ended.domain_deleted {
        "deleted this part of the workspace"
    } else {
        "took this computer's access to the workspace"
    };
    format!(
        "{who} {what}, from device {}. Sync stopped. Your files stay on this computer as they are.",
        short(&ended.by_device)
    )
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
        SkipWhy::TooLarge => "it's above the limit of 2 GiB for one file",
        SkipWhy::HeldRoot => "it waits until you confirm who may see this folder",
        SkipWhy::CrossesRoot => "it moved into or out of a restricted folder",
        SkipWhy::CantWrite => "your role can't edit here, so your change stays on this computer",
    };
    let path = if skipped.path.is_empty() {
        "The workspace folder"
    } else {
        skipped.path.as_str()
    };
    format!("{path}: {why}")
}

#[cfg(test)]
mod skipped_tests {
    use super::*;
    use crate::sync::workspace::Skipped;

    #[test]
    fn a_file_above_the_limit_names_the_limit() {
        let line = skipped_line(&Skipped {
            path: "Film.mov".into(),
            why: SkipWhy::TooLarge,
        });
        assert_eq!(line, "Film.mov: it's above the limit of 2 GiB for one file");
    }
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
    /// Keep a stopped note's last good state, which ends its live editing
    /// on every computer.
    KeepLastGood(Id),
    /// Decide about a held local version.
    KeepHeld {
        /// The object.
        rid: Id,
        /// What to keep.
        choice: HeldChoice,
    },
    /// Let the files beneath a held folder upload to the workspace.
    ConfirmFolder(String),
    /// Write a domain's `.noura-domain` file again from synced state.
    RestoreMarker(Id),
    /// Save an earlier journal's version as a new file.
    RestoreArchived(usize),
    /// Throw an earlier journal's version away.
    DiscardArchived(usize),
    /// Delete a file another device deleted while this one was away.
    ConfirmDeletedAway(String),
    /// Keep such a file: it uploads as a new file.
    KeepDeletedAway(String),
    /// Connect a copied folder, which then syncs as after a lost journal.
    Connect,
    /// You renamed the workspace: a synced one takes the name in its
    /// profile, when your role lets you (§25).
    Rename(String),
    /// Dismiss the restore notice.
    DismissRestore,
    /// Start a new account chain after a restore lost a link of it (§7).
    ResetDevices,
    /// Close the workspace and start it over from this folder (§14).
    SetUpAgain,
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
    /// Where the windows reach text-mode notes, when the workspace can
    /// sync.
    fn text_bridge(&self) -> Option<Arc<crate::text_bridge::TextBridge>> {
        None
    }
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
    fn rewrites_after_a_dismissed_alert_bring_the_notice_back() {
        let status = Status {
            written_again: vec!["Late.md".into()],
            ..Status::default()
        };
        let notice = restore_notice(&status).expect("a notice");
        assert_eq!(notice.written_again, ["Late.md"]);
        assert_eq!(restore_notice(&Status::default()), None);
    }

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
