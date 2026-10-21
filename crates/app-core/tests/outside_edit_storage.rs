//! Seam 1: the AGENTS.md storage matrix for the writes that meet another
//! app's edits of an open note: the merge, the recovery snapshot each
//! conflict choice keeps, and Restore. Malformed input, round trips,
//! external edits, stale indexes, interrupted index updates, moves,
//! duplicate IDs, path traversal and rebuilds; every check reads the disk
//! as soon as the call returns.

mod support;

use std::{path::PathBuf, time::Instant};

use app_core::{App, ConflictChoice, SAVE_PAUSE, TabId};
use support::{Fixture, note, object_id, open_from_tree, settle_saves};

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

/// Stop the index from changing its file list, through triggers added by
/// a second connection.
fn refuse_index_writes(fixture: &Fixture, app: &App) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(&index_files(fixture, app)[0]).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_insert BEFORE INSERT ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;
             CREATE TRIGGER refuse_update BEFORE UPDATE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;",
        )
        .unwrap();
    connection
}

/// Close `app` and delete every index file, so the next open rebuilds the
/// index from the files.
fn rebuild(fixture: &Fixture, app: App, connection: rusqlite::Connection) {
    let files = index_files(fixture, &app);
    drop(connection);
    drop(app);
    for file in &files {
        let _ = std::fs::remove_file(file);
    }
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

/// `relative` open with `typed` over its first line, and another app's
/// `theirs` over the same line, so the save finds a conflict.
fn conflict(fixture: &Fixture, relative: &str, theirs: &[u8]) -> (App, TabId) {
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, relative);
    let at = type_into(&mut app, tab, |text| text.replacen("one", "one, mine", 1));
    support::write(&fixture.root, relative, theirs);
    save(&mut app, at);
    assert!(app.note(tab).unwrap().conflict().is_some());
    (app, tab)
}

fn choose(app: &mut App, tab: TabId, choice: ConflictChoice) {
    app.choose_version(tab, choice);
    app.confirm_version(tab);
}

/// `relative` open with typing that another app's delete left unsaved.
fn deleted(fixture: &Fixture, relative: &str) -> (App, TabId) {
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, relative);
    type_into(&mut app, tab, |text| format!("{text}typed\n"));
    std::fs::remove_file(fixture.root.join(relative)).unwrap();
    app.rescan();
    settle_saves(&mut app);
    assert!(app.note(tab).unwrap().is_deleted());
    (app, tab)
}

#[test]
fn a_merge_keeps_frontmatter_another_app_broke_byte_for_byte() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\nthree\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    let at = type_into(&mut app, tab, |text| text.replace("one", "one, typed"));
    let broken = String::from_utf8(note(A, "one\ntwo\nthree, outside\n"))
        .unwrap()
        .replace("color: green", "color: [unclosed");
    support::write(&fixture.root, "Plan.md", broken.as_bytes());

    save(&mut app, at);

    let merged = broken.replace("\none\n", "\none, typed\n");
    assert_eq!(fixture.read("Plan.md"), merged.as_bytes());
    // The note now shows the file whole, and the next edit changes only
    // its own bytes.
    let at = type_into(&mut app, tab, |text| format!("{text}four\n"));
    save(&mut app, at);
    assert_eq!(
        fixture.read("Plan.md"),
        format!("{merged}four\n").as_bytes()
    );
}

#[test]
fn use_my_version_keeps_frontmatter_another_app_broke() {
    let base = note(A, "one\ntwo\n");
    let broken = String::from_utf8(note(A, "one, theirs\ntwo\n"))
        .unwrap()
        .replace("color: green", "color: [unclosed");
    let fixture = Fixture::new(&[("Plan.md", &base)]);
    let (mut app, tab) = conflict(&fixture, "Plan.md", broken.as_bytes());

    choose(&mut app, tab, ConflictChoice::Mine);

    assert_eq!(
        snapshots(&fixture),
        vec![("external".to_owned(), broken.clone().into_bytes())]
    );
    assert_eq!(
        fixture.read("Plan.md"),
        broken.replace("one, theirs", "one, mine").as_bytes()
    );
}

#[test]
fn each_snapshot_keeps_the_replaced_version_byte_for_byte_with_its_line_endings() {
    let crlf = |text: &str| {
        let mut bytes = b"\xEF\xBB\xBF".to_vec();
        bytes.extend(text.replace('\n', "\r\n").into_bytes());
        bytes
    };
    let fixture = Fixture::new(&[
        ("Mine.md", &crlf("one\ntwo\n")),
        ("Theirs.md", &crlf("one\ntwo\n")),
    ]);

    let (mut app, tab) = conflict(&fixture, "Mine.md", &crlf("one, theirs\ntwo\n"));
    choose(&mut app, tab, ConflictChoice::Mine);
    assert_eq!(
        snapshots(&fixture),
        vec![("external".to_owned(), crlf("one, theirs\ntwo\n"))]
    );
    assert_eq!(fixture.read("Mine.md"), crlf("one, mine\ntwo\n"));
    drop(app);

    let (mut app, tab) = conflict(&fixture, "Theirs.md", &crlf("one, theirs\ntwo\n"));
    choose(&mut app, tab, ConflictChoice::File);
    assert!(
        snapshots(&fixture).contains(&("local".to_owned(), crlf("one, mine\ntwo\n"))),
        "{:?}",
        snapshots(&fixture)
    );
    assert_eq!(fixture.read("Theirs.md"), crlf("one, theirs\ntwo\n"));
}

