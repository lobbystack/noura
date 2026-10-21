//! File tree operations through AccessKit: the header, row menus, inline
//! rename, keys and dragging rows.

mod support;

use std::path::Path;

use app_core::Platform;
use egui::{Key, Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, FakeSystem, create_workspace, harness_with, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A workspace with a folder and two notes, opened at launch.
fn opened(app_data: &Path, folder: &Path) -> Ui {
    opened_on(app_data, folder, Platform::current()).0
}

fn opened_on(
    app_data: &Path,
    folder: &Path,
    platform: Platform,
) -> (Ui, std::sync::Arc<FakeSystem>) {
    create_workspace(app_data, "Garden", folder);
    for (path, bytes) in [
        ("Docs/Guide.md", b"g".as_slice()),
        ("Readme.md", b"r"),
        ("Todo.md", b"t"),
    ] {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let system = FakeSystem::new(platform);
    let mut harness = harness_with(app_data, FakePicker::default(), system.clone());
    settle(&mut harness);
    (harness, system)
}

fn rows(harness: &Ui) -> Vec<String> {
    harness
        .get_all_by_role(Role::TreeItem)
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

fn row<'a>(harness: &'a Ui, name: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::TreeItem, name)
}

fn focus_row(harness: &mut Ui, name: &str) {
    row(harness, name).focus();
    harness.run();
}

fn press(harness: &mut Ui, key: Key) {
    harness.key_press(key);
    harness.run();
}

fn press_with(harness: &mut Ui, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run();
}

fn click(harness: &mut Ui, label: &str) {
    harness.get_by_role_and_label(Role::Button, label).click();
    harness.run();
}

fn document_shows(harness: &Ui, text: &str) -> bool {
    harness
        .get_by_role_and_label(Role::Region, "Document")
        .query_by_label(text)
        .is_some()
}

#[test]
fn the_header_creates_a_note_that_opens_and_a_folder_you_name_in_place() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);

    click(&mut harness, "New note");

    assert!(folder.join("Untitled.md").is_file());
    assert_eq!(rows(&harness), ["Docs", "Readme", "Todo", "Untitled"]);
    assert_eq!(
        row(&harness, "Untitled").accesskit_node().is_selected(),
        Some(true)
    );
    assert!(document_shows(&harness, "Untitled.md"));

    click(&mut harness, "New folder");
    let field = harness.get_by_role_and_label(Role::TextInput, "New name for Untitled");
    assert!(field.is_focused());
    field.type_text("Projects");
    harness.run();
    press(&mut harness, Key::Enter);

    assert!(folder.join("Projects").is_dir());
    assert!(harness.query_by_role(Role::TextInput).is_none());
    assert_eq!(
        rows(&harness),
        ["Docs", "Projects", "Readme", "Todo", "Untitled"]
    );
}

#[test]
fn f2_renames_in_place_where_enter_saves_and_escape_cancels() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    focus_row(&mut harness, "Readme");

    press(&mut harness, Key::F2);
    let field = harness.get_by_role_and_label(Role::TextInput, "New name for Readme");
    assert_eq!(field.value().as_deref(), Some("Readme"));
    // The whole name is selected, so typing replaces it.
    field.type_text("About");
    harness.run();
    press(&mut harness, Key::Enter);

    assert!(folder.join("About.md").is_file());
    assert_eq!(rows(&harness), ["Docs", "About", "Todo"]);
    assert_eq!(
        harness
            .get_all_by_role(Role::TreeItem)
            .find(|node| node.is_focused())
            .and_then(|node| node.accesskit_node().label())
            .as_deref(),
        Some("About")
    );

    press(&mut harness, Key::F2);
    harness
        .get_by_role_and_label(Role::TextInput, "New name for About")
        .type_text("Gone");
    harness.run();
    press(&mut harness, Key::Escape);

    assert!(folder.join("About.md").is_file());
    assert!(harness.query_by_role(Role::TextInput).is_none());
}

