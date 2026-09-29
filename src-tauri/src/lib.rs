#![allow(
    clippy::result_large_err,
    clippy::manual_div_ceil,
    reason = "Tauri commands return the complete structured CoreError contract over IPC; base64 sizing is intentional"
)]

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use local_core::{
    AiCancelOutcome, AiConsentGrant, AiConsentGrantInput, AiConsentReadInput, AiConsentRevokeInput,
    AiConsentRevokeOutcome, AiFoundation, AiProviderConfig, AiStreamFrame, AiStreamInput,
    AppendChatContextSummaryInput, AppendChatToolResultInput, AppendChatUserMessageInput,
    BeginChatAssistantInput, BeginChatToolCallInput, CalendarEntry, ChangeChatRetentionInput, Chat,
    ChatMessage, ChatRead, CoreError, CoreEvent, CreateChatInput, CreateObjectInput,
    DraftReconcileInput, DraftReconcileResult, ErrorCategory, FinishChatAssistantInput,
    FinishChatToolCallInput, ManagedConflictResolveInput, ManagedDraftInput, ManagedDraftResult,
    ManifestUpdateInput, MarkdownLinkTarget, MutationResult, ObjectFilter, ObjectPatch,
    ObjectSummary, ObjectSummaryQuery, RawConflictResolveInput, RawConflictResolveResult,
    RawMarkdownRead, RawReconcileInput, RawReconcileResult, RawSaveInput, RawSaveResult,
    RenameChatInput, ResolveConflictInput, SearchInput, SearchResult, UnmanagedFile,
    WorkspaceEngine, WorkspaceEntry, WorkspaceManifest, WorkspaceObject, WorkspaceState,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, ipc::Channel};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_dialog::DialogExt;
mod diagnostics;
mod sync_commands;

struct AppState {
    sync_auth_return: std::sync::atomic::AtomicBool,
    engine: Mutex<Option<Arc<WorkspaceEngine>>>,
    ai: Mutex<Option<Arc<AiFoundation>>>,
    ai_data_root: PathBuf,
    /// Held while the last workspace reopens in the background at launch.
    restore: RestoreGate,
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
            restore: RestoreGate::default(),
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

/// No workspace is open, or the one that was open has closed.
pub(crate) fn workspace_not_open(operation: &str) -> CoreError {
    CoreError::validation("workspace_not_open", "Open a workspace first.", operation)
}

/// Sync needs a signed-in account on this device.
pub(crate) fn sign_in_required(operation: &str) -> CoreError {
    CoreError::new(
        "sync_sign_in_required",
        ErrorCategory::Credential,
        "Sign in to use sync.",
        operation,
    )
}

/// Host state was briefly unusable: a lock was poisoned by a crashed task or
/// a background task was cancelled. Retrying usually works. The workspace
/// itself is still open, so callers must not treat this as a closed one.
pub(crate) fn state_unavailable(operation: &str) -> CoreError {
    let mut error = CoreError::new(
        "state_unavailable",
        ErrorCategory::Transient,
        "Something went wrong. Try again.",
        operation,
    );
    error.retryable = true;
    error
}

/// Run engine, disk, lock, or keychain work on the blocking thread pool so
/// it never stalls the window or the async runtime.
pub(crate) async fn blocking<T: Send + 'static>(
    operation: &'static str,
    work: impl FnOnce() -> Result<T, CoreError> + Send + 'static,
) -> Result<T, CoreError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| state_unavailable(operation))?
}

/// Lets commands wait for the launch-time restore of the last workspace, so
/// the first `workspace_state` answer is the restored workspace instead of
/// the chooser.
#[derive(Default)]
struct RestoreGate {
    restoring: Mutex<bool>,
    finished: Condvar,
}

impl RestoreGate {
    fn begin(&self) {
        if let Ok(mut restoring) = self.restoring.lock() {
            *restoring = true;
        }
    }

    fn finish(&self) {
        if let Ok(mut restoring) = self.restoring.lock() {
            *restoring = false;
        }
        self.finished.notify_all();
    }

    /// Block until the restore finishes. The timeout keeps a hung disk from
    /// wedging every command; the chooser stays available after it.
    fn wait(&self) {
        let Ok(restoring) = self.restoring.lock() else {
            return;
        };
        let _ = self
            .finished
            .wait_timeout_while(restoring, Duration::from_secs(30), |restoring| *restoring);
    }
}

/// The open workspace engine, after any launch-time restore settles.
pub(crate) fn current_engine(
    state: &AppState,
    operation: &str,
) -> Result<Arc<WorkspaceEngine>, CoreError> {
    state.restore.wait();
    state
        .engine
        .lock()
        .map_err(|_| state_unavailable(operation))?
        .clone()
        .ok_or_else(|| workspace_not_open(operation))
}

/// The engine installed right now, without waiting for a launch restore.
pub(crate) fn installed_engine(state: &AppState) -> Option<Arc<WorkspaceEngine>> {
    state.engine.lock().ok().and_then(|engine| engine.clone())
}

