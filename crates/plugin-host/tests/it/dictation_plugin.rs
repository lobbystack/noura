//! Seam 3: the Dictation plugin in the real host. Its manifest passes the
//! host's check with the speech capabilities, its shortcut and its
//! settings, and it registers its settings type and its page in Settings.
//! The app-core suite dictates with it through the fake engine.

use crate::support;
use std::{path::Path, sync::Arc};

use plugin_host::{Capability, ObjectTypes, Services, ShortcutDeclaration, Surface, read_manifest};
use support::plugins::{self, FakeObjects};

/// The Dictation plugin's manifest, as its crate ships it.
const MANIFEST: &str = include_str!("../../../plugins/dictation/plugin.toml");

#[test]
fn the_manifest_declares_speech_the_shortcut_paste_the_menu_and_settings() {
    let manifest = read_manifest(Path::new(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../plugins/dictation"
    )))
    .expect("the manifest reads");
    assert_eq!(manifest.id, "dictation");
    assert_eq!(manifest.name, "Dictation");
    assert_eq!(
        manifest.capabilities,
        [
            Capability::Speech,
            Capability::GlobalShortcuts {
                shortcuts: vec![ShortcutDeclaration {
                    id: "dictation".into(),
                    title: "Dictation".into(),
                    keys: "Ctrl+Space".into(),
                    macos_keys: Some("Alt+Space".into()),
                    windows_keys: None,
                    linux_keys: None,
                }],
            },
            Capability::Paste,
            Capability::MenuBarItems,
            Capability::Notifications,
            Capability::Settings,
            Capability::ObjectTypes,
        ]
    );
}

#[test]
fn the_plugin_installs_and_registers_its_settings_type_and_page() {
    let Some(component) = plugins::built("dictation_plugin") else {
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    plugins::write_folder(folder.path(), MANIFEST, "dictation_plugin.wasm", &component);
    let host = plugins::host(library.path());
    host.install(&host.inspect(folder.path()).unwrap()).unwrap();
    let loaded = host.load("dictation").unwrap();
    let types = ObjectTypes::new();
    let objects: Arc<dyn plugin_host::WorkspaceObjects> = Arc::new(FakeObjects::default());
    let plugin = host.start(
        &loaded,
        &loaded.manifest().capabilities,
        Services {
            objects: Some(objects),
            types: types.clone(),
            ..Services::default()
        },
    );

    assert_eq!(
        plugin.register_object_types().unwrap(),
        [("dictation-settings".to_owned(), Ok(()))]
    );
    assert_eq!(
        types.owner("dictation-settings").as_deref(),
        Some("dictation")
    );
    // Without the kernel's speech services the page still draws.
    assert_eq!(
        plugin.settings_page().unwrap().as_deref(),
        Some("Dictation")
    );
    assert!(
        !plugin
            .render_surface(&Surface::Settings)
            .unwrap()
            .is_empty()
    );
}
