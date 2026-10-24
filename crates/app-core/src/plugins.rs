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
//! haven't installed and approved it.

use std::{
    collections::{BTreeMap, HashMap},
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
    time::Duration,
};

use local_core::{CoreError, WatchCoordinator, WorkspaceEntryKind};
use plugin_host::{
    Candidate, Capability, Entry, EntryKind, Event, FileRead, InstallError, Listed, ObjectRead,
    ObjectTypes, Page, PluginHost, PluginInstance, PluginManifest, ServiceError, Services, Value,
    View, ViewError, ViewKind, ViewNode, VirtualRow, WorkspaceObjects, WorkspacePath,
    WorkspaceReads, check_rows,
};
use serde::{Deserialize, Serialize};

use crate::{AppError, Workspace, plugin_objects::ObjectStore, store::JsonStore};

/// Where the plugin host keeps installed plugins and their compiled code,
/// under the app data folder.
const LIBRARY: &str = "plugins/library";

/// This device's install records and approvals, under the app data folder.
const RECORDS: &str = "plugins/installed.json";

/// Rail pages that plugins add have IDs that start with this, then the
/// plugin's ID and the page's own ID.
const PAGE_PREFIX: &str = "plugin:";

/// How long a plugin folder must stay quiet after a change before a reload
/// reads it, so a build tool can finish writing.
const QUIET: Duration = Duration::from_millis(200);

/// How long a reload watcher waits for changes before it checks whether it
/// should stop.
const WATCH_WAIT: Duration = Duration::from_millis(500);

/// The largest file a plugin may read.
const MAX_READ_BYTES: u64 = 16 << 20;

/// How many rows of a virtual list Noura asks a plugin for at once. Each
/// batch starts at a multiple of this.
const ROW_BATCH: u32 = 64;

/// A capability as Settings and the install dialog show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityLine {
    /// The name the manifest uses, such as `workspace-read`.
    pub name: String,
    /// What approving it lets the plugin do.
    pub description: String,
}

