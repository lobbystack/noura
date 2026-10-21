//! Seam 3: a plugin reaches only the capability interfaces its manifest
//! declares, and the host links no WASI filesystem, socket or HTTP
//! interface.

mod support;

use std::sync::Arc;

use plugin_host::{InstallErrorCode, PluginError, PluginHost};
use support::plugins::{self, FakeWorkspace};

#[test]
fn a_call_to_an_undeclared_capability_names_it_and_reaches_no_service() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let workspace = Arc::new(FakeWorkspace::default());
    let (mut instance, _folders) =
        plugins::running(&probe, &["name = \"views\""], Some(workspace.clone()));

    let nodes = instance.render("list").expect("the plugin renders");

    assert_eq!(
        plugins::texts(&nodes),
        ["error capability-not-declared workspace-read"]
    );
    let nodes = instance.render("read-file:Plan.md").unwrap();
    assert_eq!(
        plugins::texts(&nodes),
        ["error capability-not-declared workspace-read"]
    );
    assert!(workspace.calls().is_empty(), "{:?}", workspace.calls());
}

#[test]
fn a_declared_capability_reaches_its_service() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let workspace = Arc::new(FakeWorkspace::default());
    let (mut instance, _folders) = plugins::running(
        &probe,
        &["name = \"workspace-read\"", "name = \"views\""],
        Some(workspace.clone()),
    );

    let nodes = instance.render("list").expect("the plugin renders");

    assert!(plugins::texts(&nodes).is_empty());
    assert_eq!(workspace.calls(), ["list"]);
}

#[test]
fn a_plugin_without_views_gets_no_view_calls() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (mut instance, _folders) = plugins::running(&probe, &["name = \"workspace-read\""], None);

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

    let error = PluginHost::new(library.path())
        .unwrap()
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

        let error = PluginHost::new(library.path())
            .unwrap()
            .inspect(folder.path())
            .err()
            .expect("the component is refused");

        assert_eq!(error.code, InstallErrorCode::ForbiddenImport, "{import}");
        assert!(error.message.contains(import), "{}", error.message);
    }
}
