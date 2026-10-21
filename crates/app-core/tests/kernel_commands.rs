//! Seam 1: what the kernel's commands do in an open workspace.

mod support;

use std::time::Instant;

use app_core::{App, CommandSource, FILES_SECTION, OpenAs, command_id};
use support::{Fixture, document};

fn run(app: &mut App, id: &str) {
    assert!(
        app.run_command(id, CommandSource::Palette, Instant::now()),
        "{id} didn't run"
    );
}

fn active(app: &App) -> Option<String> {
    app.active_tab().map(|tab| tab.title().to_owned())
}

#[test]
fn next_and_previous_tab_go_round_the_open_tabs() {
    let fixture = Fixture::new(&[("A.md", b"a"), ("B.md", b"b"), ("C.md", b"c")]);
    let mut app = fixture.app();
    for path in ["A.md", "B.md", "C.md"] {
        app.open_file(path, OpenAs::Kept);
    }
    assert_eq!(active(&app).as_deref(), Some("C"));

    run(&mut app, command_id::NEXT_TAB);
    assert_eq!(active(&app).as_deref(), Some("A"));
    run(&mut app, command_id::PREVIOUS_TAB);
    assert_eq!(active(&app).as_deref(), Some("C"));
    run(&mut app, command_id::PREVIOUS_TAB);
    assert_eq!(active(&app).as_deref(), Some("B"));
    assert_eq!(document(&app).as_deref(), Some("B.md"));
}

#[test]
fn close_tab_closes_the_active_tab() {
    let fixture = Fixture::new(&[("A.md", b"a"), ("B.md", b"b")]);
    let mut app = fixture.app();
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Kept);

    run(&mut app, command_id::CLOSE_TAB);
    assert_eq!(active(&app).as_deref(), Some("A"));
    run(&mut app, command_id::CLOSE_TAB);
    assert!(app.tabs().is_empty());
    // With no tab open, it does nothing.
    run(&mut app, command_id::CLOSE_TAB);
    assert!(app.tabs().is_empty());
}

#[test]
fn toggle_sidebar_shows_and_hides_the_sidebar() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    assert!(app.layout().sidebar_shown);

    run(&mut app, command_id::TOGGLE_SIDEBAR);
    assert!(!app.layout().sidebar_shown);
    run(&mut app, command_id::TOGGLE_SIDEBAR);
    assert!(app.layout().sidebar_shown);
}

#[test]
fn new_folder_shows_the_folded_or_hidden_tree_so_you_can_name_the_folder() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    app.toggle_sidebar();
    app.toggle_section(FILES_SECTION);

    run(&mut app, command_id::NEW_FOLDER);

    assert!(app.layout().sidebar_shown);
    assert!(!app.is_section_collapsed(FILES_SECTION));
    let renaming = app.renaming().expect("the new folder opens for naming");
    assert_eq!(renaming.relative_path, "Untitled");
    assert!(fixture.root.join("Untitled").is_dir());
}

#[test]
fn new_note_creates_a_note_and_opens_it() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();

    run(&mut app, command_id::NEW_NOTE);

    let opened = document(&app).expect("the new note is open");
    assert!(fixture.exists(&opened));
}

#[test]
fn rename_file_renames_the_open_file_in_the_tree() {
    let fixture = Fixture::new(&[("Notes/Plan.md", b"plan")]);
    let mut app = fixture.app();
    app.open_file("Notes/Plan.md", OpenAs::Kept);
    app.toggle_sidebar();

    run(&mut app, command_id::RENAME_FILE);

    assert!(app.layout().sidebar_shown);
    let renaming = app.renaming().expect("the open file is being renamed");
    assert_eq!(renaming.relative_path, "Notes/Plan.md");
    assert_eq!(renaming.name, "Plan");
    assert!(app.commit_rename("Roadmap"));
    assert!(fixture.exists("Notes/Roadmap.md"));
    assert_eq!(document(&app).as_deref(), Some("Notes/Roadmap.md"));
}

#[test]
fn rename_file_does_nothing_without_an_open_file() {
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();

    run(&mut app, command_id::RENAME_FILE);

    assert_eq!(app.renaming(), None);
}

#[test]
fn close_window_asks_the_window_to_close_once() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    assert!(!app.take_close_window());

    run(&mut app, command_id::CLOSE_WINDOW);

    assert!(app.take_close_window());
    assert!(!app.take_close_window());
}

#[test]
fn keyboard_shortcuts_opens_the_shortcuts_page() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    assert!(!app.shortcuts_open());

    run(&mut app, command_id::KEYBOARD_SHORTCUTS);
    assert!(app.shortcuts_open());
    app.close_shortcuts();
    assert!(!app.shortcuts_open());
}

#[test]
fn commands_that_need_a_workspace_do_nothing_without_one() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());

    run(&mut app, command_id::NEW_NOTE);
    run(&mut app, command_id::COMMAND_PALETTE);

    assert_eq!(app.palette(), None);
    assert!(app.tabs().is_empty());
}
