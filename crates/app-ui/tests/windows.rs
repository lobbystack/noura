//! New window and Close window, through AccessKit.

mod support;

use app_ui::NouraUi;
use egui::{Key, Modifiers, accesskit::Role};
use egui_kittest::{Harness, kittest::Queryable};
use support::{FakePicker, create_workspace, harness, settle};

#[test]
fn new_window_in_the_switcher_opens_another_window_on_the_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Current");
    create_workspace(app_data.path(), "Current", &folder);
    std::fs::write(folder.join("Plan.md"), b"x").unwrap();
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    assert!(harness.state_mut().take_new_windows().is_empty());

    harness
        .get_by_role_and_label(Role::Button, "Switch workspace. Current workspace: Current")
        .click();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "New window")
        .click();
    harness.run();

    let mut windows = harness.state_mut().take_new_windows();
    assert_eq!(windows.len(), 1);
    let window = NouraUi::new(windows.remove(0), Box::new(FakePicker::default()));
    let mut second = Harness::builder()
        .with_size([1000.0, 700.0])
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), window);
    second.run();
    second.get_by_role_and_label(Role::Button, "Switch workspace. Current workspace: Current");
    second.get_by_role_and_label(Role::TreeItem, "Plan");
}

#[test]
fn the_close_window_shortcut_closes_the_window() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Current", &parent.path().join("Current"));
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    assert!(!harness.state().is_closed());

    harness.key_press_modifiers(Modifiers::COMMAND | Modifiers::SHIFT, Key::W);
    harness.run();

    assert!(harness.state().is_closed());
}
