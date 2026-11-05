//! Plugin folders for the plugins service's tests, around the Rust plugins
//! that `crates/plugin-sdk/build-plugins.sh` builds.
//!
//! Without the built plugins the tests that need them pass with a note on
//! stderr. CI sets `NOURA_REQUIRE_PLUGINS=1`, which turns a missing plugin
//! into a failure.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use app_core::{App, ViewKind, ViewNode};

/// The hello plugin's manifest, as `crates/plugins/hello` ships it.
pub const HELLO_MANIFEST: &str = include_str!("../../../plugins/hello/plugin.toml");

/// The rail page the hello plugin adds.
pub const HELLO_PAGE: &str = "plugin:hello:hello";

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

/// A plugin folder outside the workspace, removed when dropped.
pub struct PluginFolder {
    dir: tempfile::TempDir,
}

impl PluginFolder {
    /// A folder holding `manifest` as `plugin.toml` and `component` at
    /// `entry`.
    pub fn new(manifest: &str, entry: &str, component: &[u8]) -> Self {
        let folder = Self {
            dir: tempfile::tempdir().unwrap(),
        };
        folder.write("plugin.toml", manifest.as_bytes());
        folder.write(entry, component);
        folder
    }

    /// The hello plugin as its crate ships it, or `None` when it isn't
    /// built.
    pub fn hello() -> Option<Self> {
        built("hello_plugin").map(|hello| Self::new(HELLO_MANIFEST, "hello_plugin.wasm", &hello))
    }

    /// The recipes test plugin as the plugin `id`, holding `object-types`
    /// and `views`, or `None` when it isn't built.
    pub fn recipes(id: &str) -> Option<Self> {
        built("recipes_plugin").map(|recipes| {
            Self::new(
                &manifest(
                    id,
                    "plugin.wasm",
                    &["name = \"object-types\"", "name = \"views\""],
                ),
                "plugin.wasm",
                &recipes,
            )
        })
    }

    pub fn path(&self) -> &Path {
        self.dir.path()
    }

    /// Replace a file, as a build tool saving a new build does.
    pub fn write(&self, relative: &str, bytes: &[u8]) {
        let path = self.dir.path().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
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

/// A plugin folder for the component built from the crate whose library
/// is `library`, such as `probe_plugin`, with the ID `id` and these
/// capabilities, or `None` when it isn't built.
pub fn folder_for(library: &str, id: &str, capabilities: &[&str]) -> Option<PluginFolder> {
    built(library).map(|component| {
        PluginFolder::new(
            &manifest(id, "plugin.wasm", capabilities),
            "plugin.wasm",
            &component,
        )
    })
}

/// Install the plugin in `folder`, approving every capability it
/// declares, turn it on in the open workspace, and wait until it drew.
pub fn install_and_enable(app: &mut App, folder: &Path, id: &str) {
    install(app, folder);
    assert!(app.set_plugin_enabled(id, true), "{id} turns on");
    super::settle(app);
}

/// Install the plugin in `folder`, approving every capability it declares.
pub fn install(app: &mut App, folder: &Path) {
    app.review_plugin_folder(folder.to_owned());
    let review = app.plugin_install().expect("the install dialog opens");
    assert_eq!(review.problem, None, "the folder is refused");
    assert!(app.approve_plugin_install(), "the plugin installs");
    super::settle(app);
}

/// The text of each text node in a plugin page's view, top to bottom, or
/// the error drawn in its place.
pub fn texts(app: &App, page: &str) -> Result<Vec<String>, String> {
    let view = app.plugin_view(page).expect("the page has a view")?;
    let mut texts = Vec::new();
    collect(view.root(), &mut texts);
    Ok(texts)
}

fn collect(node: &ViewNode, texts: &mut Vec<String>) {
    if let ViewKind::Text(text) = &node.kind {
        texts.push(text.clone());
    }
    for child in &node.children {
        collect(child, texts);
    }
}

/// Every file under `folder`, by path relative to it.
pub fn files_under(folder: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(folder) else {
        return found;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(
                files_under(&path).into_iter().map(|inner| {
                    format!("{}/{inner}", path.file_name().unwrap().to_string_lossy())
                }),
            );
        } else {
            found.push(path.file_name().unwrap().to_string_lossy().into_owned());
        }
    }
    found.sort();
    found
}

/// Poll the app until `done` holds, for up to 20 seconds.
pub fn wait_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        super::settle(app);
        if done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}
