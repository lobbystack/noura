//! Seam 1: the sidebar and the file tree's open folders across launches.

use std::{
    path::Path,
    time::{Duration, Instant},
};

use app_core::{App, DropSpot, OpenAs, SIDEBAR_MAX_WIDTH, SIDEBAR_MIN_WIDTH, SPRING_OPEN_DELAY};
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

/// Launch on `app_data` and wait for the last workspace to reopen.
fn launch(app_data: &Path) -> App {
    let mut app = App::launch(app_data);
    settle(&mut app);
    app
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

#[test]
fn the_sidebar_starts_shown_at_its_narrowest_width_with_every_section_open() {
    let app_data = tempfile::tempdir().unwrap();

    let app = App::launch(app_data.path());

    assert!(app.layout().sidebar_shown);
    assert_eq!(app.layout().sidebar_width, SIDEBAR_MIN_WIDTH);
    assert_eq!(SIDEBAR_MIN_WIDTH, 224.0);
    assert!(!app.is_section_collapsed("files"));
}

#[test]
fn the_sidebar_comes_back_as_you_left_it_on_this_device() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());

    app.toggle_sidebar();
    app.set_sidebar_width(312.0);
    app.toggle_section("files");
    drop(app);
    let app = launch(app_data.path());

    assert!(!app.layout().sidebar_shown);
    assert_eq!(app.layout().sidebar_width, 312.0);
    assert!(app.is_section_collapsed("files"));
}

#[test]
fn the_sidebar_is_never_narrower_than_224_points() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());

    app.set_sidebar_width(100.0);
    assert_eq!(app.layout().sidebar_width, 224.0);
    app.set_sidebar_width(f32::NAN);
    assert_eq!(app.layout().sidebar_width, 224.0);
}

#[test]
fn the_sidebar_is_never_wider_than_480_points() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());

    app.set_sidebar_width(10_000.0);
    assert_eq!(app.layout().sidebar_width, 480.0);
    assert_eq!(SIDEBAR_MAX_WIDTH, 480.0);
    app.set_sidebar_width(f32::INFINITY);
    assert_eq!(app.layout().sidebar_width, 480.0);
}

#[test]
fn a_stored_width_out_of_range_comes_back_in_range() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());
    app.set_sidebar_width(300.0);
    drop(app);
    let stored = app_data.path().join("layout.json");
    let text = std::fs::read_to_string(&stored).unwrap();
    assert!(text.contains("300"), "{text}");

    std::fs::write(&stored, text.replace("300", "100000")).unwrap();
    assert_eq!(launch(app_data.path()).layout().sidebar_width, 480.0);
    std::fs::write(&stored, text.replace("300", "-5")).unwrap();
    assert_eq!(launch(app_data.path()).layout().sidebar_width, 224.0);
}

#[test]
fn an_unreadable_layout_falls_back_to_the_defaults() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());
    app.toggle_sidebar();
    drop(app);
    for entry in std::fs::read_dir(app_data.path()).unwrap() {
        let path = entry.unwrap().path();
        if path
            .extension()
            .is_some_and(|extension| extension == "json")
            && std::fs::read_to_string(&path).unwrap().contains("sidebar")
        {
            std::fs::write(&path, b"[1, 2").unwrap();
        }
    }

    let app = App::launch(app_data.path());

    assert!(app.layout().sidebar_shown);
}

/// The row showing `relative_path`.
fn row(app: &App, relative_path: &str) -> usize {
    app.tree()
        .unwrap()
        .find_row(relative_path)
        .unwrap_or_else(|| panic!("no row for {relative_path}"))
}

fn expanded(app: &App, relative_path: &str) -> Option<bool> {
    app.tree().unwrap().rows()[row(app, relative_path)].expanded
}

/// Open or close the folder at `relative_path`, as a click does.
fn click(app: &mut App, relative_path: &str) {
    let row = row(app, relative_path);
    app.click_tree_row(row, OpenAs::Preview);
}

#[test]
fn open_folders_come_back_for_each_workspace() {
    let (app_data, _folder, mut app) = opened(&["Docs/Deep/A.md", "Notes/B.md"]);
    click(&mut app, "Docs");
    click(&mut app, "Docs/Deep");
    let second = tempfile::tempdir().unwrap();
    write(second.path(), "Docs/C.md", b"x");
    app.open_folder(second.path().to_owned());
    settle(&mut app);
    assert_eq!(expanded(&app, "Docs"), Some(false));
    drop(app);

    let mut app = launch(app_data.path());
    assert_eq!(expanded(&app, "Docs"), Some(false));
    let first = app.recents()[1].workspace_id.clone();
    app.open_recent(&first);
    settle(&mut app);

    assert_eq!(expanded(&app, "Docs"), Some(true));
    assert_eq!(expanded(&app, "Docs/Deep"), Some(true));
    assert_eq!(expanded(&app, "Notes"), Some(false));
}

