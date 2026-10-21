//! Seam 1: the rail's pages and their order across launches.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use app_core::{App, FILES_PAGE, INBOX_PAGE};
use tempfile::TempDir;

/// Poll until background work finishes.
fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        if !app.is_busy() {
            return;
        }
        assert!(Instant::now() < deadline, "background work never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Until plugins (ticket 24) add pages, tests register their own.
fn with_pages(app: &mut App) {
    app.register_page("test-alpha", "Alpha");
    app.register_page("test-beta", "Beta");
}

/// An empty workspace opened in a fresh app with the test pages.
fn opened() -> (TempDir, TempDir, App) {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());
    with_pages(&mut app);
    app.open_folder(folder.path().to_owned());
    settle(&mut app);
    (app_data, folder, app)
}

/// Quit by dropping `app`, then launch again with the pages `register`
/// adds.
fn relaunch(app: App, app_data: &Path, register: impl FnOnce(&mut App)) -> App {
    drop(app);
    launch(app_data, register)
}

/// Launch on `app_data` with the pages `register` adds, and wait for the
/// last workspace to reopen.
fn launch(app_data: &Path, register: impl FnOnce(&mut App)) -> App {
    let mut app = App::launch(app_data);
    register(&mut app);
    settle(&mut app);
    app
}

/// The rail's page titles, top to bottom.
fn rail(app: &App) -> Vec<String> {
    app.rail().iter().map(|page| page.title.clone()).collect()
}

/// The Inbox is a kernel page: it's there whatever plugins add pages.
#[test]
fn the_inbox_and_files_are_in_the_rail_before_any_page_registers() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());
    app.open_folder(folder.path().to_owned());
    settle(&mut app);

    assert_eq!(rail(&app), ["Inbox", "Files"]);
}

#[test]
fn the_rail_shows_files_and_the_pages_registered_with_the_inbox_first() {
    let (_app_data, _folder, app) = opened();

    assert_eq!(rail(&app), ["Inbox", "Files", "Alpha", "Beta"]);
}

#[test]
fn a_moved_page_keeps_its_place_in_the_workspace_after_a_relaunch() {
    let (app_data, _folder, mut app) = opened();

    app.move_page("test-beta", 0);
    assert_eq!(rail(&app), ["Beta", "Inbox", "Files", "Alpha"]);
    app.move_page(INBOX_PAGE, 3);
    assert_eq!(rail(&app), ["Beta", "Files", "Alpha", "Inbox"]);
    let app = relaunch(app, app_data.path(), with_pages);

    assert_eq!(rail(&app), ["Beta", "Files", "Alpha", "Inbox"]);
}

#[test]
fn alt_up_and_alt_down_move_a_page_one_place() {
    let (_app_data, _folder, mut app) = opened();

    app.move_page_by(FILES_PAGE, -1);
    assert_eq!(rail(&app), ["Files", "Inbox", "Alpha", "Beta"]);
    app.move_page_by(FILES_PAGE, -1);
    assert_eq!(rail(&app), ["Files", "Inbox", "Alpha", "Beta"]);
    app.move_page_by("test-alpha", 1);
    assert_eq!(rail(&app), ["Files", "Inbox", "Beta", "Alpha"]);
    app.move_page_by("test-alpha", 1);
    assert_eq!(rail(&app), ["Files", "Inbox", "Beta", "Alpha"]);
}

#[test]
fn each_workspace_keeps_its_own_order() {
    let (app_data, _folder, mut app) = opened();
    app.move_page("test-beta", 0);
    let second = tempfile::tempdir().unwrap();

    app.open_folder(second.path().to_owned());
    settle(&mut app);
    assert_eq!(rail(&app), ["Inbox", "Files", "Alpha", "Beta"]);
    let app = relaunch(app, app_data.path(), with_pages);

    assert_eq!(rail(&app), ["Inbox", "Files", "Alpha", "Beta"]);
}

#[test]
fn pages_that_appear_later_join_the_end_without_losing_the_saved_order() {
    let (app_data, _folder, mut app) = opened();
    app.move_page("test-beta", 0);

    // A plugin's page that isn't there at the next launch leaves the
    // others in their order, and comes back to its place later.
    let mut app = relaunch(app, app_data.path(), |app| {
        app.register_page("test-alpha", "Alpha");
    });
    assert_eq!(rail(&app), ["Inbox", "Files", "Alpha"]);
    app.register_page("test-gamma", "Gamma");
    app.register_page("test-beta", "Beta");

    assert_eq!(rail(&app), ["Beta", "Inbox", "Files", "Alpha", "Gamma"]);
}

#[test]
fn an_unreadable_saved_order_falls_back_to_the_default() {
    let (app_data, _folder, mut app) = opened();
    app.move_page("test-beta", 0);
    drop(app);
    for entry in walk(app_data.path()) {
        if entry
            .extension()
            .is_some_and(|extension| extension == "json")
            && std::fs::read_to_string(&entry)
                .unwrap()
                .contains("test-beta")
        {
            std::fs::write(&entry, b"\"railOrder\": ").unwrap();
        }
    }

    let app = launch(app_data.path(), with_pages);

    assert_eq!(app.screen(), &app_core::Screen::Workspace);
    assert_eq!(rail(&app), ["Inbox", "Files", "Alpha", "Beta"]);
}

/// Every file under `root`.
fn walk(root: &Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}
