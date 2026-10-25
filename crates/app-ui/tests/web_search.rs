//! The Web search section of Settings and Noura's web consent prompt
//! through AccessKit: the key field hides what you type and Settings
//! never shows the key, and the prompt names Brave Search, says what
//! leaves the computer, starts on Deny and denies on Escape.

mod support;

use app_core::Platform;
use egui::{Key, accesskit::Role};
use egui_kittest::kittest::{By, NodeT, Queryable};
use support::{
    FakePicker, FakeSystem, create_workspace, harness_with,
    kit::{Ui, focused, press, run},
    settle,
};

const KEY: &str = "BSA-ui-test-0123456789";

const TITLE: &str = "Let the AI search the web with Brave Search?";

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
fn the_key_field_hides_the_key_and_noura_s_prompt_names_brave_search_and_what_leaves() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Garden", &parent.path().join("Garden"));
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system.clone());
    settle(&mut harness);
    harness.state_mut().app_mut().open_settings_at("web-search");
    run(&mut harness);

    assert!(labels_in(&harness, "Brave Search key").contains(&"No key yet.".to_owned()));
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

    assert!(labels_in(&harness, "Brave Search key").contains(&"Your key is saved.".to_owned()));
    harness
        .get_by_role_and_label(Role::Group, "Brave Search key")
        .get_by_role_and_label(
            Role::PasswordInput,
            "New key (leave empty to keep the saved one)",
        );
    assert_eq!(system.credentials.secrets(), [KEY.to_owned()]);
    assert!(
        all_labels(&harness)
            .iter()
            .all(|label| !label.contains(KEY))
    );

    // Allowing opens Noura's prompt, which starts on Deny.
    assert!(
        labels_in(&harness, "This workspace")
            .contains(&"The AI can't search the web in Garden yet.".to_owned())
    );
    harness
        .get_by_role_and_label(Role::Group, "This workspace")
        .get_by_role_and_label(Role::Button, "Allow for this workspace")
        .click();
    run(&mut harness);
    let dialog = harness.get_by_role_and_label(Role::AlertDialog, TITLE);
    dialog.get_by_role_and_label(Role::Button, "Allow");
    let said: Vec<String> = dialog
        .query_all(By::new().role(Role::Label))
        .filter_map(|node| node.accesskit_node().value())
        .collect();
    assert!(
        said.contains(
            &"When a chat in Garden uses the web, the search queries the AI writes go to Brave \
              Search, and the addresses of the pages it reads go to the sites that host them. \
              Both leave this computer."
                .to_owned()
        ),
        "{said:?}"
    );
    assert_eq!(focused(&harness).as_deref(), Some("Deny"));

    // Escape counts as Deny.
    press(&mut harness, Key::Escape);
    assert!(
        harness
            .query_by_role_and_label(Role::AlertDialog, TITLE)
            .is_none()
    );
    assert!(!harness.state_mut().app_mut().web_search().consented);

    harness
        .get_by_role_and_label(Role::Group, "This workspace")
        .get_by_role_and_label(Role::Button, "Allow for this workspace")
        .click();
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::AlertDialog, TITLE)
        .get_by_role_and_label(Role::Button, "Allow")
        .click();
    run(&mut harness);
    assert!(harness.state_mut().app_mut().web_search().consented);
    assert!(
        labels_in(&harness, "This workspace")
            .contains(&"The AI can search the web in Garden.".to_owned())
    );

    // Turning it off takes the consent back.
    harness
        .get_by_role_and_label(Role::Group, "This workspace")
        .get_by_role_and_label(Role::Button, "Turn off for this workspace")
        .click();
    run(&mut harness);
    assert!(!harness.state_mut().app_mut().web_search().consented);

    harness
        .get_by_role_and_label(Role::Group, "Brave Search key")
        .get_by_role_and_label(Role::Button, "Remove key")
        .click();
    run(&mut harness);
    assert!(system.credentials.secrets().is_empty());
}
