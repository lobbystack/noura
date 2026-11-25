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

#[test]
fn cmd_shift_t_reopens_closed_tabs_one_after_another_in_their_places() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = mac_window(
        app_data.path(),
        &parent.path().join("Keys"),
        &["Alpha.md", "Beta.md", "Gamma.md"],
    );
    press(&mut harness, CMD, Key::Num2);
    press(&mut harness, CMD, Key::W);
    press(&mut harness, CMD, Key::W);
    assert_eq!(titles(&harness), ["Alpha"]);

    let shift = CMD.plus(Modifiers::SHIFT);
    press(&mut harness, shift, Key::T);
    assert_eq!(titles(&harness), ["Alpha", "Gamma"]);
    assert_eq!(active(&harness).as_deref(), Some("Gamma"));
    press(&mut harness, shift, Key::T);
    assert_eq!(titles(&harness), ["Alpha", "Beta", "Gamma"]);
    assert_eq!(active(&harness).as_deref(), Some("Beta"));
    assert!(harness.query_by_role_and_label(Role::Tab, "Beta").is_some());
}

#[test]
fn cmd_w_with_no_tab_left_closes_the_window() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = mac_window(app_data.path(), &parent.path().join("Keys"), &["Alpha.md"]);

    press(&mut harness, CMD, Key::W);
    assert!(titles(&harness).is_empty());
    assert!(!harness.state().is_closed());
    press(&mut harness, CMD, Key::W);
    assert!(harness.state().is_closed(), "Cmd+W closes the empty window");
}

/// Whether the tab `name` shows whole in the part of the row that shows.
fn in_sight(harness: &Ui, name: &str) -> bool {
    let strip = harness.get_by_role_and_label(Role::TabList, "Tabs").rect();
    let tab = harness.get_by_role_and_label(Role::Tab, name).rect();
    tab.left() >= strip.left() - 0.5 && tab.right() <= strip.right() + 0.5
}

#[test]
fn a_tab_the_keys_go_to_comes_into_sight_when_the_row_overflows() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let names: Vec<String> = (1..=14).map(|n| format!("Note {n:02}.md")).collect();
    let files: Vec<&str> = names.iter().map(String::as_str).collect();
    let mut harness = mac_window(app_data.path(), &parent.path().join("Keys"), &files);
    harness.set_size(egui::vec2(1000.0, 750.0));
    settle(&mut harness);
    assert!(in_sight(&harness, "Note 14"));
    assert!(!in_sight(&harness, "Note 01"), "the row doesn't overflow");

    press(&mut harness, CMD, Key::Num1);
    assert_eq!(active(&harness).as_deref(), Some("Note 01"));
    assert!(in_sight(&harness, "Note 01"));
    assert!(!in_sight(&harness, "Note 14"));
    // ⌥⌘← from the first tab goes round to the last one.
    press(&mut harness, CMD.plus(Modifiers::ALT), Key::ArrowLeft);
    assert_eq!(active(&harness).as_deref(), Some("Note 14"));
    assert!(in_sight(&harness, "Note 14"));
    press(&mut harness, CMD.plus(Modifiers::ALT), Key::ArrowRight);
    assert!(in_sight(&harness, "Note 01"));
    press(&mut harness, CMD, Key::Num9);
    assert!(in_sight(&harness, "Note 14"));
    // Cmd+Shift+T puts a closed tab back in sight too.
    press(&mut harness, CMD, Key::Num1);
    press(&mut harness, CMD, Key::W);
    press(&mut harness, CMD, Key::Num9);
    assert!(!in_sight(&harness, "Note 02"));
    press(&mut harness, CMD.plus(Modifiers::SHIFT), Key::T);
    assert_eq!(active(&harness).as_deref(), Some("Note 01"));
    assert!(in_sight(&harness, "Note 01"));
}
