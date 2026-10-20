//! The workspace switcher in the sidebar, through AccessKit.

mod support;

use egui::accesskit::Role;
use egui_kittest::kittest::{NodeT, Queryable};
use support::{FakePicker, create_workspace, harness, settle};

#[test]
fn the_switcher_lists_recent_workspaces_and_opens_another() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let first = parent.path().join("First");
    create_workspace(app_data.path(), "First", &first);
    std::fs::write(first.join("Only in first.md"), b"x").unwrap();
    create_workspace(app_data.path(), "Second", &parent.path().join("Second"));
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);

    harness
        .get_by_role_and_label(Role::Button, "Switch workspace. Current workspace: Second")
        .click();
    harness.run();
    assert!(
        harness
            .get_by_role_and_label(Role::Button, "Second")
            .accesskit_node()
            .is_disabled()
    );
    harness.get_by_role_and_label(Role::Button, "Open folder…");
    harness.get_by_role_and_label(Role::Button, "New workspace…");
    harness.get_by_label("Remove from recents");
    assert!(harness.unnamed_widgets().is_empty());

    harness.get_by_role_and_label(Role::Button, "First").click();
    settle(&mut harness);

    harness.get_by_role_and_label(Role::Button, "Switch workspace. Current workspace: First");
    harness.get_by_role_and_label(Role::TreeItem, "Only in first");
}

#[test]
fn remove_from_recents_in_the_switcher_drops_the_entry() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let first = parent.path().join("First");
    create_workspace(app_data.path(), "First", &first);
    create_workspace(app_data.path(), "Second", &parent.path().join("Second"));
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::Button, "Switch workspace. Current workspace: Second")
        .click();
    harness.run();

    harness.get_by_label("Remove from recents").click();
    harness.run();
    let entries: Vec<_> = harness.get_all_by_label("First").collect();
    entries.last().unwrap().click();
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "Switch workspace. Current workspace: Second")
        .click();
    harness.run();
    assert!(harness.query_by_label("First").is_none());
    assert!(first.join(".noura/workspace.yaml").is_file());
}

#[test]
fn new_workspace_from_the_switcher_asks_for_a_name_then_a_folder() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Current", &parent.path().join("Current"));
    let fresh = parent.path().join("Fresh");
    std::fs::create_dir(&fresh).unwrap();
    let picker = FakePicker::default();
    picker.answer(Some(&fresh));
    let mut harness = harness(app_data.path(), picker);
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::Button, "Switch workspace. Current workspace: Current")
        .click();
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "New workspace…")
        .click();
    harness.run();
    harness
        .get_by_role_and_label(Role::TextInput, "Workspace name")
        .type_text("Fresh start");
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Choose folder…")
        .click();
    settle(&mut harness);

    harness.get_by_role_and_label(
        Role::Button,
        "Switch workspace. Current workspace: Fresh start",
    );
}
