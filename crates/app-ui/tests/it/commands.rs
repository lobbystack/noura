//! Commands through AccessKit: commands in search, the shortcuts page,
//! and shortcuts pressed in the window.

use crate::support;
use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use app_core::{App, Area, Command, Platform, Shortcut, settings_section};
use app_ui::NouraUi;
use egui::{Key, Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{create_workspace, harness_for, settle};

/// A workspace with two notes, opened at launch on `platform` after
/// `register` runs.
fn opened(
    app_data: &Path,
    folder: &Path,
    platform: Platform,
    register: impl FnOnce(&mut App),
) -> Harness<'static, NouraUi> {
    create_workspace(app_data, folder);
    for name in ["Ideas.md", "Plan.md"] {
        std::fs::write(folder.join(name), b"x").unwrap();
    }
    let mut harness = harness_for(app_data, platform, register);
    settle(&mut harness);
    harness
}

fn row<'a>(harness: &'a Harness<'static, NouraUi>, label: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::ListBoxOption, label)
}

fn listed(harness: &Harness<'static, NouraUi>) -> Vec<String> {
    harness
        .query_all_by_role(Role::ListBoxOption)
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

fn palette_open(harness: &Harness<'static, NouraUi>) -> bool {
    harness
        .query_by_role_and_label(Role::Dialog, "Search")
        .is_some()
}

fn press(harness: &mut Harness<'static, NouraUi>, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run();
}

fn tree_row<'a>(harness: &'a Harness<'static, NouraUi>, name: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::TreeItem, name)
}

#[test]
fn mod_k_opens_search_with_recent_notes_and_commands_with_their_shortcuts() {
    for (platform, new_note, palette) in [
        (Platform::MacOs, "⌘N", "⌘K"),
        (Platform::Windows, "Ctrl+N", "Ctrl+K"),
    ] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let mut harness = opened(
            app_data.path(),
            &parent.path().join("Garden"),
            platform,
            |_| {},
        );
        assert!(!palette_open(&harness));

        press(&mut harness, Modifiers::COMMAND, Key::K);

        assert!(palette_open(&harness), "{platform:?}");
        let app = harness.state().app();
        let first: Vec<String> = app
            .inbox()
            .recent
            .into_iter()
            .map(|note| note.title)
            .chain(
                app.commands()
                    .into_iter()
                    .take(app_core::EMPTY_COMMANDS)
                    .map(|command| command.label),
            )
            .collect();
        assert_eq!(listed(&harness), first);
        // Two groups show, each under its heading.
        assert!(
            harness
                .query_by_role_and_label(Role::ListBox, "Files")
                .is_some()
        );
        assert!(
            harness
                .query_by_role_and_label(Role::ListBox, "Commands")
                .is_some()
        );
        let shortcut = |harness: &Harness<'static, NouraUi>, label| {
            row(harness, label)
                .accesskit_node()
                .data()
                .keyboard_shortcut()
                .map(str::to_owned)
        };
        assert_eq!(shortcut(&harness, "New Note").as_deref(), Some(new_note));
        assert_eq!(shortcut(&harness, "Search…").as_deref(), Some(palette));
        assert!(harness.unnamed_widgets().is_empty());
        harness
            .get_by_role_and_label(Role::TextInput, "Search")
            .type_text("keyboard shortcuts");
        harness.run();
        assert_eq!(shortcut(&harness, "Keyboard Shortcuts"), None);
    }
}

#[test]
fn typing_filters_commands_and_enter_runs_the_highlighted_one() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::Windows,
        |_| {},
    );
    press(&mut harness, Modifiers::COMMAND, Key::K);
    assert!(
        harness
            .get_by_role_and_label(Role::TextInput, "Search")
            .is_focused()
    );

    harness
        .get_by_role_and_label(Role::TextInput, "Search")
        .type_text("tab");
    harness.run();
    assert_eq!(listed(&harness), ["Close Tab", "Next Tab", "Previous Tab"]);
    assert_eq!(
        row(&harness, "Close Tab").accesskit_node().is_selected(),
        Some(true)
    );

    press(&mut harness, Modifiers::NONE, Key::ArrowDown);
    assert_eq!(
        row(&harness, "Next Tab").accesskit_node().is_selected(),
        Some(true)
    );
    assert!(
        harness
            .get_by_role_and_label(Role::TextInput, "Search")
            .is_focused()
    );

    // Enter runs the highlighted command, as its shortcut would.
    harness
        .state_mut()
        .app_mut()
        .open_file("Ideas.md", app_core::OpenAs::Kept);
    harness
        .state_mut()
        .app_mut()
        .open_file("Plan.md", app_core::OpenAs::Kept);
    harness.run();
    press(&mut harness, Modifiers::NONE, Key::Enter);

    assert!(!palette_open(&harness));
    let active = harness
        .state()
        .app()
        .active_tab()
        .map(|tab| tab.title().to_owned());
    assert_eq!(active.as_deref(), Some("Ideas"));

    // No match says so once the index answers. The note's text has the
    // keyboard now, where Mod-K inserts a link, so search opens from the
    // tree.
    tree_row(&harness, "Plan").focus();
    harness.run();
    press(&mut harness, Modifiers::COMMAND, Key::K);
    harness
        .get_by_role_and_label(Role::TextInput, "Search")
        .type_text("zzz");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while harness.query_by_label("No results").is_none() {
        assert!(std::time::Instant::now() < deadline, "no empty state");
        std::thread::sleep(std::time::Duration::from_millis(5));
        harness.step();
    }
    assert!(listed(&harness).is_empty());
}

