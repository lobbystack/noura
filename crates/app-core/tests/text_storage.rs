//! Seam 1: the AGENTS.md storage matrix for the raw text path, widened from
//! Markdown to plain-text and code files: malformed input, round trips,
//! external edits, stale indexes, interrupted index updates, moves,
//! duplicate IDs, path traversal, rebuilds and durable bytes. Every check
//! reads the disk.

mod support;

use std::{path::PathBuf, time::Instant};

use app_core::{App, ConflictChoice, DropSpot, Platform, SAVE_PAUSE, TabId};
use support::{FakeSystem, Fixture, note, open_from_tree, settle_saves};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";

fn type_into(app: &mut App, tab: TabId, change: impl FnOnce(&str) -> String) -> Instant {
    let at = Instant::now();
    let text = change(app.note(tab).unwrap().text());
    app.edit_note(tab, 1, &text, at);
    at
}

fn save(app: &mut App, at: Instant) {
    app.save_due_notes(at + SAVE_PAUSE);
    settle_saves(app);
}

/// The workspace's index files in the app data folder.
fn index_files(fixture: &Fixture, app: &App) -> Vec<PathBuf> {
    let folder = fixture
        .app_data
        .path()
        .join("workspaces")
        .join(app.workspace().unwrap().id());
    ["index.sqlite", "index.sqlite-wal", "index.sqlite-shm"]
        .into_iter()
        .map(|name| folder.join(name))
        .collect()
}

/// The recovery snapshots in the workspace's history folder, as the kind
/// of version each keeps and its bytes.
fn snapshots(fixture: &Fixture) -> Vec<(String, Vec<u8>)> {
    let mut found = Vec::new();
    let history = fixture.root.join(".noura/history");
    for folder in std::fs::read_dir(history).into_iter().flatten() {
        for file in std::fs::read_dir(folder.unwrap().path()).unwrap() {
            let path = file.unwrap().path();
            let name = path.file_stem().unwrap().to_str().unwrap().to_owned();
            let kind = name.rsplit('-').next().unwrap().to_owned();
            found.push((kind, std::fs::read(path).unwrap()));
        }
    }
    found.sort();
    found
}

#[test]
fn a_text_file_with_a_bom_and_bytes_that_arent_utf8_is_never_written() {
    let bytes = b"\xEF\xBB\xBFkey = \"caf\xe9\"\r\n";
    let fixture = Fixture::new(&[("config.toml", bytes)]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "config.toml");

    assert!(app.note(tab).is_none());
    assert!(app.note_unreadable(tab).is_some());
    app.rescan();
    app.close_tab(tab);

    assert_eq!(fixture.read("config.toml"), bytes);
}

