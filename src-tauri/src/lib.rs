#![allow(
    clippy::result_large_err,
    clippy::manual_div_ceil,
    reason = "Tauri commands return the complete structured CoreError contract over IPC; base64 sizing is intentional"
)]

use std::{
    collections::{HashMap, hash_map::Entry},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use local_core::{
    AiCancelOutcome, AiConsentGrant, AiConsentGrantInput, AiConsentReadInput, AiConsentRevokeInput,
    AiConsentRevokeOutcome, AiFoundation, AiProviderConfig, AiStreamFrame, AiStreamInput,
    AppendChatContextSummaryInput, AppendChatToolResultInput, AppendChatUserMessageInput,
    BeginChatAssistantInput, BeginChatToolCallInput, CalendarEntry, ChangeChatRetentionInput, Chat,
    ChatMessage, ChatRead, CoreError, CoreEvent, CreateChatInput, CreateObjectInput,
    DraftReconcileInput, DraftReconcileResult, ErrorCategory, FinishChatAssistantInput,
    FinishChatToolCallInput, ManagedConflictResolveInput, ManagedDraftInput, ManagedDraftResult,
    ManifestUpdateInput, MarkdownLinkTarget, MutationResult, ObjectPatch, RawConflictResolveInput,
    RawConflictResolveResult, RawMarkdownRead, RawReconcileInput, RawReconcileResult, RawSaveInput,
    RawSaveResult, RenameChatInput, ResolveConflictInput, SearchInput, SearchResult, UnmanagedFile,
    WorkspaceEngine, WorkspaceEntry, WorkspaceManifest, WorkspaceObject, WorkspaceState,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, ipc::Channel};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
mod sync_commands;

struct AppState {
    sync_auth_return: std::sync::atomic::AtomicBool,
    engine: Mutex<Option<Arc<WorkspaceEngine>>>,
    ai: Mutex<Option<Arc<AiFoundation>>>,
    ai_data_root: PathBuf,
    runtime_spike: Arc<PiRuntimeSpikeRegistry>,
    sync_account: tokio::sync::Mutex<local_core::sync::SyncAccountService>,
    sync_gate: tokio::sync::Mutex<()>,
    sync_cancel: tokio::sync::Notify,
    sync_wake: tokio::sync::Notify,
    sync_status: Mutex<Option<(PathBuf, local_core::sync::WorkspaceSyncStatus)>>,
    sync_realtime: Mutex<Option<sync_commands::RealtimeHandle>>,
}

impl AppState {
    fn new(ai_data_root: PathBuf) -> Self {
        Self {
            engine: Mutex::new(None),
            ai: Mutex::new(None),
            ai_data_root,
            runtime_spike: Arc::new(PiRuntimeSpikeRegistry::default()),
            sync_auth_return: std::sync::atomic::AtomicBool::new(false),
            sync_account: tokio::sync::Mutex::new(local_core::sync::SyncAccountService::default()),
            sync_gate: tokio::sync::Mutex::new(()),
            sync_cancel: tokio::sync::Notify::new(),
            sync_wake: tokio::sync::Notify::new(),
            sync_status: Mutex::new(None),
            sync_realtime: Mutex::new(None),
        }
    }

    fn ai(&self, operation: &str) -> Result<Arc<AiFoundation>, CoreError> {
        let mut ai = self.ai.lock().map_err(|_| ai_unavailable(operation))?;
        if let Some(ai) = ai.as_ref() {
            return Ok(ai.clone());
        }

        let foundation = AiFoundation::open(
            self.ai_data_root.join("ai/providers.json"),
            self.ai_data_root.join("ai/consents.json"),
        )
        .map_err(|_| ai_unavailable(operation))?;
        let foundation = Arc::new(foundation);
        *ai = Some(foundation.clone());
        Ok(foundation)
    }
}

fn ai_unavailable(operation: &str) -> CoreError {
    let mut error = CoreError::new(
        "ai_unavailable",
        ErrorCategory::Transient,
        "AI settings are unavailable. Check them and try again.",
        operation,
    );
    error.retryable = true;
    error
}

#[derive(Default)]
struct PiRuntimeSpikeRegistry {
    operations: Mutex<HashMap<String, tokio::sync::watch::Sender<()>>>,
}

struct PiRuntimeSpikeOperation {
    registry: Arc<PiRuntimeSpikeRegistry>,
    operation_id: String,
    cancellation: tokio::sync::watch::Receiver<()>,
}

impl Drop for PiRuntimeSpikeOperation {
    fn drop(&mut self) {
        if let Ok(mut operations) = self.registry.operations.lock() {
            operations.remove(&self.operation_id);
        }
    }
}

impl PiRuntimeSpikeRegistry {
    fn start(self: &Arc<Self>, operation_id: String) -> Result<PiRuntimeSpikeOperation, CoreError> {
        let (sender, cancellation) = tokio::sync::watch::channel(());
        let mut operations = self.operations.lock().map_err(|_| {
            CoreError::validation(
                "ai_operation_lock_unavailable",
                "The AI operation state is unavailable",
                "pi_runtime_spike_stream",
            )
        })?;
        match operations.entry(operation_id.clone()) {
            Entry::Vacant(entry) => {
                entry.insert(sender);
            }
            Entry::Occupied(_) => {
                return Err(CoreError::validation(
                    "ai_operation_in_progress",
                    "An AI operation with this identifier is already running",
                    "pi_runtime_spike_stream",
                ));
            }
        }
        Ok(PiRuntimeSpikeOperation {
            registry: self.clone(),
            operation_id,
            cancellation,
        })
    }

    fn cancel(&self, operation_id: &str) -> Result<bool, CoreError> {
        let operations = self.operations.lock().map_err(|_| {
            CoreError::validation(
                "ai_operation_lock_unavailable",
                "The AI operation state is unavailable",
                "pi_runtime_spike_cancel",
            )
        })?;
        Ok(operations
            .get(operation_id)
            .is_some_and(|cancellation| cancellation.send(()).is_ok()))
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PiRuntimeSpikeFrame {
    operation_id: String,
    sequence: u64,
    kind: PiRuntimeSpikeFrameKind,
    text: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "snake_case")]
enum PiRuntimeSpikeFrameKind {
    Delta,
    Done,
    Aborted,
}

fn validate_pi_runtime_spike_operation(operation_id: &str) -> Result<(), CoreError> {
    uuid::Uuid::parse_str(operation_id).map_err(|_| {
        CoreError::validation(
            "ai_operation_invalid",
            "The AI operation identifier is invalid",
            "pi_runtime_spike_stream",
        )
    })?;
    Ok(())
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RecentWorkspace {
    path: String,
    name: String,
    workspace_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateWorkspaceInput {
    path: String,
    name: String,
}
#[derive(Deserialize)]
struct OpenWorkspaceInput {
    path: String,
}
#[derive(Deserialize)]
struct ObjectQuery {
    #[serde(rename = "type")]
    object_type: Option<String>,
    project: Option<String>,
    status: Option<String>,
    priority: Option<String>,
    #[serde(rename = "pathPrefix")]
    path_prefix: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MoveInput {
    id: String,
    relative_path: String,
    expected_revision: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteInput {
    id: String,
    expected_revision: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AdoptInput {
    relative_path: String,
    expected_revision: String,
    #[serde(rename = "type")]
    object_type: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CalendarInput {
    start: String,
    end: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FolderInput {
    relative_path: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FolderMoveInput {
    from: String,
    to: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialSetInput {
    provider_id: String,
    secret: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct CredentialDeleteInput {
    credential_ref: String,
}

fn unavailable(operation: &str) -> CoreError {
    CoreError::validation(
        "workspace_not_open",
        "Open a workspace before using this operation",
        operation,
    )
}
fn with_engine<T>(
    state: &State<AppState>,
    operation: &str,
    run: impl FnOnce(&WorkspaceEngine) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    let engine = state
        .engine
        .lock()
        .map_err(|_| {
            CoreError::validation(
                "workspace_lock_unavailable",
                "The workspace state is unavailable",
                operation,
            )
        })?
        .clone()
        .ok_or_else(|| unavailable(operation))?;
    run(&engine)
}
fn forward_events(app: AppHandle, engine: Arc<WorkspaceEngine>) {
    let mut events = engine.subscribe();
    tauri::async_runtime::spawn(async move {
        loop {
            match events.recv().await {
                Ok(event) => {
                    // Opening another workspace replaces the engine. Do not leak
                    // delayed events from the retired workspace into its UI projection.
                    let current = app
                        .state::<AppState>()
                        .engine
                        .lock()
                        .ok()
                        .and_then(|value| value.clone());
                    if !current
                        .as_ref()
                        .is_some_and(|value| Arc::ptr_eq(value, &engine))
                    {
                        break;
                    }
                    let _ = app.emit("noura://core-event", event);
                }
                // A bounded broadcast channel may drop a burst. Keep the bridge
                // alive: the next event or a projection refresh will reconcile
                // the canonical files rather than leaving the session stale.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    eprintln!("noura event bridge lagged; skipped {skipped} events");
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    });
}
fn recent_path(app: &AppHandle) -> Result<std::path::PathBuf, CoreError> {
    app.path()
        .app_local_data_dir()
        .map(|path| path.join("recent-workspaces.json"))
        .map_err(|_| {
            CoreError::validation(
                "app_data_unavailable",
                "The application-data directory is unavailable",
                "workspace_recent",
            )
        })
}
fn load_recent(app: &AppHandle) -> Vec<RecentWorkspace> {
    recent_path(app)
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn recent_workspace_at_path<'a>(
    recent: &'a [RecentWorkspace],
    path: &str,
) -> Option<&'a RecentWorkspace> {
    let requested = Path::new(path).canonicalize().ok()?;
    recent.iter().find(|workspace| {
        Path::new(&workspace.path)
            .canonicalize()
            .is_ok_and(|candidate| candidate == requested)
    })
}

fn workspace_name_from_path(path: &str) -> String {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("Workspace")
        .to_owned()
}

// The path locates the last workspace; its manifest ID establishes identity.
// Do not silently open an older entry or a different workspace at a reused path.
fn restore_last_workspace(
    recent: &[RecentWorkspace],
    open: impl FnOnce(&RecentWorkspace) -> Result<WorkspaceEngine, CoreError>,
) -> Result<Option<WorkspaceEngine>, CoreError> {
    let Some(last) = recent.first() else {
        return Ok(None);
    };
    let engine = open(last)?;
    if engine.manifest().id != last.workspace_id {
        return Err(CoreError::validation(
            "workspace_identity_changed",
            "The last workspace is no longer at its saved location",
            "workspace_restore",
        ));
    }
    Ok(Some(engine))
}

fn save_recent(app: &AppHandle, workspace: &WorkspaceEngine) -> Result<(), CoreError> {
    let path = recent_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CoreError::io(error, "workspace_recent", parent.to_str()))?;
    }
    let mut values = load_recent(app);
    let root = workspace.root().to_string_lossy().into_owned();
    let manifest = workspace.manifest();
    values.retain(|value| value.workspace_id != manifest.id);
    values.insert(
        0,
        RecentWorkspace {
            path: root,
            name: manifest.name,
            workspace_id: manifest.id,
        },
    );
    values.truncate(20);
    let bytes = serde_json::to_vec_pretty(&values).map_err(|_| {
        CoreError::validation(
            "recent_serialize_failed",
            "Recent workspaces could not be saved",
            "workspace_recent",
        )
    })?;
    std::fs::write(&path, bytes)
        .map_err(|error| CoreError::io(error, "workspace_recent", path.to_str()))
}

#[tauri::command]
fn workspace_create(
    app: AppHandle,
    state: State<AppState>,
    input: CreateWorkspaceInput,
) -> Result<WorkspaceState, CoreError> {
    let engine = WorkspaceEngine::create(&input.path, &input.name)?;
    let value = engine.state();
    save_recent(&app, &engine)?;
    let engine = Arc::new(engine);
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    *state
        .engine
        .lock()
        .map_err(|_| unavailable("workspace_create"))? = Some(engine.clone());
    forward_events(app, engine);
    Ok(value)
}
#[tauri::command]
fn workspace_open(
    app: AppHandle,
    state: State<AppState>,
    input: OpenWorkspaceInput,
) -> Result<WorkspaceState, CoreError> {
    let recent = load_recent(&app);
    let registered = recent_workspace_at_path(&recent, &input.path);
    let name = registered
        .map(|workspace| workspace.name.clone())
        .unwrap_or_else(|| workspace_name_from_path(&input.path));
    let workspace_id = registered.map(|workspace| workspace.workspace_id.as_str());
    let engine = WorkspaceEngine::open_or_initialize(&input.path, &name, workspace_id)?;
    if workspace_id.is_some_and(|workspace_id| workspace_id != engine.manifest().id) {
        return Err(CoreError::validation(
            "workspace_identity_changed",
            "The selected workspace is no longer at its saved location",
            "workspace_open",
        ));
    }
    let value = engine.state();
    save_recent(&app, &engine)?;
    let engine = Arc::new(engine);
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    *state
        .engine
        .lock()
        .map_err(|_| unavailable("workspace_open"))? = Some(engine.clone());
    forward_events(app, engine);
    Ok(value)
}
#[tauri::command]
fn workspace_close(app: AppHandle, state: State<AppState>) -> Result<(), CoreError> {
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let engine = state
        .engine
        .lock()
        .map_err(|_| unavailable("workspace_close"))?
        .take();
    if let Some(engine) = engine {
        let _ = app.emit(
            "noura://core-event",
            CoreEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                event_type: "workspace:closed".into(),
                workspace_id: engine.manifest().id,
                occurred_at: local_core::now_rfc3339(),
                source: "application".into(),
                payload: serde_json::json!({}),
            },
        );
    }
    Ok(())
}
#[tauri::command]
fn workspace_state(state: State<AppState>) -> Result<WorkspaceState, CoreError> {
    let engine = state
        .engine
        .lock()
        .map_err(|_| unavailable("workspace_state"))?
        .clone();
    Ok(engine.as_ref().map_or(
        WorkspaceState {
            phase: local_core::WorkspacePhase::Idle,
            workspace_id: None,
            root_path: None,
            indexed_files: 0,
            diagnostics: Vec::new(),
        },
        |workspace| workspace.state(),
    ))
}
#[tauri::command]
fn workspace_rebuild_index(state: State<AppState>) -> Result<WorkspaceState, CoreError> {
    let engine = state
        .engine
        .lock()
        .map_err(|_| unavailable("workspace_rebuild_index"))?
        .clone()
        .ok_or_else(|| unavailable("workspace_rebuild_index"))?;
    engine.rebuild_index()?;
    Ok(engine.state())
}
#[tauri::command]
fn manifest_read(state: State<AppState>) -> Result<WorkspaceManifest, CoreError> {
    with_engine(&state, "manifest_read", WorkspaceEngine::read_manifest)
}
#[tauri::command]
fn manifest_update(
    state: State<AppState>,
    input: ManifestUpdateInput,
) -> Result<WorkspaceManifest, CoreError> {
    with_engine(&state, "manifest_update", |engine| {
        engine.manifest_update(input)
    })
}
#[tauri::command]
fn plugin_state_get(
    state: State<AppState>,
    plugin_id: String,
    key: String,
) -> Result<Option<serde_json::Value>, CoreError> {
    with_engine(&state, "plugin_state_get", |engine| {
        engine.plugin_state_get(&plugin_id, &key)
    })
}
#[tauri::command]
fn plugin_state_set(
    state: State<AppState>,
    plugin_id: String,
    key: String,
    value: serde_json::Value,
) -> Result<(), CoreError> {
    with_engine(&state, "plugin_state_set", |engine| {
        engine.plugin_state_set(&plugin_id, &key, value)
    })
}
#[tauri::command]
fn plugin_state_delete(
    state: State<AppState>,
    plugin_id: String,
    key: String,
) -> Result<bool, CoreError> {
    with_engine(&state, "plugin_state_delete", |engine| {
        engine.plugin_state_delete(&plugin_id, &key)
    })
}
#[tauri::command]
fn workspace_list_recent(app: AppHandle) -> Vec<RecentWorkspace> {
    load_recent(&app)
}
#[tauri::command]
async fn workspace_pick_folder(app: AppHandle, title: String) -> Result<Option<String>, CoreError> {
    let selected = app.dialog().file().set_title(title).blocking_pick_folder();
    selected
        .map(|path| {
            path.into_path()
                .map_err(|_| {
                    CoreError::validation(
                        "unsupported_path",
                        "The selected folder cannot be represented as a local path",
                        "workspace_pick_folder",
                    )
                })?
                .into_os_string()
                .into_string()
                .map_err(|_| {
                    CoreError::validation(
                        "unsupported_path",
                        "The selected folder uses a path that Noura cannot represent safely",
                        "workspace_pick_folder",
                    )
                })
        })
        .transpose()
}
#[tauri::command]
fn objects_query(
    state: State<AppState>,
    query: ObjectQuery,
) -> Result<Vec<WorkspaceObject>, CoreError> {
    with_engine(&state, "objects_query", |engine| {
        let mut values = engine.query_objects(query.object_type.as_deref())?;
        values.retain(|value| {
            query.project.as_ref().is_none_or(|expected| {
                value
                    .properties
                    .get("project")
                    .and_then(serde_json::Value::as_str)
                    == Some(expected)
            }) && query.status.as_ref().is_none_or(|expected| {
                value
                    .properties
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    == Some(expected)
            }) && query.priority.as_ref().is_none_or(|expected| {
                value
                    .properties
                    .get("priority")
                    .and_then(serde_json::Value::as_str)
                    == Some(expected)
            }) && query
                .path_prefix
                .as_ref()
                .is_none_or(|prefix| value.relative_path.starts_with(prefix))
        });
        Ok(values)
    })
}
#[tauri::command]
fn objects_get(state: State<AppState>, id: String) -> Result<WorkspaceObject, CoreError> {
    with_engine(&state, "objects_get", |engine| {
        engine.get_object(&id)?.ok_or_else(|| {
            CoreError::validation(
                "object_not_found",
                "The object does not exist",
                "objects_get",
            )
        })
    })
}
#[tauri::command]
async fn objects_create(
    state: State<'_, AppState>,
    input: CreateObjectInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    let engine = state
        .engine
        .lock()
        .map_err(|_| unavailable("objects_create"))?
        .clone()
        .ok_or_else(|| unavailable("objects_create"))?;
    if engine.sync_configuration()?.is_none() {
        return engine.create_object(input);
    }
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&local_core::sync::OsSyncCredentials)?
        .ok_or_else(|| unavailable("sync_sign_in_required"))?;
    local_core::sync::WorkspaceSyncCoordinator::collaboration_create_object(
        &engine,
        &connection,
        &local_core::sync::OsSyncCredentials,
        input,
    )
}
#[tauri::command]
async fn objects_update(
    state: State<'_, AppState>,
    id: String,
    patch: ObjectPatch,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    let engine = state
        .engine
        .lock()
        .map_err(|_| unavailable("objects_update"))?
        .clone()
        .ok_or_else(|| unavailable("objects_update"))?;
    if !engine.collaboration_object_is_active(&id)? {
        return engine.update_object(&id, patch);
    }
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&local_core::sync::OsSyncCredentials)?
        .ok_or_else(|| unavailable("sync_sign_in_required"))?;
    local_core::sync::WorkspaceSyncCoordinator::collaboration_update_object(
        &engine,
        &connection,
        &local_core::sync::OsSyncCredentials,
        &id,
        patch,
    )
}

#[tauri::command]
fn chats_create(
    state: State<AppState>,
    input: CreateChatInput,
) -> Result<MutationResult<Chat>, CoreError> {
    with_engine(&state, "chat_create", |engine| engine.create_chat(input))
}

#[tauri::command]
fn chats_change_retention(
    state: State<AppState>,
    input: ChangeChatRetentionInput,
) -> Result<MutationResult<Chat>, CoreError> {
    with_engine(&state, "chat_change_retention", |engine| {
        engine.change_chat_retention(input)
    })
}

#[tauri::command]
fn chats_rename(
    state: State<AppState>,
    input: RenameChatInput,
) -> Result<MutationResult<Chat>, CoreError> {
    with_engine(&state, "chat_rename", |engine| engine.rename_chat(input))
}

#[tauri::command]
fn chats_list(state: State<AppState>) -> Result<Vec<Chat>, CoreError> {
    with_engine(&state, "chat_list", WorkspaceEngine::list_chats)
}

#[tauri::command]
fn chats_read(state: State<AppState>, id: String) -> Result<ChatRead, CoreError> {
    with_engine(&state, "chat_read", |engine| engine.read_chat(&id))
}

#[tauri::command]
fn chats_append_user_message(
    state: State<AppState>,
    input: AppendChatUserMessageInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_append_user_message", |engine| {
        engine.append_chat_user_message(input)
    })
}

#[tauri::command]
fn chats_begin_assistant(
    state: State<AppState>,
    input: BeginChatAssistantInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_begin_assistant", |engine| {
        engine.begin_chat_assistant(input)
    })
}

#[tauri::command]
fn chats_finish_assistant(
    state: State<AppState>,
    input: FinishChatAssistantInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_finish_assistant", |engine| {
        engine.finish_chat_assistant(input)
    })
}

#[tauri::command]
fn chats_begin_tool_call(
    state: State<AppState>,
    input: BeginChatToolCallInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_begin_tool_call", |engine| {
        engine.begin_chat_tool_call(input)
    })
}

