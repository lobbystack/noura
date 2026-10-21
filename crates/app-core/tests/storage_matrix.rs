//! Seam 1: the AGENTS.md storage matrix for the file tree's changes:
//! malformed input, round trips, external edits, stale indexes,
//! interrupted index updates, moves, duplicate IDs, path traversal and
//! rebuilds. Every check reads the disk as soon as the call returns.

mod support;

use std::path::{Path, PathBuf};

use app_core::App;
use support::{FakeSystem, Fixture, document, note, notice, object_id, rows};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";
const B: &str = "01jbcdefghjkmnpqrstvwxyz0b";

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

fn copy_present(files: &[PathBuf], to: &Path) {
    for file in files {
        if file.exists() {
            std::fs::copy(file, to.join(file.file_name().unwrap())).unwrap();
        }
    }
}

fn remove_present(files: &[PathBuf]) {
    for file in files {
        if file.exists() {
            std::fs::remove_file(file).unwrap();
        }
    }
}

/// Stop the index from changing its file list, as a full disk or a broken
/// index would, through triggers added by a second connection. Dropping
/// the triggers lets writes through again.
fn refuse_index_writes(fixture: &Fixture, app: &App) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(&index_files(fixture, app)[0]).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_insert BEFORE INSERT ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;
             CREATE TRIGGER refuse_update BEFORE UPDATE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;
             CREATE TRIGGER refuse_delete BEFORE DELETE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;",
        )
        .unwrap();
    connection
}

fn accept_index_writes(connection: &rusqlite::Connection) {
    connection
        .execute_batch(
            "DROP TRIGGER refuse_insert; DROP TRIGGER refuse_update; DROP TRIGGER refuse_delete;",
        )
        .unwrap();
}

/// The Markdown files the index lists.
fn indexed_markdown(connection: &rusqlite::Connection) -> Vec<String> {
    let mut statement = connection
        .prepare("SELECT relative_path FROM files WHERE relative_path LIKE '%.md' ORDER BY 1")
        .unwrap();
    statement
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

/// Every Markdown file's path and stable ID, for files that have one.
fn ids(fixture: &Fixture) -> Vec<(String, String)> {
    fixture
        .snapshot()
        .into_iter()
        .filter_map(|(path, bytes)| {
            let bytes = bytes?;
            match local_core::parse_markdown(&path, &bytes) {
                local_core::ParsedMarkdown::Managed(object) => Some((path, object.id)),
                _ => None,
            }
        })
        .collect()
}

#[test]
fn malformed_files_rename_move_and_copy_byte_for_byte() {
    let broken = b"---\nid: note_x\ntype: [unclosed\n---\nbody";
    let invalid = b"\xff\xfe not UTF-8";
    let fixture = Fixture::new(&[
        ("Broken.md", broken),
        ("Bytes.md", invalid),
        ("Docs/a.txt", b"a"),
    ]);
    let mut app = fixture.app();

    app.start_rename("Broken.md");
    assert!(app.commit_rename("Still broken"));
    app.move_into("Bytes.md", "Docs");
    app.duplicate("Still broken.md");
    app.duplicate("Docs/Bytes.md");

    assert!(app.notice().is_none(), "{:?}", app.notice());
    assert_eq!(fixture.read("Still broken.md"), broken);
    assert_eq!(fixture.read("Still broken 1.md"), broken);
    assert_eq!(fixture.read("Docs/Bytes.md"), invalid);
    assert_eq!(fixture.read("Docs/Bytes 1.md"), invalid);
}

#[test]
fn a_malformed_manifest_lists_no_patterns_and_saves_nothing() {
    let fixture = Fixture::new(&[("Readme.md", b"r")]);
    let mut app = fixture.app();
    let manifest = fixture.root.join(".noura/workspace.yaml");
    std::fs::write(&manifest, b"id: [unclosed\n").unwrap();

    app.open_ignored_files();
    let page = app.ignored_files().unwrap();
    assert!(page.patterns.is_empty());
    assert!(page.problem.is_some());
    assert!(!app.add_ignored("build/"));

    assert_eq!(std::fs::read(&manifest).unwrap(), b"id: [unclosed\n");
}

#[test]
fn renaming_and_moving_there_and_back_restores_every_byte() {
    let bytes = note(A, "Body\n");
    let fixture = Fixture::new(&[("Plan.md", &bytes), ("Docs/Guide.md", b"g")]);
    let mut app = fixture.app();
    let before = fixture.snapshot();

    app.start_rename("Plan.md");
    assert!(app.commit_rename("Roadmap"));
    app.move_into("Roadmap.md", "Docs");
    app.start_rename("Docs");
    assert!(app.commit_rename("Guides"));
    app.move_into("Guides/Roadmap.md", "");
    app.start_rename("Guides");
    assert!(app.commit_rename("Docs"));
    app.start_rename("Roadmap.md");
    assert!(app.commit_rename("Plan"));

    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn changes_keep_edits_made_outside_noura() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Old\n")), ("Docs/a.txt", b"a")]);
    let mut app = fixture.app();
    let edited = note(A, "Edited in another app\n");
    std::fs::write(fixture.root.join("Plan.md"), &edited).unwrap();
    std::fs::write(fixture.root.join("Untitled.md"), b"made elsewhere").unwrap();

    app.start_rename("Plan.md");
    assert!(app.commit_rename("Roadmap"));
    app.new_note(Some(""));
    app.duplicate("Roadmap.md");

    assert_eq!(fixture.read("Roadmap.md"), edited);
    assert_eq!(fixture.read("Untitled.md"), b"made elsewhere");
    assert!(fixture.exists("Untitled 1.md"));
    let copy = fixture.read("Roadmap 1.md");
    assert_ne!(object_id(&copy), format!("note_{A}"));
    assert!(
        String::from_utf8(copy)
            .unwrap()
            .contains("Edited in another app")
    );
}

#[test]
fn an_item_moved_outside_noura_says_it_left_and_works_at_its_new_path() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n")), ("Docs/a.txt", b"a")]);
    let mut app = fixture.app();
    // The index and the tree still say Plan.md.
    std::fs::rename(
        fixture.root.join("Plan.md"),
        fixture.root.join("Docs/Moved.md"),
    )
    .unwrap();

    app.duplicate("Plan.md");
    assert_eq!(
        app.notice().unwrap().message,
        "Plan.md isn't in the workspace anymore."
    );
    // The tree reads the files again, so the row it showed is gone.
    assert_eq!(rows(&app), ["Docs"]);
    app.duplicate("Docs/Moved.md");
    app.start_rename("Docs/Moved.md");
    assert!(app.commit_rename("Renamed"));

    assert_eq!(
        object_id(&fixture.read("Docs/Renamed.md")),
        format!("note_{A}")
    );
    let copy = object_id(&fixture.read("Docs/Moved 1.md"));
    assert_ne!(copy, format!("note_{A}"));
    assert!(!fixture.exists("Plan.md"));
}

