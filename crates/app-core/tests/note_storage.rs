//! Seam 1: the AGENTS.md storage matrix for the note editor's saves:
//! malformed input, round trips, external edits, stale indexes,
//! interrupted index updates, moves, duplicate IDs, path traversal,
//! rebuilds and durable bytes. Every check reads the disk.

mod support;

use std::{path::PathBuf, time::Instant};

use app_core::{App, DropSpot, Platform, SAVE_PAUSE, TabId};
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

#[test]
fn malformed_frontmatter_opens_as_text_and_saves_only_the_edit() {
    let broken = b"---\nid: [unclosed\n---\nBody\r\nmore";
    let fixture = Fixture::new(&[("Broken.md", broken)]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Broken.md");
    assert_eq!(
        app.note(tab).unwrap().text(),
        "---\nid: [unclosed\n---\nBody\nmore"
    );
    assert!(app.note(tab).unwrap().properties().is_none());

    let at = type_into(&mut app, tab, |text| text.replace("more", "more text"));
    save(&mut app, at);

    assert_eq!(
        fixture.read("Broken.md"),
        b"---\nid: [unclosed\n---\nBody\r\nmore text"
    );
}

#[test]
fn edits_round_trip_through_saves_and_reopening() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = type_into(&mut app, tab, |text| format!("{text}typed\n"));
    save(&mut app, at);
    drop(app);

    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");

    assert_eq!(app.note(tab).unwrap().text(), "\nBody\ntyped\n");
    assert_eq!(fixture.read("Plan.md"), note(A, "Body\ntyped\n"));
}

#[test]
fn an_outside_edit_to_other_lines_merges_into_the_save() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\nthree\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = type_into(&mut app, tab, |text| text.replace("one", "one, typed"));
    support::write(
        &fixture.root,
        "Plan.md",
        &note(A, "one\ntwo\nthree, outside\n"),
    );

    save(&mut app, at);

    assert_eq!(
        fixture.read("Plan.md"),
        note(A, "one, typed\ntwo\nthree, outside\n")
    );
    let sync = app.take_note_sync(tab).expect("the editor catches up");
    assert_eq!(sync.saved.1, "\none, typed\ntwo\nthree\n");
    assert_eq!(
        sync.merged.as_deref(),
        Some("\none, typed\ntwo\nthree, outside\n")
    );
    assert!(!app.note(tab).unwrap().is_unsaved());
}

#[test]
fn typing_during_a_save_that_merges_an_outside_edit_keeps_both() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\nthree\nfour\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = type_into(&mut app, tab, |text| text.replace("one", "one, typed"));
    support::write(
        &fixture.root,
        "Plan.md",
        &note(A, "one\ntwo\nthree\nfour, outside\n"),
    );
    let lock = support::hold_writes(&fixture, &app);
    app.save_due_notes(at + SAVE_PAUSE);
    // More typing while the save waits for the disk.
    let later = type_into(&mut app, tab, |text| {
        text.replace("two", "two, typed later")
    });
    lock.unlock().unwrap();
    settle_saves(&mut app);

    // The note's next save keeps the outside edit, even before the editor
    // shows the merge.
    save(&mut app, later);

    assert_eq!(
        fixture.read("Plan.md"),
        note(A, "one, typed\ntwo, typed later\nthree\nfour, outside\n")
    );
}

#[test]
fn a_deleted_index_doesnt_stop_saves_and_rebuilds_from_the_files() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let files = index_files(&fixture, &app);
    for file in &files {
        let _ = std::fs::remove_file(file);
    }

    let at = type_into(&mut app, tab, |text| format!("{text}typed\n"));
    save(&mut app, at);
    drop(app);

    assert_eq!(fixture.read("Plan.md"), note(A, "Body\ntyped\n"));
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    assert_eq!(
        app.note(tab).unwrap().object_id(),
        Some(format!("note_{A}").as_str())
    );
}

#[test]
fn a_stale_index_never_decides_what_a_note_shows_or_saves() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    let app = fixture.app();
    let files = index_files(&fixture, &app);
    drop(app);
    let stale = tempfile::tempdir().unwrap();
    for file in &files {
        if file.exists() {
            std::fs::copy(file, stale.path().join(file.file_name().unwrap())).unwrap();
        }
    }
    // An edit outside Noura while it's closed, then the index from before.
    support::write(&fixture.root, "Plan.md", &note(A, "Body, edited outside\n"));
    for file in &files {
        let _ = std::fs::remove_file(file);
        let old = stale.path().join(file.file_name().unwrap());
        if old.exists() {
            std::fs::copy(old, file).unwrap();
        }
    }

    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    assert_eq!(app.note(tab).unwrap().text(), "\nBody, edited outside\n");
    let at = type_into(&mut app, tab, |text| format!("{text}typed\n"));
    save(&mut app, at);

    assert_eq!(
        fixture.read("Plan.md"),
        note(A, "Body, edited outside\ntyped\n")
    );
    assert!(app.note(tab).unwrap().save_problem().is_none());
}

