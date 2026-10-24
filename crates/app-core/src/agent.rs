//! What an AI tool outside Noura, such as Claude Desktop or Cursor,
//! reaches through MCP: one workspace, with the same search, file rules
//! and plugins as the app. It runs beside the app on this device's app
//! data, so the two share the workspace's index and write lock, and each
//! sees the other's changes as outside edits. The plugins the workspace
//! enables start with this device's approvals, and only those that may
//! offer tools offer them.

use std::{
    path::Path,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

use crate::{
    AppError, NoSystem, PluginTool, System, Workspace, WorkspaceService,
    plugins::{Plugins, SystemClock},
    watch::{Rescans, Snapshot, Watch},
};

/// How long opening waits for the enabled plugins to say what they offer.
/// A plugin that hasn't answered by then offers no tools.
const PLUGIN_START_WAIT: Duration = Duration::from_secs(30);

/// A workspace as an AI tool outside Noura uses it. Searches and reads
/// go through [`Workspace`], as the app's do, and so do note changes,
/// which reach disk before they return.
pub struct AgentWorkspace {
    workspace: Arc<Workspace>,
    plugins: Mutex<Plugins>,
    /// Keeps the index in step with the folder while the app isn't
    /// running.
    _watch: Option<Watch>,
}

impl AgentWorkspace {
    /// Open the workspace in `folder`, with per-device state in
    /// `app_data`, and start the plugins it enables that this device
    /// installed and approved. The folder must hold a workspace already,
    /// and the recent list stays as it is.
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
        plugins.open_workspace(Some(workspace.clone()));
        let deadline = Instant::now() + PLUGIN_START_WAIT;
        loop {
            plugins.poll();
            if !plugins.is_working() {
                break;
            }
            if Instant::now() >= deadline {
                tracing::warn!("a plugin didn't say what it offers in time");
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(Self {
            workspace,
            plugins: Mutex::new(plugins),
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
    /// device.
    pub fn plugin_tools(&self) -> Vec<PluginTool> {
        self.plugins().tools()
    }

    /// Call the tool `name` of the plugin `plugin_id` with JSON
    /// `arguments`, and wait for its answer. Arguments that don't match
    /// the tool's schema never reach the plugin, and neither does a call
    /// once the plugin was turned off in the workspace or lost the `tools`
    /// capability on this device. A plugin that crashes or runs past its
    /// limits answers with an error, and its next call starts afresh.
    pub fn call_plugin_tool(
        &self,
        plugin_id: &str,
        name: &str,
        arguments: &str,
    ) -> Result<String, AppError> {
        let call = self.plugins().agent_tool_call(plugin_id, name, arguments)?;
        call.run()
    }

    fn plugins(&self) -> MutexGuard<'_, Plugins> {
        self.plugins.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
