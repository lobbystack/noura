//! Seam 3: plugin manifests, read from fixture folders.

use std::path::PathBuf;

use plugin_host::{Capability, InstallErrorCode, Platform, read_manifest};

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/manifests")
        .join(name)
}

fn refusal(name: &str) -> (InstallErrorCode, String) {
    let error = read_manifest(&fixture(name)).expect_err("the manifest is refused");
    (error.code, error.message)
}

#[test]
fn a_manifest_declares_its_identity_entry_capabilities_and_platforms() {
    let manifest = read_manifest(&fixture("valid")).expect("the manifest reads");

    assert_eq!(manifest.id, "weather-board");
    assert_eq!(manifest.name, "Weather board");
    assert_eq!(manifest.version.to_string(), "1.2.0");
    assert_eq!(manifest.interface_version.to_string(), "0.1.0");
    assert_eq!(manifest.entry, "build/weather.wasm");
    assert_eq!(manifest.platforms, [Platform::Desktop, Platform::Mobile]);
    assert_eq!(
        manifest.capabilities,
        [
            Capability::WorkspaceRead,
            Capability::Views,
            Capability::Network {
                hosts: vec!["api.weather.example".into(), "tiles.weather.example".into()],
            },
            Capability::Commands {
                targets: vec!["calendar".into(), "tasks/create-task".into()],
            },
            Capability::InboxSections,
            Capability::Settings,
            Capability::Tools,
        ]
    );
}

#[test]
fn commands_need_no_targets() {
    let manifest = read_manifest(&fixture("commands-only")).expect("the manifest reads");

    assert_eq!(
        manifest.capabilities,
        [Capability::Commands {
            targets: Vec::new()
        }]
    );
}

#[test]
fn a_command_target_names_a_plugin_or_one_of_its_commands() {
    let (code, message) = refusal("bad-target");

    assert_eq!(code, InstallErrorCode::InvalidManifest);
    assert!(message.contains("tasks/Create Task"), "{message}");
}

#[test]
fn a_network_host_on_the_local_network_is_refused() {
    let (code, message) = refusal("local-host");

    assert_eq!(code, InstallErrorCode::InvalidManifest);
    assert!(message.contains("printer.local"), "{message}");
}

#[test]
fn malformed_toml_is_refused_with_the_line_and_column() {
    let error = read_manifest(&fixture("malformed")).expect_err("refused");

    assert_eq!(error.code, InstallErrorCode::InvalidManifest);
    assert_eq!(error.line, Some(3));
    assert!(error.message.contains("line 3"), "{}", error.message);
}

#[test]
fn a_missing_or_unknown_field_is_refused_by_name() {
    let (code, message) = refusal("missing-field");
    assert_eq!(code, InstallErrorCode::InvalidManifest);
    assert!(message.contains("entry"), "{message}");

    let (code, message) = refusal("unknown-field");
    assert_eq!(code, InstallErrorCode::InvalidManifest);
    assert!(message.contains("run-at-startup"), "{message}");
}

#[test]
fn an_unknown_interface_version_is_refused() {
    let (code, message) = refusal("unknown-interface");

    assert_eq!(code, InstallErrorCode::UnknownInterfaceVersion);
    assert!(message.contains("9.0.0"), "{message}");
    assert!(message.contains("0.1.0"), "{message}");
}

#[test]
fn an_unknown_capability_is_refused_by_name() {
    let (code, message) = refusal("unknown-capability");

    assert_eq!(code, InstallErrorCode::UnknownCapability);
    assert!(message.contains("read-keychain"), "{message}");
}

#[test]
fn hosts_belong_only_to_the_network_capability() {
    let (code, message) = refusal("hosts-without-network");

    assert_eq!(code, InstallErrorCode::InvalidManifest);
    assert!(message.contains("hosts"), "{message}");
}

#[test]
fn a_platform_list_without_desktop_is_refused() {
    let (code, _) = refusal("no-desktop");

    assert_eq!(code, InstallErrorCode::DesktopNotSupported);
}

#[test]
fn an_entry_component_outside_the_plugin_folder_is_refused() {
    for name in ["entry-outside", "absolute-entry"] {
        let (code, _) = refusal(name);
        assert_eq!(code, InstallErrorCode::EntryOutsideFolder, "{name}");
    }
}

#[test]
fn a_folder_without_a_manifest_is_refused() {
    let (code, message) = refusal("no-manifest");

    assert_eq!(code, InstallErrorCode::ManifestMissing);
    assert!(message.contains("plugin.toml"), "{message}");
}

/// The manifest `capabilities` makes, with this ID, read from a folder of
/// its own.
fn read_with(
    id: &str,
    capabilities: &str,
) -> Result<plugin_host::PluginManifest, (InstallErrorCode, String)> {
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(
        folder.path().join("plugin.toml"),
        format!(
            "id = \"{id}\"\nname = \"Pinger\"\nversion = \"0.1.0\"\ninterface-version = \"0.1.0\"\n\
             entry = \"plugin.wasm\"\nplatforms = [\"desktop\"]\n\n{capabilities}"
        ),
    )
    .unwrap();
    read_manifest(folder.path()).map_err(|error| (error.code, error.message))
}

#[test]
fn events_declare_an_id_and_a_title_each() {
    let manifest = read_with(
        "pinger",
        "[[capabilities]]\nname = \"events\"\nevents = [{ id = \"ping\", title = \"Ping arrives\" }]\n",
    )
    .unwrap();
    assert_eq!(
        manifest.capabilities,
        [Capability::Events {
            events: vec![plugin_host::EventDeclaration {
                id: "ping".to_owned(),
                title: "Ping arrives".to_owned(),
            }]
        }]
    );
    assert_eq!(
        manifest.capabilities[0].description(),
        "Tell Noura when these happen, so your Orbs can act on them: Ping arrives."
    );
    for (capabilities, message) in [
        ("name = \"events\"\n", "The events capability needs events."),
        (
            "name = \"events\"\nevents = []\n",
            "The events capability needs at least one event.",
        ),
        (
            "name = \"events\"\nevents = [{ id = \"Ping\", title = \"Ping\" }]\n",
            "must start with a lowercase letter",
        ),
        (
            "name = \"events\"\nevents = [{ id = \"ping\", title = \"A\" }, { id = \"ping\", title = \"B\" }]\n",
            "lists the event ping twice",
        ),
        (
            "name = \"views\"\nevents = [{ id = \"ping\", title = \"Ping\" }]\n",
            "The views capability doesn't take events.",
        ),
    ] {
        let (code, refused) =
            read_with("pinger", &format!("[[capabilities]]\n{capabilities}")).unwrap_err();
        assert_eq!(code, InstallErrorCode::InvalidManifest, "{capabilities}");
        assert!(refused.contains(message), "{refused}");
    }
}

#[test]
fn no_plugin_takes_the_id_of_a_part_of_noura() {
    for id in ["folders", "notes", "sync"] {
        let (code, refused) = read_with(id, "").unwrap_err();
        assert_eq!(code, InstallErrorCode::InvalidManifest, "{id}");
        assert!(refused.contains("names a part of Noura"), "{refused}");
    }
}

#[test]
fn no_plugin_takes_the_id_of_nouras_own_tools() {
    let (code, refused) = read_with("noura", "").unwrap_err();
    assert_eq!(code, InstallErrorCode::InvalidManifest);
    assert!(refused.contains("names Noura's own tools"), "{refused}");
}
