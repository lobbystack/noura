//! A plugin's properties panel through AccessKit: an object of the
//! plugin's own type opens from the tree in the note editor with the
//! plugin's panel beside the body, in place of the kernel's.

mod support;

use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness, plugins, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

const RECIPE: &str = "---\nid: recipe_01jbcdefghjkmnpqrstvwxyz0a\ntype: recipe\ncourse: dessert\n\
                      servings: 6\ncreated: 2026-10-20T10:00:00Z\n\
                      updated: 2026-10-20T10:00:00Z\n---\nLayer and chill.\n";

const MANIFEST: &str = "id = \"recipes\"\nname = \"Recipes\"\nversion = \"0.1.0\"\n\
                        interface-version = \"0.1.0\"\nentry = \"plugin.wasm\"\n\
                        platforms = [\"desktop\"]\n\n[[capabilities]]\nname = \"object-types\"\n\n\
                        [[capabilities]]\nname = \"views\"\n";

/// Run frames, waiting in real time, until `done` holds.
fn wait_until(harness: &mut Ui, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(harness) {
        assert!(Instant::now() < deadline, "the screen never got there");
        std::thread::sleep(Duration::from_millis(20));
        harness.step();
    }
    harness.run_steps(4);
}

/// Every label in the properties panel, top to bottom.
fn panel_labels(harness: &Ui) -> Vec<String> {
    harness
        .get_by_role_and_label(Role::Group, "Properties")
        .get_all_by_role(Role::Label)
        .filter_map(|label| {
            label
                .accesskit_node()
                .value()
                .or(label.accesskit_node().label())
        })
        .collect()
}

#[test]
fn a_recipe_opens_with_the_plugins_panel_whose_button_changes_the_file() {
    let Some(component) = plugins::built("recipes_plugin") else {
        return;
    };
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    std::fs::create_dir_all(folder.join("recipes")).unwrap();
    std::fs::write(folder.join("recipes/Tiramisu.md"), RECIPE).unwrap();
    let plugin = plugins::folder(parent.path(), MANIFEST, "plugin.wasm", &component);
    plugins::install_and_enable(app_data.path(), &plugin, "recipes");
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);

    harness
        .get_by_role_and_label(Role::TreeItem, "recipes")
        .click();
    harness.run_steps(4);
    harness
        .get_by_role_and_label(Role::TreeItem, "Tiramisu")
        .click();
    harness.run_steps(4);

    harness.get_by_role_and_label(Role::Document, "Note text");
    let panel = harness.get_by_role_and_label(Role::Group, "Properties");
    panel.get_by_role_and_label(Role::Group, "Recipe");
    // The kernel's fields don't show.
    assert!(
        panel
            .query_by_role_and_label(Role::TextInput, "id")
            .is_none()
    );
    let labels = panel_labels(&harness);
    for expected in ["Course: dessert", "Servings: 6", "Vegetarian: no"] {
        assert!(labels.iter().any(|label| label == expected), "{labels:?}");
    }

    harness
        .get_by_role_and_label(Role::Group, "Properties")
        .get_by_role_and_label(Role::Button, "Mark vegetarian")
        .click();
    wait_until(&mut harness, |harness| {
        panel_labels(harness)
            .iter()
            .any(|label| label == "Vegetarian: yes")
    });

    let written = std::fs::read_to_string(folder.join("recipes/Tiramisu.md")).unwrap();
    assert!(written.contains("\nvegetarian: true\n"), "{written}");
    assert!(written.ends_with("---\nLayer and chill.\n"), "{written}");
    harness
        .get_by_role_and_label(Role::Group, "Properties")
        .get_by_role_and_label(Role::Button, "Mark not vegetarian");
}
