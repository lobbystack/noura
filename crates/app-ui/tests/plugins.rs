//! Plugins through AccessKit: the install dialog and its keyboard path,
//! the Plugins page, and plugin pages drawn from UI nodes.

mod support;

use egui::{Key, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{
    FakePicker, create_workspace, harness, plugins,
    plugins::{HELLO_MANIFEST, install_and_enable},
    settle,
};

type Ui = Harness<'static, app_ui::NouraUi>;

struct Opened {
    harness: Ui,
    picker: FakePicker,
    /// The app data and the folders around the workspace, which last as
    /// long as the test binds them.
    folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace named Garden with two notes, where `prepare` may install
/// plugins, opened at launch.
fn opened(prepare: impl FnOnce(&std::path::Path, &std::path::Path)) -> Opened {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    for name in ["One.md", "Two.md"] {
        std::fs::write(folder.join(name), b"x").unwrap();
    }
    prepare(app_data.path(), parent.path());
    let picker = FakePicker::default();
    let mut harness = harness(app_data.path(), picker.clone());
    settle(&mut harness);
    Opened {
        harness,
        picker,
        folders: (app_data, parent),
    }
}

fn dialog<'a>(harness: &'a Ui, label: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::Dialog, label)
}

fn open_plugins_page(harness: &mut Ui) {
    harness
        .get_by_role_and_label(Role::Button, "Settings")
        .click();
    harness.run();
    dialog(harness, "Settings")
        .get_by_role_and_label(Role::Button, "Plugins")
        .click();
    harness.run();
}

fn page_link<'a>(harness: &'a Ui, title: &'a str) -> egui_kittest::Node<'a> {
    harness
        .get_by_role_and_label(Role::Navigation, "Pages")
        .get_by_role_and_label(Role::Link, title)
}

/// Every label in `node`, top to bottom.
fn labels(node: &egui_kittest::Node<'_>) -> Vec<String> {
    node.query_all(egui_kittest::kittest::By::new().role(Role::Label))
        .filter_map(|label| label.accesskit_node().value())
        .collect()
}

#[test]
fn the_install_dialog_lists_each_capability_and_installs_from_the_keyboard() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let mut opened = opened(|_, _| {});
    let folder = plugins::folder(
        opened.folders.1.path(),
        HELLO_MANIFEST,
        "hello_plugin.wasm",
        &hello,
    );
    opened.picker.answer(Some(&folder));
    let harness = &mut opened.harness;
    open_plugins_page(harness);

    dialog(harness, "Settings")
        .get_by_role_and_label(Role::Button, "Install from folder\u{2026}")
        .click();
    harness.run();

    let install = dialog(harness, "Install plugin");
    let shown = labels(&install);
    for expected in [
        "Install Hello?",
        "Read the files and folders in your workspace.",
        "Add pages to the rail and show panels in them.",
    ] {
        assert!(shown.iter().any(|label| label == expected), "{shown:?}");
    }
    // The keyboard starts on Cancel, so Enter alone declines.
    assert!(
        install
            .get_by_role_and_label(Role::Button, "Cancel")
            .is_focused()
    );
    harness.key_press(Key::Tab);
    harness.run();
    assert!(
        dialog(harness, "Install plugin")
            .get_by_role_and_label(Role::Button, "Install")
            .is_focused()
    );
    harness.key_press(Key::Enter);
    harness.run();

    assert!(
        harness
            .query_by_role_and_label(Role::Dialog, "Install plugin")
            .is_none()
    );
    let row = dialog(harness, "Settings").get_by_role_and_label(Role::Group, "Hello");
    let shown = labels(&row);
    assert!(
        shown.iter().any(|label| label == "Version 0.1.0"),
        "{shown:?}"
    );
    assert!(
        shown
            .iter()
            .any(|label| label == "Read the files and folders in your workspace."),
        "{shown:?}"
    );
    assert!(
        harness
            .state()
            .app()
            .plugins()
            .iter()
            .any(|row| row.installed)
    );
}

#[test]
fn escape_closes_the_install_dialog_and_installs_nothing() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let mut opened = opened(|_, _| {});
    let folder = plugins::folder(
        opened.folders.1.path(),
        HELLO_MANIFEST,
        "hello_plugin.wasm",
        &hello,
    );
    opened.picker.answer(Some(&folder));
    let harness = &mut opened.harness;
    open_plugins_page(harness);
    dialog(harness, "Settings")
        .get_by_role_and_label(Role::Button, "Install from folder\u{2026}")
        .click();
    harness.run();

    harness.key_press(Key::Escape);
    harness.run();

    assert!(
        harness
            .query_by_role_and_label(Role::Dialog, "Install plugin")
            .is_none()
    );
    assert!(
        harness
            .query_by_role_and_label(Role::Dialog, "Settings")
            .is_some()
    );
    assert!(
        !harness
            .state()
            .app()
            .plugins()
            .iter()
            .any(|row| row.installed)
    );
}

#[test]
fn the_hello_page_shows_the_file_count_and_its_button_counts_from_the_keyboard() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let mut opened = opened(|app_data, parent| {
        let folder = plugins::folder(parent, HELLO_MANIFEST, "hello_plugin.wasm", &hello);
        install_and_enable(app_data, &folder, "hello");
    });
    let harness = &mut opened.harness;

    page_link(harness, "Hello").click();
    harness.run();

    let panel = harness.get_by_role_and_label(Role::Group, "Hello");
    let shown = labels(&panel);
    assert_eq!(shown, ["This workspace has 2 files.", "Count: 0"]);
    panel.get_by_role_and_label(Role::Button, "Add one").focus();
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();

    let panel = harness.get_by_role_and_label(Role::Group, "Hello");
    assert_eq!(labels(&panel), ["This workspace has 2 files.", "Count: 1"]);
    assert!(
        panel
            .get_by_role_and_label(Role::Button, "Add one")
            .is_focused()
    );
}

#[test]
fn markdown_shows_images_as_their_description_and_loads_none() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let mut opened = opened(|app_data, parent| {
        let folder = plugins::folder(parent, &plugins::probe_manifest(), "plugin.wasm", &probe);
        install_and_enable(app_data, &folder, "probe");
    });
    let harness = &mut opened.harness;

    page_link(harness, "Markdown").click();
    harness.run();

    let panel = harness.get_by_role_and_label(Role::Group, "Markdown");
    let shown = labels(&panel).join(" ");
    for expected in [
        "Tomatoes",
        "Grow",
        "tomatoes",
        "full sun",
        "A ripe tomato",
        "Water daily",
    ] {
        assert!(
            shown.contains(expected),
            "{expected} is missing from {shown:?}"
        );
    }
    assert!(panel.query_by_role(Role::Image).is_none());
    assert!(
        panel
            .query_by_role_and_label(Role::Link, "the guide")
            .is_some()
    );
}

#[test]
fn a_view_that_breaks_the_rules_draws_an_error_in_its_place() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let mut opened = opened(|app_data, parent| {
        let folder = plugins::folder(parent, &plugins::probe_manifest(), "plugin.wasm", &probe);
        install_and_enable(app_data, &folder, "probe");
    });
    let harness = &mut opened.harness;

    page_link(harness, "Broken").click();
    harness.run();

    let alert = harness.get_by_role(Role::Alert);
    let shown = labels(&alert).join(" ");
    assert!(shown.contains("cycle"), "{shown}");
    // Noura keeps running: another page still draws.
    page_link(harness, "Probe").click();
    harness.run();
    assert!(
        harness
            .query_by_role_and_label(Role::Group, "Probe")
            .is_some()
    );
}
