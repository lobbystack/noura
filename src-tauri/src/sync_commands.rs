use crate::AppState;
use futures::{SinkExt, StreamExt};
use local_core::sync::{
    DeviceKeys, OsSyncCredentials, SyncCredentials, SyncInvitationRole, WorkspaceSyncCoordinator,
    WorkspaceSyncPhase, WorkspaceSyncStatus,
};
use serde::Deserialize;
use std::{path::PathBuf, sync::Arc, time::Duration};
use tauri::AppHandle;
use tauri::Manager;
use tauri_plugin_dialog::DialogExt;
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{
        client::IntoClientRequest,
        http::{HeaderValue, header},
        protocol::{Message, WebSocketConfig},
    },
};

#[derive(Clone)]
pub(super) struct RealtimeHandle {
    id: String,
    root: PathBuf,
    sender: mpsc::Sender<RealtimeCommand>,
}

enum RealtimeCommand {
    SetPresence {
        input: local_core::sync::CollaborationPresenceInput,
        response: oneshot::Sender<Result<(), CoreError>>,
    },
}

struct PendingPresence {
    input: local_core::sync::CollaborationPresenceInput,
    responses: Vec<oneshot::Sender<Result<(), CoreError>>>,
}

struct RealtimeRegistration<'a> {
    state: &'a AppState,
    id: String,
}

impl Drop for RealtimeRegistration<'_> {
    fn drop(&mut self) {
        if let Ok(mut current) = self.state.sync_realtime.lock()
            && current.as_ref().is_some_and(|value| value.id == self.id)
        {
            *current = None;
        }
    }
}

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

fn realtime_error() -> CoreError {
    let mut error = CoreError::new(
        "sync_realtime_unavailable",
        local_core::ErrorCategory::Transient,
        "The realtime sync connection is unavailable",
        "sync_realtime",
    );
    error.retryable = true;
    error
}

fn realtime_url(origin: &str, workspace: &str) -> Result<String, CoreError> {
    realtime_identifier(workspace)?;
    let base = if let Some(value) = origin.strip_prefix("https://") {
        format!("wss://{value}")
    } else if let Some(value) = origin.strip_prefix("http://") {
        format!("ws://{value}")
    } else {
        return Err(realtime_error());
    };
    Ok(format!("{base}/v1/workspaces/{workspace}/realtime"))
}

