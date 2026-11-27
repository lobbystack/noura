//! The Files tab through AccessKit: its list of a folder's items, going
//! into a folder and back up, an empty folder, the tree's menu on a row,
//! what the window shows with every tab closed, and the Files in every
//! sidebar setting.

use crate::support;
use egui::{Key, PointerButton, accesskit::Role};
use egui_kittest::kittest::{NodeT as _, Queryable};
use support::{FakePicker, create_workspace, harness, settle};

type Ui = egui_kittest::Harness<'static, app_ui::NouraUi>;

/// A workspace named Garden with a folder, a nested folder, an empty
/// folder and two notes, opened at launch.
fn opened() -> (Ui, std::path::PathBuf, tempfile::TempDir, tempfile::TempDir) {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), &folder);
    for (path, text) in [
        ("Plan.md", "plan\n"),
        ("Beds.md", "beds\n"),
        ("Docs/Guide.md", "guide\n"),
        ("Docs/Old/Notes.md", "old\n"),
    ] {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    std::fs::create_dir_all(folder.join("Empty")).unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    (harness, folder, app_data, parent)
}

fn open_files(harness: &mut Ui) {
    harness
        .get_by_role_and_label(Role::Navigation, "Pages")
        .get_by_role_and_label(Role::Link, "Files")
        .click();
    harness.run();
}

/// The rows the Files tab lists, top to bottom.
fn rows(harness: &Ui) -> Vec<String> {
    harness
        .get_by_role_and_label(Role::ListBox, "Files")
        .get_all_by_role(Role::ListBoxOption)
        .filter_map(|row| row.accesskit_node().label())
        .collect()
}

fn row<'a>(harness: &'a Ui, name: &'a str) -> egui_kittest::Node<'a> {
    harness
        .get_by_role_and_label(Role::ListBox, "Files")
        .get_by_role_and_label(Role::ListBoxOption, name)
}

#[test]
fn the_files_tab_lists_folders_first_and_goes_into_a_folder_and_back_up() {
    let (mut harness, _folder, _app_data, _parent) = opened();
    open_files(&mut harness);

    assert_eq!(rows(&harness), ["Docs", "Empty", "Beds", "Plan"]);
    let docs = row(&harness, "Docs").accesskit_node().description();
    assert!(
        docs.as_deref()
            .is_some_and(|text| text.starts_with("Folder, ")),
        "{docs:?}"
    );
    // A file says when it changed; today, that's the time.
    let plan = row(&harness, "Plan").accesskit_node().description();
    assert!(plan.is_some_and(|text| text.contains(':')));

    row(&harness, "Docs").click();
    harness.run();
    assert_eq!(rows(&harness), ["Old", "Guide"]);
    // The tree shows the folder too, as its current row.
    harness.get_by_role_and_label(Role::TreeItem, "Guide");

    row(&harness, "Old").click();
    harness.run();
    assert_eq!(rows(&harness), ["Notes"]);

    // The path bar goes back up to any folder above.
    let path = harness.get_by_role_and_label(Role::Navigation, "Path");
    path.get_by_role_and_label(Role::Button, "Docs");
    path.get_by_role_and_label(Role::Button, "Garden").click();
    harness.run();
    assert_eq!(rows(&harness), ["Docs", "Empty", "Beds", "Plan"]);

    // Backspace on a row goes up a folder.
    row(&harness, "Docs").click();
    harness.run();
    row(&harness, "Guide").focus();
    harness.run();
    harness.key_press(Key::Backspace);
    harness.run();
    assert_eq!(rows(&harness), ["Docs", "Empty", "Beds", "Plan"]);
}

#[test]
fn an_empty_folder_offers_a_new_note_that_lands_in_it() {
    let (mut harness, folder, _app_data, _parent) = opened();
    open_files(&mut harness);
    row(&harness, "Empty").click();
    harness.run();

    assert!(
        harness
            .query_by_role_and_label(Role::ListBox, "Files")
            .is_none()
    );
    harness.get_by_label("Empty folder");
    harness
        .get_by_role_and_label(Role::Region, "Document")
        .get_by_role_and_label(Role::Button, "New note")
        .click();
    settle(&mut harness);

    let made: Vec<_> = std::fs::read_dir(folder.join("Empty"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(made, ["Untitled.md"]);
}

#[test]
fn a_row_s_right_click_opens_the_tree_s_menu_for_it() {
    let (mut harness, _folder, _app_data, _parent) = opened();
    open_files(&mut harness);
    row(&harness, "Plan").click_button(PointerButton::Secondary);
    harness.run();

    let menu = harness.get_by_role_and_label(Role::Menu, "Actions for Plan");
    assert!(menu.query_by_role_and_label(Role::Button, "Open").is_none());
    for item in [
        "Open in new tab",
        "Rename\u{2026}",
        "Duplicate",
        "Copy path",
    ] {
        menu.get_by_role_and_label(Role::Button, item);
    }
    menu.get_by_role_and_label(Role::Button, "Open in new tab")
        .click();
    harness.run();
    let tabs: Vec<_> = harness
        .get_by_role_and_label(Role::TabList, "Tabs")
        .get_all_by_role(Role::Tab)
        .filter_map(|tab| tab.accesskit_node().label())
        .collect();
    assert_eq!(tabs, ["Files", "Plan"]);
}

#[test]
fn with_every_tab_closed_the_window_says_so_in_one_line() {
    let (mut harness, _folder, _app_data, _parent) = opened();
    open_files(&mut harness);
    harness
        .get_by_role_and_label(Role::Button, "Close Files")
        .click();
    harness.run();

    assert!(harness.state().app().tabs().is_empty());
    harness.get_by_label("No tabs open");
    assert!(harness.query_by_label("No file open").is_none());
}

/// Files in every sidebar is a switch in Settings, off at first.
#[test]
fn files_in_every_sidebar_is_a_named_switch_in_settings() {
    let (mut harness, _folder, _app_data, _parent) = opened();
    harness
        .get_by_role_and_label(Role::Button, "Settings")
        .click();
    harness.run();
    let switch = harness
        .get_by_role_and_label(Role::Dialog, "Settings")
        .get_by_role_and_label(Role::Switch, "Files in every sidebar");
    assert_eq!(
        switch.accesskit_node().toggled(),
        Some(egui::accesskit::Toggled::False)
    );
    switch.click();
    harness.run();
    assert!(harness.state().app().preferences().files_in_every_sidebar);
}
