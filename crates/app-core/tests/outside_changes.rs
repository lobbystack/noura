//! Seam 1: files that other apps change on disk reach the open workspace's
//! view state through `app-core`'s change stream, with no action from you.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{Receiver, channel},
    },
    time::{Duration, Instant},
};

use app_core::{
    App, Change, DropSpot, NoSystem, OpenAs, Origin, Rescans, SPRING_OPEN_DELAY, Screen,
    WatchFault, Workspace,
};

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
        let mut app = App::launch_with_rescans(app_data.path(), Arc::new(NoSystem), rescans);
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
const TASK_ID: &str = "task_01k00000000000000000000002";
const PROJECT_ID: &str = "project_01k00000000000000000000003";

fn task() -> String {
    format!("---\nid: {TASK_ID}\ntype: task\nstatus: todo\n---\n\nShip it.\n")
}

fn project() -> String {
    format!("---\nid: {PROJECT_ID}\ntype: project\nstatus: active\n---\n\nLaunch.\n")
}

/// The path of each object the workspace knows, by ID.
fn objects(app: &App) -> Vec<(String, String)> {
    let mut objects: Vec<(String, String)> = app
        .workspace()
        .unwrap()
        .objects()
        .unwrap()
        .into_iter()
        .map(|object| (object.id, object.relative_path))
        .collect();
    objects.sort();
    objects
}

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
    live.app.open_file("Plan.md", OpenAs::Kept);

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
    live.app.open_file("Loose.md", OpenAs::Kept);

    std::fs::rename(live.path("Loose.md"), live.path("Elsewhere.md")).unwrap();

    live.wait_until("the document closes", |app| app.document().is_none());
    assert_eq!(rows(&live.app), ["Elsewhere.md"]);
}

/// Whether the folder at `relative_path` shows its contents.
fn is_open(app: &App, relative_path: &str) -> bool {
    let tree = app.tree().unwrap();
    let row = tree.find_row(relative_path).unwrap();
    tree.rows()[row].expanded == Some(true)
}

/// Rest a drag of `dragged` on the closed folder `folder` until it opens.
fn drag_until_open(app: &mut App, dragged: &str, folder: &str) {
    let row = |app: &App, path: &str| app.tree().unwrap().find_row(path).unwrap();
    app.start_tree_drag(row(app, dragged));
    let start = Instant::now();
    let over = DropSpot::Row(row(app, folder));
    app.drag_tree_over(over, start);
    app.drag_tree_over(over, start + SPRING_OPEN_DELAY);
    assert!(is_open(app, folder));
}

#[test]
fn a_drag_goes_on_while_another_app_changes_files() {
    let mut live = Live::open(&[
        ("Plan.md", "# Plan\n"),
        ("Docs/Spec.md", "# Spec\n"),
        ("Archive/Old.md", "# Old\n"),
    ]);
    drag_until_open(&mut live.app, "Plan.md", "Docs");
    let archive = live.app.tree().unwrap().find_row("Archive").unwrap();
    let resting = Instant::now();
    live.app.drag_tree_over(DropSpot::Row(archive), resting);

    // A new folder at the top moves every row down one.
    write(&live.folder, "About/Team.md", "# Team\n");
    live.wait_until("the new folder shows", |app| {
        rows(app).contains(&"About".to_owned())
    });

    let tree = live.app.tree().unwrap();
    assert_eq!(tree.dragging(), Some("Plan.md"));
    assert_eq!(tree.drop_target(), Some("Archive"));
    assert_eq!(tree.spring_open_at(), Some(resting + SPRING_OPEN_DELAY));
    assert!(is_open(&live.app, "Docs"));
    let archive = tree.find_row("Archive").unwrap();
    assert_eq!(archive, 1);
    live.app
        .drag_tree_over(DropSpot::Row(archive), Instant::now());
    live.app.finish_tree_drag();
    assert!(live.path("Archive/Plan.md").is_file());
    assert!(!live.path("Plan.md").exists());
    // The folder the drag opened closes again.
    assert!(!is_open(&live.app, "Docs"));
}

