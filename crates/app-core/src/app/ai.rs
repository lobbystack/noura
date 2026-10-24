//! AI providers in Settings, your consent for each in the open workspace,
//! and the prompt that asks for it before a provider receives the
//! workspace's content. Every window shares the session's AI service.
//! Noura draws the prompt itself, so no plugin can draw one that grants
//! consent.

use std::sync::Arc;

use local_core::AiStreamer;

use super::{App, Change};
use crate::{AiConsentPrompt, AiProviderInput, AiProviderRow, AppError, ai::provider_kinds};

impl App {
    /// The AI providers set up on this device, with whether each has a key
    /// and your consent for the open workspace.
    pub fn ai_providers(&self) -> Vec<AiProviderRow> {
        self.session.ai.provider_rows()
    }

    /// The kinds of provider Noura connects to, each with the name
    /// Settings shows.
    pub fn ai_provider_kinds(&self) -> &'static [(&'static str, &'static str)] {
        provider_kinds()
    }

    /// Why Noura can't read its AI provider settings on this computer, if
    /// it can't.
    pub fn ai_settings_problem(&self) -> Option<String> {
        self.session.ai.problem()
    }

    /// Add a provider, or change one. Its key goes to this computer's
    /// credential store, and the settings stay in this device's app data.
    /// An endpoint must use HTTPS unless it points at this computer.
    /// Returns the provider's ID. Changing a provider's kind, endpoint or
    /// model lapses your consent for it.
    pub fn save_ai_provider(&mut self, input: AiProviderInput) -> Result<String, AppError> {
        let saved = self.session.ai.save_provider(input);
        self.ai_changed();
        saved
    }

    /// Remove a provider and its key. Its running requests stop.
    pub fn remove_ai_provider(&mut self, id: &str) -> Result<(), AppError> {
        let removed = self.session.ai.remove_provider(id);
        self.ai_changed();
        removed
    }

    /// Remove a provider's key from the credential store.
    pub fn remove_ai_provider_key(&mut self, id: &str) -> Result<(), AppError> {
        let removed = self.session.ai.remove_key(id);
        self.ai_changed();
        removed
    }

    /// Consent to the provider `id` receiving the open workspace's content.
    /// Noura keeps the consent on this device, never in the workspace.
    pub fn grant_ai_consent(&mut self, id: &str) -> Result<(), AppError> {
        let granted = self.session.ai.grant(id);
        self.ai_changed();
        granted
    }

    /// Withdraw your consent for the provider `id` in the open workspace.
    /// Its running requests stop, and its next request asks again.
    pub fn revoke_ai_consent(&mut self, id: &str) -> Result<(), AppError> {
        let revoked = self.session.ai.revoke(id);
        self.ai_changed();
        revoked
    }

    /// The question a chat waits on: may this provider receive the open
    /// workspace's content?
    pub fn ai_consent_prompt(&self) -> Option<AiConsentPrompt> {
        self.session.ai.prompt()
    }

    /// Let the provider the prompt names receive the open workspace's
    /// content, and send what waited.
    pub fn allow_ai_consent(&mut self) {
        if let Err(error) = self.session.ai.answer_prompt(true) {
            self.show_notice("Couldn't save your consent", error, None);
        }
        self.ai_changed();
    }

    /// Don't let the provider the prompt names receive the workspace's
    /// content. Nothing waiting for it is sent or written.
    pub fn deny_ai_consent(&mut self) {
        if let Err(error) = self.session.ai.answer_prompt(false) {
            self.show_notice("Couldn't answer", error, None);
        }
        self.ai_changed();
    }

    /// Talk to AI providers through `client` from the next request on.
    /// Noura uses the `genai` client until a client sets another, as tests
    /// do with a fake provider.
    pub fn set_ai_client(&mut self, client: Arc<dyn AiStreamer>) {
        self.session.ai.set_client(client);
    }

    fn ai_changed(&mut self) {
        self.ai_seen = self.session.ai.version();
        self.listeners.emit(Change::Ai);
    }

    /// Show what changed in the AI service since this window looked.
    pub(super) fn poll_ai(&mut self) -> bool {
        let version = self.session.ai.version();
        if version == self.ai_seen {
            return false;
        }
        self.ai_seen = version;
        self.listeners.emit(Change::Ai);
        true
    }
}
