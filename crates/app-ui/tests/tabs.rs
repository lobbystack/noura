//! The tab bar, through AccessKit.

mod support;

use egui::{Event, Key, Modifiers, PointerButton, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness_registering, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A workspace with three notes, opened at launch with two test pages.
fn opened(app_data: &std::path::Path, folder: &std::path::Path) -> Ui {
    create_workspace(app_data, "Garden", folder);
    for name in ["Alpha.md", "Beta.md", "Gamma.md"] {
        std::fs::write(folder.join(name), b"x").unwrap();
    }
    let mut harness = harness_registering(app_data, FakePicker::default(), |app| {
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
fn the_tab_key_reaches_the_selected_tab_from_the_tree() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    for name in ["Alpha", "Beta", "Gamma"] {
        double_click(&mut harness, name);
    }
    harness.get_by_role_and_label(Role::Tab, "Beta").click();
    harness.run();
    row(&harness, "Alpha").focus();
    harness.run();

    let mut presses = 0;
    while focused_tab(&harness).is_none() {
        assert!(presses < 5, "Tab never reached the tab bar");
        harness.key_press(Key::Tab);
        harness.run();
        presses += 1;
    }

    assert_eq!(focused_tab(&harness).as_deref(), Some("Beta"));
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

/// Open `name`'s row menu with a right-click and choose `item`.
fn choose_from_row_menu(harness: &mut Ui, name: &str, item: &str) {
    row(harness, name).click_secondary();
    harness.run();
    harness
        .get_by_label(&format!("Actions for {name}"))
        .get_by_role_and_label(Role::Button, item)
        .click();
    harness.run();
}

#[test]
fn renaming_a_file_from_its_row_menu_renames_its_tab() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    choose_from_row_menu(&mut harness, "Alpha", "Open in new tab");
    row(&harness, "Beta").click();
    harness.run();
    assert_eq!(tabs(&harness), ["Alpha", "Beta (preview)"]);

    choose_from_row_menu(&mut harness, "Alpha", "Rename\u{2026}");
    harness
        .get_by_role_and_label(Role::TextInput, "New name for Alpha")
        .type_text("Omega");
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();

    assert!(folder.join("Omega.md").is_file());
    assert_eq!(tabs(&harness), ["Omega", "Beta (preview)"]);
    assert_eq!(selected(&harness).as_deref(), Some("Beta (preview)"));
}

#[test]
fn moving_a_file_to_the_trash_from_its_row_menu_closes_its_tab() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    choose_from_row_menu(&mut harness, "Alpha", "Open in new tab");
    choose_from_row_menu(&mut harness, "Beta", "Open in new tab");
    choose_from_row_menu(&mut harness, "Gamma", "Open");
    assert_eq!(tabs(&harness), ["Alpha", "Beta", "Gamma (preview)"]);
    harness.get_by_role_and_label(Role::Tab, "Beta").click();
    harness.run();

    choose_from_row_menu(&mut harness, "Beta", "Move to Trash");

    assert!(!folder.join("Beta.md").exists());
    assert_eq!(tabs(&harness), ["Alpha", "Gamma (preview)"]);
    assert_eq!(selected(&harness).as_deref(), Some("Gamma (preview)"));
}

/// At a normal window width, tabs past the window's edge scroll sideways
/// with the mouse wheel, inside the title bar row, with no scrollbar.
#[test]
fn tabs_fit_the_title_bar_row_with_no_scrollbar_and_still_scroll() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    let names: Vec<String> = (1..=12).map(|n| format!("A long note name {n}")).collect();
    for name in &names {
        std::fs::write(folder.join(format!("{name}.md")), b"x").unwrap();
    }
    let mut harness = harness_registering(app_data.path(), FakePicker::default(), |_| {});
    settle(&mut harness);
    harness.set_size(egui::vec2(1100.0, 750.0));
    harness.run();
    for name in &names {
        harness.state_mut().app_mut().open_file(
            &format!("{name}.md"),
            app_core::OpenAs::Kept,
        );
    }
    harness.state_mut().app_mut().open_page(app_core::INBOX_PAGE, app_core::OpenAs::Kept);
    harness.run();

    let tab_rect = |harness: &Ui, name: &str| harness.get_by_role_and_label(Role::Tab, name).rect();
    let first = tab_rect(&harness, &names[0]);
    let last = tab_rect(&harness, "Inbox");
    // The row starts where the window's content does, here the harness's
    // margin, and no scrollbar takes room from its 40 points.
    let row_top = harness
        .get_by_role_and_label(Role::Splitter, "Resize panel")
        .rect()
        .top();
    assert!(first.top() > row_top, "{first:?} under {row_top}");
    assert!(first.bottom() < row_top + 40.0, "{first:?} under {row_top}");
    assert!(
        (first.center().y - (row_top + 20.0)).abs() <= 1.0,
        "{first:?} under {row_top}"
    );
    assert!(
        first.left() < 0.0 || last.right() > 1100.0,
        "the tabs should run past the window: {first:?} {last:?}"
    );

    let wheel = |harness: &mut Ui, delta: f32| {
        harness.hover_at(egui::pos2(600.0, 28.0));
        harness.run();
        for _ in 0..8 {
            harness.event(Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, delta),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: Modifiers::NONE,
            });
            harness.step();
        }
        harness.run();
    };

    // The Inbox opened last, so the bar may sit anywhere; scroll to its
    // start, then down moves the tabs left.
    wheel(&mut harness, 300.0);
    let start = tab_rect(&harness, &names[0]).left();
    wheel(&mut harness, -300.0);
    let moved = tab_rect(&harness, &names[0]).left();
    assert!(moved < start - 200.0, "the wheel moved {start} to {moved}");
    assert!(
        tab_rect(&harness, "Inbox").right() <= 1100.0,
        "the wheel reaches the last tab"
    );
}
