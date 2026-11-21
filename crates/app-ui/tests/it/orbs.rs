//! The Orbs page through AccessKit: the sidebar lists the Orbs with how
//! each stands and names a new one, the message box names the open Orb,
//! the pill with its name opens and closes its side panel, and the
//! panel's Model and Effort menus write the Orb's file, or for the default
//! Orb this device's app data.

use crate::support;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use app_core::{AiEffort, Platform};
use egui::{Key, accesskit::Role};
use egui_kittest::kittest::{By, NodeT, Queryable};
use support::{
    FakePicker, FakeSystem, create_workspace, harness_with,
    kit::{Ui, open_page, press, run},
    plugins, settle,
};

const MANIFEST: &str = include_str!("../../../plugins/ai/plugin.toml");

struct Opened {
    harness: Ui,
    root: PathBuf,
    /// The Anthropic provider's ID.
    claude: String,
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace with the AI plugin on, the providers Local and Claude set
/// up, opened on the AI page.
fn opened() -> Option<Opened> {
    let component = plugins::built("ai_plugin")?;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = parent.path().join("Garden");
    create_workspace(app_data.path(), &root);
    let plugin = plugins::folder(parent.path(), MANIFEST, "ai_plugin.wasm", &component);
    plugins::install_and_enable(app_data.path(), &plugin, "ai");
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system);
    settle(&mut harness);
    let app = harness.state_mut().app_mut();
    let provider =
        |name: &str, kind: &str, model: &str, key: Option<&str>| app_core::AiProviderInput {
            id: None,
            name: name.into(),
            kind: kind.into(),
            model: model.into(),
            endpoint: None,
            enabled: true,
            key: key.map(Into::into),
            context_window: None,
        };
    app.save_ai_provider(provider("Local", "ollama", "llama3", None))
        .unwrap();
    let claude = app
        .save_ai_provider(provider(
            "Claude",
            "anthropic",
            "claude-sonnet-5-5",
            Some("sk-ant-test"),
        ))
        .unwrap();
    settle(&mut harness);
    open_page(&mut harness, "AI");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Form, "Message Noura")
            .is_some()
    });
    Some(Opened {
        harness,
        root,
        claude,
        _folders: (app_data, parent),
    })
}

/// Run frames until `done` holds, for up to 20 seconds.
fn wait_until(harness: &mut Ui, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(harness) {
        assert!(
            Instant::now() < deadline,
            "timed out; the window shows {:?}",
            harness
                .query_all(By::new().role(Role::Label))
                .filter_map(|node| node.accesskit_node().value())
                .collect::<Vec<_>>()
        );
        std::thread::sleep(Duration::from_millis(10));
        harness.step();
    }
    run(harness);
}

/// The labels of the Orbs the sidebar lists, each with how it stands.
fn orbs(harness: &Ui) -> Vec<String> {
    harness
        .query_by_role_and_label(Role::ListBox, "Orbs")
        .map(|list| {
            list.query_all(By::new().role(Role::ListBoxOption))
                .filter_map(|node| node.accesskit_node().label())
                .collect()
        })
        .unwrap_or_default()
}

/// The Orb's file `name`, as its folder holds it.
fn orb_file(root: &std::path::Path, name: &str) -> String {
    std::fs::read_to_string(root.join(format!("Orbs/{name}/{name}.md"))).unwrap_or_default()
}

/// Whether the pill `name` says its panel is open.
fn expanded(harness: &Ui, name: &str) -> Option<bool> {
    harness
        .get_by_role_and_label(Role::Button, name)
        .accesskit_node()
        .data()
        .is_expanded()
}

/// Pick `option` from the panel's menu `menu`.
fn pick(harness: &mut Ui, menu: &str, option: &str) {
    harness
        .get_by_role_and_label(Role::Complementary, "Meetings")
        .get_by_role_and_label(Role::ComboBox, menu)
        .click();
    run(harness);
    harness
        .get_by_role_and_label(Role::MenuItem, option)
        .click();
    run(harness);
}

