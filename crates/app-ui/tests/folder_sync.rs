//! Settings' Sync section through AccessKit (sync-v2.md §15, §16): whether
//! the open workspace syncs, whether sync runs, the changes waiting to
//! upload, the conflicts with a button for each version, the last error
//! and the last successful sync, with Pause and Resume.

mod support;

use std::sync::{Arc, Mutex};

use app_core::folder_sync::{
    ArchivedEntry, BranchEntry, ConflictEntry, FolderSyncAction, FolderSyncBackend,
    FolderSyncPhase, FolderSyncScreen, HeldChoice, HeldEntry, HeldRootEntry, Id, OpKey,
    StoppedNoteEntry,
};
use app_core::{App, settings_section};
use app_ui::NouraUi;
use egui::accesskit::Role;
use egui_kittest::{Harness, kittest::Queryable};
use support::FakePicker;

/// A sync backend whose screen the test sets and that records what you do.
#[derive(Default)]
struct FakeSync {
    screen: Mutex<FolderSyncScreen>,
    actions: Mutex<Vec<FolderSyncAction>>,
}

impl FakeSync {
    fn show(&self, screen: FolderSyncScreen) {
        *self.screen.lock().unwrap() = screen;
    }

    fn taken(&self) -> Vec<FolderSyncAction> {
        std::mem::take(&mut *self.actions.lock().unwrap())
    }
}

impl FolderSyncBackend for FakeSync {
    fn screen(&self) -> FolderSyncScreen {
        self.screen.lock().unwrap().clone()
    }

    fn act(&self, action: FolderSyncAction) {
        self.actions.lock().unwrap().push(action);
    }

    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}

    fn attach(&self, _engine: Option<Arc<app_core::folder_sync::WorkspaceEngine>>) {}
}

fn harness(app_data: &std::path::Path, sync: Arc<FakeSync>) -> Harness<'static, NouraUi> {
    let mut app = App::launch(app_data);
    app.set_folder_sync_backend(sync);
    app.open_settings_at(settings_section::SYNC);
    let ui = NouraUi::new(app, Box::new(FakePicker::default()));
    let mut harness = Harness::builder()
        .with_size([1000.0, 900.0])
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui);
    harness.run();
    harness
}

#[test]
fn a_workspace_that_doesnt_sync_offers_to_turn_it_on() {
    let app_data = tempfile::tempdir().unwrap();
    let sync = Arc::new(FakeSync::default());
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::Off,
        ..FolderSyncScreen::default()
    });
    let mut harness = harness(app_data.path(), sync.clone());
    harness.get_by_label("This workspace doesn't sync. Its files stay on this computer.");
    harness
        .get_by_role_and_label(Role::Button, "Turn on sync")
        .click();
    harness.run();
    assert_eq!(sync.taken(), vec![FolderSyncAction::TurnOn]);
}

#[test]
fn a_running_sync_shows_its_status_and_pauses() {
    let app_data = tempfile::tempdir().unwrap();
    let sync = Arc::new(FakeSync::default());
    let key = OpKey {
        device: Id([0xab; 16]),
        counter: 3,
    };
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::On,
        pending: 2,
        conflicts: vec![ConflictEntry {
            rid: Id([1; 16]),
            path: "Notes/Plan.md".into(),
            branches: vec![
                BranchEntry {
                    label: "Keep this computer's version".into(),
                    key: OpKey {
                        device: Id([0xcd; 16]),
                        counter: 4,
                    },
                },
                BranchEntry {
                    label: "Keep the version from device abababab".into(),
                    key,
                },
            ],
        }],
        skipped: vec!["Notes/CON: its name doesn't work on every computer".into()],
        last_error: Some(
            "The sync server can't be reached. Your changes wait on this computer.".into(),
        ),
        last_success: Some("2026-10-24 14:05".into()),
        ..FolderSyncScreen::default()
    });
    let mut harness = harness(app_data.path(), sync.clone());
    harness.get_by_label("Sync is on.");
    harness.get_by_label("2 changes wait to upload.");
    harness.get_by_label("Last synced 2026-10-24 14:05.");
    harness.get_by_role_and_label(
        Role::Alert,
        "The sync server can't be reached. Your changes wait on this computer.",
    );
    harness.get_by_role_and_label(Role::Group, "1 conflict");
    harness.get_by_label("Notes/Plan.md has versions from two computers. Choose the one to keep:");
    harness.get_by_label("Notes/CON: its name doesn't work on every computer");
    harness
        .get_by_role_and_label(Role::Button, "Keep the version from device abababab")
        .click();
    harness.run();
    assert_eq!(
        sync.taken(),
        vec![FolderSyncAction::Choose {
            rid: Id([1; 16]),
            key
        }]
    );
    harness
        .get_by_role_and_label(Role::Button, "Pause sync")
        .click();
    harness.run();
    assert_eq!(sync.taken(), vec![FolderSyncAction::Pause]);
}

