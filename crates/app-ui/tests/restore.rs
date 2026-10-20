//! Reopening the last workspace at launch, through AccessKit.

mod support;

use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui_kittest::kittest::Queryable;
use support::{FakePicker, create_workspace, harness, settle};

/// Windows can hold a just-closed watcher handle on the folder for a moment.
fn rename_with_retry(from: &std::path::Path, to: &std::path::Path) {
    for attempt in 0.. {
        match std::fs::rename(from, to) {
            Ok(()) => return,
            Err(_) if attempt < 50 => std::thread::sleep(Duration::from_millis(100)),
            Err(error) => panic!("could not move {from:?}: {error}"),
        }
    }
}

#[test]
fn launch_shows_a_loading_state_and_never_flashes_the_chooser() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Big");
    create_workspace(app_data.path(), "Big", &folder);
    for index in 0..300 {
        std::fs::write(folder.join(format!("Note {index}.md")), b"x").unwrap();
    }

    let mut harness = harness(app_data.path(), FakePicker::default());
    harness.step();
    // The walk of 300 files outlasts the first frame on any machine we
    // test on; the check below still holds if it doesn't.
    if harness.state().is_busy() {
        harness.get_by_label("Opening workspace");
    }
    let deadline = Instant::now() + Duration::from_secs(30);
    while harness.state().is_busy() {
        assert!(
            harness.query_by_label("Open folder…").is_none(),
            "the chooser flashed while the workspace reopened"
        );
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(5));
        harness.step();
    }
    harness.run();

    harness.get_by_role_and_label(Role::TreeItem, "Note 0");
}

#[test]
fn a_missing_last_workspace_says_why_and_try_again_opens_it_once_it_is_back() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Field notes");
    create_workspace(app_data.path(), "Field notes", &folder);
    std::fs::write(folder.join("Birds.md"), b"x").unwrap();
    let saved_path = app_core::WorkspaceService::new(app_data.path()).recents()[0]
        .path
        .clone();
    let away = parent.path().join("Unplugged");
    rename_with_retry(&folder, &away);

    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);

    harness.get_by_label("Can't open the workspace");
    harness.get_by_label(&format!(
        "The folder {saved_path} isn't there. If it's on a drive that isn't connected, \
         connect the drive and try again."
    ));
    harness.get_by_role_and_label(Role::Button, "Open folder…");
    harness.get_by_role_and_label(Role::Button, "Field notes");
    assert!(harness.unnamed_widgets().is_empty());

    rename_with_retry(&away, &folder);
    harness
        .get_by_role_and_label(Role::Button, "Try again")
        .click();
    settle(&mut harness);

    harness.get_by_role_and_label(Role::TreeItem, "Birds");
}

#[test]
fn a_recent_workspace_that_fails_to_open_offers_remove_from_recents() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let gone = parent.path().join("Gone");
    create_workspace(app_data.path(), "Gone", &gone);
    create_workspace(app_data.path(), "Last", &parent.path().join("Last"));
    rename_with_retry(
        &parent.path().join("Last"),
        &parent.path().join("Last moved"),
    );
    rename_with_retry(&gone, &parent.path().join("Gone moved"));
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);

    harness.get_by_role_and_label(Role::Button, "Gone").click();
    settle(&mut harness);

    harness.get_by_label("Couldn't open \u{201c}Gone\u{201d}");
    harness
        .get_by_role_and_label(Role::Button, "Remove from recents")
        .click();
    harness.run();

    assert!(harness.query_by_label("Gone").is_none());
    assert!(
        parent
            .path()
            .join("Gone moved/.noura/workspace.yaml")
            .is_file()
    );
}

#[test]
fn removing_the_workspace_that_failed_clears_the_problem_and_try_again() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Older", &parent.path().join("Older"));
    let last = parent.path().join("Last");
    create_workspace(app_data.path(), "Last", &last);
    rename_with_retry(&last, &parent.path().join("Away"));
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    harness.get_by_label("Can't open the workspace");

    harness.get_by_role_and_label(Role::Button, "Last").click();
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::Button, "Remove from recents")
        .click();
    harness.run();

    assert!(harness.query_by_label("Can't open the workspace").is_none());
    assert!(harness.query_by_label("Try again").is_none());
    harness.get_by_label("Open a folder");
    harness.get_by_role_and_label(Role::Button, "Older");
    assert!(!harness.state().is_busy());
}
