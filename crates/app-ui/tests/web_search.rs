//! The Web search section of Settings through AccessKit: the key field
//! hides what you type, Settings never shows the key, and no prompt asks
//! per workspace.

mod support;

use app_core::Platform;
use egui::accesskit::Role;
use egui_kittest::kittest::{By, NodeT, Queryable};
use support::{
    FakePicker, FakeSystem, create_workspace, harness_with,
    kit::{Ui, run},
    settle,
};

const KEY: &str = "BSA-ui-test-0123456789";

/// The text of every label under the group `group`.
fn labels_in(harness: &Ui, group: &str) -> Vec<String> {
    harness
        .get_by_role_and_label(Role::Group, group)
        .query_all(By::new().role(Role::Label))
        .filter_map(|node| node.accesskit_node().value())
        .collect()
}

fn all_labels(harness: &Ui) -> Vec<String> {
    harness
        .query_all(By::new().role(Role::Label))
        .filter_map(|node| node.accesskit_node().value())
        .collect()
}

#[test]
fn the_key_field_hides_the_key_and_no_prompt_asks_per_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), &parent.path().join("Garden"));
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system.clone());
    settle(&mut harness);
    harness.state_mut().app_mut().open_settings_at("web-search");
    run(&mut harness);

    assert!(labels_in(&harness, "Brave Search key").contains(&"No key".to_owned()));
    harness
        .get_by_role_and_label(Role::Group, "Brave Search key")
        .get_by_role_and_label(Role::PasswordInput, "Key")
        .focus();
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::Group, "Brave Search key")
        .get_by_role_and_label(Role::PasswordInput, "Key")
        .type_text(KEY);
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::Group, "Brave Search key")
        .get_by_role_and_label(Role::Button, "Save key")
        .click_accesskit();
    run(&mut harness);

    assert!(labels_in(&harness, "Brave Search key").contains(&"Saved".to_owned()));
    harness
        .get_by_role_and_label(Role::Group, "Brave Search key")
        .get_by_role_and_label(Role::PasswordInput, "New key");
    assert_eq!(system.credentials.secrets(), [KEY.to_owned()]);
    assert!(
        all_labels(&harness)
            .iter()
            .all(|label| !label.contains(KEY))
    );

    // Adding the key is your say-so: no section or prompt asks per
    // workspace.
    assert!(
        harness
            .query_by_role_and_label(Role::Group, "This workspace")
            .is_none()
    );
    assert!(harness.query_by_role(Role::AlertDialog).is_none());

    harness
        .get_by_role_and_label(Role::Group, "Brave Search key")
        .get_by_role_and_label(Role::Button, "Remove key")
        .click_accesskit();
    run(&mut harness);
    assert!(system.credentials.secrets().is_empty());
}