#[tauri::command]
fn chats_finish_tool_call(
    state: State<AppState>,
    input: FinishChatToolCallInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_finish_tool_call", |engine| {
        engine.finish_chat_tool_call(input)
    })
}

#[tauri::command]
fn chats_append_tool_result(
    state: State<AppState>,
    input: AppendChatToolResultInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_append_tool_result", |engine| {
        engine.append_chat_tool_result(input)
    })
}

#[tauri::command]
fn chats_append_context_summary(
    state: State<AppState>,
    input: AppendChatContextSummaryInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_append_context_summary", |engine| {
        engine.append_chat_context_summary(input)
    })
}

#[tauri::command]
fn chats_recover_interrupted(state: State<AppState>, id: String) -> Result<ChatRead, CoreError> {
    with_engine(&state, "chat_recover_interrupted", |engine| {
        engine.recover_interrupted_chat(&id)
    })
}

#[tauri::command]
fn chats_expire(state: State<AppState>, now: String) -> Result<Vec<String>, CoreError> {
    with_engine(&state, "chat_expire", |engine| engine.expire_chats(&now))
}

#[tauri::command]
fn notes_reconcile_draft(
    state: State<AppState>,
    input: DraftReconcileInput,
) -> Result<DraftReconcileResult, CoreError> {
    with_engine(&state, "notes_reconcile_draft", |engine| {
        engine.reconcile_note_draft(input)
    })
}
#[tauri::command]
fn notes_resolve_conflict(
    state: State<AppState>,
    input: ResolveConflictInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    with_engine(&state, "notes_resolve_conflict", |engine| {
        engine.resolve_note_conflict(input)
    })
}