#[test]
fn the_rail_s_search_button_opens_search_and_a_click_runs_a_command() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::MacOs,
        |_| {},
    );
    let search = harness.get_by_role_and_label(Role::Button, "Search");
    assert_eq!(
        search.accesskit_node().data().keyboard_shortcut(),
        Some("⌘K")
    );

    search.click();
    harness.run();
    assert!(palette_open(&harness));

    // The list is taller than the palette, so narrow it to bring the row
    // into view.
    harness
        .get_by_role_and_label(Role::TextInput, "Search")
        .type_text("sidebar");
    harness.run();
    row(&harness, "Toggle Sidebar").click();
    harness.run();
    assert!(!palette_open(&harness));
    assert!(!harness.state().app().layout().sidebar_shown);
}

#[test]
fn keys_typed_right_after_mod_k_land_in_search_and_escape_returns_focus() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::MacOs,
        |_| {},
    );
    tree_row(&harness, "Ideas").focus();
    harness.run();
    assert!(tree_row(&harness, "Ideas").is_focused());

    // The shortcut and the next keys arrive in one frame. Typed into the
    // tree, "pl" would move to Plan.
    let key = |pressed| egui::Event::Key {
        key: Key::K,
        physical_key: None,
        pressed,
        repeat: false,
        modifiers: Modifiers::COMMAND,
    };
    harness
        .input_mut()
        .events
        .extend([key(true), key(false), egui::Event::Text("pl".to_owned())]);
    harness.run();

    assert!(palette_open(&harness));
    let field = harness.get_by_role_and_label(Role::TextInput, "Search");
    assert!(field.is_focused());
    assert_eq!(field.value().as_deref(), Some("pl"));
    // Keys typed in later frames land there too.
    harness.event(egui::Event::Text("an".to_owned()));
    harness.run();
    assert_eq!(
        harness
            .state()
            .app()
            .palette()
            .map(|palette| palette.query().to_owned()),
        Some("plan".to_owned())
    );
    let current = harness.state().app().tree().unwrap().focused();
    assert_eq!(current, Some(0), "the tree stayed on Ideas");

    press(&mut harness, Modifiers::NONE, Key::Escape);

    assert!(!palette_open(&harness));
    assert!(tree_row(&harness, "Ideas").is_focused());
}

#[test]
fn a_key_the_focused_text_field_handles_runs_no_command() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let runs = Arc::new(AtomicUsize::new(0));
    let counter = runs.clone();
    let mut harness = opened(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::MacOs,
        move |app| {
            let command = Command::new("test.erase", "Erase", Area::Anywhere)
                .with_shortcuts([Shortcut::new(app_core::Key::Backspace).with_alt()]);
            app.register_command(command, move |_| {
                counter.fetch_add(1, Ordering::SeqCst);
            })
            .unwrap();
        },
    );
    tree_row(&harness, "Ideas").focus();
    harness.run();
    press(&mut harness, Modifiers::ALT, Key::Backspace);
    assert_eq!(
        runs.load(Ordering::SeqCst),
        1,
        "the tree doesn't use Alt-Backspace"
    );

    // F2 in the tree renames its row.
    press(&mut harness, Modifiers::NONE, Key::F2);
    let field = harness.get_by_label("New name for Ideas");
    assert!(field.is_focused());
    field.type_text("Big plans");
    harness.run();
    // In the rename field, Alt-Backspace deletes a word.
    press(&mut harness, Modifiers::ALT, Key::Backspace);
    assert_eq!(runs.load(Ordering::SeqCst), 1);
    assert_eq!(
        harness
            .get_by_label("New name for Ideas")
            .value()
            .as_deref(),
        Some("Big ")
    );
    // F2 in a text field keeps typing there.
    press(&mut harness, Modifiers::NONE, Key::F2);
    assert!(harness.get_by_label("New name for Ideas").is_focused());
}

