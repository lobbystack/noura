//! Seam 1: a Markdown note whose properties don't parse opens as source
//! under a notice, saves only your edits, and opens as a note once a save
//! or a conflict choice leaves its properties readable, including
//! properties that broke while you typed.

mod support;

use std::time::{Duration, Instant};

use app_core::{App, ConflictChoice, SAVE_PAUSE, TabId};
use support::{Fixture, open_from_tree, settle_saves};

const ID: &str = "note_01jbcdefghjkmnpqrstvwxyz0a";

/// The editor's text after `change`, as the editor reports it.
fn type_into(app: &mut App, tab: TabId, at: Instant, change: impl FnOnce(&str) -> String) {
    let text = change(app.note(tab).unwrap().text());
    app.edit_note(tab, 1, &text, at);
}

/// Let the save that typing at `at` started run to its end.
fn save_after_pause(app: &mut App, at: Instant) {
    app.save_due_notes(at + SAVE_PAUSE);
    settle_saves(app);
}

fn modified(fixture: &Fixture, relative: &str) -> std::time::SystemTime {
    std::fs::metadata(fixture.root.join(relative))
        .unwrap()
        .modified()
        .unwrap()
}

/// Each broken note: its file, its text, and why its properties don't
/// parse, as the notice says.
fn broken() -> [(&'static str, String, &'static str); 4] {
    [
        (
            "Unclosed.md",
            format!("---\nid: {ID}\ntype: note\n\nBody\n"),
            "The properties have no closing --- line.",
        ),
        (
            "Invalid YAML.md",
            format!("---\nid: {ID}\ntype: note\ntags: [\n---\nBody\n"),
            "The properties aren\u{2019}t valid YAML.",
        ),
        (
            "No type.md",
            format!("---\nid: {ID}\n---\nBody\n"),
            "The properties have an ID but no type.",
        ),
        (
            "Mismatch.md",
            "---\nid: task_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\n---\nBody\n".to_owned(),
            "The ID doesn\u{2019}t match the type.",
        ),
    ]
}

#[test]
fn a_note_with_broken_properties_opens_whole_as_source_and_says_why() {
    let notes = broken();
    let files: Vec<(&str, &[u8])> = notes
        .iter()
        .map(|(path, text, _)| (*path, text.as_bytes()))
        .collect();
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();

    for (path, text, reason) in &notes {
        let tab = open_from_tree(&mut app, path);
        let note = app.note(tab).unwrap();
        assert_eq!(note.text(), text, "{path}");
        assert!(note.shows_source(), "{path}");
        assert_eq!(note.broken_properties(), Some(*reason), "{path}");
        assert_eq!(note.properties(), None, "{path}");
        assert_eq!(note.object_id(), None, "{path}");
    }
}

#[test]
fn notes_with_readable_properties_have_no_notice() {
    let fixture = Fixture::new(&[
        (
            "Note.md",
            format!("---\nid: {ID}\ntype: note\n---\nBody\n").as_bytes(),
        ),
        ("Plain.md", b"---\ntags: [a]\n---\nText\n"),
        ("No frontmatter.md", b"# Title\n"),
    ]);
    let mut app = fixture.app();

    for path in ["Note.md", "Plain.md", "No frontmatter.md"] {
        let tab = open_from_tree(&mut app, path);
        let note = app.note(tab).unwrap();
        assert_eq!(note.broken_properties(), None, "{path}");
        assert!(!note.shows_source(), "{path}");
    }
}

