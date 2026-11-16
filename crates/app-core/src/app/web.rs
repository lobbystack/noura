//! Web search in Settings: the Brave Search key. Adding it is your say-so,
//! so Noura asks nothing per workspace.

use std::sync::Arc;

use local_core::WebClient;

use super::App;
use crate::{AppError, WebSearchRow};

impl App {
    /// Whether the Brave Search key is saved.
    pub fn web_search(&self) -> WebSearchRow {
        self.session.ai.web_row()
    }

    /// Why Noura can't read its web search settings on this computer, if
    /// it can't.
    pub fn web_search_problem(&self) -> Option<String> {
        self.session.ai.web_problem()
    }

    /// Keep `key` as the Brave Search key. It goes to this computer's
    /// credential store, and the settings in this device's app data keep
    /// only a reference to it.
    pub fn save_web_search_key(&mut self, key: &str) -> Result<(), AppError> {
        let saved = self.session.ai.set_web_key(key);
        self.ai_changed();
        saved
    }

    /// Remove the Brave Search key from the credential store.
    pub fn remove_web_search_key(&mut self) -> Result<(), AppError> {
        let removed = self.session.ai.remove_web_key();
        self.ai_changed();
        removed
    }

    /// Search the web and read pages through `client` from the next call
    /// on. Noura uses Brave Search until a client sets another, as tests
    /// do with a fake.
    pub fn set_web_client(&mut self, client: Arc<dyn WebClient>) {
        self.session.ai.set_web_client(client);
    }
}
