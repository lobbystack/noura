//! Text files and notes with broken properties through AccessKit: a text
//! file opens as source with no formatting toolbar, a broken note shows a
//! notice above its text, and fixing its properties opens it as a note with
//! its properties panel.

mod support;

use std::{
    path::Path,
    time::{Duration, Instant},
};

use egui::{Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

const NOTICE: &str = "The properties at the top of this file have an error";

/// A workspace holding `files`, each a name and its text, with `open`
/// opened from the tree.
fn opened(app_data: &Path, folder: &Path, files: &[(&str, &str)], open: &str) -> Ui {
    create_workspace(app_data, "Garden", folder);
    for (name, text) in files {
        std::fs::write(folder.join(name), text).unwrap();
    }
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness.get_by_role_and_label(Role::TreeItem, open).click();
    harness.run_steps(4);
    harness
}

/// Run frames, waiting in real time, until `done` holds.
fn wait_until(harness: &mut Ui, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(harness) {
        assert!(Instant::now() < deadline, "the screen never got there");
        std::thread::sleep(Duration::from_millis(20));
        harness.step();
    }
    harness.run_steps(4);
}

/// The open note's text as the editor last reported it.
fn note_text(harness: &Ui) -> String {
    let app = harness.state().app();
    let tab = app.active_tab().expect("a tab is open").id();
    app.note(tab)
        .expect("the tab shows a note")
        .text()
        .to_owned()
}

#[test]
fn a_text_file_opens_as_source_without_the_formatting_toolbar() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let harness = opened(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("notes.txt", "# not a heading\n")],
        "notes.txt",
    );

    harness.get_by_role_and_label(Role::Document, "Note text");
    assert!(
        harness
            .query_by_role_and_label(Role::Toolbar, "Formatting")
            .is_none()
    );
    assert!(harness.query_by_label(NOTICE).is_none());
    assert_eq!(
        harness
            .get_by_role_and_label(Role::TextInput, "File name")
            .accesskit_node()
            .value()
            .as_deref(),
        Some("notes")
    );
}

#[test]
fn a_note_with_broken_properties_shows_a_notice_above_its_text() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let broken = "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\n---\nBody\n";
    let harness = opened(
        app_data.path(),
        &parent.path().join("Garden"),
        &[("Plan.md", broken)],
        "Plan",
    );

    harness.get_by_role_and_label(Role::Alert, NOTICE);
    harness.get_by_label("The properties have an ID but no type. Fix them in the text below.");
    harness.get_by_role_and_label(Role::Document, "Note text");
    assert!(
        harness
            .query_by_role_and_label(Role::Group, "Properties")
            .is_none()
    );
    assert!(
        harness
            .query_by_role_and_label(Role::Toolbar, "Formatting")
            .is_none()
    );
    assert_eq!(note_text(&harness), broken);
}

#[test]
fn fixing_the_properties_opens_the_note_with_its_properties_panel() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(
        app_data.path(),
        &folder,
        &[(
            "Plan.md",
            "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\n---\nBody\n",
        )],
        "Plan",
    );
    let fixed = "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\n---\nBody\n";

    harness.key_press_modifiers(Modifiers::COMMAND, egui::Key::A);
    harness.run_steps(2);
    harness.event(egui::Event::Paste(fixed.to_owned()));
    harness.run_steps(2);
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Group, "Properties")
            .is_some()
    });

    assert!(harness.query_by_label(NOTICE).is_none());
    harness.get_by_role_and_label(Role::Toolbar, "Formatting");
    assert_eq!(note_text(&harness), "Body\n");
    assert_eq!(
        std::fs::read_to_string(folder.join("Plan.md")).unwrap(),
        fixed
    );
    let written = std::fs::metadata(folder.join("Plan.md"))
        .unwrap()
        .modified()
        .unwrap();
    // The new editor reports the note's text, which is no edit.
    for _ in 0..30 {
        std::thread::sleep(Duration::from_millis(20));
        harness.step();
    }
    let app = harness.state().app();
    assert!(!app.is_saving_notes());
    assert!(!app.has_unsaved_notes());
    assert_eq!(
        std::fs::metadata(folder.join("Plan.md"))
            .unwrap()
            .modified()
            .unwrap(),
        written
    );

    // The keyboard stays in the note's text, which holds only the body now.
    harness.event(egui::Event::Text("x".into()));
    harness.run_steps(3);
    let text = note_text(&harness);
    assert_eq!(text.len(), "xBody\n".len(), "{text:?}");
    assert!(text.contains('x') && text.contains("Body"), "{text:?}");
}
