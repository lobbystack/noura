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

#[test]
fn a_text_file_that_isnt_utf8_says_noura_cant_read_it_and_stays_untouched() {
    let bytes = b"caf\xe9 au lait\r\n";
    let fixture = Fixture::new(&[("latin1.txt", bytes)]);
    let mut app = fixture.app();
    let before = modified(&fixture, "latin1.txt");

    let tab = open_from_tree(&mut app, "latin1.txt");

    assert!(app.note(tab).is_none());
    assert_eq!(
        app.note_unreadable(tab).unwrap().message,
        "Noura can't read this file as text because it isn't UTF-8."
    );
    app.retry_note(tab);
    assert!(app.note_unreadable(tab).is_some());
    app.run_command("save", CommandSource::Key, Instant::now());
    app.save_due_notes(Instant::now() + Duration::from_secs(10));
    settle_saves(&mut app);
    app.close_tab(tab);
    assert!(app.tabs().iter().all(|open| open.id() != tab));
    assert_eq!(fixture.read("latin1.txt"), bytes);
    assert_eq!(modified(&fixture, "latin1.txt"), before);
}

#[test]
fn opening_showing_and_closing_an_unchanged_text_file_writes_nothing() {
    let files: [(&str, &[u8]); 6] = [
        ("crlf.txt", b"one\r\ntwo\r\n"),
        ("mixed.json", b"{\r\n  \"a\": 1,\n  \"b\": 2\r\n}"),
        ("bom.csv", b"\xEF\xBB\xBFname,value\r\nx,1\n"),
        ("lone-cr.log", b"start\rmiddle\r\nend"),
        ("no-newline.rs", b"fn main() {}"),
        ("empty.toml", b""),
    ];
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();
    for (path, bytes) in files {
        let before = modified(&fixture, path);
        let tab = open_from_tree(&mut app, path);
        // The editor reports its text every frame it changes; unchanged
        // text isn't an edit.
        let text = app.note(tab).unwrap().text().to_owned();
        let now = Instant::now();
        app.edit_note(tab, 0, &text, now);
        app.save_due_notes(now + Duration::from_secs(10));
        settle_saves(&mut app);
        assert!(!app.note(tab).unwrap().is_unsaved(), "{path}");
        app.close_tab(tab);
        assert!(app.tabs().iter().all(|open| open.id() != tab), "{path}");
        assert_eq!(fixture.read(path), bytes, "{path}");
        assert_eq!(modified(&fixture, path), before, "{path} wasn't written");
    }
}

#[test]
fn an_edit_changes_only_the_bytes_it_touched_in_a_text_file() {
    // Each file, its bytes, the text replaced and its replacement, and the
    // bytes expected after the save.
    let cases: [(&str, &[u8], &str, &str, &[u8]); 4] = [
        (
            "crlf.txt",
            b"one\r\ntwo\r\nthree\r\n",
            "two\n",
            "2\nnew\n",
            b"one\r\n2\r\nnew\r\nthree\r\n",
        ),
        (
            // Mostly LF: the edited CRLF line keeps CRLF, and the new line
            // takes LF.
            "mixed.yaml",
            b"a: 1\nb: 2\r\nc: 3\nd: 4\n",
            "b: 2\n",
            "b: two\nadded: yes\n",
            b"a: 1\nb: two\r\nadded: yes\nc: 3\nd: 4\n",
        ),
        (
            "bom.py",
            b"\xEF\xBB\xBFprint(1)\r\nprint(2)",
            "print(2)",
            "print(2, 3)",
            b"\xEF\xBB\xBFprint(1)\r\nprint(2, 3)",
        ),
        (
            "Makefile",
            b"all:\n\tcargo build\n",
            "build",
            "test",
            b"all:\n\tcargo test\n",
        ),
    ];
    let files: Vec<(&str, &[u8])> = cases.iter().map(|case| (case.0, case.1)).collect();
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();
    for (path, _, from, to, expected) in cases {
        let tab = open_from_tree(&mut app, path);
        let at = Instant::now();
        type_into(&mut app, tab, at, |text| text.replacen(from, to, 1));
        save_after_pause(&mut app, at);
        assert_eq!(
            String::from_utf8_lossy(&fixture.read(path)),
            String::from_utf8_lossy(expected),
            "{path}"
        );
        assert!(!app.note(tab).unwrap().is_unsaved(), "{path}");
    }
}