impl CapabilityLine {
    fn of(capability: &Capability) -> Self {
        Self {
            name: capability.name().to_owned(),
            description: capability.description(),
        }
    }
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
    /// The capabilities you approved on this device.
    pub approved: Vec<CapabilityLine>,
    /// Whether it is installed on this device.
    pub installed: bool,
    /// Whether the open workspace's manifest lists it as enabled.
    pub enabled: bool,
    pub running: bool,
    /// Why it isn't running or its last reload failed.
    pub problem: Option<String>,
    /// The capabilities a new build in its folder adds, which wait for
    /// your approval before the build runs. Empty when no build waits.
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
    /// Every capability the manifest declares, in its order.
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

/// What a plugin page shows: its view, or why it can't be drawn.
pub type PluginView = Result<View, String>;

/// This device's record of an installed plugin.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    folder: String,
    approved: Vec<Capability>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Records {
    #[serde(default)]
    plugins: BTreeMap<String, Record>,
}

/// A running plugin, its pages and the views it last drew.
struct Running {
    instance: PluginInstance,
    pages: Vec<Page>,
    views: BTreeMap<String, PluginView>,
    /// The properties panels it last drew, by object ID, each with the
    /// revision of the file it drew from.
    panels: BTreeMap<String, (String, PluginView)>,
    /// The rows of each virtual list fetched since the views were drawn,
    /// by page and node ID.
    rows: HashMap<(String, String), Rows>,
}

/// The rows of one virtual list fetched so far.
#[derive(Default)]
struct Rows {
    /// The start of each batch asked for, whatever came back.
    asked: std::collections::HashSet<u32>,
    rows: HashMap<u32, VirtualRow>,
    /// The ID of every row fetched, which no other row may repeat.
    ids: std::collections::HashSet<String>,
    /// Why the plugin's rows can't be shown.
    problem: Option<String>,
}

/// A new build a watcher found in a plugin's folder, or why it can't load.
struct Reload {
    id: String,
    result: Result<Candidate, InstallError>,
}

/// Stops its reload watcher when dropped.
struct Watcher {
    stop: Arc<AtomicBool>,
    /// The digest of the build the watcher offered last, shared with it.
    offered: Offered,
}

/// The digest of the build a reload watcher offered last. The watcher
/// skips a build with this digest; clearing it makes the next change in
/// the folder offer the build again.
type Offered = Arc<Mutex<String>>;

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

type Wake = Arc<dyn Fn() + Send + Sync>;

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
    /// The object types the running plugins registered.
    types: ObjectTypes,
    running: BTreeMap<String, Running>,
    problems: BTreeMap<String, String>,
    /// New builds whose capabilities wait for your approval.
    updates: BTreeMap<String, Candidate>,
    watchers: BTreeMap<String, Watcher>,
    reload_sender: Sender<Reload>,
    reloads: Receiver<Reload>,
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
        Self {
            app_data: app_data.to_owned(),
            store,
            records,
            host: None,
            manifests: BTreeMap::new(),
            workspace: None,
            types: ObjectTypes::new(),
            running: BTreeMap::new(),
            problems: BTreeMap::new(),
            updates: BTreeMap::new(),
            watchers: BTreeMap::new(),
            reload_sender,
            reloads,
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

    /// Change the records and save them. Returns the error to show when
    /// they couldn't be saved; the records then stay as they were.
    fn save_records(&mut self, change: impl FnOnce(&mut Records)) -> Result<(), AppError> {
        let mut records = Records {
            plugins: self.records.plugins.clone(),
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
                let update = match (self.updates.get(id), record) {
                    (Some(candidate), Some(record)) => candidate
                        .manifest()
                        .capabilities
                        .iter()
                        .filter(|capability| !record.approved.contains(capability))
                        .map(CapabilityLine::of)
                        .collect(),
                    _ => Vec::new(),
                };
                PluginRow {
                    id: id.clone(),
                    name: manifest.map_or_else(|| id.clone(), |manifest| manifest.name.clone()),
                    version: manifest.map(|manifest| manifest.version.to_string()),
                    folder: record.map(|record| record.folder.clone()),
                    approved: record
                        .map(|record| record.approved.iter().map(CapabilityLine::of).collect())
                        .unwrap_or_default(),
                    installed: record.is_some(),
                    enabled: enabled.contains(id),
                    running: self.running.contains_key(id),
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
        self.running.clear();
        self.workspace = workspace;
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

    /// Start the installed plugin `id` when every capability its manifest
    /// declares is approved, and draw its pages.
    fn start(&mut self, id: &str) {
        self.running.remove(id);
        self.problems.remove(id);
        let Some(record) = self.records.plugins.get(id).cloned() else {
            return;
        };
        let Some(workspace) = self.workspace.clone() else {
            return;
        };
        let started = self.host().and_then(|host| {
            let loaded = host.load(id).map_err(|error| from_install(&error))?;
            let unapproved: Vec<&str> = loaded
                .manifest()
                .capabilities
                .iter()
                .filter(|capability| !record.approved.contains(capability))
                .map(Capability::name)
                .collect();
            if !unapproved.is_empty() {
                return Err(AppError::new(
                    "plugin_not_approved",
                    format!(
                        "It asks for capabilities you haven't approved: {}.",
                        unapproved.join(", ")
                    ),
                ));
            }
            let reader: Arc<dyn WorkspaceReads> = Arc::new(WorkspaceReader(workspace.clone()));
            let objects: Arc<dyn WorkspaceObjects> =
                Arc::new(ObjectStore::new(workspace, self.types.clone()));
            let services = Services {
                workspace: Some(reader),
                objects: Some(objects),
                types: self.types.clone(),
            };
            let mut instance = host
                .start_with(&loaded, &record.approved, services)
                .map_err(|error| {
                    tracing::warn!(%error, "a plugin didn't start");
                    AppError::new("plugin_start_failed", "It failed to start.")
                })?;
            let refused = if loaded
                .manifest()
                .capabilities
                .contains(&Capability::ObjectTypes)
            {
                let registered = instance.register_object_types().map_err(|error| {
                    tracing::warn!(%error, "a plugin didn't list its object types");
                    AppError::new(
                        "plugin_types_failed",
                        "It failed while listing its object types.",
                    )
                })?;
                registered
                    .into_iter()
                    .filter_map(|(object_type, result)| {
                        result.err().map(|error| {
                            tracing::warn!(code = error.code(), "a plugin's type wasn't added");
                            format!("Noura didn't add the type {object_type}: {error}")
                        })
                    })
                    .collect::<Vec<_>>()
            } else {
                Vec::new()
            };
            let declares_views = loaded.manifest().capabilities.contains(&Capability::Views);
            let pages = if declares_views {
                instance.pages().map_err(|error| {
                    tracing::warn!(%error, "a plugin didn't list its pages");
                    AppError::new("plugin_pages_failed", "It failed while listing its pages.")
                })?
            } else {
                Vec::new()
            };
            Ok((instance, pages, refused))
        });
        match started {
            Ok((instance, pages, refused)) => {
                let pages = pages.into_iter().filter(valid_page).collect();
                let mut running = Running {
                    instance,
                    pages,
                    views: BTreeMap::new(),
                    panels: BTreeMap::new(),
                    rows: HashMap::new(),
                };
                running.render_all();
                self.running.insert(id.to_owned(), running);
                if !refused.is_empty() {
                    self.problems.insert(id.to_owned(), refused.join(" "));
                }
            }
            Err(error) => {
                self.problems.insert(id.to_owned(), error.message);
            }
        }
        self.changed();
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
                    capabilities: manifest
                        .capabilities
                        .iter()
                        .map(CapabilityLine::of)
                        .collect(),
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
        let host = self.host()?;
        host.install(candidate)
            .map_err(|error| from_install(&error))?;
        let id = candidate.manifest().id.clone();
        let record = Record {
            folder: install.folder.clone(),
            approved: candidate.manifest().capabilities.clone(),
        };
        if let Err(error) = self.save_records(|records| {
            records.plugins.insert(id.clone(), record);
        }) {
            let _ = host.uninstall(&id);
            return Err(error);
        }
        self.manifests
            .insert(id.clone(), candidate.manifest().clone());
        self.watch(&id, &install.folder, candidate.digest());
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
            self.running.remove(id);
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
        self.running.remove(id);
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

    /// Approve the capabilities a waiting build adds, and run it.
    pub(crate) fn approve_update(&mut self, id: &str) -> Result<(), AppError> {
        let Some(candidate) = self.updates.remove(id) else {
            return Err(AppError::new(
                "no_plugin_update",
                "No new build of this plugin is waiting.",
            ));
        };
        if !self.records.plugins.contains_key(id) {
            return Err(AppError::new(
                "plugin_not_installed",
                "This plugin isn't installed on this device.",
            ));
        }
        let approved = candidate.manifest().capabilities.clone();
        self.save_records(|records| {
            if let Some(record) = records.plugins.get_mut(id) {
                for capability in approved {
                    if !record.approved.contains(&capability) {
                        record.approved.push(capability);
                    }
                }
            }
        })?;
        self.apply(id, &candidate).inspect_err(|error| {
            self.failed_update(id, error);
        })
    }

    /// Take the builds the reload watchers found. A build that asks for no
    /// capability beyond your approvals replaces the running one; any
    /// other waits for approval while the old build keeps running.
    pub(crate) fn poll(&mut self) {
        while let Ok(reload) = self.reloads.try_recv() {
            let Some(record) = self.records.plugins.get(&reload.id).cloned() else {
                continue;
            };
            match reload.result {
                Ok(candidate) => {
                    let approved = candidate
                        .manifest()
                        .capabilities
                        .iter()
                        .all(|capability| record.approved.contains(capability));
                    if approved {
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
    }

    /// Show why a new build of the plugin `id` didn't install, and let its
    /// watcher offer that build again when the folder next changes.
    fn failed_update(&mut self, id: &str, error: &AppError) {
        if let Some(watcher) = self.watchers.get(id) {
            watcher
                .offered
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clear();
        }
        self.problems.insert(
            id.to_owned(),
            format!("{} Save the build again to retry.", error.message),
        );
        self.changed();
    }

    /// Install `candidate` over the plugin `id`, and run it in place of the
    /// old build when the plugin is running.
    fn apply(&mut self, id: &str, candidate: &Candidate) -> Result<(), AppError> {
        let host = self.host()?;
        host.update(candidate)
            .map_err(|error| from_install(&error))?;
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
        let stop = Arc::new(AtomicBool::new(false));
        let offered = Arc::new(Mutex::new(installed));
        let watcher = ReloadWatcher {
            id: id.to_owned(),
            folder: PathBuf::from(folder),
            host,
            offered: offered.clone(),
            sender: self.reload_sender.clone(),
            wake: self.wake.clone(),
            stop: stop.clone(),
        };
        let spawned = std::thread::Builder::new()
            .name("plugin-reload".into())
            .spawn(move || watcher.run());
        match spawned {
            Ok(_) => {
                self.watchers
                    .insert(id.to_owned(), Watcher { stop, offered });
            }
            Err(error) => tracing::warn!(%error, "a plugin's folder isn't watched for new builds"),
        }
    }

    /// Every page the running plugins add to the rail, as (rail page ID,
    /// title), by plugin.
    pub(crate) fn pages(&self) -> Vec<(String, String)> {
        self.running
            .iter()
            .flat_map(|(id, running)| {
                running
                    .pages
                    .iter()
                    .map(move |page| (page_id(id, &page.id), page.title.clone()))
            })
            .collect()
    }

    /// The view of the rail page `page_id`, when a running plugin adds it.
    pub(crate) fn view(&self, page_id: &str) -> Option<PluginView> {
        let (plugin, page) = split_page_id(page_id)?;
        self.running.get(plugin)?.views.get(page).cloned()
    }

    /// Tell the plugin behind `page_id` about `event` in the page's view,
    /// and draw its pages again. An input value of the wrong form, such as
    /// a date that isn't `YYYY-MM-DD`, never reaches the plugin.
    pub(crate) fn send(&mut self, page_id: &str, event: &Event) {
        let Some((plugin, page)) = split_page_id(page_id) else {
            return;
        };
        let Some(running) = self.running.get_mut(plugin) else {
            return;
        };
        if !running.pages.iter().any(|known| known.id == page) {
            return;
        }
        if !well_formed_event(event) {
            return;
        }
        if let Err(error) = running.instance.handle_event(page, event) {
            tracing::warn!(%error, "a plugin failed to handle an event");
        }
        // The event may have written objects whose panels show.
        running.panels.clear();
        running.render_all();
        self.changed();
    }

    /// The properties panel of the object `object_id` of type
    /// `object_type`, whose file has the revision `revision`, as the
    /// running plugin that holds the type draws it. `None` when no running
    /// plugin holds the type, so the kernel's panel shows.
    pub(crate) fn panel(
        &mut self,
        object_type: &str,
        object_id: &str,
        revision: &str,
    ) -> Option<PluginView> {
        let owner = self.types.owner(object_type)?;
        let running = self.running.get_mut(&owner)?;
        if let Some((drawn, view)) = running.panels.get(object_id)
            && drawn == revision
        {
            return Some(view.clone());
        }
        let view = match running.instance.render_panel(object_id) {
            Ok(nodes) => View::from_nodes(nodes)
                .map_err(|error| error.to_string())
                .and_then(|view| match virtual_list_in(view.root()) {
                    // A panel has no `rows` to fetch a virtual list's rows.
                    Some(list) => Err(format!(
                        "The virtual list {list} can't show in a properties panel."
                    )),
                    None => Ok(view),
                }),
            Err(error) => {
                tracing::warn!(%error, "a plugin failed to draw a properties panel");
                Err("The plugin failed while drawing this panel.".to_owned())
            }
        };
        running
            .panels
            .insert(object_id.to_owned(), (revision.to_owned(), view.clone()));
        Some(view)
    }

    /// Tell the plugin that holds `object_type` about `event` in the
    /// properties panel of the object `object_id`, and draw its panels and
    /// pages again. An input value of the wrong form never reaches the
    /// plugin.
    pub(crate) fn send_panel(&mut self, object_type: &str, object_id: &str, event: &Event) {
        let Some(owner) = self.types.owner(object_type) else {
            return;
        };
        let Some(running) = self.running.get_mut(&owner) else {
            return;
        };
        if !well_formed_event(event) {
            return;
        }
        if let Err(error) = running.instance.handle_panel_event(object_id, event) {
            tracing::warn!(%error, "a plugin failed to handle an event in a panel");
        }
        running.panels.clear();
        running.render_all();
        self.changed();
    }

    /// The rows in `range` of the virtual list `node` in the plugin page
    /// `page_id`, each `None` until the plugin returns it. Rows the
    /// plugin hasn't returned yet are asked for in batches of
    /// [`ROW_BATCH`], once each until the page draws again. A row whose ID
    /// repeats one fetched before breaks the list's rules.
    pub(crate) fn virtual_rows(
        &mut self,
        page_id: &str,
        node: &str,
        range: Range<u32>,
    ) -> Result<Vec<Option<VirtualRow>>, String> {
        let gone = || "The plugin that adds this page isn't running.".to_owned();
        let (plugin, page) = split_page_id(page_id).ok_or_else(gone)?;
        let running = self.running.get_mut(plugin).ok_or_else(gone)?;
        let total = match running.views.get(page) {
            Some(Ok(view)) => virtual_list_rows(view.root(), node),
            _ => None,
        }
        .ok_or_else(|| format!("The page has no virtual list {node}."))?;
        let range = range.start.min(total)..range.end.min(total);
        let rows = running
            .rows
            .entry((page.to_owned(), node.to_owned()))
            .or_default();
        if let Some(problem) = &rows.problem {
            return Err(problem.clone());
        }
        let first = range.start / ROW_BATCH * ROW_BATCH;
        for start in (first..range.end).step_by(ROW_BATCH as usize) {
            if !rows.asked.insert(start) {
                continue;
            }
            let count = ROW_BATCH.min(total - start);
            let fetched = running
                .instance
                .rows(page, node, start, count)
                .map_err(|error| {
                    tracing::warn!(%error, "a plugin failed to list rows");
                    "The plugin failed while listing these rows.".to_owned()
                })
                .and_then(|fetched| check_rows(fetched, count).map_err(|error| error.to_string()))
                .and_then(|fetched| {
                    // `check_rows` checks one batch; a row may not repeat
                    // the ID of a row in another.
                    match fetched.iter().find(|row| rows.ids.contains(&row.id)) {
                        Some(row) => {
                            Err(ViewError::DuplicateRowId { id: row.id.clone() }.to_string())
                        }
                        None => Ok(fetched),
                    }
                });
            match fetched {
                Ok(fetched) => {
                    rows.ids.extend(fetched.iter().map(|row| row.id.clone()));
                    rows.rows.extend((start..).zip(fetched));
                }
                Err(problem) => {
                    rows.problem = Some(problem.clone());
                    return Err(problem);
                }
            }
        }
        Ok(range.map(|index| rows.rows.get(&index).cloned()).collect())
    }
}

/// How many rows the virtual list `id` under `node` holds.
fn virtual_list_rows(node: &ViewNode, id: &str) -> Option<u32> {
    match &node.kind {
        ViewKind::VirtualList { rows, .. } if node.id == id => Some(*rows),
        _ => node
            .children
            .iter()
            .find_map(|child| virtual_list_rows(child, id)),
    }
}

/// The ID of the first virtual list under `node`, if any.
fn virtual_list_in(node: &ViewNode) -> Option<&str> {
    match &node.kind {
        ViewKind::VirtualList { .. } => Some(&node.id),
        _ => node.children.iter().find_map(virtual_list_in),
    }
}

/// Whether the input values `event` carries have the form the plugin
/// interface promises. Logs the ones that don't.
fn well_formed_event(event: &Event) -> bool {
    let well_formed = match event {
        Event::Changed(field) => well_formed(&field.value),
        Event::Submitted(submission) => submission
            .fields
            .iter()
            .all(|field| well_formed(&field.value)),
        _ => true,
    };
    if !well_formed {
        tracing::warn!("an input value of the wrong form didn't reach a plugin");
    }
    well_formed
}

/// Whether an input value has the form the plugin interface promises:
/// dates as `YYYY-MM-DD` and dates and times in RFC 3339 with an offset.
fn well_formed(value: &Value) -> bool {
    match value {
        Value::Date(Some(date)) => workspace_format::is_civil_date(date),
        Value::DateTime(Some(at)) => workspace_format::is_instant(at),
        _ => true,
    }
}

impl Running {
    /// Ask the plugin for each page's view and check it. Virtual lists ask
    /// for their rows again.
    fn render_all(&mut self) {
        self.rows.clear();
        for page in &self.pages {
            let view = match self.instance.render(&page.id) {
                Ok(nodes) => View::from_nodes(nodes).map_err(|error| error.to_string()),
                Err(error) => {
                    tracing::warn!(%error, "a plugin failed to draw a page");
                    Err("The plugin failed while drawing this page.".to_owned())
                }
            };
            self.views.insert(page.id.clone(), view);
        }
    }
}

/// Whether a plugin's page has an ID and a title the rail can show.
fn valid_page(page: &Page) -> bool {
    let id_ok = !page.id.is_empty()
        && page.id.len() <= 64
        && !page.id.chars().any(|c| c.is_control() || c.is_whitespace());
    let title = page.title.trim();
    let title_ok =
        !title.is_empty() && title.chars().count() <= 80 && !title.chars().any(char::is_control);
    if !(id_ok && title_ok) {
        tracing::warn!("a plugin page without a usable ID or title was left out");
    }
    id_ok && title_ok
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

/// Watches a plugin's folder and reads each new build there.
struct ReloadWatcher {
    id: String,
    folder: PathBuf,
    host: Arc<PluginHost>,
    /// The digest of the build offered last, which a change must differ
    /// from. It starts as the installed build's.
    offered: Offered,
    sender: Sender<Reload>,
    wake: Option<Wake>,
    stop: Arc<AtomicBool>,
}

impl ReloadWatcher {
    fn offered(&self) -> std::sync::MutexGuard<'_, String> {
        self.offered.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(self) {
        let watcher = WatchCoordinator::new(&self.folder, |_| {});
        let mut last_problem: Option<String> = None;
        // The folder may have changed while Noura wasn't running.
        let mut look = true;
        while !self.stop.load(Ordering::Relaxed) {
            if !look {
                let changed = !watcher
                    .drain_coalesced(WATCH_WAIT)
                    .unwrap_or_default()
                    .is_empty()
                    || watcher.take_overflow()
                    || !watcher.is_available();
                if !changed {
                    continue;
                }
                // Let the build tool finish writing.
                while !self.stop.load(Ordering::Relaxed)
                    && !watcher
                        .drain_coalesced(QUIET)
                        .unwrap_or_default()
                        .is_empty()
                {}
            }
            look = false;
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            let result = match self.host.folder_digest(&self.folder) {
                Ok(digest) if digest == *self.offered() => continue,
                Ok(_) => self.host.inspect_update(&self.id, &self.folder),
                Err(error) => Err(error),
            };
            match &result {
                Ok(candidate) => {
                    *self.offered() = candidate.digest();
                    last_problem = None;
                }
                Err(error) if last_problem.as_ref() == Some(&error.message) => continue,
                Err(error) => last_problem = Some(error.message.clone()),
            }
            let reload = Reload {
                id: self.id.clone(),
                result,
            };
            if self.sender.send(reload).is_err() {
                return;
            }
            if let Some(wake) = &self.wake {
                wake();
            }
        }
    }
}

/// Reads the open workspace for plugins through `local-core`, which keeps
/// every path inside the workspace.
struct WorkspaceReader(Arc<Workspace>);

impl WorkspaceReads for WorkspaceReader {
    fn list_entries(&self) -> Result<Vec<Listed>, ServiceError> {
        let (entries, unsupported) = self
            .0
            .engine()
            .list_workspace_entries_counting_unsupported()
            .map_err(|error| service_error(&error, ""))?;
        let mut listed = Vec::with_capacity(entries.len() + unsupported);
        for entry in entries {
            let Ok(path) = WorkspacePath::new(&entry.relative_path) else {
                listed.push(Listed::Unsupported);
                continue;
            };
            listed.push(Listed::Entry(Entry {
                path,
                kind: match entry.kind {
                    WorkspaceEntryKind::Folder => EntryKind::Folder,
                    _ => EntryKind::File,
                },
                object_id: entry.object_id,
                object_type: entry.object_type,
                revision: entry.revision,
            }));
        }
        listed.extend(std::iter::repeat_n(Listed::Unsupported, unsupported));
        Ok(listed)
    }

    fn read_file(&self, path: &WorkspacePath) -> Result<FileRead, ServiceError> {
        let (bytes, revision) = self
            .0
            .engine()
            .read_file(path.as_str(), MAX_READ_BYTES)
            .map_err(|error| service_error(&error, path.as_str()))?;
        Ok(FileRead {
            path: path.clone(),
            bytes,
            revision,
        })
    }

    fn read_object(&self, id: &str) -> Result<ObjectRead, ServiceError> {
        let object = self
            .0
            .engine()
            .get_object(id)
            .map_err(|error| service_error(&error, id))?
            .ok_or_else(|| ServiceError::NotFound(id.to_owned()))?;
        let path =
            WorkspacePath::new(&object.relative_path).map_err(|_| ServiceError::UnsupportedPath)?;
        // An object's title is its file name without the extension.
        let title = Path::new(&object.relative_path)
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default()
            .to_owned();
        Ok(ObjectRead {
            id: object.id,
            object_type: object.object_type,
            title,
            path,
            body: object.body,
            properties: serde_json::to_string(&object.properties)
                .unwrap_or_else(|_| "{}".to_owned()),
            revision: object.revision,
        })
    }
}

/// A `local-core` error as a plugin sees it, with the details logged.
fn service_error(error: &CoreError, what: &str) -> ServiceError {
    match error.code.as_str() {
        "empty_path" | "absolute_path" | "unsafe_path" | "reserved_path" | "symlink_escape" => {
            ServiceError::InvalidPath(what.to_owned())
        }
        "unsupported_path" | "non_utf8_path" => ServiceError::UnsupportedPath,
        "file_not_found" | "object_not_found" => ServiceError::NotFound(what.to_owned()),
        "file_too_large" => ServiceError::Failed("The file is larger than 16 MB.".to_owned()),
        "file_not_downloaded" => ServiceError::Failed(error.message.clone()),
        code => {
            tracing::warn!(code, operation = %error.operation, "a plugin's workspace read failed");
            ServiceError::Failed("Noura couldn't read the workspace.".to_owned())
        }
    }
}

#[cfg(test)]
mod tests {
    use plugin_host::{Field, Node, NodeKind, VirtualList};

    use super::*;

    #[test]
    fn a_virtual_list_inside_a_panel_is_found_at_any_depth() {
        let node = |id: &str, kind, children| Node {
            id: id.to_owned(),
            kind,
            children,
        };
        let seeds = NodeKind::VirtualList(VirtualList {
            label: "Seeds".to_owned(),
            rows: 10,
        });
        let nested = View::from_nodes(vec![
            node("recipe", NodeKind::Panel(None), vec![1]),
            node("more", NodeKind::Panel(None), vec![2]),
            node("seeds", seeds, Vec::new()),
        ])
        .expect("a well-formed view");
        assert_eq!(virtual_list_in(nested.root()), Some("seeds"));

        let plain = View::from_nodes(vec![node("recipe", NodeKind::Panel(None), Vec::new())])
            .expect("a well-formed view");
        assert_eq!(virtual_list_in(plain.root()), None);
    }

    #[test]
    fn an_event_with_a_date_of_the_wrong_form_is_refused() {
        let changed = |date: &str| {
            Event::Changed(Field {
                node: "sown".to_owned(),
                value: Value::Date(Some(date.to_owned())),
            })
        };
        assert!(well_formed_event(&changed("2026-10-21")));
        assert!(!well_formed_event(&changed("21/10/2026")));
        assert!(well_formed_event(&Event::Pressed("vegetarian".to_owned())));
    }
}
