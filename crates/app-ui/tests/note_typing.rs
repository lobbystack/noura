//! Typing in a real note through the window: each test presses keys in the
//! editor, lets the save run, and reads the file back, so what the editor
//! shows and what the disk holds are checked together.

mod support;

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use app_core::Platform;
use egui::{Event, Key, Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::Queryable,
};
use support::{create_workspace, harness_for, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

const FRONTMATTER: &str = "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\n\
                           created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n---\n";

/// A workspace holding the note `Plan.md` with `body`, opened from the
/// tree, with the caret at the start of its text.
fn opened(app_data: &Path, folder: &Path, body: &str) -> (Ui, PathBuf) {
    create_workspace(app_data, "Garden", folder);
    let path = folder.join("Plan.md");
    std::fs::write(&path, format!("{FRONTMATTER}{body}")).unwrap();
    let mut harness = harness_for(app_data, Platform::Windows, |_| {});
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
    (harness, path)
}

fn type_text(harness: &mut Ui, text: &str) {
    harness.event(Event::Text(text.to_owned()));
    harness.run_steps(3);
}

fn press(harness: &mut Ui, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run_steps(3);
}

/// The note's text as the editor reports it to assistive technology.
fn shown(harness: &Ui) -> String {
    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Note text")
        .value()
        .expect("the note reports its text")
}

/// What the open note's session holds.
fn session_text(harness: &Ui) -> String {
    let app = harness.state().app();
    let tab = app.active_tab().expect("a tab is open").id();
    app.note(tab)
        .expect("the tab shows a note")
        .text()
        .to_owned()
}

/// Run frames until the file holds exactly `body` after its frontmatter
/// and nothing waits to save, then check the editor shows the same.
#[track_caller]
fn expect_saved(harness: &mut Ui, path: &Path, body: &str) {
    let expected = format!("{FRONTMATTER}{body}");
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        harness.step();
        let app = harness.state().app();
        if std::fs::read_to_string(path).unwrap() == expected
            && !app.has_unsaved_notes()
            && !app.is_saving_notes()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the file never held {expected:?}: it holds {:?}",
            std::fs::read_to_string(path).unwrap()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(session_text(harness), body);
    assert_eq!(shown(harness), body);
}

#[test]
fn double_spaces_and_a_markdown_line_break_survive() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, path) = opened(app_data.path(), &parent.path().join("Garden"), "");

    type_text(&mut harness, "line  ");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    type_text(&mut harness, "next  word");

    expect_saved(&mut harness, &path, "line  \nnext  word");
}

#[test]
fn spaces_at_the_end_of_a_line_and_of_the_note_stay_across_saves() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, path) = opened(app_data.path(), &parent.path().join("Garden"), "");

    type_text(&mut harness, "Hello ");
    expect_saved(&mut harness, &path, "Hello ");
    type_text(&mut harness, " world  ");
    expect_saved(&mut harness, &path, "Hello  world  ");

    // The save leaves the caret after the spaces.
    type_text(&mut harness, "!");
    expect_saved(&mut harness, &path, "Hello  world  !");
}

#[test]
fn enter_at_the_end_at_the_start_of_a_line_and_blank_lines_stay() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, path) = opened(app_data.path(), &parent.path().join("Garden"), "");

    type_text(&mut harness, "first");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    expect_saved(&mut harness, &path, "first\n");
    press(&mut harness, Modifiers::NONE, Key::Enter);
    expect_saved(&mut harness, &path, "first\n\n");
    type_text(&mut harness, "second");
    expect_saved(&mut harness, &path, "first\n\nsecond");

    // Enter at the start of a line pushes it down.
    press(&mut harness, Modifiers::NONE, Key::Home);
    press(&mut harness, Modifiers::NONE, Key::Enter);
    expect_saved(&mut harness, &path, "first\n\n\nsecond");
    type_text(&mut harness, "x");
    expect_saved(&mut harness, &path, "first\n\n\nxsecond");
}

#[test]
fn undo_and_redo_after_saves_reach_the_file() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, path) = opened(app_data.path(), &parent.path().join("Garden"), "");

    type_text(&mut harness, "alpha ");
    expect_saved(&mut harness, &path, "alpha ");
    std::thread::sleep(Duration::from_millis(600));
    harness.step();
    type_text(&mut harness, "beta");
    expect_saved(&mut harness, &path, "alpha beta");

    press(&mut harness, Modifiers::COMMAND, Key::Z);
    assert_eq!(session_text(&harness), "alpha ");
    expect_saved(&mut harness, &path, "alpha ");

    press(
        &mut harness,
        Modifiers::COMMAND | Modifiers::SHIFT,
        Key::Z,
    );
    assert_eq!(session_text(&harness), "alpha beta");
    expect_saved(&mut harness, &path, "alpha beta");
}

