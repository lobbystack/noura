//! Seam 1: the workspace's name, which is its folder's name, and its
//! location in Settings.

use crate::support;
use app_core::{App, NoticeKind, Platform, settings_section};
use support::{Call, FakeSystem, Fixture, settle};

const MANIFEST: &str = ".noura/workspace.yaml";

fn name(app: &App) -> String {
    app.workspace().unwrap().name()
}

fn recent_names(app: &App) -> Vec<String> {
    app.recents()
        .iter()
        .map(|recent| recent.name.clone())
        .collect()
}

/// Settings' notice, as its kind and message.
fn settings_notice(app: &App) -> Option<(NoticeKind, String)> {
    app.settings()
        .unwrap()
        .notice
        .as_ref()
        .map(|notice| (notice.kind, notice.message.clone()))
}

#[test]
fn the_workspace_takes_its_folder_s_name_and_the_manifest_holds_none() {
    let fixture = Fixture::new(&[]);
    let other = fixture.outside();
    let mut app = fixture.app();
    app.open_folder(other.clone());
    settle(&mut app);
    app.open_recent(&app.recents()[1].workspace_id.clone());
    settle(&mut app);

    assert_eq!(name(&app), "Garden");
    assert_eq!(recent_names(&app), ["Garden", "Outside"]);
    let manifest = String::from_utf8(fixture.read(MANIFEST)).unwrap();
    assert!(!manifest.contains("name"), "{manifest}");
}

/// A manifest that an earlier version wrote holds a `name`. The workspace
/// opens, shows its folder's name, and Noura leaves the file as it is.
#[test]
fn a_name_in_the_manifest_changes_nothing() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    drop(app);
    let manifest = String::from_utf8(fixture.read(MANIFEST)).unwrap().replace(
        "format_version: 1\n",
        "format_version: 1\nname: Greenhouse\n",
    );
    std::fs::write(fixture.root.join(MANIFEST), &manifest).unwrap();

    app = App::launch(fixture.app_data.path());
    settle(&mut app);

    assert_eq!(name(&app), "Garden");
    assert_eq!(recent_names(&app), ["Garden"]);
    assert_eq!(fixture.read(MANIFEST), manifest.as_bytes());
}

#[test]
fn show_folder_reveals_the_workspace_folder_in_the_file_manager() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::on(Platform::MacOs);
    let mut app = fixture.app_with(system.clone());
    app.open_settings_at(settings_section::WORKSPACE);

    assert_eq!(
        app.workspace().unwrap().root(),
        fixture.root.to_str().unwrap()
    );
    app.reveal_workspace();

    assert_eq!(system.calls(), [Call::Reveal(fixture.root.clone())]);
    assert_eq!(settings_notice(&app), None);
}
