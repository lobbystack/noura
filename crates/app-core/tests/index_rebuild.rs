//! Seam 1: the index is a disposable copy of what the files say. Deleting
//! it, breaking it, or crashing while Noura writes it gives you the same
//! workspace back at the next open.

use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use app_core::{App, Problem, Screen};

/// The files of the shared queries-v1 fixture: notes, tasks and a project,
/// two notes that share an ID, a broken note, a hidden note and an image.
fn fixture_files() -> Vec<(String, String)> {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/workspace-format/fixtures/queries-v1.json"
    );
    let fixture: serde_json::Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
    fixture["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap().to_owned(),
                file["content"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

fn write(root: &Path, relative: &str, text: &str) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A workspace folder on disk and the device's app data, which outlive
/// each app that opens them.
struct Device {
    app_data: tempfile::TempDir,
    _parent: tempfile::TempDir,
    folder: PathBuf,
}

impl Device {
    fn with_fixture() -> Self {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        for (relative, text) in fixture_files() {
            write(&folder, &relative, &text);
        }
        Self {
            app_data,
            _parent: parent,
            folder,
        }
    }

    /// Launch Noura. The first launch opens the folder; later launches
    /// reopen it as the last workspace.
    fn launch(&self) -> App {
        let mut app = App::launch(self.app_data.path());
        if app.screen() != &Screen::Loading {
            app.open_folder(self.folder.clone());
        }
        let deadline = Instant::now() + Duration::from_secs(20);
        while app.is_busy() || app.screen() == &Screen::Loading {
            assert!(Instant::now() < deadline, "the workspace never opened");
            std::thread::sleep(Duration::from_millis(5));
            app.poll();
        }
        assert_eq!(app.screen(), &Screen::Workspace, "{:?}", app.notice());
        app
    }

    /// This workspace's folder of per-device state, where its index lives.
    fn local_dir(&self, app: &App) -> PathBuf {
        self.app_data
            .path()
            .join("workspaces")
            .join(app.workspace().unwrap().id())
    }
}

/// Every file whose name starts with `index.sqlite` in `dir`: the index and
/// whatever SQLite or an interrupted rebuild left next to it.
fn index_files(dir: &Path) -> Vec<PathBuf> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("index.sqlite"))
        })
        .collect();
    files.sort();
    files
}

/// What an unfinished rebuild left behind. The index itself, and the
/// `-wal` and `-shm` files SQLite keeps beside it while it's open, aren't
/// leftovers.
fn leftovers(dir: &Path) -> Vec<String> {
    let names: Vec<String> = index_files(dir)
        .iter()
        .map(|path| path.file_name().unwrap().to_str().unwrap().to_owned())
        .collect();
    assert!(names.contains(&"index.sqlite".to_owned()), "{names:?}");
    names
        .into_iter()
        .filter(|name| {
            !matches!(
                name.as_str(),
                "index.sqlite" | "index.sqlite-wal" | "index.sqlite-shm"
            )
        })
        .collect()
}

/// What you see of the workspace: the whole tree, the objects, search
/// results and problems.
#[derive(Debug, PartialEq)]
struct Seen {
    tree: Vec<(String, bool)>,
    objects: Vec<(String, String, String)>,
    search: Vec<(String, Vec<(Option<String>, String, String)>)>,
    problems: Vec<Problem>,
}

const QUERIES: [&str; 6] = ["release", "plan", "copy", "loose", "secret", "draft"];

fn seen(app: &mut App) -> Seen {
    let tree = app.tree_mut().unwrap();
    while let Some(closed) = tree
        .rows()
        .iter()
        .position(|row| row.expanded == Some(false))
    {
        tree.toggle(closed);
    }
    let tree = tree
        .rows()
        .iter()
        .map(|row| (row.relative_path.clone(), row.not_downloaded))
        .collect();
    let workspace = app.workspace().unwrap();
    let objects = workspace
        .objects()
        .unwrap()
        .into_iter()
        .map(|object| (object.id, object.object_type, object.relative_path))
        .collect();
    let search = QUERIES
        .iter()
        .map(|query| {
            let mut hits: Vec<_> = workspace
                .search(query)
                .unwrap()
                .into_iter()
                .map(|hit| (hit.object_id, hit.relative_path, hit.title))
                .collect();
            hits.sort();
            ((*query).to_owned(), hits)
        })
        .collect();
    Seen {
        tree,
        objects,
        search,
        problems: app.problems().to_vec(),
    }
}

/// Every file under `folder` with its bytes, the workspace settings
/// included.
fn file_bytes(folder: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut files = Vec::new();
    let mut pending = vec![folder.to_owned()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                files.push((path, bytes));
            }
        }
    }
    files.sort();
    files
}

#[test]
fn deleting_every_index_file_rebuilds_the_same_workspace() {
    let device = Device::with_fixture();
    let mut app = device.launch();
    let before = seen(&mut app);
    let bytes = file_bytes(&device.folder);
    let local = device.local_dir(&app);
    drop(app);
    assert!(!before.objects.is_empty() && !before.problems.is_empty());

    for file in index_files(&local) {
        std::fs::remove_file(file).unwrap();
    }
    let mut app = device.launch();

    assert_eq!(seen(&mut app), before);
    assert_eq!(leftovers(&local), Vec::<String>::new());
    drop(app);
    assert!(
        file_bytes(&device.folder) == bytes,
        "the rebuild changed a file"
    );
}