#[test]
fn the_keyboard_opens_folders_that_come_back_too() {
    let (app_data, _folder, mut app) = opened(&["Docs/A.md"]);
    app.tree_mut().unwrap().set_focus(0);

    app.press_tree_key(app_core::TreeKey::Right);
    drop(app);
    let app = launch(app_data.path());

    assert_eq!(expanded(&app, "Docs"), Some(true));
}

#[test]
fn open_folders_stay_open_through_renames_and_moves() {
    let (app_data, _folder, mut app) = opened(&["Docs/Deep/A.md", "Archive/Old.md"]);
    click(&mut app, "Docs");
    click(&mut app, "Docs/Deep");

    app.start_rename("Docs");
    assert!(app.commit_rename("Guides"));
    assert_eq!(expanded(&app, "Guides"), Some(true));
    assert_eq!(expanded(&app, "Guides/Deep"), Some(true));
    drop(app);
    let mut app = launch(app_data.path());
    assert_eq!(expanded(&app, "Guides"), Some(true));
    assert_eq!(expanded(&app, "Guides/Deep"), Some(true));

    // A move opens the folder it moved into, and that lasts too.
    app.move_into("Guides", "Archive");
    drop(app);
    let app = launch(app_data.path());

    assert_eq!(expanded(&app, "Archive"), Some(true));
    assert_eq!(expanded(&app, "Archive/Guides"), Some(true));
    assert_eq!(expanded(&app, "Archive/Guides/Deep"), Some(true));
}

#[test]
fn collapse_all_lasts_after_a_relaunch() {
    let (app_data, _folder, mut app) = opened(&["Docs/Deep/A.md", "Notes/B.md"]);
    click(&mut app, "Docs");
    click(&mut app, "Docs/Deep");
    click(&mut app, "Notes");

    app.collapse_all();
    drop(app);
    let app = launch(app_data.path());

    assert_eq!(expanded(&app, "Docs"), Some(false));
    assert_eq!(expanded(&app, "Notes"), Some(false));
    assert_eq!(app.tree().unwrap().rows().len(), 2);
}

#[test]
fn selecting_a_new_item_opens_its_folders_and_that_lasts() {
    let (app_data, _folder, mut app) = opened(&["Docs/Deep/A.md"]);
    click(&mut app, "Docs");
    click(&mut app, "Docs/Deep");
    let deep = row(&app, "Docs/Deep");
    app.tree_mut().unwrap().set_focus(deep);

    // New folder selects the folder it makes, inside the current folder.
    app.new_folder(None);
    app.cancel_rename();
    click(&mut app, "Docs");
    app.duplicate("Docs/Deep/A.md");
    drop(app);
    let app = launch(app_data.path());

    assert_eq!(expanded(&app, "Docs"), Some(true));
    assert_eq!(expanded(&app, "Docs/Deep"), Some(true));
}

/// Rest a drag of `dragged` on the closed folder `folder` until it opens.
fn spring_open(app: &mut App, dragged: &str, folder: &str) {
    app.start_tree_drag(row(app, dragged));
    let start = Instant::now();
    let over = DropSpot::Row(row(app, folder));
    app.drag_tree_over(over, start);
    app.drag_tree_over(over, start + SPRING_OPEN_DELAY);
    assert_eq!(expanded(app, folder), Some(true));
}

#[test]
fn a_folder_a_drop_lands_in_stays_open_after_a_relaunch() {
    let (app_data, folder, mut app) = opened(&["Plan.md", "Archive/Old.md", "Notes/B.md"]);

    spring_open(&mut app, "Plan.md", "Archive");
    app.finish_tree_drag();
    assert!(folder.path().join("Archive/Plan.md").is_file());
    drop(app);
    let app = launch(app_data.path());

    assert_eq!(expanded(&app, "Archive"), Some(true));
    assert_eq!(expanded(&app, "Notes"), Some(false));
}

#[test]
fn folders_a_cancelled_drag_opened_stay_closed_after_a_relaunch() {
    let (app_data, folder, mut app) = opened(&["Plan.md", "Archive/Old.md"]);

    spring_open(&mut app, "Plan.md", "Archive");
    app.cancel_tree_drag();
    assert_eq!(expanded(&app, "Archive"), Some(false));
    drop(app);
    let app = launch(app_data.path());

    assert!(folder.path().join("Plan.md").is_file());
    assert_eq!(expanded(&app, "Archive"), Some(false));
}