#[test]
fn a_stale_index_never_gives_a_copy_an_id_in_use() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    let mut app = fixture.app();
    // Another app writes a second note with B, which the index hasn't seen.
    std::fs::write(fixture.root.join("Other.md"), note(B, "Other\n")).unwrap();

    app.duplicate("Other.md");
    app.duplicate("Plan.md");

    let found = ids(&fixture);
    let mut unique: Vec<&String> = found.iter().map(|(_, id)| id).collect();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), 4, "{found:?}");
}

#[test]
fn files_that_share_an_id_stay_as_they_are_and_their_copies_get_new_ids() {
    let shared = note(A, "Same ID\n");
    let fixture = Fixture::new(&[("One.md", &shared), ("Two.md", &shared)]);
    let mut app = fixture.app();

    app.duplicate("One.md");
    app.duplicate("Two.md");
    app.start_rename("Two.md");
    assert!(app.commit_rename("Second"));
    app.move_into("One.md", "");

    assert_eq!(fixture.read("One.md"), shared);
    assert_eq!(fixture.read("Second.md"), shared);
    let first = object_id(&fixture.read("One 1.md"));
    let second = object_id(&fixture.read("Two 1.md"));
    assert_ne!(first, format!("note_{A}"));
    assert_ne!(second, format!("note_{A}"));
    assert_ne!(first, second);
}

#[test]
fn every_change_refuses_traversal_absolute_paths_and_links() {
    let fixture = Fixture::new(&[("Plan.md", b"p"), ("Docs/a.txt", b"a")]);
    let outside = fixture.outside();
    std::fs::write(outside.join("Secret.md"), b"secret").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(&outside, fixture.root.join("Linked")).unwrap();
        std::os::unix::fs::symlink(outside.join("Secret.md"), fixture.root.join("Link.md"))
            .unwrap();
    }
    let absolute = outside.join("Secret.md");
    let absolute = absolute.to_str().unwrap();
    let mut app = fixture.app();
    let before = fixture.snapshot();

    let paths = [
        "..",
        "../Outside/Secret.md",
        "Docs/../../Outside/Secret.md",
        absolute,
        "Linked",
        "Linked/Secret.md",
        "Link.md",
    ];
    for path in paths {
        app.start_rename(path);
        app.commit_rename("Taken over");
        app.cancel_rename();
        app.move_into(path, "Docs");
        app.move_into("Plan.md", path);
        app.duplicate(path);
        app.trash(path);
        app.new_note(Some(path));
        app.new_folder(Some(path));
        app.cancel_rename();
        app.reveal(path);
        app.open_terminal(path);
        app.copy_path(path);
        assert!(app.notice().is_some(), "{path}");
        app.dismiss_notice();
    }

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(std::fs::read(outside.join("Secret.md")).unwrap(), b"secret");
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
}

