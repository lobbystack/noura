//! The chooser shown when no workspace is open, through AccessKit.

mod support;

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use support::{FakePicker, harness, settle};

#[test]
fn with_no_recent_workspace_the_chooser_offers_open_folder_and_new_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());

    harness.run();

    harness.get_by_role_and_label(Role::Button, "Open folder…");
    harness.get_by_role_and_label(Role::Button, "New workspace…");
    assert!(harness.query_by_label("Recent workspaces").is_none());
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn open_folder_opens_the_picked_folder() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("Plants.md"), b"x").unwrap();
    let picker = FakePicker::default();
    picker.answer(Some(&folder));
    let mut harness = harness(app_data.path(), picker.clone());
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "Open folder…")
        .click();
    settle(&mut harness);

    assert_eq!(*picker.titles.lock().unwrap(), ["Open a folder"]);
    harness.get_by_role_and_label(Role::TreeItem, "Plants");
    harness.get_by_label_contains("Garden");
}

#[test]
fn new_workspace_asks_for_a_name_then_a_folder() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let picker = FakePicker::default();
    picker.answer(Some(folder.path()));
    let mut harness = harness(app_data.path(), picker.clone());
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "New workspace…")
        .click();
    harness.run();
    let choose = harness.get_by_role_and_label(Role::Button, "Choose folder…");
    assert!(choose.accesskit_node().is_disabled(), "a name comes first");
    harness
        .get_by_role_and_label(Role::TextInput, "Workspace name")
        .type_text("Lab notes");
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Choose folder…")
        .click();
    settle(&mut harness);

    assert_eq!(
        *picker.titles.lock().unwrap(),
        ["Choose a folder for the new workspace"]
    );
    let manifest = std::fs::read_to_string(folder.path().join(".noura/workspace.yaml")).unwrap();
    assert!(manifest.contains("name: Lab notes"), "{manifest}");
    harness.get_by_label_contains("Lab notes");
}

#[test]
fn a_failed_new_workspace_says_why() {
    let app_data = tempfile::tempdir().unwrap();
    let other_app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    support::create_workspace(other_app_data.path(), "Taken", folder.path());
    let picker = FakePicker::default();
    picker.answer(Some(folder.path()));
    let mut harness = harness(app_data.path(), picker);
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "New workspace…")
        .click();
    harness.run();
    harness
        .get_by_role_and_label(Role::TextInput, "Workspace name")
        .type_text("Again");
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Choose folder…")
        .click();
    settle(&mut harness);

    harness.get_by_label("Couldn't create the workspace");
    harness.get_by_label("This folder already has a workspace. Use Open folder to open it.");
}
