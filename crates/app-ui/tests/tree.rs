//! The file tree and the document area, through AccessKit.

mod support;

use egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase, accesskit::Role, vec2,
};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness, settle};

/// A workspace with two folders and two notes, opened at launch.
fn opened(
    app_data: &std::path::Path,
    folder: &std::path::Path,
) -> Harness<'static, app_ui::NouraUi> {
    create_workspace(app_data, "Garden", folder);
    for (path, bytes) in [
        ("Docs/Guide.md", b"# Guide\n".as_slice()),
        ("Notes/Seeds.md", b"x"),
        ("Readme.md", b"x"),
        ("Todo.md", b"x"),
    ] {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness
}

fn row<'a>(
    harness: &'a Harness<'static, app_ui::NouraUi>,
    name: &'a str,
) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::TreeItem, name)
}

fn expanded(harness: &Harness<'static, app_ui::NouraUi>, name: &str) -> Option<bool> {
    row(harness, name).accesskit_node().data().is_expanded()
}

fn focused(harness: &Harness<'static, app_ui::NouraUi>) -> String {
    harness
        .get_all_by_role(Role::TreeItem)
        .find(|node| node.is_focused())
        .and_then(|node| node.accesskit_node().label())
        .unwrap_or_default()
}

fn press(harness: &mut Harness<'static, app_ui::NouraUi>, key: Key) {
    harness.key_press(key);
    harness.run();
}

#[test]
fn rows_expose_their_name_level_and_expanded_state() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));

    harness.get_by_role_and_label(Role::Tree, "Files");
    let names: Vec<String> = harness
        .get_all_by_role(Role::TreeItem)
        .filter_map(|node| node.accesskit_node().label())
        .collect();
    assert_eq!(names, ["Docs", "Notes", "Readme", "Todo"]);
    assert_eq!(row(&harness, "Docs").accesskit_node().level(), Some(0));
    assert_eq!(expanded(&harness, "Docs"), Some(false));
    assert_eq!(expanded(&harness, "Readme"), None);

    row(&harness, "Docs").click();
    harness.run();

    assert_eq!(expanded(&harness, "Docs"), Some(true));
    assert_eq!(row(&harness, "Guide").accesskit_node().level(), Some(1));
    assert!(harness.unnamed_widgets().is_empty());
}

fn description(harness: &Harness<'static, app_ui::NouraUi>, name: &str) -> Option<String> {
    row(harness, name).accesskit_node().description()
}

/// accesskit_macos doesn't pass the expanded state to VoiceOver, so the
/// row's description says it there.
#[cfg(target_os = "macos")]
#[test]
fn on_macos_a_folder_row_describes_whether_it_is_open() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    assert_eq!(description(&harness, "Docs").as_deref(), Some("collapsed"));
    assert_eq!(description(&harness, "Readme"), None);

    row(&harness, "Docs").click();
    harness.run();

    assert_eq!(description(&harness, "Docs").as_deref(), Some("expanded"));
}

#[test]
fn a_file_cloud_storage_has_not_downloaded_says_so_and_does_not_open() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    std::fs::write(folder.join(".Cloud note.md.icloud"), b"placeholder").unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);

    assert_eq!(
        description(&harness, "Cloud note").as_deref(),
        Some("not downloaded")
    );
    row(&harness, "Cloud note").click();
    harness.run();

    harness.get_by_role(Role::Alert);
    harness.get_by_label("Couldn't open \u{201c}Cloud note\u{201d}");
    harness.get_by_label("This file isn't downloaded yet. Try again once it is.");
    let document = harness.get_by_role_and_label(Role::Region, "Document");
    assert!(document.query_by_label("Cloud note.md").is_none());
}

#[test]
fn a_file_cloud_storage_has_not_downloaded_offers_no_duplicate() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    std::fs::write(folder.join(".Cloud note.md.icloud"), b"placeholder").unwrap();
    std::fs::write(folder.join("Local note.md"), b"# Local note\n").unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);

    let items = |harness: &mut Harness<'static, app_ui::NouraUi>, name: &str| -> Vec<String> {
        row(harness, name).click_secondary();
        harness.run();
        let items = harness
            .get_by_label(&format!("Actions for {name}"))
            .get_all_by_role(Role::Button)
            .filter_map(|node| node.accesskit_node().label())
            .collect();
        press(harness, Key::Escape);
        items
    };

    assert!(items(&mut harness, "Local note").contains(&"Duplicate".to_owned()));
    let cloud = items(&mut harness, "Cloud note");
    assert!(cloud.contains(&"Open".to_owned()), "{cloud:?}");
    assert!(!cloud.contains(&"Duplicate".to_owned()), "{cloud:?}");
}

#[test]
fn the_not_downloaded_marker_clears_once_the_cloud_drive_downloads_the_file() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    std::fs::write(folder.join(".Cloud note.md.icloud"), b"placeholder").unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    assert_eq!(
        description(&harness, "Cloud note").as_deref(),
        Some("not downloaded")
    );

    // The download replaces the placeholder with the note, and the tree
    // follows with no action from you.
    std::fs::write(folder.join("Cloud note.md"), b"# Cloud note\n").unwrap();
    std::fs::remove_file(folder.join(".Cloud note.md.icloud")).unwrap();

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while description(&harness, "Cloud note").is_some() {
        assert!(
            std::time::Instant::now() < deadline,
            "the marker never cleared"
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        harness.run();
    }
}

