//! AI providers in Settings and the prompt that asks before a tool call
//! runs. Every window shares the session's AI service. Noura draws the
//! prompt itself, so no plugin can draw one that allows a call.

use std::sync::Arc;

use local_core::AiStreamer;

use super::{App, Change};
use crate::{
    AiProviderChoice, AiProviderInput, AiProviderRow, AiToolPrompt, AppError,
    ai::{provider_choices, provider_kinds},
};

impl App {
    /// The AI providers set up on this device, with whether each has a
    /// key.
    pub fn ai_providers(&self) -> Vec<AiProviderRow> {
        self.session.ai.provider_rows()
    }

    /// The kinds of provider Noura connects to, each with the name
    /// Settings shows.
    pub fn ai_provider_kinds(&self) -> &'static [(&'static str, &'static str)] {
        provider_kinds()
    }

    /// The providers you connect with only a key, each with whether it is
    /// connected.
    pub fn ai_provider_choices(&self) -> Vec<AiProviderChoice> {
        provider_choices(&self.ai_providers())
    }

    /// Connect the provider of `kind`, one of [`Self::ai_provider_choices`],
    /// with `key`: the key goes to this computer's credential store, and a
    /// provider you hadn't set up gets the kind's default model and turns
    /// on. A provider you had keeps its settings. Returns its ID.
    pub fn connect_ai_provider(&mut self, kind: &str, key: &str) -> Result<String, AppError> {
        let Some(choice) = self
            .ai_provider_choices()
            .into_iter()
            .find(|choice| choice.kind == kind)
        else {
            return Err(AppError::new(
                "provider_kind_invalid",
                "Noura doesn't connect to that provider with a key.",
            ));
        };
        if key.trim().is_empty() {
            return Err(AppError::new(
                "provider_key_missing",
                "Paste the provider's key.",
            ));
        }
        let rows = self.ai_providers();
        let input = match choice
            .provider
            .as_ref()
            .and_then(|id| rows.iter().find(|row| row.id == *id))
        {
            Some(row) => AiProviderInput {
                id: Some(row.id.clone()),
                name: row.name.clone(),
                kind: row.kind.clone(),
                model: row.model.clone(),
                endpoint: None,
                enabled: row.enabled,
                key: Some(key.to_owned()),
                context_window: row.context_window,
            },
            None => AiProviderInput {
                id: Some(choice.kind.to_owned()),
                name: choice.name.to_owned(),
                kind: choice.kind.to_owned(),
                model: choice.default_model.to_owned(),
                endpoint: None,
                enabled: true,
                key: Some(key.to_owned()),
                context_window: None,
            },
        };
        self.save_ai_provider(input)
    }

    /// Why Noura can't read its AI provider settings on this computer, if
    /// it can't.
    pub fn ai_settings_problem(&self) -> Option<String> {
        self.session.ai.problem()
    }

    /// Add a provider, or change one. Its key goes to this computer's
    /// credential store, and the settings stay in this device's app data.
    /// An endpoint must use HTTPS unless it points at this computer.
    /// Returns the provider's ID.
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

    /// The tool call waiting for your answer, if one is: the model asked to
    /// run a plugin's tool with this input.
    pub fn ai_tool_prompt(&self) -> Option<AiToolPrompt> {
        self.session.ai.tool_prompt()
    }

    /// Answer the prompt about `prompt`'s tool call: run it once, or not.
    /// Noura writes the call's result to the chat either way, and an answer
    /// to a prompt that's no longer showing changes nothing.
    pub fn answer_ai_tool(&mut self, prompt: &AiToolPrompt, allow: bool) {
        self.session
            .ai
            .answer_tool(&prompt.run, &prompt.call, allow);
        self.ai_changed();
    }

    /// Talk to AI providers through `client` from the next request on.
    /// Noura uses the `genai` client until a client sets another, as tests
    /// do with a fake provider.
    pub fn set_ai_client(&mut self, client: Arc<dyn AiStreamer>) {
        self.session.ai.set_client(client);
    }

    pub(super) fn ai_changed(&mut self) {
        self.ai_seen = self.session.ai.version();
        self.listeners.emit(Change::Ai);
    }

    /// Show what changed in the AI service since this window looked. The
    /// open workspace's expired chats go to the trash once an hour.
    pub(super) fn poll_ai(&mut self) -> bool {
        self.session.ai.expire_chats_if_due();
        let version = self.session.ai.version();
        if version == self.ai_seen {
            return false;
        }
        self.ai_seen = version;
        self.listeners.emit(Change::Ai);
        true
    }
}
