//! The sync account screens every window shares.

use std::sync::Arc;

use super::App;
use crate::account::{AccountAction, AccountBackend, AccountScreen};
use crate::folder_sync::{FolderSyncAction, FolderSyncBackend, FolderSyncScreen};

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
}