#[test]
fn deleting_the_index_after_changes_rebuilds_the_same_tree_and_ids() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n")), ("Docs/Guide.md", b"g")]);
    let mut app = fixture.app();
    app.new_note(Some("Docs"));
    app.duplicate("Plan.md");
    app.start_rename("Plan.md");
    assert!(app.commit_rename("Roadmap"));
    app.move_into("Roadmap.md", "Docs");
    app.trash("Docs/Guide.md");
    support::select(&mut app, "Docs/Roadmap.md");
    let tree = rows(&app);
    let before = ids(&fixture);
    let index = index_files(&fixture, &app);
    drop(app);

    remove_present(&index);
    let mut app = fixture.app();
    support::select(&mut app, "Docs/Roadmap.md");

    assert_eq!(rows(&app), tree);
    assert_eq!(ids(&fixture), before);
    assert!(index[0].exists());
}

#[test]
fn an_index_update_lost_after_the_file_change_heals_on_the_next_open() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n")), ("Docs/Guide.md", b"g")]);
    let app = fixture.app();
    let index = index_files(&fixture, &app);
    drop(app);
    let saved = tempfile::tempdir().unwrap();
    copy_present(&index, saved.path());

    // The files change and the index records it; then the index goes back
    // to how it was before, as if its update never reached the disk.
    let mut app = fixture.app();
    app.duplicate("Plan.md");
    app.move_into("Plan.md", "Docs");
    app.new_note(Some(""));
    support::select(&mut app, "Docs/Plan.md");
    let tree = rows(&app);
    let before = ids(&fixture);
    drop(app);
    remove_present(&index);
    for file in &index {
        let saved = saved.path().join(file.file_name().unwrap());
        if saved.exists() {
            std::fs::copy(saved, file).unwrap();
        }
    }

    let mut app = fixture.app();
    support::select(&mut app, "Docs/Plan.md");
    assert_eq!(rows(&app), tree);
    assert_eq!(ids(&fixture), before);
    app.duplicate("Docs/Plan.md");
    let copy = object_id(&fixture.read("Docs/Plan 1.md"));
    assert!(before.iter().all(|(_, id)| *id != copy));
}

#[test]
fn changes_finish_and_the_tree_follows_when_the_index_refuses_writes() {
    let plan = note(A, "Body\n");
    let fixture = Fixture::new(&[
        ("Plan.md", &plan),
        ("Loose.md", b"loose"),
        ("Docs/Guide.md", b"g"),
        ("Old/Inner.md", b"i"),
    ]);
    let mut app = fixture.app_with(FakeSystem::without_trash());
    app.open_file("Plan.md");
    let index = refuse_index_writes(&fixture, &app);

    // Each change is on disk, so each one succeeds and the tree shows it,
    // though the index can't record it yet.
    app.start_rename("Plan.md");
    assert!(app.commit_rename("Roadmap"));
    assert_eq!(notice(&app), None);
    app.move_into("Roadmap.md", "Docs");
    assert_eq!(notice(&app), None);
    app.duplicate("Loose.md");
    assert_eq!(notice(&app), None);
    app.trash("Old");
    assert_eq!(
        notice(&app).unwrap().0,
        "Moved \u{201c}Old\u{201d} to .noura/trash"
    );

    assert_eq!(fixture.read("Docs/Roadmap.md"), plan);
    assert_eq!(fixture.read("Loose 1.md"), b"loose");
    assert!(!fixture.exists("Old"));
    assert_eq!(rows(&app), ["Docs", "Guide", "Roadmap", "Loose 1", "Loose"]);
    assert_eq!(document(&app).as_deref(), Some("Loose 1.md"));
    assert!(indexed_markdown(&index).contains(&"Plan.md".to_owned()));

    // Once the index takes writes again, the next look at the files brings
    // it in line with them.
    accept_index_writes(&index);
    app.new_note(Some(""));
    assert_eq!(
        indexed_markdown(&index),
        [
            "Docs/Guide.md",
            "Docs/Roadmap.md",
            "Loose 1.md",
            "Loose.md",
            "Untitled.md"
        ]
    );
}
