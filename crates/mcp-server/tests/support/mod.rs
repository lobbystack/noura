//! A workspace on disk with the app open on it, the plugins
//! `crates/plugin-sdk/build-plugins.sh` builds, and an MCP client
//! connected to the server in this process.

#![allow(dead_code, reason = "each test file uses a different subset")]

use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use app_core::{AgentWorkspace, App, WorkspaceService};
use noura_mcp::NouraMcp;
use rmcp::{
    RoleClient, ServiceExt,
    model::{CallToolRequestParams, CallToolResult, ContentBlock},
    service::RunningService,
};
use serde_json::Value;

/// A workspace named Garden holding some files, and the app data beside it.
pub struct Fixture {
    pub app_data: tempfile::TempDir,
    _parent: tempfile::TempDir,
    pub root: PathBuf,
}

impl Fixture {
    pub fn new(files: &[(&str, &[u8])]) -> Self {
        // A workspace under the current directory keeps notify's reported
        // paths in one form on macOS, where `/var` is also `/private/var`.
        let parent = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        std::fs::create_dir_all(&folder).unwrap();
        let workspace = WorkspaceService::new(app_data.path())
            .create("Garden", &folder)
            .unwrap();
        let root = PathBuf::from(workspace.root());
        drop(workspace);
        for (path, bytes) in files {
            write(&root, path, bytes);
        }
        Self {
            app_data,
            _parent: parent,
            root,
        }
    }

    /// The app with this workspace open.
    pub fn app(&self) -> App {
        let mut app = App::launch(self.app_data.path());
        settle(&mut app);
        assert!(app.workspace().is_some(), "the workspace didn't open");
        app
    }

    /// The workspace as the MCP server opens it, beside the app.
    pub fn agent(&self) -> Arc<AgentWorkspace> {
        Arc::new(AgentWorkspace::open(self.app_data.path(), &self.root).unwrap())
    }

    pub fn read(&self, relative: &str) -> String {
        std::fs::read_to_string(self.root.join(relative))
            .unwrap_or_else(|error| panic!("{relative} can't be read: {error}"))
    }
}

pub fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Poll until the app's background work finishes.
pub fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        if !app.is_busy() && !app.is_plugin_work_pending() {
            return;
        }
        assert!(Instant::now() < deadline, "background work never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// A managed note's bytes.
pub fn note(id_suffix: &str, body: &str) -> Vec<u8> {
    format!(
        "---\nid: note_{id_suffix}\ntype: note\ncreated: 2026-10-20T10:00:00Z\n\
         updated: 2026-10-20T10:00:00Z\n---\n{body}"
    )
    .into_bytes()
}

/// The component built from the crate whose library is `name`, or `None`
/// when it isn't built and CI doesn't require it.
pub fn built(name: &str) -> Option<Vec<u8>> {
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target"));
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

/// A plugin folder holding `manifest` and the component `library` builds,
/// removed when dropped, or `None` when it isn't built.
pub fn plugin_folder(manifest: &str, entry: &str, library: &str) -> Option<tempfile::TempDir> {
    let component = built(library)?;
    let folder = tempfile::tempdir().unwrap();
    std::fs::write(folder.path().join("plugin.toml"), manifest).unwrap();
    std::fs::write(folder.path().join(entry), component).unwrap();
    Some(folder)
}

/// Install the plugin in `folder` in the app, approving everything it
/// declares, and turn it on in the workspace.
pub fn install_and_enable(app: &mut App, folder: &Path, id: &str) {
    app.review_plugin_folder(folder.to_owned());
    let review = app.plugin_install().expect("the install dialog opens");
    assert_eq!(review.problem, None, "the folder is refused");
    assert!(app.approve_plugin_install(), "the plugin installs");
    settle(app);
    assert!(app.set_plugin_enabled(id, true), "{id} turns on");
    settle(app);
}

pub const TASKS_MANIFEST: &str = include_str!("../../../plugins/tasks/plugin.toml");
pub const PROJECTS_MANIFEST: &str = include_str!("../../../plugins/projects/plugin.toml");

/// The Tasks and Projects plugins installed and on in the app's workspace,
/// or `None` when they aren't built.
pub fn with_tasks_and_projects(app: &mut App) -> Option<(tempfile::TempDir, tempfile::TempDir)> {
    let tasks = plugin_folder(TASKS_MANIFEST, "tasks_plugin.wasm", "tasks_plugin")?;
    let projects = plugin_folder(PROJECTS_MANIFEST, "projects_plugin.wasm", "projects_plugin")?;
    install_and_enable(app, tasks.path(), "tasks");
    install_and_enable(app, projects.path(), "projects");
    Some((tasks, projects))
}

pub type Client = RunningService<RoleClient, ()>;

/// An MCP client connected to `server` in this process.
pub async fn connect(server: NouraMcp) -> Client {
    let (client_end, server_end) = tokio::io::duplex(1 << 20);
    tokio::spawn(async move {
        let running = server.serve(server_end).await.unwrap();
        let _ = running.waiting().await;
    });
    ().serve(client_end).await.unwrap()
}

/// Call the tool `name` with `arguments`.
pub async fn call(client: &Client, name: &str, arguments: Value) -> CallToolResult {
    let Value::Object(arguments) = arguments else {
        panic!("arguments must be an object");
    };
    client
        .call_tool(CallToolRequestParams::new(name.to_owned()).with_arguments(arguments))
        .await
        .unwrap()
}

/// The JSON a tool answered with, which must not be an error.
pub fn answer(result: &CallToolResult) -> Value {
    assert_ne!(
        result.is_error,
        Some(true),
        "the tool failed: {}",
        text(result)
    );
    serde_json::from_str(&text(result)).unwrap()
}

/// The JSON error a tool answered with.
pub fn failure(result: &CallToolResult) -> Value {
    assert_eq!(
        result.is_error,
        Some(true),
        "the tool didn't fail: {}",
        text(result)
    );
    serde_json::from_str(&text(result)).unwrap()
}

fn text(result: &CallToolResult) -> String {
    result
        .content
        .iter()
        .filter_map(|content| match content {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect()
}
