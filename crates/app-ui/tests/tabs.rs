//! The tab bar, through AccessKit.

mod support;

use egui::{Event, Key, Modifiers, PointerButton, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness_with, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A workspace with three notes, opened at launch with two test pages.
fn opened(app_data: &std::path::Path, folder: &std::path::Path) -> Ui {
    create_workspace(app_data, "Garden", folder);
    for name in ["Alpha.md", "Beta.md", "Gamma.md"] {
        std::fs::write(folder.join(name), b"x").unwrap();
    }
    let mut harness = harness_with(app_data, FakePicker::default(), |app| {
        app.register_page("test-inbox", "Test inbox");
    });
    settle(&mut harness);
    harness
}

fn row<'a>(harness: &'a Ui, name: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::TreeItem, name)
}

/// Each tab's label, left to right.
fn tabs(harness: &Ui) -> Vec<String> {
    match harness.query_by_role_and_label(Role::TabList, "Tabs") {
        Some(list) => list
            .get_all_by_role(Role::Tab)
            .filter_map(|tab| tab.accesskit_node().label())
            .collect(),
        None => Vec::new(),
    }
}

/// The selected tab's label.
fn selected(harness: &Ui) -> Option<String> {
    harness
        .get_all_by_role(Role::Tab)
        .find(|tab| tab.accesskit_node().is_selected() == Some(true))
        .and_then(|tab| tab.accesskit_node().label())
}

/// Double-click at `pos`. Both clicks land in one frame: the harness
/// steps a quarter second per event, slower than a double-click.
fn double_click_at(harness: &mut Ui, pos: egui::Pos2) {
    let button = |pressed| Event::PointerButton {
        pos,
        button: PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    harness.input_mut().events.extend([
        Event::PointerMoved(pos),
        button(true),
        button(false),
        button(true),
        button(false),
    ]);
    harness.step();
    harness.run();
}

/// Double-click the tree row `name`.
fn double_click(harness: &mut Ui, name: &str) {
    let pos = row(harness, name).rect().center();
    double_click_at(harness, pos);
}

#[test]
fn opening_a_file_opens_a_selected_tab_with_a_close_button() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    assert!(tabs(&harness).is_empty());

    row(&harness, "Alpha").click();
    harness.run();

    assert_eq!(tabs(&harness), ["Alpha (preview)"]);
    assert_eq!(selected(&harness).as_deref(), Some("Alpha (preview)"));
    harness.get_by_role_and_label(Role::Button, "Close Alpha");
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn a_click_replaces_the_preview_and_a_double_click_keeps_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));

    row(&harness, "Alpha").click();
    harness.run();
    row(&harness, "Beta").click();
    harness.run();
    assert_eq!(tabs(&harness), ["Beta (preview)"]);

    double_click(&mut harness, "Beta");
    row(&harness, "Gamma").click();
    harness.run();

    assert_eq!(tabs(&harness), ["Beta", "Gamma (preview)"]);
}

#[test]
fn a_double_click_on_a_tab_keeps_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    row(&harness, "Alpha").click();
    harness.run();

    let tab = harness
        .get_by_role_and_label(Role::Tab, "Alpha (preview)")
        .rect()
        .left_center()
        + egui::vec2(12.0, 0.0);
    double_click_at(&mut harness, tab);

    assert_eq!(tabs(&harness), ["Alpha"]);
}

#[test]
fn the_close_button_and_a_middle_click_close_tabs() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    for name in ["Alpha", "Beta", "Gamma"] {
        double_click(&mut harness, name);
    }
    harness.get_by_role_and_label(Role::Tab, "Beta").click();
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "Close Beta")
        .click();
    harness.run();
    assert_eq!(tabs(&harness), ["Alpha", "Gamma"]);
    assert_eq!(selected(&harness).as_deref(), Some("Gamma"));

    harness
        .get_by_role_and_label(Role::Tab, "Alpha")
        .click_button(PointerButton::Middle);
    harness.run();
    assert_eq!(tabs(&harness), ["Gamma"]);
    assert_eq!(selected(&harness).as_deref(), Some("Gamma"));
}

#[test]
fn closing_the_last_tab_leaves_nothing_open_or_highlighted() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    row(&harness, "Alpha").click();
    harness.run();

    harness
        .get_by_role_and_label(Role::Button, "Close Alpha")
        .click();
    harness.run();

    assert!(tabs(&harness).is_empty());
    assert_eq!(row(&harness, "Alpha").accesskit_node().is_selected(), None);
    let document = harness.get_by_role_and_label(Role::Region, "Document");
    assert!(document.query_by_label("Alpha").is_none());
}

/// The tab with keyboard focus.
fn focused_tab(harness: &Ui) -> Option<String> {
    harness
        .get_all_by_role(Role::Tab)
        .find(|tab| tab.is_focused())
        .and_then(|tab| tab.accesskit_node().label())
}

#[test]
fn the_keyboard_moves_between_tabs_activates_and_closes_them() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    for name in ["Alpha", "Beta", "Gamma"] {
        double_click(&mut harness, name);
    }
    harness.get_by_role_and_label(Role::Tab, "Gamma").focus();
    harness.run();

    harness.key_press(Key::ArrowRight);
    harness.run();
    assert_eq!(focused_tab(&harness).as_deref(), Some("Alpha"));
    harness.key_press(Key::End);
    harness.run();
    assert_eq!(focused_tab(&harness).as_deref(), Some("Gamma"));
    harness.key_press(Key::ArrowLeft);
    harness.run();
    assert_eq!(focused_tab(&harness).as_deref(), Some("Beta"));
    assert_eq!(selected(&harness).as_deref(), Some("Gamma"));

    harness.key_press(Key::Enter);
    harness.run();
    assert_eq!(selected(&harness).as_deref(), Some("Beta"));

    harness.key_press(Key::Delete);
    harness.run();
    assert_eq!(tabs(&harness), ["Alpha", "Gamma"]);
    assert_eq!(focused_tab(&harness).as_deref(), Some("Gamma"));
}