/// Install a newly opened engine and route its events to the window.
fn install_engine(
    app: &AppHandle,
    state: &AppState,
    engine: WorkspaceEngine,
    operation: &str,
) -> Result<Arc<WorkspaceEngine>, CoreError> {
    let engine = Arc::new(engine);
    *state
        .engine
        .lock()
        .map_err(|_| state_unavailable(operation))? = Some(engine.clone());
    // Wake the sync loop only after the new engine is visible, so a parked
    // loop never wakes, sees no engine, and parks again.
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    forward_events(app.clone(), engine.clone());
    Ok(engine)
}

// Commands that touch the engine, the disk, locks, or the keychain are
// declared `#[tauri::command(async)]` or `async` so they never run on the
// main thread. The `commands_stay_off_the_main_thread` test enforces it.
fn with_engine<T>(
    state: &State<AppState>,
    operation: &str,
    run: impl FnOnce(&WorkspaceEngine) -> Result<T, CoreError>,
) -> Result<T, CoreError> {
    let engine = current_engine(state, operation)?;
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
                    // Turning the sync plugin on or off must reach the sync
                    // loop, whether it is parked or mid-pass.
                    if sync_commands::wakes_sync(&event) {
                        let state = app.state::<AppState>();
                        state.sync_cancel.notify_one();
                        state.sync_wake.notify_one();
                    }
                    if let Err(error) = app.emit("noura://core-event", event) {
                        log::warn!("could not deliver a workspace event: {error}");
                    }
                }
                // A bounded broadcast channel may drop a burst. Keep the bridge
                // alive: the next event or a projection refresh will reconcile
                // the canonical files rather than leaving the session stale.
                Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                    log::warn!("event bridge lagged; skipped {skipped} events");
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
    write_recent(&path, &values)
}

fn write_recent(path: &Path, values: &[RecentWorkspace]) -> Result<(), CoreError> {
    let bytes = serde_json::to_vec_pretty(values).map_err(|_| {
        CoreError::validation(
            "recent_serialize_failed",
            "Recent workspaces could not be saved",
            "workspace_recent",
        )
    })?;
    std::fs::write(path, bytes)
        .map_err(|error| CoreError::io(error, "workspace_recent", path.to_str()))
}

/// The recent list without one workspace. The workspace folder is untouched.
fn without_recent(values: Vec<RecentWorkspace>, workspace_id: &str) -> Vec<RecentWorkspace> {
    values
        .into_iter()
        .filter(|value| value.workspace_id != workspace_id)
        .collect()
}

