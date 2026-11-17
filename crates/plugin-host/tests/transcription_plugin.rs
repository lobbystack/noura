//! Seam 3: the Transcription plugin in the real host. Its manifest
//! declares what it needs, from reading the workspace to calling Tasks
//! and Projects, and the running plugin registers the schemas its fixture
//! holds.

mod support;

use std::path::Path;

use plugin_host::{Capability, ObjectTypes, Services, ShortcutDeclaration, read_manifest};
use support::plugins;
use workspace_format::PropertySchema;

const MANIFEST: &str = include_str!("../../plugins/transcription/plugin.toml");
const FIXTURE: &str = include_str!("../../plugins/transcription/fixtures/transcription-v1.json");

#[test]
fn the_manifest_declares_speech_ai_and_tasks_and_projects_as_targets() {
    let manifest = read_manifest(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../plugins/transcription"
    )))
    .expect("the manifest reads");
    assert_eq!(manifest.id, "transcription");
    assert_eq!(
        manifest.capabilities,
        [
            Capability::WorkspaceRead,
            Capability::ObjectTypes,
            Capability::Views,
            Capability::Settings,
            Capability::Speech,
            Capability::GlobalShortcuts {
                shortcuts: vec![ShortcutDeclaration {
                    id: "transcription".to_owned(),
                    title: "Transcription".to_owned(),
                    keys: "Ctrl+Alt+T".to_owned(),
                    macos_keys: None,
                    windows_keys: None,
                    linux_keys: None,
                }]
            },
            Capability::MenuBarItems,
            Capability::Notifications,
            Capability::Ai,
            Capability::Commands {
                targets: vec!["projects".to_owned(), "tasks".to_owned()]
            },
        ]
    );
}

#[test]
fn the_plugin_registers_the_fixtures_schemas() {
    let Some(component) = plugins::built("transcription_plugin") else {
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    plugins::write_folder(
        folder.path(),
        MANIFEST,
        "transcription_plugin.wasm",
        &component,
    );
    let library = tempfile::tempdir().unwrap();
    let host = plugins::host(library.path());
    host.install(&host.inspect(folder.path()).unwrap()).unwrap();
    let loaded = host.load("transcription").unwrap();
    let types = ObjectTypes::new();
    let running = host.start(
        &loaded,
        &loaded.manifest().capabilities,
        Services {
            types: types.clone(),
            ..Services::default()
        },
    );

    let registered = running.register_object_types().unwrap();

    let names = ["transcription", "transcript", "transcription-settings"];
    assert_eq!(
        registered,
        names.map(|name| (name.to_owned(), Ok(()))).to_vec()
    );
    let fixture: serde_json::Value = serde_json::from_str(FIXTURE).unwrap();
    for name in names {
        assert_eq!(types.owner(name).as_deref(), Some("transcription"));
        let expected: PropertySchema =
            serde_json::from_value(fixture["schemas"][name].clone()).unwrap();
        assert_eq!(types.schemas().schema(name), Some(&expected), "{name}");
    }
}
