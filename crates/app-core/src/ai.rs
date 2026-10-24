//! The AI service: the providers you set up on this device, their keys in
//! the operating system's credential store, your consent per workspace
//! and provider, and the runs that send a chat to a provider and stream
//! its answer into the chat's files.
//!
//! Plugins that hold the `ai` capability reach it through the host AI
//! interface, and Settings and the consent prompt through [`App`]. Every
//! window shares one service. Provider settings and consents live in this
//! device's app data, never in the workspace, and nothing the service
//! hands a plugin holds a key.
//!
//! A run writes your message and an in-progress answer to the chat before
//! the request leaves the computer. The answer streams in on a thread of
//! its own, outside any plugin call, so an answer may take longer than a
//! plugin call's deadline; the plugin draws again as text arrives. The
//! answer and its status reach the chat's files before the run shows as
//! finished.
//!
//! [`App`]: crate::App

#![allow(
    clippy::result_large_err,
    reason = "the run steps pass local-core's structured CoreError along, as local-core does"
)]

mod history;

use std::{
    collections::{HashMap, VecDeque},
    path::Path,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use local_core::{
    AI_PROVIDER_KINDS, AiConsentDataCategory, AiConsentGrantInput, AiConsentReadInput,
    AiConsentRevokeInput, AiFoundation, AiModelRef, AiProviderConfig, AiStreamEvent, AiStreamFrame,
    AiStreamInput, AiStreamer, AppendChatToolResultInput, AppendChatUserMessageInput,
    BeginChatAssistantInput, BeginChatToolCallInput, ChatMessage, ChatMessageKind,
    ChatMessageStatus, ChatRead, CoreError, CreateChatInput, CredentialStore,
    FinishChatAssistantInput, FinishChatToolCallInput, GenAiStreamer, RenameChatInput,
    WorkspaceEngine,
};
use plugin_host::{
    AiChat, AiChats, AiFailure, AiMessage, AiMessageKind, AiMessageStatus, AiProvider, AiRequest,
    AiRun, AiRunStatus, AiToolCall, AiToolDefinition, AiToolResult, ServiceError,
};

use crate::{AppError, Workspace};

/// The version of what Noura tells you before a provider receives your
/// workspace's content. Consent holds for one version, so a new one asks
/// again.
pub const AI_POLICY_VERSION: &str = "2026-09";

/// The title a chat has until its first message names it.
pub const NEW_CHAT_TITLE: &str = "New chat";

/// The longest title a chat's first message gives it, in characters.
const TITLE_CHARS: usize = 64;

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
    /// While it waits for consent: the message to write and the tools to
    /// offer.
    waiting: Option<(String, Vec<AiToolDefinition>)>,
    /// The stream's operation ID while it streams.
    operation: Option<String>,
}

impl Run {
    fn public(&self) -> AiRun {
        AiRun {
            id: self.id.clone(),
            chat: self.chat.clone(),
            provider: self.provider.clone(),
            status: self.status,
            text: self.text.clone(),
            message: self.message.clone(),
            tool_calls: self.tool_calls.clone(),
            failure: self.failure.clone(),
        }
    }

    fn active(&self) -> bool {
        matches!(
            self.status,
            AiRunStatus::Streaming | AiRunStatus::WaitingForConsent
        )
    }
}

#[derive(Default)]
struct State {
    workspace: Option<Arc<Workspace>>,
    runs: HashMap<String, Run>,
    /// The runs waiting for your consent, oldest first. The prompt asks
    /// about the first one's provider.
    waiting: VecDeque<String>,
}