fn realtime_identifier(value: &str) -> Result<(), CoreError> {
    if value.is_empty()
        || value.len() > 128
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
    {
        return Err(CoreError::validation(
            "sync_invalid_identifier",
            "The realtime identifier is invalid",
            "sync_realtime",
        ));
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RealtimeHello {
    #[serde(rename = "type")]
    kind: String,
    workspace_id: String,
    session_id: String,
    expires_in: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RealtimePresenceMessage {
    #[serde(rename = "type")]
    kind: String,
    presence: local_core::sync::EncryptedPresence,
    expires_at: u64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RealtimePresenceLeft {
    #[serde(rename = "type")]
    kind: String,
    workspace_id: String,
    device_id: String,
    session_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RealtimeServerError {
    #[serde(rename = "type")]
    kind: String,
    code: String,
}

async fn realtime_wait(
    state: &AppState,
    engine: &Arc<local_core::WorkspaceEngine>,
    connection: &local_core::sync::DeviceConnection,
) -> Result<(), CoreError> {
    let token = OsSyncCredentials.read(&connection.token_reference)?;
    let mut request = realtime_url(&connection.origin, &engine.manifest().id)?
        .into_client_request()
        .map_err(|_| realtime_error())?;
    request.headers_mut().insert(
        header::AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {}", token.as_str()))
            .map_err(|_| realtime_error())?,
    );
    request.headers_mut().insert(
        header::ORIGIN,
        HeaderValue::from_str(&connection.origin).map_err(|_| realtime_error())?,
    );
    let config = WebSocketConfig::default()
        .read_buffer_size(16 * 1024)
        .write_buffer_size(0)
        .max_write_buffer_size(1024 * 1024)
        .max_message_size(Some(16 * 1024))
        .max_frame_size(Some(16 * 1024));
    let (mut socket, _) = connect_async_with_config(request, Some(config), true)
        .await
        .map_err(|_| realtime_error())?;
    let hello = tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .map_err(|_| realtime_error())?
        .ok_or_else(realtime_error)?
        .map_err(|_| realtime_error())?;
    let Message::Text(hello) = hello else {
        return Err(realtime_error());
    };
    let hello: RealtimeHello =
        serde_json::from_str(hello.as_str()).map_err(|_| realtime_error())?;
    if hello.kind != "hello" || hello.workspace_id != engine.manifest().id || hello.expires_in != 30
    {
        return Err(realtime_error());
    }
    realtime_identifier(&hello.session_id)?;
    let config = engine.sync_configuration()?.ok_or_else(realtime_error)?;
    let device = DeviceKeys::load(&OsSyncCredentials, &connection.device_id)?;
    let secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
    let (sender, mut commands) = mpsc::channel(256);
    let registration_id = uuid::Uuid::new_v4().to_string();
    *state.sync_realtime.lock().map_err(|_| realtime_error())? = Some(RealtimeHandle {
        id: registration_id.clone(),
        root: engine.root().into(),
        sender,
    });
    let _registration = RealtimeRegistration {
        state,
        id: registration_id,
    };
    let mut sequence = 0_u64;
    let mut pending_presence: Option<PendingPresence> = None;
    let mut presence_tick = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_millis(100),
        Duration::from_millis(100),
    );
    presence_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut heartbeat = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(20),
        Duration::from_secs(20),
    );
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut expiry = tokio::time::interval(Duration::from_secs(1));
    expiry.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut durable_recovery = tokio::time::interval_at(
        tokio::time::Instant::now() + Duration::from_secs(5),
        Duration::from_secs(5),
    );
    durable_recovery.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            message = socket.next() => {
                match message {
                    Some(Ok(Message::Text(value))) => {
                        let value: serde_json::Value = serde_json::from_str(value.as_str())
                            .map_err(|_| realtime_error())?;
                        match value.get("type").and_then(serde_json::Value::as_str) {
                            Some("changed") => return Ok(()),
                            Some("presence") => {
                                let message: RealtimePresenceMessage = serde_json::from_value(value)
                                    .map_err(|_| realtime_error())?;
                                let now = std::time::SystemTime::now()
                                    .duration_since(std::time::UNIX_EPOCH)
                                    .map_err(|_| realtime_error())?
                                    .as_millis() as u64;
                                if message.kind != "presence"
                                    || message.expires_at <= now
                                    || message.expires_at > now.saturating_add(35_000)
                                {
                                    return Err(realtime_error());
                                }
                                if message.presence.device_id != device.device_id() {
                                    engine.collaboration_receive_presence(&message.presence, &secrets)?;
                                }
                            }
                            Some("presence-left") => {
                                let message: RealtimePresenceLeft = serde_json::from_value(value)
                                    .map_err(|_| realtime_error())?;
                                if message.kind != "presence-left" || message.workspace_id != engine.manifest().id {
                                    return Err(realtime_error());
                                }
                                engine.collaboration_remove_presence(
                                    &message.device_id,
                                    &message.session_id,
                                )?;
                            }
                            Some("error") => {
                                let message: RealtimeServerError = serde_json::from_value(value)
                                    .map_err(|_| realtime_error())?;
                                if message.kind != "error" {
                                    return Err(realtime_error());
                                }
                                if message.code != "sync.rate_limited" {
                                    return Err(realtime_error());
                                }
                            }
                            None | Some(_) => return Err(realtime_error()),
                        }
                    }
                    Some(Ok(Message::Ping(value))) => {
                        socket.send(Message::Pong(value)).await.map_err(|_| realtime_error())?;
                    }
                    Some(Ok(Message::Pong(_))) => {}
                    Some(Ok(Message::Close(_))) | None => return Err(realtime_error()),
                    Some(Ok(_)) | Some(Err(_)) => return Err(realtime_error()),
                }
            }
            command = commands.recv() => {
                let Some(RealtimeCommand::SetPresence { input, response }) = command else {
                    return Err(realtime_error());
                };
                if let Some(pending) = pending_presence.as_mut() {
                    pending.input = input;
                    pending.responses.push(response);
                } else {
                    pending_presence = Some(PendingPresence {
                        input,
                        responses: vec![response],
                    });
                }
            }
            _ = presence_tick.tick() => {
                let Some(pending) = pending_presence.take() else {
                    continue;
                };
                sequence = sequence.checked_add(1).ok_or_else(realtime_error)?;
                let result = engine
                    .collaboration_seal_presence(
                        pending.input,
                        &hello.session_id,
                        sequence,
                        &device,
                        &secrets,
                    )
                    .and_then(|presence| {
                        serde_json::to_string(&serde_json::json!({
                            "type": "presence",
                            "presence": presence,
                        }))
                        .map_err(|_| realtime_error())
                    });
                let result = match result {
                    Ok(value) => socket
                        .send(Message::text(value))
                        .await
                        .map_err(|_| realtime_error()),
                    Err(error) => Err(error),
                };
                let failed = result.is_err();
                for response in pending.responses {
                    let _ = response.send(result.clone());
                }
                if failed {
                    return Err(realtime_error());
                }
            }
            _ = heartbeat.tick() => {
                socket
                    .send(Message::Ping(Vec::new().into()))
                    .await
                    .map_err(|_| realtime_error())?;
            }
            _ = expiry.tick() => {
                engine.collaboration_expire_presence()?;
            }
            _ = durable_recovery.tick() => {
                // Socket state cannot prove that the application processed every
                // invalidation. Return control to the authoritative sync pass.
                return Ok(());
            }
        }
    }
}

/// The native host owns the loop; leaving Settings does not interrupt synchronization.
pub fn start(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let mut delay = std::time::Duration::from_secs(1);
        let mut reconnect_attempt = 0;
        let mut realtime_retry_at: Option<tokio::time::Instant> = None;
        loop {
            let state = app.state::<AppState>();
            tokio::select! {
                _ = tokio::time::sleep(delay) => {},
                _ = state.sync_wake.notified() => {},
            }
            let _gate = state.sync_gate.lock().await;
            let Ok(engine) = current_engine(&state) else {
                continue;
            };
            if reconnect_attempt > 0 {
                let _ = engine.collaboration_transport_status(
                    local_core::sync::collaboration::CollaborationStatus::Reconnecting,
                );
            }
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
                transition: None,
                activation: None,
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
                let transport = connection.transport(&OsSyncCredentials)?;
                if transport.realtime_available().await? {
                    WorkspaceSyncCoordinator::pass(&engine, &connection, &OsSyncCredentials)
                        .await?;
                    if realtime_retry_at.is_some_and(|retry| retry > tokio::time::Instant::now()) {
                        Ok::<bool, CoreError>(false)
                    } else {
                        realtime_wait(&state, &engine, &connection).await?;
                        Ok::<bool, CoreError>(true)
                    }
                } else {
                    WorkspaceSyncCoordinator::pass_wait(&engine, &connection, &OsSyncCredentials)
                        .await?;
                    Ok::<bool, CoreError>(true)
                }
            };
            let result = tokio::select! {
                result = run => Some(result),
                _ = state.sync_cancel.notified() => None,
                _ = local_change => None,
            };
            match result {
                Some(Ok(realtime_ready)) => {
                    if realtime_ready {
                        delay = std::time::Duration::ZERO;
                        reconnect_attempt = 0;
                        realtime_retry_at = None;
                    } else {
                        let until_retry = realtime_retry_at
                            .map(|retry| {
                                retry.saturating_duration_since(tokio::time::Instant::now())
                            })
                            .unwrap_or_default();
                        delay = until_retry.min(std::time::Duration::from_secs(5));
                    }
                    live.phase = WorkspaceSyncPhase::Idle;
                    live.last_success = Some(local_core::now_rfc3339());
                    live.error_code = None;
                }
                Some(Err(error)) => {
                    let _ = engine.collaboration_transport_status(
                        local_core::sync::collaboration::CollaborationStatus::Offline,
                    );
                    live.phase = WorkspaceSyncPhase::Error;
                    let realtime_failed = error.code == "sync_realtime_unavailable";
                    live.error_code = Some(error.code);
                    if error.retryable {
                        let jitter = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map_or(0, |value| value.subsec_nanos() as u64);
                        if realtime_failed {
                            realtime_retry_at = Some(
                                tokio::time::Instant::now()
                                    + reconnect_delay(reconnect_attempt, jitter),
                            );
                        }
                        delay = pull_fallback_delay(reconnect_attempt, jitter);
                        reconnect_attempt = reconnect_attempt.saturating_add(1);
                    } else {
                        delay = std::time::Duration::from_secs(30);
                        reconnect_attempt = 0;
                        realtime_retry_at = None;
                    }
                }
                None => {
                    delay = std::time::Duration::ZERO;
                    reconnect_attempt = 0;
                    realtime_retry_at = None;
                    live.phase = WorkspaceSyncPhase::Idle;
                }
            }
            if let Ok(mut saved) = state.sync_status.lock() {
                *saved = Some((engine.root().into(), live));
            }
        }
    });
}

