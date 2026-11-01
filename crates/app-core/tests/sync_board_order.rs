//! Board order through sync (seam 1 against the real server): two desktops
//! show one project's board, each moves a card in its window, and the
//! order keys in the task files carry each move to the other board.

mod support;
mod sync_support;

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use app_core::folder_sync::{
    FolderSyncAction, FolderSyncBackend, FolderSyncScreen, WorkspaceEngine,
};
use app_core::sync::registry::Registry;
use app_core::sync::workspace::WorkspaceSync;
use app_core::{App, NoSystem, PluginEvent, PluginMove, Screen, TabId};
use support::{
    open_from_tree,
    plugins::PluginFolder,
    projects::{self, board, project_file, send_panel, with_projects},
    tasks::{A, B, C, H, task_file, with_tasks},
};
use sync_support::{Server, approve, first_desktop};

const PROJECT: &str = "projects/Garden/Garden.md";

/// A backend that keeps the engine of the workspace the window shows.
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

/// One desktop's window on the workspace.
struct Window {
    app: App,
    engine: Arc<WorkspaceEngine>,
    registry: Registry,
    root: PathBuf,
    _plugins: Vec<PluginFolder>,
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

fn wait_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        if done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A window on a folder holding `files`, with Tasks and Projects on, or
/// `None` when they aren't built.
fn window(files: &[(&str, &[u8])]) -> Option<Window> {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("Garden");
    std::fs::create_dir(&root).unwrap();
    for (path, bytes) in files {
        support::write(&root, path, bytes);
    }
    let attached = Arc::new(Attached::default());
    let mut app = App::launch_with(app_data.path(), Arc::new(NoSystem));
    app.set_folder_sync_backend(attached.clone());
    app.open_folder(root.clone());
    wait_until(&mut app, "the workspace opens", |app| {
        !app.is_busy() && app.screen() == &Screen::Workspace
    });
    let engine = attached.engine.lock().unwrap().clone()?;
    let registry = Registry::open(app_data.path()).unwrap();
    let tasks = with_tasks(&mut app)?;
    let projects = with_projects(&mut app)?;
    Some(Window {
        app,
        engine,
        registry,
        root,
        _plugins: vec![tasks, projects],
        _folders: (app_data, parent),
    })
}

/// The board's first column, top to bottom.
fn to_do(app: &App, tab: TabId) -> Vec<String> {
    board(app, tab)
        .into_iter()
        .find(|(id, _, _)| id == "column:todo")
        .map(|(_, _, cards)| cards)
        .unwrap_or_default()
}

fn wait_for_board(app: &mut App, tab: TabId, expected: &[&str]) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        let drawn = app.plugin_panel(tab).and_then(Result::ok).is_some();
        if drawn && to_do(app, tab) == expected {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the board shows {:?}, not {expected:?}; notice {:?}",
            drawn.then(|| to_do(app, tab)),
            support::notice(app)
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn moved(task: &str, position: u32) -> PluginEvent {
    PluginEvent::Moved(PluginMove {
        node: "board".to_owned(),
        item: format!("card:task_{task}"),
        to: "column:todo".to_owned(),
        position,
    })
}

fn task(suffix: &str, order: &str) -> Vec<u8> {
    task_file(
        suffix,
        &format!("status: todo\npriority: medium\nkanban_order: {order}\nproject: project_{H}\n"),
        "2026-10-01T10:00:00Z",
    )
}

fn files_under(root: &Path) -> Vec<(String, Vec<u8>)> {
    let mut found: Vec<(String, Vec<u8>)> = ["tasks/Weed.md", "tasks/Water.md", "tasks/Mulch.md"]
        .into_iter()
        .map(|path| (path.to_owned(), std::fs::read(root.join(path)).unwrap()))
        .collect();
    found.sort();
    found
}

#[test]
fn board_order_carries_between_two_desktops_through_the_task_files() {
    let weed = task(A, "a0");
    let water = task(B, "a1");
    let mulch = task(C, "a2");
    let project = project_file(H, "status: active\n", "Grow vegetables.\n");
    let Some(mut one) = window(&[
        (PROJECT, &project),
        ("tasks/Weed.md", &weed),
        ("tasks/Water.md", &water),
        ("tasks/Mulch.md", &mulch),
    ]) else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let server = runtime.block_on(Server::start());
    let mut laptop = runtime.block_on(first_desktop(&server, "ana@example.com"));
    let mut sync_one = runtime
        .block_on(WorkspaceSync::turn_on(
            &mut laptop.service,
            one.engine.clone(),
            "Garden",
            &mut one.registry,
        ))
        .expect("sync turns on");
    runtime
        .block_on(sync_one.round(&mut laptop.service, 0))
        .expect("a round");
    let mut desktop = runtime.block_on(approve(
        &server,
        &mut laptop,
        std::slice::from_mut(sync_one.remote_mut().expect("connected")),
    ));

    let Some(mut two) = window(&[]) else {
        return;
    };
    let mut sync_two = runtime
        .block_on(WorkspaceSync::download(
            &mut desktop.service,
            two.engine.clone(),
            sync_one.workspace(),
            &mut two.registry,
        ))
        .expect("the download starts");
    runtime
        .block_on(sync_two.round(&mut desktop.service, 0))
        .expect("a round");
    wait_until(&mut two.app, "the project arrives", |_| {
        two.root.join(PROJECT).exists()
    });
    let mut rounds = |one: &mut Window, two: &mut Window| {
        for _ in 0..3 {
            one.app.poll();
            two.app.poll();
            runtime
                .block_on(sync_one.round(&mut laptop.service, 0))
                .expect("a round");
            runtime
                .block_on(sync_two.round(&mut desktop.service, 0))
                .expect("a round");
        }
    };

    let tab_one = open_from_tree(&mut one.app, PROJECT);
    projects::panel_with(&mut one.app, tab_one, "board");
    let tab_two = open_from_tree(&mut two.app, PROJECT);
    projects::panel_with(&mut two.app, tab_two, "board");
    wait_for_board(&mut one.app, tab_one, &["Weed", "Water", "Mulch"]);
    wait_for_board(&mut two.app, tab_two, &["Weed", "Water", "Mulch"]);

    // The first desktop moves Mulch to the top, and the second board
    // follows.
    send_panel(&mut one.app, tab_one, moved(C, 0));
    wait_for_board(&mut one.app, tab_one, &["Mulch", "Weed", "Water"]);
    rounds(&mut one, &mut two);
    wait_for_board(&mut two.app, tab_two, &["Mulch", "Weed", "Water"]);
    assert_eq!(files_under(&one.root), files_under(&two.root));

    // Now the second desktop moves a card, and the first follows.
    send_panel(&mut two.app, tab_two, moved(B, 1));
    wait_for_board(&mut two.app, tab_two, &["Mulch", "Water", "Weed"]);
    rounds(&mut one, &mut two);
    wait_for_board(&mut one.app, tab_one, &["Mulch", "Water", "Weed"]);
    assert_eq!(files_under(&one.root), files_under(&two.root));

    // Each desktop moves its own card before either syncs. The order keys
    // sit in separate files, so both moves land.
    send_panel(&mut one.app, tab_one, moved(A, 0));
    wait_for_board(&mut one.app, tab_one, &["Weed", "Mulch", "Water"]);
    send_panel(&mut two.app, tab_two, moved(C, 2));
    wait_for_board(&mut two.app, tab_two, &["Water", "Weed", "Mulch"]);
    rounds(&mut one, &mut two);

    assert_eq!(files_under(&one.root), files_under(&two.root));
    // Weed sorts first from the first desktop's move, and Mulch last from
    // the second's.
    wait_for_board(&mut one.app, tab_one, &["Weed", "Water", "Mulch"]);
    wait_for_board(&mut two.app, tab_two, &["Weed", "Water", "Mulch"]);

    drop((one, two));
    drop((sync_one, sync_two));
    runtime.block_on(async move {
        drop(desktop);
        drop(laptop);
        drop(server);
    });
}
