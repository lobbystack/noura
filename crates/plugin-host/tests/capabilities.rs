//! Seam 3: a plugin reaches only the capability interfaces its manifest
//! declares, and the host links no WASI filesystem, socket or HTTP
//! interface.

mod support;

use std::sync::Arc;

use plugin_host::{Capability, InstallErrorCode, PluginError};
use support::plugins::{self, FakeWorkspace};

#[test]
fn a_call_to_an_undeclared_capability_names_it_and_reaches_no_service() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let workspace = Arc::new(FakeWorkspace::default());
    let (instance, _folders) =
        plugins::running(&probe, &["name = \"views\""], Some(workspace.clone()));

    let nodes = instance.render("list").expect("the plugin renders");

    assert_eq!(
        plugins::texts(&nodes),
        ["error capability-not-declared workspace-read"]
    );
    for call in ["read-file:Plan.md", "read-object-at:Plan.md"] {
        let nodes = instance.render(call).unwrap();
        assert_eq!(
            plugins::texts(&nodes),
            ["error capability-not-declared workspace-read"],
            "{call}"
        );
    }
    assert!(workspace.calls().is_empty(), "{:?}", workspace.calls());
}

#[test]
fn a_declared_capability_reaches_its_service() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let workspace = Arc::new(FakeWorkspace::default());
    let (instance, _folders) = plugins::running(
        &probe,
        &["name = \"workspace-read\"", "name = \"views\""],
        Some(workspace.clone()),
    );

    let nodes = instance.render("list").expect("the plugin renders");

    assert!(plugins::texts(&nodes).is_empty());
    assert_eq!(workspace.calls(), ["list"]);
}

#[test]
fn a_revoked_capability_is_refused_from_the_plugins_next_call_on() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let workspace = Arc::new(FakeWorkspace::default());
    let (plugin, _folders) = plugins::running(
        &probe,
        &["name = \"workspace-read\"", "name = \"views\""],
        Some(workspace.clone()),
    );
    plugin.render("list").expect("the plugin renders");
    assert_eq!(workspace.calls(), ["list"]);

    plugin.set_approved(&[Capability::Views]);

    let nodes = plugin.render("list").expect("the plugin renders");
    assert_eq!(
        plugins::texts(&nodes),
        ["error capability-not-declared workspace-read"]
    );
    assert_eq!(workspace.calls(), ["list"]);
    assert_eq!(plugin.granted(), [Capability::Views]);
}

#[test]
fn a_plugin_without_views_gets_no_view_calls() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &["name = \"workspace-read\""], None);

    assert_eq!(
        instance.pages().err(),
        Some(PluginError::CapabilityNotDeclared {
            capability: "views"
        })
    );
    assert_eq!(
        instance.render("probe").err(),
        Some(PluginError::CapabilityNotDeclared {
            capability: "views"
        })
    );
}

#[test]
fn a_rust_plugin_that_uses_the_filesystem_fails_to_install_naming_the_import() {
    let Some(uses_files) = plugins::built("uses_files_plugin") else {
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    plugins::write_folder(
        folder.path(),
        &plugins::manifest("uses-files", "plugin.wasm", &[]),
        "plugin.wasm",
        &uses_files,
    );

    let error = plugins::host(library.path())
        .inspect(folder.path())
        .err()
        .expect("the plugin is refused");

    assert_eq!(error.code, InstallErrorCode::ForbiddenImport);
    assert!(
        error.message.contains("wasi:filesystem/"),
        "{}",
        error.message
    );
}

#[test]
fn a_component_that_imports_sockets_or_http_fails_to_install_naming_the_import() {
    for import in ["wasi:sockets/tcp@0.2.0", "wasi:http/outgoing-handler@0.2.0"] {
        let component =
            wat::parse_str(format!("(component (import \"{import}\" (instance)))")).unwrap();
        let folder = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        plugins::write_folder(
            folder.path(),
            &plugins::manifest("network", "plugin.wasm", &[]),
            "plugin.wasm",
            &component,
        );

        let error = plugins::host(library.path())
            .inspect(folder.path())
            .err()
            .expect("the component is refused");

        assert_eq!(error.code, InstallErrorCode::ForbiddenImport, "{import}");
        assert!(error.message.contains(import), "{}", error.message);
    }
}