pub(crate) struct Ai {
    /// The provider settings and consents, or why they can't be read.
    store: Result<AiFoundation, AppError>,
    /// Drives the provider requests.
    runtime: tokio::runtime::Runtime,
    /// Talks to the providers: `genai`, or a fake in tests.
    client: Mutex<Arc<dyn AiStreamer>>,
    state: Mutex<State>,
    /// The plugins to draw again, as text arrived for their runs.
    redraws: Mutex<Vec<String>>,
    wake: Mutex<Option<Wake>>,
    /// Counts the changes windows show: the prompt, providers and consents.
    version: AtomicU64,
    /// The providers or consents changed since the plugins drew, so every
    /// plugin that holds `ai` draws again.
    settings_changed: std::sync::atomic::AtomicBool,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Ai {
    /// The service, with provider settings and consents in `app_data` and
    /// keys in `credentials`.
    pub(crate) fn new(app_data: &Path, credentials: Arc<dyn CredentialStore>) -> Arc<Self> {
        let store = AiFoundation::open_with(
            app_data.join(PROVIDERS_FILE),
            app_data.join(CONSENTS_FILE),
            credentials,
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
            runtime,
            client: Mutex::new(Arc::new(GenAiStreamer)),
            state: Mutex::new(State::default()),
            redraws: Mutex::new(Vec::new()),
            wake: Mutex::new(None),
            version: AtomicU64::new(0),
            settings_changed: std::sync::atomic::AtomicBool::new(false),
        })
    }

