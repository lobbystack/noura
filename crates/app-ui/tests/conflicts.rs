//! Another app's edit of the text you typed, through AccessKit: conflict
//! review labels both versions and its choices, Escape keeps editing and
//! leaves a banner that opens the review again, and each choice asks for
//! confirmation. A note whose file was deleted offers Restore.

mod support;

use std::time::{Duration, Instant};

use egui::{Event, Key, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

const NOTE: &str = "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\n\
                    created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n\
                    ---\nFirst line\n";

const THEIRS: &str = "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\n\
                      created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n\
                      ---\nFirst line, theirs\n";

/// A workspace holding `Plan.md`, opened, with the note open in the editor.
fn opened(app_data: &std::path::Path, folder: &std::path::Path) -> Ui {
    create_workspace(app_data, "Garden", folder);
    std::fs::write(folder.join("Plan.md"), NOTE).unwrap();
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "Plan")
        .click();
    harness.run_steps(4);
    harness
}

/// Run frames, waiting in real time, until `done` holds.
fn wait_until(harness: &mut Ui, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while !done(harness) {
        assert!(Instant::now() < deadline, "the screen never got there");
        std::thread::sleep(Duration::from_millis(20));
        harness.step();
    }
    harness.run_steps(4);
}

fn has(harness: &Ui, role: Role, label: &str) -> bool {
    harness.query_by_role_and_label(role, label).is_some()
}

/// Type in the note, then let another app change the same line before
/// the save runs, and wait for the review.
fn in_conflict(folder: &std::path::Path, harness: &mut Ui) {
    harness.event(Event::Text("Typed. ".into()));
    harness.run_steps(2);
    std::fs::write(folder.join("Plan.md"), THEIRS).unwrap();
    wait_until(harness, |harness| {
        has(harness, Role::Dialog, "Choose a version")
    });
}

#[test]
fn conflict_review_labels_both_versions_and_escape_leaves_a_banner_that_opens_it_again() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);

    in_conflict(&folder, &mut harness);
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());

    let dialog = harness.get_by_role_and_label(Role::Dialog, "Choose a version");
    let mine = dialog.get_by_role_and_label(Role::MultilineTextInput, "Your version");
    assert!(
        mine.accesskit_node()
            .value()
            .is_some_and(|text| text.contains("Typed. ")),
        "{:?}",
        mine.accesskit_node().value()
    );
    let theirs =
        dialog.get_by_role_and_label(Role::MultilineTextInput, "The file\u{2019}s version");
    assert_eq!(
        theirs.accesskit_node().value().as_deref(),
        Some("First line, theirs\n")
    );
    dialog.get_by_role_and_label(Role::Button, "Use the file\u{2019}s version");
    dialog.get_by_role_and_label(Role::Button, "Use my version");
    assert!(
        dialog
            .get_by_role_and_label(Role::Button, "Keep editing")
            .accesskit_node()
            .is_focused()
    );

    harness.key_press(Key::Escape);
    harness.run_steps(4);

    assert!(!has(&harness, Role::Dialog, "Choose a version"));
    let banner = harness.get_by_role_and_label(Role::Alert, "This file changed in another app");
    banner.get_by_label("Your text is still here. Compare both versions and choose one.");
    assert_eq!(harness.unnamed_widgets(), Vec::<String>::new());
    banner
        .get_by_role_and_label(Role::Button, "Compare")
        .click();
    harness.run_steps(4);
    assert!(has(&harness, Role::Dialog, "Choose a version"));
    assert_eq!(
        std::fs::read_to_string(folder.join("Plan.md")).unwrap(),
        THEIRS,
        "nothing saves while the conflict waits"
    );
}

#[test]
fn use_my_version_from_the_keyboard_asks_first_then_writes_it() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);
    in_conflict(&folder, &mut harness);

    // Keep editing, Use the file's version, then Use my version.
    harness.key_press(Key::Tab);
    harness.key_press(Key::Tab);
    harness.run_steps(2);
    harness.key_press(Key::Enter);
    harness.run_steps(4);

    let confirm = harness.get_by_role_and_label(Role::AlertDialog, "Use your version?");
    confirm.get_by_label("The file\u{2019}s version is kept as a backup first.");
    confirm.get_by_role_and_label(Role::Button, "Cancel");
    assert!(
        confirm
            .get_by_role_and_label(Role::Button, "Continue")
            .accesskit_node()
            .is_focused()
    );
    // Escape takes the choice back, and the review stays.
    harness.key_press(Key::Escape);
    harness.run_steps(4);
    assert!(!has(&harness, Role::AlertDialog, "Use your version?"));
    assert!(has(&harness, Role::Dialog, "Choose a version"));
    assert_eq!(
        std::fs::read_to_string(folder.join("Plan.md")).unwrap(),
        THEIRS
    );

    harness
        .get_by_role_and_label(Role::Button, "Use my version")
        .click();
    harness.run_steps(4);
    harness.key_press(Key::Enter);
    harness.run_steps(4);

    let written = std::fs::read_to_string(folder.join("Plan.md")).unwrap();
    assert!(written.contains("Typed. "), "{written:?}");
    assert!(!written.contains("theirs"), "{written:?}");
    assert!(!has(&harness, Role::Dialog, "Choose a version"));
    assert!(!has(
        &harness,
        Role::Alert,
        "This file changed in another app"
    ));
}

#[test]
fn a_note_deleted_while_you_type_offers_restore() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    let mut harness = opened(app_data.path(), &folder);

    harness.event(Event::Text("Typed. ".into()));
    harness.run_steps(2);
    std::fs::remove_file(folder.join("Plan.md")).unwrap();
    wait_until(&mut harness, |harness| {
        has(harness, Role::Alert, "This file was deleted")
    });

    let banner = harness.get_by_role_and_label(Role::Alert, "This file was deleted");
    banner.get_by_label("Your text is still here. You can put the file back.");
    banner
        .get_by_role_and_label(Role::Button, "Restore")
        .click();
    harness.run_steps(4);

    let written = std::fs::read_to_string(folder.join("Plan.md")).unwrap();
    assert_eq!(written.replace("Typed. ", ""), NOTE);
    assert!(!has(&harness, Role::Alert, "This file was deleted"));
}
