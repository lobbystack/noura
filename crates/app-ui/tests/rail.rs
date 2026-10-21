//! The rail and Open in new tab, through AccessKit.

mod support;

use egui::{Key, Modifiers, PointerButton, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness_with, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// Until the Inbox (ticket 16) and plugins (ticket 24) add pages, tests
/// register their own.
fn with_pages(app: &mut app_core::App) {
    app.register_page("test-alpha", "Alpha");
    app.register_page("test-beta", "Beta");
}

/// A workspace with two notes, opened at launch with the test pages.
fn opened(app_data: &std::path::Path, folder: &std::path::Path) -> Ui {
    create_workspace(app_data, "Garden", folder);
    for name in ["One.md", "Two.md"] {
        std::fs::write(folder.join(name), b"x").unwrap();
    }
    let mut harness = harness_with(app_data, FakePicker::default(), with_pages);
    settle(&mut harness);
    harness
}

fn page<'a>(harness: &'a Ui, title: &'a str) -> egui_kittest::Node<'a> {
    harness
        .get_by_role_and_label(Role::Navigation, "Pages")
        .get_by_role_and_label(Role::Link, title)
}

/// The rail's page titles, top to bottom.
fn rail(harness: &Ui) -> Vec<String> {
    harness
        .get_by_role_and_label(Role::Navigation, "Pages")
        .get_all_by_role(Role::Link)
        .filter_map(|link| link.accesskit_node().label())
        .collect()
}

fn current(harness: &Ui) -> Option<String> {
    harness
        .get_all_by_role(Role::Link)
        .find(|link| link.accesskit_node().data().aria_current().is_some())
        .and_then(|link| link.accesskit_node().label())
}

/// Each tab's label, left to right.
fn tabs(harness: &Ui) -> Vec<String> {
    harness
        .query_by_role_and_label(Role::TabList, "Tabs")
        .map(|list| {
            list.get_all_by_role(Role::Tab)
                .filter_map(|tab| tab.accesskit_node().label())
                .collect()
        })
        .unwrap_or_default()
}

fn selected(harness: &Ui) -> Option<String> {
    harness
        .get_all_by_role(Role::Tab)
        .find(|tab| tab.accesskit_node().is_selected() == Some(true))
        .and_then(|tab| tab.accesskit_node().label())
}

/// Open the right-click menu of `node` and choose Open in new tab.
fn open_in_new_tab(harness: &mut Ui, node: impl Fn(&Ui) -> egui_kittest::Node<'_>) {
    node(harness).click_secondary();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Open in new tab")
        .click();
    harness.run();
}

#[test]
fn the_rail_labels_its_pages_and_marks_the_current_one() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));

    assert_eq!(rail(&harness), ["Files", "Alpha", "Beta"]);
    assert_eq!(current(&harness).as_deref(), Some("Files"));

    page(&harness, "Alpha").click();
    harness.run();

    assert_eq!(current(&harness).as_deref(), Some("Alpha"));
    assert_eq!(tabs(&harness), ["Alpha (preview)"]);
    assert!(harness.unnamed_widgets().is_empty());
}

#[test]
fn open_in_new_tab_from_a_tree_file_keeps_the_current_tab() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    harness.get_by_role_and_label(Role::TreeItem, "One").click();
    harness.run();

    open_in_new_tab(&mut harness, |harness| {
        harness.get_by_role_and_label(Role::TreeItem, "Two")
    });

    assert_eq!(tabs(&harness), ["One (preview)", "Two"]);
    assert_eq!(selected(&harness).as_deref(), Some("Two"));
}

#[test]
fn open_in_new_tab_cmd_click_and_middle_click_on_a_page_keep_the_current_tab() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    harness.get_by_role_and_label(Role::TreeItem, "One").click();
    harness.run();

    open_in_new_tab(&mut harness, |harness| page(harness, "Alpha"));
    assert_eq!(tabs(&harness), ["One (preview)", "Alpha"]);

    page(&harness, "Beta").click_modifiers(Modifiers::COMMAND);
    harness.run();
    assert_eq!(tabs(&harness), ["One (preview)", "Alpha", "Beta"]);

    harness
        .get_by_role_and_label(Role::Button, "Close Beta")
        .click();
    harness.run();
    page(&harness, "Beta").click_button(PointerButton::Middle);
    harness.run();
    assert_eq!(tabs(&harness), ["One (preview)", "Alpha", "Beta"]);
    assert_eq!(selected(&harness).as_deref(), Some("Beta"));
}

#[test]
fn files_shows_the_document_tab_a_page_took_over_from() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    harness.get_by_role_and_label(Role::TreeItem, "One").click();
    harness.run();
    page(&harness, "Alpha").click_modifiers(Modifiers::COMMAND);
    harness.run();
    assert_eq!(selected(&harness).as_deref(), Some("Alpha"));

    page(&harness, "Files").click();
    harness.run();

    assert_eq!(selected(&harness).as_deref(), Some("One (preview)"));
    assert_eq!(current(&harness).as_deref(), Some("Files"));
}

fn focused_page(harness: &Ui) -> Option<String> {
    harness
        .get_all_by_role(Role::Link)
        .find(|link| link.is_focused())
        .and_then(|link| link.accesskit_node().label())
}

#[test]
fn alt_up_and_alt_down_reorder_the_rail_and_keep_focus_on_the_page() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    page(&harness, "Beta").focus();
    harness.run();

    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
    harness.run();
    assert_eq!(rail(&harness), ["Files", "Beta", "Alpha"]);
    assert_eq!(focused_page(&harness).as_deref(), Some("Beta"));

    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
    harness.run();
    assert_eq!(rail(&harness), ["Beta", "Files", "Alpha"]);
    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    harness.run();
    assert_eq!(rail(&harness), ["Files", "Beta", "Alpha"]);
    assert_eq!(focused_page(&harness).as_deref(), Some("Beta"));
}

#[test]
fn the_keyboard_reaches_and_opens_rail_pages() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    page(&harness, "Files").focus();
    harness.run();

    harness.key_press(Key::Tab);
    harness.run();
    assert_eq!(focused_page(&harness).as_deref(), Some("Alpha"));
    harness.key_press(Key::Enter);
    harness.run();

    assert_eq!(current(&harness).as_deref(), Some("Alpha"));
}

/// Drag `title`'s page to `to` in steps, as a mouse does.
fn drag_page(harness: &mut Ui, title: &str, to: egui::Pos2, before_drop: impl FnOnce(&mut Ui)) {
    let from = page(harness, title).rect().center();
    harness.hover_at(from);
    harness.run();
    harness.drag_at(from);
    harness.run();
    for step in 1..=4 {
        harness.hover_at(from + (to - from) * (step as f32 / 4.0));
        harness.run();
    }
    before_drop(harness);
    harness.drop_at(to);
    harness.run();
}

#[test]
fn dragging_a_page_moves_it_and_escape_cancels_the_drag() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));

    let above_files = page(&harness, "Files").rect().center_top() + egui::vec2(0.0, 2.0);
    drag_page(&mut harness, "Beta", above_files, |_| {});
    assert_eq!(rail(&harness), ["Beta", "Files", "Alpha"]);

    // Without Escape, this drop would put Beta after Alpha.
    let below_alpha = page(&harness, "Alpha").rect().center_bottom() - egui::vec2(0.0, 4.0);
    drag_page(&mut harness, "Beta", below_alpha, |harness| {
        harness.key_press(Key::Escape);
        harness.run();
    });
    assert_eq!(rail(&harness), ["Beta", "Files", "Alpha"]);
    assert!(tabs(&harness).is_empty(), "a drag must not open the page");
}