#[tauri::command]
fn managed_draft_save(
    state: State<AppState>,
    input: ManagedDraftInput,
) -> Result<ManagedDraftResult, CoreError> {
    with_engine(&state, "managed_draft_save", |engine| {
        engine.save_managed_draft(input)
    })
}

#[tauri::command]
fn managed_draft_reconcile(
    state: State<AppState>,
    input: ManagedDraftInput,
) -> Result<ManagedDraftResult, CoreError> {
    with_engine(&state, "managed_draft_reconcile", |engine| {
        engine.reconcile_managed_draft(input)
    })
}

#[tauri::command]
fn managed_conflict_resolve(
    state: State<AppState>,
    input: ManagedConflictResolveInput,
) -> Result<WorkspaceObject, CoreError> {
    with_engine(&state, "managed_conflict_resolve", |engine| {
        engine.resolve_managed_conflict(input)
    })
}

#[tauri::command]
fn raw_markdown_read(
    state: State<AppState>,
    relative_path: String,
) -> Result<RawMarkdownRead, CoreError> {
    with_engine(&state, "raw_markdown_read", |engine| {
        engine.read_raw_markdown(&relative_path)
    })
}

#[tauri::command]
fn raw_markdown_save(
    state: State<AppState>,
    input: RawSaveInput,
) -> Result<RawSaveResult, CoreError> {
    with_engine(&state, "raw_markdown_save", |engine| {
        engine.save_raw_markdown(input)
    })
}

