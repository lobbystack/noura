//! Finds the Rust plugins that `crates/plugin-sdk/build-plugins.sh` builds,
//! and lays out plugin folders around them.
//!
//! Without the built plugins the tests that need them pass with a note on
//! stderr, so a machine without the wasm32-wasip2 target can still run the
//! workspace tests. CI sets `NOURA_REQUIRE_PLUGINS=1`, which turns a
//! missing plugin into a failure.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use plugin_host::{
    Backend, FileRead, Limits, Listed, Node, NodeKind, ObjectRead, PluginHost, RunningPlugin,
    ServiceError, Services, WorkspacePath, WorkspaceReads,
};

/// The code generator the tests run plugins on: Wasmtime's Pulley
/// interpreter when the `pulley` feature is on, as CI runs the seam 3
/// suite a second time, and Cranelift otherwise.
pub fn backend() -> Backend {
    #[cfg(feature = "pulley")]
    return Backend::Pulley;
    #[cfg(not(feature = "pulley"))]
    Backend::Cranelift
}

/// A host on [`backend`] whose library is `library`.
pub fn host(library: &Path) -> PluginHost {
    host_with(library, Limits::default())
}

/// [`host`] with these limits.
pub fn host_with(library: &Path, limits: Limits) -> PluginHost {
    PluginHost::with_backend(library, backend(), limits).expect("the host starts")
}

/// The hello plugin's manifest, as `crates/plugins/hello` ships it.
pub const HELLO_MANIFEST: &str = include_str!("../../../plugins/hello/plugin.toml");

/// The component the build script built for the crate whose library is
/// `name`, such as `hello_plugin`, or `None` when it isn't built and CI
/// doesn't require it.
pub fn built(name: &str) -> Option<Vec<u8>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../target"));
    let path = target_dir.join(format!("wasm32-wasip2/debug/{name}.wasm"));
    match std::fs::read(&path) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            let message = format!(
                "the plugin {name} isn't built at {} ({error}); run crates/plugin-sdk/build-plugins.sh",
                path.display()
            );
            if std::env::var_os("NOURA_REQUIRE_PLUGINS").is_some_and(|value| value == "1") {
                panic!("{message}");
            }
            eprintln!("skipping a plugin test: {message}");
            None
        }
    }
}

/// A plugin manifest with this ID, entry and capabilities, each written as
/// it appears in `[[capabilities]]`, such as `name = "views"`.
pub fn manifest(id: &str, entry: &str, capabilities: &[&str]) -> String {
    let mut text = format!(
        "id = \"{id}\"\nname = \"{id}\"\nversion = \"0.1.0\"\ninterface-version = \"0.1.0\"\n\
         entry = \"{entry}\"\nplatforms = [\"desktop\"]\n"
    );
    for capability in capabilities {
        text.push_str(&format!("\n[[capabilities]]\n{capability}\n"));
    }
    text
}

/// Write a plugin folder at `folder`: `plugin.toml` and the component at
/// `entry`.
pub fn write_folder(folder: &Path, manifest: &str, entry: &str, component: &[u8]) {
    std::fs::create_dir_all(folder).unwrap();
    std::fs::write(folder.join("plugin.toml"), manifest).unwrap();
    let entry = folder.join(entry);
    std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
    std::fs::write(entry, component).unwrap();
}

/// A plugin installed from `component` and started with every capability
/// its manifest declares, as `capabilities` lists them. Its folders last as
/// long as the returned guard.
pub fn running(
    component: &[u8],
    capabilities: &[&str],
    workspace: Option<Arc<dyn WorkspaceReads>>,
) -> (Arc<RunningPlugin>, [tempfile::TempDir; 2]) {
    start(
        "probe",
        component,
        capabilities,
        Services {
            workspace,
            ..Services::default()
        },
        Limits::default(),
    )
}

/// A plugin with the ID `id` installed from `component` and started with
/// every capability its manifest declares, using `services`, under
/// `limits`. Its folders last as long as the returned guard.
pub fn start(
    id: &str,
    component: &[u8],
    capabilities: &[&str],
    services: Services,
    limits: Limits,
) -> (Arc<RunningPlugin>, [tempfile::TempDir; 2]) {
    let folder = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    write_folder(
        folder.path(),
        &manifest(id, "plugin.wasm", capabilities),
        "plugin.wasm",
        component,
    );
    let host = host_with(library.path(), limits);
    host.install(&host.inspect(folder.path()).unwrap()).unwrap();
    let loaded = host.load(id).unwrap();
    let plugin = host.start(&loaded, &loaded.manifest().capabilities, services);
    (plugin, [folder, library])
}

/// The text of each text node in `nodes`, in order.
pub fn texts(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::Text(text) => Some(text.clone()),
            _ => None,
        })
        .collect()
}

/// A workspace that answers from fixed lists and counts the calls that
/// reach it.
#[derive(Default)]
pub struct FakeWorkspace {
    pub listing: Vec<Listed>,
    pub files: Vec<FileRead>,
    pub objects: Vec<ObjectRead>,
    pub calls: Mutex<Vec<String>>,
}

impl FakeWorkspace {
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

impl WorkspaceReads for FakeWorkspace {
    fn list_entries(&self) -> Result<Vec<Listed>, ServiceError> {
        self.calls.lock().unwrap().push("list".into());
        Ok(self.listing.clone())
    }

    fn read_file(&self, path: &WorkspacePath) -> Result<FileRead, ServiceError> {
        self.calls.lock().unwrap().push(format!("read-file {path}"));
        self.files
            .iter()
            .find(|file| &file.path == path)
            .cloned()
            .ok_or_else(|| ServiceError::NotFound(path.to_string()))
    }

    fn read_object(&self, id: &str) -> Result<ObjectRead, ServiceError> {
        self.calls.lock().unwrap().push(format!("read-object {id}"));
        self.objects
            .iter()
            .find(|object| object.id == id)
            .cloned()
            .ok_or_else(|| ServiceError::NotFound(id.to_owned()))
    }
}
