//! Settings > Speech through AccessKit: the Models table with its search,
//! its Live preview and Translates filters, its sections, the radio that
//! picks the model in use, download with its progress in the row, and
//! delete, and a plugin's shortcut shown with its keys as this system
//! writes them.

mod support;

use app_core::Platform;
use egui::accesskit::{Role, Toggled};
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

/// The rows of section `group`, by model name.
fn rows_in(harness: &Ui, group: &str) -> Vec<String> {
    harness
        .get_by_role_and_label(Role::Group, group)
        .query_all(By::new().role(Role::Group))
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

fn has_section(harness: &Ui, group: &str) -> bool {
    harness
        .query_by_role_and_label(Role::Group, group)
        .is_some()
}

fn toggled(harness: &Ui, role: Role, label: &str) -> bool {
    harness
        .get_by_role_and_label(role, label)
        .accesskit_node()
        .toggled()
        == Some(Toggled::True)
}

fn speech_settings(
    app_data: &std::path::Path,
    system: std::sync::Arc<FakeSystem>,
) -> (Ui, tempfile::TempDir) {
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data, &parent.path().join("Garden"));
    let mut harness = harness_with(app_data, FakePicker::default(), system);
    settle(&mut harness);
    harness.state_mut().app_mut().open_settings_at("speech");
    run(&mut harness);
    (harness, parent)
}

/// A model file where the catalog expects it.
fn downloaded(app_data: &std::path::Path, file: &str) -> std::path::PathBuf {
    let path = app_data.join("speech/models").join(file);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, b"GGUF").unwrap();
    path
}

fn search(harness: &mut Ui, text: &str) {
    let field = harness.get_by_label("Search models");
    field.focus();
    field.type_text(text);
    run(harness);
}

#[test]
fn the_models_table_lists_your_sections_with_grades_and_labeled_controls() {
    let app_data = tempfile::tempdir().unwrap();
    let (harness, _parent) = speech_settings(app_data.path(), FakeSystem::new(Platform::current()));

    harness.get_by_role_and_label(Role::Group, "Models");
    harness.get_by_label("Search models");
    harness.get_by_label("Language");
    assert!(!toggled(&harness, Role::Button, "Live preview"));
    assert!(!toggled(&harness, Role::Button, "Translates"));
    assert!(labels_in(&harness, "On this computer").contains(&"No models yet".to_owned()));
    let yours = harness.get_by_role_and_label(Role::Group, "For your languages");
    let first = yours
        .query_all(By::new().role(Role::Group))
        .next()
        .expect("a model for English");
    let name = first.accesskit_node().label().unwrap();
    assert!(
        first.query_all(By::new().role(Role::Label)).any(|node| node
            .accesskit_node()
            .value()
            .as_deref()
            == Some("Recommended")),
        "{name} comes first and is recommended"
    );
    yours.get_by_role_and_label(Role::Button, &format!("Download {name}"));
    yours.get_by_role_and_label(Role::Group, "Parakeet Ultra");
    let grades = labels_in(&harness, "Parakeet Ultra");
    assert!(grades.contains(&"English A".to_owned()), "{grades:?}");
    // English is the only column until you speak another language.
    assert!(!grades.iter().any(|text| text.contains("French")));
    harness
        .get_by_role_and_label(Role::Group, "Parakeet Ultra")
        .get_by_role_and_label(Role::Meter, "Speed");
    // The onboarding block and its Not now are gone.
    assert!(!has_section(&harness, "Get started"));
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "Not now")
            .is_none()
    );
    assert!(!has_section(&harness, "Other models"));
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
}

#[test]
fn another_language_adds_its_column_and_grades() {
    let app_data = tempfile::tempdir().unwrap();
    let (mut harness, _parent) =
        speech_settings(app_data.path(), FakeSystem::new(Platform::current()));
    harness
        .state_mut()
        .app_mut()
        .set_speech_language(Some("fr"))
        .unwrap();
    run(&mut harness);

    harness
        .get_by_role_and_label(Role::Group, "For your languages")
        .get_by_role_and_label(Role::Group, "Parakeet Ultra");
    let grades = labels_in(&harness, "Parakeet Ultra");
    assert!(grades.contains(&"English A".to_owned()), "{grades:?}");
    assert!(grades.contains(&"French A".to_owned()), "{grades:?}");
    // Parakeet TDT v2 hears only English.
    assert!(!rows_in(&harness, "For your languages").contains(&"Parakeet TDT 0.6B v2".to_owned()));
}

