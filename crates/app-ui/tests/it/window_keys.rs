//! The window's keys through AccessKit, as a Mac user presses them: Cmd+W
//! over Settings and search, going to a tab by its number, reopening
//! closed tabs and closing an empty window.

use crate::support;
use std::path::Path;

use app_core::{OpenAs, Platform};
use app_ui::NouraUi;
use egui::{Key, Modifiers, accesskit::Role};
use egui_kittest::{Harness, kittest::Queryable};
use support::{create_workspace, harness_for, settle};

type Ui = Harness<'static, NouraUi>;

const CMD: Modifiers = Modifiers {
    mac_cmd: true,
    command: true,
    ..Modifiers::NONE
};

/// A Mac window on a workspace holding `names`, each opened in a kept tab
/// in that order.
fn mac_window(app_data: &Path, folder: &Path, names: &[&str]) -> Ui {
    create_workspace(app_data, folder);
    for name in names {
        std::fs::write(folder.join(name), format!("# {name}\n")).unwrap();
    }
    let mut harness = harness_for(app_data, Platform::MacOs, |_| {});
    settle(&mut harness);
    for name in names {
        harness.state_mut().app_mut().open_file(name, OpenAs::Kept);
    }
    settle(&mut harness);
    harness
}

fn press(harness: &mut Ui, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    settle(harness);
}

fn titles(harness: &Ui) -> Vec<String> {
    harness
        .state()
        .app()
        .tabs()
        .iter()
        .map(|tab| tab.title().to_owned())
        .collect()
}

fn active(harness: &Ui) -> Option<String> {
    harness
        .state()
        .app()
        .active_tab()
        .map(|tab| tab.title().to_owned())
}

fn dialog(harness: &Ui, label: &str) -> bool {
    harness
        .query_by_role_and_label(Role::Dialog, label)
        .is_some()
}

#[test]
fn cmd_w_closes_settings_or_search_and_leaves_the_tab_under_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = mac_window(
        app_data.path(),
        &parent.path().join("Keys"),
        &["Alpha.md", "Beta.md"],
    );

    press(&mut harness, CMD, Key::Comma);
    assert!(dialog(&harness, "Settings"));
    press(&mut harness, CMD, Key::W);
    assert!(!dialog(&harness, "Settings"), "Cmd+W closes Settings");
    assert_eq!(titles(&harness), ["Alpha", "Beta"]);

    // In the note, Cmd+K adds a link; Cmd+O opens search.
    press(&mut harness, CMD, Key::O);
    assert!(dialog(&harness, "Search"));
    press(&mut harness, CMD, Key::W);
    assert!(!dialog(&harness, "Search"), "Cmd+W closes search");
    assert_eq!(titles(&harness), ["Alpha", "Beta"]);
    assert_eq!(active(&harness).as_deref(), Some("Beta"));

    // Keys for the tabs wait while Settings covers them.
    press(&mut harness, CMD, Key::Comma);
    press(&mut harness, CMD.plus(Modifiers::SHIFT), Key::OpenBracket);
    press(&mut harness, CMD, Key::N);
    assert_eq!(titles(&harness), ["Alpha", "Beta"]);
    assert_eq!(active(&harness).as_deref(), Some("Beta"));
    assert!(dialog(&harness, "Settings"));
}

#[test]
fn cmd_1_to_8_show_that_tab_cmd_9_the_last_and_ctrl_cmd_a_digit_a_page() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = mac_window(
        app_data.path(),
        &parent.path().join("Keys"),
        &["Alpha.md", "Beta.md", "Gamma.md"],
    );
    assert_eq!(active(&harness).as_deref(), Some("Gamma"));

    press(&mut harness, CMD, Key::Num1);
    assert_eq!(active(&harness).as_deref(), Some("Alpha"));
    press(&mut harness, CMD, Key::Num2);
    assert_eq!(active(&harness).as_deref(), Some("Beta"));
    press(&mut harness, CMD, Key::Num9);
    assert_eq!(active(&harness).as_deref(), Some("Gamma"));
    assert_eq!(harness.state().app().current_page(), app_core::FILES_PAGE);
    // ⌥⌘← and ⌥⌘→ go to the tab before and after, as in Chrome.
    let option = CMD.plus(Modifiers::ALT);
    press(&mut harness, option, Key::ArrowLeft);
    assert_eq!(active(&harness).as_deref(), Some("Beta"));
    press(&mut harness, option, Key::ArrowRight);
    press(&mut harness, option, Key::ArrowRight);
    assert_eq!(active(&harness).as_deref(), Some("Alpha"));
    // ⇧⌘[ still works.
    press(&mut harness, CMD.plus(Modifiers::SHIFT), Key::OpenBracket);
    assert_eq!(active(&harness).as_deref(), Some("Gamma"));

    // ⌃⌘1 shows the Inbox, in a tab of its own.
    press(&mut harness, CMD.plus(Modifiers::CTRL), Key::Num1);
    assert_eq!(harness.state().app().current_page(), app_core::INBOX_PAGE);
    assert_eq!(titles(&harness).len(), 4);
}