fn reconnect_delay(attempt: u32, jitter_seed: u64) -> std::time::Duration {
    let base = 1_u64.checked_shl(attempt.min(5)).unwrap_or(30).min(30);
    let jitter_window_ms = (base * 250).min(30_000 - base * 1_000);
    let jitter_ms = if jitter_window_ms == 0 {
        0
    } else {
        jitter_seed % (jitter_window_ms + 1)
    };
    std::time::Duration::from_millis(base * 1_000 + jitter_ms)
}

/// Durable HTTP pulls remain authoritative even when the realtime socket appears connected.
/// Never let notification reconnect backoff postpone that recovery path beyond five seconds.
fn pull_fallback_delay(attempt: u32, jitter_seed: u64) -> std::time::Duration {
    reconnect_delay(attempt, jitter_seed).min(std::time::Duration::from_secs(5))
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
    sync::{DeviceSignInInfo, SyncAccount, SyncAccountPoll},
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

#[tauri::command]
pub async fn collaboration_open(
    state: State<'_, AppState>,
    input: local_core::sync::collaboration::CollaborationOpenInput,
) -> Result<Option<local_core::sync::collaboration::CollaborationSession>, CoreError> {
    let engine = current_engine(&state)?;
    if engine.sync_configuration()?.is_none() {
        return Ok(None);
    }
    let Some(connection) = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
    else {
        return Ok(None);
    };
    WorkspaceSyncCoordinator::collaboration_open(&engine, &connection, &OsSyncCredentials, input)
}
#[tauri::command]
pub async fn collaboration_submit_updates(
    state: State<'_, AppState>,
    input: local_core::sync::collaboration::CollaborationSubmitInput,
) -> Result<local_core::sync::collaboration::CollaborationReceipt, CoreError> {
    let engine = current_engine(&state)?;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    let receipt = WorkspaceSyncCoordinator::collaboration_submit(
        &engine,
        &connection,
        &OsSyncCredentials,
        input,
    )?;
    state.sync_wake.notify_one();
    Ok(receipt)
}
#[tauri::command]
pub fn collaboration_close(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), CoreError> {
    current_engine(&state)?.collaboration_close(&session_id)
}
#[tauri::command]
pub async fn collaboration_flush(
    state: State<'_, AppState>,
    session_id: String,
) -> Result<(), CoreError> {
    let engine = current_engine(&state)?;
    let connection = state
        .sync_account
        .lock()
        .await
        .connection(&OsSyncCredentials)?
        .ok_or_else(|| crate::unavailable("sync_sign_in_required"))?;
    WorkspaceSyncCoordinator::collaboration_flush(
        &engine,
        &connection,
        &OsSyncCredentials,
        &session_id,
    )
}

#[tauri::command]
pub async fn collaboration_set_presence(
    state: State<'_, AppState>,
    input: local_core::sync::CollaborationPresenceInput,
) -> Result<(), CoreError> {
    let root = current_engine(&state)?.root().to_path_buf();
    let handle = state
        .sync_realtime
        .lock()
        .map_err(|_| realtime_error())?
        .as_ref()
        .filter(|handle| handle.root == root)
        .cloned()
        .ok_or_else(realtime_error)?;
    let (response, result) = oneshot::channel();
    handle
        .sender
        .send(RealtimeCommand::SetPresence { input, response })
        .await
        .map_err(|_| realtime_error())?;
    tokio::time::timeout(Duration::from_secs(5), result)
        .await
        .map_err(|_| realtime_error())?
        .map_err(|_| realtime_error())?
}

#[cfg(test)]
mod tests {
    use super::{pull_fallback_delay, realtime_url, reconnect_delay};

    #[test]
    fn reconnect_backoff_is_jittered_and_bounded() {
        let ranges = [
            (1, 1_250),
            (2, 2_500),
            (4, 5_000),
            (8, 10_000),
            (16, 20_000),
        ];
        for (attempt, (minimum, maximum)) in ranges.into_iter().enumerate() {
            let low = reconnect_delay(attempt as u32, 0).as_millis();
            let high = reconnect_delay(attempt as u32, u64::MAX).as_millis();
            assert!(low >= minimum * 1_000 && low <= maximum);
            assert!(high >= minimum * 1_000 && high <= maximum);
        }
        assert_eq!(reconnect_delay(20, u64::MAX).as_secs(), 30);
    }

    #[test]
    fn durable_pull_fallback_never_waits_more_than_five_seconds() {
        assert_eq!(pull_fallback_delay(0, 0).as_secs(), 1);
        assert!(pull_fallback_delay(1, u64::MAX) > std::time::Duration::from_secs(2));
        assert_eq!(pull_fallback_delay(20, u64::MAX).as_secs(), 5);
    }

    #[test]
    fn realtime_urls_preserve_validated_origin_security() {
        assert_eq!(
            realtime_url("https://sync.example", "workspace").unwrap(),
            "wss://sync.example/v1/workspaces/workspace/realtime"
        );
        assert_eq!(
            realtime_url("http://127.0.0.1:1900", "workspace").unwrap(),
            "ws://127.0.0.1:1900/v1/workspaces/workspace/realtime"
        );
        assert!(realtime_url("ftp://sync.example", "workspace").is_err());
        assert!(realtime_url("https://sync.example", "../workspace").is_err());
    }
}
