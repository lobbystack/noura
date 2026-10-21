//! The plugins service: installs plugins from folders once you approve
//! their capabilities, keeps those approvals in this device's app data,
//! turns plugins on and off in the open workspace's manifest, runs the
//! ones that are on, and reloads a plugin when a new build lands in its
//! folder.
//!
//! Every window shares one service. Installing a plugin from a folder is
//! Noura's dev mode: the folder stays where it is, and the service copies
//! each build into the plugin host's library, which compiles it on this
//! device. Approvals never come from workspace files, so a workspace that
//! lists a plugin as enabled runs none of its code on a device where you
//! haven't installed and approved it. The plugins that ship with Noura
//! wait for one approval on this device's first-run screen.
//!
//! Each running plugin's calls run on a thread of its own, so a plugin that
//! loops, allocates past its memory cap or crashes never holds up a
//! window: its views, commands, Inbox sections, Settings page and tools
//! update when its answers arrive, through [`Plugins::poll`].

mod reader;
mod reload;
mod worker;

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
};

use local_core::HostRequests;
use plugin_host::{
    Candidate, Capability, Event, Grant, InstallError, PluginHost, PluginManifest, Router,
    RunningPlugin, Services, Surface, View, WorkspaceReads,
};
use serde::{Deserialize, Serialize};

use crate::{AppError, Workspace, store::JsonStore};
use reload::{Reload, Watcher};
use worker::{Job, Outcome, Registrations, ToolEntry, Worker};

/// Where the plugin host keeps installed plugins and their compiled code,
/// under the app data folder.
const LIBRARY: &str = "plugins/library";

/// This device's install records and approvals, under the app data folder.
const RECORDS: &str = "plugins/installed.json";

/// Rail pages that plugins add have IDs that start with this, then the
/// plugin's ID and the page's own ID.
const PAGE_PREFIX: &str = "plugin:";

/// The IDs of plugins' pages in Settings start with this, then the
/// plugin's ID.
pub(crate) const SETTINGS_PREFIX: &str = "plugin-settings:";

/// The IDs of plugins' commands in the command palette start with this,
/// then the plugin's ID and the command's own ID.
pub(crate) const COMMAND_PREFIX: &str = "plugin-command:";

/// A capability, or one host or target of one, as Settings, the install
/// dialog and the first-run screen show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityLine {
    /// The name the manifest uses, such as `workspace-read`.
    pub name: String,
    /// What approving it lets the plugin do.
    pub description: String,
    /// What revoking or allowing this line changes.
    pub grant: Grant,
}

/// The lines for `capabilities`: one per capability, and one per network
/// host and command target.
pub(crate) fn lines(capabilities: &[Capability]) -> Vec<CapabilityLine> {
    capabilities
        .iter()
        .flat_map(|capability| {
            capability
                .grants()
                .into_iter()
                .map(|(grant, description)| CapabilityLine {
                    name: capability.name().to_owned(),
                    description,
                    grant,
                })
        })
        .collect()
}

/// A plugin as the Plugins page lists it: installed on this device,
/// enabled in the open workspace, or both.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginRow {
    pub id: String,
    /// The manifest's name, or the ID when it isn't installed here.
    pub name: String,
    pub version: Option<String>,
    /// The folder it was installed from.
    pub folder: Option<String>,
    /// What it may use on this device: each capability, network host and
    /// command target it declares that you approved.
    pub approved: Vec<CapabilityLine>,
    /// What it declares that you revoked, which you can allow again.
    pub revoked: Vec<CapabilityLine>,
    /// Whether it is installed on this device.
    pub installed: bool,
    /// Whether it ships with Noura.
    pub shipped: bool,
    /// Whether the open workspace's manifest lists it as enabled.
    pub enabled: bool,
    pub running: bool,
    /// It failed too often and stopped for this session. Starting it
    /// again leaves the workspace's enabled plugins list as it is.
    pub failed: bool,
    /// Why it isn't running or its last reload failed.
    pub problem: Option<String>,
    /// What a new build in its folder adds, which waits for your approval
    /// before the build runs. Empty when no build waits.
    pub update: Vec<CapabilityLine>,
}

/// The install dialog: a plugin folder, read and compiled but not run,
/// with the capabilities it asks for, or why Noura won't install it.
#[derive(Clone)]
pub struct PluginInstall {
    /// The folder you chose.
    pub folder: String,
    pub id: String,
    pub name: String,
    pub version: String,
    /// Every capability, network host and command target the manifest
    /// declares, in its order.
    pub capabilities: Vec<CapabilityLine>,
    /// Why the folder can't be installed. Nothing installs while it is
    /// set.
    pub problem: Option<String>,
    candidate: Option<Candidate>,
}

impl std::fmt::Debug for PluginInstall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginInstall")
            .field("folder", &self.folder)
            .field("id", &self.id)
            .field("capabilities", &self.capabilities)
            .field("problem", &self.problem)
            .finish_non_exhaustive()
    }
}

/// A plugin that ships with Noura, as the first-run screen lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippedPlugin {
    pub id: String,
    pub name: String,
    pub version: String,
    /// Every capability, network host and command target it declares.
    pub capabilities: Vec<CapabilityLine>,
}