#[tauri::command(async)]
fn workspace_create(
    app: AppHandle,
    state: State<AppState>,
    input: CreateWorkspaceInput,
) -> Result<WorkspaceState, CoreError> {
    state.restore.wait();
    let engine = WorkspaceEngine::create(&input.path, &input.name)?;
    engine.set_system_trash(local_core::os_trash());
    let value = engine.state();
    save_recent(&app, &engine)?;
    install_engine(&app, &state, engine, "workspace_create")?;
    Ok(value)
}
#[tauri::command(async)]
fn workspace_open(
    app: AppHandle,
    state: State<AppState>,
    input: OpenWorkspaceInput,
) -> Result<WorkspaceState, CoreError> {
    state.restore.wait();
    let recent = load_recent(&app);
    let registered = recent_workspace_at_path(&recent, &input.path);
    let name = registered
        .map(|workspace| workspace.name.clone())
        .unwrap_or_else(|| workspace_name_from_path(&input.path));
    let workspace_id = registered.map(|workspace| workspace.workspace_id.as_str());
    let engine = WorkspaceEngine::open_or_initialize(&input.path, &name, workspace_id)?;
    engine.set_system_trash(local_core::os_trash());
    if workspace_id.is_some_and(|workspace_id| workspace_id != engine.manifest().id) {
        return Err(CoreError::validation(
            "workspace_identity_changed",
            "The selected workspace is no longer at its saved location",
            "workspace_open",
        ));
    }
    let value = engine.state();
    save_recent(&app, &engine)?;
    install_engine(&app, &state, engine, "workspace_open")?;
    Ok(value)
}
#[tauri::command(async)]
fn workspace_close(app: AppHandle, state: State<AppState>) -> Result<(), CoreError> {
    state.restore.wait();
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let engine = state
        .engine
        .lock()
        .map_err(|_| state_unavailable("workspace_close"))?
        .take();
    if let Some(engine) = engine
        && let Err(error) = app.emit(
            "noura://core-event",
            CoreEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                event_type: "workspace:closed".into(),
                workspace_id: engine.manifest().id,
                occurred_at: local_core::now_rfc3339(),
                source: "application".into(),
                payload: serde_json::json!({}),
            },
        )
    {
        log::warn!("could not announce the closed workspace: {error}");
    }
    Ok(())
}
#[tauri::command(async)]
fn workspace_state(state: State<AppState>) -> Result<WorkspaceState, CoreError> {
    state.restore.wait();
    let engine = state
        .engine
        .lock()
        .map_err(|_| state_unavailable("workspace_state"))?
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
#[tauri::command(async)]
fn workspace_rebuild_index(state: State<AppState>) -> Result<WorkspaceState, CoreError> {
    let engine = current_engine(&state, "workspace_rebuild_index")?;
    engine.rebuild_index()?;
    Ok(engine.state())
}
#[tauri::command(async)]
fn manifest_read(state: State<AppState>) -> Result<WorkspaceManifest, CoreError> {
    with_engine(&state, "manifest_read", WorkspaceEngine::read_manifest)
}
#[tauri::command(async)]
fn manifest_update(
    app: AppHandle,
    state: State<AppState>,
    input: ManifestUpdateInput,
) -> Result<WorkspaceManifest, CoreError> {
    with_engine(&state, "manifest_update", |engine| {
        let renamed = input.name.is_some();
        let manifest = engine.manifest_update(input)?;
        // The recent list caches the name for the workspace switcher; the
        // manifest stays canonical, so a failed cache refresh is not an error.
        if renamed && let Err(error) = save_recent(&app, engine) {
            log::warn!(
                "could not refresh the recent workspace list: {}",
                error.code
            );
        }
        Ok(manifest)
    })
}
#[tauri::command(async)]
fn plugin_state_get(
    state: State<AppState>,
    plugin_id: String,
    key: String,
) -> Result<Option<serde_json::Value>, CoreError> {
    with_engine(&state, "plugin_state_get", |engine| {
        engine.plugin_state_get(&plugin_id, &key)
    })
}
#[tauri::command(async)]
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
#[tauri::command(async)]
fn plugin_state_delete(
    state: State<AppState>,
    plugin_id: String,
    key: String,
) -> Result<bool, CoreError> {
    with_engine(&state, "plugin_state_delete", |engine| {
        engine.plugin_state_delete(&plugin_id, &key)
    })
}
#[tauri::command(async)]
fn workspace_list_recent(app: AppHandle) -> Vec<RecentWorkspace> {
    load_recent(&app)
}
#[tauri::command(async)]
fn workspace_forget_recent(
    app: AppHandle,
    workspace_id: String,
) -> Result<Vec<RecentWorkspace>, CoreError> {
    let path = recent_path(&app)?;
    let values = without_recent(load_recent(&app), &workspace_id);
    if path.exists() {
        write_recent(&path, &values)?;
    }
    Ok(values)
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
#[tauri::command(async)]
fn objects_query(
    state: State<AppState>,
    query: ObjectFilter,
) -> Result<Vec<WorkspaceObject>, CoreError> {
    with_engine(&state, "objects_query", |engine| {
        engine.query_objects_filtered(&query)
    })
}
#[tauri::command(async)]
fn objects_summaries(
    state: State<AppState>,
    query: ObjectSummaryQuery,
) -> Result<Vec<ObjectSummary>, CoreError> {
    with_engine(&state, "objects_summaries", |engine| {
        engine.query_object_summaries(&query)
    })
}
#[tauri::command(async)]
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
/// Run an object mutation on a blocking thread with the shared routing rule.
/// The device connection is read only when the mutation collaborates.
async fn route_object_mutation<T: Send + 'static>(
    app: AppHandle,
    operation: &'static str,
    mutation: impl FnOnce(
        &WorkspaceEngine,
        &dyn Fn() -> Result<Option<local_core::sync::DeviceConnection>, CoreError>,
    ) -> Result<T, CoreError>
    + Send
    + 'static,
) -> Result<T, CoreError> {
    let engine = current_engine(&app.state::<AppState>(), operation)?;
    blocking(operation, move || {
        let state = app.state::<AppState>();
        // The account lock keeps the stored connection consistent with an
        // in-flight sign-in, as in `stored_sync_connection`.
        let connection = || {
            let _account = state.sync_account.blocking_lock();
            local_core::sync::SyncAccountService::stored_connection(
                &local_core::sync::OsSyncCredentials,
            )
        };
        mutation(&engine, &connection)
    })
    .await
}
#[tauri::command]
async fn objects_create(
    app: AppHandle,
    input: CreateObjectInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    route_object_mutation(app, "objects_create", move |engine, connection| {
        local_core::sync::route_create_object(
            engine,
            connection,
            &local_core::sync::OsSyncCredentials,
            input,
        )
    })
    .await
}
#[tauri::command]
async fn objects_update(
    app: AppHandle,
    id: String,
    patch: ObjectPatch,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    route_object_mutation(app, "objects_update", move |engine, connection| {
        local_core::sync::route_update_object(
            engine,
            connection,
            &local_core::sync::OsSyncCredentials,
            &id,
            patch,
            None,
        )
    })
    .await
}

/// The saved sync connection, read from the keychain on the blocking pool.
/// The account lock keeps it consistent with an in-flight sign-in.
pub(crate) async fn stored_sync_connection(
    state: &AppState,
    operation: &'static str,
) -> Result<Option<local_core::sync::DeviceConnection>, CoreError> {
    let _account = state.sync_account.lock().await;
    blocking(operation, || {
        local_core::sync::SyncAccountService::stored_connection(
            &local_core::sync::OsSyncCredentials,
        )
    })
    .await
}

/// Like `stored_sync_connection`, but sync needs a signed-in account.
pub(crate) async fn sync_connection(
    state: &AppState,
    operation: &'static str,
) -> Result<local_core::sync::DeviceConnection, CoreError> {
    stored_sync_connection(state, operation)
        .await?
        .ok_or_else(|| sign_in_required(operation))
}

#[tauri::command(async)]
fn chats_create(
    state: State<AppState>,
    input: CreateChatInput,
) -> Result<MutationResult<Chat>, CoreError> {
    with_engine(&state, "chat_create", |engine| engine.create_chat(input))
}

#[tauri::command(async)]
fn chats_change_retention(
    state: State<AppState>,
    input: ChangeChatRetentionInput,
) -> Result<MutationResult<Chat>, CoreError> {
    with_engine(&state, "chat_change_retention", |engine| {
        engine.change_chat_retention(input)
    })
}

#[tauri::command(async)]
fn chats_rename(
    state: State<AppState>,
    input: RenameChatInput,
) -> Result<MutationResult<Chat>, CoreError> {
    with_engine(&state, "chat_rename", |engine| engine.rename_chat(input))
}

#[tauri::command(async)]
fn chats_list(state: State<AppState>) -> Result<Vec<Chat>, CoreError> {
    with_engine(&state, "chat_list", WorkspaceEngine::list_chats)
}

#[tauri::command(async)]
fn chats_read(state: State<AppState>, id: String) -> Result<ChatRead, CoreError> {
    with_engine(&state, "chat_read", |engine| engine.read_chat(&id))
}

#[tauri::command(async)]
fn chats_append_user_message(
    state: State<AppState>,
    input: AppendChatUserMessageInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_append_user_message", |engine| {
        engine.append_chat_user_message(input)
    })
}

