//! The share dialog's state (docs/workspace-format/sync-v2.md §24): Share
//! link on a note creates a read-only link to a snapshot of it, with an
//! expiry from 1 to 30 days, and copies it. The dialog lists the note's
//! links that this device created, which it can update and revoke, and
//! every active link of the workspace, which an owner or admin device can
//! revoke, flagging those whose creating device left.
//!
//! [`ShareScreen`] is plain view state that `app-ui` draws; what you do
//! there arrives as a [`ShareAction`] at the [`ShareBackend`] the app runs
//! with. The desktop's [`LiveAccount`](crate::account::live::LiveAccount)
//! carries the actions out with its sync account; without one,
//! [`NoShares`] says that sharing needs an account.

use std::sync::{Arc, Mutex, PoisonError};

pub use sync_protocol::types::Digest;
use zeroize::Zeroizing;

pub use crate::sync::shares::DEFAULT_SHARE_DAYS;

/// What the dialog says on a computer without a sync account.
pub const NEEDS_ACCOUNT: &str = "Sharing needs an account. Sign in under Settings > Sync account, then turn on sync for this workspace.";

/// What the dialog says while Noura keeps trying a server it can't reach.
pub const UNREACHABLE: &str =
    "Noura can't reach the sync server, so sharing waits. It keeps trying.";

/// A link to copy, kept out of logs and wiped from memory when it goes.
#[derive(Clone, PartialEq, Eq)]
pub struct LinkText(pub Zeroizing<String>);

impl std::fmt::Debug for LinkText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LinkText(..)")
    }
}

/// One of the note's links that this device created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NoteLinkEntry {
    /// The token's hash, which the actions name.
    pub token_hash: Digest,
    /// When it expires, as local date and time.
    pub expires: String,
    /// The snapshot's version.
    pub revision: u64,
}

/// One active link of the workspace, as owner and admin devices see it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceLinkEntry {
    /// The token's hash, which revoking names.
    pub token_hash: Digest,
    /// What the list shows: the token hash's start, the folder and device
    /// it came from, and the expiry.
    pub label: String,
    /// When it expires, in Unix seconds.
    pub expires_at: u64,
    /// Whether this device created it.
    pub own: bool,
    /// Whether the device that created it left the workspace, so a
    /// malicious server working with it could still replace its snapshot.
    pub creator_removed: bool,
}

/// What the share dialog shows.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShareScreen {
    /// Whether the dialog is open.
    pub open: bool,
    /// The note's name.
    pub note: String,
    /// Why this note can't be shared from this computer, such as when it
    /// has no sync account. The dialog then shows only this.
    pub unavailable: Option<String>,
    /// The expiry a new link takes, in days.
    pub days: u8,
    /// The note's links that this device created.
    pub links: Vec<NoteLinkEntry>,
    /// Every active link of the workspace.
    pub workspace_links: Vec<WorkspaceLinkEntry>,
    /// A link to put on the clipboard, which the window copies once.
    pub copy: Option<LinkText>,
    /// What the last action did, such as "Link copied".
    pub notice: Option<String>,
    /// What went wrong last.
    pub problem: Option<String>,
    /// Whether an action is running.
    pub busy: bool,
}

/// What you do in the share dialog.
#[derive(Clone, PartialEq, Eq)]
pub enum ShareAction {
    /// Share link on a note: opens the dialog for it.
    Open {
        /// The note's path in the workspace.
        path: String,
        /// The note's name, which the snapshot takes as its title.
        name: String,
        /// The note's text, from which the snapshot leaves the properties
        /// out.
        text: String,
    },
    /// Close the dialog.
    Close,
    /// Pick the expiry of the next link, 1 to 30 days.
    SetDays(u8),
    /// Create a link and copy it.
    Create,
    /// Copy one of this device's links again.
    Copy(Digest),
    /// The window copied the link.
    Copied,
    /// Replace a link's snapshot with the note as it is now.
    Update(Digest),
    /// Revoke a link.
    Revoke(Digest),
}

impl std::fmt::Debug for ShareAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShareAction::Open { path, .. } => f
                .debug_struct("Open")
                .field("path", path)
                .finish_non_exhaustive(),
            ShareAction::Close => f.write_str("Close"),
            ShareAction::SetDays(days) => f.debug_tuple("SetDays").field(days).finish(),
            ShareAction::Create => f.write_str("Create"),
            ShareAction::Copy(hash) => f.debug_tuple("Copy").field(hash).finish(),
            ShareAction::Copied => f.write_str("Copied"),
            ShareAction::Update(hash) => f.debug_tuple("Update").field(hash).finish(),
            ShareAction::Revoke(hash) => f.debug_tuple("Revoke").field(hash).finish(),
        }
    }
}

/// Carries out what you do in the share dialog and reports what it shows.
pub trait ShareBackend: Send + Sync {
    /// What the dialog shows now.
    fn screen(&self) -> ShareScreen;
    /// Starts `action`; the dialog changes once it runs.
    fn act(&self, action: ShareAction);
    /// Calls `wake` whenever the dialog changes.
    fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>);
}

/// The backend of an app without a sync account: the dialog opens and says
/// that sharing needs an account.
#[derive(Default)]
pub struct NoShares {
    screen: Mutex<ShareScreen>,
}

impl ShareBackend for NoShares {
    fn screen(&self) -> ShareScreen {
        self.screen
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    fn act(&self, action: ShareAction) {
        let mut screen = self.screen.lock().unwrap_or_else(PoisonError::into_inner);
        match action {
            ShareAction::Open { name, .. } => {
                *screen = ShareScreen {
                    open: true,
                    note: name,
                    unavailable: Some(NEEDS_ACCOUNT.into()),
                    days: DEFAULT_SHARE_DAYS,
                    ..ShareScreen::default()
                };
            }
            ShareAction::Close => *screen = ShareScreen::default(),
            _ => {}
        }
    }

    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}
}