/// A tool a running plugin offers to AI assistants.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginTool {
    pub plugin_id: String,
    pub name: String,
    pub description: String,
    /// The JSON Schema, as JSON text, that the tool's arguments match.
    pub input_schema: String,
}

/// What a plugin view shows: the view, or why it can't be drawn.
pub type PluginView = Result<View, String>;

/// A running plugin's Inbox section.
pub(crate) struct PluginSection {
    pub(crate) plugin_id: String,
    pub(crate) section_id: String,
    pub(crate) title: String,
    /// The plugin's first rail page, whose place the section takes among
    /// the Inbox's sections.
    pub(crate) page_id: Option<String>,
    pub(crate) content: PluginView,
}

/// This device's record of an installed plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    folder: String,
    /// What you approved. A plugin may use what it declares and this
    /// holds.
    approved: Vec<Capability>,
    /// It ships with Noura, and you approved it on the first-run screen.
    #[serde(default)]
    shipped: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Records {
    #[serde(default)]
    plugins: BTreeMap<String, Record>,
    /// The plugins shipping with Noura that this device's first-run screen
    /// showed you.
    #[serde(default)]
    shipped_reviewed: BTreeSet<String>,
}

/// A running plugin, the thread its calls run on, and what it registered
/// and drew last.
struct Running {
    plugin: Arc<RunningPlugin>,
    /// Which start this is, so an answer from an earlier one is ignored.
    generation: u64,
    jobs: Sender<Job>,
    /// Jobs sent whose answers the service hasn't taken yet.
    outstanding: usize,
    redraw_queued: Arc<AtomicBool>,
    registrations: Registrations,
    views: BTreeMap<Surface, PluginView>,
    /// It failed too often and stopped for this session.
    failed: bool,
}

type Wake = Arc<dyn Fn() + Send + Sync>;

/// A tool call checked against the tool's schema, ready to run off the
/// service's lock.
pub(crate) struct ToolCall {
    plugin: Arc<RunningPlugin>,
    name: String,
    arguments: String,
}

impl ToolCall {
    /// Run the tool in its plugin and wait for its answer.
    pub(crate) fn run(&self) -> Result<String, AppError> {
        match self.plugin.call_tool(&self.name, &self.arguments) {
            Ok(Ok(answer)) => Ok(answer),
            Ok(Err(message)) => Err(AppError::new("plugin_tool_failed", message)),
            Err(error) => {
                tracing::warn!(%error, "a plugin tool failed");
                Err(AppError::new(
                    "plugin_failed",
                    worker::failure_message(&error),
                ))
            }
        }
    }

    pub(crate) fn plugin_id(&self) -> &str {
        self.plugin.id()
    }
}

pub(crate) struct Plugins {
    app_data: PathBuf,
    store: JsonStore<Records>,
    /// What the store holds. Every window shares this service, so only it
    /// writes the store.
    records: Records,
    host: Option<Arc<PluginHost>>,
    /// The installed plugins' manifests, read once rather than per frame.
    manifests: BTreeMap<String, PluginManifest>,
    /// The workspace the running plugins read.
    workspace: Option<Arc<Workspace>>,
    /// Carries one running plugin's command calls to another.
    router: Arc<Router>,
    /// The plugins whose commands another plugin called, to draw again.
    called: Receiver<String>,
    /// Sends plugins' requests to their declared hosts.
    network: Arc<HostRequests>,
    running: BTreeMap<String, Running>,
    generations: u64,
    outcomes: (Sender<Outcome>, Receiver<Outcome>),
    problems: BTreeMap<String, String>,
    /// New builds that wait for your approval of what they add.
    updates: BTreeMap<String, Candidate>,
    watchers: BTreeMap<String, Watcher>,
    reload_sender: Sender<Reload>,
    reloads: Receiver<Reload>,
    /// The folder holding the plugins that ship with Noura.
    shipped: Option<PathBuf>,
    /// The listing whose file changes the running plugins drew last.
    listing_drawn: u64,
    /// Wakes every window, as a reload watcher does once it read a build.
    wake: Option<Wake>,
    /// Counts the changes the windows show: plugins, pages and views.
    version: u64,
}

impl Plugins {
    pub(crate) fn new(app_data: &Path) -> Self {
        let (reload_sender, reloads) = channel();
        // A missing or unreadable store holds no approvals, so no plugin
        // runs until you install it again.
        let store = JsonStore::new(app_data.join(RECORDS), "plugin approvals");
        let records = store.load();
        let router = Arc::new(Router::default());
        let (called_sender, called) = channel();
        router.on_called(Arc::new(move |id: &str| {
            let _ = called_sender.send(id.to_owned());
        }));
        Self {
            app_data: app_data.to_owned(),
            store,
            records,
            host: None,
            manifests: BTreeMap::new(),
            workspace: None,
            router,
            called,
            network: Arc::new(HostRequests::https()),
            running: BTreeMap::new(),
            generations: 0,
            outcomes: channel(),
            problems: BTreeMap::new(),
            updates: BTreeMap::new(),
            watchers: BTreeMap::new(),
            reload_sender,
            reloads,
            shipped: None,
            listing_drawn: 0,
            wake: None,
            version: 0,
        }
    }

    pub(crate) fn set_wake(&mut self, wake: Wake) {
        self.wake = Some(wake);
    }

    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    fn changed(&mut self) {
        self.version += 1;
    }