    pub(crate) fn set_wake(&self, wake: Wake) {
        *lock(&self.wake) = Some(wake);
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

    /// Why the provider settings can't be read, if they can't.
    pub(crate) fn problem(&self) -> Option<String> {
        self.store.as_ref().err().map(|error| error.message.clone())
    }

    /// Chat in `workspace` from now on. Runs in the workspace before stop.
    pub(crate) fn open_workspace(&self, workspace: Option<Arc<Workspace>>) {
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
        state.workspace = workspace;
        drop(state);
        if let Ok(store) = self.store() {
            for operation in operations {
                let _ = store.cancel_stream(&operation);
            }
        }
        self.changed();
    }

    fn workspace(&self) -> Option<Arc<Workspace>> {
        lock(&self.state).workspace.clone()
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
        let provider = AiProviderConfig {
            id: id.clone(),
            kind: input.kind.trim().to_owned(),
            display_name: if name.is_empty() { id.clone() } else { name },
            model: input.model.trim().to_owned(),
            endpoint,
            credential_ref: current.and_then(|current| current.credential_ref.clone()),
            enabled: input.enabled,
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

    /// Remove a provider and its key. Its running requests stop.
    pub(crate) fn remove_provider(&self, id: &str) -> Result<(), AppError> {
        self.store()?.remove_provider(id).map_err(settings_error)?;
        self.changed();
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
        let run = state.runs.get(state.waiting.front()?)?;
        let workspace = state.workspace.as_ref()?;
        let provider = self
            .store()
            .ok()?
            .list_providers()
            .ok()?
            .into_iter()
            .find(|provider| provider.id == run.provider)?;
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
        let workspace = self
            .workspace()
            .ok_or_else(|| ServiceError::Failed("No workspace is open.".to_owned()))?;
        let provider = self.provider(&request.provider)?;
        if request.message.trim().is_empty() {
            return Err(ServiceError::Failed("The message is empty.".to_owned()));
        }
        check_tools(&request.tools)?;
        engine
            .read_chat(&request.chat)
            .map_err(|error| chat_error(error, &request.chat))?;
        let id = format!("run_{}", uuid_v4());
        let run = {
            let mut state = lock(&self.state);
            if state
                .runs
                .values()
                .any(|run| run.chat == request.chat && run.active())
            {
                return Err(ServiceError::Failed(
                    "This chat is still answering your last message.".to_owned(),
                ));
            }
            let run = Run {
                id: id.clone(),
                plugin: plugin.to_owned(),
                chat: request.chat.clone(),
                provider: provider.id.clone(),
                status: AiRunStatus::WaitingForConsent,
                text: String::new(),
                message: None,
                message_revision: String::new(),
                written: Vec::new(),
                tool_calls: Vec::new(),
                tool_messages: HashMap::new(),
                failure: None,
                waiting: Some((request.message, request.tools)),
                operation: None,
            };
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

    /// Write the message and the in-progress answer of the run `id`, then
    /// ask its provider.
    fn start(self: &Arc<Self>, id: &str) {
        let (chat, provider_id, message, tools) = {
            let mut state = lock(&self.state);
            let Some(run) = state.runs.get_mut(id) else {
                return;
            };
            let Some((message, tools)) = run.waiting.take() else {
                return;
            };
            run.status = AiRunStatus::Streaming;
            (run.chat.clone(), run.provider.clone(), message, tools)
        };
        let written = (|| {
            let engine = self.engine().map_err(|_| no_workspace_core())?;
            let read = engine.read_chat(&chat)?;
            let names = read.chat.title == NEW_CHAT_TITLE
                && !read
                    .messages
                    .iter()
                    .any(|message| message.kind == ChatMessageKind::User);
            let appended = engine.append_chat_user_message(AppendChatUserMessageInput {
                chat_id: chat.clone(),
                run_id: id.to_owned(),
                content: message.clone(),
                expected_chat_revision: read.chat.revision,
            })?;
            if let Some(run) = lock(&self.state).runs.get_mut(id) {
                run.written.push(appended.value.clone());
            }
            if names {
                let revision = appended.chat_revision.clone().unwrap_or_default();
                // The title waits for the next message when the chat
                // changed elsewhere meanwhile: a change made there wins.
                if let Err(error) = engine.rename_chat(RenameChatInput {
                    chat_id: chat.clone(),
                    title: chat_title(&message),
                    expected_chat_revision: revision,
                }) {
                    tracing::warn!(code = %error.code, "a chat kept its title");
                }
            }
            Ok::<_, CoreError>(engine)
        })();
        match written {
            Ok(engine) => self.ask(id, &engine, &provider_id, tools),
            Err(error) => self.fail(id, &error),
        }
    }

    /// Begin the run's next answer and ask its provider, with the chat's
    /// messages so far.
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
        let asked = (|| {
            let provider = self.provider(provider_id).map_err(|_| provider_missing())?;
            let read = engine.read_chat(&chat)?;
            let begun = engine.begin_chat_assistant(BeginChatAssistantInput {
                chat_id: chat.clone(),
                run_id: id.to_owned(),
                provider_id: provider.id.clone(),
                model_id: provider.model.clone(),
                expected_chat_revision: read.chat.revision,
            })?;
            let message = begun.value.id;
            let written = {
                let mut state = lock(&self.state);
                match state.runs.get_mut(id) {
                    Some(run) => {
                        run.message = Some(message.clone());
                        run.message_revision = begun.revision.clone();
                        run.text.clear();
                        run.tool_calls.clear();
                        run.tool_messages.clear();
                        run.written.clone()
                    }
                    None => Vec::new(),
                }
            };
            let mut read = engine.read_chat(&chat)?;
            for mine in written {
                if !read.messages.iter().any(|known| known.id == mine.id) {
                    read.messages.push(mine);
                }
            }
            let workspace = self.workspace().ok_or_else(no_workspace_core)?;
            let operation = uuid_v4();
            let input = AiStreamInput {
                operation_id: operation.clone(),
                model: AiModelRef {
                    provider_id: provider.id.clone(),
                    model: provider.model.clone(),
                },
                messages: history::transport(&read.messages, &message),
                tools: Some(history::tools(&tools)),
                policy_version: Some(AI_POLICY_VERSION.to_owned()),
            };
            let client = lock(&self.client).clone();
            let store = self.store().map_err(|_| no_workspace_core())?;
            let stream = {
                let _entered = self.runtime.enter();
                store.start_stream_with(workspace.id(), input, client)
            };
            match stream {
                Ok(stream) => Ok((message, operation, stream)),
                Err(error) => {
                    let code = error_code(&error);
                    finish_answer(
                        engine,
                        &chat,
                        (&message, &begun.revision),
                        "",
                        ChatMessageStatus::Failed,
                        Some(&code),
                    )?;
                    Err(error)
                }
            }
        })();
        match asked {
            Ok((_, operation, stream)) => {
                if let Some(run) = lock(&self.state).runs.get_mut(id) {
                    run.operation = Some(operation);
                }
                let ai = self.clone();
                let run = id.to_owned();
                let engine = engine.clone();
                let started = std::thread::Builder::new()
                    .name("ai-run".to_owned())
                    .spawn(move || ai.follow(&run, &engine, stream.receiver));
                if let Err(error) = started {
                    tracing::warn!(%error, "an AI run's thread didn't start");
                }
            }
            Err(error) => self.fail(id, &error),
        }
        self.redraw_run(id);
    }

    /// Take the run's stream until it ends, then write the answer and its
    /// status.
    fn follow(
        self: &Arc<Self>,
        id: &str,
        engine: &Arc<WorkspaceEngine>,
        mut frames: tokio::sync::mpsc::Receiver<AiStreamFrame>,
    ) {
        let ended = loop {
            let Some(frame) = frames.blocking_recv() else {
                break Ended::Failed(incomplete());
            };
            match frame.event {
                AiStreamEvent::Started { .. } => {}
                AiStreamEvent::TextDelta { text } => {
                    if let Some(run) = lock(&self.state).runs.get_mut(id) {
                        run.text.push_str(&text);
                    }
                    self.redraw_run(id);
                }
                AiStreamEvent::ToolCall { call } => {
                    if let Some(run) = lock(&self.state).runs.get_mut(id) {
                        run.tool_calls.push(AiToolCall {
                            id: call.call_id,
                            name: call.name,
                            arguments: call.arguments.to_string(),
                        });
                    }
                }
                AiStreamEvent::Completed { .. } => break Ended::Completed,
                AiStreamEvent::Cancelled => break Ended::Cancelled,
                AiStreamEvent::Error { error } => break Ended::Failed(error),
            }
        };
        self.end(id, engine, ended);
    }

    /// Write the run's answer with how it ended, and the tool calls it
    /// asked for, then show the run as ended.
    fn end(self: &Arc<Self>, id: &str, engine: &Arc<WorkspaceEngine>, ended: Ended) {
        let Some((chat, message, begun, text, calls)) =
            lock(&self.state).runs.get(id).and_then(|run| {
                Some((
                    run.chat.clone(),
                    run.message.clone()?,
                    run.message_revision.clone(),
                    run.text.clone(),
                    run.tool_calls.clone(),
                ))
            })
        else {
            return;
        };
        let (status, error) = match &ended {
            Ended::Completed => (ChatMessageStatus::Completed, None),
            Ended::Cancelled => (ChatMessageStatus::Cancelled, None),
            Ended::Failed(error) => (ChatMessageStatus::Failed, Some(error_code(error))),
        };
        let written = finish_answer(
            engine,
            &chat,
            (&message, &begun),
            &text,
            status,
            error.as_deref(),
        )
        .and_then(|()| {
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
                    (Ended::Failed(error), Ok(_)) => {
                        run.status = AiRunStatus::Failed;
                        run.failure = Some(failure_of(error));
                    }
                }
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
            if run.status != AiRunStatus::Completed || run.tool_calls.is_empty() {
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
            (run.chat.clone(), run.provider.clone(), calls)
        };
        let written = (|| {
            for (call, content, message) in &calls {
                let read = engine.read_chat(&chat)?;
                engine.append_chat_tool_result(AppendChatToolResultInput {
                    chat_id: chat.clone(),
                    run_id: id.to_owned(),
                    tool_call_id: call.id.clone(),
                    tool_name: call.name.clone(),
                    content: content.clone(),
                    expected_chat_revision: read.chat.revision,
                })?;
                if let Some(message) = message {
                    let read = engine.read_chat(&chat)?;
                    if let Some(current) = read.messages.iter().find(|known| &known.id == message)
                        && current.status == ChatMessageStatus::InProgress
                    {
                        engine.finish_chat_tool_call(FinishChatToolCallInput {
                            chat_id: chat.clone(),
                            message_id: message.clone(),
                            content: call.arguments.clone(),
                            status: ChatMessageStatus::Completed,
                            error_code: None,
                            expected_chat_revision: read.chat.revision,
                            expected_message_revision: current.revision.clone(),
                        })?;
                    }
                }
            }
            Ok::<(), CoreError>(())
        })();
        match written {
            Ok(()) => self.ask(id, &engine, &provider, tools),
            Err(error) => self.fail(id, &error),
        }
        self.read_run(id)
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
            if run.status == AiRunStatus::WaitingForConsent {
                // Nothing was written or sent yet.
                run.status = AiRunStatus::Cancelled;
                run.waiting = None;
                let plugin = run.plugin.clone();
                state.waiting.retain(|waiting| waiting != id);
                (None, Some(plugin))
            } else {
                (run.operation.clone(), None)
            }
        };
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
            .map(|chat| AiChat {
                id: chat.id,
                title: chat.title,
                updated: chat.updated,
            })
            .collect())
    }

    fn read_chat(&self, id: &str) -> Result<(AiChat, Vec<AiMessage>), ServiceError> {
        let engine = self.engine()?;
        let ChatRead { chat, messages } = engine
            .read_chat(id)
            .map_err(|error| chat_error(error, id))?;
        Ok((
            AiChat {
                id: chat.id,
                title: chat.title,
                updated: chat.updated,
            },
            messages.into_iter().map(public_message).collect(),
        ))
    }

    fn create_chat(&self) -> Result<AiChat, ServiceError> {
        let engine = self.engine()?;
        let created = engine
            .create_chat(CreateChatInput {
                title: NEW_CHAT_TITLE.to_owned(),
                retention: None,
                retention_days: None,
            })
            .map_err(service_error)?;
        Ok(AiChat {
            id: created.value.id,
            title: created.value.title,
            updated: created.value.updated,
        })
    }
}

/// How a provider's answer ended.
enum Ended {
    Completed,
    Cancelled,
    Failed(CoreError),
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

    fn send(&self, request: AiRequest) -> Result<AiRun, ServiceError> {
        self.ai.send(&self.plugin, request)
    }

    fn answer_tools(
        &self,
        run: &str,
        results: Vec<AiToolResult>,
        tools: Vec<AiToolDefinition>,
    ) -> Result<AiRun, ServiceError> {
        self.ai.answer_tools(run, results, tools)
    }

    fn read_run(&self, id: &str) -> Result<AiRun, ServiceError> {
        self.ai.read_run(id)
    }

    fn stop(&self, id: &str) -> Result<(), ServiceError> {
        self.ai.stop(id)
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

/// Finish the assistant message `message` with `content` and `status`,
/// unless something else finished it. A change to the chat made elsewhere
/// meanwhile stays: the write reads the files again and tries once more.
fn finish_answer(
    engine: &WorkspaceEngine,
    chat: &str,
    (message, begun): (&str, &str),
    content: &str,
    status: ChatMessageStatus,
    error_code: Option<&str>,
) -> Result<(), CoreError> {
    let mut attempts = 0;
    loop {
        attempts += 1;
        let read = engine.read_chat(chat)?;
        // The index may not list the message yet, as after it failed to
        // record it: the file then still holds what Noura wrote.
        let revision = match read.messages.iter().find(|known| known.id == message) {
            Some(current) if current.status != ChatMessageStatus::InProgress => return Ok(()),
            Some(current) => current.revision.clone(),
            None => begun.to_owned(),
        };
        let finished = engine.finish_chat_assistant(FinishChatAssistantInput {
            chat_id: chat.to_owned(),
            message_id: message.to_owned(),
            content: content.to_owned(),
            status: status.clone(),
            error_code: error_code.map(str::to_owned),
            expected_chat_revision: read.chat.revision.clone(),
            expected_message_revision: revision,
        });
        match finished {
            Ok(_) => return Ok(()),
            Err(error) if error.code == "revision_conflict" && attempts < 2 => {}
            Err(error) => return Err(error),
        }
    }
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
        _ => match error.category {
            local_core::ErrorCategory::Provider | local_core::ErrorCategory::Transient => {
                "The provider couldn't answer. Try again."
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
        };
        assert_eq!(new_provider_id("My OpenAI", &[]), "my-openai");
        assert_eq!(
            new_provider_id("My OpenAI", &[existing("my-openai")]),
            "my-openai-2"
        );
        assert_eq!(new_provider_id("✨", &[]), "provider");
    }
}
