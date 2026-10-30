//! The AI service: the providers you set up on this device, their keys in
//! the operating system's credential store, your consent per workspace
//! and provider, and the runs that send a chat to a provider and stream
//! its answer into the chat's files.
//!
//! Plugins that hold the `ai` capability reach it through the host AI
//! interface, and Settings, the consent prompt and the tool prompt through
//! [`App`]. Every window shares one service. Provider settings and
//! consents live in this device's app data, never in the workspace, and
//! nothing the service hands a plugin holds a key.
//!
//! A run writes your message and an in-progress answer to the chat before
//! the request leaves the computer. The answer streams in on a thread of
//! its own, outside any plugin call, so an answer may take longer than a
//! plugin call's deadline; the plugin draws again as text arrives. The
//! answer and its status reach the chat's files before the run shows as
//! finished. Each request starts with Noura's instructions and the
//! workspace's `AGENTS.md`, and a chat that outgrew the provider's context
//! window gets a summary of its earlier messages first.
//!
//! The model may ask to call the tools that plugins on in the workspace
//! register. Each call waits for the prompt Noura draws, and runs in its
//! plugin only once you allow that one call.
//!
//! The service also holds web search: the Brave Search key, your web
//! consent per workspace, and the searches and page reads plugins ask
//! for, in [`web`].
//!
//! [`App`]: crate::App

#![allow(
    clippy::result_large_err,
    reason = "the run steps pass local-core's structured CoreError along, as local-core does"
)]

mod history;
mod tools;
mod web;

pub use web::{WEB_POLICY_VERSION, WEB_SEARCH_PROVIDER_NAME, WebConsentPrompt, WebSearchRow};

use std::{
    collections::{HashMap, HashSet, VecDeque},
    path::Path,
    sync::{
        Arc, Condvar, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use local_core::{
    AI_PROVIDER_KINDS, AiConsentDataCategory, AiConsentGrantInput, AiConsentReadInput,
    AiConsentRevokeInput, AiFoundation, AiModelRef, AiProviderConfig, AiStreamEvent, AiStreamFrame,
    AiStreamInput, AiStreamer, AiTransportMessage, AppendChatContextSummaryInput,
    AppendChatToolResultInput, AppendChatUserMessageInput, BeginChatAssistantInput,
    BeginChatToolCallInput, ChangeChatRetentionInput, Chat, ChatMessage, ChatMessageKind,
    ChatMessageStatus, ChatRead, ChatRetention, CoreError, CreateChatInput,
    FinishChatAssistantInput, FinishChatToolCallInput, GenAiStreamer, RenameChatInput,
    WorkspaceEngine,
};
use plugin_host::{
    AiChat, AiChats, AiFailure, AiMessage, AiMessageKind, AiMessageStatus, AiProvider, AiRequest,
    AiRetention, AiRun, AiRunStatus, AiToolCall, AiToolDefinition, AiToolResult, ServiceError,
};
pub use tools::AiToolPrompt;
pub(crate) use tools::ToolBox;
use tools::{PLUGIN_DISABLED, Settled, TOOL_NOT_FOUND};

use crate::{AppError, System, Workspace};

/// The version of what Noura tells you before a provider receives your
/// workspace's content. Consent holds for one version, so a new one asks
/// again.
pub const AI_POLICY_VERSION: &str = "2026-09";

/// The title a chat has until its first message names it.
pub const NEW_CHAT_TITLE: &str = "New chat";

/// The context window Noura counts on for a provider whose window you
/// didn't set, in tokens.
pub const DEFAULT_CONTEXT_WINDOW: u32 = 8_192;

/// The longest title a chat's first message gives it, in characters.
const TITLE_CHARS: usize = 64;

/// How many days an ephemeral chat lasts, as the workspace format says.
const EPHEMERAL_DAYS: u32 = 30;

/// How often Noura moves expired chats to the trash while a workspace
/// stays open. It also does so each time a workspace opens.
const EXPIRY_INTERVAL: Duration = Duration::from_secs(60 * 60);

/// Where the provider settings and the consents live, under the app data
/// folder.
const PROVIDERS_FILE: &str = "ai/providers.json";
const CONSENTS_FILE: &str = "ai/consents.json";

type Wake = Arc<dyn Fn() + Send + Sync>;

/// A provider as Settings lists it. It holds whether the provider has a
/// key, never the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiProviderRow {
    pub id: String,
    pub name: String,
    /// One of [`AI_PROVIDER_KINDS`], such as `openai`.
    pub kind: String,
    pub model: String,
    /// The address Noura sends requests to, when not the kind's own.
    pub endpoint: Option<String>,
    pub enabled: bool,
    /// Whether its key is in the credential store.
    pub has_key: bool,
    /// Whether you consented to it receiving the open workspace's content,
    /// for this provider's kind, endpoint and model and the current policy.
    pub consented: bool,
    /// How many tokens its model reads at most, when you set it. Without
    /// it, Noura counts on [`DEFAULT_CONTEXT_WINDOW`].
    pub context_window: Option<u32>,
}

/// A provider to add or change in Settings.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct AiProviderInput {
    /// The provider to change, or none to add one.
    pub id: Option<String>,
    pub name: String,
    pub kind: String,
    pub model: String,
    pub endpoint: Option<String>,
    pub enabled: bool,
    /// A new key, which goes to the credential store. None keeps the key
    /// the provider has.
    pub key: Option<String>,
    /// How many tokens its model reads at most, or none for
    /// [`DEFAULT_CONTEXT_WINDOW`].
    pub context_window: Option<u32>,
}

impl std::fmt::Debug for AiProviderInput {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AiProviderInput")
            .field("id", &self.id)
            .field("name", &self.name)
            .field("kind", &self.kind)
            .field("model", &self.model)
            .field("endpoint", &self.endpoint)
            .field("enabled", &self.enabled)
            .field("key", &self.key.as_ref().map(|_| "<hidden>"))
            .field("context_window", &self.context_window)
            .finish()
    }
}

/// The question Noura asks before a provider receives the open
/// workspace's content for the first time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiConsentPrompt {
    pub provider_id: String,
    pub provider_name: String,
    pub kind: String,
    pub model: String,
    pub endpoint: Option<String>,
    /// The open workspace's name.
    pub workspace: String,
}

/// What a run asks its provider once you consent.
enum Ask {
    /// Write this message of yours to the chat, then answer it.
    Message(String),
    /// Answer your last message in the chat again.
    Resume,
}

/// The registered tool a tool call names.
#[derive(Clone)]
struct Target {
    plugin: String,
    plugin_name: String,
    tool: String,
    description: String,
}

/// A tool call of a run that `run-tools` handed to Noura.
#[derive(Clone)]
struct PendingCall {
    call: AiToolCall,
    /// Its tool-call message.
    message: Option<String>,
    /// The registered tool it names, when the run offered it to the model
    /// and a plugin on in the workspace still offered it when the call was
    /// handed over.
    target: Option<Target>,
    /// Your answer to the prompt about it, once you gave one.
    answer: Option<bool>,
}

/// A message sent and its answer.
struct Run {
    id: String,
    /// The plugin that sent it, which draws again as the answer arrives.
    plugin: String,
    chat: String,
    provider: String,
    status: AiRunStatus,
    text: String,
    /// The assistant message the answer goes to.
    message: Option<String>,
    /// That message's revision as Noura began it.
    message_revision: String,
    /// The messages of yours this run wrote, which its requests carry even
    /// when the index doesn't list them yet.
    written: Vec<ChatMessage>,
    tool_calls: Vec<AiToolCall>,
    /// The tool-call message of each of `tool_calls`, by call ID.
    tool_messages: HashMap<String, String>,
    failure: Option<AiFailure>,
    /// While it waits for consent: what to ask.
    waiting: Option<Ask>,
    /// The stream's operation ID while it streams.
    operation: Option<String>,
    /// Stop was pressed while the run streamed but before its stream
    /// started, or while its tool calls waited, so it stops as soon as it
    /// can.
    stop_requested: bool,
    /// The tools it offers the model.
    tools: Vec<AiToolDefinition>,
    /// The plugins whose tools it offers.
    relies_on: Vec<String>,
    /// The tool calls `run-tools` handed over, in order, and the one the
    /// prompt asks about.
    calls: Vec<PendingCall>,
    asking: Option<usize>,
    /// A plugin it relied on was turned off.
    interrupted: bool,
    /// It answers your last message again, which it doesn't write.
    resumed: bool,
}

impl Run {
    fn new(
        id: &str,
        plugin: &str,
        chat: &str,
        provider: &str,
        ask: Ask,
        tools: Vec<AiToolDefinition>,
    ) -> Self {
        Self {
            id: id.to_owned(),
            plugin: plugin.to_owned(),
            chat: chat.to_owned(),
            provider: provider.to_owned(),
            status: AiRunStatus::WaitingForConsent,
            text: String::new(),
            message: None,
            message_revision: String::new(),
            written: Vec::new(),
            tool_calls: Vec::new(),
            tool_messages: HashMap::new(),
            failure: None,
            resumed: matches!(ask, Ask::Resume),
            waiting: Some(ask),
            operation: None,
            stop_requested: false,
            relies_on: tools::plugins_of(&tools),
            tools,
            calls: Vec::new(),
            asking: None,
            interrupted: false,
        }
    }

    fn public(&self) -> AiRun {
        AiRun {
            id: self.id.clone(),
            chat: self.chat.clone(),
            provider: self.provider.clone(),
            status: self.status,
            text: self.text.clone(),
            message: self.message.clone(),
            message_written: self.resumed || !self.written.is_empty(),
            tool_calls: self.tool_calls.clone(),
            failure: self.failure.clone(),
        }
    }

    /// Whether the run still has work in its chat: it waits for you, it
    /// streams, or the model's tool calls wait for `run-tools`. Its
    /// messages in progress are its own.
    fn live(&self) -> bool {
        match self.status {
            AiRunStatus::Streaming
            | AiRunStatus::WaitingForConsent
            | AiRunStatus::WaitingForApproval => true,
            AiRunStatus::Completed => !self.tool_calls.is_empty() && self.calls.is_empty(),
            _ => false,
        }
    }

    /// Whether it relies on the plugin `plugin`: the plugin sent it, or it
    /// offers the plugin's tools.
    fn relies_on(&self, plugin: &str) -> bool {
        self.plugin == plugin || self.relies_on.iter().any(|known| known == plugin)
    }

    fn interrupt(&mut self) {
        self.interrupted = true;
        self.waiting = None;
        self.status = AiRunStatus::Interrupted;
        self.failure = Some(failure_of(&plugin_disabled()));
    }
}

