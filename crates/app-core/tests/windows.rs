//! Seam 1: several windows over one workspace session.

use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

use app_core::{App, Change, OpenAs, Screen, TabId};
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

fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// A workspace holding `files`, opened in a fresh app.
fn opened(files: &[&str]) -> (TempDir, TempDir, App) {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    for file in files {
        write(folder.path(), file, b"x");
    }
    let mut app = App::launch(app_data.path());
    app.open_folder(folder.path().to_owned());
    settle(&mut app);
    (app_data, folder, app)
}

/// Each tab's title, in order.
fn titles(app: &App) -> Vec<String> {
    app.tabs()
        .iter()
        .map(|tab| tab.title().to_owned())
        .collect()
}

fn tab_id(app: &App, title: &str) -> TabId {
    app.tabs()
        .iter()
        .find(|tab| tab.title() == title)
        .unwrap_or_else(|| panic!("no tab named {title}"))
        .id()
}

/// The tree's visible rows, by workspace path.
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

/// Every change `app` announces from now on.
fn changes(app: &App) -> Receiver<Change> {
    let (sender, receiver) = channel();
    app.subscribe(move |change| {
        let _ = sender.send(change);
    });
    receiver
}

#[test]
fn a_new_window_opens_on_the_open_workspace_with_its_own_tabs() {
    let (_app_data, _folder, mut first) = opened(&["A.md", "B.md"]);
    first.open_file("A.md", OpenAs::Kept);

    let mut second = first.new_window().expect("a workspace is open");

    assert_eq!(second.screen(), &Screen::Workspace);
    assert_eq!(
        second.workspace().unwrap().id(),
        first.workspace().unwrap().id()
    );
    assert!(second.tabs().is_empty());

    second.open_file("B.md", OpenAs::Kept);
    first.close_tab(tab_id(&first, "A"));
    first.poll();
    second.poll();

    assert!(first.tabs().is_empty());
    assert_eq!(titles(&second), ["B"]);
}

/// Today's "same note in two browser tabs" check, in two windows.
#[test]
fn files_one_window_creates_renames_or_deletes_show_in_the_other_without_a_refresh() {
    let (_app_data, _folder, mut first) = opened(&["Plan.md", "Docs/Guide.md"]);
    let mut second = first.new_window().unwrap();
    second.open_file("Plan.md", OpenAs::Kept);
    let woken = changes(&second);

    first.new_note(Some(""));
    let created = first.document().unwrap().relative_path.clone();
    assert!(
        woken.try_iter().count() > 0,
        "the other window wasn't woken"
    );
    second.poll();
    assert_eq!(rows(&second), ["Docs", "Plan.md", created.as_str()]);

    first.start_rename("Plan.md");
    assert!(first.commit_rename("Brief"));
    second.poll();
    assert_eq!(rows(&second), ["Docs", "Brief.md", created.as_str()]);
    assert_eq!(titles(&second), ["Brief"]);
    assert_eq!(second.document().unwrap().relative_path, "Brief.md");

    first.request_trash("Docs");
    first.confirm_trash();
    second.poll();
    assert_eq!(rows(&second), ["Brief.md", created.as_str()]);
    assert_eq!(rows(&first), rows(&second));
}

/// The windows share one open workspace, and so its one index and one
/// watcher. One rescan after an outside change, as the watcher runs,
/// brings every window up to date.
#[test]
fn every_window_shares_one_index_and_one_outside_change_updates_them_all() {
    let (_app_data, folder, mut first) = opened(&["A.md"]);
    let mut second = first.new_window().unwrap();
    let mut third = second.new_window().unwrap();
    let shared = first.workspace().unwrap();
    assert!(std::ptr::eq(shared, second.workspace().unwrap()));
    assert!(std::ptr::eq(shared, third.workspace().unwrap()));

    write(folder.path(), "From outside.md", b"x");
    second.rescan();
    first.poll();
    third.poll();

    for window in [&first, &second, &third] {
        assert_eq!(rows(window), ["A.md", "From outside.md"]);
    }
}

