#![allow(
    clippy::result_large_err,
    clippy::manual_div_ceil,
    reason = "Tauri commands return the complete structured CoreError contract over IPC; base64 sizing is intentional"
)]

use std::sync::{Arc, Mutex};

use local_core::{
    AiFoundation, AiInvokeInput, AiProviderConfig, AiResponse, CalendarEntry, CoreError, CoreEvent,
    CreateObjectInput, DraftReconcileInput, DraftReconcileResult, ManagedConflictResolveInput,
    ManagedDraftInput, ManagedDraftResult, MarkdownLinkTarget, MutationResult, ObjectPatch,
    RawConflictResolveInput, RawConflictResolveResult, RawMarkdownRead, RawReconcileInput,
    RawReconcileResult, RawSaveInput, RawSaveResult, ResolveConflictInput, SearchInput,
    SearchResult, UnmanagedFile, WorkspaceEngine, WorkspaceEntry, WorkspaceObject, WorkspaceState,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

#[derive(Default)]
struct AppState {
    engine: Mutex<Option<Arc<WorkspaceEngine>>>,
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
fn forward_events(app: AppHandle, engine: &WorkspaceEngine) {
    let mut events = engine.subscribe();
    tauri::async_runtime::spawn(async move {
        while let Ok(event) = events.recv().await {
            let _ = app.emit("noura://core-event", event);
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
fn ai_foundation(app: &AppHandle) -> Result<AiFoundation, CoreError> {
    let root = app.path().app_local_data_dir().map_err(|_| {
        CoreError::validation(
            "app_data_unavailable",
            "The application-data directory is unavailable",
            "ai_provider_load",
        )
    })?;
    AiFoundation::open(root.join("ai/providers.json"))
}
fn load_recent(app: &AppHandle) -> Vec<RecentWorkspace> {
    recent_path(app)
        .ok()
        .and_then(|path| std::fs::read(path).ok())
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}
fn save_recent(app: &AppHandle, workspace: &WorkspaceEngine) -> Result<(), CoreError> {
    let path = recent_path(app)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| CoreError::io(error, "workspace_recent", parent.to_str()))?;
    }
    let mut values = load_recent(app);
    let root = workspace.root().to_string_lossy().into_owned();
    values.retain(|value| value.workspace_id != workspace.manifest().id);
    values.insert(
        0,
        RecentWorkspace {
            path: root,
            name: workspace.manifest().name.clone(),
            workspace_id: workspace.manifest().id.clone(),
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
    forward_events(app, &engine);
    *state
        .engine
        .lock()
        .map_err(|_| unavailable("workspace_create"))? = Some(Arc::new(engine));
    Ok(value)
}
#[tauri::command]
fn workspace_open(
    app: AppHandle,
    state: State<AppState>,
    input: OpenWorkspaceInput,
) -> Result<WorkspaceState, CoreError> {
    let engine = WorkspaceEngine::open(&input.path)?;
    let value = engine.state();
    save_recent(&app, &engine)?;
    forward_events(app, &engine);
    *state
        .engine
        .lock()
        .map_err(|_| unavailable("workspace_open"))? = Some(Arc::new(engine));
    Ok(value)
}
#[tauri::command]
fn workspace_close(app: AppHandle, state: State<AppState>) -> Result<(), CoreError> {
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
                workspace_id: engine.manifest().id.clone(),
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
fn objects_create(
    state: State<AppState>,
    input: CreateObjectInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    with_engine(&state, "objects_create", |engine| {
        engine.create_object(input)
    })
}
#[tauri::command]
fn objects_update(
    state: State<AppState>,
    id: String,
    patch: ObjectPatch,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    with_engine(&state, "objects_update", |engine| {
        engine.update_object(&id, patch)
    })
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
fn objects_move(
    state: State<AppState>,
    input: MoveInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    with_engine(&state, "objects_move", |engine| {
        engine.move_object(&input.id, &input.relative_path, &input.expected_revision)
    })
}
#[tauri::command]
fn objects_delete(
    state: State<AppState>,
    input: DeleteInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    with_engine(&state, "objects_delete", |engine| {
        engine.delete_object(&input.id, &input.expected_revision)
    })
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
fn ai_provider_list(app: AppHandle) -> Result<Vec<AiProviderConfig>, CoreError> {
    ai_foundation(&app)?.list_providers()
}
#[tauri::command]
fn ai_provider_save(app: AppHandle, input: AiProviderConfig) -> Result<(), CoreError> {
    ai_foundation(&app)?.save_provider(input)
}
#[tauri::command]
fn ai_credential_set(
    app: AppHandle,
    input: CredentialSetInput,
) -> Result<serde_json::Value, CoreError> {
    let credential_ref = ai_foundation(&app)?.set_credential(&input.provider_id, &input.secret)?;
    Ok(serde_json::json!({"credentialRef":credential_ref}))
}
#[tauri::command]
fn ai_credential_delete(app: AppHandle, input: CredentialDeleteInput) -> Result<(), CoreError> {
    ai_foundation(&app)?.delete_credential(&input.credential_ref)
}
#[tauri::command]
async fn ai_invoke(app: AppHandle, input: AiInvokeInput) -> Result<AiResponse, CoreError> {
    ai_foundation(&app)?.invoke(input).await
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
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(AppState::default())
        .setup(|app| {
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
            workspace_create,
            workspace_open,
            workspace_close,
            workspace_state,
            workspace_rebuild_index,
            workspace_list_recent,
            workspace_pick_folder,
            objects_query,
            objects_get,
            objects_create,
            objects_update,
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
            ai_invoke
        ])
        .run(tauri::generate_context!())
        .expect("failed to run the Noura desktop host");
}