#[test]
fn a_paused_sync_says_so_and_resumes() {
    let app_data = tempfile::tempdir().unwrap();
    let sync = Arc::new(FakeSync::default());
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::Paused,
        pending: 1,
        ..FolderSyncScreen::default()
    });
    let mut harness = harness(app_data.path(), sync.clone());
    harness.get_by_label("Sync is paused. Your changes wait on this computer until you resume.");
    harness.get_by_label("1 change waits to upload.");
    harness.get_by_label("This computer hasn't finished a sync yet.");
    harness.get_by_role_and_label(Role::Group, "No conflicts");
    harness
        .get_by_role_and_label(Role::Button, "Resume sync")
        .click();
    harness.run();
    assert_eq!(sync.taken(), vec![FolderSyncAction::Resume]);
    harness
        .get_by_role_and_label(Role::Button, "Turn off sync")
        .click();
    harness.run();
    assert_eq!(sync.taken(), vec![FolderSyncAction::TurnOff]);
}

#[test]
fn versions_held_after_a_lost_journal_wait_for_a_choice() {
    let app_data = tempfile::tempdir().unwrap();
    let sync = Arc::new(FakeSync::default());
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::On,
        held: vec![HeldEntry {
            rid: Id([2; 16]),
            path: "Plan.md".into(),
        }],
        held_roots: vec![HeldRootEntry {
            folder: "Private".into(),
            files: vec!["Private/Secret.md".into()],
            people: 3,
        }],
        archived: vec![ArchivedEntry {
            index: 0,
            path: "Old plan.md".into(),
        }],
        ..FolderSyncScreen::default()
    });
    let mut harness = harness(app_data.path(), sync.clone());
    harness.get_by_role_and_label(Role::Group, "Versions to check");
    harness.get_by_label("Plan.md differs from the synced version. Choose which to keep:");
    harness
        .get_by_role_and_label(Role::Button, "Keep both")
        .click();
    harness.run();
    assert_eq!(
        sync.taken(),
        vec![FolderSyncAction::KeepHeld {
            rid: Id([2; 16]),
            choice: HeldChoice::KeepBoth
        }]
    );
    harness.get_by_label(
        "Private has a .noura-domain file, so its files wait here. Syncing them shows them to 3 people in this workspace:",
    );
    harness.get_by_label("Private/Secret.md");
    harness
        .get_by_role_and_label(Role::Button, "Sync the files in Private")
        .click();
    harness.run();
    assert_eq!(
        sync.taken(),
        vec![FolderSyncAction::ConfirmFolder("Private".into())]
    );
    harness.get_by_role_and_label(Role::Group, "Versions from an earlier sync");

    // Alone in the section, the earlier sync's version sits in view.
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::On,
        archived: vec![ArchivedEntry {
            index: 0,
            path: "Old plan.md".into(),
        }],
        ..FolderSyncScreen::default()
    });
    harness.run();
    harness.get_by_label("Old plan.md");
    harness
        .get_by_role_and_label(Role::Button, "Save as a copy")
        .click();
    harness.run();
    assert_eq!(sync.taken(), vec![FolderSyncAction::RestoreArchived(0)]);
}

#[test]
fn a_held_removal_and_unreadable_versions_show() {
    let app_data = tempfile::tempdir().unwrap();
    let sync = Arc::new(FakeSync::default());
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::On,
        removal_hold: true,
        unreadable: vec!["Plan.md: a version from device abababab couldn't be read, so this computer keeps the last version that could.".into()],
        ..FolderSyncScreen::default()
    });
    let harness = harness(app_data.path(), sync);
    harness.get_by_role_and_label(
        Role::Alert,
        "A device left your account. Sync holds your changes until this workspace records it and changes its key.",
    );
    harness.get_by_role_and_label(Role::Group, "Versions that couldn't be read");
    harness.get_by_label("Plan.md: a version from device abababab couldn't be read, so this computer keeps the last version that could.");
}

