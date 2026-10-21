//! The sidebar's layout across launches, through AccessKit.

mod support;

use egui::{Key, Modifiers, accesskit::Role};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

fn opened(app_data: &std::path::Path, folder: &std::path::Path) -> Ui {
    create_workspace(app_data, "Garden", folder);
    std::fs::write(folder.join("Note.md"), b"x").unwrap();
    let mut harness = harness(app_data, FakePicker::default());
    settle(&mut harness);
    harness
}

/// Quit by dropping the harness, then launch again.
fn relaunch(harness: Ui, app_data: &std::path::Path) -> Ui {
    drop(harness);
    let mut harness = harness_launch(app_data);
    settle(&mut harness);
    harness
}

fn harness_launch(app_data: &std::path::Path) -> Ui {
    harness(app_data, FakePicker::default())
}

fn tree_shown(harness: &Ui) -> bool {
    harness
        .query_by_role_and_label(Role::TreeItem, "Note")
        .is_some()
}

#[test]
fn cmd_backslash_hides_and_shows_the_sidebar_and_the_choice_lasts() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    assert!(tree_shown(&harness));

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    harness.run();
    assert!(!tree_shown(&harness));
    // The workspace switcher stays in reach.
    harness.get_by_role_and_label(Role::Button, "Switch workspace. Current workspace: Garden");
    let mut harness = relaunch(harness, app_data.path());
    assert!(!tree_shown(&harness));

    harness.key_press_modifiers(Modifiers::COMMAND, Key::Backslash);
    harness.run();
    assert!(tree_shown(&harness));
}

/// Where the sidebar's resize handle sits, which moves with its width.
fn sidebar_edge(harness: &Ui) -> f32 {
    harness.get_by_role(Role::Splitter).rect().center().x
}

#[test]
fn the_sidebar_keeps_its_width_and_folded_files_section_after_a_relaunch() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let mut harness = opened(app_data.path(), &parent.path().join("Garden"));
    let narrowest = sidebar_edge(&harness);
    for _ in 0..4 {
        harness.get_by_role(Role::Splitter).increment_accesskit();
        harness.run();
    }
    let widened = sidebar_edge(&harness);
    assert!(widened > narrowest, "the sidebar didn't widen");
    harness.get_by_role_and_label(Role::Button, "Files").click();
    harness.run();
    assert!(!tree_shown(&harness));
    assert_eq!(
        harness
            .get_by_role_and_label(Role::Button, "Files")
            .accesskit_node()
            .data()
            .is_expanded(),
        Some(false)
    );

    let mut harness = relaunch(harness, app_data.path());
    assert!(!tree_shown(&harness));
    harness.get_by_role_and_label(Role::Button, "Files").click();
    harness.run();
    assert_eq!(sidebar_edge(&harness), widened);
}