#[test]
fn edits_to_a_text_file_round_trip_through_saves_and_reopening() {
    let fixture = Fixture::new(&[("ci.yml", b"on:\r\n  push:\r\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "ci.yml");
    let at = type_into(&mut app, tab, |text| format!("{text}jobs: {{}}\n"));
    save(&mut app, at);
    drop(app);

    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "ci.yml");

    assert_eq!(app.note(tab).unwrap().text(), "on:\n  push:\njobs: {}\n");
    assert_eq!(fixture.read("ci.yml"), b"on:\r\n  push:\r\njobs: {}\r\n");
}

#[test]
fn an_outside_edit_to_other_lines_of_a_text_file_merges_into_the_save() {
    let fixture = Fixture::new(&[("list.txt", b"one\ntwo\nthree\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "list.txt");
    let at = type_into(&mut app, tab, |text| text.replace("one", "one, typed"));
    support::write(&fixture.root, "list.txt", b"one\ntwo\nthree, outside\n");

    save(&mut app, at);

    assert_eq!(
        fixture.read("list.txt"),
        b"one, typed\ntwo\nthree, outside\n"
    );
    let sync = app.take_note_sync(tab).expect("the editor catches up");
    assert_eq!(
        sync.merged.as_deref(),
        Some("one, typed\ntwo\nthree, outside\n")
    );
    assert!(!app.note(tab).unwrap().is_unsaved());
}

#[test]
fn an_outside_edit_to_a_text_file_with_nothing_unsaved_shows_in_the_editor() {
    let fixture = Fixture::new(&[("list.txt", b"one\ntwo\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "list.txt");

    support::write(&fixture.root, "list.txt", b"one\ntwo, outside\n");
    app.rescan();
    settle_saves(&mut app);

    assert_eq!(app.note(tab).unwrap().text(), "one\ntwo, outside\n");
    assert_eq!(
        app.take_note_reset(tab).as_deref(),
        Some("one\ntwo, outside\n")
    );
    assert_eq!(fixture.read("list.txt"), b"one\ntwo, outside\n");
}

#[test]
fn an_outside_edit_of_the_same_lines_opens_review_and_use_my_version_keeps_a_snapshot() {
    let fixture = Fixture::new(&[("list.txt", b"one\r\ntwo\r\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "list.txt");
    let at = type_into(&mut app, tab, |text| text.replace("one", "one, mine"));
    support::write(&fixture.root, "list.txt", b"one, theirs\r\ntwo\r\n");
    save(&mut app, at);

    let conflict = app.note(tab).unwrap().conflict().expect("review opens");
    assert_eq!(conflict.file_text(), "one, theirs\ntwo\n");
    assert_eq!(fixture.read("list.txt"), b"one, theirs\r\ntwo\r\n");

    app.choose_version(tab, ConflictChoice::Mine);
    app.confirm_version(tab);

    assert_eq!(fixture.read("list.txt"), b"one, mine\r\ntwo\r\n");
    assert_eq!(
        snapshots(&fixture),
        vec![("external".to_owned(), b"one, theirs\r\ntwo\r\n".to_vec())]
    );
}

#[test]
fn a_deleted_index_doesnt_stop_a_text_files_saves() {
    let fixture = Fixture::new(&[("notes.txt", b"one\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "notes.txt");
    for file in index_files(&fixture, &app) {
        let _ = std::fs::remove_file(file);
    }

    let at = type_into(&mut app, tab, |text| format!("{text}two\n"));
    save(&mut app, at);

    assert_eq!(fixture.read("notes.txt"), b"one\ntwo\n");
    assert!(app.note(tab).unwrap().save_problem().is_none());
}

#[test]
fn a_text_files_save_reaches_disk_when_the_index_refuses_writes() {
    let fixture = Fixture::new(&[("notes.txt", b"one\n"), ("Plan.md", &note(A, "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "notes.txt");
    let connection = rusqlite::Connection::open(&index_files(&fixture, &app)[0]).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_insert BEFORE INSERT ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;
             CREATE TRIGGER refuse_update BEFORE UPDATE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;",
        )
        .unwrap();

    let at = type_into(&mut app, tab, |text| format!("{text}two\n"));
    save(&mut app, at);

    assert_eq!(fixture.read("notes.txt"), b"one\ntwo\n");
    assert!(app.note(tab).unwrap().save_problem().is_none());
    assert!(!app.note(tab).unwrap().is_unsaved());
}

#[test]
fn moving_an_open_text_file_saves_its_typing_and_the_tab_follows() {
    let fixture = Fixture::new(&[("notes.txt", b"one\n"), ("Docs/a.md", b"a")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "notes.txt");
    type_into(&mut app, tab, |text| format!("{text}two\n"));

    app.move_into("notes.txt", "Docs");

    assert_eq!(fixture.read("Docs/notes.txt"), b"one\ntwo\n");
    assert!(!fixture.exists("notes.txt"));
    assert_eq!(app.note(tab).unwrap().relative_path(), "Docs/notes.txt");
    let at = type_into(&mut app, tab, |text| format!("{text}three\n"));
    save(&mut app, at);
    assert_eq!(fixture.read("Docs/notes.txt"), b"one\ntwo\nthree\n");
}

#[test]
fn a_text_file_holding_a_notes_id_never_takes_the_notes_place() {
    let copy = String::from_utf8(note(A, "Body\n")).unwrap();
    let fixture = Fixture::new(&[
        ("Plan.md", &note(A, "Body\n")),
        ("copy.yaml", copy.as_bytes()),
    ]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "copy.yaml");
    assert_eq!(app.note(tab).unwrap().object_id(), None);
    let at = type_into(&mut app, tab, |text| {
        text.replace("color: green", "color: red")
    });
    save(&mut app, at);
    app.keep_tab(tab);

    assert_eq!(
        fixture.read("copy.yaml"),
        copy.replace("color: green", "color: red").as_bytes()
    );
    assert_eq!(fixture.read("Plan.md"), note(A, "Body\n"));
    let plan = open_from_tree(&mut app, "Plan.md");
    assert_eq!(
        app.note(plan).unwrap().object_id(),
        Some(format!("note_{A}").as_str())
    );
}

#[test]
fn a_rebuilt_index_shows_the_saved_text_file_the_same_way() {
    let fixture = Fixture::new(&[("notes.txt", b"one\n")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "notes.txt");
    let at = type_into(&mut app, tab, |text| format!("{text}two\n"));
    save(&mut app, at);
    let files = index_files(&fixture, &app);
    drop(app);
    for file in &files {
        let _ = std::fs::remove_file(file);
    }

    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "notes.txt");

    assert_eq!(app.note(tab).unwrap().text(), "one\ntwo\n");
    assert!(app.note(tab).unwrap().shows_source());
}

#[test]
fn a_text_files_save_before_a_drag_out_is_durable_before_the_drag_starts() {
    let fixture = Fixture::new(&[("Docs/notes.txt", b"one\n")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let tab = open_from_tree(&mut app, "Docs/notes.txt");
    type_into(&mut app, tab, |text| format!("{text}two\n"));
    let row = app.tree().unwrap().find_row("Docs/notes.txt").unwrap();

    // The drag leaves the file where it is, so only the save syncs its
    // folder.
    let ((), synced) = local_core::durability_probe::record(|| {
        app.start_tree_drag(row);
        app.drag_tree_over(DropSpot::Away, Instant::now());
        app.drag_row_out();
    });

    assert!(app.take_drag_out().is_some());
    assert_eq!(fixture.read("Docs/notes.txt"), b"one\ntwo\n");
    let docs = fixture.root.join("Docs").canonicalize().unwrap();
    assert!(
        synced
            .iter()
            .any(|folder| folder.canonicalize().ok().as_ref() == Some(&docs)),
        "{synced:?}"
    );
    assert!(!app.note(tab).unwrap().is_unsaved());
}

/// Change the files with `change` while Noura is closed, then put back
/// the index from before the change.
fn with_a_stale_index(fixture: &Fixture, change: impl FnOnce()) {
    let app = fixture.app();
    let files = index_files(fixture, &app);
    drop(app);
    let stale = tempfile::tempdir().unwrap();
    for file in &files {
        if file.exists() {
            std::fs::copy(file, stale.path().join(file.file_name().unwrap())).unwrap();
        }
    }
    change();
    for file in &files {
        let _ = std::fs::remove_file(file);
        let old = stale.path().join(file.file_name().unwrap());
        if old.exists() {
            std::fs::copy(old, file).unwrap();
        }
    }
}

#[test]
fn a_stale_index_never_hides_a_notes_broken_properties() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    let broken = format!("---\nid: note_{A}\ncolor: green\n---\n\nBody\n");
    with_a_stale_index(&fixture, || {
        support::write(&fixture.root, "Plan.md", broken.as_bytes());
    });

    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let shown = app.note(tab).unwrap();
    assert_eq!(shown.text(), broken);
    assert!(shown.shows_source());
    assert_eq!(
        shown.broken_properties(),
        Some("The properties have an ID but no type.")
    );
    assert_eq!(shown.object_id(), None);
    assert_eq!(shown.properties(), None);
    let at = type_into(&mut app, tab, |text| format!("{text}typed\n"));
    save(&mut app, at);

    assert_eq!(
        String::from_utf8(fixture.read("Plan.md")).unwrap(),
        format!("{broken}typed\n")
    );
    assert!(app.note(tab).unwrap().save_problem().is_none());
}

#[test]
fn a_stale_index_never_makes_a_note_renamed_to_a_text_file_a_note() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    with_a_stale_index(&fixture, || {
        std::fs::rename(fixture.root.join("Plan.md"), fixture.root.join("Plan.txt")).unwrap();
    });

    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.txt");
    let shown = app.note(tab).unwrap();
    let text = String::from_utf8(note(A, "Body\n")).unwrap();
    assert_eq!(shown.text(), text);
    assert!(shown.shows_source());
    assert_eq!(shown.object_id(), None);
    assert_eq!(shown.properties(), None);
    assert_eq!(shown.broken_properties(), None);
    let at = type_into(&mut app, tab, |text| {
        text.replace("color: green", "color: red")
    });
    save(&mut app, at);

    assert_eq!(
        fixture.read("Plan.txt"),
        text.replace("color: green", "color: red").as_bytes()
    );
    assert!(app.note(tab).unwrap().save_problem().is_none());
    // The text file stays out of the index, and the note it held is gone
    // from it.
    let connection = rusqlite::Connection::open(&index_files(&fixture, &app)[0]).unwrap();
    let indexed: Vec<String> = connection
        .prepare("SELECT relative_path FROM files")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert!(
        !indexed.iter().any(|path| path.starts_with("Plan.")),
        "{indexed:?}"
    );
    let objects = app.workspace().unwrap().objects().unwrap();
    assert!(objects.is_empty(), "{objects:?}");
}