#[test]
fn a_removed_account_says_who_removed_it_and_offers_no_pause() {
    let app_data = tempfile::tempdir().unwrap();
    let sync = Arc::new(FakeSync::default());
    let line = "Account 1a2b3c4d removed your account from this workspace, from device 5e6f7a8b. Sync stopped. Your files stay on this computer as they are.";
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::On,
        ended: Some(line.into()),
        ..FolderSyncScreen::default()
    });
    let harness = harness(app_data.path(), sync);
    harness.get_by_role_and_label(Role::Alert, line);
    assert!(
        harness
            .query_by_role_and_label(Role::Button, "Pause sync")
            .is_none()
    );
}

#[test]
fn a_copied_folder_offers_to_connect() {
    let app_data = tempfile::tempdir().unwrap();
    let sync = Arc::new(FakeSync::default());
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::NotConnected,
        ..FolderSyncScreen::default()
    });
    let mut harness = harness(app_data.path(), sync.clone());
    harness
        .get_by_role_and_label(Role::Button, "Connect this folder")
        .click();
    harness.run();
    assert_eq!(sync.taken(), vec![FolderSyncAction::Connect]);
}

#[test]
fn a_note_that_stopped_merging_waits_for_your_choice_and_left_out_edits_show() {
    let app_data = tempfile::tempdir().unwrap();
    let sync = Arc::new(FakeSync::default());
    let rid = Id([3; 16]);
    let moved = Id([4; 16]);
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::On,
        stopped_notes: vec![
            StoppedNoteEntry {
                rid,
                path: "Plan.md".into(),
                outside: false,
            },
            StoppedNoteEntry {
                rid: moved,
                path: "Pay/Rates.md".into(),
                outside: true,
            },
        ],
        text_reviews: vec![
            "Plan.md: a version from device 1a2b3c4d disagreed with this computer's records, and Noura rebuilt the note.".into(),
        ],
        ..FolderSyncScreen::default()
    });
    let mut harness = harness(app_data.path(), sync.clone());
    harness.get_by_label("A collaborator's edit stopped Plan.md from merging on this computer.");
    harness.get_by_label(
        "A rename took Pay/Rates.md outside its shared folder, so its file stays where it is.",
    );
    harness.get_by_label(
        "Plan.md: a version from device 1a2b3c4d disagreed with this computer's records, and Noura rebuilt the note.",
    );
    harness
        .get_all_by_role_and_label(Role::Button, "Keep the last good version")
        .next()
        .unwrap()
        .click();
    harness.run();
    assert_eq!(sync.taken(), vec![FolderSyncAction::KeepLastGood(rid)]);
}

#[test]
fn changes_waiting_for_space_replace_the_uploaded_line() {
    let app_data = tempfile::tempdir().unwrap();
    let sync = Arc::new(FakeSync::default());
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::On,
        waiting_for_space: 2,
        ..FolderSyncScreen::default()
    });
    let harness = harness(app_data.path(), sync);
    harness.get_by_label("2 changes stay on this computer until the workspace has space.");
    assert!(
        harness
            .query_by_label("Every change is uploaded.")
            .is_none(),
        "the change hasn't reached the server"
    );
}

#[test]
fn a_file_deleted_while_away_offers_delete_and_keep() {
    let app_data = tempfile::tempdir().unwrap();
    let sync = Arc::new(FakeSync::default());
    sync.show(FolderSyncScreen {
        phase: FolderSyncPhase::On,
        deleted_away: vec!["Notes/Old.md".into()],
        ..FolderSyncScreen::default()
    });
    let mut harness = harness(app_data.path(), sync.clone());
    harness.get_by_role_and_label(
        Role::Group,
        "Deleted on another device while this one was away",
    );
    harness.get_by_label("Notes/Old.md");
    harness
        .get_by_role_and_label(Role::Button, "Delete")
        .click();
    harness.run();
    assert_eq!(
        sync.taken(),
        vec![FolderSyncAction::ConfirmDeletedAway("Notes/Old.md".into())]
    );
    harness.get_by_role_and_label(Role::Button, "Keep").click();
    harness.run();
    assert_eq!(
        sync.taken(),
        vec![FolderSyncAction::KeepDeletedAway("Notes/Old.md".into())]
    );
}
