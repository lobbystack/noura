//! Web search in Settings: the Brave Search key, your web consent for the
//! open workspace, and the prompt that asks for it. Noura draws the
//! prompt itself, so no plugin can draw one that grants web consent.

use std::sync::Arc;

use local_core::WebClient;

use super::App;
use crate::{AppError, WebConsentPrompt, WebSearchRow};

impl App {
    /// Whether the Brave Search key is saved, and whether you allowed web
    /// search for the open workspace.
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

    /// Ask whether the AI may search the web and read pages in the open
    /// workspace, in Noura's own prompt.
    pub fn ask_web_consent(&mut self) -> Result<(), AppError> {
        let asked = self.session.ai.ask_web_consent();
        self.ai_changed();
        asked
    }

    /// The web consent question waiting for your answer, if one is.
    pub fn web_consent_prompt(&self) -> Option<WebConsentPrompt> {
        self.session.ai.web_prompt()
    }

    /// Let the AI search the web and read pages in the open workspace.
    /// Noura keeps the consent on this device, never in the workspace.
    pub fn allow_web_consent(&mut self) {
        if let Err(error) = self.session.ai.answer_web_prompt(true) {
            self.show_notice("Couldn't save your consent", error, None);
        }
        self.ai_changed();
    }

    /// Don't let the AI search the web in the open workspace. Noura stores
    /// nothing.
    pub fn deny_web_consent(&mut self) {
        if let Err(error) = self.session.ai.answer_web_prompt(false) {
            self.show_notice("Couldn't answer", error, None);
        }
        self.ai_changed();
    }

    /// Withdraw your web consent for the open workspace. The next web
    /// search or page read fails, and chats go on without them.
    pub fn revoke_web_consent(&mut self) -> Result<(), AppError> {
        let revoked = self.session.ai.revoke_web();
        self.ai_changed();
        revoked
    }

    /// Search the web and read pages through `client` from the next call
    /// on. Noura uses Brave Search until a client sets another, as tests
    /// do with a fake.
    pub fn set_web_client(&mut self, client: Arc<dyn WebClient>) {
        self.session.ai.set_web_client(client);
    }
}