    /// Whether a running plugin has calls whose answers haven't been
    /// taken yet.
    pub(crate) fn is_working(&self) -> bool {
        self.running.values().any(|running| running.outstanding > 0)
    }

    /// Change the records and save them. Returns the error to show when
    /// they couldn't be saved; the records then stay as they were.
    fn save_records(&mut self, change: impl FnOnce(&mut Records)) -> Result<(), AppError> {
        let mut records = Records {
            plugins: self.records.plugins.clone(),
            shipped_reviewed: self.records.shipped_reviewed.clone(),
        };
        change(&mut records);
        self.store.save(&records).map_err(|error| {
            tracing::warn!(%error, "the plugin approvals weren't saved");
            AppError::new(
                "plugin_approvals_unsaved",
                "Noura couldn't save the plugin approvals in its app data.",
            )
        })?;
        self.records = records;
        Ok(())
    }

    /// The plugin host, started the first time something needs it. Starting
    /// it drops library entries this device has no record of, as after the
    /// records file was lost or couldn't be read: such an entry holds no
    /// approval, and installing the plugin from its folder again needs its
    /// ID free. It also watches each installed plugin's folder for new
    /// builds.
    fn host(&mut self) -> Result<Arc<PluginHost>, AppError> {
        if let Some(host) = &self.host {
            return Ok(host.clone());
        }
        let host = Arc::new(
            PluginHost::new(self.app_data.join(LIBRARY)).map_err(|error| from_install(&error))?,
        );
        self.host = Some(host.clone());
        let records = self.records.plugins.clone();
        for id in host.installed_ids() {
            if !records.contains_key(&id)
                && let Err(error) = host.uninstall(&id)
            {
                tracing::warn!(
                    code = error.code.as_str(),
                    "a plugin without approvals stayed"
                );
            }
        }
        for (id, record) in records {
            match host.installed(&id) {
                Ok((manifest, digest)) => {
                    self.manifests.insert(id.clone(), manifest);
                    self.watch(&id, &record.folder, digest);
                }
                Err(error) => {
                    tracing::warn!(
                        code = error.code.as_str(),
                        "an installed plugin can't be read"
                    );
                    self.problems.insert(id, error.message);
                }
            }
        }
        Ok(host)
    }

    /// The rows of the Plugins page: every plugin installed here or
    /// enabled in the open workspace, by ID.
    pub(crate) fn rows(&mut self) -> Vec<PluginRow> {
        if !self.records.plugins.is_empty()
            && let Err(error) = self.host()
        {
            tracing::warn!(code = %error.code, "the plugin host didn't start");
        }
        let records = &self.records.plugins;
        let enabled = self.enabled();
        let mut ids: Vec<&String> = records.keys().chain(enabled.iter()).collect();
        ids.sort();
        ids.dedup();
        ids.into_iter()
            .map(|id| {
                let record = records.get(id);
                let manifest = self.manifests.get(id).filter(|_| record.is_some());
                let (approved, revoked) = match (manifest, record) {
                    (Some(manifest), Some(record)) => {
                        let granted = Capability::granted(&manifest.capabilities, &record.approved);
                        let revoked = Capability::added(&granted, &manifest.capabilities);
                        (lines(&granted), lines(&revoked))
                    }
                    (None, Some(record)) => (lines(&record.approved), Vec::new()),
                    _ => (Vec::new(), Vec::new()),
                };
                let update = match (self.updates.get(id), manifest) {
                    (Some(candidate), Some(manifest)) => lines(&Capability::added(
                        &manifest.capabilities,
                        &candidate.manifest().capabilities,
                    )),
                    _ => Vec::new(),
                };
                let running = self.running.get(id);
                PluginRow {
                    id: id.clone(),
                    name: manifest.map_or_else(|| id.clone(), |manifest| manifest.name.clone()),
                    version: manifest.map(|manifest| manifest.version.to_string()),
                    folder: record.map(|record| record.folder.clone()),
                    approved,
                    revoked,
                    installed: record.is_some(),
                    shipped: record.is_some_and(|record| record.shipped),
                    enabled: enabled.contains(id),
                    running: running.is_some_and(|running| !running.failed),
                    failed: running.is_some_and(|running| running.failed),
                    problem: self.problems.get(id).cloned(),
                    update,
                }
            })
            .collect()
    }

    /// The plugin IDs the open workspace's manifest lists as enabled.
    fn enabled(&self) -> Vec<String> {
        self.workspace
            .as_ref()
            .map(|workspace| workspace.engine().manifest().enabled_plugins)
            .unwrap_or_default()
    }

    /// Run the plugins `workspace` enables that this device installed and
    /// approved, after stopping the ones another workspace ran.
    pub(crate) fn open_workspace(&mut self, workspace: Option<Arc<Workspace>>) {
        let ids: Vec<String> = self.running.keys().cloned().collect();
        for id in ids {
            self.stop(&id);
        }
        self.workspace = workspace;
        self.listing_drawn = 0;
        self.changed();
        if self.workspace.is_none() || self.records.plugins.is_empty() {
            return;
        }
        if let Err(error) = self.host() {
            tracing::warn!(code = %error.code, "the plugin host didn't start");
            return;
        }
        for id in self.enabled() {
            self.start(&id);
        }
        self.changed();
    }