#[test]
fn a_drag_stops_when_another_app_deletes_the_dragged_file() {
    let mut live = Live::open(&[("Plan.md", "# Plan\n"), ("Docs/Spec.md", "# Spec\n")]);
    drag_until_open(&mut live.app, "Plan.md", "Docs");

    std::fs::remove_file(live.path("Plan.md")).unwrap();
    live.wait_until("the file leaves the tree", |app| {
        !rows(app).contains(&"Plan.md".to_owned())
    });

    assert_eq!(live.app.tree().unwrap().dragging(), None);
    assert!(!is_open(&live.app, "Docs"));
    live.app.finish_tree_drag();
    assert_eq!(rows(&live.app), ["Docs"]);
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

/// A folder or a picture has no index entry, so the rescan after lost
/// events changes nothing in the index, and the view lists the files again
/// all the same.
#[test]
fn after_the_watcher_overflows_a_new_folder_and_picture_show_up() {
    let mut live = Live::open_with(&[("A.md", "# A\n")], NO_RESCANS);
    live.workspace().force_watch_fault(WatchFault::Overflow);

    std::fs::create_dir(live.path("Photos")).unwrap();
    write(&live.folder, "Photos/cat.jpg", "jpg");

    live.wait_until("a rescan finds the changes", |app| {
        rows(app) == ["Photos", "A.md"]
    });
    assert!(live.seen.contains(&Change::Files(Origin::Outside)));
}

#[test]
fn when_the_platform_asks_for_a_rescan_a_full_rescan_finds_the_changes() {
    let mut live = Live::open_with(&[("A.md", "# A\n")], NO_RESCANS);
    // The watcher reports nothing on its own, as when the platform dropped
    // its events and only says to rescan.
    live.workspace().force_watch_fault(WatchFault::Silent);

    write(&live.folder, "Fresh.md", "# Fresh\n");
    write(&live.folder, "Scan.pdf", "%PDF-1.7\n");
    std::fs::remove_file(live.path("A.md")).unwrap();
    live.workspace().inject_watch_rescan_request();

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

#[test]
fn a_note_task_or_project_another_app_moves_keeps_its_id_whatever_order_the_events_arrive_in() {
    let mut live = Live::open_with(
        &[
            ("Plan.md", &note(PLAN_ID, "The plan.")),
            ("Ship.md", &task()),
            ("Launch.md", &project()),
        ],
        NO_RESCANS,
    );
    // The test delivers each move's events itself, in the order it chooses.
    live.workspace().force_watch_fault(WatchFault::Silent);
    // (ID, from, to, batches of "old" and "new" events)
    let moves: [(&str, &str, &str, &[&[&str]]); 4] = [
        (PLAN_ID, "Plan.md", "Notes/Plan.md", &[&["new"], &["old"]]),
        (
            TASK_ID,
            "Ship.md",
            "Tasks/Ship it.md",
            &[&["old"], &["new"]],
        ),
        (
            PROJECT_ID,
            "Launch.md",
            "Projects/Launch.md",
            &[&["new", "old"]],
        ),
        (
            PLAN_ID,
            "Notes/Plan.md",
            "Plan again.md",
            &[&["old", "new"]],
        ),
    ];
    let mut expected = vec![
        (PLAN_ID.to_owned(), "Plan.md".to_owned()),
        (PROJECT_ID.to_owned(), "Launch.md".to_owned()),
        (TASK_ID.to_owned(), "Ship.md".to_owned()),
    ];
    let mut batch_number = 0;
    for (id, from, to, batches) in moves {
        let destination = live.path(to);
        std::fs::create_dir_all(destination.parent().unwrap()).unwrap();
        std::fs::rename(live.path(from), destination).unwrap();
        for batch in batches {
            // A new file in each batch shows when the batch has landed.
            batch_number += 1;
            let marker = format!("Marker {batch_number}.txt");
            write(&live.folder, &marker, "");
            let mut paths: Vec<&str> = batch
                .iter()
                .map(|event| if *event == "old" { from } else { to })
                .collect();
            paths.push(&marker);
            live.workspace().inject_watch_event(&paths);
            live.wait_until("the batch lands", |app| rows(app).contains(&marker));
            for (known, path) in &mut expected {
                if known == id {
                    *path = to.to_owned();
                }
            }
            assert_eq!(objects(&live.app), expected, "after {batch:?} for {from}");
            let found = live.workspace().find_object(id).unwrap().unwrap();
            assert_eq!(found.relative_path, to);
        }
    }
}

fn identity_problems(app: &App) -> Vec<(Vec<String>, Option<String>)> {
    app.problems()
        .iter()
        .filter(|problem| problem.code == "identity_conflict")
        .map(|problem| (problem.paths.clone(), problem.object_id.clone()))
        .collect()
}

#[test]
fn two_files_that_share_an_id_stay_untouched_and_block_the_id_until_one_loses_it() {
    let original = note(PLAN_ID, "The plan.");
    let mut live = Live::open(&[("Plan.md", &original)]);

    // Another app copies the note, ID and all.
    write(&live.folder, "Plan copy.md", &original);

    live.wait_until("the problem shows", |app| {
        !identity_problems(app).is_empty()
    });
    assert_eq!(
        identity_problems(&live.app),
        [(
            vec!["Plan copy.md".to_owned(), "Plan.md".to_owned()],
            Some(PLAN_ID.to_owned())
        )]
    );
    assert!(live.seen.contains(&Change::Problems));
    let error = live.workspace().find_object(PLAN_ID).unwrap_err();
    assert_eq!(error.code, "identity_conflict");
    assert!(objects(&live.app).is_empty(), "neither copy may win");
    for path in ["Plan.md", "Plan copy.md"] {
        assert_eq!(std::fs::read_to_string(live.path(path)).unwrap(), original);
    }

    // You remove the ID from the copy in another app.
    write(&live.folder, "Plan copy.md", "The plan.\n");

    live.wait_until("the problem goes", |app| identity_problems(app).is_empty());
    let found = live.workspace().find_object(PLAN_ID).unwrap().unwrap();
    assert_eq!(found.relative_path, "Plan.md");
    assert_eq!(
        std::fs::read_to_string(live.path("Plan.md")).unwrap(),
        original
    );
}

/// Whether the tree row at `relative_path` exists and is marked not
/// downloaded.
fn not_downloaded(app: &App, relative_path: &str) -> Option<bool> {
    let tree = app.tree()?;
    let row = tree.find_row(relative_path)?;
    Some(tree.rows()[row].not_downloaded)
}

fn hit_paths(app: &App, query: &str) -> Vec<String> {
    app.workspace()
        .unwrap()
        .search(query)
        .unwrap()
        .into_iter()
        .map(|hit| hit.relative_path)
        .collect()
}

#[test]
fn a_note_the_cloud_drive_evicts_keeps_its_entry_and_id_until_it_downloads_again() {
    let mut live = Live::open(&[("Journal/today.md", &note(PLAN_ID, "Morning pages."))]);
    expand(&mut live.app, "Journal");

    // iCloud swaps the file for a placeholder named after it.
    std::fs::remove_file(live.path("Journal/today.md")).unwrap();
    write(&live.folder, "Journal/.today.md.icloud", "bplist00");

    live.wait_until("the row shows as not downloaded", |app| {
        not_downloaded(app, "Journal/today.md") == Some(true)
    });
    assert_eq!(rows(&live.app), ["Journal", "Journal/today.md"]);
    let found = live.workspace().find_object(PLAN_ID).unwrap().unwrap();
    assert_eq!(found.relative_path, "Journal/today.md");
    assert_eq!(hit_paths(&live.app, "morning"), ["Journal/today.md"]);
    assert!(live.app.problems().is_empty(), "{:?}", live.app.problems());

    live.app.open_file("Journal/today.md", OpenAs::Kept);
    assert_eq!(live.app.document(), None);
    assert_eq!(
        live.app.notice().unwrap().message,
        "This file isn't downloaded yet. Try again once it is."
    );

    // The download replaces the placeholder with the file.
    write(
        &live.folder,
        "Journal/today.md",
        &note(PLAN_ID, "Evening pages."),
    );
    std::fs::remove_file(live.path("Journal/.today.md.icloud")).unwrap();

    live.wait_until("the downloaded file shows", |app| {
        not_downloaded(app, "Journal/today.md") == Some(false)
            && hit_paths(app, "evening") == ["Journal/today.md"]
    });
    let found = live.workspace().find_object(PLAN_ID).unwrap().unwrap();
    assert_eq!(found.relative_path, "Journal/today.md");
    live.app.open_file("Journal/today.md", OpenAs::Kept);
    assert_eq!(
        live.app.document().map(|file| file.relative_path.as_str()),
        Some("Journal/today.md")
    );
}

#[test]
fn any_file_the_cloud_drive_has_not_downloaded_shows_marked() {
    let mut live = Live::open(&[("Notes.md", "# Notes\n")]);

    write(&live.folder, ".Scan.pdf.icloud", "bplist00");

    live.wait_until("the placeholder shows as its file", |app| {
        not_downloaded(app, "Scan.pdf") == Some(true)
    });
    assert_eq!(rows(&live.app), ["Notes.md", "Scan.pdf"]);
}

/// Set or clear the attribute a cloud provider gives a file whose content
/// lives only online.
#[cfg(windows)]
fn set_online_only(path: &Path, online_only: bool) {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_OFFLINE, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
    };

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
    // SAFETY: `wide` is a NUL-terminated path that outlives both calls.
    unsafe {
        let attributes = GetFileAttributesW(wide.as_ptr());
        assert_ne!(attributes, INVALID_FILE_ATTRIBUTES);
        let attributes = if online_only {
            attributes | FILE_ATTRIBUTE_OFFLINE
        } else {
            attributes & !FILE_ATTRIBUTE_OFFLINE
        };
        assert_ne!(SetFileAttributesW(wide.as_ptr(), attributes), 0);
    }
}

#[cfg(windows)]
#[test]
fn a_file_the_cloud_provider_marks_online_only_shows_marked_and_noura_never_reads_it() {
    let mut live = Live::open_with(
        &[
            ("Online.md", &note(PLAN_ID, "Morning pages.")),
            ("Scan.pdf", "%PDF-1.7\n"),
        ],
        NO_RESCANS,
    );
    // The test delivers the watcher's events itself, so nothing reads the
    // new text before the file goes online-only.
    live.workspace().force_watch_fault(WatchFault::Silent);
    write(&live.folder, "Online.md", &note(PLAN_ID, "Evening pages."));
    set_online_only(&live.path("Online.md"), true);
    set_online_only(&live.path("Scan.pdf"), true);
    live.workspace()
        .inject_watch_event(&["Online.md", "Scan.pdf"]);

    live.wait_until("both files show as not downloaded", |app| {
        not_downloaded(app, "Online.md") == Some(true)
            && not_downloaded(app, "Scan.pdf") == Some(true)
    });
    // The index keeps the last text Noura read, and the new text stays unread.
    assert_eq!(hit_paths(&live.app, "morning"), ["Online.md"]);
    assert!(hit_paths(&live.app, "evening").is_empty());
    let found = live.workspace().find_object(PLAN_ID).unwrap().unwrap();
    assert_eq!(found.relative_path, "Online.md");
    live.app.open_file("Online.md", OpenAs::Kept);
    assert_eq!(live.app.document(), None);
    assert_eq!(
        live.app.notice().unwrap().message,
        "This file isn't downloaded yet. Try again once it is."
    );

    // The provider downloads both files.
    set_online_only(&live.path("Online.md"), false);
    set_online_only(&live.path("Scan.pdf"), false);
    live.workspace()
        .inject_watch_event(&["Online.md", "Scan.pdf"]);

    live.wait_until("the downloaded files show", |app| {
        not_downloaded(app, "Online.md") == Some(false)
            && not_downloaded(app, "Scan.pdf") == Some(false)
            && hit_paths(app, "evening") == ["Online.md"]
    });
}

#[test]
fn an_edit_another_app_makes_reaches_search() {
    let mut live = Live::open(&[("Plan.md", &note(PLAN_ID, "Morning pages."))]);

    write(&live.folder, "Plan.md", &note(PLAN_ID, "Evening pages."));

    live.wait_until("search finds the new words", |app| {
        hit_paths(app, "evening") == ["Plan.md"] && hit_paths(app, "morning").is_empty()
    });
    assert!(live.seen.contains(&Change::Files(Origin::Outside)));
}

#[test]
fn properties_another_app_breaks_show_as_a_problem_until_they_are_fixed() {
    let original = note(PLAN_ID, "The plan.");
    let mut live = Live::open(&[("Plan.md", &original)]);
    let parse_problems = |app: &App| -> Vec<Vec<String>> {
        app.problems()
            .iter()
            .filter(|problem| problem.code == "parse_error")
            .map(|problem| problem.paths.clone())
            .collect()
    };

    let broken = format!("---\nid: {PLAN_ID}\ntype: [note\n---\n\nThe plan.\n");
    write(&live.folder, "Plan.md", &broken);

    live.wait_until("the problem shows", |app| {
        parse_problems(app) == [vec!["Plan.md".to_owned()]]
    });
    assert_eq!(rows(&live.app), ["Plan.md"]);
    assert_eq!(
        std::fs::read_to_string(live.path("Plan.md")).unwrap(),
        broken
    );

    write(&live.folder, "Plan.md", &original);

    live.wait_until("the problem goes", |app| parse_problems(app).is_empty());
    let found = live.workspace().find_object(PLAN_ID).unwrap().unwrap();
    assert_eq!(found.relative_path, "Plan.md");
}

#[test]
fn a_watcher_event_for_a_path_outside_the_workspace_changes_nothing() {
    let mut live = Live::open_with(&[("A.md", "# A\n")], NO_RESCANS);
    live.workspace().force_watch_fault(WatchFault::Silent);
    let outside = live.folder.parent().unwrap().join("Outside.md");
    std::fs::write(&outside, "# Outside\n").unwrap();
    write(&live.folder, "Fresh.md", "# Fresh\n");

    live.workspace().inject_watch_event(&[
        outside.as_path(),
        Path::new("../Outside.md"),
        Path::new("Fresh.md"),
    ]);

    live.wait_until("the batch lands", |app| rows(app) == ["A.md", "Fresh.md"]);
    assert!(hit_paths(&live.app, "outside").is_empty());
}

#[test]
fn opening_a_path_that_climbs_out_of_the_workspace_reads_nothing() {
    let mut live = Live::open(&[("A.md", "# A\n")]);
    std::fs::write(
        live.folder.parent().unwrap().join("Secret.md"),
        "# Secret\n",
    )
    .unwrap();

    live.app.open_file("../Secret.md", OpenAs::Kept);

    assert_eq!(live.app.document(), None);
    assert!(live.app.notice().is_some());
}

#[cfg(unix)]
#[test]
fn a_link_another_app_adds_to_a_folder_outside_the_workspace_brings_nothing_in() {
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("Secret.md"), "# Secret\n\nSaffron.\n").unwrap();
    let mut live = Live::open(&[("A.md", "# A\n")]);

    std::os::unix::fs::symlink(outside.path(), live.path("Linked")).unwrap();
    write(&live.folder, "Fresh.md", "# Fresh\n\nSaffron.\n");

    live.wait_until("the new note shows", |app| {
        rows(app).contains(&"Fresh.md".to_owned())
    });
    assert!(!rows(&live.app).iter().any(|row| row.starts_with("Linked")));
    assert_eq!(hit_paths(&live.app, "saffron"), ["Fresh.md"]);
}

