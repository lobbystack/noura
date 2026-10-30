//! The sync account screens every window shares.

use std::sync::Arc;

use super::App;
use crate::account::{AccountAction, AccountBackend, AccountScreen};
use crate::folder_sync::{FolderSyncAction, FolderSyncBackend, FolderSyncScreen};
use crate::members::{MembersAction, MembersBackend, MembersScreen};
use crate::shares::{ShareAction, ShareBackend, ShareScreen};

impl App {
    /// What the account screens show.
    pub fn account(&self) -> AccountScreen {
        self.session.account().screen()
    }

    /// Carries out what you did on the account screens.
    pub fn account_action(&self, action: AccountAction) {
        self.session.account().act(action);
    }

    /// Runs the account screens with `backend`, such as the desktop's
    /// [`LiveAccount`](crate::account::live::LiveAccount). Every window
    /// hears when the screens change.
    pub fn set_account_backend(&self, backend: Arc<dyn AccountBackend>) {
        self.session.set_account(backend);
    }

    /// What the Workspace sync section shows.
    pub fn folder_sync(&self) -> FolderSyncScreen {
        self.session.folder_sync().screen()
    }

    /// Carries out what you did in the Workspace sync section.
    pub fn folder_sync_action(&self, action: FolderSyncAction) {
        self.session.folder_sync().act(action);
    }

    /// Runs the open workspace's sync with `backend`, such as the desktop's
    /// [`LiveAccount`](crate::account::live::LiveAccount), which hears of
    /// each workspace the windows open.
    pub fn set_folder_sync_backend(&self, backend: Arc<dyn FolderSyncBackend>) {
        self.session.set_folder_sync(backend);
    }

    /// What the Members section shows.
    pub fn members(&self) -> MembersScreen {
        self.session.members().screen()
    }

    /// Carries out what you did in the Members section.
    pub fn members_action(&self, action: MembersAction) {
        self.session.members().act(action);
    }

    /// Runs the Members section with `backend`, such as the desktop's
    /// [`LiveAccount`](crate::account::live::LiveAccount).
    pub fn set_members_backend(&self, backend: Arc<dyn MembersBackend>) {
        self.session.set_members(backend);
    }

    /// What the share dialog shows.
    pub fn share_screen(&self) -> ShareScreen {
        self.session.shares().screen()
    }

    /// Carries out what you did in the share dialog.
    pub fn share_action(&self, action: ShareAction) {
        self.session.shares().act(action);
    }

    /// Runs the share dialog with `backend`, such as the desktop's
    /// [`LiveAccount`](crate::account::live::LiveAccount).
    pub fn set_share_backend(&self, backend: Arc<dyn ShareBackend>) {
        self.session.set_shares(backend);
    }

    /// Share link on the open note: opens the share dialog for it.
    pub fn share_open_note(&mut self) {
        let Some(note) = self.active_note_for_sharing() else {
            return;
        };
        self.session.shares().act(note);
    }

    /// The Open action of the note in the active tab, when it's a Markdown
    /// note Noura could read.
    fn active_note_for_sharing(&self) -> Option<ShareAction> {
        let tab = self.active_tab()?.id();
        let note = self.note(tab)?;
        if !note.relative_path().to_ascii_lowercase().ends_with(".md") {
            return None;
        }
        Some(ShareAction::Open {
            path: note.relative_path().to_owned(),
            name: note.name(),
            text: note.text().to_owned(),
        })
    }
}
