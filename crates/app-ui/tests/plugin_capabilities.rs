//! Plugins through AccessKit: a plugin's page in Settings drawn from its
//! nodes, the first-run screen for the plugins that ship with Noura, and
//! the Plugins page, which lists each capability, network host and
//! command target a plugin may use, shipped plugins included, and revokes
//! them one at a time.

mod support;

use std::path::Path;

use egui::{Key, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{By, NodeT, Queryable},
};
use support::{
    FakePicker, create_workspace, harness, harness_registering, plugins,
    plugins::{HELLO_MANIFEST, install_and_enable},
    settle,
};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A workspace named Garden with two notes, opened at launch, where
/// `prepare` may install plugins first. The temporary folders last as long
/// as the test binds them.
fn opened(prepare: impl FnOnce(&Path, &Path)) -> (Ui, (tempfile::TempDir, tempfile::TempDir)) {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    for name in ["One.md", "Two.md"] {
        std::fs::write(folder.join(name), b"x").unwrap();
    }
    prepare(app_data.path(), parent.path());
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    (harness, (app_data, parent))
}

fn dialog<'a>(harness: &'a Ui, label: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::Dialog, label)
}

/// Every label in `node`, top to bottom.
fn labels(node: &egui_kittest::Node<'_>) -> Vec<String> {
    node.query_all(By::new().role(Role::Label))
        .filter_map(|label| label.accesskit_node().value())
        .collect()
}

fn open_settings_section(harness: &mut Ui, section: &str) {
    harness
        .get_by_role_and_label(Role::Button, "Settings")
        .click();
    harness.run();
    dialog(harness, "Settings")
        .get_by_role_and_label(Role::Navigation, "Settings sections")
        .get_by_role_and_label(Role::Button, section)
        .click();
    harness.run();
}

#[test]
fn a_plugin_page_in_settings_draws_its_nodes_and_its_button_reaches_the_plugin() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (mut harness, _folders) = opened(|app_data, parent| {
        let manifest = plugins::probe_manifest_with(&["name = \"settings\""]);
        let folder = plugins::folder(parent, &manifest, "plugin.wasm", &probe);
        install_and_enable(app_data, &folder, "probe");
    });

    open_settings_section(&mut harness, "Probe settings");

    let page = dialog(&harness, "Settings").get_by_role_and_label(Role::Group, "Probe settings");
    assert_eq!(labels(&page), ["Pressed 0 times"]);
    page.get_by_role_and_label(Role::Button, "Count a press")
        .click();
    harness.run();
    settle(&mut harness);

    let page = dialog(&harness, "Settings").get_by_role_and_label(Role::Group, "Probe settings");
    assert_eq!(labels(&page), ["Pressed 1 times"]);
}

/// A folder of the plugins that ship with Noura, holding the hello plugin.
fn shipped_with_hello(hello: &[u8]) -> tempfile::TempDir {
    let shipped = tempfile::tempdir().unwrap();
    plugins::folder(shipped.path(), HELLO_MANIFEST, "hello_plugin.wasm", hello);
    shipped
}

#[test]
fn the_first_run_screen_lists_each_shipped_plugin_and_one_key_press_approves_them() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let shipped = shipped_with_hello(&hello);
    let app_data = tempfile::tempdir().unwrap();
    let mut harness = harness_registering(app_data.path(), FakePicker::default(), |app| {
        app.set_shipped_plugins(shipped.path().to_owned());
    });
    harness.run();

    let screen = dialog(&harness, "Plugins that ship with Noura");
    let shown = labels(&screen);
    for expected in [
        "Hello",
        "Version 0.1.0",
        "Read the files and folders in your workspace.",
        "Add pages to the rail and show panels in them.",
    ] {
        assert!(shown.iter().any(|label| label == expected), "{shown:?}");
    }
    assert!(
        screen
            .get_by_role_and_label(Role::Button, "Allow and continue")
            .is_focused()
    );
    assert!(
        !harness
            .state()
            .app()
            .plugins()
            .iter()
            .any(|row| row.installed)
    );

    harness.key_press(Key::Enter);
    harness.run();

    assert!(
        harness
            .query_by_role_and_label(Role::Dialog, "Plugins that ship with Noura")
            .is_none()
    );
    let rows = harness.state().app().plugins();
    assert!(
        rows.iter()
            .any(|row| row.id == "hello" && row.installed && row.shipped),
        "{rows:?}"
    );
}

#[test]
fn a_plugin_that_kept_failing_shows_as_failed_with_a_button_that_starts_it_again() {
    let Some(misbehave) = plugins::built("misbehave_plugin") else {
        return;
    };
    let (mut harness, _folders) = opened(|app_data, parent| {
        let manifest = "id = \"misbehave\"\nname = \"Misbehave\"\nversion = \"0.1.0\"\n\
                        interface-version = \"0.1.0\"\nentry = \"plugin.wasm\"\n\
                        platforms = [\"desktop\"]\n\n[[capabilities]]\nname = \"views\"\n";
        let folder = plugins::folder(parent, manifest, "plugin.wasm", &misbehave);
        install_and_enable(app_data, &folder, "misbehave");
    });
    for _ in 0..3 {
        harness
            .state_mut()
            .app_mut()
            .press_plugin_button("plugin:misbehave:misbehave", "panic");
        settle(&mut harness);
    }
    open_settings_section(&mut harness, "Plugins");

    let row = dialog(&harness, "Settings").get_by_role_and_label(Role::Group, "Misbehave");
    assert!(
        row.query_by_role_and_label(
            Role::Alert,
            "It failed too often and stopped for this session."
        )
        .is_some()
    );
    row.get_by_role_and_label(Role::Button, "Start again")
        .focus();
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    settle(&mut harness);

    let rows = harness.state().app().plugins();
    let misbehave = rows.iter().find(|row| row.id == "misbehave").unwrap();
    assert!(misbehave.running && !misbehave.failed, "{misbehave:?}");
    assert!(
        dialog(&harness, "Settings")
            .get_by_role_and_label(Role::Group, "Misbehave")
            .query_by_role_and_label(Role::Button, "Start again")
            .is_none()
    );
}

