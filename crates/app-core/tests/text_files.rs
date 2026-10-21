//! Seam 1: plain-text and code files open in the editor as source, with the
//! note editor's autosave, failed-save notice, close guard and byte
//! preservation, and a name field that keeps the file's extension.

mod support;

use std::time::{Duration, Instant};

use app_core::{App, CommandSource, SAVE_PAUSE, TabId};
use support::{Fixture, hold_writes, open_from_tree, settle_saves};

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

/// Text that the live preview would draw as Markdown.
const MARKDOWN_LOOKING: &str = "# not a heading\n- not a list\n**not bold**\n";

#[test]
fn plain_text_and_code_files_open_in_the_editor_as_source() {
    let names = [
        "notes.txt",
        "data.json",
        "Cargo.toml",
        "config.yaml",
        "ci.yml",
        "main.rs",
        "index.ts",
        "script.py",
        "run.sh",
        "Makefile",
        "Dockerfile",
        "LICENSE",
        "NOTICE",
        "SHOUTING.TXT",
        "Code/page.html",
    ];
    let files: Vec<(&str, &[u8])> = names
        .iter()
        .map(|name| (*name, MARKDOWN_LOOKING.as_bytes()))
        .collect();
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();

    for name in names {
        let tab = open_from_tree(&mut app, name);
        let note = app
            .note(tab)
            .unwrap_or_else(|| panic!("{name} opens in the editor"));
        assert!(note.shows_source(), "{name} shows as source");
        assert_eq!(note.text(), MARKDOWN_LOOKING, "{name}");
        assert_eq!(note.properties(), None, "{name}");
    }
}

#[test]
fn a_text_file_that_looks_like_a_note_still_opens_whole_as_source() {
    let note_like = "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\n---\nBody\n";
    let broken_like = "---\nkey: [unclosed\n";
    let fixture = Fixture::new(&[
        ("front.yaml", note_like.as_bytes()),
        ("open.yaml", broken_like.as_bytes()),
    ]);
    let mut app = fixture.app();

    for (name, text) in [("front.yaml", note_like), ("open.yaml", broken_like)] {
        let tab = open_from_tree(&mut app, name);
        let note = app.note(tab).unwrap();
        assert_eq!(note.text(), text, "{name}");
        assert!(note.shows_source(), "{name}");
        assert_eq!(note.object_id(), None, "{name}");
        assert_eq!(note.properties(), None, "{name}");
    }
}

#[test]
fn other_files_stay_out_of_the_editor() {
    let fixture = Fixture::new(&[("photo.png", b"\x89PNG\r\n"), ("archive.zip", b"PK")]);
    let mut app = fixture.app();

    for name in ["photo.png", "archive.zip"] {
        let tab = open_from_tree(&mut app, name);
        assert!(app.note(tab).is_none(), "{name}");
        assert!(app.note_unreadable(tab).is_none(), "{name}");
    }
}
