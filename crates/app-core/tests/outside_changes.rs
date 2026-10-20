//! Seam 1: files that other apps change on disk reach the open workspace's
//! view state through `app-core`'s change stream, with no action from you.

use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

use app_core::{App, Change, Origin, Rescans, Screen, WatchFault, Workspace};

/// How long a test waits for the change stream before it fails.
const PATIENCE: Duration = Duration::from_secs(15);

/// An app with a workspace open and every change it announces.
struct Live {
    app: App,
    changes: Receiver<Change>,
    seen: Vec<Change>,
    folder: PathBuf,
    _app_data: tempfile::TempDir,
    _parent: tempfile::TempDir,
}

impl Live {
    /// Open `files` as a workspace with the default rescan intervals.
    fn open(files: &[(&str, &str)]) -> Self {
        Self::open_with(files, Rescans::default())
    }

    fn open_with(files: &[(&str, &str)], rescans: Rescans) -> Self {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        std::fs::create_dir(&folder).unwrap();
        for (relative, text) in files {
            write(&folder, relative, text);
        }
        let mut app = App::launch_with(app_data.path(), rescans);
        let (sender, changes) = channel();
        app.subscribe(move |change| {
            let _ = sender.send(change);
        });
        app.open_folder(folder.clone());
        let mut live = Self {
            app,
            changes,
            seen: Vec::new(),
            folder,
            _app_data: app_data,
            _parent: parent,
        };
        live.wait_until("the workspace opens", |app| {
            !app.is_busy() && app.screen() == &Screen::Workspace
        });
        live.seen.clear();
        live
    }

    /// Wait on the change stream, polling after each change, until `done`
    /// holds for the view state. Only a change wakes the wait, so a view
    /// that updates without announcing it fails the test.
    fn wait_until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.app.poll();
            self.seen.extend(self.changes.try_iter());
            if done(&self.app) {
                return;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "timed out waiting until {what}");
            if let Ok(change) = self.changes.recv_timeout(left) {
                self.seen.push(change);
            }
        }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.folder.join(relative)
    }

    fn workspace(&self) -> &Workspace {
        self.app.workspace().unwrap()
    }
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// The relative paths of the visible rows.
fn rows(app: &App) -> Vec<String> {
    app.tree()
        .map(|tree| {
            tree.rows()
                .iter()
                .map(|row| row.relative_path.clone())
                .collect()
        })
        .unwrap_or_default()
}

/// Expand the folder shown at `relative_path`.
fn expand(app: &mut App, relative_path: &str) {
    let tree = app.tree_mut().unwrap();
    let row = tree.find_row(relative_path).unwrap();
    if tree.rows()[row].expanded == Some(false) {
        tree.toggle(row);
    }
}

/// A managed note with a fixed ID.
fn note(id: &str, body: &str) -> String {
    format!("---\nid: {id}\ntype: note\n---\n\n{body}\n")
}

const PLAN_ID: &str = "note_01k00000000000000000000001";

#[test]
fn a_file_another_app_creates_appears_in_the_tree_marked_as_from_outside() {
    let mut live = Live::open(&[("Existing.md", "# Existing\n")]);

    write(&live.folder, "Fresh.md", "# Fresh\n");

    live.wait_until("the new file shows", |app| {
        rows(app) == ["Existing.md", "Fresh.md"]
    });
    assert!(live.seen.contains(&Change::Files(Origin::Outside)));
    assert!(!live.seen.contains(&Change::Files(Origin::Noura)));
    assert!(live.seen.contains(&Change::Tree));
}

#[test]
fn renames_moves_and_deletes_by_another_app_reach_the_tree_and_keep_open_folders_open() {
    let mut live = Live::open(&[
        ("A.md", "# A\n"),
        ("B.md", "# B\n"),
        ("Old.md", "# Old\n"),
        ("Docs/C.md", "# C\n"),
    ]);
    expand(&mut live.app, "Docs");

    std::fs::rename(live.path("A.md"), live.path("Renamed.md")).unwrap();
    std::fs::rename(live.path("B.md"), live.path("Docs/B.md")).unwrap();
    std::fs::remove_file(live.path("Old.md")).unwrap();

    live.wait_until("the tree shows the moves", |app| {
        rows(app) == ["Docs", "Docs/B.md", "Docs/C.md", "Renamed.md"]
    });
    assert!(live.seen.contains(&Change::Files(Origin::Outside)));
}