#[tauri::command(async)]
fn chats_begin_assistant(
    state: State<AppState>,
    input: BeginChatAssistantInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_begin_assistant", |engine| {
        engine.begin_chat_assistant(input)
    })
}

#[tauri::command(async)]
fn chats_finish_assistant(
    state: State<AppState>,
    input: FinishChatAssistantInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_finish_assistant", |engine| {
        engine.finish_chat_assistant(input)
    })
}

#[tauri::command(async)]
fn chats_begin_tool_call(
    state: State<AppState>,
    input: BeginChatToolCallInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_begin_tool_call", |engine| {
        engine.begin_chat_tool_call(input)
    })
}

#[tauri::command(async)]
fn chats_finish_tool_call(
    state: State<AppState>,
    input: FinishChatToolCallInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_finish_tool_call", |engine| {
        engine.finish_chat_tool_call(input)
    })
}

#[tauri::command(async)]
fn chats_append_tool_result(
    state: State<AppState>,
    input: AppendChatToolResultInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_append_tool_result", |engine| {
        engine.append_chat_tool_result(input)
    })
}

#[tauri::command(async)]
fn chats_append_context_summary(
    state: State<AppState>,
    input: AppendChatContextSummaryInput,
) -> Result<MutationResult<ChatMessage>, CoreError> {
    with_engine(&state, "chat_append_context_summary", |engine| {
        engine.append_chat_context_summary(input)
    })
}

#[tauri::command(async)]
fn chats_recover_interrupted(state: State<AppState>, id: String) -> Result<ChatRead, CoreError> {
    with_engine(&state, "chat_recover_interrupted", |engine| {
        engine.recover_interrupted_chat(&id)
    })
}

#[tauri::command(async)]
fn chats_expire(state: State<AppState>, now: String) -> Result<Vec<String>, CoreError> {
    with_engine(&state, "chat_expire", |engine| engine.expire_chats(&now))
}

#[tauri::command(async)]
fn notes_reconcile_draft(
    state: State<AppState>,
    input: DraftReconcileInput,
) -> Result<DraftReconcileResult, CoreError> {
    with_engine(&state, "notes_reconcile_draft", |engine| {
        engine.reconcile_note_draft(input)
    })
}
#[tauri::command(async)]
fn notes_resolve_conflict(
    state: State<AppState>,
    input: ResolveConflictInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    with_engine(&state, "notes_resolve_conflict", |engine| {
        engine.resolve_note_conflict(input)
    })
}

#[tauri::command(async)]
fn managed_draft_save(
    state: State<AppState>,
    input: ManagedDraftInput,
) -> Result<ManagedDraftResult, CoreError> {
    with_engine(&state, "managed_draft_save", |engine| {
        engine.save_managed_draft(input)
    })
}

#[tauri::command(async)]
fn managed_draft_reconcile(
    state: State<AppState>,
    input: ManagedDraftInput,
) -> Result<ManagedDraftResult, CoreError> {
    with_engine(&state, "managed_draft_reconcile", |engine| {
        engine.reconcile_managed_draft(input)
    })
}

