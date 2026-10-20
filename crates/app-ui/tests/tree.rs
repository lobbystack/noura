//! The file tree and the document area, through AccessKit.

mod support;

use egui::{Event, Key, accesskit::Role};
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
