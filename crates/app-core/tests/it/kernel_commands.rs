//! Seam 1: what the kernel's commands do in an open workspace.

use crate::support;
use std::time::{Duration, Instant};

use app_core::{
    App, Area, CommandSource, FILES_SECTION, Key, KeyPress, OpenAs, Overlay, PdfZoom, Platform,
    command_id, settings_section,
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
    assert!(!app.is_closed());
    // With no tab open, it closes the window, as in Safari and Chrome.
    run(&mut app, command_id::CLOSE_TAB);
    assert!(app.is_closed());
}

#[test]
fn where_the_app_stays_open_the_last_window_hides_and_a_menu_item_shows_it_again() {
    let fixture = Fixture::new(&[("A.md", b"a"), ("B.md", b"b")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    app.hide_when_last_closes(true);
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Kept);
    let start = Instant::now();
    let at = |seconds| start + Duration::from_secs(seconds);
    let cmd_w = KeyPress::new(Key::W).with_cmd();

    // Another window is open: this one closes for real.
    let mut other = app.new_window().unwrap();
    other.close_window();
    assert!(other.is_closed());

    assert!(app.press_key(&cmd_w, None, at(1)));
    assert!(app.press_key(&cmd_w, None, at(2)));
    assert!(app.tabs().is_empty());
    assert!(app.press_key(&cmd_w, None, at(3)));
    assert!(app.is_hidden());
    assert!(!app.is_closed(), "the workspace stays open");
    // Close Tab and Close Window from the menu leave it hidden.
    assert!(!app.run_command(command_id::CLOSE_TAB, CommandSource::Menu, at(4)));
    assert!(app.is_hidden());

    // New Window shows it, with no tabs, and opens no other window.
    assert!(app.run_command(command_id::NEW_WINDOW, CommandSource::Menu, at(5)));
    assert!(!app.is_hidden());
    assert!(app.take_opened_windows().is_empty());
    assert!(app.tabs().is_empty());
    // Its tabs wait for Reopen Closed Tab.
    run(&mut app, command_id::REOPEN_CLOSED_TAB);
    run(&mut app, command_id::REOPEN_CLOSED_TAB);
    let titles: Vec<&str> = app.tabs().iter().map(|tab| tab.title()).collect();
    assert_eq!(titles, ["A", "B"]);

    // Closed by its button, it hides too, and New Note shows it again.
    app.close_window();
    assert!(app.is_hidden());
    assert!(app.run_command(command_id::NEW_NOTE, CommandSource::Menu, at(6)));
    assert!(!app.is_hidden());
    assert!(document(&app).is_some());
    // Quit doesn't show it.
    app.close_window();
    assert!(app.run_command(command_id::QUIT, CommandSource::Menu, at(7)));
    assert!(app.is_hidden());
}

#[test]
fn close_tab_pressed_twice_quickly_closes_two_tabs() {
    let fixture = Fixture::new(&[("One.md", b"1"), ("Two.md", b"2"), ("Three.md", b"3")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    for path in ["One.md", "Two.md", "Three.md"] {
        app.open_file(path, OpenAs::Kept);
    }
    let press = KeyPress::new(Key::W).with_cmd();
    let start = Instant::now();

    assert!(app.press_key(&press, None, start));
    assert!(app.press_key(&press, None, start + Duration::from_millis(40)));

    let titles: Vec<&str> = app.tabs().iter().map(|tab| tab.title()).collect();
    assert_eq!(titles, ["One"]);
    assert_eq!(document(&app).as_deref(), Some("One.md"));
}

#[test]
fn close_tab_closes_settings_or_search_and_not_the_tab_under_them() {
    let fixture = Fixture::new(&[("A.md", b"a"), ("B.md", b"b")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Kept);
    let cmd = |key| KeyPress::new(key).with_cmd();
    let start = Instant::now();
    let at = |seconds| start + Duration::from_secs(seconds);
    let titles = |app: &App| -> Vec<String> {
        app.tabs()
            .iter()
            .map(|tab| tab.title().to_owned())
            .collect()
    };

    app.open_settings();
    assert_eq!(app.overlay(), Some(Overlay::Settings));
    assert!(app.press_key(&cmd(Key::W), None, at(1)));
    assert!(app.settings().is_none());
    assert_eq!(titles(&app), ["A", "B"]);

    // The menu's report of the same press closes nothing more.
    app.toggle_palette();
    assert_eq!(app.overlay(), Some(Overlay::Palette));
    assert!(app.press_key(&cmd(Key::W), Some(Area::Writing), at(2)));
    assert!(!app.run_command(
        command_id::CLOSE_TAB,
        CommandSource::Menu,
        at(2) + Duration::from_millis(20)
    ));
    assert_eq!(app.palette(), None);
    assert_eq!(titles(&app), ["A", "B"]);
    // File > Close Tab chosen while Settings shows closes Settings too.
    app.open_settings();
    assert!(app.run_command(command_id::CLOSE_TAB, CommandSource::Menu, at(3)));
    assert!(app.settings().is_none());
    assert_eq!(titles(&app), ["A", "B"]);
    // With nothing over them, Close Tab closes the tab again.
    assert!(app.press_key(&cmd(Key::W), None, at(4)));
    assert_eq!(titles(&app), ["A"]);
}

#[test]
fn keys_for_the_tabs_and_files_wait_while_settings_search_or_a_dialog_covers_them() {
    let fixture = Fixture::new(&[("A.md", b"a"), ("B.md", b"b"), ("Folder/C.md", b"c")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    app.open_file("A.md", OpenAs::Kept);
    app.open_file("B.md", OpenAs::Kept);
    let cmd = |key| KeyPress::new(key).with_cmd();
    let mut at = Instant::now();
    let mut press = |app: &mut App, press: KeyPress| {
        at += Duration::from_secs(1);
        app.press_key(&press, None, at);
    };
    let unchanged = |app: &App| {
        let titles: Vec<&str> = app.tabs().iter().map(|tab| tab.title()).collect();
        assert_eq!(titles, ["A", "B"]);
        assert_eq!(active(app).as_deref(), Some("B"));
        assert!(app.layout().sidebar_shown);
        assert_eq!(app.renaming(), None);
    };

    app.open_settings();
    for key in [
        cmd(Key::N),
        cmd(Key::Backslash),
        cmd(Key::CloseBracket).with_shift(),
        KeyPress::new(Key::Tab).with_ctrl(),
        KeyPress::new(Key::F2),
        cmd(Key::K),
    ] {
        press(&mut app, key);
        unchanged(&app);
        assert_eq!(app.overlay(), Some(Overlay::Settings), "{key:?}");
    }
    // Settings' own commands and zoom still work.
    press(&mut app, cmd(Key::Equals));
    assert_eq!(app.preferences().zoom, 110);
    app.close_settings();

    app.toggle_palette();
    press(&mut app, cmd(Key::N));
    press(&mut app, cmd(Key::Comma));
    unchanged(&app);
    assert!(
        app.settings().is_none(),
        "Settings doesn't open under search"
    );
    // Search's own keys change its filter.
    press(&mut app, cmd(Key::O));
    assert_eq!(app.overlay(), Some(Overlay::Palette));
    app.close_palette();

    // A dialog that asks something waits for its answer: Close Tab and
    // Close Window do nothing.
    app.request_trash("Folder");
    assert_eq!(app.overlay(), Some(Overlay::Dialog));
    press(&mut app, cmd(Key::W));
    press(&mut app, cmd(Key::W).with_shift());
    unchanged(&app);
    assert!(!app.is_closed());
    assert!(app.pending_trash().is_some());
    app.cancel_trash();
    assert_eq!(app.overlay(), None);
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
fn close_window_closes_the_window() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    assert!(!app.is_closed());

    run(&mut app, command_id::CLOSE_WINDOW);

    assert!(app.is_closed());
}

#[test]
fn new_window_opens_another_window_on_the_open_workspace() {
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    app.open_file("Plan.md", OpenAs::Kept);
    assert!(app.take_opened_windows().is_empty());

    run(&mut app, command_id::NEW_WINDOW);

    let windows = app.take_opened_windows();
    assert_eq!(windows.len(), 1);
    assert_eq!(
        windows[0].workspace().map(|workspace| workspace.id()),
        app.workspace().map(|workspace| workspace.id())
    );
    assert!(windows[0].tabs().is_empty(), "a new window has no tabs");
    assert_eq!(app.tabs().len(), 1);
    assert!(app.take_opened_windows().is_empty());
    // The new window runs the same commands, New Window among them.
    assert_eq!(windows[0].commands(), app.commands());
}

#[test]
fn new_window_does_nothing_without_a_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());

    run(&mut app, command_id::NEW_WINDOW);

    assert!(app.take_opened_windows().is_empty());
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
    assert!(!app.is_closed(), "Quit isn't Close Window");
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

#[test]
fn zoom_keys_zoom_the_pdf_while_its_tab_is_active_and_the_interface_elsewhere() {
    let fixture = Fixture::new(&[
        ("text.pdf", &crate::pdf_viewer::pdf("text.pdf")),
        ("Plan.md", b"# Plan\n"),
    ]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let mut at = Instant::now();
    let mut press = |app: &mut App, key| {
        at += Duration::from_secs(1);
        assert!(
            app.press_key(&KeyPress::new(key).with_cmd(), None, at),
            "{key:?}"
        );
    };
    app.open_file("text.pdf", OpenAs::Kept);
    let pdf = app.active_tab().unwrap().id();
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.pdf(pdf).unwrap().state() != &app_core::PdfState::Ready {
        assert!(Instant::now() < deadline, "the PDF never opened");
        std::thread::sleep(Duration::from_millis(5));
        app.poll();
    }
    let zoom = |app: &App| app.pdf(pdf).unwrap().zoom();

    // As in Preview: Command-= and Command-minus step the PDF's zoom, and
    // Command-0 shows its pages at their own size.
    press(&mut app, Key::Equals);
    assert_ne!(zoom(&app), PdfZoom::FitWidth);
    press(&mut app, Key::Num0);
    assert_eq!(zoom(&app), PdfZoom::Scale(1.0));
    press(&mut app, Key::Minus);
    assert_eq!(zoom(&app), PdfZoom::Scale(0.9));
    assert_eq!(app.preferences().zoom, 100);

    // In a note, they zoom the interface, and leave the PDF alone.
    app.open_file("Plan.md", OpenAs::Kept);
    press(&mut app, Key::Equals);
    assert_eq!(app.preferences().zoom, 110);
    press(&mut app, Key::Num0);
    assert_eq!(app.preferences().zoom, 100);
    assert_eq!(zoom(&app), PdfZoom::Scale(0.9));
}
