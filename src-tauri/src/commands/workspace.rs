//! Opening, creating, and closing workspaces, and the recent-workspace list
//! the app reopens at launch.

use std::{path::Path, time::Duration};

use local_core::{CoreError, CoreEvent, ErrorCategory, WorkspaceEngine, WorkspaceState};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_dialog::DialogExt;

use crate::{AppState, current_engine, install_engine, state_unavailable};

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentWorkspace {
    path: String,
    name: String,
    workspace_id: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateWorkspaceInput {
    path: String,
    name: String,
}

#[derive(Deserialize)]
pub struct OpenWorkspaceInput {
    path: String,
}

fn recent_path(app: &AppHandle) -> Result<std::path::PathBuf, CoreError> {
    app.path()
        .app_local_data_dir()
        .map(|path| path.join("recent-workspaces.json"))
        .map_err(|_| {
            CoreError::new(
                "app_data_unavailable",
                ErrorCategory::Filesystem,
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
        return Err(CoreError::new(
            "workspace_identity_changed",
            ErrorCategory::Identity,
            "The last workspace is no longer at its saved location",
            "workspace_restore",
        ));
    }
    Ok(Some(engine))
}

pub(crate) fn save_recent(app: &AppHandle, workspace: &WorkspaceEngine) -> Result<(), CoreError> {
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
        CoreError::new(
            "recent_serialize_failed",
            ErrorCategory::Parse,
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
pub fn workspace_create(
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
pub fn workspace_open(
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
        return Err(CoreError::new(
            "workspace_identity_changed",
            ErrorCategory::Identity,
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
pub fn workspace_close(app: AppHandle, state: State<AppState>) -> Result<(), CoreError> {
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
                source: local_core::EventSource::Application,
                payload: serde_json::json!({}),
            },
        )
    {
        log::warn!("could not announce the closed workspace: {error}");
    }
    Ok(())
}

#[tauri::command(async)]
pub fn workspace_state(state: State<AppState>) -> Result<WorkspaceState, CoreError> {
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
pub fn workspace_rebuild_index(state: State<AppState>) -> Result<WorkspaceState, CoreError> {
    let engine = current_engine(&state, "workspace_rebuild_index")?;
    engine.rebuild_index()?;
    Ok(engine.state())
}

#[tauri::command(async)]
pub fn workspace_list_recent(app: AppHandle) -> Vec<RecentWorkspace> {
    load_recent(&app)
}

#[tauri::command(async)]
pub fn workspace_forget_recent(
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
pub async fn workspace_pick_folder(
    app: AppHandle,
    title: String,
) -> Result<Option<String>, CoreError> {
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

/// Reopen the last workspace after the window shows. The frontend shows its
/// loading state meanwhile, and `workspace_state` waits for the result, so
/// the chooser never flashes before a restored workspace.
pub(crate) fn restore_last_workspace_in_background(app: AppHandle) {
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
pub(crate) fn watch_workspaces(app: AppHandle) {
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

#[cfg(test)]
mod workspace_restore_tests {
    use super::*;
    use std::path::PathBuf;

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
