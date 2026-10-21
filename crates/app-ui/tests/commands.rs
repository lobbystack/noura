//! Commands through AccessKit: the command palette, the shortcuts page,
//! and shortcuts pressed in the window.

mod support;

use std::{
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

use app_core::{App, Area, Command, Platform, Shortcut};
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
    create_workspace(app_data, "Garden", folder);
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
        .query_by_role_and_label(Role::Dialog, "Command palette")
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
fn mod_k_opens_a_palette_of_every_command_with_its_shortcut() {
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
        let every: Vec<String> = harness
            .state()
            .app()
            .commands()
            .into_iter()
            .map(|command| command.label)
            .collect();
        assert_eq!(listed(&harness), every);
        let shortcut = |label| {
            row(&harness, label)
                .accesskit_node()
                .data()
                .keyboard_shortcut()
                .map(str::to_owned)
        };
        assert_eq!(shortcut("New Note").as_deref(), Some(new_note));
        assert_eq!(shortcut("Command Palette").as_deref(), Some(palette));
        assert_eq!(shortcut("Keyboard Shortcuts"), None);
        assert!(harness.unnamed_widgets().is_empty());
    }
}

#[test]
fn typing_filters_the_palette_and_enter_runs_the_highlighted_command() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::Windows,
        |_| {},
    );
    press(&mut harness, Modifiers::COMMAND, Key::K);
    assert!(harness.get_by_label("Search commands").is_focused());

    harness.get_by_label("Search commands").type_text("tab");
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
    assert!(harness.get_by_label("Search commands").is_focused());

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

    // No match says so.
    press(&mut harness, Modifiers::COMMAND, Key::K);
    harness.get_by_label("Search commands").type_text("zzz");
    harness.run();
    assert!(listed(&harness).is_empty());
    harness.get_by_label("No commands match \u{201c}zzz\u{201d}.");
}

#[test]
fn the_rail_s_search_button_opens_the_palette_and_a_click_runs_a_command() {
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

    row(&harness, "Toggle Sidebar").click();
    harness.run();
    assert!(!palette_open(&harness));
    assert!(!harness.state().app().layout().sidebar_shown);
}

#[test]
fn keys_typed_right_after_mod_k_land_in_the_palette_and_escape_returns_focus() {
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
    let field = harness.get_by_label("Search commands");
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
            .map(|palette| palette.query.clone()),
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

    press(&mut harness, Modifiers::NONE, Key::F2);
    assert!(
        harness.state().app().renaming().is_some(),
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
        .get_by_label("Search commands")
        .type_text("keyboard");
    harness.run();
    press(&mut harness, Modifiers::NONE, Key::Enter);

    for area in ["Anywhere", "File tree", "Tabs", "Pages", "Writing"] {
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
        ("Undo", "Ctrl+Z"),
        ("Redo", "Ctrl+Shift+Z or Ctrl+Y"),
        ("Cut", "Ctrl+X"),
        ("Copy", "Ctrl+C"),
        ("Paste", "Ctrl+V"),
        ("Select All", "Ctrl+A"),
    ] {
        let description = harness
            .get_by_role_and_label(Role::ListItem, label)
            .accesskit_node()
            .description();
        assert_eq!(description.as_deref(), Some(keys), "{label}");
    }

    harness.get_by_label("Done").click();
    harness.run();
    assert!(
        harness
            .query_by_role_and_label(Role::List, "Anywhere")
            .is_none()
    );
}

#[test]
fn a_test_command_shows_in_the_palette_and_the_shortcuts_page() {
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
    assert_eq!(
        row(&harness, "Ping")
            .accesskit_node()
            .data()
            .keyboard_shortcut(),
        Some("Ctrl+Shift+P")
    );
    press(&mut harness, Modifiers::NONE, Key::Escape);

    harness.state_mut().app_mut().open_shortcuts();
    harness.run();
    let entry = harness.get_by_role_and_label(Role::ListItem, "Ping");
    assert_eq!(
        entry.accesskit_node().description().as_deref(),
        Some("Ctrl+Shift+P")
    );
}
