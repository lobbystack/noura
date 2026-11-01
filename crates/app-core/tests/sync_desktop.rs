//! The open workspace's sync in the app (seam 1 against the real server,
//! sync-v2.md §15): files another desktop syncs reach the open window's
//! file tree and the index without a restart, through the workspace engine
//! the windows share.

mod sync_support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use app_core::folder_sync::{
    FolderSyncAction, FolderSyncBackend, FolderSyncScreen, WorkspaceEngine,
};
use app_core::sync::registry::Registry;
use app_core::sync::workspace::WorkspaceSync;
use app_core::{App, NoSystem, Screen};
use sync_support::{Folder, Server, approve, first_desktop};

/// A backend that keeps the engine of the workspace the windows show.
#[derive(Default)]
struct Attached {
    engine: Mutex<Option<Arc<WorkspaceEngine>>>,
}

impl FolderSyncBackend for Attached {
    fn screen(&self) -> FolderSyncScreen {
        FolderSyncScreen::default()
    }
    fn act(&self, _action: FolderSyncAction) {}
    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}
    fn attach(&self, engine: Option<Arc<WorkspaceEngine>>) {
        *self.engine.lock().unwrap() = engine;
    }
}

fn rows(app: &App) -> Vec<String> {
    app.tree()
        .map(|tree| {
            tree.rows()
                .iter()
                .map(|row| row.relative_path.clone())
                .collect()
        })
        .unwrap_or_default()
}

fn wait_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        app.poll();
        if done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

const NOTE: &[u8] = b"---\nid: note_01k00000000000000000000009\ntype: note\n---\n\n# Plan\n";

// The app keeps runtimes of its own, so the test drives the server and the
// sync calls on a runtime it holds outside them.
#[test]
fn files_another_desktop_syncs_show_in_the_open_window() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let server = runtime.block_on(Server::start());
    let mut laptop = runtime.block_on(first_desktop(&server, "ana@example.com"));
    let mut a = Folder::new("Field notes");
    a.write("Plan.md", NOTE);
    let mut sync_a = runtime
        .block_on(WorkspaceSync::turn_on(
            &mut laptop.service,
            a.engine.clone(),
            "Field notes",
            &mut a.registry,
        ))
        .expect("sync turns on");
    runtime
        .block_on(sync_a.round(&mut laptop.service, 0))
        .expect("a round");
    let mut desktop = runtime.block_on(approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_a.remote_mut().expect("connected")),
    ));

    // The desktop's window opens an empty folder, and sync downloads into
    // the engine the window shows.
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Field notes");
    std::fs::create_dir(&folder).unwrap();
    let attached = Arc::new(Attached::default());
    let mut app = App::launch_with(app_data.path(), Arc::new(NoSystem));
    app.set_folder_sync_backend(attached.clone());
    app.open_folder(folder.clone());
    wait_until(&mut app, "the workspace opens", |app| {
        !app.is_busy() && app.screen() == &Screen::Workspace
    });
    let engine = attached
        .engine
        .lock()
        .unwrap()
        .clone()
        .expect("the window's workspace reached the backend");
    let mut registry = Registry::open(app_data.path()).unwrap();
    let mut sync_b = runtime
        .block_on(WorkspaceSync::download(
            &mut desktop.service,
            engine,
            sync_a.workspace(),
            &mut registry,
        ))
        .expect("the download starts");
    runtime
        .block_on(sync_b.round(&mut desktop.service, 0))
        .expect("a round");
    wait_until(&mut app, "the synced note shows", |app| {
        rows(app) == ["Plan.md"]
    });
    let indexed = app
        .workspace()
        .unwrap()
        .objects()
        .unwrap()
        .into_iter()
        .any(|o| o.id == "note_01k00000000000000000000009");
    assert!(indexed, "the index holds the synced note");

    // A later change from the laptop shows too.
    a.write("Ideas.md", b"# Ideas\n");
    runtime
        .block_on(sync_a.round(&mut laptop.service, 0))
        .expect("a round");
    runtime
        .block_on(sync_b.round(&mut desktop.service, 0))
        .expect("a round");
    wait_until(&mut app, "the second file shows", |app| {
        rows(app) == ["Ideas.md", "Plan.md"]
    });
    drop(app);
    drop(sync_b);
    drop(sync_a);
    runtime.block_on(async move {
        drop(desktop);
        drop(laptop);
        drop(server);
    });
}