#[test]
fn the_open_document_follows_a_note_another_app_moves_and_closes_once_it_is_deleted() {
    let mut live = Live::open(&[("Plan.md", &note(PLAN_ID, "The plan."))]);
    live.app.open_file("Plan.md");

    std::fs::create_dir(live.path("Archive")).unwrap();
    std::fs::rename(live.path("Plan.md"), live.path("Archive/Plan 2026.md")).unwrap();

    live.wait_until("the document follows the note", |app| {
        app.document()
            .is_some_and(|file| file.relative_path == "Archive/Plan 2026.md")
    });
    assert_eq!(live.app.document().unwrap().name, "Plan 2026");
    assert!(live.seen.contains(&Change::Document));

    std::fs::remove_file(live.path("Archive/Plan 2026.md")).unwrap();

    live.wait_until("the document closes", |app| app.document().is_none());
}

#[test]
fn a_plain_file_that_another_app_renames_closes_in_the_document_area() {
    let mut live = Live::open(&[("Loose.md", "# Loose\n")]);
    live.app.open_file("Loose.md");

    std::fs::rename(live.path("Loose.md"), live.path("Elsewhere.md")).unwrap();

    live.wait_until("the document closes", |app| app.document().is_none());
    assert_eq!(rows(&live.app), ["Elsewhere.md"]);
}

/// Intervals so long that no rescan runs during a test unless the test
/// shortens one of them.
const NO_RESCANS: Rescans = Rescans {
    periodic: Duration::from_secs(3600),
    without_watcher: Duration::from_secs(3600),
};

#[test]
fn without_the_native_watcher_full_rescans_find_outside_changes() {
    let mut live = Live::open_with(
        &[("A.md", "# A\n")],
        Rescans {
            without_watcher: Duration::from_millis(50),
            ..NO_RESCANS
        },
    );
    live.workspace().force_watch_fault(WatchFault::Unavailable);

    write(&live.folder, "Fresh.md", "# Fresh\n");
    write(&live.folder, "Scan.pdf", "%PDF-1.7\n");
    std::fs::remove_file(live.path("A.md")).unwrap();

    live.wait_until("a rescan finds the changes", |app| {
        rows(app) == ["Fresh.md", "Scan.pdf"]
    });
    assert!(live.seen.contains(&Change::Files(Origin::Outside)));
}

#[test]
fn after_the_watcher_overflows_a_full_rescan_finds_the_lost_changes() {
    let mut live = Live::open_with(&[("A.md", "# A\n")], NO_RESCANS);
    live.workspace().force_watch_fault(WatchFault::Overflow);

    write(&live.folder, "Fresh.md", "# Fresh\n");
    write(&live.folder, "Scan.pdf", "%PDF-1.7\n");
    std::fs::remove_file(live.path("A.md")).unwrap();

    live.wait_until("a rescan finds the changes", |app| {
        rows(app) == ["Fresh.md", "Scan.pdf"]
    });
    assert!(live.seen.contains(&Change::Files(Origin::Outside)));
}

#[test]
fn the_periodic_rescan_finds_a_change_the_watcher_missed() {
    let periodic = Duration::from_secs(3);
    let mut live = Live::open_with(
        &[("A.md", "# A\n")],
        Rescans {
            periodic,
            ..NO_RESCANS
        },
    );
    live.workspace().force_watch_fault(WatchFault::Silent);
    let written = Instant::now();

    write(&live.folder, "Missed.md", "# Missed\n");
    std::thread::sleep(Duration::from_millis(300));
    live.app.poll();
    assert_eq!(rows(&live.app), ["A.md"], "the silent watcher reported it");

    live.wait_until("the periodic rescan finds it", |app| {
        rows(app) == ["A.md", "Missed.md"]
    });
    assert!(written.elapsed() < periodic + PATIENCE);
}
