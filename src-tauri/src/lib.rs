#![allow(
    clippy::result_large_err,
    clippy::manual_div_ceil,
    reason = "Tauri commands return the complete structured CoreError contract over IPC; base64 sizing is intentional"
)]

use std::{
    path::PathBuf,
    sync::{Arc, Condvar, Mutex},
    time::Duration,
};

use local_core::{AiFoundation, CoreError, ErrorCategory, WorkspaceEngine};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_deep_link::DeepLinkExt;

mod commands;
mod diagnostics;
mod file_actions;
mod menu;
mod os_files;
mod sync_commands;

use commands::{ai, chats, files, objects, os_integration, plugins, raw_markdown, workspace};

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
                CoreError::new(
                    "app_data_unavailable",
                    ErrorCategory::Filesystem,
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
            workspace::restore_last_workspace_in_background(app.handle().clone());
            menu::install(app.handle())?;
            sync_commands::start(app.handle().clone());
            workspace::watch_workspaces(app.handle().clone());
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
            workspace::workspace_create,
            workspace::workspace_open,
            workspace::workspace_close,
            workspace::workspace_state,
            workspace::workspace_rebuild_index,
            workspace::workspace_list_recent,
            workspace::workspace_forget_recent,
            workspace::workspace_pick_folder,
            plugins::manifest_read,
            plugins::manifest_update,
            plugins::plugin_state_get,
            plugins::plugin_state_set,
            plugins::plugin_state_delete,
            objects::objects_query,
            objects::objects_get,
            objects::objects_create,
            objects::objects_update,
            objects::notes_reconcile_draft,
            objects::notes_resolve_conflict,
            objects::managed_draft_save,
            objects::managed_draft_reconcile,
            objects::managed_conflict_resolve,
            objects::objects_move,
            objects::objects_delete,
            objects::objects_adopt,
            objects::search_query,
            objects::calendar_query,
            objects::objects_summaries,
            raw_markdown::raw_markdown_read,
            raw_markdown::raw_markdown_save,
            raw_markdown::raw_markdown_reconcile,
            raw_markdown::raw_markdown_resolve,
            files::files_read_local_asset,
            files::files_fetch_remote_image,
            files::files_inspect_pdf,
            files::files_read_pdf_range,
            files::files_open_pdf_link,
            files::files_resolve_markdown_link,
            files::folders_create,
            files::folders_list,
            files::files_list,
            files::files_list_non_managed_markdown,
            files::folders_move,
            files::folders_remove,
            files::files_move,
            files::files_trash,
            file_actions::files_copy,
            file_actions::files_reveal,
            file_actions::files_open_default,
            chats::chats_create,
            chats::chats_change_retention,
            chats::chats_rename,
            chats::chats_list,
            chats::chats_read,
            chats::chats_append_user_message,
            chats::chats_begin_assistant,
            chats::chats_finish_assistant,
            chats::chats_begin_tool_call,
            chats::chats_finish_tool_call,
            chats::chats_append_tool_result,
            chats::chats_append_context_summary,
            chats::chats_recover_interrupted,
            chats::chats_expire,
            ai::ai_provider_list,
            ai::ai_provider_save,
            ai::ai_credential_set,
            ai::ai_credential_delete,
            ai::ai_consent_read,
            ai::ai_consent_grant,
            ai::ai_consent_revoke,
            ai::ai_stream,
            ai::ai_stream_cancel,
            os_integration::object_show_in_folder,
            os_integration::workspace_show_in_folder,
            os_integration::app_open_link,
            os_integration::mcp_connection,
            os_integration::mcp_test_connection,
            os_integration::object_open_terminal,
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
        // Read every source file so commands in new modules are checked too.
        fn rust_sources(directory: &std::path::Path, sources: &mut Vec<String>) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    rust_sources(&path, sources);
                } else if path.extension().is_some_and(|extension| extension == "rs") {
                    sources.push(std::fs::read_to_string(path).unwrap());
                }
            }
        }
        let mut sources = Vec::new();
        rust_sources(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            &mut sources,
        );
        assert!(
            sources
                .iter()
                .any(|source| source.contains("fn workspace_create(")),
            "the command sources were not found"
        );
        let mut main_thread = Vec::new();
        for source in &sources {
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
