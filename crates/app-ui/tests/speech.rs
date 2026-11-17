//! Settings > Speech through AccessKit: onboarding recommends Parakeet
//! Ultra, the models on this computer and those for your language show
//! with their grade and a labeled button, Delete removes a model's file,
//! the language and paste choices are labeled controls, and a plugin's
//! shortcut shows its keys as this system writes them.

mod support;

use app_core::Platform;
use egui::accesskit::Role;
use egui_kittest::kittest::{By, NodeT, Queryable};
use support::{
    FakePicker, FakeSystem, create_workspace, harness_with,
    kit::{Ui, run},
    plugins, settle,
};

fn labels_in(harness: &Ui, group: &str) -> Vec<String> {
    harness
        .get_by_role_and_label(Role::Group, group)
        .query_all(By::new().role(Role::Label))
        .filter_map(|node| node.accesskit_node().value())
        .collect()
}

fn speech_settings(app_data: &std::path::Path) -> (Ui, tempfile::TempDir) {
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data, "Garden", &parent.path().join("Garden"));
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_with(app_data, FakePicker::default(), system);
    settle(&mut harness);
    harness.state_mut().app_mut().open_settings_at("speech");
    run(&mut harness);
    (harness, parent)
}

#[test]
fn onboarding_recommends_parakeet_ultra_and_each_model_has_a_labeled_button() {
    let app_data = tempfile::tempdir().unwrap();
    let (mut harness, _parent) = speech_settings(app_data.path());

    let started = harness.get_by_role_and_label(Role::Group, "Get started");
    started.get_by_role_and_label(Role::Group, "Parakeet Ultra");
    started.get_by_role_and_label(Role::Button, "Download Parakeet Ultra");
    assert!(labels_in(&harness, "On this computer").contains(&"No models yet".to_owned()));
    let yours = harness.get_by_role_and_label(Role::Group, "For your language");
    assert!(
        yours
            .query_all(By::new().role(Role::Button))
            .filter_map(|node| node.accesskit_node().label())
            .any(|label| label.starts_with("Download "))
    );
    harness.get_by_label("Language");
    // Dictation's settings hold how text goes in.
    assert!(
        harness
            .query_by_role_and_label(Role::Switch, "Paste by typing")
            .is_none()
    );
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());

    harness
        .get_by_role_and_label(Role::Button, "Not now")
        .click_accesskit();
    run(&mut harness);
    assert!(
        harness
            .query_by_role_and_label(Role::Group, "Get started")
            .is_none()
    );
}

#[test]
fn a_downloaded_model_shows_on_this_computer_and_delete_removes_its_file() {
    let app_data = tempfile::tempdir().unwrap();
    // Parakeet Ultra's file, as the catalog names it.
    let file = app_data
        .path()
        .join("speech/models/parakeet-ultra-Q8_0.gguf");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, b"GGUF").unwrap();
    let (mut harness, _parent) = speech_settings(app_data.path());

    let here = harness.get_by_role_and_label(Role::Group, "On this computer");
    let row = here.get_by_role_and_label(Role::Group, "Parakeet Ultra");
    assert!(
        row.query_all(By::new().role(Role::Label))
            .filter_map(|node| node.accesskit_node().value())
            .any(|label| label == "A")
    );
    row.get_by_role_and_label(Role::Button, "Delete Parakeet Ultra")
        .click_accesskit();
    run(&mut harness);

    assert!(!file.exists());
    assert!(labels_in(&harness, "On this computer").contains(&"No models yet".to_owned()));
}

#[test]
fn a_shortcut_shows_its_keys_as_this_system_writes_them() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Garden", &parent.path().join("Garden"));
    let manifest = plugins::probe_manifest_with(&["name = \"global-shortcuts\"\n\
         shortcuts = [{ id = \"paste\", title = \"Dictation\", keys = \"Alt+Space\" }]"]);
    let folder = plugins::folder(parent.path(), &manifest, "plugin.wasm", &probe);
    plugins::install_and_enable(app_data.path(), &folder, "probe");
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system);
    settle(&mut harness);
    harness.state_mut().app_mut().open_settings_at("speech");
    run(&mut harness);

    let keys = if cfg!(target_os = "macos") {
        "\u{2325}Space"
    } else {
        "Alt+Space"
    };
    let field = harness
        .get_by_role_and_label(Role::Group, "Shortcuts")
        .get_by_role_and_label(Role::TextInput, "Dictation keys");
    assert_eq!(field.accesskit_node().value().as_deref(), Some(keys));
}
