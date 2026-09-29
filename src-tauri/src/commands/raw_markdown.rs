//! Markdown files without a managed ID, read and saved as raw text.

use local_core::{
    CoreError, RawConflictResolveInput, RawConflictResolveResult, RawMarkdownRead,
    RawReconcileInput, RawReconcileResult, RawSaveInput, RawSaveResult,
};
use tauri::State;

use crate::{AppState, with_engine};

#[tauri::command(async)]
pub fn raw_markdown_read(
    state: State<AppState>,
    relative_path: String,
) -> Result<RawMarkdownRead, CoreError> {
    with_engine(&state, "raw_markdown_read", |engine| {
        engine.read_raw_markdown(&relative_path)
    })
}

#[tauri::command(async)]
pub fn raw_markdown_save(
    state: State<AppState>,
    input: RawSaveInput,
) -> Result<RawSaveResult, CoreError> {
    with_engine(&state, "raw_markdown_save", |engine| {
        engine.save_raw_markdown(input)
    })
}

#[tauri::command(async)]
pub fn raw_markdown_reconcile(
    state: State<AppState>,
    input: RawReconcileInput,
) -> Result<RawReconcileResult, CoreError> {
    with_engine(&state, "raw_markdown_reconcile", |engine| {
        engine.reconcile_raw_markdown(input)
    })
}

#[tauri::command(async)]
pub fn raw_markdown_resolve(
    state: State<AppState>,
    input: RawConflictResolveInput,
) -> Result<RawConflictResolveResult, CoreError> {
    with_engine(&state, "raw_markdown_resolve", |engine| {
        engine.resolve_raw_conflict(input)
    })
}
