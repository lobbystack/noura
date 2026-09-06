use crate::AppState;
use local_core::sync::{
    SyncInvitationRole, WorkspaceSyncCoordinator, WorkspaceSyncPhase, WorkspaceSyncStatus,
};
use tauri::AppHandle;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;

fn current_engine(
    state: &AppState,
) -> Result<std::sync::Arc<local_core::WorkspaceEngine>, CoreError> {
    state
        .engine
        .lock()
        .map_err(|_| crate::unavailable("sync"))?
        .clone()
        .ok_or_else(|| crate::unavailable("sync"))
}

#[tauri::command]
pub async fn sync_workspace_conflicts(
    state: State<'_, AppState>,
) -> Result<Vec<local_core::sync::SyncConflict>, CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    WorkspaceSyncCoordinator::conflicts(&engine, &connection, &OsSyncCredentials)
}

#[tauri::command]
pub async fn sync_workspace_resolve_conflict(
    state: State<'_, AppState>,
    input: local_core::sync::ResolveSyncConflict,
) -> Result<(), CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    WorkspaceSyncCoordinator::resolve_conflict(&engine, &connection, &OsSyncCredentials, &input)
}

#[tauri::command]
pub async fn sync_workspace_devices(
    state: State<'_, AppState>,
) -> Result<Vec<local_core::sync::SyncDevice>, CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    WorkspaceSyncCoordinator::devices(&engine, &connection, &OsSyncCredentials).await
}

#[tauri::command]
pub async fn sync_remote_workspaces(
    state: State<'_, AppState>,
) -> Result<Vec<local_core::sync::RemoteSyncWorkspace>, CoreError> {
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    connection.transport(&OsSyncCredentials)?.workspaces().await
}

#[tauri::command]
pub async fn sync_workspace_join(
    app: AppHandle,
    state: State<'_, AppState>,
    workspace_id: String,
    name: String,
) -> Result<Option<local_core::WorkspaceState>, CoreError> {
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    let dialog = app.clone();
    let app_data = state.ai_data_root.clone();
    let path = tauri::async_runtime::spawn_blocking(move || {
        let Some(selected) = dialog
            .dialog()
            .file()
            .set_title("Choose an empty folder for this workspace replica")
            .blocking_pick_folder()
        else {
            return Ok(None);
        };
        let path = selected
            .into_path()
            .map_err(|_| crate::unavailable("sync_replica_path_unavailable"))?;
        Ok(Some(path))
    })
    .await
    .map_err(|_| crate::unavailable("sync_join"))??;
    let Some(path) = path else {
        return Ok(None);
    };
    let engine = WorkspaceSyncCoordinator::join(
        path,
        &name,
        &workspace_id,
        app_data,
        &connection,
        &OsSyncCredentials,
    )
    .await?;
    crate::save_recent(&app, &engine)?;
    let result = engine.state();
    let engine = std::sync::Arc::new(engine);
    *state
        .engine
        .lock()
        .map_err(|_| crate::unavailable("sync_join"))? = Some(engine.clone());
    crate::forward_events(app, engine);
    Ok(Some(result))
}

#[tauri::command]
pub async fn sync_workspace_approve_device(
    state: State<'_, AppState>,
    device_id: String,
    fingerprint: String,
) -> Result<(), CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    WorkspaceSyncCoordinator::approve_device(
        &engine,
        &connection,
        &OsSyncCredentials,
        &device_id,
        &fingerprint,
    )
    .await
}

#[tauri::command]
pub async fn sync_workspace_invitations(
    state: State<'_, AppState>,
) -> Result<Vec<local_core::sync::SyncInvitation>, CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    WorkspaceSyncCoordinator::invitations(&engine, &connection, &OsSyncCredentials).await
}

#[tauri::command]
pub async fn sync_workspace_create_invitation(
    state: State<'_, AppState>,
    role: SyncInvitationRole,
) -> Result<local_core::sync::SyncInvitationLink, CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    WorkspaceSyncCoordinator::create_invitation(&engine, &connection, &OsSyncCredentials, role)
        .await
}

#[tauri::command]
pub async fn sync_workspace_approve_invited_device(
    state: State<'_, AppState>,
    invitation_id: String,
    device_id: String,
    fingerprint: String,
) -> Result<(), CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    WorkspaceSyncCoordinator::approve_invited_device(
        &engine,
        &connection,
        &OsSyncCredentials,
        &invitation_id,
        &device_id,
        &fingerprint,
    )
    .await
}

