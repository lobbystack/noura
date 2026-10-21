//! Seam 1: the workspace's name and location in Settings.

mod support;

use app_core::{App, NoticeKind, Platform, settings_section};
use support::{Call, FakeSystem, Fixture, settle};

const MANIFEST: &str = ".noura/workspace.yaml";

/// The name in the manifest on disk.
fn manifest_name(fixture: &Fixture) -> String {
    let manifest: local_core::WorkspaceManifest =
        serde_yaml_ng::from_slice(&fixture.read(MANIFEST)).unwrap();
    manifest.name
}

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
fn renaming_writes_the_manifest_durably_and_survives_quit_and_reopen() {
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let mut app = fixture.app();
    app.open_settings_at(settings_section::WORKSPACE);

    let (renamed, synced) =
        local_core::durability_probe::record(|| app.rename_workspace("Orchard"));

    assert!(renamed);
    // The new manifest is on disk before the call returns.
    assert_eq!(manifest_name(&fixture), "Orchard");
    assert!(
        synced.contains(&fixture.root.join(".noura")),
        "the manifest's folder wasn't synced: {synced:?}"
    );
    assert_eq!(name(&app), "Orchard");
    assert_eq!(recent_names(&app), ["Orchard"]);
    assert_eq!(settings_notice(&app), None);
    // The folder keeps its name, and nothing else in it changed.
    assert_eq!(fixture.root.file_name().unwrap(), "Garden");
    assert_eq!(fixture.read("Plan.md"), b"plan");

    drop(app);
    let mut app = App::launch(fixture.app_data.path());
    settle(&mut app);
    assert_eq!(name(&app), "Orchard");
    assert_eq!(recent_names(&app), ["Orchard"]);
}

#[test]
fn the_new_name_shows_in_the_recent_list_beside_other_workspaces() {
    let fixture = Fixture::new(&[]);
    let other = fixture.outside();
    let mut app = fixture.app();
    app.open_folder(other.clone());
    settle(&mut app);
    app.open_recent(&app.recents()[1].workspace_id.clone());
    settle(&mut app);
    assert_eq!(recent_names(&app), ["Garden", "Outside"]);

    app.open_settings_at(settings_section::WORKSPACE);
    assert!(app.rename_workspace("  Orchard  "));

    assert_eq!(recent_names(&app), ["Orchard", "Outside"]);
    assert_eq!(manifest_name(&fixture), "Orchard");
}

#[test]
fn an_empty_name_keeps_the_old_one() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    app.open_settings_at(settings_section::WORKSPACE);
    let before = fixture.read(MANIFEST);

    for empty in ["", "   ", "\t\n"] {
        assert!(!app.rename_workspace(empty));
    }

    assert_eq!(name(&app), "Garden");
    assert_eq!(recent_names(&app), ["Garden"]);
    assert_eq!(fixture.read(MANIFEST), before);
}

#[test]
fn an_outside_edit_since_settings_read_the_manifest_gets_an_error_instead_of_an_overwrite() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    app.open_settings_at(settings_section::WORKSPACE);
    let edited = String::from_utf8(fixture.read(MANIFEST))
        .unwrap()
        .replace("name: Garden", "name: Greenhouse");
    std::fs::write(fixture.root.join(MANIFEST), &edited).unwrap();

    assert!(!app.rename_workspace("Orchard"));

    assert_eq!(fixture.read(MANIFEST), edited.as_bytes());
    assert_eq!(
        settings_notice(&app),
        Some((
            NoticeKind::Problem,
            "The settings file .noura/workspace.yaml changed on disk since Noura read it. \
             Try again."
                .to_owned()
        ))
    );
    // Settings shows the manifest as it is now, so trying again starts
    // from the outside edit.
    assert_eq!(name(&app), "Greenhouse");
    assert!(app.rename_workspace("Orchard"));
    assert_eq!(manifest_name(&fixture), "Orchard");
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
