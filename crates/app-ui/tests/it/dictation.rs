//! The Dictation plugin's page in Settings through AccessKit: the
//! shortcut's behavior and the paste method as radio buttons, and custom
//! words you add from a form and remove from each word's menu. Every
//! change lands in the workspace file `Settings/Dictation.md`.

use crate::support;
use std::path::PathBuf;

use egui::{Key, Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{By, NodeT, Queryable},
};
use support::{
    FakePicker, create_workspace, harness,
    kit::{press, press_with, run},
    plugins, settle,
};

type Ui = Harness<'static, app_ui::NouraUi>;

const MANIFEST: &str = include_str!("../../../plugins/dictation/plugin.toml");

struct Opened {
    harness: Ui,
    workspace: PathBuf,
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace with Dictation installed and on, with Settings open at its
/// page. `None` when the plugin isn't built.
fn opened() -> Option<Opened> {
    let component = plugins::built("dictation_plugin")?;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let workspace = parent.path().join("Garden");
    create_workspace(app_data.path(), &workspace);
    let plugin_parent = parent.path().join("plugin");
    std::fs::create_dir_all(&plugin_parent).unwrap();
    let folder = plugins::folder(
        &plugin_parent,
        MANIFEST,
        "dictation_plugin.wasm",
        &component,
    );
    plugins::install_and_enable(app_data.path(), &folder, "dictation");
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    open_page(&mut harness);
    Some(Opened {
        harness,
        workspace,
        _folders: (app_data, parent),
    })
}

/// Open Settings at Dictation's page.
fn open_page(harness: &mut Ui) {
    harness
        .get_by_role_and_label(Role::Button, "Settings")
        .click();
    harness.run();
    settings(harness)
        .get_by_role_and_label(Role::Navigation, "Settings sections")
        .get_by_role_and_label(Role::Button, "Dictation")
        .click();
    run(harness);
}

fn settings(harness: &Ui) -> egui_kittest::Node<'_> {
    harness.get_by_role_and_label(Role::Dialog, "Settings")
}

fn page(harness: &Ui) -> egui_kittest::Node<'_> {
    settings(harness).get_by_role_and_label(Role::Group, "Dictation")
}

fn file(opened: &Opened) -> String {
    std::fs::read_to_string(opened.workspace.join("Settings/Dictation.md")).unwrap_or_default()
}

fn add_word(harness: &mut Ui, word: &str) {
    page(harness)
        .get_by_role_and_label(Role::Form, "New custom word")
        .get_by_role_and_label(Role::TextInput, "Word")
        .focus();
    run(harness);
    harness
        .query_all(By::new())
        .find(|node| node.is_focused())
        .expect("the word field has focus")
        .type_text(word);
    run(harness);
    page(harness)
        .get_by_role_and_label(Role::Form, "New custom word")
        .get_by_role_and_label(Role::Button, "Add")
        .click();
    run(harness);
}

#[test]
fn the_page_holds_the_shortcut_behavior_the_paste_method_and_custom_words() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;

    let shortcut = page(harness).get_by_role_and_label(Role::RadioGroup, "Shortcut");
    let modes: Vec<String> = shortcut
        .query_all(By::new().role(Role::RadioButton))
        .filter_map(|button| button.accesskit_node().label())
        .collect();
    assert_eq!(modes, ["Hold or tap", "Hold", "Toggle"]);
    let methods: Vec<String> = page(harness)
        .get_by_role_and_label(Role::RadioGroup, "Paste with")
        .query_all(By::new().role(Role::RadioButton))
        .filter_map(|button| button.accesskit_node().label())
        .collect();
    assert_eq!(methods, ["Clipboard", "Typing"]);
    assert!(
        page(harness)
            .query_by_role_and_label(Role::ListBox, "Custom words")
            .is_none(),
        "no list before the first word"
    );
    assert!(
        !opened.workspace.join("Settings/Dictation.md").exists(),
        "the page writes nothing until you change something"
    );

    page(harness)
        .get_by_role_and_label(Role::RadioButton, "Toggle")
        .click();
    run(harness);
    page(harness)
        .get_by_role_and_label(Role::RadioButton, "Typing")
        .click();
    run(harness);
    add_word(harness, "Noura");
    add_word(harness, "ChargeBee");

    let text = file(&opened);
    assert!(text.contains("type: dictation-settings"), "{text}");
    assert!(text.contains("activation: toggle"), "{text}");
    assert!(text.contains("paste: typing"), "{text}");
    assert!(text.contains("- Noura"), "{text}");
    assert!(text.contains("- ChargeBee"), "{text}");

    let harness = &mut opened.harness;
    let words: Vec<String> = page(harness)
        .get_by_role_and_label(Role::ListBox, "Custom words")
        .query_all(By::new().role(Role::ListBoxOption))
        .filter_map(|item| item.accesskit_node().label())
        .collect();
    assert_eq!(words, ["Noura", "ChargeBee"]);
    assert_eq!(
        page(harness)
            .get_by_role_and_label(Role::Form, "New custom word")
            .get_by_role_and_label(Role::TextInput, "Word")
            .value()
            .as_deref(),
        Some(""),
        "the field clears once the word is in"
    );
}

#[test]
fn a_word_already_in_the_list_says_so_and_a_menu_removes_one() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;
    add_word(harness, "Noura");
    add_word(harness, "Noura");
    let shown: Vec<String> = page(harness)
        .query_all(By::new().role(Role::Label))
        .filter_map(|label| label.accesskit_node().value())
        .collect();
    assert!(
        shown
            .iter()
            .any(|label| label == "\u{201c}Noura\u{201d} is already in the list."),
        "{shown:?}"
    );

    // From the keyboard: select the word, open its menu, choose Remove.
    page(harness)
        .get_by_role_and_label(Role::ListBox, "Custom words")
        .get_by_role_and_label(Role::ListBoxOption, "Noura")
        .click();
    run(harness);
    press_with(harness, Modifiers::SHIFT, Key::F10);
    assert!(
        harness
            .get_by_label("Actions for Noura")
            .get_by_role_and_label(Role::Button, "Remove")
            .is_focused()
    );
    press(harness, Key::Enter);
    settle(harness);
    let text = file(&opened);
    assert!(text.contains("words: []"), "{text}");
    let harness = &mut opened.harness;
    // Choosing a menu item keeps Settings open.
    harness.get_by_role_and_label(Role::Dialog, "Settings");
    assert!(
        page(harness)
            .query_by_role_and_label(Role::ListBox, "Custom words")
            .is_none()
    );
}

#[test]
fn change_keys_opens_speech_where_the_dictation_shortcut_records_its_keys() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;

    page(harness)
        .get_by_role_and_label(Role::Button, "Change keys")
        .click_accesskit();
    run(harness);

    let section = harness.state().app().settings().unwrap().section.clone();
    assert_eq!(section, app_core::settings_section::SPEECH);
    settings(harness)
        .get_by_role_and_label(Role::Group, "Shortcuts")
        .get_by_role_and_label(Role::Button, "Dictation");
}