#[tauri::command]
fn raw_markdown_reconcile(
    state: State<AppState>,
    input: RawReconcileInput,
) -> Result<RawReconcileResult, CoreError> {
    with_engine(&state, "raw_markdown_reconcile", |engine| {
        engine.reconcile_raw_markdown(input)
    })
}

#[tauri::command]
fn raw_markdown_resolve(
    state: State<AppState>,
    input: RawConflictResolveInput,
) -> Result<RawConflictResolveResult, CoreError> {
    with_engine(&state, "raw_markdown_resolve", |engine| {
        engine.resolve_raw_conflict(input)
    })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct AssetInput {
    source_relative_path: String,
    target: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarkdownLinkInput {
    source_relative_path: String,
    target: String,
}

const MIME_BY_EXTENSION: &[(&str, &str)] = &[
    ("apng", "image/apng"),
    ("avif", "image/avif"),
    ("gif", "image/gif"),
    ("jpeg", "image/jpeg"),
    ("jpg", "image/jpeg"),
    ("png", "image/png"),
    ("svg", "image/svg+xml"),
    ("webp", "image/webp"),
];

const MAX_ASSET_BYTES: i64 = 20 * 1024 * 1024;

#[tauri::command]
fn files_inspect_pdf(
    state: State<AppState>,
    relative_path: String,
) -> Result<local_core::PdfInfo, CoreError> {
    with_engine(&state, "files_inspect_pdf", |engine| {
        engine.inspect_pdf(&relative_path)
    })
}

#[tauri::command]
fn files_read_pdf_range(
    state: State<AppState>,
    input: local_core::PdfRangeInput,
) -> Result<tauri::ipc::Response, CoreError> {
    with_engine(&state, "files_read_pdf_range", |engine| {
        engine.read_pdf_range(&input).map(tauri::ipc::Response::new)
    })
}

#[tauri::command]
fn files_open_pdf_link(url: String) -> Result<(), CoreError> {
    os_files::open_http_link(&url)
}

#[tauri::command]
fn files_read_local_asset(
    state: State<AppState>,
    input: AssetInput,
) -> Result<serde_json::Value, CoreError> {
    with_engine(&state, "files_read_local_asset", |engine| {
        let (relative_path, bytes) =
            engine.read_local_asset(&input.source_relative_path, &input.target, MAX_ASSET_BYTES)?;
        let extension = relative_path
            .rsplit('.')
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        let mime = MIME_BY_EXTENSION
            .iter()
            .find(|(candidate, _)| *candidate == extension)
            .map(|(_, mime)| *mime)
            .unwrap_or("application/octet-stream");
        Ok(serde_json::json!({
            "dataUrl": format!("data:{mime};base64,{}", base64_encode(&bytes)),
        }))
    })
}

#[tauri::command]
fn files_resolve_markdown_link(
    state: State<AppState>,
    input: MarkdownLinkInput,
) -> Result<MarkdownLinkTarget, CoreError> {
    with_engine(&state, "files_resolve_markdown_link", |engine| {
        engine.resolve_markdown_link(&input.source_relative_path, &input.target)
    })
}

fn base64_encode(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity((bytes.len() + 2) / 3 * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = chunk.get(1).copied().unwrap_or(0) as u32;
        let b2 = chunk.get(2).copied().unwrap_or(0) as u32;
        let triple = (b0 << 16) | (b1 << 8) | b2;
        output.push(TABLE[((triple >> 18) & 63) as usize] as char);
        output.push(TABLE[((triple >> 12) & 63) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[((triple >> 6) & 63) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(triple & 63) as usize] as char
        } else {
            '='
        });
    }
    output
}
#[tauri::command]
async fn objects_move(
    state: State<'_, AppState>,
    input: MoveInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    let engine = state
        .engine
        .lock()
        .map_err(|_| unavailable("objects_move"))?
        .clone()
        .ok_or_else(|| unavailable("objects_move"))?;
    if !engine.collaboration_object_is_active(&input.id)? {
        return engine.move_object(&input.id, &input.relative_path, &input.expected_revision);
    }
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&local_core::sync::OsSyncCredentials)?
        .ok_or_else(|| unavailable("sync_sign_in_required"))?;
    local_core::sync::WorkspaceSyncCoordinator::collaboration_move_object(
        &engine,
        &connection,
        &local_core::sync::OsSyncCredentials,
        &input.id,
        &input.relative_path,
        &input.expected_revision,
    )
}
#[tauri::command]
async fn objects_delete(
    state: State<'_, AppState>,
    input: DeleteInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    let engine = state
        .engine
        .lock()
        .map_err(|_| unavailable("objects_delete"))?
        .clone()
        .ok_or_else(|| unavailable("objects_delete"))?;
    if !engine.collaboration_object_is_active(&input.id)? {
        return engine.delete_object(&input.id, &input.expected_revision);
    }
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&local_core::sync::OsSyncCredentials)?
        .ok_or_else(|| unavailable("sync_sign_in_required"))?;
    local_core::sync::WorkspaceSyncCoordinator::collaboration_delete_object(
        &engine,
        &connection,
        &local_core::sync::OsSyncCredentials,
        &input.id,
        &input.expected_revision,
    )
}
#[tauri::command]
fn objects_adopt(
    state: State<AppState>,
    input: AdoptInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    with_engine(&state, "objects_adopt", |engine| {
        engine.adopt_markdown(
            &input.relative_path,
            &input.object_type,
            &input.expected_revision,
        )
    })
}
#[tauri::command]
fn search_query(
    state: State<AppState>,
    input: SearchInput,
) -> Result<Vec<SearchResult>, CoreError> {
    with_engine(&state, "search_query", |engine| engine.search(&input))
}
#[tauri::command]
fn calendar_query(
    state: State<AppState>,
    input: CalendarInput,
) -> Result<Vec<CalendarEntry>, CoreError> {
    with_engine(&state, "calendar_query", |engine| {
        engine.calendar(&input.start, &input.end)
    })
}
#[tauri::command]
fn folders_create(state: State<AppState>, input: FolderInput) -> Result<(), CoreError> {
    with_engine(&state, "folders_create", |engine| {
        engine.create_folder(&input.relative_path)
    })
}
#[tauri::command]
fn folders_list(state: State<AppState>) -> Result<Vec<local_core::FolderEntry>, CoreError> {
    with_engine(&state, "folders_list", WorkspaceEngine::list_folders)
}
#[tauri::command]
fn files_list(state: State<AppState>) -> Result<Vec<WorkspaceEntry>, CoreError> {
    with_engine(
        &state,
        "files_list",
        WorkspaceEngine::list_workspace_entries,
    )
}
#[tauri::command]
fn files_list_non_managed_markdown(
    state: State<AppState>,
) -> Result<Vec<UnmanagedFile>, CoreError> {
    with_engine(&state, "files_list_non_managed_markdown", |engine| {
        engine.list_non_managed_markdown()
    })
}
#[tauri::command]
fn folders_move(state: State<AppState>, input: FolderMoveInput) -> Result<(), CoreError> {
    with_engine(&state, "folders_move", |engine| {
        engine.move_folder(&input.from, &input.to)
    })
}
#[tauri::command]
fn folders_remove(state: State<AppState>, input: FolderInput) -> Result<(), CoreError> {
    with_engine(&state, "folders_remove", |engine| {
        engine.remove_empty_folder(&input.relative_path)
    })
}
#[tauri::command]
fn ai_provider_list(state: State<AppState>) -> Result<Vec<AiProviderConfig>, CoreError> {
    state.ai("ai_provider_list")?.list_providers()
}
#[tauri::command]
fn ai_provider_save(state: State<AppState>, input: AiProviderConfig) -> Result<(), CoreError> {
    state.ai("ai_provider_save")?.save_provider(input)
}
#[tauri::command]
fn ai_credential_set(
    state: State<AppState>,
    input: CredentialSetInput,
) -> Result<serde_json::Value, CoreError> {
    let credential_ref = state
        .ai("ai_credential_set")?
        .set_credential(&input.provider_id, &input.secret)?;
    Ok(serde_json::json!({"credentialRef":credential_ref}))
}
#[tauri::command]
fn ai_credential_delete(
    state: State<AppState>,
    input: CredentialDeleteInput,
) -> Result<(), CoreError> {
    state
        .ai("ai_credential_delete")?
        .delete_credential(&input.credential_ref)
}
#[tauri::command]
fn ai_consent_read(
    state: State<AppState>,
    input: AiConsentReadInput,
) -> Result<Option<AiConsentGrant>, CoreError> {
    with_engine(&state, "ai_consent_read", |engine| {
        state
            .ai("ai_consent_read")?
            .read_consent(&engine.manifest().id, input)
    })
}
#[tauri::command]
fn ai_consent_grant(
    state: State<AppState>,
    input: AiConsentGrantInput,
) -> Result<AiConsentGrant, CoreError> {
    with_engine(&state, "ai_consent_grant", |engine| {
        state
            .ai("ai_consent_grant")?
            .grant_consent(&engine.manifest().id, input)
    })
}
#[tauri::command]
fn ai_consent_revoke(
    state: State<AppState>,
    input: AiConsentRevokeInput,
) -> Result<AiConsentRevokeOutcome, CoreError> {
    with_engine(&state, "ai_consent_revoke", |engine| {
        state
            .ai("ai_consent_revoke")?
            .revoke_consent(&engine.manifest().id, input)
    })
}
#[tauri::command]
async fn ai_stream(
    state: State<'_, AppState>,
    input: AiStreamInput,
    channel: Channel<AiStreamFrame>,
) -> Result<(), CoreError> {
    let operation_id = input.operation_id.clone();
    let workspace_id = with_engine(&state, "ai_stream", |engine| Ok(engine.manifest().id))?;
    let ai = state.ai("ai_stream")?;
    let mut operation = ai.start_stream(&workspace_id, input)?;
    while let Some(frame) = operation.receiver.recv().await {
        if channel.send(frame).is_err() {
            let _ = ai.cancel_stream(&operation_id);
            break;
        }
    }
    Ok(())
}
#[tauri::command]
fn ai_stream_cancel(
    state: State<AppState>,
    operation_id: String,
) -> Result<AiCancelOutcome, CoreError> {
    state.ai("ai_stream_cancel")?.cancel_stream(&operation_id)
}

