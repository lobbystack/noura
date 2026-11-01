//! Plugin folders around the Rust plugins that
//! `crates/plugin-sdk/build-plugins.sh` builds.
//!
//! Without the built plugins the tests that need them pass with a note on
//! stderr. CI sets `NOURA_REQUIRE_PLUGINS=1`, which turns a missing plugin
//! into a failure.

use std::path::{Path, PathBuf};

/// The hello plugin's manifest, as `crates/plugins/hello` ships it.
pub const HELLO_MANIFEST: &str = include_str!("../../../plugins/hello/plugin.toml");

/// The component built for the crate whose library is `name`, such as
/// `hello_plugin`, or `None` when it isn't built and CI doesn't require
/// it.
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

/// A plugin folder holding `manifest` as `plugin.toml` and `component` at
/// `entry`, inside `parent`.
pub fn folder(parent: &Path, manifest: &str, entry: &str, component: &[u8]) -> PathBuf {
    let folder = parent.join("plugin");
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("plugin.toml"), manifest).unwrap();
    std::fs::write(folder.join(entry), component).unwrap();
    folder
}

/// A manifest for the probe test plugin with the capabilities it needs.
pub fn probe_manifest() -> String {
    probe_manifest_with(&[])
}

/// [`probe_manifest`] with more capabilities, each as `[[capabilities]]`
/// writes it, such as `name = "settings"`.
pub fn probe_manifest_with(more: &[&str]) -> String {
    let mut manifest =
        "id = \"probe\"\nname = \"Probe\"\nversion = \"0.1.0\"\ninterface-version = \"0.1.0\"\n\
         entry = \"plugin.wasm\"\nplatforms = [\"desktop\"]\n\n[[capabilities]]\n\
         name = \"workspace-read\"\n\n[[capabilities]]\nname = \"views\"\n"
            .to_owned();
    for capability in more {
        manifest.push_str(&format!("\n[[capabilities]]\n{capability}\n"));
    }
    manifest
}

/// A manifest for the kit test plugin, which adds pages to the rail.
pub fn kit_manifest() -> String {
    "id = \"kit\"\nname = \"Kit\"\nversion = \"0.1.0\"\ninterface-version = \"0.1.0\"\n\
     entry = \"plugin.wasm\"\nplatforms = [\"desktop\"]\n\n[[capabilities]]\nname = \"views\"\n"
        .to_owned()
}

/// Install the plugin in `folder` in the workspace that `app_data` opens
/// at launch, and turn it on there, before the harness starts.
pub fn install_and_enable(app_data: &Path, folder: &Path, id: &str) {
    let mut app = app_core::App::launch(app_data);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while app.is_busy() || app.workspace().is_none() {
        app.poll();
        assert!(
            std::time::Instant::now() < deadline,
            "the workspace didn't open"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    app.review_plugin_folder(folder.to_owned());
    assert_eq!(app.plugin_install().unwrap().problem, None);
    assert!(app.approve_plugin_install());
    assert!(app.set_plugin_enabled(id, true));
}
