//! Seam 1: the app's view state across launches, against real folders.

use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use app_core::{App, Change, OpenedFile, Screen, WorkspaceService};

/// Poll until background work finishes.
fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        if !app.is_busy() {
            return;
        }
        assert!(Instant::now() < deadline, "background work never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn create(app_data: &Path, name: &str, folder: &Path) -> String {
    std::fs::create_dir_all(folder).unwrap();
    WorkspaceService::new(app_data)
        .create(name, folder)
        .unwrap()
        .id()
        .to_owned()
}

/// Windows can hold a just-closed watcher handle on the folder for a moment.
fn rename_with_retry(from: &Path, to: &Path) {
    for attempt in 0.. {
        match std::fs::rename(from, to) {
            Ok(()) => return,
            Err(_) if attempt < 50 => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => panic!("could not move {from:?}: {error}"),
        }
    }
}

#[test]
fn the_first_launch_shows_the_chooser_without_loading() {
    let app_data = tempfile::tempdir().unwrap();

    let app = App::launch(app_data.path());

    assert_eq!(app.screen(), &Screen::Chooser { problem: None });
    assert!(!app.is_busy());
    assert!(app.recents().is_empty());
}

#[test]
fn launch_reopens_the_last_workspace_in_the_background() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Last");
    let id = create(app_data.path(), "Last", &folder);
    std::fs::write(folder.join("Note.md"), b"x").unwrap();

    let mut app = App::launch(app_data.path());
    let changes = Arc::new(Mutex::new(Vec::new()));
    let seen = changes.clone();
    app.subscribe(move |change| seen.lock().unwrap().push(change));

    assert_eq!(app.screen(), &Screen::Loading);
    settle(&mut app);
    assert_eq!(app.screen(), &Screen::Workspace);
    assert_eq!(app.workspace().unwrap().id(), id);
    let names: Vec<&str> = app
        .tree()
        .unwrap()
        .rows()
        .iter()
        .map(|row| row.name.as_str())
        .collect();
    assert_eq!(names, ["Note"]);
    assert!(changes.lock().unwrap().contains(&Change::Screen));
}

#[test]
fn a_missing_last_workspace_says_why_and_try_again_opens_it_once_it_is_back() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Last");
    let id = create(app_data.path(), "Last", &folder);
    let saved_path = WorkspaceService::new(app_data.path()).recents()[0]
        .path
        .clone();
    let away = parent.path().join("Away");
    rename_with_retry(&folder, &away);

    let mut app = App::launch(app_data.path());
    settle(&mut app);

    assert_eq!(
        app.screen(),
        &Screen::Chooser {
            problem: Some(format!(
                "The folder {saved_path} isn't there. If it's on a drive that isn't \
                 connected, connect the drive and try again."
            ))
        }
    );
    assert_eq!(app.recents().len(), 1);

    rename_with_retry(&away, &folder);
    app.try_again();
    settle(&mut app);

    assert_eq!(app.screen(), &Screen::Workspace);
    assert_eq!(app.workspace().unwrap().id(), id);
}

#[test]
fn a_recent_workspace_that_fails_to_open_offers_remove_from_recents() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let gone = parent.path().join("Gone");
    let kept = parent.path().join("Kept");
    let gone_id = create(app_data.path(), "Gone", &gone);
    let kept_id = create(app_data.path(), "Kept", &kept);
    let mut app = App::launch(app_data.path());
    settle(&mut app);
    rename_with_retry(&gone, &parent.path().join("Moved"));

    app.open_recent(&gone_id);
    settle(&mut app);

    let notice = app.notice().unwrap().clone();
    assert_eq!(notice.title, "Couldn't open \u{201c}Gone\u{201d}");
    assert_eq!(notice.remove_recent.as_deref(), Some(gone_id.as_str()));
    assert_eq!(app.workspace().unwrap().id(), kept_id);

    app.remove_recent(&gone_id);

    let ids: Vec<&str> = app
        .recents()
        .iter()
        .map(|recent| recent.workspace_id.as_str())
        .collect();
    assert_eq!(ids, [kept_id.as_str()]);
    assert!(app.notice().is_none());
    assert!(parent.path().join("Moved/.noura/workspace.yaml").is_file());
}

#[test]
fn the_switcher_moves_to_another_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let first = create(app_data.path(), "First", &parent.path().join("First"));
    create(app_data.path(), "Second", &parent.path().join("Second"));
    let mut app = App::launch(app_data.path());
    settle(&mut app);

    app.open_recent(&first);
    settle(&mut app);

    assert_eq!(app.workspace().unwrap().name(), "First");
    assert_eq!(app.recents()[0].workspace_id, first);
}