#[derive(Default)]
struct State {
    workspace: Option<Arc<Workspace>>,
    runs: HashMap<String, Run>,
    /// The runs waiting for your consent, oldest first. The prompt asks
    /// about the first one's provider.
    waiting: VecDeque<String>,
    /// The runs whose tool calls wait for your answers, oldest first.
    approvals: VecDeque<String>,
    /// The chats `create-chat` started whose files aren't written yet:
    /// their first message hasn't gone out.
    new_chats: HashSet<String>,
    /// By chat: the prompt tokens the provider counted for its last
    /// request, and that request's size in characters.
    counted: HashMap<String, (u64, usize)>,
}

pub(crate) struct Ai {
    /// The provider settings and consents, or why they can't be read.
    store: Result<AiFoundation, AppError>,
    /// The clock that expires chats.
    system: Arc<dyn System>,
    /// Drives the provider requests.
    runtime: tokio::runtime::Runtime,
    /// Talks to the providers: `genai`, or a fake in tests.
    client: Mutex<Arc<dyn AiStreamer>>,
    /// The tools of the plugins running in the open workspace.
    toolbox: Mutex<Option<Arc<dyn ToolBox>>>,
    state: Mutex<State>,
    /// Wakes the runs whose tool calls wait, when you answer the prompt,
    /// stop a run, a plugin goes or the workspace closes.
    answered: Condvar,
    /// The plugins to draw again, as text arrived for their runs.
    redraws: Mutex<Vec<String>>,
    wake: Mutex<Option<Wake>>,
    /// Counts the changes windows show: the prompts, providers and
    /// consents.
    version: AtomicU64,
    /// The providers or consents changed since the plugins drew, so every
    /// plugin that holds `ai` draws again.
    settings_changed: std::sync::atomic::AtomicBool,
    /// When the open workspace's chats last expired.
    expired: Mutex<Option<Instant>>,
    /// The web search key and consents.
    web: web::Web,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Ai {
    /// The service, with provider settings and consents in `app_data`,
    /// keys in `system`'s credential store and chats expiring by its
    /// clock.
    pub(crate) fn new(app_data: &Path, system: Arc<dyn System>) -> Arc<Self> {
        let web = web::Web::new(app_data, system.credentials());
        let store = AiFoundation::open_with(
            app_data.join(PROVIDERS_FILE),
            app_data.join(CONSENTS_FILE),
            system.credentials(),
        )
        .map_err(|error| {
            tracing::warn!(code = %error.code, "the AI settings can't be read");
            AppError::new(
                "ai_settings_unreadable",
                "Noura can't read its AI provider settings on this computer, so it left them \
                 as they were.",
            )
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .thread_name("ai")
            .enable_all()
            .build()
            .expect("the AI service's runtime starts");
        Arc::new(Self {
            store,
            system,
            runtime,
            client: Mutex::new(Arc::new(GenAiStreamer)),
            toolbox: Mutex::new(None),
            state: Mutex::new(State::default()),
            answered: Condvar::new(),
            redraws: Mutex::new(Vec::new()),
            wake: Mutex::new(None),
            version: AtomicU64::new(0),
            settings_changed: std::sync::atomic::AtomicBool::new(false),
            expired: Mutex::new(None),
            web,
        })
    }

    pub(crate) fn set_wake(&self, wake: Wake) {
        *lock(&self.wake) = Some(wake);
    }

    /// Reach the running plugins' tools through `toolbox`.
    pub(crate) fn set_toolbox(&self, toolbox: Arc<dyn ToolBox>) {
        *lock(&self.toolbox) = Some(toolbox);
    }

    /// Talk to providers through `client` from the next request on.
    pub(crate) fn set_client(&self, client: Arc<dyn AiStreamer>) {
        *lock(&self.client) = client;
    }

    pub(crate) fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    /// Something windows and plugins show changed: wake them.
    fn changed(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
        self.settings_changed.store(true, Ordering::SeqCst);
        self.wake();
    }

    /// Whether the providers or consents changed since the last call, so
    /// every plugin that holds `ai` draws again.
    pub(crate) fn take_settings_changed(&self) -> bool {
        self.settings_changed.swap(false, Ordering::SeqCst)
    }

    fn wake(&self) {
        let wake = lock(&self.wake).clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    /// Draw the plugin `plugin` again, as its run changed.
    fn redraw(&self, plugin: &str) {
        let mut redraws = lock(&self.redraws);
        if !redraws.iter().any(|known| known == plugin) {
            redraws.push(plugin.to_owned());
        }
        drop(redraws);
        self.wake();
    }

    /// The plugins whose runs changed since the last call.
    pub(crate) fn take_redraws(&self) -> Vec<String> {
        std::mem::take(&mut *lock(&self.redraws))
    }

    fn store(&self) -> Result<&AiFoundation, AppError> {
        self.store.as_ref().map_err(Clone::clone)
    }

    fn toolbox(&self) -> Option<Arc<dyn ToolBox>> {
        lock(&self.toolbox).clone()
    }

    /// Why the provider settings can't be read, if they can't.
    pub(crate) fn problem(&self) -> Option<String> {
        self.store.as_ref().err().map(|error| error.message.clone())
    }

    /// Chat in `workspace` from now on. Runs in the workspace before stop,
    /// and the tool calls that waited for you count as denied. The chats
    /// of the workspace that opens expire.
    pub(crate) fn open_workspace(self: &Arc<Self>, workspace: Option<Arc<Workspace>>) {
        let mut state = lock(&self.state);
        let same = match (&state.workspace, &workspace) {
            (Some(open), Some(next)) => open.id() == next.id(),
            (None, None) => true,
            _ => false,
        };
        if same {
            state.workspace = workspace;
            return;
        }
        let operations: Vec<String> = state
            .runs
            .values()
            .filter_map(|run| run.operation.clone())
            .collect();
        state.runs.clear();
        state.waiting.clear();
        state.approvals.clear();
        state.new_chats.clear();
        state.counted.clear();
        state.workspace = workspace;
        drop(state);
        // The runs whose tool calls wait find themselves gone, and write a
        // denial for each call to the closing workspace's chat.
        self.answered.notify_all();
        // The web prompt asked about the workspace that closed.
        self.web.close_prompt();
        if let Ok(store) = self.store() {
            for operation in operations {
                let _ = store.cancel_stream(&operation);
            }
        }
        *lock(&self.expired) = None;
        self.expire_chats_if_due();
        self.changed();
    }

    fn workspace(&self) -> Option<Arc<Workspace>> {
        lock(&self.state).workspace.clone()
    }

    /// Move the open workspace's expired chats to the trash, on a thread of
    /// its own, when they haven't expired in the last hour. An expired
    /// chat goes only when its folder holds nothing but the chat's own
    /// files, unchanged since it expired.
    pub(crate) fn expire_chats_if_due(self: &Arc<Self>) {
        {
            let mut expired = lock(&self.expired);
            if expired.is_some_and(|at| at.elapsed() < EXPIRY_INTERVAL) {
                return;
            }
            *expired = Some(Instant::now());
        }
        let Some(workspace) = self.workspace() else {
            return;
        };
        let now = self.system.now().to_string();
        let ai = self.clone();
        let started = std::thread::Builder::new()
            .name("ai-expiry".to_owned())
            .spawn(move || match workspace.shared_engine().expire_chats(&now) {
                Ok(expired) if !expired.is_empty() => {
                    tracing::info!(count = expired.len(), "expired chats went to the trash");
                    ai.changed();
                }
                Ok(_) => {}
                Err(error) => tracing::warn!(code = %error.code, "expired chats stayed"),
            });
        if let Err(error) = started {
            tracing::warn!(%error, "the chat expiry's thread didn't start");
        }
    }

    // Settings.

    /// Every provider you set up, as Settings lists them.
    pub(crate) fn provider_rows(&self) -> Vec<AiProviderRow> {
        let Ok(store) = self.store() else {
            return Vec::new();
        };
        let workspace = self.workspace();
        store
            .list_providers()
            .unwrap_or_default()
            .into_iter()
            .map(|provider| {
                let consented = workspace
                    .as_ref()
                    .is_some_and(|workspace| consented(store, workspace.id(), &provider.id));
                AiProviderRow {
                    id: provider.id,
                    name: provider.display_name,
                    kind: provider.kind,
                    model: provider.model,
                    endpoint: provider.endpoint,
                    enabled: provider.enabled,
                    has_key: provider.credential_ref.is_some(),
                    consented,
                    context_window: provider.context_window,
                }
            })
            .collect()
    }

    /// Add or change a provider, and keep its new key in the credential
    /// store. Returns the provider's ID.
    pub(crate) fn save_provider(&self, input: AiProviderInput) -> Result<String, AppError> {
        let store = self.store()?;
        let existing = store.list_providers().map_err(settings_error)?;
        let name = input.name.trim().to_owned();
        let id = match &input.id {
            Some(id) => id.clone(),
            None => new_provider_id(&name, &existing),
        };
        let current = existing.iter().find(|provider| provider.id == id);
        let endpoint = input
            .endpoint
            .as_deref()
            .map(str::trim)
            .filter(|endpoint| !endpoint.is_empty())
            .map(str::to_owned);
        if input.context_window == Some(0) {
            return Err(AppError::new(
                "provider_context_window_invalid",
                "Give the context window as a number of tokens above zero.",
            ));
        }
        let provider = AiProviderConfig {
            id: id.clone(),
            kind: input.kind.trim().to_owned(),
            display_name: if name.is_empty() { id.clone() } else { name },
            model: input.model.trim().to_owned(),
            endpoint,
            credential_ref: current.and_then(|current| current.credential_ref.clone()),
            enabled: input.enabled,
            context_window: input.context_window,
        };
        store.save_provider(provider).map_err(settings_error)?;
        if let Some(key) = input.key.filter(|key| !key.trim().is_empty()) {
            store
                .set_credential(&id, key.trim())
                .map_err(settings_error)?;
        }
        self.changed();
        Ok(id)
    }

    /// Remove a provider, its key and its consents. Its running requests
    /// stop, and the runs that waited for consent to it fail, so the
    /// prompt moves on to the next provider.
    pub(crate) fn remove_provider(&self, id: &str) -> Result<(), AppError> {
        self.store()?.remove_provider(id).map_err(settings_error)?;
        let plugins: Vec<String> = {
            let mut state = lock(&self.state);
            let gone: Vec<String> = state
                .waiting
                .iter()
                .filter(|run| state.runs.get(*run).is_some_and(|run| run.provider == id))
                .cloned()
                .collect();
            state.waiting.retain(|run| !gone.contains(run));
            gone.iter()
                .filter_map(|run| {
                    let run = state.runs.get_mut(run)?;
                    run.status = AiRunStatus::Failed;
                    run.failure = Some(failure_of(&provider_missing()));
                    run.waiting = None;
                    Some(run.plugin.clone())
                })
                .collect()
        };
        self.changed();
        for plugin in plugins {
            self.redraw(&plugin);
        }
        Ok(())
    }

    /// Forget a provider's key.
    pub(crate) fn remove_key(&self, id: &str) -> Result<(), AppError> {
        self.store()?.clear_credential(id).map_err(settings_error)?;
        self.changed();
        Ok(())
    }

    /// Consent to the provider `id` receiving the open workspace's
    /// content, and send what waited for it.
    pub(crate) fn grant(self: &Arc<Self>, id: &str) -> Result<(), AppError> {
        let workspace = self.workspace().ok_or_else(no_workspace)?;
        self.store()?
            .grant_consent(
                workspace.id(),
                AiConsentGrantInput {
                    provider_id: id.to_owned(),
                    policy_version: AI_POLICY_VERSION.to_owned(),
                    data_category: AiConsentDataCategory::WorkspaceContent,
                },
            )
            .map_err(settings_error)?;
        let waiting: Vec<String> = {
            let mut state = lock(&self.state);
            let ready: Vec<String> = state
                .waiting
                .iter()
                .filter(|run| state.runs.get(*run).is_some_and(|run| run.provider == id))
                .cloned()
                .collect();
            state.waiting.retain(|run| !ready.contains(run));
            ready
        };
        self.changed();
        for run in waiting {
            self.start(&run);
        }
        Ok(())
    }

    /// Withdraw your consent for the provider `id` in the open workspace.
    /// Its running requests stop, and its next request asks again.
    pub(crate) fn revoke(&self, id: &str) -> Result<(), AppError> {
        let workspace = self.workspace().ok_or_else(no_workspace)?;
        self.store()?
            .revoke_consent(
                workspace.id(),
                AiConsentRevokeInput {
                    provider_id: id.to_owned(),
                    policy_version: AI_POLICY_VERSION.to_owned(),
                },
            )
            .map_err(settings_error)?;
        self.changed();
        Ok(())
    }

    /// The question waiting for your answer, if one is.
    pub(crate) fn prompt(&self) -> Option<AiConsentPrompt> {
        let state = lock(&self.state);
        let workspace = state.workspace.as_ref()?;
        let providers = self.store().ok()?.list_providers().ok()?;
        // A run whose provider went waits for nothing: the prompt asks
        // about the next one.
        let provider = state
            .waiting
            .iter()
            .filter_map(|run| state.runs.get(run))
            .find_map(|run| {
                providers
                    .iter()
                    .find(|provider| provider.id == run.provider)
            })?
            .clone();
        Some(AiConsentPrompt {
            provider_id: provider.id,
            provider_name: provider.display_name,
            kind: provider.kind,
            model: provider.model,
            endpoint: provider.endpoint,
            workspace: workspace.name(),
        })
    }

    /// Answer the prompt: allow the provider it names, or not. Saying no
    /// sends and writes nothing for the runs that waited for it.
    pub(crate) fn answer_prompt(self: &Arc<Self>, allow: bool) -> Result<(), AppError> {
        let Some(prompt) = self.prompt() else {
            return Ok(());
        };
        if allow {
            return self.grant(&prompt.provider_id);
        }
        let plugins: Vec<String> = {
            let mut state = lock(&self.state);
            let denied: Vec<String> = state
                .waiting
                .iter()
                .filter(|run| {
                    state
                        .runs
                        .get(*run)
                        .is_some_and(|run| run.provider == prompt.provider_id)
                })
                .cloned()
                .collect();
            state.waiting.retain(|run| !denied.contains(run));
            denied
                .iter()
                .filter_map(|id| {
                    let run = state.runs.get_mut(id)?;
                    run.status = AiRunStatus::Denied;
                    run.waiting = None;
                    Some(run.plugin.clone())
                })
                .collect()
        };
        self.changed();
        for plugin in plugins {
            self.redraw(&plugin);
        }
        Ok(())
    }

    /// The tool call waiting for your answer, if one is: the oldest run's
    /// call that the run waits on.
    pub(crate) fn tool_prompt(&self) -> Option<AiToolPrompt> {
        let state = lock(&self.state);
        state.approvals.iter().find_map(|id| {
            let run = state.runs.get(id)?;
            let pending = run.calls.get(run.asking?)?;
            let target = pending.target.as_ref()?;
            pending.answer.is_none().then(|| AiToolPrompt {
                run: run.id.clone(),
                call: pending.call.id.clone(),
                tool: target.tool.clone(),
                description: target.description.clone(),
                plugin_id: target.plugin.clone(),
                plugin_name: target.plugin_name.clone(),
                input: tools::indented(&pending.call.arguments),
            })
        })
    }

    /// Answer the prompt about the call `call` of the run `run`: run it
    /// once, or not. An answer about a call the prompt no longer asks
    /// about changes nothing.
    pub(crate) fn answer_tool(&self, run: &str, call: &str, allow: bool) {
        {
            let mut state = lock(&self.state);
            let Some(run) = state.runs.get_mut(run) else {
                return;
            };
            let Some(index) = run.asking else {
                return;
            };
            match run.calls.get_mut(index) {
                Some(pending) if pending.call.id == call && pending.answer.is_none() => {
                    pending.answer = Some(allow);
                }
                _ => return,
            }
            run.asking = None;
        }
        self.answered.notify_all();
        self.changed();
    }

    /// The plugin `plugin` was turned off or removed: the runs it sent,
    /// and the runs that offer its tools, end as interrupted with the
    /// `plugin-disabled` code. Its files stay.
    pub(crate) fn plugin_disabled(self: &Arc<Self>, plugin: &str) {
        let engine = self.workspace().map(|workspace| workspace.shared_engine());
        let mut operations = Vec::new();
        let mut rounds = Vec::new();
        let mut plugins = Vec::new();
        {
            let mut state = lock(&self.state);
            let mut stopped = Vec::new();
            for run in state.runs.values_mut() {
                if !run.relies_on(plugin) {
                    continue;
                }
                match run.status {
                    AiRunStatus::Streaming => {
                        run.interrupted = true;
                        match &run.operation {
                            Some(operation) => operations.push(operation.clone()),
                            // The stream stops as soon as it starts.
                            None => run.stop_requested = true,
                        }
                    }
                    // The run's round writes a result for each call left.
                    AiRunStatus::WaitingForApproval => run.interrupted = true,
                    AiRunStatus::WaitingForConsent => {
                        run.interrupt();
                        stopped.push(run.id.clone());
                    }
                    AiRunStatus::Completed if run.live() => {
                        // Nobody ran the model's calls: each gets its
                        // result now, and the run ends.
                        run.interrupted = true;
                        run.calls = pending_calls(run, &[], &HashMap::new());
                        run.status = AiRunStatus::WaitingForApproval;
                        rounds.push(run.id.clone());
                    }
                    _ => continue,
                }
                plugins.push(run.plugin.clone());
            }
            state.waiting.retain(|run| !stopped.contains(run));
            for run in &rounds {
                state.approvals.push_back(run.clone());
            }
        }
        if plugins.is_empty() {
            return;
        }
        self.answered.notify_all();
        if let Ok(store) = self.store() {
            for operation in operations {
                let _ = store.cancel_stream(&operation);
            }
        }
        if let Some(engine) = engine {
            for run in rounds {
                self.start_round(&run, engine.clone());
            }
        }
        self.changed();
        for plugin in plugins {
            self.redraw(&plugin);
        }
    }

    /// What the plugin `plugin` reaches through the host AI interface.
    pub(crate) fn access(self: &Arc<Self>, plugin: &str) -> Arc<dyn AiChats> {
        Arc::new(Access {
            ai: self.clone(),
            plugin: plugin.to_owned(),
        })
    }

    // Runs.

    fn engine(&self) -> Result<Arc<WorkspaceEngine>, ServiceError> {
        self.workspace()
            .map(|workspace| workspace.shared_engine())
            .ok_or_else(|| ServiceError::Failed("No workspace is open.".to_owned()))
    }

    /// The enabled provider `id`.
    fn provider(&self, id: &str) -> Result<AiProviderConfig, ServiceError> {
        let store = self
            .store()
            .map_err(|error| ServiceError::Failed(error.message))?;
        store
            .list_providers()
            .map_err(service_error)?
            .into_iter()
            .find(|provider| provider.id == id && provider.enabled)
            .ok_or_else(|| ServiceError::NotFound(id.to_owned()))
    }

    fn send(self: &Arc<Self>, plugin: &str, request: AiRequest) -> Result<AiRun, ServiceError> {
        let engine = self.engine()?;
        if request.message.trim().is_empty() {
            return Err(ServiceError::Failed("The message is empty.".to_owned()));
        }
        if !lock(&self.state).new_chats.contains(&request.chat) {
            engine
                .read_chat(&request.chat)
                .map_err(|error| chat_error(error, &request.chat))?;
        }
        self.begin_run(
            plugin,
            &request.chat,
            &request.provider,
            Ask::Message(request.message),
            request.tools,
        )
    }

    fn resume(
        self: &Arc<Self>,
        plugin: &str,
        chat: &str,
        provider: &str,
        tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError> {
        let engine = self.engine()?;
        let nothing = || {
            ServiceError::Failed("This chat has no message of yours to answer again.".to_owned())
        };
        if lock(&self.state).new_chats.contains(chat) {
            return Err(nothing());
        }
        let read = engine
            .read_chat(chat)
            .map_err(|error| chat_error(error, chat))?;
        if !read.messages.iter().any(|message| {
            message.kind == ChatMessageKind::User && message.status == ChatMessageStatus::Completed
        }) {
            return Err(nothing());
        }
        self.begin_run(plugin, chat, provider, Ask::Resume, tools)
    }

    /// Start a run of `plugin` in `chat` that asks `provider` what `ask`
    /// says, offering `tools`: now when you consented, otherwise once you
    /// do.
    fn begin_run(
        self: &Arc<Self>,
        plugin: &str,
        chat: &str,
        provider: &str,
        ask: Ask,
        tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError> {
        let workspace = self
            .workspace()
            .ok_or_else(|| ServiceError::Failed("No workspace is open.".to_owned()))?;
        let provider = self.provider(provider)?;
        check_tools(&tools)?;
        let id = format!("run_{}", uuid_v4());
        let run = {
            let mut state = lock(&self.state);
            if state
                .runs
                .values()
                .any(|run| run.chat == chat && run.live())
            {
                return Err(ServiceError::Failed(
                    "This chat is still answering your last message.".to_owned(),
                ));
            }
            let run = Run::new(&id, plugin, chat, &provider.id, ask, tools);
            let public = run.public();
            state.runs.insert(id.clone(), run);
            public
        };
        let store = self
            .store()
            .map_err(|error| ServiceError::Failed(error.message))?;
        if consented(store, workspace.id(), &provider.id) {
            self.start(&id);
            return self.read_run(&id);
        }
        lock(&self.state).waiting.push_back(id);
        self.changed();
        Ok(run)
    }

    /// Write the message of the run `id`, unless it answers your last one
    /// again, then begin its answer.
    fn start(self: &Arc<Self>, id: &str) {
        let (chat, provider_id, ask, tools) = {
            let mut state = lock(&self.state);
            let Some(run) = state.runs.get_mut(id) else {
                return;
            };
            let Some(ask) = run.waiting.take() else {
                return;
            };
            run.status = AiRunStatus::Streaming;
            (
                run.chat.clone(),
                run.provider.clone(),
                ask,
                run.tools.clone(),
            )
        };
        let written = (|| {
            let engine = self.engine().map_err(|_| no_workspace_core())?;
            if let Ask::Message(message) = ask {
                self.write_message(id, &engine, &chat, &message)?;
            }
            Ok::<_, CoreError>(engine)
        })();
        match written {
            Ok(engine) => self.ask(id, &engine, &provider_id, tools),
            Err(error) => self.fail(id, &error),
        }
    }

    /// Write your `message` to `chat` for the run `id`. A chat
    /// `create-chat` started gets its file now, named after it.
    fn write_message(
        &self,
        id: &str,
        engine: &WorkspaceEngine,
        chat: &str,
        message: &str,
    ) -> Result<(), CoreError> {
        let new = lock(&self.state).new_chats.contains(chat);
        let (revision, names) = if new {
            let created = engine.create_chat_with_id(
                chat,
                CreateChatInput {
                    title: chat_title(message),
                    retention: None,
                    retention_days: None,
                },
            )?;
            lock(&self.state).new_chats.remove(chat);
            (created.revision, false)
        } else {
            let read = engine.read_chat(chat)?;
            let names = read.chat.title == NEW_CHAT_TITLE
                && !read
                    .messages
                    .iter()
                    .any(|message| message.kind == ChatMessageKind::User);
            (read.chat.revision, names)
        };
        let appended = engine.append_chat_user_message(AppendChatUserMessageInput {
            chat_id: chat.to_owned(),
            run_id: id.to_owned(),
            content: message.to_owned(),
            expected_chat_revision: revision,
        })?;
        if let Some(run) = lock(&self.state).runs.get_mut(id) {
            run.written.push(appended.value.clone());
        }
        if names {
            let revision = appended.chat_revision.clone().unwrap_or_default();
            // The title waits for the next message when the chat changed
            // elsewhere meanwhile: a change made there wins.
            if let Err(error) = engine.rename_chat(RenameChatInput {
                chat_id: chat.to_owned(),
                title: chat_title(message),
                expected_chat_revision: revision,
            }) {
                tracing::warn!(code = %error.code, "a chat kept its title");
            }
        }
        Ok(())
    }

    /// Begin the run's next answer in progress, then ask its provider on a
    /// thread of the run's own.
    fn ask(
        self: &Arc<Self>,
        id: &str,
        engine: &Arc<WorkspaceEngine>,
        provider_id: &str,
        tools: Vec<AiToolDefinition>,
    ) {
        let chat = match lock(&self.state).runs.get(id) {
            Some(run) => run.chat.clone(),
            None => return,
        };
        let begun = (|| {
            let provider = self.provider(provider_id).map_err(|_| provider_missing())?;
            let read = engine.read_chat(&chat)?;
            let begun = engine.begin_chat_assistant(BeginChatAssistantInput {
                chat_id: chat.clone(),
                run_id: id.to_owned(),
                provider_id: provider.id.clone(),
                model_id: provider.model.clone(),
                expected_chat_revision: read.chat.revision,
            })?;
            Ok::<_, CoreError>((provider, begun.value.id, begun.revision))
        })();
        let (provider, message, revision) = match begun {
            Ok(begun) => begun,
            Err(error) => {
                self.fail(id, &error);
                return;
            }
        };
        if let Some(run) = lock(&self.state).runs.get_mut(id) {
            run.message = Some(message.clone());
            run.message_revision = revision.clone();
            run.text.clear();
            run.tool_calls.clear();
            run.tool_messages.clear();
            run.calls.clear();
            run.asking = None;
            run.tools = tools.clone();
        }
        let answer = Answer {
            chat,
            message,
            begun: revision,
            provider: provider.id.clone(),
            model: provider.model.clone(),
            text: String::new(),
            calls: Vec::new(),
        };
        let ai = self.clone();
        let run = id.to_owned();
        let followed = engine.clone();
        let started = std::thread::Builder::new()
            .name("ai-run".to_owned())
            .spawn(move || ai.answer(&run, &followed, &provider, answer, &tools));
        if let Err(error) = started {
            tracing::warn!(%error, "an AI run's thread didn't start");
            self.fail(id, &incomplete());
        }
        self.redraw_run(id);
    }

    /// Ask the provider for the answer the run `id` began, summarizing the
    /// chat first when it no longer fits the provider's context window,
    /// then write the answer and how it ended.
    fn answer(
        self: &Arc<Self>,
        id: &str,
        engine: &Arc<WorkspaceEngine>,
        provider: &AiProviderConfig,
        mut answer: Answer,
        tools: &[AiToolDefinition],
    ) {
        let ended = self
            .answer_steps(id, engine, provider, &mut answer, tools)
            .unwrap_or_else(|error| {
                if error.code == CANCELLED {
                    Ended::Cancelled
                } else {
                    Ended::Failed(Box::new(error))
                }
            });
        self.end(id, engine, answer, ended);
    }

    fn answer_steps(
        &self,
        id: &str,
        engine: &WorkspaceEngine,
        provider: &AiProviderConfig,
        answer: &mut Answer,
        tools: &[AiToolDefinition],
    ) -> Result<Ended, CoreError> {
        let instructions = history::instructions(engine);
        let mut request = self.request(id, engine, &instructions, &answer.message)?;
        if self.too_long(&answer.chat, provider, &request) {
            if let Some(ended) =
                self.summarize(id, engine, provider, &instructions, &answer.chat)?
            {
                return Ok(ended);
            }
            request = self.request(id, engine, &instructions, &answer.message)?;
        }
        let characters = history::characters(&request);
        let Some(frames) = self.open_stream(id, provider, request, Some(history::tools(tools)))?
        else {
            return Ok(Ended::Cancelled);
        };
        let chat = answer.chat.clone();
        let mut text = String::new();
        let mut calls = Vec::new();
        let ended = self.take(
            id,
            &chat,
            frames,
            Some(characters),
            |delta| {
                text.push_str(delta);
                if let Some(run) = lock(&self.state).runs.get_mut(id) {
                    run.text.push_str(delta);
                }
                self.redraw_run(id);
            },
            |call| {
                if let Some(run) = lock(&self.state).runs.get_mut(id) {
                    run.tool_calls.push(call.clone());
                }
                calls.push(call);
            },
        );
        answer.text = text;
        answer.calls = calls;
        Ok(ended)
    }

    /// The request for the answer `answering` of the run `id`, with the
    /// messages of yours it wrote even when the index doesn't list them
    /// yet.
    fn request(
        &self,
        id: &str,
        engine: &WorkspaceEngine,
        instructions: &str,
        answering: &str,
    ) -> Result<Vec<AiTransportMessage>, CoreError> {
        let (chat, written) = match lock(&self.state).runs.get(id) {
            Some(run) => (run.chat.clone(), run.written.clone()),
            None => return Err(cancelled()),
        };
        let mut read = engine.read_chat(&chat)?;
        for mine in written {
            if !read.messages.iter().any(|known| known.id == mine.id) {
                read.messages.push(mine);
            }
        }
        Ok(history::request(
            instructions,
            &read.messages,
            answering,
            id,
        ))
    }

    /// Whether `request` no longer fits the context window of `provider`,
    /// with an eighth of the window left for the answer.
    fn too_long(
        &self,
        chat: &str,
        provider: &AiProviderConfig,
        request: &[AiTransportMessage],
    ) -> bool {
        let window = u64::from(provider.context_window.unwrap_or(DEFAULT_CONTEXT_WINDOW));
        let counted = lock(&self.state).counted.get(chat).copied();
        history::estimate(request, counted) + window / 8 > window
    }

    /// Ask the provider to summarize the chat's messages before your last
    /// one, and write the summary to the chat as a context summary that
    /// names the last message it covers. Returns how the run ended when
    /// the summary didn't come, as when you pressed Stop.
    fn summarize(
        &self,
        id: &str,
        engine: &WorkspaceEngine,
        provider: &AiProviderConfig,
        instructions: &str,
        chat: &str,
    ) -> Result<Option<Ended>, CoreError> {
        let read = engine.read_chat(chat)?;
        let Some((through, request)) = history::summary_request(instructions, &read.messages)
        else {
            return Ok(None);
        };
        let Some(frames) = self.open_stream(id, provider, request, None)? else {
            return Ok(Some(Ended::Cancelled));
        };
        let mut summary = String::new();
        let ended = self.take(
            id,
            chat,
            frames,
            None,
            |delta| summary.push_str(delta),
            |_| {},
        );
        if !matches!(ended, Ended::Completed) {
            return Ok(Some(ended));
        }
        if summary.trim().is_empty() {
            return Ok(None);
        }
        let mut attempts = 0;
        loop {
            attempts += 1;
            let read = engine.read_chat(chat)?;
            let appended = engine.append_chat_context_summary(AppendChatContextSummaryInput {
                chat_id: chat.to_owned(),
                run_id: id.to_owned(),
                summarizes_through_message_id: through.clone(),
                content: summary.trim().to_owned(),
                expected_chat_revision: read.chat.revision,
            });
            match appended {
                Ok(_) => break,
                // The chat changed elsewhere after the read.
                Err(error) if error.code == "revision_conflict" && attempts < 3 => {}
                Err(error) => return Err(error),
            }
        }
        // The provider's count was of the longer chat.
        lock(&self.state).counted.remove(chat);
        Ok(None)
    }

    /// Start a stream of `messages` to `provider` for the run `id`. None
    /// when the run went, as after a switch to another workspace. A Stop
    /// pressed before the stream started stops it at once.
    fn open_stream(
        &self,
        id: &str,
        provider: &AiProviderConfig,
        messages: Vec<AiTransportMessage>,
        tools: Option<Vec<local_core::AiToolDefinition>>,
    ) -> Result<Option<tokio::sync::mpsc::Receiver<AiStreamFrame>>, CoreError> {
        if !lock(&self.state).runs.contains_key(id) {
            return Ok(None);
        }
        let workspace = self.workspace().ok_or_else(no_workspace_core)?;
        let operation = uuid_v4();
        let input = AiStreamInput {
            operation_id: operation.clone(),
            model: AiModelRef {
                provider_id: provider.id.clone(),
                model: provider.model.clone(),
            },
            messages,
            tools,
            policy_version: Some(AI_POLICY_VERSION.to_owned()),
        };
        let client = lock(&self.client).clone();
        let store = self.store().map_err(|_| no_workspace_core())?;
        let stream = {
            let _entered = self.runtime.enter();
            store.start_stream_with(workspace.id(), input, client)?
        };
        if self.streaming(id, &operation) {
            let _ = store.cancel_stream(&operation);
        }
        Ok(Some(stream.receiver))
    }

    /// Record that the run `id` streams as `operation`. Returns whether
    /// you pressed Stop before the stream started, so it stops now.
    fn streaming(&self, id: &str, operation: &str) -> bool {
        let mut state = lock(&self.state);
        let Some(run) = state.runs.get_mut(id) else {
            return true;
        };
        run.operation = Some(operation.to_owned());
        std::mem::take(&mut run.stop_requested)
    }

    /// Take a stream's frames until it ends, handing on its text and tool
    /// calls. With `characters`, the size of the request, the provider's
    /// count of the request's tokens is kept for the chat's next estimate.
    fn take(
        &self,
        id: &str,
        chat: &str,
        mut frames: tokio::sync::mpsc::Receiver<AiStreamFrame>,
        characters: Option<usize>,
        mut on_text: impl FnMut(&str),
        mut on_call: impl FnMut(AiToolCall),
    ) -> Ended {
        let ended = loop {
            let Some(frame) = frames.blocking_recv() else {
                break Ended::Failed(Box::new(incomplete()));
            };
            match frame.event {
                AiStreamEvent::Started { .. } => {}
                AiStreamEvent::TextDelta { text } => on_text(&text),
                AiStreamEvent::ToolCall { call } => on_call(AiToolCall {
                    id: call.call_id,
                    name: call.name,
                    arguments: call.arguments.to_string(),
                }),
                AiStreamEvent::Completed { summary } => {
                    let tokens = summary
                        .usage
                        .and_then(|usage| usage.prompt_tokens)
                        .and_then(|tokens| u64::try_from(tokens).ok())
                        .filter(|tokens| *tokens > 0);
                    if let (Some(tokens), Some(characters)) = (tokens, characters) {
                        lock(&self.state)
                            .counted
                            .insert(chat.to_owned(), (tokens, characters));
                    }
                    break Ended::Completed;
                }
                AiStreamEvent::Cancelled => break Ended::Cancelled,
                AiStreamEvent::Error { error } => break Ended::Failed(Box::new(error)),
            }
        };
        if let Some(run) = lock(&self.state).runs.get_mut(id) {
            run.operation = None;
        }
        ended
    }

    /// Write the run's answer with how it ended, and the tool calls it
    /// asked for, then show the run as ended. The answer reaches its file
    /// even when the run is gone, as after a switch to another workspace.
    /// A run a plugin it relied on left ends as interrupted.
    fn end(
        self: &Arc<Self>,
        id: &str,
        engine: &Arc<WorkspaceEngine>,
        answer: Answer,
        ended: Ended,
    ) {
        let interrupted = lock(&self.state)
            .runs
            .get(id)
            .is_some_and(|run| run.interrupted);
        let ended = match ended {
            Ended::Completed | Ended::Cancelled if interrupted => Ended::Interrupted,
            ended => ended,
        };
        let (status, error) = match &ended {
            Ended::Completed => (ChatMessageStatus::Completed, None),
            Ended::Cancelled => (ChatMessageStatus::Cancelled, None),
            Ended::Interrupted => (
                ChatMessageStatus::Interrupted,
                Some(PLUGIN_DISABLED.to_owned()),
            ),
            Ended::Failed(error) => (ChatMessageStatus::Failed, Some(error_code(error))),
        };
        let chat = answer.chat.clone();
        let calls = answer.calls.clone();
        let mut holder = None;
        let written =
            finish_answer(engine, id, &answer, status, error.as_deref()).and_then(|message| {
                holder = Some(message);
                if !matches!(ended, Ended::Completed) {
                    return Ok(HashMap::new());
                }
                let mut messages = HashMap::new();
                for call in &calls {
                    let read = engine.read_chat(&chat)?;
                    let begun = engine.begin_chat_tool_call(BeginChatToolCallInput {
                        chat_id: chat.clone(),
                        run_id: id.to_owned(),
                        tool_call_id: call.id.clone(),
                        tool_name: call.name.clone(),
                        content: call.arguments.clone(),
                        expected_chat_revision: read.chat.revision,
                    })?;
                    messages.insert(call.id.clone(), begun.value.id);
                }
                Ok(messages)
            });
        {
            let mut state = lock(&self.state);
            if let Some(run) = state.runs.get_mut(id) {
                run.operation = None;
                if holder.is_some() {
                    run.message = holder;
                }
                match (&ended, written) {
                    (_, Err(error)) => {
                        run.status = AiRunStatus::Failed;
                        run.failure = Some(failure_of(&error));
                    }
                    (Ended::Completed, Ok(messages)) => {
                        run.status = AiRunStatus::Completed;
                        run.tool_messages = messages;
                    }
                    (Ended::Cancelled, Ok(_)) => run.status = AiRunStatus::Cancelled,
                    (Ended::Interrupted, Ok(_)) => run.interrupt(),
                    (Ended::Failed(error), Ok(_)) => {
                        run.status = AiRunStatus::Failed;
                        run.failure = Some(failure_of(error));
                    }
                }
                run.stop_requested = false;
            }
        }
        self.redraw_run(id);
    }

    /// The run `id` failed before its provider answered.
    fn fail(&self, id: &str, error: &CoreError) {
        tracing::warn!(code = %error.code, "an AI run failed");
        if let Some(run) = lock(&self.state).runs.get_mut(id) {
            run.status = AiRunStatus::Failed;
            run.failure = Some(failure_of(error));
            run.operation = None;
        }
        self.redraw_run(id);
    }

    fn redraw_run(&self, id: &str) {
        let plugin = lock(&self.state).runs.get(id).map(|run| run.plugin.clone());
        if let Some(plugin) = plugin {
            self.redraw(&plugin);
        }
    }

    fn answer_tools(
        self: &Arc<Self>,
        id: &str,
        results: Vec<AiToolResult>,
        tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError> {
        let engine = self.engine()?;
        check_tools(&tools)?;
        let (chat, provider, calls) = {
            let mut state = lock(&self.state);
            let run = state
                .runs
                .get_mut(id)
                .ok_or_else(|| ServiceError::NotFound(id.to_owned()))?;
            if run.status != AiRunStatus::Completed
                || run.tool_calls.is_empty()
                || !run.calls.is_empty()
            {
                return Err(ServiceError::Failed(
                    "This run isn't waiting for tool results.".to_owned(),
                ));
            }
            let mut calls = Vec::new();
            for call in &run.tool_calls {
                let result = results
                    .iter()
                    .find(|result| result.call_id == call.id)
                    .ok_or_else(|| {
                        ServiceError::Failed(format!("The tool call {} has no result.", call.id))
                    })?;
                if serde_json::from_str::<serde_json::Value>(&result.content).is_err() {
                    return Err(ServiceError::Failed(
                        "A tool result must be JSON.".to_owned(),
                    ));
                }
                let message = run.tool_messages.get(&call.id).cloned();
                calls.push((call.clone(), result.content.clone(), message));
            }
            run.status = AiRunStatus::Streaming;
            run.stop_requested = false;
            (run.chat.clone(), run.provider.clone(), calls)
        };
        let written = calls.iter().try_for_each(|(call, content, message)| {
            write_result(
                &engine,
                id,
                &chat,
                call,
                message.as_deref(),
                content,
                ChatMessageStatus::Completed,
                None,
            )
        });
        match written {
            Ok(()) => self.ask(id, &engine, &provider, tools),
            Err(error) => self.fail(id, &error),
        }
        self.read_run(id)
    }

    /// Hand the model's tool calls of the completed run `id` to Noura: each
    /// waits for the prompt in turn, and the provider answers again once
    /// each has its result.
    fn run_tools(self: &Arc<Self>, id: &str) -> Result<AiRun, ServiceError> {
        let engine = self.engine()?;
        let toolbox = self.toolbox();
        let offered = toolbox
            .as_ref()
            .map(|toolbox| toolbox.tools())
            .unwrap_or_default();
        let names: HashMap<String, String> = offered
            .iter()
            .map(|tool| {
                let name = toolbox
                    .as_ref()
                    .and_then(|toolbox| toolbox.plugin_name(&tool.plugin_id))
                    .unwrap_or_else(|| tool.plugin_id.clone());
                (tool.plugin_id.clone(), name)
            })
            .collect();
        {
            let mut state = lock(&self.state);
            let run = state
                .runs
                .get_mut(id)
                .ok_or_else(|| ServiceError::NotFound(id.to_owned()))?;
            if !run.live() || run.status != AiRunStatus::Completed {
                return Err(ServiceError::Failed(
                    "This run has no tool calls waiting to run.".to_owned(),
                ));
            }
            run.calls = pending_calls(run, &offered, &names);
            run.status = AiRunStatus::WaitingForApproval;
            run.stop_requested = false;
            state.approvals.push_back(id.to_owned());
        }
        self.start_round(id, engine);
        self.changed();
        self.redraw_run(id);
        self.read_run(id)
    }

    /// Run the tool calls of the run `id` on a thread of its own.
    fn start_round(self: &Arc<Self>, id: &str, engine: Arc<WorkspaceEngine>) {
        let ai = self.clone();
        let run = id.to_owned();
        let started = std::thread::Builder::new()
            .name("ai-tools".to_owned())
            .spawn(move || ai.round(&run, &engine));
        if let Err(error) = started {
            tracing::warn!(%error, "an AI run's tool thread didn't start");
            lock(&self.state).approvals.retain(|waiting| waiting != id);
            self.fail(id, &incomplete());
        }
    }

    /// Settle each tool call of the run `id` in turn: wait for your answer,
    /// run the call in its plugin when you allow it, and write its result.
    /// Then ask the provider again, unless the run stopped or a plugin it
    /// relied on went.
    fn round(self: &Arc<Self>, id: &str, engine: &Arc<WorkspaceEngine>) {
        let Some((chat, calls)) = lock(&self.state)
            .runs
            .get(id)
            .map(|run| (run.chat.clone(), run.calls.clone()))
        else {
            return;
        };
        for (index, pending) in calls.iter().enumerate() {
            let settled = match self.decide(id, index, pending) {
                Decision::Run(target) => self.run_call(&target, &pending.call),
                Decision::Deny => Settled::Denied,
                Decision::Unknown => Settled::Failed {
                    code: TOOL_NOT_FOUND,
                    message: "This chat offers no tool by this name.".to_owned(),
                },
                Decision::Disabled => Settled::Disabled,
            };
            let (status, error_code) = settled.status();
            let written = write_result(
                engine,
                id,
                &chat,
                &pending.call,
                pending.message.as_deref(),
                &settled.content(),
                status,
                error_code,
            );
            if let Err(error) = written {
                // The calls left stay in progress, and read as interrupted
                // once the run is over.
                lock(&self.state).approvals.retain(|waiting| waiting != id);
                self.fail(id, &error);
                return;
            }
        }
        let next = {
            let mut state = lock(&self.state);
            state.approvals.retain(|waiting| waiting != id);
            match state.runs.get_mut(id) {
                None => None,
                Some(run) => {
                    run.asking = None;
                    if run.interrupted {
                        run.interrupt();
                        None
                    } else if std::mem::take(&mut run.stop_requested) {
                        run.status = AiRunStatus::Cancelled;
                        None
                    } else {
                        run.status = AiRunStatus::Streaming;
                        Some((run.provider.clone(), run.tools.clone()))
                    }
                }
            }
        };
        match next {
            Some((provider, tools)) => self.ask(id, engine, &provider, tools),
            None => self.redraw_run(id),
        }
        self.changed();
    }

    /// What to do with the tool call at `index` of the run `id`: wait for
    /// your answer to the prompt, unless the run stopped, a plugin it
    /// relied on went, or the workspace closed.
    fn decide(&self, id: &str, index: usize, pending: &PendingCall) -> Decision {
        let mut announced = false;
        loop {
            let mut state = lock(&self.state);
            let Some(run) = state.runs.get_mut(id) else {
                return Decision::Deny;
            };
            if run.interrupted {
                return Decision::Disabled;
            }
            if run.stop_requested {
                return Decision::Deny;
            }
            let Some(target) = &pending.target else {
                return Decision::Unknown;
            };
            match run.calls.get(index).and_then(|call| call.answer) {
                Some(true) => return Decision::Run(target.clone()),
                Some(false) => return Decision::Deny,
                None => {}
            }
            if !announced {
                run.asking = Some(index);
                drop(state);
                announced = true;
                self.changed();
                self.redraw_run(id);
                continue;
            }
            drop(
                self.answered
                    .wait(state)
                    .unwrap_or_else(PoisonError::into_inner),
            );
        }
    }

    /// Run `call` in the plugin `target` names, which you allowed. A
    /// plugin that stopped offering the tool meanwhile runs nothing.
    fn run_call(&self, target: &Target, call: &AiToolCall) -> Settled {
        let Some(toolbox) = self.toolbox() else {
            return Settled::Disabled;
        };
        let offered = toolbox.tools();
        if tools::find(&call.name, &offered).is_none() {
            return Settled::Disabled;
        }
        Settled::of(toolbox.call(&target.plugin, &target.tool, &call.arguments))
    }

    fn read_run(&self, id: &str) -> Result<AiRun, ServiceError> {
        lock(&self.state)
            .runs
            .get(id)
            .map(Run::public)
            .ok_or_else(|| ServiceError::NotFound(id.to_owned()))
    }

    fn stop(&self, id: &str) -> Result<(), ServiceError> {
        let (operation, plugin) = {
            let mut state = lock(&self.state);
            let run = state
                .runs
                .get_mut(id)
                .ok_or_else(|| ServiceError::NotFound(id.to_owned()))?;
            match run.status {
                AiRunStatus::WaitingForConsent => {
                    // Nothing was written or sent yet.
                    run.status = AiRunStatus::Cancelled;
                    run.waiting = None;
                    let plugin = run.plugin.clone();
                    state.waiting.retain(|waiting| waiting != id);
                    (None, Some(plugin))
                }
                AiRunStatus::WaitingForApproval => {
                    // The calls left count as denied, and the run ends.
                    run.stop_requested = true;
                    (None, None)
                }
                AiRunStatus::Streaming if run.operation.is_none() => {
                    // The stream hasn't started yet: it stops as it does.
                    run.stop_requested = true;
                    (None, None)
                }
                _ => (run.operation.clone(), None),
            }
        };
        self.answered.notify_all();
        if let Some(plugin) = plugin {
            self.changed();
            self.redraw(&plugin);
        }
        if let Some(operation) = operation {
            self.store()
                .map_err(|error| ServiceError::Failed(error.message))?
                .cancel_stream(&operation)
                .map_err(service_error)?;
        }
        Ok(())
    }

    fn providers(&self) -> Result<Vec<AiProvider>, ServiceError> {
        let store = self
            .store()
            .map_err(|error| ServiceError::Failed(error.message))?;
        Ok(store
            .list_providers()
            .map_err(service_error)?
            .into_iter()
            .filter(|provider| provider.enabled)
            .map(|provider| AiProvider {
                id: provider.id,
                name: provider.display_name,
                kind: provider.kind,
                model: provider.model,
            })
            .collect())
    }

    fn chats(&self) -> Result<Vec<AiChat>, ServiceError> {
        let engine = self.engine()?;
        let mut seen = std::collections::HashSet::new();
        Ok(engine
            .list_chats()
            .map_err(service_error)?
            .into_iter()
            // Two files with one ID, as after a copy in another app, list
            // as one chat.
            .filter(|chat| seen.insert(chat.id.clone()))
            .map(public_chat)
            .collect())
    }

    /// The chat `id` and its messages. Unless a run of this session works
    /// in the chat, a message a crash or a quit left in progress is
    /// written as interrupted first, or as completed for a tool call whose
    /// result the chat holds.
    fn read_chat(&self, id: &str) -> Result<(AiChat, Vec<AiMessage>), ServiceError> {
        let engine = self.engine()?;
        let live = {
            let state = lock(&self.state);
            if state.new_chats.contains(id) {
                return Ok((new_chat(id), Vec::new()));
            }
            state.runs.values().any(|run| run.chat == id && run.live())
        };
        let read = if live {
            engine.read_chat(id)
        } else {
            engine.recover_interrupted_chat(id).or_else(|error| {
                if matches!(
                    error.code.as_str(),
                    "chat_not_found" | "invalid_chat_id" | "identity_conflict"
                ) {
                    return Err(error);
                }
                // The chat still reads when its messages can't be written.
                tracing::warn!(code = %error.code, "a chat's interrupted messages stayed in progress");
                engine.read_chat(id)
            })
        };
        let ChatRead { chat, messages } = read.map_err(|error| chat_error(error, id))?;
        Ok((
            public_chat(chat),
            messages.into_iter().map(public_message).collect(),
        ))
    }

    /// A new chat, whose files wait for its first message to go out, so
    /// a chat whose message you don't let go out leaves nothing behind.
    fn create_chat(&self) -> Result<AiChat, ServiceError> {
        self.engine()?;
        let id = local_core::new_object_id("chat");
        lock(&self.state).new_chats.insert(id.clone());
        Ok(new_chat(&id))
    }

    fn rename_chat(
        &self,
        id: &str,
        title: &str,
        expected_revision: &str,
    ) -> Result<AiChat, ServiceError> {
        let engine = self.engine()?;
        if lock(&self.state).new_chats.contains(id) {
            return Err(unwritten());
        }
        engine
            .rename_chat(RenameChatInput {
                chat_id: id.to_owned(),
                title: title.to_owned(),
                expected_chat_revision: expected_revision.to_owned(),
            })
            .map(|written| public_chat(written.value))
            .map_err(|error| chat_write_error(error, id))
    }

    fn set_retention(
        &self,
        id: &str,
        retention: AiRetention,
        expected_revision: &str,
    ) -> Result<AiChat, ServiceError> {
        let engine = self.engine()?;
        if lock(&self.state).new_chats.contains(id) {
            return Err(unwritten());
        }
        let (retention, retention_days) = match retention {
            AiRetention::Permanent => (ChatRetention::Permanent, None),
            AiRetention::Ephemeral => (ChatRetention::Ephemeral, Some(EPHEMERAL_DAYS)),
        };
        engine
            .change_chat_retention(ChangeChatRetentionInput {
                chat_id: id.to_owned(),
                retention,
                retention_days,
                expected_chat_revision: expected_revision.to_owned(),
            })
            .map(|written| public_chat(written.value))
            .map_err(|error| chat_write_error(error, id))
    }

    /// The tools of the plugins running in the open workspace, as the model
    /// sees them.
    fn workspace_tools(&self) -> Result<Vec<AiToolDefinition>, ServiceError> {
        self.engine()?;
        let offered = self
            .toolbox()
            .map(|toolbox| toolbox.tools())
            .unwrap_or_default();
        Ok(tools::definitions(&offered))
    }
}

/// The tool calls of `run` for a round, each with the registered tool it
/// names among `offered`, and that tool's plugin named as `names` says. A
/// call names no tool unless the run offered it to the model, whatever
/// else the workspace's plugins register.
fn pending_calls(
    run: &Run,
    offered: &[crate::PluginTool],
    names: &HashMap<String, String>,
) -> Vec<PendingCall> {
    run.tool_calls
        .iter()
        .map(|call| PendingCall {
            call: call.clone(),
            message: run.tool_messages.get(&call.id).cloned(),
            target: run
                .tools
                .iter()
                .any(|tool| tool.name == call.name)
                .then(|| tools::find(&call.name, offered))
                .flatten()
                .map(|tool| Target {
                    plugin: tool.plugin_id.clone(),
                    plugin_name: names
                        .get(&tool.plugin_id)
                        .cloned()
                        .unwrap_or_else(|| tool.plugin_id.clone()),
                    tool: tool.name.clone(),
                    description: tool.description.clone(),
                }),
            answer: None,
        })
        .collect()
}

/// What happens to a tool call that waited for you.
enum Decision {
    /// You allowed it: run it in this plugin.
    Run(Target),
    /// You denied it or closed the prompt, the run stopped, or the
    /// workspace closed.
    Deny,
    /// The run didn't offer its tool, or no plugin on in the workspace
    /// offers it.
    Unknown,
    /// A plugin the run relied on was turned off.
    Disabled,
}

/// Write the result `content` of the tool call `call` of the run `run` to
/// `chat`, then finish the call's message, `message`, with `status`.
#[expect(
    clippy::too_many_arguments,
    reason = "a tool result names the run, the chat, the call and its message, and how it ended"
)]
fn write_result(
    engine: &WorkspaceEngine,
    run: &str,
    chat: &str,
    call: &AiToolCall,
    message: Option<&str>,
    content: &str,
    status: ChatMessageStatus,
    error_code: Option<String>,
) -> Result<(), CoreError> {
    let read = engine.read_chat(chat)?;
    engine.append_chat_tool_result(AppendChatToolResultInput {
        chat_id: chat.to_owned(),
        run_id: run.to_owned(),
        tool_call_id: call.id.clone(),
        tool_name: call.name.clone(),
        content: content.to_owned(),
        expected_chat_revision: read.chat.revision,
    })?;
    let Some(message) = message else {
        return Ok(());
    };
    let read = engine.read_chat(chat)?;
    if let Some(current) = read.messages.iter().find(|known| known.id == message)
        && current.status == ChatMessageStatus::InProgress
    {
        engine.finish_chat_tool_call(FinishChatToolCallInput {
            chat_id: chat.to_owned(),
            message_id: message.to_owned(),
            content: call.arguments.clone(),
            status,
            error_code,
            expected_chat_revision: read.chat.revision,
            expected_message_revision: current.revision.clone(),
        })?;
    }
    Ok(())
}

/// The chat `id` that `create-chat` started, before its first file.
fn new_chat(id: &str) -> AiChat {
    AiChat {
        id: id.to_owned(),
        title: NEW_CHAT_TITLE.to_owned(),
        updated: local_core::now_rfc3339(),
        revision: String::new(),
        retention: AiRetention::Permanent,
    }
}

fn public_chat(chat: Chat) -> AiChat {
    AiChat {
        id: chat.id,
        title: chat.title,
        updated: chat.updated,
        revision: chat.revision,
        retention: match chat.retention {
            ChatRetention::Permanent => AiRetention::Permanent,
            ChatRetention::Ephemeral => AiRetention::Ephemeral,
        },
    }
}

/// An answer as it streams in, and the message it goes to.
struct Answer {
    chat: String,
    message: String,
    /// The message's revision as Noura began it.
    begun: String,
    /// The provider and model that write it.
    provider: String,
    model: String,
    text: String,
    calls: Vec<AiToolCall>,
}

/// How a provider's answer ended.
enum Ended {
    Completed,
    Cancelled,
    /// A plugin the run relied on was turned off.
    Interrupted,
    Failed(Box<CoreError>),
}

/// What one plugin reaches through the host AI interface.
struct Access {
    ai: Arc<Ai>,
    plugin: String,
}

impl AiChats for Access {
    fn providers(&self) -> Result<Vec<AiProvider>, ServiceError> {
        self.ai.providers()
    }

    fn chats(&self) -> Result<Vec<AiChat>, ServiceError> {
        self.ai.chats()
    }

    fn read_chat(&self, id: &str) -> Result<(AiChat, Vec<AiMessage>), ServiceError> {
        self.ai.read_chat(id)
    }

    fn create_chat(&self) -> Result<AiChat, ServiceError> {
        self.ai.create_chat()
    }

    fn rename_chat(
        &self,
        id: &str,
        title: &str,
        expected_revision: &str,
    ) -> Result<AiChat, ServiceError> {
        self.ai.rename_chat(id, title, expected_revision)
    }

    fn set_retention(
        &self,
        id: &str,
        retention: AiRetention,
        expected_revision: &str,
    ) -> Result<AiChat, ServiceError> {
        self.ai.set_retention(id, retention, expected_revision)
    }

    fn workspace_tools(&self) -> Result<Vec<AiToolDefinition>, ServiceError> {
        self.ai.workspace_tools()
    }

    fn send(&self, request: AiRequest) -> Result<AiRun, ServiceError> {
        self.ai.send(&self.plugin, request)
    }

    fn answer_tools(
        &self,
        run: &str,
        results: Vec<AiToolResult>,
        tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError> {
        self.own(run)?;
        self.ai.answer_tools(run, results, tools)
    }

    fn run_tools(&self, run: &str) -> Result<AiRun, ServiceError> {
        self.own(run)?;
        self.ai.run_tools(run)
    }

    fn resume(
        &self,
        chat: &str,
        provider: &str,
        tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError> {
        self.ai.resume(&self.plugin, chat, provider, tools)
    }

    fn read_run(&self, id: &str) -> Result<AiRun, ServiceError> {
        self.own(id)?;
        self.ai.read_run(id)
    }

    fn stop(&self, id: &str) -> Result<(), ServiceError> {
        self.own(id)?;
        self.ai.stop(id)
    }

    fn web_ready(&self) -> Result<bool, ServiceError> {
        Ok(self.ai.web_ready())
    }

    fn web_search(
        &self,
        query: &str,
        max_results: Option<u32>,
    ) -> Result<Vec<plugin_host::AiWebResult>, AiFailure> {
        self.ai.web_search(&self.plugin, query, max_results)
    }

    fn web_fetch(&self, url: &str) -> Result<plugin_host::AiWebPage, AiFailure> {
        self.ai.web_fetch(&self.plugin, url)
    }
}

impl Access {
    /// Refuse the run `id` unless this plugin sent it: another plugin's
    /// run is not found, though its ID is in the chat's files.
    fn own(&self, id: &str) -> Result<(), ServiceError> {
        let state = lock(&self.ai.state);
        match state.runs.get(id) {
            Some(run) if run.plugin == self.plugin => Ok(()),
            _ => Err(ServiceError::NotFound(id.to_owned())),
        }
    }
}

/// Whether you consented to the provider `provider` receiving the
/// workspace `workspace`'s content under the current policy, for the
/// provider's kind, endpoint and model as they are now.
fn consented(store: &AiFoundation, workspace: &str, provider: &str) -> bool {
    store
        .read_consent(
            workspace,
            AiConsentReadInput {
                provider_id: provider.to_owned(),
                policy_version: AI_POLICY_VERSION.to_owned(),
            },
        )
        .ok()
        .flatten()
        .is_some()
}

/// Finish the assistant message `answer` began with its text, `status`
/// and `error_code`, and return the ID of the message that holds the
/// answer. A change to the chat made elsewhere meanwhile stays: the write
/// reads the chat again and tries once more. A change to the message
/// itself, as when another app edited or finished it, stays too, and the
/// answer goes to a new message after it.
fn finish_answer(
    engine: &WorkspaceEngine,
    run: &str,
    answer: &Answer,
    status: ChatMessageStatus,
    error_code: Option<&str>,
) -> Result<String, CoreError> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let read = engine.read_chat(&answer.chat)?;
        // The index may not list the message yet, as after it failed to
        // record it: the file then still holds what Noura wrote, or the
        // write finds that it doesn't.
        let changed = read
            .messages
            .iter()
            .any(|known| known.id == answer.message && known.revision != answer.begun);
        if changed {
            return answer_again(engine, run, answer, &read.chat.revision, status, error_code);
        }
        let finished = engine.finish_chat_assistant(FinishChatAssistantInput {
            chat_id: answer.chat.clone(),
            message_id: answer.message.clone(),
            content: answer.text.clone(),
            status: status.clone(),
            error_code: error_code.map(str::to_owned),
            expected_chat_revision: read.chat.revision.clone(),
            expected_message_revision: answer.begun.clone(),
        });
        match finished {
            Ok(_) => return Ok(answer.message.clone()),
            // The chat changed, or the message did after the read.
            Err(error)
                if matches!(
                    error.code.as_str(),
                    "revision_conflict" | "chat_message_not_in_progress"
                ) && attempts < 3 => {}
            Err(error) => return Err(error),
        }
    }
}

/// Write `answer` as a new assistant message of its chat, since the one
/// Noura began changed elsewhere. Returns the new message's ID.
fn answer_again(
    engine: &WorkspaceEngine,
    run: &str,
    answer: &Answer,
    chat_revision: &str,
    status: ChatMessageStatus,
    error_code: Option<&str>,
) -> Result<String, CoreError> {
    tracing::warn!("an answer's message changed elsewhere, so the answer went to a new message");
    let begun = engine.begin_chat_assistant(BeginChatAssistantInput {
        chat_id: answer.chat.clone(),
        run_id: run.to_owned(),
        provider_id: answer.provider.clone(),
        model_id: answer.model.clone(),
        expected_chat_revision: chat_revision.to_owned(),
    })?;
    engine.finish_chat_assistant(FinishChatAssistantInput {
        chat_id: answer.chat.clone(),
        message_id: begun.value.id.clone(),
        content: answer.text.clone(),
        status,
        error_code: error_code.map(str::to_owned),
        expected_chat_revision: begun.chat_revision.clone().unwrap_or_default(),
        expected_message_revision: begun.revision.clone(),
    })?;
    Ok(begun.value.id)
}

/// A chat's title from the first line of its first message.
fn chat_title(message: &str) -> String {
    let line = message.trim().lines().next().unwrap_or_default();
    let title: String = line
        .chars()
        .filter(|character| !character.is_control())
        .take(TITLE_CHARS)
        .collect();
    let title = title.trim();
    if title.is_empty() {
        NEW_CHAT_TITLE.to_owned()
    } else {
        title.to_owned()
    }
}

/// A new provider's ID, from its name, unused among `existing`.
fn new_provider_id(name: &str, existing: &[AiProviderConfig]) -> String {
    let base: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect::<String>()
        .split('-')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("-");
    let base = if base.is_empty() {
        "provider".to_owned()
    } else {
        base.chars().take(48).collect()
    };
    let taken = |id: &str| existing.iter().any(|provider| provider.id == id);
    if !taken(&base) {
        return base;
    }
    (2..)
        .map(|number| format!("{base}-{number}"))
        .find(|id| !taken(id))
        .unwrap_or(base)
}

/// The tools a plugin offers with a request, checked: unique names and a
/// JSON Schema for each.
fn check_tools(tools: &[AiToolDefinition]) -> Result<(), ServiceError> {
    let mut names = std::collections::HashSet::new();
    for tool in tools {
        let valid_name = !tool.name.is_empty()
            && tool.name.len() <= 64
            && tool.name.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '-' | '_')
            });
        if !valid_name || !names.insert(tool.name.as_str()) {
            return Err(ServiceError::Failed(format!(
                "The tool name \u{201c}{}\u{201d} isn't valid or repeats.",
                tool.name
            )));
        }
        match serde_json::from_str::<serde_json::Value>(&tool.input_schema) {
            Ok(schema) if schema.is_object() || schema.is_boolean() => {}
            _ => {
                return Err(ServiceError::Failed(format!(
                    "The input schema of the tool {} isn't a JSON Schema.",
                    tool.name
                )));
            }
        }
    }
    Ok(())
}

fn public_message(message: ChatMessage) -> AiMessage {
    AiMessage {
        id: message.id,
        kind: match message.kind {
            ChatMessageKind::User => AiMessageKind::User,
            ChatMessageKind::Assistant => AiMessageKind::Assistant,
            ChatMessageKind::ToolCall => AiMessageKind::ToolCall,
            ChatMessageKind::ToolResult => AiMessageKind::ToolResult,
            ChatMessageKind::ContextSummary => AiMessageKind::ContextSummary,
        },
        status: match message.status {
            ChatMessageStatus::InProgress => AiMessageStatus::InProgress,
            ChatMessageStatus::Completed => AiMessageStatus::Completed,
            ChatMessageStatus::Interrupted => AiMessageStatus::Interrupted,
            ChatMessageStatus::Cancelled => AiMessageStatus::Cancelled,
            ChatMessageStatus::Failed => AiMessageStatus::Failed,
        },
        content: message.content,
        provider: message.provider_id,
        model: message.model_id,
        error_code: message.error_code,
        created: message.created,
        tool_name: message.tool_name,
    }
}

/// A failure as a plugin sees it: a code, and a message written for
/// people. Neither ever holds a provider's own words, headers or a key.
fn failure_of(error: &CoreError) -> AiFailure {
    let message = match error.code.as_str() {
        "ai_consent_required" => {
            "Noura needs your consent before this provider receives the workspace's content."
        }
        "provider_not_found" => "The provider isn't set up or turned on anymore.",
        "credential_store_error" => {
            "Noura couldn't read the provider's key from this computer's credential store."
        }
        "endpoint_insecure" => {
            "Use an HTTPS address. Plain HTTP works only for a server on this computer."
        }
        "revision_conflict" => "The chat changed elsewhere, so Noura didn't save the answer.",
        "provider_stream_incomplete" => {
            "The provider stopped before it finished answering. Try again."
        }
        PLUGIN_DISABLED => "You turned off a plugin this answer relied on.",
        _ => match error.category {
            local_core::ErrorCategory::Provider | local_core::ErrorCategory::Transient => {
                "The provider couldn't answer. Try again."
            }
            local_core::ErrorCategory::Filesystem | local_core::ErrorCategory::Permission => {
                "Noura couldn't write to this chat's files."
            }
            _ => "Noura couldn't finish this answer.",
        },
    };
    AiFailure {
        code: error_code(error),
        message: message.to_owned(),
    }
}

/// The code of `error` as a chat message records it: ASCII letters,
/// digits, dots, hyphens and underscores, up to 128 bytes.
fn error_code(error: &CoreError) -> String {
    let code = error.code.as_str();
    let valid = !code.is_empty()
        && code.len() <= 128
        && code
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'));
    if valid {
        code.to_owned()
    } else {
        "ai_request_failed".to_owned()
    }
}

/// An error from a chat read, for the plugin: a chat Noura can't find is
/// `NotFound`.
fn chat_error(error: CoreError, id: &str) -> ServiceError {
    match error.code.as_str() {
        "chat_not_found" | "invalid_chat_id" => ServiceError::NotFound(id.to_owned()),
        "identity_conflict" => ServiceError::IdentityConflict(id.to_owned()),
        _ => service_error(error),
    }
}

/// An error from a write to the chat `id`'s own file, for the plugin: the
/// file no longer holding the revision the write carried is a
/// `RevisionConflict`, and nothing was written.
fn chat_write_error(error: CoreError, id: &str) -> ServiceError {
    match error.code.as_str() {
        "revision_conflict" => ServiceError::RevisionConflict(id.to_owned()),
        "chat_title_required" => ServiceError::Failed("Give the chat a name.".to_owned()),
        _ => chat_error(error, id),
    }
}

fn service_error(error: CoreError) -> ServiceError {
    tracing::warn!(code = %error.code, operation = %error.operation, "an AI call failed");
    match error.code.as_str() {
        "revision_conflict" => ServiceError::Failed(
            "The chat changed elsewhere. Showing the latest version.".to_owned(),
        ),
        "file_not_downloaded" => {
            ServiceError::Failed("This chat isn't downloaded yet. Try again once it is.".to_owned())
        }
        _ => ServiceError::Failed(error.message),
    }
}

fn settings_error(error: CoreError) -> AppError {
    tracing::warn!(code = %error.code, "an AI setting wasn't saved");
    let message = match error.code.as_str() {
        "provider_invalid" => "Give the provider a model.".to_owned(),
        "provider_kind_invalid" => "Choose one of the kinds Noura offers.".to_owned(),
        "credential_store_error" => {
            "Noura couldn't reach this computer's credential store for the key.".to_owned()
        }
        "credential_empty" => "Enter the key.".to_owned(),
        "provider_not_found" => "Turn the provider on first.".to_owned(),
        _ => error.message,
    };
    AppError::new(&error.code, message)
}

fn no_workspace() -> AppError {
    AppError::new("no_workspace", "Open a workspace first.")
}

fn unwritten() -> ServiceError {
    ServiceError::Failed(
        "This chat isn't saved until its first message goes out, so it can't change yet."
            .to_owned(),
    )
}

fn no_workspace_core() -> CoreError {
    CoreError::new(
        "workspace_not_open",
        local_core::ErrorCategory::Validation,
        "No workspace is open",
        "ai_run",
    )
}

fn provider_missing() -> CoreError {
    CoreError::validation(
        "provider_not_found",
        "The enabled AI provider does not exist",
        "ai_run",
    )
}

fn plugin_disabled() -> CoreError {
    CoreError::validation(
        PLUGIN_DISABLED,
        "A plugin the AI run relied on was turned off",
        "ai_run",
    )
}

/// The code of a run that went before its request left, as after a switch
/// to another workspace.
const CANCELLED: &str = "ai_run_cancelled";

fn cancelled() -> CoreError {
    CoreError::new(
        CANCELLED,
        local_core::ErrorCategory::Transient,
        "The AI run ended before its request left",
        "ai_run",
    )
}

fn incomplete() -> CoreError {
    CoreError::new(
        "provider_stream_incomplete",
        local_core::ErrorCategory::Transient,
        "The AI provider stream ended without a terminal response",
        "ai_run",
    )
}

fn uuid_v4() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The kinds of provider Noura offers, each with the name Settings shows.
pub(crate) fn provider_kinds() -> &'static [(&'static str, &'static str)] {
    &AI_PROVIDER_KINDS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_first_message_names_its_chat_from_its_first_line() {
        assert_eq!(
            chat_title("  Plan the garden\nwith tomatoes"),
            "Plan the garden"
        );
        assert_eq!(chat_title("\n\n"), NEW_CHAT_TITLE);
        assert_eq!(chat_title(&"a".repeat(100)).chars().count(), TITLE_CHARS);
        assert_eq!(chat_title("Tabs\tgo"), "Tabsgo");
    }

    /// A run of `plugin` with `status`, in a service with no workspace.
    fn service_with(id: &str, plugin: &str, status: AiRunStatus) -> (Arc<Ai>, tempfile::TempDir) {
        let app_data = tempfile::tempdir().unwrap();
        let ai = Ai::new(app_data.path(), Arc::new(crate::NoSystem));
        let mut run = Run::new(
            id,
            plugin,
            "chat_01jbcdefghjkmnpqrstvwxyz0a",
            "local",
            Ask::Resume,
            Vec::new(),
        );
        run.status = status;
        run.waiting = None;
        lock(&ai.state).runs.insert(id.to_owned(), run);
        (ai, app_data)
    }

    /// Stop pressed after a run shows as streaming but before its stream
    /// started, as when you allow a provider and press Stop at once, stops
    /// the stream as soon as it starts.
    #[test]
    fn stop_before_the_stream_starts_stops_it_once_it_does() {
        let (ai, _app_data) = service_with("run_1", "ai", AiRunStatus::Streaming);

        ai.access("ai").stop("run_1").unwrap();

        assert!(ai.streaming("run_1", "operation_1"), "the stream stops");
        assert_eq!(
            lock(&ai.state).runs["run_1"].operation.as_deref(),
            Some("operation_1")
        );
        // A stop that came once stops one stream.
        assert!(!ai.streaming("run_1", "operation_2"));
    }

    #[test]
    fn a_plugin_reaches_only_its_own_runs() {
        let (ai, _app_data) = service_with("run_1", "ai", AiRunStatus::Streaming);
        let other = ai.access("other");

        assert_eq!(
            other.read_run("run_1"),
            Err(ServiceError::NotFound("run_1".to_owned()))
        );
        assert_eq!(
            other.stop("run_1"),
            Err(ServiceError::NotFound("run_1".to_owned()))
        );
        assert_eq!(
            other.answer_tools("run_1", Vec::new(), Vec::new()),
            Err(ServiceError::NotFound("run_1".to_owned()))
        );
        assert_eq!(
            other.run_tools("run_1"),
            Err(ServiceError::NotFound("run_1".to_owned()))
        );
        assert!(!lock(&ai.state).runs["run_1"].stop_requested);
        assert_eq!(ai.access("ai").read_run("run_1").unwrap().id, "run_1");
    }

    #[test]
    fn new_provider_ids_come_from_names_and_never_repeat() {
        let existing = |id: &str| AiProviderConfig {
            id: id.into(),
            kind: "openai".into(),
            display_name: id.into(),
            model: "m".into(),
            endpoint: None,
            credential_ref: None,
            enabled: true,
            context_window: None,
        };
        assert_eq!(new_provider_id("My OpenAI", &[]), "my-openai");
        assert_eq!(
            new_provider_id("My OpenAI", &[existing("my-openai")]),
            "my-openai-2"
        );
        assert_eq!(new_provider_id("✨", &[]), "provider");
    }
}