#[test]
fn undo_after_an_outside_edit_merged_keeps_the_outside_edit_in_the_file() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, path) = opened(app_data.path(), &parent.path().join("Garden"), "");

    type_text(&mut harness, "mine");
    expect_saved(&mut harness, &path, "mine");

    std::fs::write(&path, format!("{FRONTMATTER}mine\ntheirs\n")).unwrap();
    harness.state_mut().app_mut().rescan();
    expect_saved(&mut harness, &path, "mine\ntheirs\n");

    press(&mut harness, Modifiers::COMMAND, Key::End);
    type_text(&mut harness, "!");
    expect_saved(&mut harness, &path, "mine\ntheirs\n!");

    press(&mut harness, Modifiers::COMMAND, Key::Z);
    assert_eq!(session_text(&harness), "mine\ntheirs\n");
    expect_saved(&mut harness, &path, "mine\ntheirs\n");
}

/// The toolbar's Checklist makes a checklist, Mod-Enter and a click on the
/// box check its items, and the toolbar's Undo reaches the file.
#[test]
fn toolbar_buttons_and_checklists_reach_the_file() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let (mut harness, path) = opened(app_data.path(), &parent.path().join("Garden"), "");

    type_text(&mut harness, "buy milk");
    harness.get_by_label("Checklist (Ctrl+Shift+9)").click();
    harness.run_steps(3);
    assert_eq!(session_text(&harness), "- [ ] buy milk");

    press(&mut harness, Modifiers::COMMAND, Key::End);
    press(&mut harness, Modifiers::NONE, Key::Enter);
    type_text(&mut harness, "eggs");
    assert_eq!(session_text(&harness), "- [ ] buy milk\n- [ ] eggs");

    press(&mut harness, Modifiers::COMMAND, Key::Enter);
    assert_eq!(session_text(&harness), "- [ ] buy milk\n- [x] eggs");

    // The caret sits in the second item, so the first item draws its box,
    // left of its text.
    let milk = support::point_at(&harness, "buy milk", 0);
    let checkbox = egui::pos2(milk.x - 16.0, milk.y);
    harness.hover_at(checkbox);
    harness.run_steps(1);
    harness.drag_at(checkbox);
    harness.run_steps(1);
    harness.drop_at(checkbox);
    harness.run_steps(3);
    assert_eq!(session_text(&harness), "- [x] buy milk\n- [x] eggs");
    expect_saved(&mut harness, &path, "- [x] buy milk\n- [x] eggs");

    harness.get_by_label("Undo (Ctrl+Z)").click();
    harness.run_steps(3);
    assert_eq!(session_text(&harness), "- [ ] buy milk\n- [x] eggs");
    expect_saved(&mut harness, &path, "- [ ] buy milk\n- [x] eggs");
}

/// With a new note's name field, Enter moves the keyboard to the text at
/// once, so the next keys land in the note.
#[test]
fn enter_in_a_new_note_s_name_moves_to_its_text_at_once() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    let mut harness = harness_for(app_data.path(), Platform::Windows, |_| {});
    settle(&mut harness);

    harness
        .get_by_role_and_label(Role::Button, "New note")
        .click();
    harness.run_steps(4);
    harness
        .get_by_role_and_label(Role::TextInput, "File name")
        .focus();
    harness.run_steps(2);
    harness.key_press_modifiers(Modifiers::COMMAND, Key::A);
    harness.event(Event::Text("Quick".into()));
    harness.run_steps(3);
    press(&mut harness, Modifiers::NONE, Key::Enter);

    assert!(
        harness
            .get_by_role_and_label(Role::MultilineTextInput, "Note text")
            .is_focused()
    );
    type_text(&mut harness, "first words");
    let path = folder.join("Quick.md");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !std::fs::read_to_string(&path).is_ok_and(|text| text.ends_with("---\nfirst words")) {
        assert!(
            Instant::now() < deadline,
            "the typing never saved: {:?}, shown {:?}",
            std::fs::read_to_string(&path),
            shown(&harness)
        );
        std::thread::sleep(Duration::from_millis(10));
        harness.step();
    }
    assert_eq!(shown(&harness), "first words");
}