#[test]
fn enter_on_the_open_file_starts_a_rename() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    focus_row(&mut harness, "Todo");

    press(&mut harness, Key::Enter);
    assert!(document_shows(&harness, "Todo.md"));
    assert!(harness.query_by_role(Role::TextInput).is_none());
    press(&mut harness, Key::Enter);

    harness.get_by_role_and_label(Role::TextInput, "New name for Todo");
}

#[test]
fn a_refused_name_says_why_beside_the_field() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    focus_row(&mut harness, "Readme");
    press(&mut harness, Key::F2);

    harness
        .get_by_role_and_label(Role::TextInput, "New name for Readme")
        .type_text("Todo");
    harness.run();
    press(&mut harness, Key::Enter);

    harness.get_by_label("Something named \u{201c}Todo\u{201d} is already in this folder.");
    harness.get_by_role_and_label(Role::TextInput, "New name for Readme");
    assert!(folder.join("Readme.md").is_file());
}

#[test]
fn shift_f10_the_menu_key_and_a_right_click_open_a_rows_menu_with_labeled_items() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, _system) = opened_on(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::MacOs,
    );
    focus_row(&mut harness, "Readme");

    press_with(&mut harness, Modifiers::SHIFT, Key::F10);
    let menu = harness.get_by_label("Actions for Readme");
    let items: Vec<String> = menu
        .get_all_by_role(Role::Button)
        .filter_map(|node| node.accesskit_node().label())
        .collect();
    assert_eq!(
        items,
        [
            "Rename\u{2026}",
            "Duplicate",
            "Reveal in Finder",
            "Copy path",
            "Open in Terminal",
            "Move to Trash",
        ]
    );
    assert!(
        menu.get_by_role_and_label(Role::Button, "Rename\u{2026}")
            .is_focused()
    );
    assert!(harness.unnamed_widgets().is_empty());
    press(&mut harness, Key::Escape);
    assert!(harness.query_by_label("Actions for Readme").is_none());
    assert!(row(&harness, "Readme").is_focused());

    focus_row(&mut harness, "Docs");
    press(&mut harness, Key::Menu);
    let menu = harness.get_by_label("Actions for Docs");
    let items: Vec<String> = menu
        .get_all_by_role(Role::Button)
        .filter_map(|node| node.accesskit_node().label())
        .collect();
    assert_eq!(
        items,
        [
            "New note",
            "New folder",
            "Rename\u{2026}",
            "Reveal in Finder",
            "Copy path",
            "Open in Terminal",
            "Move to Trash",
        ]
    );
    press(&mut harness, Key::Escape);

    row(&harness, "Todo").click_secondary();
    harness.run();
    harness
        .get_by_label("Actions for Todo")
        .get_by_role_and_label(Role::Button, "Rename\u{2026}");
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn linux_hides_open_in_terminal_and_names_its_file_manager() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, _system) = opened_on(
        app_data.path(),
        &parent.path().join("Garden"),
        Platform::Linux,
    );

    row(&harness, "Readme").click_secondary();
    harness.run();

    harness.get_by_role_and_label(Role::Button, "Show in file manager");
    assert!(harness.query_by_label("Open in Terminal").is_none());
}

#[test]
fn a_folders_menu_creates_a_note_inside_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);

    row(&harness, "Docs").click_secondary();
    harness.run();
    harness
        .get_by_label("Actions for Docs")
        .get_by_role_and_label(Role::Button, "New note")
        .click();
    harness.run();

    assert!(folder.join("Docs/Untitled.md").is_file());
    assert_eq!(
        rows(&harness),
        ["Docs", "Guide", "Untitled", "Readme", "Todo"]
    );
}

#[test]
fn menu_items_reveal_copy_the_path_open_a_terminal_and_duplicate() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let (mut harness, system) = opened_on(app_data.path(), &folder, Platform::MacOs);
    let root = std::fs::canonicalize(&folder).unwrap();

    for item in [
        "Reveal in Finder",
        "Copy path",
        "Open in Terminal",
        "Duplicate",
    ] {
        row(&harness, "Readme").click_secondary();
        harness.run();
        click(&mut harness, item);
    }

    assert_eq!(
        system.calls(),
        [
            format!("reveal {}", root.join("Readme.md").display()),
            "copy Readme.md".to_owned(),
            format!("terminal {}", root.display()),
        ]
    );
    assert_eq!(std::fs::read(folder.join("Readme 1.md")).unwrap(), b"r");
}