#[tauri::command(async)]
fn managed_conflict_resolve(
    state: State<AppState>,
    input: ManagedConflictResolveInput,
) -> Result<WorkspaceObject, CoreError> {
    with_engine(&state, "managed_conflict_resolve", |engine| {
        engine.resolve_managed_conflict(input)
    })
}

#[tauri::command(async)]
fn raw_markdown_read(
    state: State<AppState>,
    relative_path: String,
) -> Result<RawMarkdownRead, CoreError> {
    with_engine(&state, "raw_markdown_read", |engine| {
        engine.read_raw_markdown(&relative_path)
    })
}

#[tauri::command(async)]
fn raw_markdown_save(
    state: State<AppState>,
    input: RawSaveInput,
) -> Result<RawSaveResult, CoreError> {
    with_engine(&state, "raw_markdown_save", |engine| {
        engine.save_raw_markdown(input)
    })
}

#[tauri::command(async)]
fn raw_markdown_reconcile(
    state: State<AppState>,
    input: RawReconcileInput,
) -> Result<RawReconcileResult, CoreError> {
    with_engine(&state, "raw_markdown_reconcile", |engine| {
        engine.reconcile_raw_markdown(input)
    })
}

#[tauri::command(async)]
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

#[tauri::command(async)]
fn files_inspect_pdf(
    state: State<AppState>,
    relative_path: String,
) -> Result<local_core::PdfInfo, CoreError> {
    with_engine(&state, "files_inspect_pdf", |engine| {
        engine.inspect_pdf(&relative_path)
    })
}

#[tauri::command(async)]
fn files_read_pdf_range(
    state: State<AppState>,
    input: local_core::PdfRangeInput,
) -> Result<tauri::ipc::Response, CoreError> {
    with_engine(&state, "files_read_pdf_range", |engine| {
        engine.read_pdf_range(&input).map(tauri::ipc::Response::new)
    })
}

#[tauri::command(async)]
fn files_open_pdf_link(url: String) -> Result<(), CoreError> {
    os_files::open_http_link(&url)
}

#[tauri::command(async)]
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

#[tauri::command(async)]
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
    app: AppHandle,
    input: MoveInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    route_object_mutation(app, "objects_move", move |engine, connection| {
        local_core::sync::route_move_object(
            engine,
            connection,
            &local_core::sync::OsSyncCredentials,
            &input.id,
            &input.relative_path,
            &input.expected_revision,
        )
    })
    .await
}
#[tauri::command]
async fn objects_delete(
    app: AppHandle,
    input: DeleteInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    route_object_mutation(app, "objects_delete", move |engine, connection| {
        local_core::sync::route_delete_object(
            engine,
            connection,
            &local_core::sync::OsSyncCredentials,
            &input.id,
            &input.expected_revision,
        )
    })
    .await
}
#[tauri::command(async)]
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
#[tauri::command(async)]
fn search_query(
    state: State<AppState>,
    input: SearchInput,
) -> Result<Vec<SearchResult>, CoreError> {
    with_engine(&state, "search_query", |engine| engine.search(&input))
}
#[tauri::command(async)]
fn calendar_query(
    state: State<AppState>,
    input: CalendarInput,
) -> Result<Vec<CalendarEntry>, CoreError> {
    with_engine(&state, "calendar_query", |engine| {
        engine.calendar(&input.start, &input.end)
    })
}
#[tauri::command(async)]
fn folders_create(state: State<AppState>, input: FolderInput) -> Result<(), CoreError> {
    with_engine(&state, "folders_create", |engine| {
        engine.create_folder(&input.relative_path)
    })
}
#[tauri::command(async)]
fn folders_list(state: State<AppState>) -> Result<Vec<local_core::FolderEntry>, CoreError> {
    with_engine(&state, "folders_list", WorkspaceEngine::list_folders)
}
#[tauri::command(async)]
fn files_list(state: State<AppState>) -> Result<Vec<WorkspaceEntry>, CoreError> {
    with_engine(
        &state,
        "files_list",
        WorkspaceEngine::list_workspace_entries,
    )
}
#[tauri::command(async)]
fn files_list_non_managed_markdown(
    state: State<AppState>,
) -> Result<Vec<UnmanagedFile>, CoreError> {
    with_engine(&state, "files_list_non_managed_markdown", |engine| {
        engine.list_non_managed_markdown()
    })
}
#[tauri::command(async)]
fn folders_move(state: State<AppState>, input: FolderMoveInput) -> Result<(), CoreError> {
    with_engine(&state, "folders_move", |engine| {
        engine.move_folder(&input.from, &input.to)
    })
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FileTrashInput {
    relative_path: String,
}
#[tauri::command(async)]
fn files_move(state: State<AppState>, input: FolderMoveInput) -> Result<(), CoreError> {
    with_engine(&state, "files_move", |engine| {
        engine.move_file(&input.from, &input.to)
    })
}
#[tauri::command(async)]
fn files_trash(state: State<AppState>, input: FileTrashInput) -> Result<Option<String>, CoreError> {
    with_engine(&state, "files_trash", |engine| {
        engine.trash_path(&input.relative_path)
    })
}
#[tauri::command(async)]
fn folders_remove(state: State<AppState>, input: FolderInput) -> Result<(), CoreError> {
    with_engine(&state, "folders_remove", |engine| {
        engine.remove_empty_folder(&input.relative_path)
    })
}
#[tauri::command(async)]
fn ai_provider_list(state: State<AppState>) -> Result<Vec<AiProviderConfig>, CoreError> {
    state.ai("ai_provider_list")?.list_providers()
}
#[tauri::command(async)]
fn ai_provider_save(state: State<AppState>, input: AiProviderConfig) -> Result<(), CoreError> {
    state.ai("ai_provider_save")?.save_provider(input)
}
#[tauri::command(async)]
fn ai_credential_set(
    state: State<AppState>,
    input: CredentialSetInput,
) -> Result<serde_json::Value, CoreError> {
    let credential_ref = state
        .ai("ai_credential_set")?
        .set_credential(&input.provider_id, &input.secret)?;
    Ok(serde_json::json!({"credentialRef":credential_ref}))
}
#[tauri::command(async)]
fn ai_credential_delete(
    state: State<AppState>,
    input: CredentialDeleteInput,
) -> Result<(), CoreError> {
    state
        .ai("ai_credential_delete")?
        .delete_credential(&input.credential_ref)
}
#[tauri::command(async)]
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
#[tauri::command(async)]
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
#[tauri::command(async)]
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
    let workspace_id = current_engine(&state, "ai_stream")?.manifest().id;
    let ai = state.ai("ai_stream")?;
    let starter = ai.clone();
    let mut operation = blocking("ai_stream", move || {
        starter.start_stream(&workspace_id, input)
    })
    .await?;
    while let Some(frame) = operation.receiver.recv().await {
        if channel.send(frame).is_err() {
            if let Err(error) = ai.cancel_stream(&operation_id) {
                log::warn!("could not cancel an abandoned AI stream: {}", error.code);
            }
            break;
        }
    }
    Ok(())
}
#[tauri::command(async)]
fn ai_stream_cancel(
    state: State<AppState>,
    operation_id: String,
) -> Result<AiCancelOutcome, CoreError> {
    state.ai("ai_stream_cancel")?.cancel_stream(&operation_id)
}