#[test]
fn open_folder_and_new_workspace_open_the_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let plain = parent.path().join("Plain");
    let fresh = parent.path().join("Fresh");
    std::fs::create_dir(&plain).unwrap();
    std::fs::create_dir(&fresh).unwrap();
    let mut app = App::launch(app_data.path());

    app.open_folder(plain.clone());
    settle(&mut app);
    assert_eq!(app.screen(), &Screen::Workspace);
    assert_eq!(app.workspace().unwrap().name(), "Plain");

    app.create_workspace("Fresh start", fresh.clone());
    settle(&mut app);
    assert_eq!(app.workspace().unwrap().name(), "Fresh start");
    assert_eq!(app.recents().len(), 2);
}

#[test]
fn a_failed_new_workspace_keeps_the_chooser_and_says_why() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    WorkspaceService::new(tempfile::tempdir().unwrap().path())
        .create("Taken", folder.path())
        .unwrap();
    let mut app = App::launch(app_data.path());

    app.create_workspace("Another", folder.path().to_owned());
    settle(&mut app);

    assert_eq!(app.screen(), &Screen::Chooser { problem: None });
    let notice = app.notice().unwrap();
    assert_eq!(notice.title, "Couldn't create the workspace");
    assert_eq!(
        notice.message,
        "This folder already has a workspace. Use Open folder to open it."
    );
    assert_eq!(notice.remove_recent, None);
}

#[test]
fn opening_a_file_shows_its_name_and_workspace_relative_path() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    std::fs::create_dir(folder.path().join("Docs")).unwrap();
    std::fs::write(folder.path().join("Docs/Guide.md"), b"# Guide\n").unwrap();
    let mut app = App::launch(app_data.path());
    app.open_folder(folder.path().to_owned());
    settle(&mut app);

    app.open_file("Docs/Guide.md");

    assert_eq!(
        app.document(),
        Some(&OpenedFile {
            name: "Guide".into(),
            relative_path: "Docs/Guide.md".into(),
        })
    );
    assert_eq!(
        std::fs::read(folder.path().join("Docs/Guide.md")).unwrap(),
        b"# Guide\n"
    );
}

#[test]
fn opening_a_file_cloud_storage_has_not_downloaded_says_so_and_shows_nothing() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join(".Cloud note.md.icloud"), b"placeholder").unwrap();
    let mut app = App::launch(app_data.path());
    app.open_folder(folder.path().to_owned());
    settle(&mut app);
    assert!(app.tree().unwrap().rows()[0].not_downloaded);

    app.open_file("Cloud note.md");

    assert_eq!(app.document(), None);
    let notice = app.notice().unwrap();
    assert_eq!(notice.title, "Couldn't open \u{201c}Cloud note\u{201d}");
    assert_eq!(
        notice.message,
        "This file isn't downloaded yet. Try again once it is."
    );
}

#[test]
fn try_again_retries_only_the_workspace_that_failed() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create(app_data.path(), "Older", &parent.path().join("Older"));
    let last = parent.path().join("Last");
    let last_id = create(app_data.path(), "Last", &last);
    rename_with_retry(&last, &parent.path().join("Away"));
    let mut app = App::launch(app_data.path());
    settle(&mut app);
    assert!(matches!(app.screen(), Screen::Chooser { problem: Some(_) }));

    app.open_recent(&last_id);
    settle(&mut app);
    assert_eq!(
        app.notice().unwrap().remove_recent.as_deref(),
        Some(last_id.as_str())
    );
    app.remove_recent(&last_id);

    assert_eq!(app.screen(), &Screen::Chooser { problem: None });
    app.try_again();
    assert!(
        !app.is_busy(),
        "Try again must not open the older workspace"
    );
    assert!(app.workspace().is_none());
}

#[test]
fn a_last_workspace_that_lost_its_manifest_says_why_and_stays_as_it_was() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Last");
    create(app_data.path(), "Last", &folder);
    std::fs::remove_file(folder.join(".noura/workspace.yaml")).unwrap();
    let saved_path = WorkspaceService::new(app_data.path()).recents()[0]
        .path
        .clone();

    let mut app = App::launch(app_data.path());
    settle(&mut app);

    assert_eq!(
        app.screen(),
        &Screen::Chooser {
            problem: Some(format!(
                "The settings file .noura/workspace.yaml is missing from {saved_path}. Noura \
                 left the folder as it was. Choose Open folder to use it as a new workspace."
            ))
        }
    );
    assert!(!folder.join(".noura/workspace.yaml").exists());
}