#[test]
fn a_workspace_that_opens_with_a_shared_id_reports_both_files() {
    let original = note(PLAN_ID, "The plan.");
    let live = Live::open(&[("A/Plan.md", &original), ("B/Plan.md", &original)]);

    assert_eq!(
        identity_problems(&live.app),
        [(
            vec!["A/Plan.md".to_owned(), "B/Plan.md".to_owned()],
            Some(PLAN_ID.to_owned())
        )]
    );
}

/// `{stem}�.md`, a name that has no UTF-8 form: an invalid byte on Unix,
/// an unpaired surrogate on Windows. macOS refuses to create such names, so
/// only Linux and Windows can put one on disk.
#[cfg(unix)]
fn unrepresentable(stem: &str) -> std::ffi::OsString {
    use std::os::unix::ffi::OsStringExt;
    std::ffi::OsString::from_vec([stem.as_bytes(), b"\xe9.md"].concat())
}

#[cfg(windows)]
fn unrepresentable(stem: &str) -> std::ffi::OsString {
    use std::os::windows::ffi::OsStringExt;
    let mut wide: Vec<u16> = stem.encode_utf16().collect();
    wide.push(0xD800);
    wide.extend(".md".encode_utf16());
    std::ffi::OsString::from_wide(&wide)
}