#[test]
fn delete_and_cmd_backspace_trash_a_file_and_a_folder_asks_first() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);

    focus_row(&mut harness, "Readme");
    press(&mut harness, Key::Delete);
    assert!(!folder.join("Readme.md").exists());
    harness.get_by_label("Moved \u{201c}Readme\u{201d} to .noura/trash");

    focus_row(&mut harness, "Todo");
    press_with(&mut harness, Modifiers::COMMAND, Key::Backspace);
    assert!(!folder.join("Todo.md").exists());

    focus_row(&mut harness, "Docs");
    press(&mut harness, Key::Delete);
    harness.get_by_label("Move \u{201c}Docs\u{201d} to the trash?");
    harness.get_by_label(
        "The folder and everything in it go to your system's trash, where you can restore \
         them.",
    );
    click(&mut harness, "Cancel");
    assert!(folder.join("Docs/Guide.md").exists());

    focus_row(&mut harness, "Docs");
    press(&mut harness, Key::Delete);
    click(&mut harness, "Move to Trash");
    assert!(!folder.join("Docs").exists());
}

#[test]
fn collapse_all_closes_every_folder() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    row(&harness, "Docs").click();
    harness.run();
    assert_eq!(rows(&harness), ["Docs", "Guide", "Readme", "Todo"]);

    click(&mut harness, "Collapse all");

    assert_eq!(rows(&harness), ["Docs", "Readme", "Todo"]);
}

/// Drag the row named `from` and drop it on the row named `onto`.
fn drag_row(harness: &mut Ui, from: &str, onto: &str) {
    let start = row(harness, from).rect().center();
    let end = row(harness, onto).rect().center();
    harness.hover_at(start);
    harness.run();
    harness.drag_at(start);
    harness.run();
    for step in 1..=10 {
        harness.hover_at(start + (end - start) * (step as f32 / 10.0));
        harness.run();
    }
    harness.drop_at(end);
    harness.run();
}

#[test]
fn dragging_a_row_onto_a_folder_or_a_file_moves_it_into_that_folder() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);

    drag_row(&mut harness, "Readme", "Docs");
    assert!(folder.join("Docs/Readme.md").is_file());
    assert_eq!(rows(&harness), ["Docs", "Guide", "Readme", "Todo"]);

    drag_row(&mut harness, "Todo", "Guide");
    assert!(folder.join("Docs/Todo.md").is_file());
}

#[test]
fn escape_cancels_a_drag() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    let start = row(&harness, "Readme").rect().center();
    let end = row(&harness, "Docs").rect().center();

    harness.hover_at(start);
    harness.run();
    harness.drag_at(start);
    harness.run();
    for step in 1..=10 {
        harness.hover_at(start + (end - start) * (step as f32 / 10.0));
        harness.run();
    }
    press(&mut harness, Key::Escape);
    harness.drop_at(end);
    harness.run();

    assert!(folder.join("Readme.md").is_file());
}

#[test]
fn resting_on_a_closed_folder_while_dragging_opens_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    let start = row(&harness, "Readme").rect().center();
    let end = row(&harness, "Docs").rect().center();

    harness.hover_at(start);
    harness.run();
    harness.drag_at(start);
    harness.run();
    for step in 1..=10 {
        harness.hover_at(start + (end - start) * (step as f32 / 10.0));
        harness.run();
    }
    std::thread::sleep(app_core::SPRING_OPEN_DELAY);
    harness.hover_at(end);
    harness.run();
    assert_eq!(rows(&harness), ["Docs", "Guide", "Readme", "Todo"]);

    press(&mut harness, Key::Escape);
    harness.drop_at(end);
    harness.run();
    assert_eq!(rows(&harness), ["Docs", "Readme", "Todo"]);
}