    /// Stop the plugin `id`: its thread ends, other plugins can no longer
    /// call its commands, and what it registered goes.
    fn stop(&mut self, id: &str) {
        self.running.remove(id);
        self.router.remove(id);
    }

    /// Start the installed plugin `id` with what you approved, and ask it
    /// what it registers. Its code runs on its own thread. A plugin that
    /// was running, as before a reload, keeps showing what it registered
    /// and drew until the new start answers, so its open pages stay.
    fn start(&mut self, id: &str) {
        let (registrations, views) = self
            .running
            .get(id)
            .filter(|running| !running.failed)
            .map(|running| (running.registrations.clone(), running.views.clone()))
            .unwrap_or_default();
        self.stop(id);
        self.problems.remove(id);
        let Some(record) = self.records.plugins.get(id).cloned() else {
            return;
        };
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let loaded = match self.host().and_then(|host| {
            let loaded = host.load(id).map_err(|error| from_install(&error))?;
            Ok((host, loaded))
        }) {
            Ok(loaded) => loaded,
            Err(error) => {
                self.problems.insert(id.to_owned(), error.message);
                self.changed();
                return;
            }
        };
        let (host, loaded) = loaded;
        let reader: Arc<dyn WorkspaceReads> = Arc::new(reader::WorkspaceReader(workspace));
        let services = Services {
            workspace: Some(reader),
            network: Some(self.network.clone()),
            router: Arc::downgrade(&self.router),
        };
        let plugin = host.start(&loaded, &record.approved, services);
        self.manifests
            .insert(id.to_owned(), loaded.manifest().clone());
        self.router.insert(plugin.clone());
        self.generations += 1;
        let (jobs, received) = channel();
        let redraw_queued = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            id: id.to_owned(),
            generation: self.generations,
            plugin: plugin.clone(),
            jobs: received,
            outcomes: self.outcomes.0.clone(),
            wake: self.wake.clone(),
            redraw_queued: redraw_queued.clone(),
        };
        if let Err(error) = std::thread::Builder::new()
            .name(format!("plugin-{id}"))
            .spawn(move || worker.run())
        {
            tracing::warn!(%error, "a plugin's thread didn't start");
            self.router.remove(id);
            self.problems
                .insert(id.to_owned(), "It failed to start.".to_owned());
            self.changed();
            return;
        }
        self.running.insert(
            id.to_owned(),
            Running {
                plugin,
                generation: self.generations,
                jobs,
                outstanding: 0,
                redraw_queued,
                registrations,
                views,
                failed: false,
            },
        );
        self.send(id, Job::Refresh);
        self.changed();
    }

    /// Send `job` to the running plugin `id`.
    fn send(&mut self, id: &str, job: Job) {
        let Some(running) = self.running.get_mut(id) else {
            return;
        };
        if running.failed {
            return;
        }
        if matches!(job, Job::Redraw) && running.redraw_queued.swap(true, Ordering::Relaxed) {
            return;
        }
        if running.jobs.send(job).is_ok() {
            running.outstanding += 1;
        }
    }

    /// Read the plugin folder `folder` for the install dialog, without
    /// running any of its code.
    pub(crate) fn review(&mut self, folder: &Path) -> PluginInstall {
        let shown = folder.to_string_lossy().into_owned();
        let refused = |problem: String| PluginInstall {
            folder: shown.clone(),
            id: String::new(),
            name: String::new(),
            version: String::new(),
            capabilities: Vec::new(),
            problem: Some(problem),
            candidate: None,
        };
        let Some(path) = folder.to_str() else {
            return refused(AppError::unsupported_path().message);
        };
        let host = match self.host() {
            Ok(host) => host,
            Err(error) => return refused(error.message),
        };
        match host.inspect(folder) {
            Ok(candidate) => {
                let manifest = candidate.manifest();
                PluginInstall {
                    folder: path.to_owned(),
                    id: manifest.id.clone(),
                    name: manifest.name.clone(),
                    version: manifest.version.to_string(),
                    capabilities: lines(&manifest.capabilities),
                    problem: None,
                    candidate: Some(candidate),
                }
            }
            Err(error) => refused(error.message),
        }
    }

    /// Install the plugin the dialog shows, approving every capability it
    /// lists. A plugin the open workspace enables starts at once.
    pub(crate) fn install(&mut self, install: &PluginInstall) -> Result<(), AppError> {
        let Some(candidate) = &install.candidate else {
            return Err(AppError::new(
                "plugin_refused",
                install.problem.clone().unwrap_or_default(),
            ));
        };
        self.install_candidate(candidate, &install.folder, false)
    }

    fn install_candidate(
        &mut self,
        candidate: &Candidate,
        folder: &str,
        shipped: bool,
    ) -> Result<(), AppError> {
        let host = self.host()?;
        host.install(candidate)
            .map_err(|error| from_install(&error))?;
        let id = candidate.manifest().id.clone();
        let record = Record {
            folder: folder.to_owned(),
            approved: candidate.manifest().capabilities.clone(),
            shipped,
        };
        if let Err(error) = self.save_records(|records| {
            records.plugins.insert(id.clone(), record);
            if shipped {
                records.shipped_reviewed.insert(id.clone());
            }
        }) {
            let _ = host.uninstall(&id);
            return Err(error);
        }
        self.manifests
            .insert(id.clone(), candidate.manifest().clone());
        self.watch(&id, folder, candidate.digest());
        if self.enabled().contains(&id) {
            self.start(&id);
        }
        self.changed();
        Ok(())
    }

    /// Turn the plugin `id` on or off in the open workspace: rewrite the
    /// manifest's enabled plugins list, then start or stop the plugin.
    pub(crate) fn set_enabled(&mut self, id: &str, enabled: bool) -> Result<(), AppError> {
        let Some(workspace) = self.workspace.clone() else {
            return Err(AppError::new(
                "no_workspace",
                "Open a workspace to turn plugins on or off.",
            ));
        };
        workspace.set_plugin_enabled(id, enabled)?;
        if enabled {
            if self.records.plugins.contains_key(id) {
                self.start(id);
            }
        } else {
            self.stop(id);
            self.problems.remove(id);
        }
        self.changed();
        Ok(())
    }

    /// Remove the plugin `id` from this device: its code, its compiled
    /// code and its approvals. The workspace's enabled list stays, for
    /// your other devices.
    pub(crate) fn uninstall(&mut self, id: &str) -> Result<(), AppError> {
        let host = self.host()?;
        self.stop(id);
        self.watchers.remove(id);
        self.updates.remove(id);
        self.problems.remove(id);
        self.manifests.remove(id);
        self.save_records(|records| {
            records.plugins.remove(id);
        })?;
        let removed = host.uninstall(id).map_err(|error| from_install(&error));
        self.changed();
        removed
    }

    /// Approve what a waiting build adds, and run it.
    pub(crate) fn approve_update(&mut self, id: &str) -> Result<(), AppError> {
        let Some(candidate) = self.updates.remove(id) else {
            return Err(no_update());
        };
        if !self.records.plugins.contains_key(id) {
            return Err(AppError::new(
                "plugin_not_installed",
                "This plugin isn't installed on this device.",
            ));
        }
        let added = self.added(id, &candidate);
        self.save_records(|records| {
            if let Some(record) = records.plugins.get_mut(id) {
                Capability::approve(&mut record.approved, &added);
            }
        })?;
        self.apply(id, &candidate).inspect_err(|error| {
            self.failed_update(id, error);
        })
    }

    /// Keep the installed build of the plugin `id` and drop the waiting
    /// one. Its folder offers a build again once it changes.
    pub(crate) fn decline_update(&mut self, id: &str) -> Result<(), AppError> {
        if self.updates.remove(id).is_none() {
            return Err(no_update());
        }
        self.changed();
        Ok(())
    }

    /// What `candidate` declares that the installed build of `id` doesn't.
    fn added(&self, id: &str, candidate: &Candidate) -> Vec<Capability> {
        let installed = self
            .manifests
            .get(id)
            .map(|manifest| manifest.capabilities.as_slice())
            .unwrap_or_default();
        Capability::added(installed, &candidate.manifest().capabilities)
    }

    /// Take `grant` away from the plugin `id`. Its next call sees the
    /// change, and what it registered under a revoked capability goes.
    pub(crate) fn revoke(&mut self, id: &str, grant: &Grant) -> Result<(), AppError> {
        self.change_approval(id, |approved, _| Capability::revoke(approved, grant))
    }

    /// Give the plugin `id` back something it declares that you revoked.
    pub(crate) fn allow(&mut self, id: &str, grant: &Grant) -> Result<(), AppError> {
        self.change_approval(id, |approved, declared| {
            let wanted = declared
                .iter()
                .find_map(|capability| match (capability, grant) {
                    (Capability::Network { hosts }, Grant::Host(host)) if hosts.contains(host) => {
                        Some(Capability::Network {
                            hosts: vec![host.clone()],
                        })
                    }
                    (Capability::Commands { targets }, Grant::Target(target))
                        if targets.contains(target) =>
                    {
                        Some(Capability::Commands {
                            targets: vec![target.clone()],
                        })
                    }
                    (Capability::Commands { .. }, Grant::Capability(name))
                        if name == "commands" =>
                    {
                        Some(Capability::Commands {
                            targets: Vec::new(),
                        })
                    }
                    (capability, Grant::Capability(name)) if capability.name() == name => {
                        Some(capability.clone())
                    }
                    _ => None,
                });
            if let Some(wanted) = wanted {
                Capability::approve(approved, &[wanted]);
            }
        })
    }

    fn change_approval(
        &mut self,
        id: &str,
        change: impl FnOnce(&mut Vec<Capability>, &[Capability]),
    ) -> Result<(), AppError> {
        let Some(record) = self.records.plugins.get(id) else {
            return Err(AppError::new(
                "plugin_not_installed",
                "This plugin isn't installed on this device.",
            ));
        };
        let declared = self
            .manifests
            .get(id)
            .map(|manifest| manifest.capabilities.clone())
            .unwrap_or_default();
        let mut approved = record.approved.clone();
        change(&mut approved, &declared);
        let saved = approved.clone();
        self.save_records(|records| {
            if let Some(record) = records.plugins.get_mut(id) {
                record.approved = saved;
            }
        })?;
        if let Some(running) = self.running.get_mut(id) {
            running.plugin.set_approved(&approved);
            let granted = running.plugin.granted();
            running
                .registrations
                .keep(|name| Capability::holds(&granted, name));
            let surfaces = running.registrations.surfaces();
            running
                .views
                .retain(|surface, _| surfaces.contains(surface));
        }
        self.send(id, Job::Refresh);
        self.changed();
        Ok(())
    }

    /// Start the plugin `id` again after it failed too often, for this
    /// session.
    pub(crate) fn restart(&mut self, id: &str) -> Result<(), AppError> {
        let Some(running) = self.running.get_mut(id) else {
            return Err(AppError::new(
                "plugin_not_running",
                "This plugin isn't on in this workspace.",
            ));
        };
        running.plugin.restart();
        running.failed = false;
        self.problems.remove(id);
        self.send(id, Job::Refresh);
        self.changed();
        Ok(())
    }

    /// Take the builds the reload watchers found and the answers the
    /// running plugins sent. A build that adds nothing to what the
    /// installed build declares replaces it at once; any other waits for
    /// your approval while the old build keeps running.
    pub(crate) fn poll(&mut self) {
        while let Ok(reload) = self.reloads.try_recv() {
            if !self.records.plugins.contains_key(&reload.id) {
                continue;
            }
            match reload.result {
                Ok(candidate) => {
                    if self.added(&reload.id, &candidate).is_empty() {
                        self.updates.remove(&reload.id);
                        if let Err(error) = self.apply(&reload.id, &candidate) {
                            self.failed_update(&reload.id, &error);
                        }
                    } else {
                        self.updates.insert(reload.id.clone(), candidate);
                    }
                }
                Err(error) => {
                    self.problems.insert(
                        reload.id.clone(),
                        format!("The new build didn't load: {}", error.message),
                    );
                }
            }
            self.changed();
        }
        while let Ok(outcome) = self.outcomes.1.try_recv() {
            self.take(outcome);
        }
        let called: Vec<String> = self.called.try_iter().collect();
        for id in called {
            self.send(&id, Job::Redraw);
        }
    }

    /// Show what a running plugin's job changed.
    fn take(&mut self, outcome: Outcome) {
        let Some(running) = self
            .running
            .get_mut(&outcome.id)
            .filter(|running| running.generation == outcome.generation)
        else {
            return;
        };
        running.outstanding = running.outstanding.saturating_sub(1);
        if let Some(registrations) = outcome.registrations {
            running.registrations = registrations;
        }
        running.views = outcome.views;
        if outcome.health.stopped && !running.failed {
            running.failed = true;
            running.registrations = Registrations::default();
            running.views.clear();
            self.router.remove(&outcome.id);
            self.problems.insert(
                outcome.id.clone(),
                "It failed too often and stopped for this session.".to_owned(),
            );
        }
        self.changed();
    }

    /// Show why a new build of the plugin `id` didn't install, and let its
    /// watcher offer that build again when the folder next changes.
    fn failed_update(&mut self, id: &str, error: &AppError) {
        if let Some(watcher) = self.watchers.get(id) {
            watcher.forget_offered();
        }
        self.problems.insert(
            id.to_owned(),
            format!("{} Save the build again to retry.", error.message),
        );
        self.changed();
    }

    /// Install `candidate` over the plugin `id`, and run it in place of the
    /// old build when the plugin is running. What you approved for
    /// capabilities the new build no longer declares goes.
    fn apply(&mut self, id: &str, candidate: &Candidate) -> Result<(), AppError> {
        let host = self.host()?;
        host.update(candidate)
            .map_err(|error| from_install(&error))?;
        let declared = candidate.manifest().capabilities.clone();
        let pruned = self.save_records(|records| {
            if let Some(record) = records.plugins.get_mut(id) {
                record.approved = Capability::granted(&declared, &record.approved);
            }
        });
        if let Err(error) = pruned {
            tracing::warn!(code = %error.code, "approvals for removed capabilities stayed");
        }
        self.manifests
            .insert(id.to_owned(), candidate.manifest().clone());
        self.problems.remove(id);
        if self.enabled().iter().any(|enabled| enabled == id) {
            self.start(id);
        }
        self.changed();
        Ok(())
    }

    /// Watch the folder the plugin `id` came from for new builds.
    fn watch(&mut self, id: &str, folder: &str, installed: String) {
        let Some(host) = self.host.clone() else {
            return;
        };
        if let Some(watcher) = reload::watch(
            id,
            folder,
            installed,
            host,
            self.reload_sender.clone(),
            self.wake.clone(),
        ) {
            self.watchers.insert(id.to_owned(), watcher);
        }
    }

    /// The running plugins that draw, in order.
    fn drawing(&self) -> impl Iterator<Item = (&String, &Running)> {
        self.running.iter().filter(|(_, running)| !running.failed)
    }

    /// Every page the running plugins add to the rail, as (rail page ID,
    /// title), by plugin.
    pub(crate) fn pages(&self) -> Vec<(String, String)> {
        self.drawing()
            .flat_map(|(id, running)| {
                running
                    .registrations
                    .pages
                    .iter()
                    .map(move |page| (page_id(id, &page.id), page.title.clone()))
            })
            .collect()
    }

    /// The view of the rail page `page_id`, when a running plugin adds it.
    pub(crate) fn view(&self, page_id: &str) -> Option<PluginView> {
        let (plugin, page) = split_page_id(page_id)?;
        self.view_at(plugin, &Surface::Page(page.to_owned()))
    }

    fn view_at(&self, plugin: &str, surface: &Surface) -> Option<PluginView> {
        self.running
            .get(plugin)
            .filter(|running| !running.failed)?
            .views
            .get(surface)
            .cloned()
    }

    /// Tell the plugin `plugin` that the button `node` was pressed in the
    /// view at `surface`, and draw its views again once it has heard.
    fn press_at(&mut self, plugin: &str, surface: Surface, node: &str) {
        let shown = self
            .running
            .get(plugin)
            .is_some_and(|running| running.views.contains_key(&surface));
        if shown {
            self.send(plugin, Job::Event(surface, Event::Pressed(node.to_owned())));
        }
    }

    /// Press the button `node` in the rail page `page_id`.
    pub(crate) fn press(&mut self, page_id: &str, node: &str) {
        if let Some((plugin, page)) = split_page_id(page_id) {
            let (plugin, page) = (plugin.to_owned(), page.to_owned());
            self.press_at(&plugin, Surface::Page(page), node);
        }
    }

    /// The commands the running plugins add to the command palette, as
    /// (command ID, title).
    pub(crate) fn commands(&self) -> Vec<(String, String)> {
        self.drawing()
            .flat_map(|(id, running)| {
                running.registrations.commands.iter().map(move |command| {
                    (
                        format!("{COMMAND_PREFIX}{id}:{}", command.id),
                        command.title.clone(),
                    )
                })
            })
            .collect()
    }

    /// Run the palette command `command_id`. Returns whether a running
    /// plugin offers it.
    pub(crate) fn run_command(&mut self, command_id: &str) -> bool {
        let Some((plugin, command)) = command_id
            .strip_prefix(COMMAND_PREFIX)
            .and_then(|rest| rest.split_once(':'))
        else {
            return false;
        };
        let offered = self.drawing().any(|(id, running)| {
            id == plugin
                && running
                    .registrations
                    .commands
                    .iter()
                    .any(|known| known.id == command)
        });
        if offered {
            let (plugin, command) = (plugin.to_owned(), command.to_owned());
            self.send(&plugin, Job::Command(command));
        }
        offered
    }

    /// The running plugins' Inbox sections, by plugin.
    pub(crate) fn inbox_sections(&self) -> Vec<PluginSection> {
        self.drawing()
            .flat_map(|(id, running)| {
                let page_id = running
                    .registrations
                    .pages
                    .first()
                    .map(|page| page_id(id, &page.id));
                running
                    .registrations
                    .sections
                    .iter()
                    .filter_map(move |section| {
                        let content = running
                            .views
                            .get(&Surface::InboxSection(section.id.clone()))?
                            .clone();
                        Some(PluginSection {
                            plugin_id: id.clone(),
                            section_id: section.id.clone(),
                            title: section.title.clone(),
                            page_id: page_id.clone(),
                            content,
                        })
                    })
            })
            .collect()
    }

    /// Press the button `node` in the Inbox section `section` of the
    /// plugin `plugin`.
    pub(crate) fn press_inbox(&mut self, plugin: &str, section: &str, node: &str) {
        self.press_at(plugin, Surface::InboxSection(section.to_owned()), node);
    }

    /// The files changed, as the listing `listing` shows: each running
    /// plugin draws its views again, once per listing.
    pub(crate) fn files_changed(&mut self, listing: u64) {
        if listing <= self.listing_drawn {
            return;
        }
        self.listing_drawn = listing;
        let ids: Vec<String> = self.running.keys().cloned().collect();
        for id in ids {
            self.send(&id, Job::Redraw);
        }
    }

    /// The running plugins' pages in Settings, as (section ID, title).
    pub(crate) fn settings_pages(&self) -> Vec<(String, String)> {
        self.drawing()
            .filter_map(|(id, running)| {
                let title = running.registrations.settings.clone()?;
                running
                    .views
                    .contains_key(&Surface::Settings)
                    .then(|| (format!("{SETTINGS_PREFIX}{id}"), title))
            })
            .collect()
    }

    /// The view of a plugin's page in Settings.
    pub(crate) fn settings_view(&self, section_id: &str) -> Option<PluginView> {
        let plugin = section_id.strip_prefix(SETTINGS_PREFIX)?;
        self.view_at(plugin, &Surface::Settings)
    }

    /// Press the button `node` on a plugin's page in Settings.
    pub(crate) fn press_settings(&mut self, section_id: &str, node: &str) {
        if let Some(plugin) = section_id.strip_prefix(SETTINGS_PREFIX) {
            let plugin = plugin.to_owned();
            self.press_at(&plugin, Surface::Settings, node);
        }
    }

    /// The tools the running plugins offer.
    pub(crate) fn tools(&self) -> Vec<PluginTool> {
        self.drawing()
            .flat_map(|(id, running)| {
                running
                    .registrations
                    .tools
                    .iter()
                    .map(move |tool| PluginTool {
                        plugin_id: id.clone(),
                        name: tool.name.clone(),
                        description: tool.description.clone(),
                        input_schema: tool.input_schema.clone(),
                    })
            })
            .collect()
    }

    /// A call to the tool `name` of the plugin `plugin`, once `arguments`
    /// match its schema. A call that doesn't match never reaches the
    /// plugin.
    pub(crate) fn tool_call(
        &self,
        plugin: &str,
        name: &str,
        arguments: &str,
    ) -> Result<ToolCall, AppError> {
        let found = self.drawing().find_map(|(id, running)| {
            (id == plugin).then_some(running).and_then(|running| {
                let tool: &ToolEntry = running
                    .registrations
                    .tools
                    .iter()
                    .find(|tool| tool.name == name)?;
                Some((running.plugin.clone(), tool.validator.clone()))
            })
        });
        let Some((running, validator)) = found else {
            return Err(AppError::new(
                "plugin_tool_unknown",
                format!("No running plugin offers the tool {plugin}/{name}."),
            ));
        };
        let value: serde_json::Value = serde_json::from_str(arguments).map_err(|_| {
            AppError::new(
                "plugin_tool_arguments_invalid",
                "The tool's arguments aren't JSON.",
            )
        })?;
        let problems: Vec<String> = validator
            .iter_errors(&value)
            .map(|error| {
                let at = error.instance_path().to_string();
                let at = if at.is_empty() { "/".to_owned() } else { at };
                format!("{at}: {}", error.masked())
            })
            .collect();
        if !problems.is_empty() {
            return Err(AppError::new(
                "plugin_tool_arguments_invalid",
                format!(
                    "The tool's arguments don't match its schema: {}.",
                    problems.join("; ")
                ),
            ));
        }
        Ok(ToolCall {
            plugin: running,
            name: name.to_owned(),
            arguments: arguments.to_owned(),
        })
    }

    /// Draw the plugin `id`'s views again, as after another caller ran it.
    pub(crate) fn redraw(&mut self, id: &str) {
        self.send(id, Job::Redraw);
    }

    /// Use the plugins in `folder` as the ones that ship with Noura: each
    /// subfolder that holds a plugin.
    pub(crate) fn set_shipped(&mut self, folder: PathBuf) {
        self.shipped = Some(folder);
        self.changed();
    }

    /// The plugins shipping with Noura that this device hasn't shown you
    /// yet, each read from its manifest. None of their code has run.
    pub(crate) fn shipped_review(&self) -> Vec<ShippedPlugin> {
        self.shipped_folders()
            .into_iter()
            .map(|(manifest, _)| ShippedPlugin {
                id: manifest.id.clone(),
                name: manifest.name.clone(),
                version: manifest.version.to_string(),
                capabilities: lines(&manifest.capabilities),
            })
            .collect()
    }

    /// The manifest and folder of each shipping plugin this device hasn't
    /// reviewed, by ID.
    fn shipped_folders(&self) -> Vec<(PluginManifest, PathBuf)> {
        let Some(folder) = &self.shipped else {
            return Vec::new();
        };
        let Ok(entries) = std::fs::read_dir(folder) else {
            return Vec::new();
        };
        let mut found: Vec<(PluginManifest, PathBuf)> = entries
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.join(plugin_host::MANIFEST_FILE).is_file())
            .filter_map(|path| match plugin_host::read_manifest(&path) {
                Ok(manifest) => Some((manifest, path)),
                Err(error) => {
                    tracing::warn!(
                        code = error.code.as_str(),
                        "a plugin shipping with Noura can't be read"
                    );
                    None
                }
            })
            .filter(|(manifest, _)| !self.records.shipped_reviewed.contains(&manifest.id))
            .collect();
        found.sort_by(|(a, _), (b, _)| a.id.cmp(&b.id));
        found
    }

    /// Approve every plugin the first-run screen lists, for this device,
    /// and install each one. One that can't install is noted on the
    /// Plugins page and not offered again.
    pub(crate) fn approve_shipped(&mut self) -> Result<(), AppError> {
        let host = self.host()?;
        for (manifest, folder) in self.shipped_folders() {
            let id = manifest.id.clone();
            let installed = folder
                .to_str()
                .ok_or_else(AppError::unsupported_path)
                .and_then(|path| {
                    let candidate = host
                        .inspect(&folder)
                        .map_err(|error| from_install(&error))?;
                    self.install_candidate(&candidate, path, true)
                });
            if let Err(error) = installed {
                tracing::warn!(code = %error.code, "a plugin shipping with Noura didn't install");
                self.problems.insert(id.clone(), error.message);
                self.save_records(|records| {
                    records.shipped_reviewed.insert(id);
                })?;
            }
        }
        self.changed();
        Ok(())
    }
}

fn no_update() -> AppError {
    AppError::new(
        "no_plugin_update",
        "No new build of this plugin is waiting.",
    )
}

/// The rail page ID of a plugin's page.
pub(crate) fn page_id(plugin: &str, page: &str) -> String {
    format!("{PAGE_PREFIX}{plugin}:{page}")
}

/// Whether a rail page belongs to a plugin.
pub(crate) fn is_plugin_page(page_id: &str) -> bool {
    page_id.starts_with(PAGE_PREFIX)
}

fn split_page_id(page_id: &str) -> Option<(&str, &str)> {
    page_id.strip_prefix(PAGE_PREFIX)?.split_once(':')
}

fn from_install(error: &InstallError) -> AppError {
    tracing::warn!(
        code = error.code.as_str(),
        "a plugin couldn't be installed or loaded"
    );
    AppError::new(error.code.as_str(), error.message.clone())
}