mod file_actions;
mod menu;
mod os_files;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ShowInFolderInput {
    id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
enum WorkspaceFolder {
    Root,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct WorkspaceShowInFolderInput {
    folder: WorkspaceFolder,
}

#[tauri::command(async)]
fn workspace_show_in_folder(
    state: State<AppState>,
    input: WorkspaceShowInFolderInput,
) -> Result<(), CoreError> {
    with_engine(&state, "workspace_show_in_folder", |engine| {
        let path = match input.folder {
            WorkspaceFolder::Root => engine.root().to_path_buf(),
        };
        os_files::open_directory(&path, "workspace_show_in_folder")
    })
}

#[tauri::command(async)]
fn app_open_link(url: String) -> Result<(), CoreError> {
    os_files::open_web_link(&url, "app_open_link")
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct McpConnection {
    command: String,
    args: Vec<String>,
}

/// The command an MCP client runs to reach the open workspace through this
/// app's own binary. An AppImage runs from a temporary mount, so use the
/// AppImage file itself there.
#[tauri::command(async)]
fn mcp_connection(state: State<AppState>) -> Result<McpConnection, CoreError> {
    with_engine(&state, "mcp_connection", |engine| {
        let command = std::env::var_os("APPIMAGE")
            .map(std::path::PathBuf::from)
            .map(Ok)
            .unwrap_or_else(std::env::current_exe)
            .map_err(|error| CoreError::io(error, "mcp_connection", None))?;
        let unsupported = || {
            CoreError::validation(
                "unsupported_path",
                "This path can't be represented for an MCP client",
                "mcp_connection",
            )
        };
        Ok(McpConnection {
            command: command.to_str().ok_or_else(unsupported)?.to_owned(),
            args: vec![
                "mcp".into(),
                "--workspace".into(),
                engine.root().to_str().ok_or_else(unsupported)?.to_owned(),
            ],
        })
    })
}

/// Start the MCP command exactly as a client would and wait for its
/// initialize reply, so users know the configuration works before copying it.
#[tauri::command]
async fn mcp_test_connection(state: State<'_, AppState>) -> Result<bool, CoreError> {
    let connection = mcp_connection(state)?;
    tauri::async_runtime::spawn_blocking(move || {
        use std::io::{BufRead, Write};
        let failed = || {
            CoreError::validation(
                "mcp_unavailable",
                "The MCP server did not start",
                "mcp_test_connection",
            )
        };
        let mut child = std::process::Command::new(&connection.command)
            .args(&connection.args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|_| failed())?;
        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": { "name": "noura-settings", "version": "1" }
            }
        });
        let mut stdin = child.stdin.take().ok_or_else(failed)?;
        let stdout = child.stdout.take().ok_or_else(failed)?;
        writeln!(stdin, "{request}").map_err(|_| failed())?;
        let (sender, receiver) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut line = String::new();
            let ok = std::io::BufReader::new(stdout).read_line(&mut line).is_ok()
                && serde_json::from_str::<serde_json::Value>(&line)
                    .is_ok_and(|reply| reply.get("result").is_some());
            let _ = sender.send(ok);
        });
        let ok = receiver
            .recv_timeout(std::time::Duration::from_secs(10))
            .unwrap_or(false);
        drop(stdin);
        // The probe already exited when it closed stdout early; a failed
        // kill is expected then.
        if let Err(error) = child.kill() {
            log::debug!("the MCP probe process had already stopped: {error}");
        }
        if let Err(error) = child.wait() {
            log::debug!("could not reap the MCP probe process: {error}");
        }
        Ok(ok)
    })
    .await
    .map_err(|_| state_unavailable("mcp_test_connection"))?
}

