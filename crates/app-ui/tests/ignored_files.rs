//! The workspace switcher's Reveal and Ignored files items, through
//! AccessKit.

mod support;

use app_core::Platform;
use egui::{Key, accesskit::Role};
use egui_kittest::kittest::Queryable;
use support::{FakePicker, FakeSystem, create_workspace, harness_with, settle};

const SWITCHER: &str = "Switch workspace. Current workspace: Garden";

#[test]
fn the_switcher_reveals_the_workspace_folder() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    let system = FakeSystem::new(Platform::Windows);
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system.clone());
    settle(&mut harness);

    harness
        .get_by_role_and_label(Role::Button, SWITCHER)
        .click();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Show in Explorer")
        .click();
    harness.run();

    let root = std::fs::canonicalize(&folder).unwrap();
    assert_eq!(system.calls(), [format!("reveal {}", root.display())]);
}

#[test]
fn the_ignored_files_page_adds_and_removes_patterns() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    std::fs::create_dir_all(folder.join("build")).unwrap();
    std::fs::write(folder.join("build/out.txt"), b"o").unwrap();
    std::fs::write(folder.join("Readme.md"), b"r").unwrap();
    let system = FakeSystem::new(Platform::current());
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system);
    settle(&mut harness);
    harness.get_by_role_and_label(Role::TreeItem, "build");

    harness
        .get_by_role_and_label(Role::Button, SWITCHER)
        .click();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Ignored files\u{2026}")
        .click();
    harness.run();
    harness.get_by_label("Ignored files");
    harness
        .get_by_role_and_label(Role::TextInput, "Pattern to ignore")
        .type_text("build/");
    harness.run();
    harness.get_by_role_and_label(Role::Button, "Add").click();
    harness.run();

    harness.get_by_label("build/");
    assert!(
        harness
            .query_by_role_and_label(Role::TreeItem, "build")
            .is_none()
    );
    let manifest = std::fs::read_to_string(folder.join(".noura/workspace.yaml")).unwrap();
    assert!(manifest.contains("- build/"), "{manifest}");
    assert!(harness.unnamed_widgets().is_empty());

    harness
        .get_by_role_and_label(Role::TextInput, "Pattern to ignore")
        .type_text("notes/{a,b");
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    harness.get_by_label("\u{201c}notes/{a,b\u{201d} isn't a valid pattern.");

    harness
        .get_by_role_and_label(Role::Button, "Stop ignoring build/")
        .click();
    harness.run();
    harness.get_by_role_and_label(Role::TreeItem, "build");

    harness.get_by_role_and_label(Role::Button, "Done").click();
    harness.run();
    assert!(harness.query_by_label("Pattern to ignore").is_none());
}
