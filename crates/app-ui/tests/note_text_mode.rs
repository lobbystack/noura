//! A text-mode note in the window (sync-v2.md §22, §23): an edit asks for
//! the note's document, the editor shows what the document merges and the
//! collaborators' cursors, and it goes back to its file when the document
//! ends, stops or moves on. The sync thread's side runs in `app-core`'s
//! tests; here a test plays it through the bridge.

mod support;

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use app_core::folder_sync::{FolderSyncAction, FolderSyncBackend, FolderSyncScreen, Id};
use app_core::sync::workspace::presence::{Cursor, SelectionRange};
use app_core::sync::workspace::text::{LeftNote, LeftWhy};
use app_core::text_bridge::TextBridge;
use app_core::{App, Platform};
use egui::accesskit::Role;
use egui_kittest::{Harness, kittest::Queryable};
use support::{create_workspace, harness_for, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

const RID: Id = Id([7; 16]);

#[derive(Default)]
struct Bridged {
    bridge: Arc<TextBridge>,
}

impl FolderSyncBackend for Bridged {
    fn screen(&self) -> FolderSyncScreen {
        FolderSyncScreen::default()
    }

    fn act(&self, _action: FolderSyncAction) {}

    fn set_wake(&self, _wake: Arc<dyn Fn() + Send + Sync>) {}

    fn attach(&self, _engine: Option<Arc<app_core::folder_sync::WorkspaceEngine>>) {}

    fn text_bridge(&self) -> Option<Arc<TextBridge>> {
        Some(self.bridge.clone())
    }
}

/// A workspace holding `Plan.md` with `text`, open in a window whose sync
/// thread the test plays.
fn opened(app_data: &Path, folder: &Path, text: &str) -> (Ui, Arc<TextBridge>) {
    create_workspace(app_data, "Garden", folder);
    std::fs::write(folder.join("Plan.md"), text).unwrap();
    let backend = Arc::new(Bridged::default());
    let bridge = backend.bridge.clone();
    let mut harness = harness_for(app_data, Platform::Windows, move |app: &mut App| {
        app.set_folder_sync_backend(backend);
    });
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
    (harness, bridge)
}

fn shown(harness: &Ui) -> String {
    harness
        .get_by_role(Role::MultilineTextInput)
        .value()
        .unwrap_or_default()
}

fn is_unsaved(harness: &Ui) -> bool {
    let app = harness.state().app();
    let tab = app.active_tab().expect("a tab").id();
    app.note(tab).expect("a note").is_unsaved()
}

fn in_text_mode(harness: &Ui) -> bool {
    let app = harness.state().app();
    let tab = app.active_tab().expect("a tab").id();
    app.note(tab).expect("a note").text_mode().is_some()
}

fn type_in(harness: &mut Ui, text: &str) {
    harness.get_by_role(Role::MultilineTextInput).focus();
    harness.run_steps(2);
    harness.event(egui::Event::Text(text.to_owned()));
    harness.run_steps(3);
}

/// Types into the note, and hands the window the document it asks for.
fn enter(harness: &mut Ui, bridge: &TextBridge) -> app_core::text_bridge::TestNote {
    type_in(harness, "x");
    assert!(
        bridge
            .take_opening_for_testing()
            .iter()
            .any(|path| path == "Plan.md"),
        "the edit asked for the document"
    );
    // The first edit's save lands, and the document holds what it wrote.
    let until = Instant::now() + Duration::from_secs(5);
    while is_unsaved(harness) && Instant::now() < until {
        harness.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    let (document, shared) = TextBridge::document_for_testing(RID, &shown(harness));
    bridge.deliver_for_testing("Plan.md", document);
    harness.run_steps(3);
    assert!(in_text_mode(harness));
    shared
}

/// Types into the note, and leaves a document waiting that can't take over
/// yet: the note stays outside text mode.
fn wait_for_a_document(harness: &mut Ui, bridge: &TextBridge) {
    type_in(harness, "x");
    let until = Instant::now() + Duration::from_secs(5);
    while is_unsaved(harness) && Instant::now() < until {
        harness.run_steps(1);
        std::thread::sleep(Duration::from_millis(5));
    }
    let (document, _shared) = TextBridge::document_for_testing(RID, "some other text");
    bridge.deliver_for_testing("Plan.md", document);
    harness.run_steps(3);
    assert!(!in_text_mode(harness));
    bridge.take_opening_for_testing();
}

fn wait_for_file(folder: &Path, text: &str, harness: &mut Ui) {
    let until = Instant::now() + Duration::from_secs(5);
    while Instant::now() < until {
        harness.run_steps(1);
        if std::fs::read_to_string(folder.join("Plan.md")).unwrap() == text {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(
        std::fs::read_to_string(folder.join("Plan.md")).unwrap(),
        text
    );
}

/// Closing the tab tells the sync thread nobody will take the document.
#[test]
fn closing_a_tab_lets_go_of_a_document_that_waits_for_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let (mut harness, bridge) = opened(app_data.path(), &folder, "# Plan\n");
    wait_for_a_document(&mut harness, &bridge);
    assert!(bridge.take_forgotten_for_testing().is_empty());

    let tab = harness.state().app().active_tab().expect("a tab").id();
    harness.state_mut().app_mut().close_tab(tab);
    harness.run_steps(3);

    assert_eq!(bridge.take_forgotten_for_testing(), ["Plan.md"]);
}

/// A rename forgets the old path and asks for the document at the new one.
#[test]
fn renaming_a_note_asks_for_its_document_at_the_new_path() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let (mut harness, bridge) = opened(app_data.path(), &folder, "# Plan\n");
    wait_for_a_document(&mut harness, &bridge);

    let name = harness.get_by_role_and_label(Role::TextInput, "File name");
    name.focus();
    harness.run_steps(2);
    harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
    harness.event(egui::Event::Text("Moved".to_owned()));
    harness.run_steps(2);
    harness.key_press(egui::Key::Enter);
    harness.run_steps(4);
    assert!(folder.join("Moved.md").exists());

    assert_eq!(bridge.take_forgotten_for_testing(), ["Plan.md"]);
    assert_eq!(bridge.take_opening_for_testing(), ["Moved.md"]);
}

#[test]
fn a_first_edit_brings_the_document_and_the_editor_follows_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let (mut harness, bridge) = opened(app_data.path(), &folder, "# Plan\n");
    let shared = enter(&mut harness, &bridge);
    assert_eq!(shared.merged_text().unwrap(), shown(&harness));

    // A collaborator's edit reaches the editor.
    let merged = shared.merged_text().unwrap();
    shared
        .merge_text(merged.clone(), format!("{merged}more\n"), 0)
        .expect("merged");
    harness.run_steps(3);
    assert_eq!(shown(&harness), format!("{merged}more\n"));

    // So does their cursor, which reaches screen readers too.
    bridge.set_cursors_for_testing(
        RID,
        vec![Cursor {
            device: Id([9; 16]),
            color: 2,
            active: true,
            selections: vec![SelectionRange { anchor: 1, head: 3 }],
        }],
    );
    harness.run_steps(3);
    harness.get_by_role_and_label(Role::Caret, "A collaborator's cursor");
}

#[test]
fn a_document_a_file_operation_ended_leaves_your_typing_to_save_as_a_file() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let (mut harness, bridge) = opened(app_data.path(), &folder, "# Plan\n");
    enter(&mut harness, &bridge);
    let merged = shown(&harness);
    type_in(&mut harness, "y");
    let typed = shown(&harness);
    assert_ne!(typed, merged);

    // The document merged up to `merged`, and the file holds that.
    std::fs::write(folder.join("Plan.md"), &merged).unwrap();
    bridge.leave_for_testing(LeftNote {
        rid: RID,
        why: LeftWhy::Ended,
        body: merged,
    });
    harness.run_steps(3);
    assert!(!in_text_mode(&harness));
    assert_eq!(shown(&harness), typed, "your typing stays");
    wait_for_file(&folder, &typed, &mut harness);

    // Edits save to the file from here on.
    type_in(&mut harness, "z");
    let more = shown(&harness);
    wait_for_file(&folder, &more, &mut harness);
}

