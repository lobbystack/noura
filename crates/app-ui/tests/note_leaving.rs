//! Leaving a note with unsaved typing through the window: renaming it from
//! the tree, trashing it, and switching tabs. Each test reads the text the
//! editor shows and the file.

mod support;

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use egui::{Event, Key, Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A managed note holding `body`.
fn note(id_end: char, body: &str) -> String {
    format!(
        "---\nid: note_01jbcdefghjkmnpqrstvwxyz0{id_end}\ntype: note\n\
         created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n---\n{body}"
    )
}

/// A workspace holding `notes`, each a file name, the end of its ID and
/// its text, opened at launch.
fn opened(app_data: &Path, folder: &Path, notes: &[(&str, char, &str)]) -> Ui {
    create_workspace(app_data, "Garden", folder);
    for (name, id_end, body) in notes {
        std::fs::write(folder.join(name), note(*id_end, body)).unwrap();
    }
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness
}

fn row<'a>(harness: &'a Ui, name: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::TreeItem, name)
}

/// Open `name` from the tree, with the keyboard in its text.
fn open(harness: &mut Ui, name: &str) {
    row(harness, name).click();
    harness.run_steps(4);
}

fn type_text(harness: &mut Ui, text: &str) {
    harness.event(Event::Text(text.to_owned()));
    harness.run_steps(3);
}

/// The text the document area's editor shows.
fn shown(harness: &Ui) -> String {
    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Note text")
        .value()
        .expect("the note reports its text")
}

fn name_shown(harness: &Ui) -> Option<String> {
    harness
        .get_by_role_and_label(Role::TextInput, "File name")
        .accesskit_node()
        .value()
}

/// The selected tab's label.
fn selected(harness: &Ui) -> Option<String> {
    harness
        .query_all_by_role(Role::Tab)
        .find(|tab| tab.accesskit_node().is_selected() == Some(true))
        .and_then(|tab| tab.accesskit_node().label())
}

fn tab_labels(harness: &Ui) -> Vec<String> {
    harness
        .query_all_by_role(Role::Tab)
        .filter_map(|tab| tab.accesskit_node().label())
        .collect()
}

/// Open `name`'s row menu with a right-click and choose `item`.
fn choose_from_row_menu(harness: &mut Ui, name: &str, item: &str) {
    row(harness, name).click_secondary();
    harness.run_steps(4);
    harness
        .get_by_label(&format!("Actions for {name}"))
        .get_by_role_and_label(Role::Button, item)
        .click();
    harness.run_steps(4);
}

/// Run frames, waiting in real time, until `done` holds.
fn wait_until(harness: &mut Ui, what: &str, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(harness) {
        assert!(Instant::now() < deadline, "never happened: {what}");
        std::thread::sleep(Duration::from_millis(10));
        harness.step();
    }
    harness.run_steps(4);
}

fn wait_for_bytes(harness: &mut Ui, path: &Path, expected: &str) {
    let what = format!("{} holds {expected:?}", path.display());
    wait_until(harness, &what, |_| {
        std::fs::read_to_string(path).is_ok_and(|text| text == expected)
    });
}

/// The names in `folder`, without `.noura`, sorted.
fn names(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name != ".noura")
        .collect();
    names.sort();
    names
}

/// The one file the workspace's trash folder holds under `name`.
fn in_trash(folder: &Path, name: &str) -> PathBuf {
    let stamps: Vec<PathBuf> = std::fs::read_dir(folder.join(".noura/trash"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join(name))
        .filter(|path| path.exists())
        .collect();
    let [found] = stamps.as_slice() else {
        panic!("{name} in the trash: {stamps:?}");
    };
    found.clone()
}

#[test]
fn renaming_from_the_tree_while_typing_keeps_the_typing() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder, &[("Before.md", 'a', "")]);
    open(&mut harness, "Before");
    type_text(&mut harness, "some text");

    choose_from_row_menu(&mut harness, "Before", "Rename\u{2026}");
    harness
        .get_by_role_and_label(Role::TextInput, "New name for Before")
        .type_text("After");
    harness.run_steps(3);
    harness.key_press(Key::Enter);
    harness.run_steps(4);

    row(&harness, "After");
    assert_eq!(names(&folder), ["After.md"]);
    assert_eq!(
        std::fs::read_to_string(folder.join("After.md")).unwrap(),
        note('a', "some text")
    );
    assert_eq!(shown(&harness), "some text");
    assert_eq!(name_shown(&harness).as_deref(), Some("After"));

    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Note text")
        .focus();
    harness.run_steps(3);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::End);
    harness.run_steps(3);
    type_text(&mut harness, " and more");
    wait_for_bytes(
        &mut harness,
        &folder.join("After.md"),
        &note('a', "some text and more"),
    );
    assert_eq!(names(&folder), ["After.md"]);
    assert_eq!(shown(&harness), "some text and more");
}