#[tauri::command]
pub async fn sync_workspace_finalize_invitation(
    state: State<'_, AppState>,
    invitation_id: String,
) -> Result<(), CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    WorkspaceSyncCoordinator::finalize_invitation(
        &engine,
        &connection,
        &OsSyncCredentials,
        &invitation_id,
    )
    .await
}

#[tauri::command]
pub async fn sync_workspace_revoke_invitation(
    state: State<'_, AppState>,
    invitation_id: String,
) -> Result<(), CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    WorkspaceSyncCoordinator::revoke_invitation(
        &engine,
        &connection,
        &OsSyncCredentials,
        &invitation_id,
    )
    .await
}

fn status(state: &AppState) -> Result<WorkspaceSyncStatus, CoreError> {
    status_for_engine(state, current_engine(state)?.as_ref())
}

fn status_for_engine(
    state: &AppState,
    engine: &local_core::WorkspaceEngine,
) -> Result<WorkspaceSyncStatus, CoreError> {
    let mut result = engine.sync_status()?;
    if result.enabled
        && let Some((root, live)) = state
            .sync_status
            .lock()
            .map_err(|_| crate::unavailable("sync"))?
            .as_ref()
        && root == engine.root()
    {
        result.phase = live.phase.clone();
        result.error_code = live.error_code.clone();
        result.last_success = live.last_success.clone();
    }
    Ok(result)
}

#[tauri::command]
pub fn sync_workspace_status(state: State<'_, AppState>) -> Result<WorkspaceSyncStatus, CoreError> {
    status(&state)
}

#[tauri::command]
pub async fn sync_workspace_enable(
    state: State<'_, AppState>,
) -> Result<WorkspaceSyncStatus, CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| {
            CoreError::validation(
                "sync_sign_in_required",
                "Sign in before enabling workspace sync",
                "sync",
            )
        })?;
    WorkspaceSyncCoordinator::enable(&engine, &connection, &OsSyncCredentials).await?;
    status_for_engine(&state, &engine)
}

#[tauri::command]
pub async fn sync_workspace_pause(
    state: State<'_, AppState>,
) -> Result<WorkspaceSyncStatus, CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    engine.sync_pause(true)?;
    status_for_engine(&state, &engine)
}

#[tauri::command]
pub async fn sync_workspace_resume(
    state: State<'_, AppState>,
) -> Result<WorkspaceSyncStatus, CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    engine.sync_pause(false)?;
    *state
        .sync_status
        .lock()
        .map_err(|_| crate::unavailable("sync"))? = None;
    status_for_engine(&state, &engine)
}

/// The native host owns the loop; leaving Settings does not interrupt synchronization.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut delay = std::time::Duration::from_secs(1);
        loop {
            let state = app.state::<AppState>();
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = state.sync_wake.notified() => {},
            }
            delay = std::time::Duration::from_secs(1);
            let _gate = state.sync_gate.lock().await;
            let Ok(engine) = current_engine(&state) else {
                continue;
            };
            let initial = engine.sync_status();
            if initial.as_ref().is_ok_and(|value| !value.enabled) {
                continue;
            }
            let mut live = initial.unwrap_or(WorkspaceSyncStatus {
                enabled: true,
                phase: WorkspaceSyncPhase::Error,
                pending: 0,
                conflicts: 0,
                error_code: None,
                last_success: None,
            });
            if let Ok(previous) = state.sync_status.lock()
                && let Some((root, previous)) = previous.as_ref()
                && root == engine.root()
            {
                live.last_success = previous.last_success.clone();
            }
            live.phase = if live.pending == 0 && live.last_success.is_some() {
                WorkspaceSyncPhase::Idle
            } else {
                WorkspaceSyncPhase::Syncing
            };
            if let Ok(mut saved) = state.sync_status.lock() {
                *saved = Some((engine.root().into(), live.clone()));
            }
            let mut events = engine.subscribe();
            let local_change = async {
                loop {
                    match events.recv().await {
                        Ok(event) if event.source == "sync" => continue,
                        _ => break,
                    }
                }
            };
            let run = async {
                let connection = state
                    .sync_account
                    .lock()
                    .await
                    .connection(&OsSyncCredentials)?
                    .ok_or_else(|| {
                        CoreError::validation(
                            "sync_sign_in_required",
                            "Sign in to resume workspace sync",
                            "sync",
                        )
                    })?;
                WorkspaceSyncCoordinator::pass_wait(&engine, &connection, &OsSyncCredentials).await
            };
            let result = tokio::select! {
                result = run => Some(result),
                _ = state.sync_cancel.notified() => None,
                _ = local_change => None,
            };
            match result {
                Some(Ok(_)) => {
                    delay = std::time::Duration::ZERO;
                    live.phase = WorkspaceSyncPhase::Idle;
                    live.last_success = Some(local_core::now_rfc3339());
                    live.error_code = None;
                }
                Some(Err(error)) => {
                    live.phase = WorkspaceSyncPhase::Error;
                    live.error_code = Some(error.code);
                    delay = std::time::Duration::from_secs(60);
                }
                None => {
                    delay = std::time::Duration::ZERO;
                    live.phase = WorkspaceSyncPhase::Idle;
                }
            }
            if let Ok(mut saved) = state.sync_status.lock() {
                *saved = Some((engine.root().into(), live));
            }
        }
    });
}