#[test]
fn search_and_the_filters_narrow_the_table() {
    let app_data = tempfile::tempdir().unwrap();
    let (mut harness, _parent) =
        speech_settings(app_data.path(), FakeSystem::new(Platform::current()));

    search(&mut harness, "whisper large");
    assert!(!has_section(&harness, "On this computer"));
    let mut found = rows_in(&harness, "For your languages");
    if has_section(&harness, "Other models") {
        found.extend(rows_in(&harness, "Other models"));
    }
    assert!(!found.is_empty());
    assert!(
        found.iter().all(|name| name.contains("Whisper Large")),
        "{found:?}"
    );

    search(&mut harness, " zzz");
    harness.get_by_label("No models match");

    harness.get_by_label("Search models").focus();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.key_press(egui::Key::Backspace);
    run(&mut harness);
    assert!(has_section(&harness, "On this computer"));

    harness
        .get_by_role_and_label(Role::Button, "Live preview")
        .click_accesskit();
    run(&mut harness);
    assert!(toggled(&harness, Role::Button, "Live preview"));
    let live = rows_in(&harness, "For your languages");
    assert!(!live.is_empty());
    for name in &live {
        harness
            .get_by_role_and_label(Role::Group, name)
            .get_by_role_and_label(Role::Image, "Live preview");
    }

    harness
        .get_by_role_and_label(Role::Button, "Translates")
        .click_accesskit();
    run(&mut harness);
    // No model both streams and translates.
    harness.get_by_label("No models match");
}

#[test]
fn the_radio_picks_the_model_in_use_and_delete_removes_its_file() {
    let app_data = tempfile::tempdir().unwrap();
    let ultra = downloaded(app_data.path(), "parakeet-ultra-Q8_0.gguf");
    let small = downloaded(app_data.path(), "whisper-small-Q8_0.gguf");
    let (mut harness, _parent) =
        speech_settings(app_data.path(), FakeSystem::new(Platform::current()));

    let here = rows_in(&harness, "On this computer");
    assert_eq!(here, ["Parakeet Ultra", "Whisper Small"]);
    assert!(toggled(&harness, Role::RadioButton, "Parakeet Ultra"));
    assert!(!toggled(&harness, Role::RadioButton, "Whisper Small"));
    assert!(!rows_in(&harness, "For your languages").contains(&"Parakeet Ultra".to_owned()));

    harness
        .get_by_role_and_label(Role::RadioButton, "Whisper Small")
        .click_accesskit();
    run(&mut harness);
    assert!(toggled(&harness, Role::RadioButton, "Whisper Small"));
    assert!(!toggled(&harness, Role::RadioButton, "Parakeet Ultra"));
    assert_eq!(rows_in(&harness, "On this computer")[0], "Whisper Small");

    harness
        .get_by_role_and_label(Role::Button, "Delete Whisper Small")
        .click_accesskit();
    run(&mut harness);
    assert!(!small.exists());
    assert!(ultra.exists());
    assert_eq!(rows_in(&harness, "On this computer"), ["Parakeet Ultra"]);
    assert!(toggled(&harness, Role::RadioButton, "Parakeet Ultra"));

    harness
        .get_by_role_and_label(Role::Button, "Delete Parakeet Ultra")
        .click_accesskit();
    run(&mut harness);
    assert!(!ultra.exists());
    assert!(labels_in(&harness, "On this computer").contains(&"No models yet".to_owned()));
}

#[test]
fn a_download_shows_its_progress_in_the_row() {
    let app_data = tempfile::tempdir().unwrap();
    // A server that takes the connection and never answers, so the
    // download stays under way.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let system = FakeSystem::new(Platform::current());
    *system.speech_endpoint.lock().unwrap() = format!("http://{}", listener.local_addr().unwrap());
    let (mut harness, _parent) = speech_settings(app_data.path(), system);

    harness
        .get_by_role_and_label(Role::Button, "Download Parakeet Ultra")
        .click_accesskit();
    // The row spins while it downloads, so it never settles.
    harness.run_steps(4);
    let row = harness.get_by_role_and_label(Role::Group, "Parakeet Ultra");
    assert!(
        row.query_all(By::new().role(Role::Label))
            .filter_map(|node| node.accesskit_node().value())
            .any(|text| text.starts_with("Downloading 0% of "))
    );
    row.get_by_role_and_label(Role::Button, "Cancel Parakeet Ultra")
        .click_accesskit();
    harness.run_steps(2);
    drop(listener);
}

#[test]
fn a_shortcut_shows_its_keys_as_this_system_writes_them() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), &parent.path().join("Garden"));
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
        .get_by_role_and_label(Role::TextInput, "Dictation");
    assert_eq!(field.accesskit_node().value().as_deref(), Some(keys));
}