#[test]
fn saving_a_broken_note_writes_only_your_edits() {
    let notes = broken();
    let files: Vec<(&str, &[u8])> = notes
        .iter()
        .map(|(path, text, _)| (*path, text.as_bytes()))
        .collect();
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();
    let edit = |text: &str| {
        text.replacen("---\n", "---\ncolor: red\n", 1)
            .replace("Body\n", "Body, edited\n")
    };

    for (path, text, reason) in &notes {
        let tab = open_from_tree(&mut app, path);
        let at = Instant::now();
        type_into(&mut app, tab, at, edit);
        save_after_pause(&mut app, at);

        assert_eq!(
            String::from_utf8(fixture.read(path)).unwrap(),
            edit(text),
            "{path}"
        );
        let note = app.note(tab).unwrap();
        assert!(!note.is_unsaved(), "{path}");
        assert_eq!(note.broken_properties(), Some(*reason), "{path}");
        assert!(note.shows_source(), "{path}");
    }
}

#[test]
fn saving_a_broken_note_keeps_its_line_endings_and_byte_order_mark() {
    let bytes =
        b"\xEF\xBB\xBF---\r\nid: note_01jbcdefghjkmnpqrstvwxyz0a\r\ntitle: [\r\n---\r\nBody\nmore";
    let fixture = Fixture::new(&[("Broken.md", bytes)]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Broken.md");
    assert_eq!(
        app.note(tab).unwrap().text(),
        "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntitle: [\n---\nBody\nmore"
    );
    let at = Instant::now();

    type_into(&mut app, tab, at, |text| text.replace("more", "more text"));
    save_after_pause(&mut app, at);

    assert_eq!(
        fixture.read("Broken.md"),
        b"\xEF\xBB\xBF---\r\nid: note_01jbcdefghjkmnpqrstvwxyz0a\r\ntitle: [\r\n---\r\nBody\nmore text"
    );
}

#[test]
fn a_save_that_fixes_the_properties_opens_the_note_without_a_second_write() {
    let notes = broken();
    let files: Vec<(&str, &[u8])> = notes
        .iter()
        .map(|(path, text, _)| (*path, text.as_bytes()))
        .collect();
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();
    let fixed = format!("---\nid: {ID}\ntype: note\n---\nBody\n");

    for (path, ..) in &notes {
        let tab = open_from_tree(&mut app, path);
        let at = Instant::now();
        type_into(&mut app, tab, at, |_| fixed.clone());
        save_after_pause(&mut app, at);
        let written = modified(&fixture, path);

        let note = app.note(tab).unwrap();
        assert_eq!(note.broken_properties(), None, "{path}");
        assert!(!note.shows_source(), "{path}");
        assert_eq!(note.object_id(), Some(ID), "{path}");
        assert_eq!(note.text(), "Body\n", "{path}");
        let properties = note.properties().expect("the properties panel shows");
        assert_eq!(properties[0].key, "id");
        assert!(!note.is_unsaved(), "{path}");
        // The editor starts again from the note's text, which is no edit.
        assert_eq!(
            app.take_note_reset(tab).as_deref(),
            Some("Body\n"),
            "{path}"
        );
        assert_eq!(app.take_note_sync(tab), None, "{path}");
        app.edit_note(tab, 0, "Body\n", at + SAVE_PAUSE);
        assert_eq!(app.next_note_save(), None, "{path}");
        app.save_due_notes(at + Duration::from_secs(10));
        settle_saves(&mut app);

        assert_eq!(
            String::from_utf8(fixture.read(path)).unwrap(),
            fixed,
            "{path}"
        );
        assert_eq!(
            modified(&fixture, path),
            written,
            "{path} was written again"
        );
        app.keep_tab(tab);
    }

    // Typing in the note now edits the body and keeps the properties.
    let tab = app.active_tab().unwrap().id();
    let path = app.note(tab).unwrap().relative_path().to_owned();
    let at = Instant::now();
    type_into(&mut app, tab, at, |text| format!("{text}more\n"));
    save_after_pause(&mut app, at);
    assert_eq!(
        String::from_utf8(fixture.read(&path)).unwrap(),
        format!("{fixed}more\n")
    );
}

#[test]
fn a_save_that_removes_the_id_opens_the_file_as_plain_markdown() {
    let fixture = Fixture::new(&[(
        "Broken.md",
        format!("---\nid: {ID}\n---\nBody\n").as_bytes(),
    )]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Broken.md");
    let at = Instant::now();

    type_into(&mut app, tab, at, |text| {
        text.replace(&format!("id: {ID}\n"), "tags: [a]\n")
    });
    save_after_pause(&mut app, at);

    let note = app.note(tab).unwrap();
    assert_eq!(note.broken_properties(), None);
    assert!(!note.shows_source());
    assert_eq!(note.properties(), None);
    assert_eq!(note.text(), "---\ntags: [a]\n---\nBody\n");
    assert!(!note.is_unsaved());
}

#[test]
fn typing_during_the_save_that_fixes_the_properties_waits_for_the_next_save() {
    let fixture = Fixture::new(&[(
        "Broken.md",
        format!("---\nid: {ID}\n---\nBody\n").as_bytes(),
    )]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Broken.md");
    let at = Instant::now();
    type_into(&mut app, tab, at, |text| {
        text.replace("---\nBody", "type: note\n---\nBody")
    });
    let lock = support::hold_writes(&fixture, &app);
    app.save_due_notes(at + SAVE_PAUSE);
    let later = at + SAVE_PAUSE;
    type_into(&mut app, tab, later, |text| format!("{text}typed\n"));
    lock.unlock().unwrap();
    settle_saves(&mut app);

    // The whole text shows until your typing is on disk too.
    let note = app.note(tab).unwrap();
    assert!(note.shows_source());
    assert_eq!(
        note.text(),
        format!("---\nid: {ID}\ntype: note\n---\nBody\ntyped\n")
    );
    save_after_pause(&mut app, later);

    let note = app.note(tab).unwrap();
    assert!(!note.shows_source());
    assert_eq!(note.text(), "Body\ntyped\n");
    assert_eq!(
        String::from_utf8(fixture.read("Broken.md")).unwrap(),
        format!("---\nid: {ID}\ntype: note\n---\nBody\ntyped\n")
    );
}

#[test]
fn properties_that_break_while_you_type_open_the_note_once_a_save_fixes_them() {
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    assert!(app.note(tab).unwrap().shows_whole_file());
    let at = Instant::now();
    type_into(&mut app, tab, at, |text| format!("---\nid: {ID}\n{text}"));
    save_after_pause(&mut app, at);

    let note = app.note(tab).unwrap();
    assert_eq!(
        note.broken_properties(),
        Some("The properties have no closing --- line.")
    );
    assert!(note.shows_whole_file());
    assert_eq!(note.properties(), None);

    let at = Instant::now();
    type_into(&mut app, tab, at, |text| {
        text.replace("# Plan", "type: note\n---\n# Plan")
    });
    save_after_pause(&mut app, at);
    let fixed = format!("---\nid: {ID}\ntype: note\n---\n# Plan\n");
    assert_eq!(String::from_utf8(fixture.read("Plan.md")).unwrap(), fixed);
    let written = modified(&fixture, "Plan.md");

    let note = app.note(tab).unwrap();
    assert_eq!(note.broken_properties(), None);
    assert!(!note.shows_whole_file());
    assert!(!note.shows_source());
    assert_eq!(note.object_id(), Some(ID));
    assert_eq!(note.text(), "# Plan\n");
    assert!(note.properties().is_some());
    assert!(!note.is_unsaved());
    assert_eq!(app.take_note_reset(tab).as_deref(), Some("# Plan\n"));
    assert_eq!(app.take_note_sync(tab), None);
    app.edit_note(tab, 0, "# Plan\n", at + SAVE_PAUSE);
    assert_eq!(app.next_note_save(), None);
    app.save_due_notes(at + Duration::from_secs(10));
    settle_saves(&mut app);

    assert_eq!(String::from_utf8(fixture.read("Plan.md")).unwrap(), fixed);
    assert_eq!(modified(&fixture, "Plan.md"), written, "written again");
}

#[test]
fn properties_without_an_id_leave_a_note_shown_whole_as_it_is() {
    let fixture = Fixture::new(&[("Plan.md", b"# Plan\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    app.take_note_reset(tab);
    let at = Instant::now();

    type_into(&mut app, tab, at, |text| {
        format!("---\ntags: [a]\n---\n{text}")
    });
    save_after_pause(&mut app, at);

    let note = app.note(tab).unwrap();
    assert!(note.shows_whole_file());
    assert!(!note.shows_source());
    assert_eq!(note.broken_properties(), None);
    assert_eq!(note.text(), "---\ntags: [a]\n---\n# Plan\n");
    assert_eq!(app.take_note_reset(tab), None);
}

/// A note opened with no type, whose typing adds the type and edits its
/// body, while another app edits the same body line: the conflict review
/// opens. Returns the tab.
fn conflict_over_a_fix(fixture: &Fixture, app: &mut App) -> TabId {
    let tab = open_from_tree(app, "Broken.md");
    let at = Instant::now();
    type_into(app, tab, at, |text| {
        text.replace("---\nBody", "type: note\n---\nBody, mine")
    });
    support::write(
        &fixture.root,
        "Broken.md",
        format!("---\nid: {ID}\ntype: note\n---\nBody, theirs\n").as_bytes(),
    );
    save_after_pause(app, at);
    assert!(app.note(tab).unwrap().conflict().is_some());
    tab
}

/// The note in `tab` opened as a note with `body` and nothing more to
/// write: the editor starts again from `body`, and a later save round
/// writes nothing.
fn opened_as_a_note_without_a_second_write(
    fixture: &Fixture,
    app: &mut App,
    tab: TabId,
    body: &str,
) {
    let written = modified(fixture, "Broken.md");
    let note = app.note(tab).unwrap();
    assert!(note.conflict().is_none());
    assert_eq!(note.broken_properties(), None);
    assert!(!note.shows_whole_file());
    assert!(!note.shows_source());
    assert_eq!(note.object_id(), Some(ID));
    assert!(note.properties().is_some());
    assert_eq!(note.text(), body);
    assert!(!note.is_unsaved());
    assert_eq!(app.take_note_reset(tab).as_deref(), Some(body));
    assert_eq!(app.take_note_sync(tab), None);
    let now = Instant::now();
    app.edit_note(tab, 0, body, now);
    assert_eq!(app.next_note_save(), None);
    app.save_due_notes(now + Duration::from_secs(10));
    settle_saves(app);
    assert_eq!(
        String::from_utf8(fixture.read("Broken.md")).unwrap(),
        format!("---\nid: {ID}\ntype: note\n---\n{body}")
    );
    assert_eq!(modified(fixture, "Broken.md"), written, "written again");
}

#[test]
fn use_my_version_with_fixed_properties_opens_the_note_without_a_second_write() {
    let fixture = Fixture::new(&[(
        "Broken.md",
        format!("---\nid: {ID}\n---\nBody\n").as_bytes(),
    )]);
    let mut app = fixture.app();
    let tab = conflict_over_a_fix(&fixture, &mut app);

    app.choose_version(tab, ConflictChoice::Mine);
    app.confirm_version(tab);

    opened_as_a_note_without_a_second_write(&fixture, &mut app, tab, "Body, mine\n");
}

#[test]
fn use_the_files_version_with_fixed_properties_opens_the_note_without_a_write() {
    let fixture = Fixture::new(&[(
        "Broken.md",
        format!("---\nid: {ID}\n---\nBody\n").as_bytes(),
    )]);
    let mut app = fixture.app();
    let tab = conflict_over_a_fix(&fixture, &mut app);
    let before = modified(&fixture, "Broken.md");

    app.choose_version(tab, ConflictChoice::File);
    app.confirm_version(tab);

    assert_eq!(modified(&fixture, "Broken.md"), before);
    opened_as_a_note_without_a_second_write(&fixture, &mut app, tab, "Body, theirs\n");
}