#[test]
fn new_orb_names_an_orb_and_the_sidebar_lists_each_with_how_it_stands() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;
    assert_eq!(orbs(harness), ["Noura"]);

    harness
        .get_by_role_and_label(Role::Button, "New Orb")
        .click();
    run(harness);
    let field = harness.get_by_role_and_label(Role::TextInput, "Orb name");
    assert!(field.is_focused());
    field.type_text("Meetings");
    run(harness);
    press(harness, Key::Enter);
    wait_until(harness, |harness| orbs(harness) == ["Noura", "Meetings"]);

    // Its file is in its folder, and its thread opens with its panel,
    // where its model waits.
    assert!(orb_file(&opened.root, "Meetings").contains("type: orb"));
    let harness = &mut opened.harness;
    harness.get_by_role_and_label(Role::Form, "Message Meetings");
    assert_eq!(expanded(harness, "Meetings"), Some(true));
    harness.get_by_role_and_label(Role::Complementary, "Meetings");
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());

    // A paused Orb says so after its name.
    let app = harness.state_mut().app_mut();
    let meetings = app
        .orbs()
        .unwrap()
        .into_iter()
        .find(|orb| orb.title == "Meetings")
        .unwrap();
    app.set_orb_paused(&meetings.id, true, &meetings.revision)
        .unwrap();
    wait_until(harness, |harness| {
        orbs(harness) == ["Noura", "Meetings, Paused"]
    });

    // Selecting the default Orb opens its thread.
    harness
        .get_by_role_and_label(Role::ListBox, "Orbs")
        .get_by_role_and_label(Role::ListBoxOption, "Noura")
        .click();
    run(harness);
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::Form, "Message Noura")
            .is_some()
    });
}

#[test]
fn the_pill_with_the_orbs_name_opens_and_closes_its_side_panel() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;
    assert_eq!(expanded(harness, "Noura"), Some(false));
    assert!(harness.query_by_role(Role::Complementary).is_none());

    harness.get_by_role_and_label(Role::Button, "Noura").click();
    run(harness);
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::Complementary, "Noura")
            .is_some()
    });
    assert_eq!(expanded(harness, "Noura"), Some(true));
    // The default Orb asks your first provider's own model, which takes
    // no effort.
    let panel = harness.get_by_role_and_label(Role::Complementary, "Noura");
    let model = panel.get_by_role_and_label(Role::ComboBox, "Model");
    assert_eq!(model.value().as_deref(), Some("llama3"));
    assert!(
        panel
            .query_by_role_and_label(Role::ComboBox, "Effort")
            .is_none()
    );
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());

    harness.get_by_role_and_label(Role::Button, "Noura").click();
    run(harness);
    wait_until(harness, |harness| {
        harness.query_by_role(Role::Complementary).is_none()
    });
    assert_eq!(expanded(harness, "Noura"), Some(false));
}

#[test]
fn the_panels_model_and_effort_menus_write_the_orbs_file() {
    let Some(mut opened) = opened() else {
        return;
    };
    let claude = opened.claude.clone();
    let harness = &mut opened.harness;
    harness
        .state_mut()
        .app_mut()
        .create_orb("Meetings")
        .unwrap();
    wait_until(harness, |harness| orbs(harness).len() == 2);
    harness
        .get_by_role_and_label(Role::ListBox, "Orbs")
        .get_by_role_and_label(Role::ListBoxOption, "Meetings")
        .click();
    run(harness);
    harness
        .get_by_role_and_label(Role::Button, "Meetings")
        .click();
    run(harness);
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::Complementary, "Meetings")
            .is_some()
    });

    pick(harness, "Model", "Claude Opus 5.5");
    wait_until(harness, |_| {
        orb_file(&opened.root, "Meetings").contains("model: claude-opus-5-5")
    });
    let harness = &mut opened.harness;
    let file = orb_file(&opened.root, "Meetings");
    assert!(file.contains(&format!("provider: {claude}")), "{file}");
    // Opus 5.5 reasons at Medium unless you choose.
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::ComboBox, "Effort")
            .is_some_and(|effort| effort.value().as_deref() == Some("Medium"))
    });

    pick(harness, "Effort", "High");
    wait_until(harness, |_| {
        orb_file(&opened.root, "Meetings").contains("effort: high")
    });
    let harness = &mut opened.harness;
    let meetings = harness
        .state()
        .app()
        .orbs()
        .unwrap()
        .into_iter()
        .find(|orb| orb.title == "Meetings")
        .unwrap();
    assert_eq!(meetings.model.as_deref(), Some("claude-opus-5-5"));
    assert_eq!(meetings.effort, Some(AiEffort::High));

    // A model that takes no effort drops it, and its menu goes.
    pick(harness, "Model", "llama3");
    wait_until(harness, |_| {
        orb_file(&opened.root, "Meetings").contains("effort: null")
    });
    let harness = &mut opened.harness;
    wait_until(harness, |harness| {
        harness
            .query_by_role_and_label(Role::ComboBox, "Effort")
            .is_none()
    });
}
