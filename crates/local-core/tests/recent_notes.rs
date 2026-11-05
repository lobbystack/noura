//! The notes whose files changed last, as the index answers for the
//! Inbox's Recent list.

use std::{
    path::Path,
    time::{Duration, SystemTime},
};

use local_core::WorkspaceEngine;
use tempfile::{TempDir, tempdir};

/// A fresh workspace, its app data folder and its engine.
fn engine() -> (TempDir, TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Recent notes", app_data.path())
            .unwrap();
    (workspace, app_data, engine)
}

/// Write `bytes` at `relative` with the modification time `minute`
/// minutes after a fixed moment.
fn write(root: &Path, relative: &str, bytes: &[u8], minute: u64) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    touch(root, relative, minute);
}

fn touch(root: &Path, relative: &str, minute: u64) {
    let file = std::fs::File::options()
        .write(true)
        .open(root.join(relative))
        .unwrap();
    let moment = SystemTime::UNIX_EPOCH + Duration::from_secs(1_800_000_000 + minute * 60);
    file.set_modified(moment).unwrap();
}

fn note(id_suffix: &str) -> Vec<u8> {
    format!(
        "---\nid: note_{id_suffix}\ntype: note\ncreated: 2026-10-20T10:00:00Z\n\
         updated: 2026-10-20T10:00:00Z\n---\n\nA note.\n"
    )
    .into_bytes()
}

const TASK: &[u8] = b"---\nid: task_01k00000000000000000000002\ntype: task\nstatus: todo\n\
    created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n---\n\nShip.\n";
const PROJECT: &[u8] = b"---\nid: project_01k00000000000000000000003\ntype: project\n\
    status: active\ncreated: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n---\n\n\
    # Launch\n";
const BROKEN: &[u8] = b"---\nid: [broken\n---\n";

#[test]
fn notes_come_newest_first_with_ties_in_path_order_and_other_types_left_out() {
    let (workspace, _app_data, engine) = engine();
    let root = workspace.path();
    write(root, "old.md", b"# Old\n", 0);
    write(root, "b.md", b"# B\n", 10);
    write(root, "a.md", b"# A\n", 10);
    write(
        root,
        "Notes/managed.md",
        &note("01k00000000000000000000001"),
        20,
    );
    write(root, "broken.md", BROKEN, 30);
    write(root, "Tasks/ship.md", TASK, 40);
    write(root, "Projects/launch.md", PROJECT, 50);
    write(root, "photo.png", b"not really an image", 60);
    write(root, ".hidden/secret.md", b"# Secret\n", 70);
    engine.reconcile().unwrap();

    assert_eq!(
        engine.recent_notes(10).unwrap(),
        ["broken.md", "Notes/managed.md", "a.md", "b.md", "old.md"]
    );
    assert_eq!(
        engine.recent_notes(2).unwrap(),
        ["broken.md", "Notes/managed.md"]
    );
    assert_eq!(engine.recent_notes(0).unwrap(), Vec::<String>::new());
}

#[test]
fn files_that_share_an_id_are_each_listed_once() {
    let (workspace, _app_data, engine) = engine();
    let root = workspace.path();
    let copy = note("01k00000000000000000000030");
    write(root, "Notes/dup-a.md", &copy, 0);
    write(root, "Notes/dup-b.md", &copy, 5);
    write(root, "Notes/other.md", b"# Other\n", 10);
    engine.reconcile().unwrap();

    assert_eq!(
        engine.recent_notes(10).unwrap(),
        ["Notes/other.md", "Notes/dup-b.md", "Notes/dup-a.md"]
    );
}

/// Recent comes from the files: a note moved in another app keeps its
/// place, an index that missed changes while Noura was closed catches up
/// when the workspace opens, and a rebuilt or deleted index gives the same
/// list.
#[test]
fn the_list_follows_the_files_through_moves_a_stale_index_and_rebuilds() {
    let (workspace, app_data, engine) = engine();
    let root = workspace.path();
    write(root, "first.md", &note("01k00000000000000000000001"), 0);
    write(root, "second.md", &note("01k00000000000000000000002"), 10);
    write(root, "third.md", b"# Third\n", 20);
    engine.reconcile().unwrap();
    assert_eq!(
        engine.recent_notes(5).unwrap(),
        ["third.md", "second.md", "first.md"]
    );
    drop(engine);

    // Another app moves a note, adds one and deletes one while Noura is
    // closed. A move keeps the file's modification time.
    std::fs::create_dir(root.join("Archive")).unwrap();
    std::fs::rename(root.join("second.md"), root.join("Archive/second.md")).unwrap();
    touch(root, "Archive/second.md", 10);
    write(root, "fourth.md", b"# Fourth\n", 30);
    std::fs::remove_file(root.join("first.md")).unwrap();
    let expected = ["fourth.md", "third.md", "Archive/second.md"];

    let engine = WorkspaceEngine::open_with_app_data(root, app_data.path()).unwrap();
    assert_eq!(engine.recent_notes(5).unwrap(), expected);

    engine.rebuild_index().unwrap();
    assert_eq!(engine.recent_notes(5).unwrap(), expected);
    drop(engine);

    let indexes = files_named(app_data.path(), "index.sqlite");
    assert_eq!(indexes.len(), 1);
    for suffix in ["", "-wal", "-shm"] {
        let mut path = indexes[0].clone().into_os_string();
        path.push(suffix);
        let _ = std::fs::remove_file(path);
    }
    assert!(!indexes[0].exists());
    let engine = WorkspaceEngine::open_with_app_data(root, app_data.path()).unwrap();
    assert_eq!(engine.recent_notes(5).unwrap(), expected);
}

/// Every file called `name` under `root`.
fn files_named(root: &Path, name: &str) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else if path.file_name().is_some_and(|file| file == name) {
                found.push(path);
            }
        }
    }
    found
}