#[tauri::command(async)]
fn object_show_in_folder(
    state: State<AppState>,
    input: ShowInFolderInput,
) -> Result<(), CoreError> {
    with_engine(&state, "object_show_in_folder", |engine| {
        os_files::reveal_in_file_manager(engine, &input.id)
    })
}

#[tauri::command(async)]
fn object_open_terminal(state: State<AppState>, input: ShowInFolderInput) -> Result<(), CoreError> {
    with_engine(&state, "object_open_terminal", |engine| {
        os_files::open_in_terminal(engine, &input.id)
    })
}

/// Reopen the last workspace after the window shows. The frontend shows its
/// loading state meanwhile, and `workspace_state` waits for the result, so
/// the chooser never flashes before a restored workspace.
fn restore_last_workspace_in_background(app: AppHandle) {
    let state = app.state::<AppState>();
    state.restore.begin();
    tauri::async_runtime::spawn_blocking(move || {
        let state = app.state::<AppState>();
        match restore_last_workspace(&load_recent(&app), |workspace| {
            WorkspaceEngine::open_or_initialize(
                &workspace.path,
                &workspace.name,
                Some(&workspace.workspace_id),
            )
        }) {
            Ok(Some(engine)) => {
                engine.set_system_trash(local_core::os_trash());
                if let Err(error) = install_engine(&app, &state, engine, "workspace_restore") {
                    log::warn!("could not install the restored workspace: {}", error.code);
                }
            }
            Ok(None) => {}
            // An unavailable disk or invalid workspace must not prevent
            // launch. The ordinary chooser remains available to recover.
            Err(error) => log::warn!("the last workspace could not be reopened: {}", error.code),
        }
        state.restore.finish();
    });
}

/// Poll the watcher and run the periodic full rescan on a dedicated thread.
/// Waiting on the watcher queue here means changes are handled as soon as
/// they arrive, and the work never touches the main thread or the async
/// runtime.
fn watch_workspaces(app: AppHandle) {
    const RESCAN_EVERY: Duration = Duration::from_secs(60);
    let spawned = std::thread::Builder::new()
        .name("workspace-watch".into())
        .spawn(move || {
            let mut last_rescan = std::time::Instant::now();
            loop {
                let state = app.state::<AppState>();
                let engine = state.engine.lock().ok().and_then(|value| value.clone());
                let Some(engine) = engine else {
                    std::thread::sleep(Duration::from_millis(250));
                    continue;
                };
                if let Err(error) = engine.poll_external_changes(Duration::from_millis(750)) {
                    log::warn!("could not apply external changes: {}", error.code);
                }
                if last_rescan.elapsed() >= RESCAN_EVERY {
                    if let Err(error) = engine.reconcile() {
                        log::warn!("the periodic rescan failed: {}", error.code);
                    }
                    last_rescan = std::time::Instant::now();
                }
            }
        });
    if let Err(error) = spawned {
        log::error!("could not start the workspace watcher thread: {error}");
    }
}

/// Paint the window in the theme's background before the page loads, so a
/// dark system theme doesn't flash white.
fn match_window_to_theme(app: &AppHandle) {
    let Some(window) = app.get_webview_window("main") else {
        return;
    };
    if matches!(window.theme(), Ok(tauri::Theme::Dark))
        && let Err(error) = window.set_background_color(Some(tauri::window::Color(10, 10, 10, 255)))
    {
        log::debug!("could not set the window background: {error}");
    }
}

