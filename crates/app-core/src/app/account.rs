//! The sync account screens every window shares.

use std::sync::Arc;

use super::App;
use crate::account::{AccountAction, AccountBackend, AccountScreen};

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
}