fn send_pi_runtime_spike_frame(
    channel: &Channel<PiRuntimeSpikeFrame>,
    operation_id: &str,
    sequence: u64,
    kind: PiRuntimeSpikeFrameKind,
    text: Option<&str>,
) -> bool {
    channel
        .send(PiRuntimeSpikeFrame {
            operation_id: operation_id.into(),
            sequence,
            kind,
            text: text.map(str::to_owned),
        })
        .is_ok()
}

#[tauri::command]
async fn pi_runtime_spike_stream(
    state: State<'_, AppState>,
    operation_id: String,
    channel: Channel<PiRuntimeSpikeFrame>,
) -> Result<(), CoreError> {
    validate_pi_runtime_spike_operation(&operation_id)?;
    let mut operation = state.runtime_spike.start(operation_id.clone())?;
    let mut sequence = 1;

    for text in ["Native ", "Tauri Channel ", "stream verified."] {
        tokio::select! {
            changed = operation.cancellation.changed() => {
                if changed.is_ok() {
                    let _ = send_pi_runtime_spike_frame(
                        &channel,
                        &operation_id,
                        sequence,
                        PiRuntimeSpikeFrameKind::Aborted,
                        None,
                    );
                }
                return Ok(());
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(120)) => {
                if !send_pi_runtime_spike_frame(
                    &channel,
                    &operation_id,
                    sequence,
                    PiRuntimeSpikeFrameKind::Delta,
                    Some(text),
                ) {
                    return Ok(());
                }
                sequence += 1;
            }
        }
    }

    let _ = send_pi_runtime_spike_frame(
        &channel,
        &operation_id,
        sequence,
        PiRuntimeSpikeFrameKind::Done,
        None,
    );
    Ok(())
}