#[test]
fn arrow_keys_home_end_and_type_ahead_move_through_the_tree() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    row(&harness, "Docs").focus();
    harness.run();
    assert_eq!(focused(&harness), "Docs");

    press(&mut harness, Key::ArrowRight);
    assert_eq!(expanded(&harness, "Docs"), Some(true));
    assert_eq!(focused(&harness), "Docs");
    press(&mut harness, Key::ArrowRight);
    assert_eq!(focused(&harness), "Guide");
    press(&mut harness, Key::ArrowLeft);
    assert_eq!(focused(&harness), "Docs");
    press(&mut harness, Key::ArrowLeft);
    assert_eq!(expanded(&harness, "Docs"), Some(false));

    press(&mut harness, Key::ArrowDown);
    assert_eq!(focused(&harness), "Notes");
    press(&mut harness, Key::End);
    assert_eq!(focused(&harness), "Todo");
    press(&mut harness, Key::ArrowUp);
    assert_eq!(focused(&harness), "Readme");
    press(&mut harness, Key::Home);
    assert_eq!(focused(&harness), "Docs");

    harness.event(Event::Text("t".into()));
    harness.run();
    assert_eq!(focused(&harness), "Todo");
}

#[test]
fn enter_or_space_toggles_a_folder_and_opens_a_file() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    row(&harness, "Notes").focus();
    harness.run();

    press(&mut harness, Key::Space);
    assert_eq!(expanded(&harness, "Notes"), Some(true));
    press(&mut harness, Key::Enter);
    assert_eq!(expanded(&harness, "Notes"), Some(false));

    press(&mut harness, Key::ArrowDown);
    press(&mut harness, Key::Enter);

    let document = harness.get_by_role_and_label(Role::Region, "Document");
    document.get_by_label("Readme");
    document.get_by_label("Readme.md");
}

#[test]
fn opening_a_file_shows_its_name_and_workspace_relative_path() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    row(&harness, "Docs").click();
    harness.run();

    let document = harness.get_by_role_and_label(Role::Region, "Document");
    assert!(document.query_by_label("Guide").is_none());
    row(&harness, "Guide").click();
    harness.run();

    let document = harness.get_by_role_and_label(Role::Region, "Document");
    document.get_by_label("Guide");
    document.get_by_label("Docs/Guide.md");
    assert_eq!(
        row(&harness, "Guide").accesskit_node().is_selected(),
        Some(true)
    );
    assert_eq!(
        std::fs::read(folder.join("Docs/Guide.md")).unwrap(),
        b"# Guide\n"
    );
}

/// A workspace with `count` notes, `N 000` and up, opened at launch.
fn many_notes(
    app_data: &std::path::Path,
    folder: &std::path::Path,
    count: usize,
) -> Harness<'static, app_ui::NouraUi> {
    create_workspace(app_data, "Notes", folder);
    for index in 0..count {
        std::fs::write(folder.join(format!("N {index:03}.md")), b"x").unwrap();
    }
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness
}

/// The part of the window the tree can show: below the workspace switcher.
fn tree_area(harness: &Harness<'static, app_ui::NouraUi>) -> egui::Rect {
    let switcher = harness
        .get_by_label_contains("Switch workspace")
        .rect()
        .bottom();
    let mut area = harness.ctx.content_rect();
    area.min.y = switcher;
    area
}

/// Whether `name`'s row is drawn where you can see it.
fn in_view(harness: &Harness<'static, app_ui::NouraUi>, name: &str) -> bool {
    let area = tree_area(harness);
    harness
        .query_by_role_and_label(Role::TreeItem, name)
        .is_some_and(|row| area.contains_rect(row.rect()))
}

/// Scroll the tree with the mouse wheel, as far as `points` goes.
fn wheel(harness: &mut Harness<'static, app_ui::NouraUi>, points: f32) {
    let first = harness
        .get_all_by_role(Role::TreeItem)
        .next()
        .unwrap()
        .rect();
    harness.hover_at(first.center());
    harness.run();
    for _ in 0..10 {
        harness.event(Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, -points / 10.0),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        });
        harness.step();
    }
    harness.run();
}

#[test]
fn the_arrow_keys_carry_on_after_the_focused_row_scrolls_out_of_view() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = many_notes(app_data.path(), &parent.path().join("Notes"), 200);
    row(&harness, "N 003").focus();
    harness.run();
    assert_eq!(focused(&harness), "N 003");

    wheel(&mut harness, 4000.0);
    assert!(!in_view(&harness, "N 003"), "the tree didn't scroll");
    assert_eq!(focused(&harness), "N 003");

    press(&mut harness, Key::ArrowDown);
    assert_eq!(focused(&harness), "N 004");
    assert!(in_view(&harness, "N 004"));
}

#[test]
fn tab_leaves_the_tree_and_shift_tab_comes_back_to_the_same_row() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    row(&harness, "Notes").focus();
    harness.run();
    assert_eq!(focused(&harness), "Notes");

    press(&mut harness, Key::Tab);
    assert_eq!(focused(&harness), "", "Tab stayed in the tree");
    let next = harness.get_by(|node| node.is_focused());
    assert_ne!(next.accesskit_node().role(), Role::TreeItem);

    harness.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
    harness.run();
    assert_eq!(focused(&harness), "Notes");
}

#[test]
fn tab_comes_back_to_a_row_scrolled_out_of_view_and_shows_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = many_notes(app_data.path(), &parent.path().join("Notes"), 200);
    row(&harness, "N 003").focus();
    harness.run();
    press(&mut harness, Key::Tab);
    assert_eq!(focused(&harness), "");

    wheel(&mut harness, 4000.0);
    assert!(!in_view(&harness, "N 003"), "the tree didn't scroll");
    harness.key_press_modifiers(Modifiers::SHIFT, Key::Tab);
    harness.run();

    assert_eq!(focused(&harness), "N 003");
    assert!(in_view(&harness, "N 003"));
}