#[test]
fn a_document_that_ended_shows_what_the_file_holds_now() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let (mut harness, bridge) = opened(app_data.path(), &folder, "# Plan\n");
    enter(&mut harness, &bridge);
    let merged = shown(&harness);
    std::fs::write(folder.join("Plan.md"), "theirs\n").unwrap();
    bridge.leave_for_testing(LeftNote {
        rid: RID,
        why: LeftWhy::Ended,
        body: merged,
    });
    harness.run_steps(4);
    assert!(!in_text_mode(&harness));
    assert_eq!(shown(&harness), "theirs\n");
}

#[test]
fn a_stopped_document_leaves_the_file_at_its_last_good_state() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let (mut harness, bridge) = opened(app_data.path(), &folder, "# Plan\n");
    enter(&mut harness, &bridge);
    let merged = shown(&harness);
    std::fs::write(folder.join("Plan.md"), &merged).unwrap();
    bridge.leave_for_testing(LeftNote {
        rid: RID,
        why: LeftWhy::Stopped,
        body: merged.clone(),
    });
    harness.run_steps(3);
    assert!(!in_text_mode(&harness));
    assert_eq!(shown(&harness), merged);
    assert!(
        bridge.take_opening_for_testing().is_empty(),
        "the note waits for your choice in Settings"
    );
}

#[test]
fn a_document_that_moved_on_brings_the_note_back_to_the_new_one() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let (mut harness, bridge) = opened(app_data.path(), &folder, "# Plan\n");
    enter(&mut harness, &bridge);
    let merged = shown(&harness);
    std::fs::write(folder.join("Plan.md"), &merged).unwrap();
    bridge.leave_for_testing(LeftNote {
        rid: RID,
        why: LeftWhy::Moved,
        body: merged.clone(),
    });
    harness.run_steps(4);
    assert!(!in_text_mode(&harness));
    assert!(
        bridge
            .take_opening_for_testing()
            .iter()
            .any(|path| path == "Plan.md"),
        "the window asks for the new lineage's document"
    );
    let (document, shared) = TextBridge::document_for_testing(RID, &merged);
    bridge.deliver_for_testing("Plan.md", document);
    harness.run_steps(3);
    assert!(in_text_mode(&harness));
    type_in(&mut harness, "!");
    assert_eq!(shared.merged_text().unwrap(), shown(&harness));
}