#[tauri::command]
fn pi_runtime_spike_cancel(
    state: State<'_, AppState>,
    operation_id: String,
) -> Result<bool, CoreError> {
    validate_pi_runtime_spike_operation(&operation_id)?;
    state.runtime_spike.cancel(&operation_id)
}

mod os_files;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShowInFolderInput {
    id: String,
}

#[tauri::command]
fn object_show_in_folder(
    state: State<AppState>,
    input: ShowInFolderInput,
) -> Result<(), CoreError> {
    with_engine(&state, "object_show_in_folder", |engine| {
        os_files::reveal_in_file_manager(engine, &input.id)
    })
}

#[tauri::command]
fn object_open_terminal(state: State<AppState>, input: ShowInFolderInput) -> Result<(), CoreError> {
    with_engine(&state, "object_open_terminal", |engine| {
        os_files::open_in_terminal(engine, &input.id)
    })
}

pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let builder = builder.plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
        for uri in args {
            sync_commands::handle_auth_return(app, &uri);
        }
    }));
    builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let root = app.path().app_local_data_dir().map_err(|_| {
                CoreError::validation(
                    "app_data_unavailable",
                    "The application-data directory is unavailable",
                    "ai_provider_load",
                )
            })?;
            app.manage(AppState::new(root));
            let handle = app.handle().clone();
            app.deep_link().on_open_url(move |event| {
                for uri in event.urls() {
                    sync_commands::handle_auth_return(&handle, uri.as_str());
                }
            });
            if let Some(urls) = app.deep_link().get_current()? {
                for uri in urls {
                    sync_commands::handle_auth_return(app.handle(), uri.as_str());
                }
            }
            #[cfg(any(target_os = "linux", target_os = "windows"))]
            app.deep_link().register_all()?;
            // Restore before the frontend asks for workspace_state, avoiding a
            // chooser flash or a late restore replacing a user's selection.
            match restore_last_workspace(&load_recent(app.handle()), |workspace| {
                WorkspaceEngine::open_or_initialize(
                    &workspace.path,
                    &workspace.name,
                    Some(&workspace.workspace_id),
                )
            }) {
                Ok(Some(engine)) => {
                    let engine = Arc::new(engine);
                    *app.state::<AppState>()
                        .engine
                        .lock()
                        .map_err(|_| unavailable("workspace_restore"))? = Some(engine.clone());
                    forward_events(app.handle().clone(), engine);
                }
                Ok(None) => {}
                Err(error) => {
                    // An unavailable disk or invalid workspace must not prevent
                    // launch. The ordinary chooser remains available to recover.
                    eprintln!("Last workspace could not be restored: {}", error.code);
                }
            }
            sync_commands::start(app.handle().clone());
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let mut last_full_reconciliation = std::time::Instant::now();
                loop {
                    tokio::time::sleep(std::time::Duration::from_millis(750)).await;
                    let state = handle.state::<AppState>();
                    let engine = state.engine.lock().ok().and_then(|value| value.clone());
                    if let Some(engine) = engine {
                        let _ = engine.poll_external_changes(std::time::Duration::from_millis(25));
                        if last_full_reconciliation.elapsed() >= std::time::Duration::from_secs(60)
                        {
                            let _ = engine.reconcile();
                            last_full_reconciliation = std::time::Instant::now();
                        }
                    }
                }
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            sync_commands::collaboration_open,
            sync_commands::collaboration_submit_updates,
            sync_commands::collaboration_close,
            sync_commands::collaboration_flush,
            sync_commands::collaboration_set_presence,
            sync_commands::sync_workspace_status,
            sync_commands::sync_workspace_devices,
            sync_commands::sync_workspace_conflicts,
            sync_commands::sync_workspace_resolve_conflict,
            sync_commands::sync_remote_workspaces,
            sync_commands::sync_workspace_join,
            sync_commands::sync_workspace_approve_device,
            sync_commands::sync_workspace_invitations,
            sync_commands::sync_workspace_create_invitation,
            sync_commands::sync_workspace_approve_invited_device,
            sync_commands::sync_workspace_finalize_invitation,
            sync_commands::sync_workspace_revoke_invitation,
            sync_commands::sync_workspace_enable,
            sync_commands::sync_workspace_pause,
            sync_commands::sync_workspace_resume,
            sync_commands::sync_service_configuration,
            sync_commands::sync_account_take_return,
            sync_commands::sync_account_current,
            sync_commands::sync_account_export_recovery,
            sync_commands::sync_account_import_recovery,
            sync_commands::sync_account_open_browser,
            sync_commands::sync_account_begin,
            sync_commands::sync_account_poll,
            sync_commands::sync_account_cancel,
            sync_commands::sync_account_disconnect,
            workspace_create,
            workspace_open,
            workspace_close,
            workspace_state,
            workspace_rebuild_index,
            workspace_list_recent,
            workspace_pick_folder,
            manifest_read,
            manifest_update,
            plugin_state_get,
            plugin_state_set,
            plugin_state_delete,
            objects_query,
            objects_get,
            objects_create,
            objects_update,
            chats_create,
            chats_change_retention,
            chats_rename,
            chats_list,
            chats_read,
            chats_append_user_message,
            chats_begin_assistant,
            chats_finish_assistant,
            chats_begin_tool_call,
            chats_finish_tool_call,
            chats_append_tool_result,
            chats_append_context_summary,
            chats_recover_interrupted,
            chats_expire,
            notes_reconcile_draft,
            notes_resolve_conflict,
            managed_draft_save,
            managed_draft_reconcile,
            managed_conflict_resolve,
            raw_markdown_read,
            raw_markdown_save,
            raw_markdown_reconcile,
            raw_markdown_resolve,
            files_read_local_asset,
            files_inspect_pdf,
            files_read_pdf_range,
            files_open_pdf_link,
            files_resolve_markdown_link,
            objects_move,
            objects_delete,
            objects_adopt,
            object_show_in_folder,
            object_open_terminal,
            search_query,
            calendar_query,
            folders_create,
            folders_list,
            files_list,
            files_list_non_managed_markdown,
            folders_move,
            folders_remove,
            ai_provider_list,
            ai_provider_save,
            ai_credential_set,
            ai_credential_delete,
            ai_consent_read,
            ai_consent_grant,
            ai_consent_revoke,
            ai_stream,
            ai_stream_cancel,
            pi_runtime_spike_stream,
            pi_runtime_spike_cancel
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the Noura desktop host");
}