pub fn run() {
    let builder = tauri::Builder::default();
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let builder = builder
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            for uri in args {
                sync_commands::handle_auth_return(app, &uri);
            }
        }))
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ));
    // On Linux, Tauri can replace only an AppImage. A .deb install skips the
    // updater, so it never downloads an update it can't apply; users install
    // the new package instead.
    #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
    let builder = if cfg!(target_os = "linux") && std::env::var_os("APPIMAGE").is_none() {
        builder
    } else {
        builder.plugin(tauri_plugin_updater::Builder::new().build())
    };
    builder
        .plugin(diagnostics::log_plugin())
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
            match_window_to_theme(app.handle());
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
            restore_last_workspace_in_background(app.handle().clone());
            menu::install(app.handle())?;
            sync_commands::start(app.handle().clone());
            watch_workspaces(app.handle().clone());
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
            workspace_forget_recent,
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
            file_actions::files_copy,
            file_actions::files_reveal,
            file_actions::files_open_default,
            workspace_show_in_folder,
            app_open_link,
            mcp_connection,
            mcp_test_connection,
            object_open_terminal,
            search_query,
            calendar_query,
            folders_create,
            folders_list,
            files_list,
            files_list_non_managed_markdown,
            folders_move,
            folders_remove,
            files_move,
            files_trash,
            ai_provider_list,
            ai_provider_save,
            ai_credential_set,
            ai_credential_delete,
            ai_consent_read,
            ai_consent_grant,
            ai_consent_revoke,
            ai_stream,
            ai_stream_cancel,
            objects_summaries,
            diagnostics::app_diagnostics,
            os_files::app_capabilities
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the Noura desktop host");
}

#[cfg(test)]
mod host_state_tests {
    use super::*;

    #[test]
    fn host_errors_keep_distinct_codes() {
        // The app deactivates plugins and clears chats on workspace_not_open,
        // so a poisoned lock or a missing sign-in must not reuse that code.
        assert_eq!(workspace_not_open("x").code, "workspace_not_open");
        assert_eq!(sign_in_required("x").code, "sync_sign_in_required");
        let busy = state_unavailable("x");
        assert_eq!(busy.code, "state_unavailable");
        assert!(busy.retryable);
    }

    #[test]
    fn commands_wait_for_the_launch_restore() {
        let gate = Arc::new(RestoreGate::default());
        gate.begin();
        let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let waiter = {
            let (gate, finished) = (gate.clone(), finished.clone());
            std::thread::spawn(move || {
                gate.wait();
                finished.load(std::sync::atomic::Ordering::SeqCst)
            })
        };
        std::thread::sleep(Duration::from_millis(50));
        finished.store(true, std::sync::atomic::Ordering::SeqCst);
        gate.finish();
        assert!(waiter.join().unwrap());
        // Once finished, waiting returns at once.
        gate.wait();
    }
}

#[cfg(test)]
mod command_thread_tests {
    /// Plain `#[tauri::command]` functions run on the main thread and freeze
    /// the window while they work. These may stay there because they only
    /// read constants or an atomic flag.
    const MAY_RUN_ON_MAIN_THREAD: &[&str] = &[
        "app_capabilities",
        "sync_service_configuration",
        "sync_account_take_return",
    ];

    #[test]
    fn commands_stay_off_the_main_thread() {
        let sources = [
            include_str!("lib.rs"),
            include_str!("sync_commands.rs"),
            include_str!("os_files.rs"),
            include_str!("diagnostics.rs"),
        ];
        let mut main_thread = Vec::new();
        for source in sources {
            let lines = source.lines().collect::<Vec<_>>();
            for (index, line) in lines.iter().enumerate() {
                if line.trim() != "#[tauri::command]" {
                    continue;
                }
                let signature = lines[index + 1..]
                    .iter()
                    .find(|line| line.contains("fn "))
                    .expect("a command attribute precedes a function");
                if signature.contains("async fn ") {
                    continue;
                }
                let name = signature
                    .split("fn ")
                    .nth(1)
                    .and_then(|rest| rest.split(['(', '<']).next())
                    .unwrap_or_default()
                    .trim()
                    .to_owned();
                if !MAY_RUN_ON_MAIN_THREAD.contains(&name.as_str()) {
                    main_thread.push(name);
                }
            }
        }
        assert!(
            main_thread.is_empty(),
            "declare these commands #[tauri::command(async)] or async: {main_thread:?}"
        );
    }
}

#[cfg(test)]
mod ai_state_tests {
    use super::*;

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
    fn forgetting_a_recent_workspace_keeps_the_others_in_order() {
        let recent = |id: &str| RecentWorkspace {
            path: format!("/tmp/{id}"),
            name: id.to_owned(),
            workspace_id: id.to_owned(),
        };
        let remaining = without_recent(vec![recent("a"), recent("b"), recent("c")], "b");
        let ids: Vec<_> = remaining
            .iter()
            .map(|value| value.workspace_id.as_str())
            .collect();
        assert_eq!(ids, ["a", "c"]);
        assert_eq!(without_recent(remaining, "missing").len(), 2);
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
        // Point at a path that never existed. Removing a created workspace
        // leaves a Windows watcher handle race; the behavior under test is the
        // missing path, not the deletion.
        let missing = fixture.0.join("missing");
        let last = RecentWorkspace {
            path: missing.to_str().unwrap().into(),
            name: "last".into(),
            workspace_id: "missing-workspace".into(),
        };
        assert!(
            restore_last_workspace(&[last, older], |workspace| fixture.open(&workspace.path))
                .is_err()
        );
        assert!(!missing.exists());
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