#[test]
fn on_linux_every_shortcut_runs_from_the_window() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder, Platform::Linux, |app| {
        app.register_page("test-page", "Test page");
    });
    let ctrl = Modifiers::CTRL | Modifiers::COMMAND;
    let shift = ctrl | Modifiers::SHIFT;
    let active = |harness: &Harness<'static, NouraUi>| {
        harness
            .state()
            .app()
            .active_tab()
            .map(|tab| tab.title().to_owned())
    };

    press(&mut harness, ctrl, Key::N);
    let note = active(&harness).expect("Ctrl+N opens a new note");
    tree_row(&harness, "Ideas").click();
    harness.run();
    tree_row(&harness, "Ideas").click_modifiers(ctrl);
    harness.run();
    assert_eq!(active(&harness).as_deref(), Some("Ideas"));

    press(&mut harness, Modifiers::CTRL, Key::Tab);
    assert_eq!(active(&harness).as_deref(), Some(note.as_str()));
    press(&mut harness, Modifiers::CTRL | Modifiers::SHIFT, Key::Tab);
    assert_eq!(active(&harness).as_deref(), Some("Ideas"));
    press(&mut harness, shift, Key::CloseCurlyBracket);
    assert_eq!(active(&harness).as_deref(), Some(note.as_str()));
    press(&mut harness, shift, Key::OpenCurlyBracket);
    assert_eq!(active(&harness).as_deref(), Some("Ideas"));

    press(&mut harness, ctrl, Key::W);
    assert_eq!(active(&harness).as_deref(), Some(note.as_str()));

    press(&mut harness, ctrl, Key::Num3);
    assert_eq!(harness.state().app().current_page(), "test-page");
    press(&mut harness, ctrl, Key::Num2);
    assert_eq!(harness.state().app().current_page(), app_core::FILES_PAGE);

    press(&mut harness, ctrl, Key::Backslash);
    assert!(!harness.state().app().layout().sidebar_shown);
    press(&mut harness, ctrl, Key::Backslash);
    assert!(harness.state().app().layout().sidebar_shown);

    // The open note's text has the keyboard, so F2 moves to its name.
    press(&mut harness, Modifiers::NONE, Key::F2);
    assert!(
        harness
            .get_by_role_and_label(Role::TextInput, "File name")
            .is_focused(),
        "F2 renames the open file"
    );
    press(&mut harness, Modifiers::NONE, Key::Escape);

    press(&mut harness, shift, Key::N);
    assert!(
        folder.join("Untitled").is_dir(),
        "Ctrl+Shift+N makes a folder"
    );
    press(&mut harness, Modifiers::NONE, Key::Escape);

    press(&mut harness, ctrl, Key::K);
    assert!(palette_open(&harness));
    press(&mut harness, ctrl, Key::K);
    assert!(!palette_open(&harness));

    // Ctrl+Q quits, which closes the main window and ends the app, even
    // from a text field. One event, so one frame whose output holds the
    // window's commands.
    tree_row(&harness, "Ideas").focus();
    harness.run();
    press(&mut harness, Modifiers::NONE, Key::F2);
    assert!(harness.ctx.text_edit_focused());
    harness.event(key_down(Key::Q, ctrl));
    harness.step();
    assert!(closes_the_main_window(&harness));
    assert!(harness.state().is_quitting());
    press(&mut harness, Modifiers::NONE, Key::Escape);

    // Ctrl+Shift+W closes the window, which the desktop then stops
    // showing.
    assert!(!harness.state().is_closed());
    press(&mut harness, shift, Key::W);
    assert!(harness.state().is_closed());
}

fn key_down(key: Key, modifiers: Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: None,
        pressed: true,
        repeat: false,
        modifiers,
    }
}

/// Whether the last frame asked the main window to close.
fn closes_the_main_window(harness: &Harness<'static, NouraUi>) -> bool {
    harness.output().viewport_output[&egui::ViewportId::ROOT]
        .commands
        .contains(&egui::ViewportCommand::Close)
}

#[test]
fn on_linux_ctrl_q_quits_from_the_chooser_too() {
    let app_data = tempfile::tempdir().unwrap();
    let mut harness = harness_for(app_data.path(), Platform::Linux, |_| {});
    settle(&mut harness);
    assert!(harness.state().app().workspace().is_none());

    harness.event(key_down(Key::Q, Modifiers::CTRL | Modifiers::COMMAND));
    harness.step();
    assert!(closes_the_main_window(&harness));
}

