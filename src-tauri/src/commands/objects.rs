//! Managed objects, their drafts and conflicts, and index queries.

use local_core::{
    CalendarEntry, CoreError, CreateObjectInput, DraftReconcileInput, DraftReconcileResult,
    ManagedConflictResolveInput, ManagedDraftInput, ManagedDraftResult, MutationResult,
    ObjectFilter, ObjectPatch, ObjectSummary, ObjectSummaryQuery, ResolveConflictInput,
    SearchInput, SearchResult, WorkspaceEngine, WorkspaceObject,
};
use serde::Deserialize;
use tauri::{AppHandle, Manager, State};

use crate::{AppState, blocking, current_engine, with_engine};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveInput {
    id: String,
    relative_path: String,
    expected_revision: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteInput {
    id: String,
    expected_revision: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdoptInput {
    relative_path: String,
    expected_revision: String,
    #[serde(rename = "type")]
    object_type: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CalendarInput {
    start: String,
    end: String,
}

#[tauri::command(async)]
pub fn objects_query(
    state: State<AppState>,
    query: ObjectFilter,
) -> Result<Vec<WorkspaceObject>, CoreError> {
    with_engine(&state, "objects_query", |engine| {
        engine.query_objects_filtered(&query)
    })
}

#[tauri::command(async)]
pub fn objects_summaries(
    state: State<AppState>,
    query: ObjectSummaryQuery,
) -> Result<Vec<ObjectSummary>, CoreError> {
    with_engine(&state, "objects_summaries", |engine| {
        engine.query_object_summaries(&query)
    })
}

#[tauri::command(async)]
pub fn objects_get(state: State<AppState>, id: String) -> Result<WorkspaceObject, CoreError> {
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
pub async fn objects_create(
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
pub async fn objects_update(
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

#[tauri::command(async)]
pub fn notes_reconcile_draft(
    state: State<AppState>,
    input: DraftReconcileInput,
) -> Result<DraftReconcileResult, CoreError> {
    with_engine(&state, "notes_reconcile_draft", |engine| {
        engine.reconcile_note_draft(input)
    })
}

#[tauri::command(async)]
pub fn notes_resolve_conflict(
    state: State<AppState>,
    input: ResolveConflictInput,
) -> Result<MutationResult<WorkspaceObject>, CoreError> {
    with_engine(&state, "notes_resolve_conflict", |engine| {
        engine.resolve_note_conflict(input)
    })
}

#[tauri::command(async)]
pub fn managed_draft_save(
    state: State<AppState>,
    input: ManagedDraftInput,
) -> Result<ManagedDraftResult, CoreError> {
    with_engine(&state, "managed_draft_save", |engine| {
        engine.save_managed_draft(input)
    })
}

#[tauri::command(async)]
pub fn managed_draft_reconcile(
    state: State<AppState>,
    input: ManagedDraftInput,
) -> Result<ManagedDraftResult, CoreError> {
    with_engine(&state, "managed_draft_reconcile", |engine| {
        engine.reconcile_managed_draft(input)
    })
}

#[tauri::command(async)]
pub fn managed_conflict_resolve(
    state: State<AppState>,
    input: ManagedConflictResolveInput,
) -> Result<WorkspaceObject, CoreError> {
    with_engine(&state, "managed_conflict_resolve", |engine| {
        engine.resolve_managed_conflict(input)
    })
}

#[tauri::command]
pub async fn objects_move(
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
pub async fn objects_delete(
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
pub fn objects_adopt(
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
pub fn search_query(
    state: State<AppState>,
    input: SearchInput,
) -> Result<Vec<SearchResult>, CoreError> {
    with_engine(&state, "search_query", |engine| engine.search(&input))
}

#[tauri::command(async)]
pub fn calendar_query(
    state: State<AppState>,
    input: CalendarInput,
) -> Result<Vec<CalendarEntry>, CoreError> {
    with_engine(&state, "calendar_query", |engine| {
        engine.calendar(&input.start, &input.end)
    })
}