#[test]
fn a_rebuild_interrupted_midway_leaves_the_previous_index_and_the_next_open_finishes_it() {
    let device = Device::with_fixture();
    let mut app = device.launch();
    let before = seen(&mut app);
    let local = device.local_dir(&app);
    drop(app);

    // Noura crashed while it built the new index beside the old one.
    std::fs::write(local.join("index.sqlite.next"), b"half a database").unwrap();
    std::fs::write(local.join("index.sqlite.next-wal"), b"half a log").unwrap();
    let mut app = device.launch();

    assert_eq!(seen(&mut app), before);
    assert_eq!(leftovers(&local), Vec::<String>::new());
}

#[test]
fn a_rebuild_interrupted_while_it_swapped_the_index_in_finishes_at_the_next_open() {
    let device = Device::with_fixture();
    let mut app = device.launch();
    let before = seen(&mut app);
    let local = device.local_dir(&app);
    drop(app);

    // Noura crashed after it set the old index aside, before the new one
    // took its place.
    std::fs::copy(local.join("index.sqlite"), local.join("index.sqlite.next")).unwrap();
    std::fs::rename(
        local.join("index.sqlite"),
        local.join("index.sqlite.previous"),
    )
    .unwrap();
    let mut app = device.launch();

    assert_eq!(seen(&mut app), before);
    assert_eq!(leftovers(&local), Vec::<String>::new());
}

#[test]
fn a_first_rebuild_interrupted_midway_starts_over_at_the_next_open() {
    let device = Device::with_fixture();
    let mut app = device.launch();
    let before = seen(&mut app);
    let local = device.local_dir(&app);
    drop(app);

    // You deleted the index, and Noura crashed while it built a new one.
    for file in index_files(&local) {
        std::fs::remove_file(file).unwrap();
    }
    std::fs::write(local.join("index.sqlite.next"), b"half a database").unwrap();
    let mut app = device.launch();

    assert_eq!(seen(&mut app), before);
    assert_eq!(leftovers(&local), Vec::<String>::new());
}

#[test]
fn an_index_that_is_not_a_database_anymore_is_rebuilt_from_the_files() {
    let device = Device::with_fixture();
    let mut app = device.launch();
    let before = seen(&mut app);
    let local = device.local_dir(&app);
    drop(app);

    for file in index_files(&local) {
        std::fs::remove_file(file).unwrap();
    }
    std::fs::write(local.join("index.sqlite"), vec![0x5a; 8192]).unwrap();
    let mut app = device.launch();

    assert_eq!(seen(&mut app), before);
}

const SHIP_ID: &str = "task_01k00000000000000000000001";

/// Edit, move, delete and add files the way another app would.
fn change_files_outside(folder: &Path) {
    let plan = std::fs::read_to_string(folder.join("Notes/plan.md"))
        .unwrap()
        .replace("The release plan.", "The saffron plan.");
    std::fs::write(folder.join("Notes/plan.md"), plan).unwrap();
    std::fs::create_dir(folder.join("Done")).unwrap();
    std::fs::rename(folder.join("Tasks/ship.md"), folder.join("Done/ship.md")).unwrap();
    std::fs::remove_file(folder.join("Notes/idless.md")).unwrap();
    write(folder, "Inbox.md", "# Inbox\n\nSaffron buns.\n");
}

fn hit_paths(app: &App, query: &str) -> Vec<String> {
    let mut paths: Vec<String> = app
        .workspace()
        .unwrap()
        .search(query)
        .unwrap()
        .into_iter()
        .map(|hit| hit.relative_path)
        .collect();
    paths.sort();
    paths
}

/// Whether `app` shows what [`change_files_outside`] did.
fn shows_outside_changes(app: &mut App) -> bool {
    let tree = seen(app).tree;
    let listed = |path: &str| tree.contains(&(path.to_owned(), false));
    let ship = app.workspace().unwrap().find_object(SHIP_ID).unwrap();
    listed("Done/ship.md")
        && listed("Inbox.md")
        && !listed("Tasks/ship.md")
        && !listed("Notes/idless.md")
        && ship.is_some_and(|ship| ship.relative_path == "Done/ship.md")
        && hit_paths(app, "saffron") == ["Inbox.md", "Notes/plan.md"]
        && hit_paths(app, "loose").is_empty()
}

#[test]
fn opening_picks_up_edits_moves_deletions_and_new_files_made_while_noura_was_closed() {
    let device = Device::with_fixture();
    drop(device.launch());

    change_files_outside(&device.folder);
    let mut app = device.launch();

    assert!(shows_outside_changes(&mut app), "{:#?}", seen(&mut app));
}

#[test]
fn opening_repairs_index_updates_that_a_crash_cut_short() {
    let device = Device::with_fixture();
    let app = device.launch();
    let local = device.local_dir(&app);
    drop(app);
    let saved = tempfile::tempdir().unwrap();
    for file in index_files(&local) {
        std::fs::copy(&file, saved.path().join(file.file_name().unwrap())).unwrap();
    }
    let mut app = device.launch();
    change_files_outside(&device.folder);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !shows_outside_changes(&mut app) {
        assert!(
            Instant::now() < deadline,
            "the outside changes never showed"
        );
        std::thread::sleep(Duration::from_millis(20));
        app.poll();
    }
    let expected = seen(&mut app);
    drop(app);

    // Noura crashed before its index updates reached the disk, so the index
    // on disk is the one from before the changes.
    for file in index_files(&local) {
        std::fs::remove_file(file).unwrap();
    }
    for file in index_files(saved.path()) {
        std::fs::copy(&file, local.join(file.file_name().unwrap())).unwrap();
    }
    let mut app = device.launch();

    assert_eq!(seen(&mut app), expected);
}