#[test]
fn quit_from_the_menu_closes_the_main_window_without_drawing_a_frame() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::MacOs,
        |_| {},
    );
    let ui = harness.state_mut();
    assert!(ui.run_menu_command(app_core::command_id::QUIT, std::time::Instant::now()));
    // The desktop shell does this after menu choices, frame or not.
    let ctx = harness.ctx.clone();
    harness.state_mut().close_requested_windows(&ctx);
    harness.step();
    assert!(closes_the_main_window(&harness));
}

#[test]
fn the_shortcuts_page_lists_every_shortcut_by_area() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::Windows,
        |_| {},
    );
    press(&mut harness, Modifiers::COMMAND, Key::K);
    harness
        .get_by_role_and_label(Role::TextInput, "Search")
        .type_text("keyboard");
    harness.run();
    press(&mut harness, Modifiers::NONE, Key::Enter);

    for area in ["Anywhere", "File tree", "Tabs", "Pages", "Writing", "Notes"] {
        let list = harness.get_by_role_and_label(Role::List, area);
        assert!(
            list.query_all_by_role(Role::ListItem).next().is_some(),
            "{area}"
        );
    }
    for (label, keys) in [
        ("New Note", "Ctrl+N"),
        ("Next Tab", "Ctrl+Shift+] or Ctrl+Tab"),
        ("Toggle Sidebar", "Ctrl+\\"),
        ("Rename File", "F2"),
        ("Close a dialog", "Escape"),
        ("Move to the trash", "Delete or Ctrl+Backspace"),
        ("Jump to a name by typing it", "A–Z"),
        ("Close the focused tab", "Delete or Backspace"),
        ("Move a page up or down", "Alt+Up or Alt+Down"),
        ("Exit", "Ctrl+Q"),
        ("Open a file", "Enter or Space"),
        ("Open or close a folder", "Right or Left or Enter or Space"),
        ("Show the focused tab", "Enter or Space"),
        ("Open a page", "Enter or Space"),
        ("Cut", "Ctrl+X"),
        ("Copy", "Ctrl+C"),
        ("Paste", "Ctrl+V"),
        ("Select All", "Ctrl+A"),
        ("Bold", "Ctrl+B"),
        ("Strikethrough", "Ctrl+Shift+X"),
        ("Heading 1", "Ctrl+Alt+1"),
        ("Check or uncheck a task", "Ctrl+Enter"),
        ("Find in the note", "Ctrl+F"),
    ] {
        let description = harness
            .get_by_role_and_label(Role::ListItem, label)
            .accesskit_node()
            .description();
        assert_eq!(description.as_deref(), Some(keys), "{label}");
    }
    // Text fields and a note's text both undo and redo.
    for area in ["Writing", "Notes"] {
        let list = harness.get_by_role_and_label(Role::List, area);
        for (label, keys) in [("Undo", "Ctrl+Z"), ("Redo", "Ctrl+Shift+Z or Ctrl+Y")] {
            let description = list
                .get_by_role_and_label(Role::ListItem, label)
                .accesskit_node()
                .description();
            assert_eq!(description.as_deref(), Some(keys), "{area}: {label}");
        }
    }

    // The page is a section of Settings.
    harness
        .get_by_role_and_label(Role::Dialog, "Settings")
        .get_by_role_and_label(Role::Button, "Close")
        .click();
    harness.run();
    assert!(
        harness
            .query_by_role_and_label(Role::List, "Anywhere")
            .is_none()
    );
}

#[test]
fn a_test_command_shows_in_search_and_the_shortcuts_page() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::Windows,
        |app| {
            let command = Command::new("test.ping", "Ping", Area::Anywhere)
                .with_shortcuts([Shortcut::command(app_core::Key::P).with_shift()]);
            app.register_command(command, |_| {}).unwrap();
        },
    );

    press(&mut harness, Modifiers::COMMAND, Key::K);
    harness
        .get_by_role_and_label(Role::TextInput, "Search")
        .type_text("ping");
    harness.run();
    assert_eq!(
        row(&harness, "Ping")
            .accesskit_node()
            .data()
            .keyboard_shortcut(),
        Some("Ctrl+Shift+P")
    );
    press(&mut harness, Modifiers::NONE, Key::Escape);

    harness
        .state_mut()
        .app_mut()
        .open_settings_at(settings_section::SHORTCUTS);
    harness.run();
    let entry = harness.get_by_role_and_label(Role::ListItem, "Ping");
    assert_eq!(
        entry.accesskit_node().description().as_deref(),
        Some("Ctrl+Shift+P")
    );
}