#[test]
fn f2_changes_only_the_case_of_the_open_note_s_name() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder, &[("draft.md", 'a', "body text")]);
    open(&mut harness, "draft");

    row(&harness, "draft").focus();
    harness.run_steps(3);
    harness.key_press(Key::F2);
    harness.run_steps(4);
    harness
        .get_by_role_and_label(Role::TextInput, "New name for draft")
        .type_text("Final");
    harness.run_steps(3);
    harness.key_press(Key::Enter);
    harness.run_steps(4);
    assert_eq!(name_shown(&harness).as_deref(), Some("Final"));

    row(&harness, "Final").focus();
    harness.run_steps(3);
    harness.key_press(Key::F2);
    harness.run_steps(4);
    harness
        .get_by_role_and_label(Role::TextInput, "New name for Final")
        .type_text("FINAL");
    harness.run_steps(3);
    harness.key_press(Key::Enter);
    harness.run_steps(4);

    row(&harness, "FINAL");
    assert_eq!(name_shown(&harness).as_deref(), Some("FINAL"));
    assert_eq!(names(&folder), ["FINAL.md"]);
    assert_eq!(
        std::fs::read_to_string(folder.join("FINAL.md")).unwrap(),
        note('a', "body text")
    );
    assert_eq!(shown(&harness), "body text");
    assert_eq!(selected(&harness).as_deref(), Some("FINAL (preview)"));
}

#[test]
fn trashing_the_open_note_saves_its_typing_and_the_next_tab_shows_its_own_text() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(
        app_data.path(),
        &folder,
        &[("Keep.md", 'a', "kept"), ("Doomed.md", 'b', "unsaved soon")],
    );
    choose_from_row_menu(&mut harness, "Keep", "Open in new tab");
    choose_from_row_menu(&mut harness, "Doomed", "Open in new tab");
    harness
        .get_by_role_and_label(Role::Tab, "Doomed")
        .click();
    harness.run_steps(4);
    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Note text")
        .focus();
    harness.run_steps(3);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::End);
    harness.run_steps(3);
    type_text(&mut harness, " last words");

    choose_from_row_menu(&mut harness, "Doomed", "Move to Trash");

    wait_until(&mut harness, "the note leaves the folder", |harness| {
        !folder.join("Doomed.md").exists() && harness.state().app().tabs().len() == 1
    });
    assert!(harness.query_by_role_and_label(Role::TreeItem, "Doomed").is_none());
    assert_eq!(tab_labels(&harness), ["Keep"]);
    assert_eq!(selected(&harness).as_deref(), Some("Keep"));
    assert_eq!(name_shown(&harness).as_deref(), Some("Keep"));
    assert_eq!(shown(&harness), "kept");
    assert_eq!(names(&folder), ["Keep.md"]);
    assert_eq!(
        std::fs::read_to_string(in_trash(&folder, "Doomed.md")).unwrap(),
        note('b', "unsaved soon last words")
    );

    // Trashing the last open note leaves nothing open or highlighted.
    choose_from_row_menu(&mut harness, "Keep", "Move to Trash");

    wait_until(&mut harness, "the last tab closes", |harness| {
        harness.state().app().tabs().is_empty()
    });
    assert!(tab_labels(&harness).is_empty());
    assert_eq!(selected(&harness), None);
    harness.get_by_label("Open a file from the sidebar.");
    assert!(
        harness
            .query_by_role_and_label(Role::MultilineTextInput, "Note text")
            .is_none()
    );
    assert!(names(&folder).is_empty());
}

#[test]
fn switching_tabs_keeps_each_note_s_text() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(
        app_data.path(),
        &folder,
        &[("One.md", 'a', ""), ("Two.md", 'b', "")],
    );
    choose_from_row_menu(&mut harness, "One", "Open in new tab");
    choose_from_row_menu(&mut harness, "Two", "Open in new tab");

    harness.get_by_role_and_label(Role::Tab, "One").click();
    harness.run_steps(4);
    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Note text")
        .focus();
    harness.run_steps(3);
    type_text(&mut harness, "text of one");
    harness.get_by_role_and_label(Role::Tab, "Two").click();
    harness.run_steps(4);
    assert_eq!(shown(&harness), "");
    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Note text")
        .focus();
    harness.run_steps(3);
    type_text(&mut harness, "text of two");

    harness.get_by_role_and_label(Role::Tab, "One").click();
    harness.run_steps(4);
    assert_eq!(shown(&harness), "text of one");
    harness.get_by_role_and_label(Role::Tab, "Two").click();
    harness.run_steps(4);
    assert_eq!(shown(&harness), "text of two");

    wait_for_bytes(&mut harness, &folder.join("One.md"), &note('a', "text of one"));
    wait_for_bytes(&mut harness, &folder.join("Two.md"), &note('b', "text of two"));
}
