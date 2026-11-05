//! Editing a note in the window: the formatting toolbar shows above the
//! note, Mod-K in the text inserts a link instead of opening the command
//! palette, find opens over the note, and the shortcuts page lists the
//! editor's keys as the editor runs them.

mod support;

use std::path::Path;

use app_core::{App, Area, Key, Platform, Shortcut};
use editor_egui::keymap;
use egui::{Modifiers, accesskit::Role};
use egui_kittest::{Harness, kittest::Queryable};
use support::{FakeSystem, create_workspace, harness_for, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A workspace holding the note `Plan.md` with `text`, opened from the
/// tree, with the caret in the note.
fn opened_note(app_data: &Path, folder: &Path, text: &str) -> Ui {
    create_workspace(app_data, "Garden", folder);
    std::fs::write(folder.join("Plan.md"), text).unwrap();
    let mut harness = harness_for(app_data, Platform::Windows, |_| {});
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
    harness
}

/// The open note's text as the editor last reported it.
fn note_text(harness: &Ui) -> String {
    let app = harness.state().app();
    let tab = app.active_tab().expect("a tab is open").id();
    app.note(tab)
        .expect("the tab shows a note")
        .text()
        .to_owned()
}

fn press(harness: &mut Ui, modifiers: Modifiers, key: egui::Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run_steps(3);
}

#[test]
fn mod_k_in_a_note_links_the_selection_and_elsewhere_opens_the_palette() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened_note(app_data.path(), &parent.path().join("Garden"), "docs");
    harness.get_by_role_and_label(Role::Toolbar, "Formatting");

    press(&mut harness, Modifiers::COMMAND, egui::Key::A);
    press(&mut harness, Modifiers::COMMAND, egui::Key::K);

    assert_eq!(note_text(&harness), "[docs]()");
    assert!(harness.query_by_label("Search commands").is_none());

    // From the note's name field, Mod-K opens the palette.
    harness
        .get_by_role_and_label(Role::TextInput, "File name")
        .focus();
    harness.run_steps(2);
    press(&mut harness, Modifiers::COMMAND, egui::Key::K);
    harness.get_by_label("Search commands");
}

#[test]
fn the_toolbar_and_find_work_on_the_open_note() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened_note(
        app_data.path(),
        &parent.path().join("Garden"),
        "one two one\n",
    );

    press(&mut harness, Modifiers::COMMAND, egui::Key::A);
    harness.get_by_label("Numbered list (Ctrl+Shift+7)").click();
    harness.run_steps(3);
    assert_eq!(note_text(&harness), "1. one two one\n");

    press(&mut harness, Modifiers::COMMAND, egui::Key::F);
    harness.get_by_label("Find in note").type_text("one");
    harness.run_steps(3);
    harness.get_by_label("1/2");
}

/// The shortcuts page lists the keys the editor runs: app-core's list of
/// a note's keys matches the editor's keymap on every platform.
#[test]
fn the_shortcuts_list_shows_the_editors_keys() {
    for platform in [Platform::MacOs, Platform::Windows, Platform::Linux] {
        let app_data = tempfile::tempdir().unwrap();
        let app = App::launch_with(app_data.path(), FakeSystem::new(platform));
        let listed: Vec<(String, Vec<Shortcut>)> = app
            .shortcut_groups()
            .into_iter()
            .find(|group| group.area == Area::Note)
            .expect("notes have keys")
            .entries
            .into_iter()
            .filter(|entry| entry.label != "Rename the note")
            .map(|entry| (entry.label, entry.shortcuts))
            .collect();
        let editor: Vec<(String, Vec<Shortcut>)> = keymap::keymap(platform == Platform::MacOs)
            .into_iter()
            .map(|(command, shortcuts)| {
                let shortcuts = shortcuts.into_iter().map(app_shortcut).collect();
                (command.label().to_owned(), shortcuts)
            })
            .collect();
        let mut listed_sorted = listed.clone();
        listed_sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let mut editor_sorted = editor;
        editor_sorted.sort_by(|a, b| a.0.cmp(&b.0));
        assert_eq!(listed_sorted, editor_sorted, "{platform:?}");
    }
}

/// `app-core`'s form of an editor shortcut.
fn app_shortcut(shortcut: keymap::Shortcut) -> Shortcut {
    use keymap::Key as K;
    let key = match shortcut.key {
        K::B => Key::B,
        K::C => Key::C,
        K::F => Key::F,
        K::I => Key::I,
        K::K => Key::K,
        K::Q => Key::Q,
        K::U => Key::U,
        K::X => Key::X,
        K::Y => Key::Y,
        K::Z => Key::Z,
        K::Enter => Key::Enter,
        K::Tab => Key::Tab,
        K::Num0 => Key::Num0,
        K::Num1 => Key::Num1,
        K::Num2 => Key::Num2,
        K::Num3 => Key::Num3,
        K::Num7 => Key::Num7,
        K::Num8 => Key::Num8,
        K::Num9 => Key::Num9,
    };
    let mut app = if shortcut.command {
        Shortcut::command(key)
    } else {
        Shortcut::new(key)
    };
    if shortcut.shift {
        app = app.with_shift();
    }
    if shortcut.alt {
        app = app.with_alt();
    }
    app
}