#[cfg(test)]
mod runtime_spike_tests {
    use super::*;

    #[test]
    fn start_rejects_an_operation_id_that_is_already_running() {
        let registry = Arc::new(PiRuntimeSpikeRegistry::default());
        let operation_id = uuid::Uuid::new_v4().to_string();
        let _operation = registry.start(operation_id.clone()).unwrap();

        let Err(error) = registry.start(operation_id) else {
            panic!("the duplicate operation should be rejected");
        };

        assert_eq!(error.code, "ai_operation_in_progress");
    }

    #[tokio::test]
    async fn duplicate_start_keeps_the_original_operation_cancellable() {
        let registry = Arc::new(PiRuntimeSpikeRegistry::default());
        let operation_id = uuid::Uuid::new_v4().to_string();
        let mut original = registry.start(operation_id.clone()).unwrap();

        assert!(registry.start(operation_id.clone()).is_err());
        assert!(registry.cancel(&operation_id).unwrap());
        assert!(original.cancellation.changed().await.is_ok());
    }

    #[tokio::test]
    async fn cancel_notifies_the_active_operation() {
        let registry = Arc::new(PiRuntimeSpikeRegistry::default());
        let operation_id = uuid::Uuid::new_v4().to_string();
        let mut operation = registry.start(operation_id.clone()).unwrap();

        let cancelled = registry.cancel(&operation_id).unwrap();

        assert!(cancelled);
        assert!(operation.cancellation.changed().await.is_ok());
    }