#[tauri::command]
pub async fn sync_account_export_recovery(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    tauri::async_runtime::spawn_blocking(move || {
        let Some(selected) = app
            .dialog()
            .file()
            .set_title("Save workspace recovery kit outside your workspace")
            .set_file_name(format!("noura-recovery-{}.json", engine.manifest().id))
            .blocking_save_file()
        else {
            return Ok(false);
        };
        let path = selected
            .into_path()
            .map_err(|_| crate::unavailable("sync_invalid_recovery_path"))?;
        WorkspaceSyncCoordinator::export_recovery_kit(
            &engine,
            &connection,
            &OsSyncCredentials,
            &path,
        )?;
        Ok(true)
    })
    .await
    .map_err(|_| crate::unavailable("sync_recovery_unavailable"))?
}

#[tauri::command]
pub async fn sync_account_import_recovery(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<bool, CoreError> {
    let engine = current_engine(&state)?;
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    let selected = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("Open the recovery kit for this workspace")
            .add_filter("Noura recovery kit", &["json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|_| crate::unavailable("sync_recovery_unavailable"))?;
    let Some(selected) = selected else {
        return Ok(false);
    };
    let path = selected
        .into_path()
        .map_err(|_| crate::unavailable("sync_invalid_recovery_path"))?;
    WorkspaceSyncCoordinator::import_recovery_kit(&engine, &connection, &OsSyncCredentials, &path)
        .await?;
    Ok(true)
}

#[tauri::command]
pub async fn sync_account_open_browser(state: State<'_, AppState>) -> Result<(), CoreError> {
    let uri = state.sync_account.lock().await.verification_uri()?;
    tauri::async_runtime::spawn_blocking(move || {
        #[cfg(target_os = "macos")]
        let result = std::process::Command::new("/usr/bin/open")
            .arg(&uri)
            .status();
        #[cfg(target_os = "linux")]
        let result = std::process::Command::new("xdg-open").arg(&uri).status();
        #[cfg(target_os = "windows")]
        let result = std::process::Command::new("rundll32.exe")
            .args(["url.dll,FileProtocolHandler", &uri])
            .status();
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
        if result.is_ok_and(|status| status.success()) {
            return Ok(());
        }
        Err(CoreError::validation(
            "sync_browser_unavailable",
            "Open the sign-in address in your browser",
            "sync_signin",
        ))
    })
    .await
    .map_err(|_| {
        CoreError::validation(
            "sync_browser_unavailable",
            "Open the sign-in address in your browser",
            "sync_signin",
        )
    })?
}
use local_core::{
    CoreError,
    sync::{DeviceSignInInfo, OsSyncCredentials, SyncAccount, SyncAccountPoll},
};
use tauri::State;

#[tauri::command]
pub async fn sync_account_current(
    state: State<'_, AppState>,
) -> Result<Option<SyncAccount>, CoreError> {
    state.sync_account.lock().await.current(&OsSyncCredentials)
}

#[tauri::command]
pub async fn sync_account_begin(
    state: State<'_, AppState>,
    origin: String,
) -> Result<DeviceSignInInfo, CoreError> {
    state
        .sync_account
        .lock()
        .await
        .begin(&origin, &OsSyncCredentials)
        .await
}

#[tauri::command]
pub async fn sync_account_poll(state: State<'_, AppState>) -> Result<SyncAccountPoll, CoreError> {
    state
        .sync_account
        .lock()
        .await
        .poll(&OsSyncCredentials)
        .await
}

#[tauri::command]
pub async fn sync_account_cancel(state: State<'_, AppState>) -> Result<(), CoreError> {
    state.sync_account.lock().await.cancel();
    Ok(())
}

#[tauri::command]
pub async fn sync_account_disconnect(state: State<'_, AppState>) -> Result<(), CoreError> {
    state.sync_cancel.notify_one();
    state.sync_wake.notify_one();
    let _gate = state.sync_gate.lock().await;
    state
        .sync_account
        .lock()
        .await
        .disconnect(&OsSyncCredentials)
        .await
}
