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
                targets: vec!["tasks/create-task".into()],
            },
        ]
    );
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