    fn temporary_ai_data_root() -> PathBuf {
        std::env::temp_dir().join(format!("noura-desktop-ai-test-{}", uuid::Uuid::new_v4()))
    }

    #[test]
    fn ai_initialization_is_lazy_and_retries_after_invalid_provider_settings() {
        let root = temporary_ai_data_root();
        std::fs::create_dir_all(root.join("ai")).unwrap();
        std::fs::write(root.join("ai/providers.json"), b"not json").unwrap();
        let state = AppState::new(root.clone());

        let Err(error) = state.ai("ai_provider_list") else {
            panic!("invalid provider settings should not initialize AI");
        };

        assert_eq!(error.code, "ai_unavailable");
        assert_eq!(error.operation, "ai_provider_list");
        assert!(error.retryable);
        assert!(error.path.is_none());
        assert!(state.ai.lock().unwrap().is_none());

        std::fs::remove_file(root.join("ai/providers.json")).unwrap();
        assert!(
            state
                .ai("ai_provider_list")
                .unwrap()
                .list_providers()
                .unwrap()
                .is_empty()
        );

        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ai_initialization_redacts_unavailable_consent_settings() {
        let root = temporary_ai_data_root();
        std::fs::create_dir_all(root.join("ai/consents.json")).unwrap();
        let state = AppState::new(root.clone());

        let Err(error) = state.ai("ai_consent_read") else {
            panic!("unavailable consent settings should not initialize AI");
        };

        assert_eq!(error.code, "ai_unavailable");
        assert_eq!(error.operation, "ai_consent_read");
        assert!(error.retryable);
        assert!(error.path.is_none());
        assert!(error.details.is_none());

        drop(state);
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod workspace_restore_tests {
    use super::*;

    struct Fixture(PathBuf);
    impl Fixture {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("noura-restore-{}", uuid::Uuid::new_v4())))
        }
        fn create(&self, name: &str) -> RecentWorkspace {
            let path = self.0.join(name);
            let engine =
                WorkspaceEngine::create_with_app_data(&path, name, self.0.join("cache")).unwrap();
            RecentWorkspace {
                path: path.to_str().unwrap().into(),
                name: name.into(),
                workspace_id: engine.manifest().id,
            }
        }
        fn open(&self, path: &str) -> Result<WorkspaceEngine, CoreError> {
            WorkspaceEngine::open_with_app_data(path, self.0.join("cache"))
        }
        fn open_or_initialize(
            &self,
            workspace: &RecentWorkspace,
        ) -> Result<WorkspaceEngine, CoreError> {
            WorkspaceEngine::open_or_initialize_with_app_data(
                &workspace.path,
                &workspace.name,
                Some(&workspace.workspace_id),
                self.0.join("cache"),
            )
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn startup_reopens_the_most_recent_workspace() {
        let fixture = Fixture::new();
        let older = fixture.create("older");
        let last = fixture.create("last");
        let expected = last.workspace_id.clone();
        let engine =
            restore_last_workspace(&[last, older], |workspace| fixture.open(&workspace.path))
                .unwrap()
                .unwrap();
        assert_eq!(engine.manifest().id, expected);
    }
    #[test]
    fn first_launch_does_not_attempt_to_open_a_workspace() {
        assert!(
            restore_last_workspace(&[], |_| panic!("no workspace to open"))
                .unwrap()
                .is_none()
        );
    }
    #[test]
    fn missing_last_workspace_does_not_open_an_older_one_or_recreate_it() {
        let fixture = Fixture::new();
        let older = fixture.create("older");
        let last = fixture.create("last");
        std::fs::remove_dir_all(&last.path).unwrap();
        let path = PathBuf::from(&last.path);
        assert!(
            restore_last_workspace(&[last, older], |workspace| fixture.open(&workspace.path))
                .is_err()
        );
        assert!(!path.exists());
    }
    #[test]
    fn missing_manifest_is_recreated_with_saved_identity() {
        let fixture = Fixture::new();
        let last = fixture.create("last");
        let expected = last.workspace_id.clone();
        std::fs::remove_file(Path::new(&last.path).join(local_core::WORKSPACE_MANIFEST_PATH))
            .unwrap();
        std::fs::remove_dir_all(Path::new(&last.path).join(".noura")).unwrap();

        let engine = restore_last_workspace(std::slice::from_ref(&last), |workspace| {
            fixture.open_or_initialize(workspace)
        })
        .unwrap()
        .unwrap();

        assert_eq!(engine.manifest().id, expected);
        assert!(Path::new(&last.path).join(".noura/trash").is_dir());
    }
    #[test]
    fn reused_path_does_not_restore_a_different_workspace() {
        let fixture = Fixture::new();
        let mut last = fixture.create("last");
        last.workspace_id = "different-workspace".into();
        let Err(error) = restore_last_workspace(&[last], |workspace| fixture.open(&workspace.path))
        else {
            panic!("identity mismatch must be rejected")
        };
        assert_eq!(error.code, "workspace_identity_changed");
    }
}
