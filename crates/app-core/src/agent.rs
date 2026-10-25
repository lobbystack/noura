//! What an AI tool outside Noura, such as Claude Desktop or Cursor,
//! reaches through MCP: one workspace, with the same search, file rules
//! and plugins as the app. It runs beside the app on this device's app
//! data, so the two share the workspace's index and write lock, and each
//! sees the other's changes as outside edits. The plugins the workspace
//! enables start with this device's approvals, and only those that may
//! offer tools offer them.

use std::{
    path::Path,
    sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use crate::{
    AppError, NoSystem, PluginTool, System, Workspace, WorkspaceService,
    plugins::{Plugins, SystemClock},
    watch::{Rescans, Snapshot, Watch},
};

/// How long the enabled plugins have to say what they offer once their
/// code loaded. A plugin that hasn't answered by then offers no tools.
const PLUGIN_START_WAIT: Duration = Duration::from_secs(30);

/// A workspace as an AI tool outside Noura uses it. Searches and reads
/// go through [`Workspace`], as the app's do, and so do note changes,
/// which reach disk before they return.
pub struct AgentWorkspace {
    workspace: Arc<Workspace>,
    plugins: Arc<Mutex<Plugins>>,
    /// Set once the plugins finished starting, so their tools are known.
    started: Arc<Started>,
    /// Keeps the index in step with the folder while the app isn't
    /// running.
    _watch: Option<Watch>,
}

/// Whether the plugins finished starting.
#[derive(Default)]
struct Started {
    done: Mutex<bool>,
    changed: Condvar,
}

impl Started {
    fn finish(&self) {
        *self.done.lock().unwrap_or_else(PoisonError::into_inner) = true;
        self.changed.notify_all();
    }

    fn wait(&self) {
        let mut done = self.done.lock().unwrap_or_else(PoisonError::into_inner);
        while !*done {
            done = self
                .changed
                .wait(done)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    fn is_done(&self) -> bool {
        *self.done.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// Marks the start finished when dropped, after a panic too, so nobody
/// waits for a start that ended.
struct Finished<'a>(&'a Started);

impl Drop for Finished<'_> {
    fn drop(&mut self) {
        self.0.finish();
    }
}

impl AgentWorkspace {
    /// Open the workspace in `folder`, with per-device state in
    /// `app_data`, and start the plugins it enables that this device
    /// installed and approved. The folder must hold a workspace already,
    /// and the recent list stays as it is. The plugins start on a thread
    /// of their own, so the workspace answers before their code loads;
    /// [`Self::plugin_tools`] and [`Self::call_plugin_tool`] wait for them.
    pub fn open(app_data: impl AsRef<Path>, folder: &Path) -> Result<Self, AppError> {
        Self::open_with(app_data, folder, Arc::new(NoSystem))
    }

    /// [`Self::open`], with `system` for the clock plugins read.
    pub fn open_with(
        app_data: impl AsRef<Path>,
        folder: &Path,
        system: Arc<dyn System>,
    ) -> Result<Self, AppError> {
        let app_data = app_data.as_ref();
        let service = WorkspaceService::with_system(app_data, system.clone());
        let workspace = Arc::new(service.open_existing(folder)?);
        let watch = match Snapshot::of(workspace.engine(), workspace.root()) {
            Ok((snapshot, counts)) => Watch::start(
                workspace.shared_engine(),
                workspace.root().to_owned(),
                snapshot,
                counts,
                Rescans::default(),
                Box::new(|_| true),
            ),
            Err(error) => {
                tracing::warn!(code = %error.code, "the workspace isn't watched for outside edits");
                None
            }
        };
        let mut plugins = Plugins::for_agent(app_data);
        plugins.set_clock(Arc::new(SystemClock(system)));
        let plugins = Arc::new(Mutex::new(plugins));
        let started = Arc::new(Started::default());
        let start = {
            let (plugins, started, workspace) =
                (plugins.clone(), started.clone(), workspace.clone());
            move || {
                let _finished = Finished(&started);
                start_plugins(&plugins, workspace);
            }
        };
        if let Err(error) = std::thread::Builder::new()
            .name("agent-plugins".into())
            .spawn(start.clone())
        {
            tracing::warn!(%error, "the plugins' thread didn't start");
            start();
        }
        Ok(Self {
            workspace,
            plugins,
            started,
            _watch: watch,
        })
    }

    /// The open workspace, whose search, reads and note changes the tool
    /// uses.
    pub fn workspace(&self) -> &Workspace {
        &self.workspace
    }

    /// The tools of the plugins that started: each plugin the workspace
    /// enabled when it opened, which holds the `tools` capability on this
    /// device. Waits for the plugins to finish starting.
    pub fn plugin_tools(&self) -> Vec<PluginTool> {
        self.started.wait();
        self.plugins().tools()
    }

    /// Whether the plugins finished starting, so [`Self::plugin_tools`]
    /// answers at once.
    pub fn plugins_started(&self) -> bool {
        self.started.is_done()
    }

    /// Call the tool `name` of the plugin `plugin_id` with JSON
    /// `arguments`, and wait for its answer. Arguments that don't match
    /// the tool's schema never reach the plugin, and neither does a call
    /// once the plugin was turned off in the workspace or lost the `tools`
    /// capability on this device. A plugin that crashes or runs past its
    /// limits answers with an error, and its next call starts afresh.
    /// Waits for the plugins to finish starting.
    pub fn call_plugin_tool(
        &self,
        plugin_id: &str,
        name: &str,
        arguments: &str,
    ) -> Result<String, AppError> {
        self.started.wait();
        let call = self.plugins().agent_tool_call(plugin_id, name, arguments)?;
        call.run()
    }

    fn plugins(&self) -> MutexGuard<'_, Plugins> {
        lock(&self.plugins)
    }
}

/// Start the plugins `workspace` enables, and wait up to
/// [`PLUGIN_START_WAIT`] for them to say what they offer. The lock is
/// taken a moment at a time while they answer.
fn start_plugins(plugins: &Mutex<Plugins>, workspace: Arc<Workspace>) {
    lock(plugins).open_workspace(Some(workspace));
    let deadline = Instant::now() + PLUGIN_START_WAIT;
    loop {
        {
            let mut plugins = lock(plugins);
            plugins.poll();
            if !plugins.is_working() {
                return;
            }
        }
        if Instant::now() >= deadline {
            tracing::warn!("a plugin didn't say what it offers in time");
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn lock(plugins: &Mutex<Plugins>) -> MutexGuard<'_, Plugins> {
    plugins.lock().unwrap_or_else(PoisonError::into_inner)
}