#[test]
fn close_window_closes_that_window_alone_and_the_others_keep_their_tabs() {
    let (_app_data, _folder, mut first) = opened(&["A.md", "B.md"]);
    first.open_file("A.md", OpenAs::Kept);
    let mut second = first.new_window().unwrap();
    second.open_file("B.md", OpenAs::Kept);

    second.close_window();
    first.poll();

    assert!(second.is_closed());
    assert!(!first.is_closed());
    assert_eq!(first.screen(), &Screen::Workspace);
    assert_eq!(titles(&first), ["A"]);
}

/// Quit by dropping every window, then launch again on the same app
/// data. Returns the first window and the others the launch brought back.
fn relaunch(app_data: &Path, windows: Vec<App>) -> (App, Vec<App>) {
    drop(windows);
    let mut app = App::launch(app_data);
    settle(&mut app);
    let others = app.take_opened_windows();
    (app, others)
}

#[test]
fn quitting_with_two_windows_open_brings_both_back_with_their_tabs() {
    let (app_data, _folder, mut first) = opened(&["A.md", "B.md", "C.md"]);
    first.open_file("A.md", OpenAs::Kept);
    let mut second = first.new_window().unwrap();
    second.open_file("B.md", OpenAs::Kept);
    second.open_file("C.md", OpenAs::Kept);
    second.activate_tab(tab_id(&second, "B"));

    let (first, others) = relaunch(app_data.path(), vec![first, second]);

    assert_eq!(first.screen(), &Screen::Workspace);
    assert_eq!(titles(&first), ["A"]);
    assert_eq!(others.len(), 1);
    assert_eq!(titles(&others[0]), ["B", "C"]);
    assert_eq!(others[0].active_tab().unwrap().title(), "B");
    assert!(std::ptr::eq(
        first.workspace().unwrap(),
        others[0].workspace().unwrap()
    ));
}

#[test]
fn a_window_closed_before_quitting_stays_closed() {
    let (app_data, _folder, mut first) = opened(&["A.md", "B.md", "C.md"]);
    first.open_file("A.md", OpenAs::Kept);
    let mut second = first.new_window().unwrap();
    second.open_file("B.md", OpenAs::Kept);
    let mut third = second.new_window().unwrap();
    third.open_file("C.md", OpenAs::Kept);

    second.close_window();
    let (first, others) = relaunch(app_data.path(), vec![first, second, third]);

    assert_eq!(titles(&first), ["A"]);
    let others: Vec<_> = others.iter().map(titles).collect();
    assert_eq!(others, [["C"]]);
}

