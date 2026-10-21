//! Seam 1: what the kernel's commands do in an open workspace.

mod support;

use std::time::{Duration, Instant};

use app_core::{
    App, Area, CommandSource, FILES_SECTION, Key, KeyPress, OpenAs, Platform, command_id,
    settings_section,
};
use support::{FakeSystem, Fixture, document};

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
fn quit_asks_the_app_to_close_once_from_its_key_anywhere() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::Linux));
    assert!(!app.take_quit());

    // Ctrl-Q quits even while a text field has the keyboard.
    let ctrl_q = KeyPress::new(Key::Q).with_ctrl();
    assert!(app.press_key(&ctrl_q, Some(Area::Writing), Instant::now()));

    assert!(app.take_quit());
    assert!(!app.take_quit());
    assert!(!app.take_close_window(), "Quit isn't Close Window");
}

#[test]
fn without_a_workspace_only_the_app_wide_commands_take_their_keys() {
    let app_data = tempfile::tempdir().unwrap();
    let app = App::launch_with(app_data.path(), FakeSystem::on(Platform::Linux));
    assert!(app.workspace().is_none());
    let ctrl = |key| KeyPress::new(key).with_ctrl();

    for (press, id) in [
        (ctrl(Key::Q), command_id::QUIT),
        (ctrl(Key::W).with_shift(), command_id::CLOSE_WINDOW),
        (ctrl(Key::Equals), command_id::ZOOM_IN),
        (ctrl(Key::Minus), command_id::ZOOM_OUT),
        (ctrl(Key::Num0), command_id::ZOOM_RESET),
        (ctrl(Key::Comma), command_id::SETTINGS),
    ] {
        assert_eq!(app.command_for_key(&press, None).as_deref(), Some(id));
    }
    for key in [Key::N, Key::K, Key::W, Key::Backslash, Key::Num2] {
        assert_eq!(app.command_for_key(&ctrl(key), None), None, "Ctrl+{key:?}");
    }
}

#[test]
fn mod_plus_equals_minus_and_0_zoom_and_mod_comma_opens_settings() {
    for platform in [Platform::MacOs, Platform::Windows, Platform::Linux] {
        let fixture = Fixture::new(&[]);
        let mut app = fixture.app_with(FakeSystem::on(platform));
        let modifier = |key| match platform {
            Platform::MacOs => KeyPress::new(key).with_cmd(),
            _ => KeyPress::new(key).with_ctrl(),
        };
        let mut at = Instant::now();
        let mut press = |app: &mut App, press: KeyPress| {
            at += Duration::from_secs(1);
            assert!(app.press_key(&press, None, at), "{platform:?} {press:?}");
            app.preferences().zoom
        };

        assert_eq!(press(&mut app, modifier(Key::Plus)), 110);
        assert_eq!(press(&mut app, modifier(Key::Equals)), 125);
        // + is Shift-= on a US keyboard.
        assert_eq!(press(&mut app, modifier(Key::Plus).with_shift()), 150);
        assert_eq!(press(&mut app, modifier(Key::Minus)), 125);
        assert_eq!(press(&mut app, modifier(Key::Num0)), 100);
        assert!(app.settings().is_none());
        press(&mut app, modifier(Key::Comma));
        let settings = app.settings().expect("Settings opened");
        assert_eq!(settings.section, settings_section::GENERAL);
    }
}

#[test]
fn the_zoom_and_settings_commands_run_from_menus_and_the_palette() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let labels: Vec<(String, String)> = app
        .commands()
        .into_iter()
        .map(|command| (command.id, command.label))
        .collect();
    for (id, label) in [
        (command_id::ZOOM_IN, "Zoom In"),
        (command_id::ZOOM_OUT, "Zoom Out"),
        (command_id::ZOOM_RESET, "Actual Size"),
        (command_id::SETTINGS, "Settings…"),
        (command_id::ABOUT, "About Noura"),
    ] {
        assert!(labels.contains(&(id.to_owned(), label.to_owned())), "{id}");
    }

    let at = Instant::now();
    assert!(app.run_command(command_id::ZOOM_IN, CommandSource::Menu, at));
    assert_eq!(app.preferences().zoom, 110);
    run(&mut app, command_id::ZOOM_OUT);
    run(&mut app, command_id::ZOOM_OUT);
    assert_eq!(app.preferences().zoom, 90);
    run(&mut app, command_id::ZOOM_RESET);
    assert_eq!(app.preferences().zoom, 100);
    assert!(app.run_command(command_id::SETTINGS, CommandSource::Menu, at));
    assert_eq!(
        app.settings().map(|settings| settings.section.as_str()),
        Some(settings_section::GENERAL)
    );
}

#[test]
fn keyboard_shortcuts_and_about_open_their_settings_sections() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    assert!(app.settings().is_none());
    let section = |app: &App| app.settings().map(|settings| settings.section.clone());

    run(&mut app, command_id::KEYBOARD_SHORTCUTS);
    assert_eq!(section(&app).as_deref(), Some(settings_section::SHORTCUTS));
    // With Settings open, About moves it to its About section.
    run(&mut app, command_id::ABOUT);
    assert_eq!(section(&app).as_deref(), Some(settings_section::ABOUT));
    app.close_settings();
    run(&mut app, command_id::ABOUT);
    assert_eq!(section(&app).as_deref(), Some(settings_section::ABOUT));
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
