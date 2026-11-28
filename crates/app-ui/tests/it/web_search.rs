//! The Web search section of Settings through AccessKit: the key field
//! hides what you type, Settings never shows the key, and no prompt asks
//! per workspace.

use crate::support;
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
    // The Brave Search key lives on the AI plugin's page in Plugins.
    let Some(component) = support::plugins::built("ai_plugin") else {
        return;
    };
    let plugin = support::plugins::folder(
        parent.path(),
        include_str!("../../../plugins/ai/plugin.toml"),
        "ai_plugin.wasm",
        &component,
    );
    support::plugins::install_and_enable(app_data.path(), &plugin, "ai");
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

/// The Brave Search card draws below the AI providers on the AI plugin's
/// page. Each of its lines keeps its own room, with a gap before the next:
/// the title, "No key", the field's label, the field and Save key. The
/// providers above once left the spacing at zero, which jammed them.
#[test]
fn the_brave_search_card_s_lines_never_overlap() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), &parent.path().join("Garden"));
    let Some(component) = support::plugins::built("ai_plugin") else {
        return;
    };
    let plugin = support::plugins::folder(
        parent.path(),
        include_str!("../../../plugins/ai/plugin.toml"),
        "ai_plugin.wasm",
        &component,
    );
    support::plugins::install_and_enable(app_data.path(), &plugin, "ai");
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system);
    settle(&mut harness);
    harness.state_mut().app_mut().open_settings_at("web-search");
    run(&mut harness);
    let card = harness.get_by_role_and_label(Role::Group, "Brave Search key");
    card.scroll_to_me();
    run(&mut harness);

    let card = harness.get_by_role_and_label(Role::Group, "Brave Search key");
    let mut lines: Vec<(String, egui::Rect)> = card
        .query_all(By::new().role(Role::Label))
        .map(|node| {
            (
                node.accesskit_node().value().unwrap_or_default(),
                node.rect(),
            )
        })
        .collect();
    let field = card.get_by_role_and_label(Role::PasswordInput, "Key");
    lines.push(("the key field".to_owned(), field.rect()));
    let save = card.get_by_role_and_label(Role::Button, "Save key");
    lines.push(("Save key".to_owned(), save.rect()));
    for (name, label) in
        ["Brave Search key", "No key", "Key"]
            .iter()
            .zip(["Brave Search key", "No key", "Key"])
    {
        assert!(
            lines.iter().any(|(text, _)| text == label),
            "{name} is missing: {lines:?}"
        );
    }
    lines.sort_by(|(_, a), (_, b)| a.top().total_cmp(&b.top()));
    for pair in lines.windows(2) {
        let ((above, upper), (below, lower)) = (&pair[0], &pair[1]);
        assert!(
            lower.top() - upper.bottom() >= 4.0,
            "{below:?} at {lower:?} sits on {above:?} at {upper:?}"
        );
    }
}
