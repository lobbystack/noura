//! The rail and Open in new tab, through AccessKit.

use crate::support;
use egui::{Key, Modifiers, PointerButton, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness_for, harness_registering, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// Until plugins (ticket 24) add pages, tests register their own.
fn with_pages(app: &mut app_core::App) {
    app.register_page("test-alpha", "Alpha");
    app.register_page("test-beta", "Beta");
}

/// A workspace with two notes, opened at launch with the test pages.
fn opened(app_data: &std::path::Path, folder: &std::path::Path) -> Ui {
    create_workspace(app_data, folder);
    for name in ["One.md", "Two.md"] {
        std::fs::write(folder.join(name), b"x").unwrap();
    }
    let mut harness = harness_registering(app_data, FakePicker::default(), with_pages);
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

    assert_eq!(rail(&harness), ["Inbox", "Files", "Alpha", "Beta"]);
    assert_eq!(current(&harness).as_deref(), Some("Files"));

    page(&harness, "Alpha").click();
    harness.run();

    assert_eq!(current(&harness).as_deref(), Some("Alpha"));
    assert_eq!(tabs(&harness), ["Alpha (preview)"]);
    assert!(harness.unnamed_widgets().is_empty());
}

/// A workspace in `folder`, opened at launch on `platform`.
fn opened_on(
    app_data: &std::path::Path,
    folder: &std::path::Path,
    platform: app_core::Platform,
) -> Ui {
    create_workspace(app_data, folder);
    let mut harness = harness_for(app_data, platform, |_| {});
    settle(&mut harness);
    harness
}

/// kittest draws the window 8 points in from its edges.
const HARNESS_MARGIN: f32 = 8.0;

/// Where the rail page `title` draws, from the window's top-left corner.
fn page_rect(harness: &Ui, title: &str) -> egui::Rect {
    page(harness, title)
        .rect()
        .translate(-egui::Vec2::splat(HARNESS_MARGIN))
}

/// AppKit's close, minimize and zoom buttons are 14 points wide.
const WINDOW_BUTTON: f32 = 14.0;

#[test]
fn on_macos_the_window_buttons_fit_in_the_rail_s_top_row() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let harness = opened_on(
        app_data.path(),
        &parent.path().join("Garden"),
        app_core::Platform::MacOs,
    );

    let inbox = page_rect(&harness, "Inbox");
    let buttons = app_ui::window_buttons(1.0);
    // Centered over the rail's pages, all three fit with room to spare,
    // and the pages start below them. A page snaps to half a point.
    assert!((buttons.center.x - inbox.center().x).abs() <= 0.5);
    let row = 3.0 * WINDOW_BUTTON + 2.0 * buttons.gap;
    assert!(buttons.center.x - row / 2.0 >= 6.0);
    assert!(inbox.top() > buttons.center.y + WINDOW_BUTTON / 2.0);
    // The workspace switcher lines up with the sidebar's rows.
    let switcher = harness
        .get_by_role_and_label(Role::Button, "Workspace: Garden")
        .rect();
    let files = harness.get_by_role_and_label(Role::Button, "Files").rect();
    assert_eq!(switcher.left(), files.left());
}

#[test]
fn zoomed_out_the_rail_stays_wide_enough_for_the_window_buttons() {
    for zoom in [0.5, 0.75, 1.0, 1.5, 2.0] {
        let buttons = app_ui::window_buttons(zoom);
        let row = 3.0 * WINDOW_BUTTON + 2.0 * buttons.gap;
        assert!(buttons.center.x - row / 2.0 >= 6.0, "{zoom}");
        // The row is the title bar row's height at that zoom.
        assert_eq!(buttons.center.y, 18.0 * zoom, "{zoom}");
    }
}

#[test]
fn windows_and_linux_keep_the_narrow_rail() {
    for platform in [app_core::Platform::Windows, app_core::Platform::Linux] {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let harness = opened_on(app_data.path(), &parent.path().join("Garden"), platform);

        let inbox = page_rect(&harness, "Inbox");
        assert!((inbox.center().x - 28.0).abs() <= 0.5);
    }
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

/// The Inbox takes over the note's tab, and Files shows that note's text
/// again.
#[test]
fn files_shows_the_note_the_inbox_took_over_from_with_its_text() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    std::fs::write(folder.join("One.md"), "shown text\n").unwrap();
    harness.state_mut().app_mut().rescan();
    settle(&mut harness);
    harness.get_by_role_and_label(Role::TreeItem, "One").click();
    harness.run();
    page(&harness, "Inbox").click_modifiers(Modifiers::COMMAND);
    harness.run();
    assert_eq!(selected(&harness).as_deref(), Some("Inbox"));
    assert!(
        harness
            .query_by_role_and_label(Role::TextInput, "File name")
            .is_none()
    );

    page(&harness, "Files").click();
    harness.run();

    assert_eq!(selected(&harness).as_deref(), Some("One (preview)"));
    assert_eq!(
        harness
            .get_by_role_and_label(Role::TextInput, "File name")
            .accesskit_node()
            .value()
            .as_deref(),
        Some("One")
    );
    assert_eq!(
        harness
            .get_by_role_and_label(Role::MultilineTextInput, "Note text")
            .value()
            .as_deref(),
        Some("shown text\n")
    );
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
    assert_eq!(rail(&harness), ["Inbox", "Files", "Beta", "Alpha"]);
    assert_eq!(focused_page(&harness).as_deref(), Some("Beta"));

    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowUp);
    harness.run();
    assert_eq!(rail(&harness), ["Inbox", "Beta", "Files", "Alpha"]);
    harness.key_press_modifiers(Modifiers::ALT, Key::ArrowDown);
    harness.run();
    assert_eq!(rail(&harness), ["Inbox", "Files", "Beta", "Alpha"]);
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
    assert_eq!(rail(&harness), ["Inbox", "Beta", "Files", "Alpha"]);

    // Without Escape, this drop would put Beta after Alpha.
    let below_alpha = page(&harness, "Alpha").rect().center_bottom() - egui::vec2(0.0, 4.0);
    drag_page(&mut harness, "Beta", below_alpha, |harness| {
        harness.key_press(Key::Escape);
        harness.run();
    });
    assert_eq!(rail(&harness), ["Inbox", "Beta", "Files", "Alpha"]);
    assert!(tabs(&harness).is_empty(), "a drag must not open the page");
}