#[cfg(any(target_os = "linux", windows))]
fn unsupported_problems(app: &App) -> Vec<Vec<String>> {
    app.problems()
        .iter()
        .filter(|problem| problem.code == "unsupported_path")
        .map(|problem| problem.paths.clone())
        .collect()
}

#[test]
fn a_watcher_event_for_a_name_noura_cannot_represent_leaves_the_rest_of_the_batch_working() {
    let mut live = Live::open_with(&[("A.md", "# A\n")], NO_RESCANS);
    live.workspace().force_watch_fault(WatchFault::Silent);
    write(&live.folder, "Fresh.md", "# Fresh\n");
    let ghost = unrepresentable("Ghost");

    live.workspace()
        .inject_watch_event(&[Path::new(&ghost), Path::new("Fresh.md")]);

    live.wait_until("the rest of the batch lands", |app| {
        rows(app) == ["A.md", "Fresh.md"]
    });
}

#[cfg(any(target_os = "linux", windows))]
#[test]
fn names_noura_cannot_represent_stay_as_they_are_and_show_as_problems() {
    let mut live = Live::open(&[
        ("Good.md", "# Good\n\nSaffron buns.\n"),
        ("Notes/Kept.md", "# Kept\n"),
    ]);
    expand(&mut live.app, "Notes");
    let file = live.folder.join(unrepresentable("Caf"));
    let folder = live.folder.join("Notes").join(unrepresentable("D"));

    // Another app adds a file and a folder whose names have no UTF-8 form,
    // and a file Noura can show.
    std::fs::write(&file, "# Odd\n\nSaffron tea.\n").unwrap();
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("inside.md"), "# Inside\n").unwrap();
    write(&live.folder, "Fresh.md", "# Fresh\n");

    live.wait_until("the problems show", |app| {
        rows(app).contains(&"Fresh.md".to_owned()) && unsupported_problems(app).len() == 2
    });
    assert_eq!(
        rows(&live.app),
        ["Notes", "Notes/Kept.md", "Fresh.md", "Good.md"]
    );
    assert_eq!(
        unsupported_problems(&live.app),
        [Vec::<String>::new(), vec!["Notes".to_owned()]]
    );
    assert_eq!(hit_paths(&live.app, "saffron"), ["Good.md"]);
    // The names and their contents stay exactly as the other app left them.
    assert_eq!(
        std::fs::read_to_string(&file).unwrap(),
        "# Odd\n\nSaffron tea.\n"
    );
    assert_eq!(
        std::fs::read_to_string(folder.join("inside.md")).unwrap(),
        "# Inside\n"
    );
}