#[test]
fn a_choice_reaches_disk_when_the_index_refuses_writes_and_a_rebuild_shows_it() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\n"))]);
    let (mut app, tab) = conflict(&fixture, "Plan.md", &note(A, "one, theirs\ntwo\n"));
    let connection = refuse_index_writes(&fixture, &app);

    choose(&mut app, tab, ConflictChoice::Mine);

    assert_eq!(fixture.read("Plan.md"), note(A, "one, mine\ntwo\n"));
    assert!(app.note(tab).unwrap().conflict().is_none());
    rebuild(&fixture, app, connection);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    assert_eq!(app.note(tab).unwrap().text(), "\none, mine\ntwo\n");
    assert_eq!(
        app.note(tab).unwrap().object_id(),
        Some(format!("note_{A}").as_str())
    );
}

#[test]
fn restore_reaches_disk_when_the_index_refuses_writes_and_a_rebuild_keeps_the_id() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\n"))]);
    let (mut app, tab) = deleted(&fixture, "Plan.md");
    let connection = refuse_index_writes(&fixture, &app);

    app.restore_note(tab);

    assert_eq!(fixture.read("Plan.md"), note(A, "one\ntyped\n"));
    assert!(!app.note(tab).unwrap().is_deleted());
    rebuild(&fixture, app, connection);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Plan.md");
    assert_eq!(
        app.note(tab).unwrap().object_id(),
        Some(format!("note_{A}").as_str())
    );
}

#[test]
fn a_choice_after_another_app_moved_the_note_writes_it_at_its_new_path() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "one\ntwo\n"))]);
    let (mut app, tab) = conflict(&fixture, "Plan.md", &note(A, "one, theirs\ntwo\n"));
    std::fs::create_dir(fixture.root.join("Archive")).unwrap();
    std::fs::rename(
        fixture.root.join("Plan.md"),
        fixture.root.join("Archive/Plan.md"),
    )
    .unwrap();
    app.rescan();
    settle_saves(&mut app);
    assert_eq!(app.note(tab).unwrap().relative_path(), "Archive/Plan.md");

    choose(&mut app, tab, ConflictChoice::Mine);

    assert_eq!(fixture.read("Archive/Plan.md"), note(A, "one, mine\ntwo\n"));
    assert!(!fixture.exists("Plan.md"));
    assert_eq!(
        snapshots(&fixture),
        vec![("external".to_owned(), note(A, "one, theirs\ntwo\n"))]
    );
}

#[test]
fn notes_that_share_an_id_each_merge_and_resolve_in_their_own_file() {
    let fixture = Fixture::new(&[
        ("One.md", &note(A, "one\ntwo\nthree\n")),
        ("Two.md", &note(A, "one\ntwo\nthree\n")),
    ]);
    let (mut app, one) = conflict(&fixture, "One.md", &note(A, "one, theirs\ntwo\nthree\n"));
    choose(&mut app, one, ConflictChoice::Mine);
    assert_eq!(fixture.read("One.md"), note(A, "one, mine\ntwo\nthree\n"));
    assert_eq!(fixture.read("Two.md"), note(A, "one\ntwo\nthree\n"));

    app.keep_tab(one);
    let two = open_from_tree(&mut app, "Two.md");
    let at = type_into(&mut app, two, |text| text.replace("one", "one, typed"));
    support::write(
        &fixture.root,
        "Two.md",
        &note(A, "one\ntwo\nthree, outside\n"),
    );
    save(&mut app, at);

    assert_eq!(
        fixture.read("Two.md"),
        note(A, "one, typed\ntwo\nthree, outside\n")
    );
    assert_eq!(fixture.read("One.md"), note(A, "one, mine\ntwo\nthree\n"));
    assert_eq!(object_id(&fixture.read("Two.md")), format!("note_{A}"));
}

/// Where each synced folder sits in `synced`, compared after resolving
/// links, as macOS puts temporary folders behind one.
fn synced_at(synced: &[PathBuf], folder: &std::path::Path) -> Option<usize> {
    let folder = folder.canonicalize().unwrap();
    synced
        .iter()
        .position(|synced| synced.canonicalize().ok().as_ref() == Some(&folder))
}

#[test]
fn restore_is_durable_with_the_folders_it_creates_before_it_returns() {
    let fixture = Fixture::new(&[("Docs/Plan.md", &note(A, "one\n"))]);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Docs/Plan.md");
    type_into(&mut app, tab, |text| format!("{text}typed\n"));
    std::fs::remove_dir_all(fixture.root.join("Docs")).unwrap();
    app.rescan();
    settle_saves(&mut app);

    let ((), synced) = local_core::durability_probe::record(|| app.restore_note(tab));

    assert_eq!(fixture.read("Docs/Plan.md"), note(A, "one\ntyped\n"));
    assert!(synced_at(&synced, &fixture.root).is_some(), "{synced:?}");
    assert!(
        synced_at(&synced, &fixture.root.join("Docs")).is_some(),
        "{synced:?}"
    );
}

#[test]
fn a_choice_makes_its_snapshot_durable_before_it_writes_the_note() {
    let fixture = Fixture::new(&[("Docs/Plan.md", &note(A, "one\ntwo\n"))]);
    let (mut app, tab) = conflict(&fixture, "Docs/Plan.md", &note(A, "one, theirs\ntwo\n"));

    let ((), synced) =
        local_core::durability_probe::record(|| choose(&mut app, tab, ConflictChoice::Mine));

    assert_eq!(fixture.read("Docs/Plan.md"), note(A, "one, mine\ntwo\n"));
    let history = std::fs::read_dir(fixture.root.join(".noura/history"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let snapshot = synced_at(&synced, &history).expect("the snapshot's folder syncs");
    let note = synced_at(&synced, &fixture.root.join("Docs")).expect("the note's folder syncs");
    assert!(snapshot < note, "{synced:?}");
}