#[test]
fn a_text_file_saves_once_typing_pauses_for_300_milliseconds() {
    let fixture = Fixture::new(&[("notes.txt", b"one\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "notes.txt");
    let at = Instant::now();

    type_into(&mut app, tab, at, |text| format!("{text}two\n"));
    assert_eq!(app.next_note_save(), Some(at + SAVE_PAUSE));
    app.save_due_notes(at + Duration::from_millis(299));

    assert!(!app.is_saving_notes());
    assert_eq!(fixture.read("notes.txt"), b"one\n");
    app.save_due_notes(at + Duration::from_millis(300));
    assert!(app.is_saving_notes());
    settle_saves(&mut app);
    assert_eq!(fixture.read("notes.txt"), b"one\ntwo\n");
}

#[test]
fn continuous_typing_in_a_text_file_saves_at_least_every_two_seconds() {
    let fixture = Fixture::new(&[("notes.txt", b"")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "notes.txt");
    let start = Instant::now();

    // A key every 200 milliseconds never pauses long enough.
    let mut saved_at = None;
    for step in 0..15u32 {
        let at = start + Duration::from_millis(200) * step;
        type_into(&mut app, tab, at, |text| format!("{text}x"));
        app.save_due_notes(at);
        if app.is_saving_notes() {
            saved_at = Some(at - start);
            break;
        }
    }

    assert_eq!(saved_at, Some(Duration::from_secs(2)));
    settle_saves(&mut app);
    assert_eq!(fixture.read("notes.txt"), b"xxxxxxxxxxx");
}

#[test]
fn mod_s_saves_a_text_file_at_once() {
    let fixture = Fixture::new(&[("data.json", b"{}\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "data.json");
    let at = Instant::now();
    type_into(&mut app, tab, at, |_| "{\"a\": 1}\n".to_owned());

    assert!(app.run_command("save", CommandSource::Key, at));

    settle_saves(&mut app);
    assert_eq!(fixture.read("data.json"), b"{\"a\": 1}\n");
}

#[test]
fn closing_a_text_file_waits_for_its_durable_save() {
    let fixture = Fixture::new(&[("notes.txt", b"one\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "notes.txt");
    let at = Instant::now();
    type_into(&mut app, tab, at, |text| format!("{text}two\n"));

    let lock = hold_writes(&fixture, &app);
    app.save_due_notes(at + SAVE_PAUSE);
    std::thread::sleep(Duration::from_millis(300));
    app.poll();
    assert!(app.note(tab).unwrap().is_unsaved());
    app.close_tab(tab);
    assert!(
        app.tabs().iter().any(|open| open.id() == tab),
        "the tab waits for its save"
    );

    lock.unlock().unwrap();
    settle_saves(&mut app);

    assert_eq!(fixture.read("notes.txt"), b"one\ntwo\n");
    assert!(app.tabs().iter().all(|open| open.id() != tab));
}

#[test]
fn a_failed_save_of_a_text_file_says_why_and_closing_asks_first() {
    let fixture = Fixture::new(&[("config.toml", b"a = 1\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "config.toml");
    // Another program writes text that isn't UTF-8 over the file.
    support::write(&fixture.root, "config.toml", b"a = \"caf\xe9\"\n");
    let at = Instant::now();

    type_into(&mut app, tab, at, |text| format!("{text}b = 2\n"));
    save_after_pause(&mut app, at);

    let note = app.note(tab).unwrap();
    assert_eq!(
        note.save_problem().unwrap().reason,
        "The file on disk isn't UTF-8 text Noura can read anymore."
    );
    assert!(note.is_unsaved());
    assert_eq!(note.text(), "a = 1\nb = 2\n");
    assert_eq!(fixture.read("config.toml"), b"a = \"caf\xe9\"\n");

    app.close_tab(tab);
    settle_saves(&mut app);
    let prompt = app.unsaved_prompt().expect("Noura asks before closing");
    assert_eq!(prompt.title, "Couldn\u{2019}t save \u{201c}config\u{201d}");
    assert!(app.tabs().iter().any(|open| open.id() == tab));
    app.discard_and_leave();
    assert!(app.tabs().iter().all(|open| open.id() != tab));
    assert_eq!(fixture.read("config.toml"), b"a = \"caf\xe9\"\n");
}

#[test]
fn the_name_field_shows_a_text_files_name_without_its_extension() {
    let fixture = Fixture::new(&[
        ("Docs/config.json", b"{}"),
        ("Docs/archive.test.ts", b""),
        ("Makefile", b""),
    ]);
    let mut app = fixture.app();

    for (path, name) in [
        ("Docs/config.json", "config"),
        ("Docs/archive.test.ts", "archive.test"),
        ("Makefile", "Makefile"),
    ] {
        let tab = open_from_tree(&mut app, path);
        assert_eq!(app.note(tab).unwrap().name(), name, "{path}");
    }
}

#[test]
fn the_name_field_renames_a_text_file_and_keeps_its_extension() {
    let fixture = Fixture::new(&[("Docs/config.json", b"{}"), ("Makefile", b"all:\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Docs/config.json");

    app.rename_note(tab, "  ..New/settings: v2?  ");

    assert!(!fixture.exists("Docs/config.json"));
    assert_eq!(fixture.read("Docs/Newsettings v2.json"), b"{}");
    let note = app.note(tab).unwrap();
    assert_eq!(note.relative_path(), "Docs/Newsettings v2.json");
    assert_eq!(note.name(), "Newsettings v2");
    assert!(note.name_problem().is_none());

    // A name with no extension gets none.
    let tab = open_from_tree(&mut app, "Makefile");
    app.rename_note(tab, "Dockerfile");
    assert_eq!(app.note(tab).unwrap().relative_path(), "Dockerfile");
    assert_eq!(fixture.read("Dockerfile"), b"all:\n");
}

#[test]
fn a_rename_off_the_text_file_list_leaves_the_editor() {
    let fixture = Fixture::new(&[("Makefile", b"all:\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Makefile");
    type_into(&mut app, tab, Instant::now(), |text| {
        format!("{text}\tmake\n")
    });

    app.rename_note(tab, "GNUmakefile");

    assert_eq!(fixture.read("GNUmakefile"), b"all:\n\tmake\n");
    let open = app.tabs().iter().find(|open| open.id() == tab).unwrap();
    assert_eq!(
        open.file().map(|file| file.relative_path.as_str()),
        Some("GNUmakefile")
    );
    assert!(app.note(tab).is_none());
}

#[test]
fn a_text_files_name_follows_the_note_naming_rules() {
    let fixture = Fixture::new(&[("config.json", b"{}"), ("Taken.json", b"other")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "config.json");

    for typed in ["", "   ", " / ", "config", " config "] {
        app.rename_note(tab, typed);
        assert_eq!(app.note(tab).unwrap().relative_path(), "config.json");
        assert!(app.note(tab).unwrap().name_problem().is_none(), "{typed:?}");
    }

    app.rename_note(tab, "taken");
    assert_eq!(fixture.read("Taken.json"), b"other");
    assert!(fixture.exists("config.json"));
    assert_eq!(
        app.note(tab).unwrap().name_problem(),
        Some("A file with that name already exists in this folder.")
    );

    app.rename_note(tab, &"n".repeat(250));
    assert_eq!(
        app.note(tab).unwrap().relative_path(),
        format!("{}.json", "n".repeat(200))
    );

    app.rename_note(tab, "CON");
    assert_eq!(app.note(tab).unwrap().relative_path(), "CON_.json");
}

#[test]
fn renaming_a_text_file_saves_its_typing_first() {
    let fixture = Fixture::new(&[("notes.txt", b"one\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "notes.txt");
    type_into(&mut app, tab, Instant::now(), |text| format!("{text}two\n"));

    app.rename_note(tab, "renamed");

    assert_eq!(fixture.read("renamed.txt"), b"one\ntwo\n");
    assert!(!app.note(tab).unwrap().is_unsaved());
}

#[test]
fn a_name_never_moves_a_text_file_out_of_its_folder() {
    let fixture = Fixture::new(&[("Docs/notes.txt", b"one\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Docs/notes.txt");

    app.rename_note(tab, "../../escape");

    assert_eq!(app.note(tab).unwrap().relative_path(), "Docs/escape.txt");
    assert!(!fixture.root.parent().unwrap().join("escape.txt").exists());
}
