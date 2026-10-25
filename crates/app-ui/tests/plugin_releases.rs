//! Installing a plugin from a release on GitHub through AccessKit: the
//! address field in Settings, the install dialog with its warning, and the
//! plugin's row with Check for updates. Downloads go to a stand-in for
//! GitHub's servers.

mod support;

use std::sync::Arc;

use app_core::fake_github::{Entry, FakeGitHub, archive};
use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness_registering, plugins, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

const RELEASE: &str = "https://github.com/maker/probe-plugin/releases/tag/v0.1.0";

struct Opened {
    harness: Ui,
    app_data: tempfile::TempDir,
    _parent: tempfile::TempDir,
}

/// A workspace open in a window whose release downloads go to `github`,
/// which publishes the probe plugin as release v0.1.0.
fn opened(probe: &[u8], github: &Arc<FakeGitHub>) -> Opened {
    let manifest = "id = \"probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\n\
                    interface-version = \"0.1.0\"\nentry = \"plugin.wasm\"\n\
                    platforms = [\"desktop\"]\n\n[[capabilities]]\nname = \"views\"\n\n\
                    [[capabilities]]\nname = \"network\"\nhosts = [\"api.example.com\"]\n\n\
                    [[capabilities]]\nname = \"commands\"\ntargets = [\"tasks/add\"]\n";
    github.publish_archive(
        "maker/probe-plugin",
        "v0.1.0",
        archive(&[
            Entry::File("plugin.toml", manifest.as_bytes()),
            Entry::File("plugin.wasm", probe),
        ]),
    );
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Garden", &parent.path().join("Garden"));
    let transport = github.clone();
    let mut harness = harness_registering(app_data.path(), FakePicker::default(), move |app| {
        app.set_plugin_release_transport(transport);
    });
    settle(&mut harness);
    Opened {
        harness,
        app_data,
        _parent: parent,
    }
}

fn dialog<'a>(harness: &'a Ui, label: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::Dialog, label)
}

/// Every label in `node`, top to bottom.
fn labels(node: &egui_kittest::Node<'_>) -> Vec<String> {
    node.query_all(egui_kittest::kittest::By::new().role(Role::Label))
        .filter_map(|label| label.accesskit_node().value())
        .collect()
}

/// Open Settings' Plugins section, paste `address` and press Install from
/// GitHub, then wait for the download.
fn install_from_github(harness: &mut Ui, address: &str) {
    harness
        .get_by_role_and_label(Role::Button, "Settings")
        .click();
    harness.run();
    dialog(harness, "Settings")
        .get_by_role_and_label(Role::Button, "Plugins")
        .click();
    harness.run();
    dialog(harness, "Settings")
        .get_by_role_and_label(Role::TextInput, "Release address")
        .click();
    harness.run();
    dialog(harness, "Settings")
        .get_by_role_and_label(Role::TextInput, "Release address")
        .type_text(address);
    harness.run();
    dialog(harness, "Settings")
        .get_by_role_and_label(Role::Button, "Install from GitHub\u{2026}")
        .click();
    settle(harness);
}

#[test]
fn the_install_dialog_shows_the_release_with_a_warning_and_cancel_installs_nothing() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let github = FakeGitHub::new();
    let mut opened = opened(&probe, &github);
    let harness = &mut opened.harness;

    install_from_github(harness, RELEASE);

    let install = dialog(harness, "Install plugin");
    let shown = labels(&install);
    for expected in [
        "Install Probe?",
        "Version 0.1.0",
        RELEASE,
        "Nobody reviewed this plugin. Anyone can publish a release on GitHub, so install it \
         if you trust its author.",
        "Add pages to the rail and show panels in them.",
        "Connect to api.example.com.",
        "network",
        "Add commands to the command palette and offer them to other plugins.",
        "Run the add command of the tasks plugin.",
    ] {
        assert!(
            shown.iter().any(|label| label == expected),
            "{expected}: {shown:?}"
        );
    }
    let cancel = install.get_by_role_and_label(Role::Button, "Cancel");
    assert!(cancel.is_focused(), "the keyboard starts on Cancel");
    cancel.click();
    harness.run();

    assert!(
        harness
            .query_by_role_and_label(Role::Dialog, "Install plugin")
            .is_none()
    );
    assert!(
        !harness
            .state()
            .app()
            .plugins()
            .iter()
            .any(|row| row.installed)
    );
    assert!(
        !opened
            .app_data
            .path()
            .join("plugins/installed.json")
            .exists()
    );
}

#[test]
fn an_address_that_isnt_a_release_says_why_in_the_dialog() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let github = FakeGitHub::new();
    let mut opened = opened(&probe, &github);
    let harness = &mut opened.harness;

    install_from_github(harness, "https://example.com/probe.zip");

    let install = dialog(harness, "Install plugin");
    let alert = install.get_by_role(Role::Alert);
    let text = alert.accesskit_node().label().unwrap_or_default();
    assert!(text.contains("release on GitHub"), "{text}");
    assert!(github.requests().is_empty());
}

#[test]
fn a_plugin_from_github_checks_for_updates_from_its_row() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let github = FakeGitHub::new();
    let mut opened = opened(&probe, &github);
    let harness = &mut opened.harness;
    install_from_github(harness, RELEASE);
    dialog(harness, "Install plugin")
        .get_by_role_and_label(Role::Button, "Install")
        .click();
    settle(harness);

    let row = dialog(harness, "Settings").get_by_role_and_label(Role::Group, "Probe");
    let shown = labels(&row);
    assert!(shown.iter().any(|label| label == RELEASE), "{shown:?}");
    // The row may sit below the visible part of Settings, so the keyboard
    // presses its button.
    row.get_by_role_and_label(Role::Button, "Check for updates")
        .focus();
    harness.run();
    harness.key_press(egui::Key::Enter);
    settle(harness);

    let row = dialog(harness, "Settings").get_by_role_and_label(Role::Group, "Probe");
    let shown = labels(&row);
    assert!(
        shown
            .iter()
            .any(|label| label == "Version 0.1.0 is the newest release."),
        "{shown:?}"
    );
}