/// Noura's operations take names it can show, so none can name these
/// directly. Those that reach them through the folder holding them carry
/// them over byte for byte, and an import leaves them where they are.
#[cfg(any(target_os = "linux", windows))]
#[test]
fn operations_that_reach_names_noura_cannot_represent_keep_them_as_they_are() {
    let mut live = Live::open(&[("Notes/Kept.md", "# Kept\n"), ("Archive/Old.md", "# Old\n")]);
    let file = unrepresentable("Caf");
    let folder = unrepresentable("D");
    // Another app adds a file and a folder whose names have no UTF-8 form.
    let notes = live.path("Notes");
    std::fs::write(notes.join(&file), "# Odd\n").unwrap();
    std::fs::create_dir(notes.join(&folder)).unwrap();
    std::fs::write(notes.join(&folder).join("inside.md"), "# Inside\n").unwrap();
    live.wait_until("the problem shows", |app| {
        unsupported_problems(app) == [vec!["Notes".to_owned()]]
    });
    let unchanged_in = |place: &Path| {
        assert_eq!(
            std::fs::read_to_string(place.join(&file)).unwrap(),
            "# Odd\n"
        );
        assert_eq!(
            std::fs::read_to_string(place.join(&folder).join("inside.md")).unwrap(),
            "# Inside\n"
        );
    };

    // Renaming and moving the folder that holds them.
    live.app.start_rename("Notes");
    assert!(live.app.commit_rename("Journal"));
    unchanged_in(&live.path("Journal"));
    live.wait_until("the problem follows the renamed folder", |app| {
        unsupported_problems(app) == [vec!["Journal".to_owned()]]
    });
    live.app.move_into("Journal", "Archive");
    unchanged_in(&live.path("Archive/Journal"));
    live.wait_until("the problem follows the moved folder", |app| {
        unsupported_problems(app) == [vec!["Archive/Journal".to_owned()]]
    });
    assert!(
        live.app
            .tree_mut()
            .unwrap()
            .select("Archive/Journal/Kept.md")
    );

    // Moving it to the trash, which is .noura/trash without a system trash.
    live.app.request_trash("Archive/Journal");
    live.app.confirm_trash();
    let trashed: Vec<PathBuf> = std::fs::read_dir(live.path(".noura/trash"))
        .unwrap()
        .map(|entry| entry.unwrap().path().join("Archive/Journal"))
        .collect();
    assert_eq!(trashed.len(), 1);
    unchanged_in(&trashed[0]);
    live.wait_until("the problem leaves with the folder", |app| {
        unsupported_problems(app).is_empty()
    });

    // Importing a folder that holds one copies the rest and leaves that
    // name where it is.
    let album = live.folder.parent().unwrap().join("Album");
    std::fs::create_dir(&album).unwrap();
    std::fs::write(album.join("Shown.md"), "# Shown\n").unwrap();
    std::fs::write(album.join(&file), "# Odd\n").unwrap();
    let imported = live
        .workspace()
        .import(std::slice::from_ref(&album), "")
        .unwrap();
    assert_eq!(imported.paths, ["Album"]);
    assert_eq!(imported.skipped, 1);
    let copied: Vec<_> = std::fs::read_dir(live.path("Album"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(copied, ["Shown.md"]);
    assert_eq!(
        std::fs::read_to_string(album.join(&file)).unwrap(),
        "# Odd\n"
    );
    live.wait_until("the copy shows with no problem", |app| {
        rows(app).contains(&"Album".to_owned()) && unsupported_problems(app).is_empty()
    });
}