#[test]
fn a_save_reaches_disk_when_the_index_refuses_writes() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let connection = rusqlite::Connection::open(&index_files(&fixture, &app)[0]).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_update BEFORE UPDATE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;",
        )
        .unwrap();

    let at = type_into(&mut app, tab, |text| format!("{text}typed\n"));
    save(&mut app, at);

    assert_eq!(fixture.read("Plan.md"), note(A, "Body\ntyped\n"));
    assert!(app.note(tab).unwrap().save_problem().is_none());
}

#[test]
fn moving_an_open_note_saves_its_typing_and_the_tab_follows() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n")), ("Docs/a.txt", b"a")]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    type_into(&mut app, tab, |text| format!("{text}typed\n"));

    app.move_into("Plan.md", "Docs");

    assert_eq!(fixture.read("Docs/Plan.md"), note(A, "Body\ntyped\n"));
    assert_eq!(app.note(tab).unwrap().relative_path(), "Docs/Plan.md");
    let at = type_into(&mut app, tab, |text| format!("{text}after\n"));
    save(&mut app, at);
    assert_eq!(
        fixture.read("Docs/Plan.md"),
        note(A, "Body\ntyped\nafter\n")
    );
    assert!(!fixture.exists("Plan.md"));
}

#[test]
fn notes_that_share_an_id_each_save_to_their_own_file() {
    let fixture = Fixture::new(&[("One.md", &note(A, "one\n")), ("Two.md", &note(A, "two\n"))]);
    let mut app = fixture.app();
    let one = open_from_tree(&mut app, "One.md");
    let at = type_into(&mut app, one, |text| format!("{text}typed one\n"));
    save(&mut app, at);
    app.keep_tab(one);
    let two = open_from_tree(&mut app, "Two.md");
    let at = type_into(&mut app, two, |text| format!("{text}typed two\n"));
    save(&mut app, at);

    assert_eq!(fixture.read("One.md"), note(A, "one\ntyped one\n"));
    assert_eq!(fixture.read("Two.md"), note(A, "two\ntyped two\n"));
}

#[test]
fn a_name_never_leaves_the_notes_folder() {
    let fixture = Fixture::new(&[("Docs/Plan.md", &note(A, "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Docs/Plan.md");

    app.rename_note(tab, "../../escape");

    assert_eq!(app.note(tab).unwrap().relative_path(), "Docs/escape.md");
    assert!(fixture.exists("Docs/escape.md"));
    assert!(!fixture.root.parent().unwrap().join("escape.md").exists());
}

#[test]
fn a_rebuilt_index_shows_the_saved_note_the_same_way() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = type_into(&mut app, tab, |text| format!("{text}typed\n"));
    save(&mut app, at);
    let properties = app.note(tab).unwrap().properties();
    let files = index_files(&fixture, &app);
    drop(app);
    for file in &files {
        let _ = std::fs::remove_file(file);
    }

    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");

    assert_eq!(app.note(tab).unwrap().properties(), properties);
    assert_eq!(app.note(tab).unwrap().text(), "\nBody\ntyped\n");
}

#[test]
fn a_save_before_a_drag_out_is_durable_before_the_drag_starts() {
    let fixture = Fixture::new(&[("Docs/Plan.md", &note(A, "Body\n"))]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let tab = open_from_tree(&mut app, "Docs/Plan.md");
    type_into(&mut app, tab, |text| format!("{text}typed\n"));
    let row = app.tree().unwrap().find_row("Docs/Plan.md").unwrap();

    let ((), synced) = local_core::durability_probe::record(|| {
        app.start_tree_drag(row);
        app.drag_tree_over(DropSpot::Away, Instant::now());
        app.drag_row_out();
    });

    assert!(app.take_drag_out().is_some());
    assert_eq!(fixture.read("Docs/Plan.md"), note(A, "Body\ntyped\n"));
    let docs = fixture.root.join("Docs").canonicalize().unwrap();
    assert!(
        synced
            .iter()
            .any(|folder| folder.canonicalize().ok().as_ref() == Some(&docs)),
        "{synced:?}"
    );
    assert!(!app.note(tab).unwrap().is_unsaved());
}