/// Every file under `root` with its bytes, in path order.
fn snapshot(root: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else {
                files.push((path.clone(), std::fs::read(&path).unwrap()));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn window_state_lives_in_app_data_and_leaves_the_workspace_folder_unchanged() {
    let (app_data, folder, mut first) = opened(&["A.md", "Docs/B.md", "Docs/C.md"]);
    let before = snapshot(folder.path());
    first.open_file("A.md", OpenAs::Kept);
    let mut second = first.new_window().unwrap();
    second.open_file("Docs/B.md", OpenAs::Kept);
    second.open_file("Docs/C.md", OpenAs::Preview);
    let mut third = second.new_window().unwrap();
    third.open_file("A.md", OpenAs::Kept);
    third.close_window();

    let (first, others) = relaunch(app_data.path(), vec![first, second, third]);
    drop((first, others));

    assert_eq!(snapshot(folder.path()), before, "the workspace changed");
    let stored: Vec<_> = snapshot(app_data.path())
        .into_iter()
        .filter(|(path, bytes)| {
            path.extension()
                .is_some_and(|extension| extension == "json")
                && String::from_utf8_lossy(bytes).contains("Docs/C.md")
        })
        .collect();
    assert_eq!(stored.len(), 1, "one view state file holds the windows");
}

#[test]
fn opening_a_different_workspace_from_any_window_closes_the_other_windows() {
    let (_app_data, _folder, mut first) = opened(&["A.md"]);
    let other = tempfile::tempdir().unwrap();
    write(other.path(), "Other.md", b"x");
    let mut second = first.new_window().unwrap();
    let mut third = first.new_window().unwrap();
    let woken = changes(&third);

    second.open_folder(other.path().to_owned());
    settle(&mut second);

    assert!(
        woken.try_iter().count() > 0,
        "the closing window wasn't woken"
    );
    assert!(first.is_closed());
    assert!(third.is_closed());
    assert!(first.poll());
    assert!(third.poll());
    assert!(third.tabs().is_empty());
    assert!(!second.is_closed());
    assert_eq!(rows(&second), ["Other.md"]);
    assert!(second.take_opened_windows().is_empty());
}

#[test]
fn a_workspace_that_fails_to_open_leaves_every_window_open() {
    let (_app_data, _folder, mut first) = opened(&["A.md"]);
    let mut second = first.new_window().unwrap();
    let missing = tempfile::tempdir().unwrap().path().join("Gone");

    second.open_folder(missing);
    settle(&mut second);
    first.poll();

    assert!(!first.is_closed());
    assert!(!second.is_closed());
    assert!(second.notice().is_some());
    assert_eq!(rows(&first), ["A.md"]);
}

/// A workspace's windows stay saved when another workspace replaces it,
/// and come back with it.
#[test]
fn a_workspace_opened_again_brings_back_the_windows_it_had() {
    let (_app_data, _folder, mut first) = opened(&["A.md", "B.md"]);
    let workspace_id = first.workspace().unwrap().id().to_owned();
    first.open_file("A.md", OpenAs::Kept);
    let mut second = first.new_window().unwrap();
    second.open_file("B.md", OpenAs::Kept);
    let other = tempfile::tempdir().unwrap();
    first.open_folder(other.path().to_owned());
    settle(&mut first);
    assert!(second.is_closed());

    first.open_recent(&workspace_id);
    settle(&mut first);

    assert_eq!(titles(&first), ["A"]);
    let others: Vec<_> = first.take_opened_windows().iter().map(titles).collect();
    assert_eq!(others, [["B"]]);
}

#[test]
fn opening_the_open_workspace_again_keeps_every_window_on_it() {
    let (_app_data, folder, mut first) = opened(&["A.md"]);
    first.open_file("A.md", OpenAs::Kept);
    let mut second = first.new_window().unwrap();

    second.open_folder(folder.path().to_owned());
    settle(&mut second);
    first.poll();

    assert!(!first.is_closed());
    assert!(!second.is_closed());
    assert_eq!(titles(&first), ["A"]);
    assert!(std::ptr::eq(
        first.workspace().unwrap(),
        second.workspace().unwrap()
    ));
}

/// The rail's page titles, top to bottom.
fn rail(app: &App) -> Vec<String> {
    app.rail().iter().map(|page| page.title.clone()).collect()
}

#[test]
fn the_windows_share_the_rail_order_and_its_pages() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let mut first = App::launch(app_data.path());
    first.register_page("test-alpha", "Alpha");
    first.open_folder(folder.path().to_owned());
    settle(&mut first);
    let mut second = first.new_window().unwrap();
    assert_eq!(rail(&second), ["Files", "Alpha"]);

    second.move_page("test-alpha", 0);
    first.poll();

    assert_eq!(rail(&first), ["Alpha", "Files"]);
    assert_eq!(rail(&second), ["Alpha", "Files"]);
}

/// Closing the last window quits, so it comes back at the next launch.
#[test]
fn the_last_window_closed_comes_back_at_the_next_launch() {
    let (app_data, _folder, mut first) = opened(&["A.md", "B.md"]);
    let mut second = first.new_window().unwrap();
    second.open_file("B.md", OpenAs::Kept);

    first.close_window();
    second.close_window();
    let (relaunched, others) = relaunch(app_data.path(), vec![first, second]);

    assert_eq!(titles(&relaunched), ["B"]);
    assert!(others.is_empty());
}