#[test]
fn the_plugins_page_lists_what_a_plugin_may_use_and_revokes_one_at_a_time() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let (mut harness, _folders) = opened(|app_data, parent| {
        let folder = plugins::folder(parent, HELLO_MANIFEST, "hello_plugin.wasm", &hello);
        install_and_enable(app_data, &folder, "hello");
    });
    open_settings_section(&mut harness, "Plugins");
    let read = "Read the files and folders in your workspace.";

    dialog(&harness, "Settings")
        .get_by_role_and_label(Role::Group, "Hello")
        .get_by_role_and_label(Role::Button, &format!("Revoke: {read}"))
        .focus();
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    settle(&mut harness);

    let row = dialog(&harness, "Settings").get_by_role_and_label(Role::Group, "Hello");
    let shown = labels(&row);
    assert!(
        shown.iter().any(|label| label == "You revoked:"),
        "{shown:?}"
    );
    assert!(
        row.query_by_role_and_label(Role::Button, &format!("Allow: {read}"))
            .is_some()
    );
    assert!(
        row.query_by_role_and_label(Role::Button, &format!("Revoke: {read}"))
            .is_none()
    );
    let hello_row = harness
        .state()
        .app()
        .plugins()
        .into_iter()
        .find(|row| row.id == "hello")
        .unwrap();
    assert!(
        hello_row
            .approved
            .iter()
            .all(|line| line.description != read)
    );
}

#[test]
fn the_plugins_page_lists_each_network_host_and_command_target_and_revokes_a_host_alone() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (mut harness, _folders) = opened(|app_data, parent| {
        let manifest = plugins::probe_manifest_with(&[
            "name = \"network\"\nhosts = [\"api.example.com\"]",
            "name = \"commands\"\ntargets = [\"tasks\"]",
        ]);
        let folder = plugins::folder(parent, &manifest, "plugin.wasm", &probe);
        install_and_enable(app_data, &folder, "probe");
    });
    open_settings_section(&mut harness, "Plugins");
    let host = "Connect to api.example.com.";
    let target = "Run the commands the tasks plugin offers.";

    let row = dialog(&harness, "Settings").get_by_role_and_label(Role::Group, "Probe");
    let shown = labels(&row);
    for expected in [
        "Read the files and folders in your workspace.",
        host,
        "Add commands to the command palette and offer them to other plugins.",
        target,
    ] {
        assert!(shown.iter().any(|label| label == expected), "{shown:?}");
    }
    row.get_by_role_and_label(Role::Button, &format!("Revoke: {host}"))
        .focus();
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    settle(&mut harness);

    let row = dialog(&harness, "Settings").get_by_role_and_label(Role::Group, "Probe");
    assert!(
        row.query_by_role_and_label(Role::Button, &format!("Allow: {host}"))
            .is_some()
    );
    assert!(
        row.query_by_role_and_label(Role::Button, &format!("Revoke: {target}"))
            .is_some()
    );
    let probe_row = harness
        .state()
        .app()
        .plugins()
        .into_iter()
        .find(|row| row.id == "probe")
        .unwrap();
    assert!(
        probe_row
            .revoked
            .iter()
            .any(|line| line.description == host)
    );
    assert!(
        probe_row
            .approved
            .iter()
            .any(|line| line.description == target)
    );
}

#[test]
fn the_plugins_page_shows_a_shipped_plugin_and_what_it_may_use() {
    let Some(hello) = plugins::built("hello_plugin") else {
        return;
    };
    let shipped = shipped_with_hello(&hello);
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Garden", &parent.path().join("Garden"));
    let mut harness = harness_registering(app_data.path(), FakePicker::default(), |app| {
        app.set_shipped_plugins(shipped.path().to_owned());
    });
    settle(&mut harness);
    dialog(&harness, "Plugins that ship with Noura")
        .get_by_role_and_label(Role::Button, "Allow and continue")
        .click();
    harness.run();
    settle(&mut harness);

    open_settings_section(&mut harness, "Plugins");

    let row = dialog(&harness, "Settings").get_by_role_and_label(Role::Group, "Hello");
    let shown = labels(&row);
    for expected in [
        "Ships with Noura",
        "You allowed it to:",
        "Read the files and folders in your workspace.",
        "Add pages to the rail and show panels in them.",
    ] {
        assert!(shown.iter().any(|label| label == expected), "{shown:?}");
    }
    assert!(
        row.query_by_role_and_label(
            Role::Button,
            "Revoke: Add pages to the rail and show panels in them."
        )
        .is_some()
    );
}
