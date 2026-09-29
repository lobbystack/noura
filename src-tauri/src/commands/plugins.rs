//! The workspace manifest and per-plugin state.

use local_core::{CoreError, ManifestUpdateInput, WorkspaceEngine, WorkspaceManifest};
use tauri::{AppHandle, State};

use super::workspace::save_recent;
use crate::{AppState, with_engine};

#[tauri::command(async)]
pub fn manifest_read(state: State<AppState>) -> Result<WorkspaceManifest, CoreError> {
    with_engine(&state, "manifest_read", WorkspaceEngine::read_manifest)
}

#[tauri::command(async)]
pub fn manifest_update(
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
pub fn plugin_state_get(
    state: State<AppState>,
    plugin_id: String,
    key: String,
) -> Result<Option<serde_json::Value>, CoreError> {
    with_engine(&state, "plugin_state_get", |engine| {
        engine.plugin_state_get(&plugin_id, &key)
    })
}

#[tauri::command(async)]
pub fn plugin_state_set(
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
pub fn plugin_state_delete(
    state: State<AppState>,
    plugin_id: String,
    key: String,
) -> Result<bool, CoreError> {
    with_engine(&state, "plugin_state_delete", |engine| {
        engine.plugin_state_delete(&plugin_id, &key)
    })
}
