//! The plugins service: installs plugins from folders and from releases
//! on GitHub once you approve their capabilities, keeps those approvals in
//! this device's app data, turns plugins on and off in the open
//! workspace's manifest, runs the ones that are on, reloads a plugin when
//! a new build lands in its folder, and checks GitHub for a newer release
//! when you ask.
//!
//! Every window shares one service. Installing a plugin from a folder is
//! Noura's dev mode: the folder stays where it is, and the service copies
//! each build into the plugin host's library, which compiles it on this
//! device. A release on GitHub downloads on a thread of its own, and its
//! archive goes through the same library. A newer release always waits
//! for you, and one that asks for more lists what it adds. Approvals
//! never come from workspace files, so a workspace that
//! lists a plugin as enabled runs none of its code on a device where you
//! haven't installed and approved it. The plugins that ship with Noura
//! wait for one approval on this device's first-run screen.
//!
//! Each running plugin's calls run on a thread of its own, so a plugin that
//! loops, allocates past its memory cap or crashes never holds up a
//! window: its views, commands, Inbox sections, Settings page and tools
//! update when its answers arrive, through [`Plugins::poll`].

mod navigation;
mod reader;
mod reload;
mod worker;

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    ops::Range,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, channel},
    },
};

use local_core::HostRequests;
use plugin_host::{
    Archive, Candidate, Capability, DeviceClock, Event, Grant, InstallError, Navigation, Node,
    NodeKind, ObjectTypes, PluginHost, PluginManifest, ReleaseUrl, Releases, Router, RunningPlugin,
    Services, Surface, Value, View, ViewError, ViewKind, ViewNode, VirtualRow, WorkspaceObjects,
    WorkspaceReads, check_rows,
};
use serde::{Deserialize, Serialize};

use crate::{
    AppError, Workspace, ai::Ai, plugin_objects::ObjectStore, session::WindowKey, store::JsonStore,
};
use navigation::{Navigator, Openings};
pub(crate) use navigation::{Opening, SystemClock};
use reload::{Reload, Watcher};
use worker::{Doing, Job, Outcome, Registrations, RowsFetched, ToolEntry, Worker};

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

/// How many rows of a virtual list Noura asks a plugin for at once. Each
/// batch starts at a multiple of this.
const ROW_BATCH: u32 = 64;

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

/// The lines of `capabilities` that `known` doesn't grant: whole
/// capabilities, network hosts and command targets.
fn lines_beyond(capabilities: &[Capability], known: &[Capability]) -> Vec<CapabilityLine> {
    let known: Vec<Grant> = lines(known).into_iter().map(|line| line.grant).collect();
    lines(capabilities)
        .into_iter()
        .filter(|line| !known.contains(&line.grant))
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
    /// What a new build adds, which waits for your approval before the
    /// build runs. Empty when no build waits, or when the waiting build
    /// adds nothing.
    pub update: Vec<CapabilityLine>,
    /// The release page on GitHub it was installed from.
    pub release: Option<String>,
    /// The version of a new build that waits for you to install it.
    pub newer: Option<String>,
    /// Where checking GitHub for a newer release stands.
    pub update_check: Option<UpdateCheck>,
}

/// Where checking GitHub for a newer release of a plugin stands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateCheck {
    /// Noura is downloading the newest release.
    Checking,
    /// No release is newer than the installed version, given here.
    Newest(String),
    /// The check failed, for this reason. The installed version stays.
    Failed(String),
}

/// The install dialog: a plugin folder or a release on GitHub, read and
/// compiled but not run, with the capabilities it asks for, or why Noura
/// won't install it.
#[derive(Clone)]
pub struct PluginInstall {
    /// The folder you chose, or the release's page on GitHub.
    pub source: String,
    /// It comes from a release on GitHub, a source nobody reviewed.
    pub from_github: bool,
    /// Noura is downloading the release. Nothing else is known yet.
    pub downloading: bool,
    pub id: String,
    pub name: String,
    pub version: String,
    /// Every capability, network host and command target the manifest
    /// declares, in its order.
    pub capabilities: Vec<CapabilityLine>,
    /// Why the plugin can't be installed. Nothing installs while it is
    /// set.
    pub problem: Option<String>,
    candidate: Option<Candidate>,
}

impl PluginInstall {
    /// A dialog that says why Noura won't install from `source`.
    fn refused(source: String, from_github: bool, problem: String) -> Self {
        Self {
            source,
            from_github,
            downloading: false,
            id: String::new(),
            name: String::new(),
            version: String::new(),
            capabilities: Vec::new(),
            problem: Some(problem),
            candidate: None,
        }
    }

    /// The dialog of a release whose download stopped without an answer.
    pub(crate) fn download_stopped(source: String) -> Self {
        let problem = "The download stopped before it finished. Try again.".to_owned();
        Self::refused(source, true, problem)
    }

    /// A dialog that shows what `candidate`, from `source`, asks for.
    fn ready(source: String, from_github: bool, candidate: Candidate) -> Self {
        let manifest = candidate.manifest();
        Self {
            source,
            from_github,
            downloading: false,
            id: manifest.id.clone(),
            name: manifest.name.clone(),
            version: manifest.version.to_string(),
            capabilities: lines(&manifest.capabilities),
            problem: None,
            candidate: Some(candidate),
        }
    }
}

impl std::fmt::Debug for PluginInstall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PluginInstall")
            .field("source", &self.source)
            .field("downloading", &self.downloading)
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

/// The plugins every package of Noura ships: each one's ID, which names
/// its folder, and its name.
pub const SHIPPED_PLUGINS: [(&str, &str); 4] = [
    ("ai", "AI"),
    ("calendar", "Calendar"),
    ("projects", "Projects"),
    ("tasks", "Tasks"),
];

/// A plugin a package should ship and can't, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShippedProblem {
    pub name: &'static str,
    pub reason: String,
}

/// Whether `folder`, a package's plugins folder if it has one, holds every
/// plugin in [`SHIPPED_PLUGINS`] in a form Noura installs: the right
/// manifest and a component that compiles. Nothing is written and none of
/// their code runs. Returns one problem per plugin that fails.
pub fn check_shipped_plugins(folder: Option<&Path>) -> Vec<ShippedProblem> {
    let problems = |reason: &str| {
        SHIPPED_PLUGINS
            .iter()
            .map(|&(_, name)| ShippedProblem {
                name,
                reason: reason.to_owned(),
            })
            .collect()
    };
    let Some(folder) = folder else {
        return problems("this copy has no plugins folder");
    };
    // No plugin is installed in a library that doesn't exist, so a
    // reviewed plugin still gets compiled here.
    let host = match PluginHost::shared(folder.join(".no-library")) {
        Ok(host) => host,
        Err(error) => return problems(&error.message),
    };
    SHIPPED_PLUGINS
        .iter()
        .filter_map(|&(id, name)| {
            let reason = match host.inspect(&folder.join(id)) {
                Ok(candidate) if candidate.manifest().id == id => return None,
                Ok(candidate) => format!("its folder holds {}", candidate.manifest().id),
                Err(error) => error.message,
            };
            Some(ShippedProblem { name, reason })
        })
        .collect()
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
    /// The folder it was installed from, which Noura watches for new
    /// builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    folder: Option<String>,
    /// The page of the release on GitHub it was installed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    release: Option<String>,
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

/// A properties panel a running plugin drew for one object.
struct Panel {
    view: PluginView,
    drawn: Drawn,
    /// The file revision a draw was asked for that hasn't answered yet.
    requested: Option<String>,
}

/// Which revision of an object's file a panel shows.
enum Drawn {
    /// None yet, or the plugin did something since that may change it.
    Nothing,
    Revision(String),
    /// The plugin drew it right after handling a press in it, so it shows
    /// the file as the plugin left it: the revision the note reads next.
    AfterPress,
}

impl Panel {
    /// A panel the plugin hasn't drawn yet, which shows nothing.
    fn empty() -> Self {
        let root = Node {
            id: "panel".to_owned(),
            kind: NodeKind::Panel(None),
            children: Vec::new(),
        };
        Self {
            view: View::from_nodes(vec![root]).map_err(|error| error.to_string()),
            drawn: Drawn::Nothing,
            requested: None,
        }
    }

    /// Whether the panel shows the object's file at `revision`.
    fn current_for(&mut self, revision: &str) -> bool {
        match &self.drawn {
            Drawn::Nothing => false,
            Drawn::Revision(drawn) => drawn == revision,
            Drawn::AfterPress => {
                self.drawn = Drawn::Revision(revision.to_owned());
                true
            }
        }
    }
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
    /// The properties panels it drew, by object ID.
    panels: BTreeMap<String, Panel>,
    /// The rows of each virtual list fetched since the views were drawn,
    /// by page and node ID.
    rows: HashMap<(String, String), Rows>,
    /// Counts the times its views were drawn, so rows fetched for views
    /// drawn before are dropped.
    drawn: u64,
}

/// The rows of one virtual list fetched so far.
#[derive(Default)]
struct Rows {
    /// The start of each batch asked for, whatever came back.
    asked: HashSet<u32>,
    rows: HashMap<u32, VirtualRow>,
    /// The rows fetched before the views were last drawn, which show until
    /// the plugin returns the rows at their places again, so the list
    /// doesn't empty while it does.
    earlier: HashMap<u32, VirtualRow>,
    /// The ID of every row fetched, which no other row may repeat.
    ids: HashSet<String>,
    /// Why the plugin's rows can't be shown.
    problem: Option<String>,
}

impl Rows {
    /// Ask for every row again, as after the views were drawn again.
    fn again(&mut self) {
        let fetched = std::mem::take(&mut self.rows);
        self.earlier.extend(fetched);
        self.asked.clear();
        self.ids.clear();
        self.problem = None;
    }

    /// The row at `index`, fetched since the views were drawn or before.
    fn get(&self, index: u32) -> Option<VirtualRow> {
        self.rows
            .get(&index)
            .or_else(|| self.earlier.get(&index))
            .cloned()
    }
}

type Wake = Arc<dyn Fn() + Send + Sync>;

/// A new build of an installed plugin that waits for you: from its folder,
/// when it adds capabilities, or from a newer release on GitHub.
#[derive(Clone)]
struct Offer {
    candidate: Candidate,
    /// The page of the release it comes from, for one from GitHub.
    release: Option<String>,
}

/// What checking GitHub for a newer release of a plugin found.
struct Checked {
    id: String,
    found: Result<Found, InstallError>,
}

enum Found {
    /// No release is newer than the installed version.
    Newest,
    /// This newer release, read and compiled.
    Newer(Box<Offer>),
}

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
                    worker::failure_message(&error, Doing::Running),
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
    /// The object types the running plugins registered.
    types: ObjectTypes,
    /// See [`Plugins::acts`].
    acts: u64,
    running: BTreeMap<String, Running>,
    generations: u64,
    outcomes: (Sender<Outcome>, Receiver<Outcome>),
    problems: BTreeMap<String, String>,
    /// New builds that wait for you to approve them.
    updates: BTreeMap<String, Offer>,
    /// Downloads plugin releases from GitHub.
    releases: Arc<Releases>,
    /// What each check for a newer release found, as it finishes.
    checks: (Sender<Checked>, Receiver<Checked>),
    /// Where each plugin's last check for a newer release stands.
    update_checks: BTreeMap<String, UpdateCheck>,
    watchers: BTreeMap<String, Watcher>,
    reload_sender: Sender<Reload>,
    reloads: Receiver<Reload>,
    /// The folder holding the plugins that ship with Noura.
    shipped: Option<PathBuf>,
    /// The manifest and folder of each plugin shipping with Noura that
    /// this device hasn't shown you yet, read once rather than per frame.
    shipped_pending: Vec<(PluginManifest, PathBuf)>,
    /// The listing whose file changes the running plugins drew last.
    listing_drawn: u64,
    /// The clock running plugins read.
    clock: Option<Arc<dyn DeviceClock>>,
    /// What running plugins asked each window to show.
    openings: Openings,
    /// Wakes every window, as a reload watcher does once it read a build.
    wake: Option<Wake>,
    /// Counts the changes the windows show: plugins, pages and views.
    version: u64,
    /// It serves an AI tool outside Noura. The app installs, updates and
    /// removes plugins and watches their folders, so this service leaves
    /// the library and the folders alone.
    agent: bool,
    /// The AI service plugins that hold `ai` chat through.
    ai: Option<Arc<Ai>>,
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
            types: ObjectTypes::new(),
            acts: 0,
            running: BTreeMap::new(),
            generations: 0,
            outcomes: channel(),
            problems: BTreeMap::new(),
            updates: BTreeMap::new(),
            releases: Arc::new(Releases::github()),
            checks: channel(),
            update_checks: BTreeMap::new(),
            watchers: BTreeMap::new(),
            reload_sender,
            reloads,
            shipped: None,
            shipped_pending: Vec::new(),
            listing_drawn: 0,
            clock: None,
            openings: Openings::default(),
            wake: None,
            version: 0,
            agent: false,
            ai: None,
        }
    }

    /// The service for an AI tool outside Noura, such as the MCP server,
    /// which runs beside the app on the same app data. It reads this
    /// device's install records and approvals, and never changes the
    /// library or watches plugin folders.
    pub(crate) fn for_agent(app_data: &Path) -> Self {
        Self {
            agent: true,
            ..Self::new(app_data)
        }
    }

    /// Let plugins that hold `ai` chat through `ai`, from the next start
    /// on.
    pub(crate) fn set_ai(&mut self, ai: Arc<Ai>) {
        self.ai = Some(ai);
    }

    pub(crate) fn set_wake(&mut self, wake: Wake) {
        self.wake = Some(wake);
    }

    /// Send release downloads through `transport` in place of the
    /// network, as tests do with a stand-in for GitHub.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn set_release_transport(&mut self, transport: Arc<dyn local_core::WebTransport>) {
        self.releases = Arc::new(Releases::with_transport(transport));
    }

    /// Give the plugins this clock and time zone, from the next start on.
    pub(crate) fn set_clock(&mut self, clock: Arc<dyn DeviceClock>) {
        self.clock = Some(clock);
    }

    /// Take what running plugins asked the window `window` to show,
    /// oldest first.
    pub(crate) fn take_openings(&mut self, window: WindowKey) -> Vec<Opening> {
        let mut openings = self
            .openings
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut taken = Vec::new();
        openings.retain(|(asked, opening)| {
            if *asked == window {
                taken.push(opening.clone());
                false
            } else {
                true
            }
        });
        taken
    }

    /// Forget what running plugins asked the window `window` to show, as
    /// when it closes.
    pub(crate) fn forget_openings(&mut self, window: WindowKey) {
        self.take_openings(window);
    }

    pub(crate) fn version(&self) -> u64 {
        self.version
    }

    /// The object types running plugins hold, with their schemas, and the
    /// name people see of each plugin that holds one, by plugin ID.
    pub(crate) fn type_owners(&self) -> (ObjectTypes, BTreeMap<String, String>) {
        let names = self
            .types
            .held()
            .into_iter()
            .map(|(_, plugin)| {
                let name = self
                    .manifests
                    .get(&plugin)
                    .map_or_else(|| plugin.clone(), |manifest| manifest.name.clone());
                (plugin, name)
            })
            .collect();
        (self.types.clone(), names)
    }

    /// Counts the times running plugins registered or released an object
    /// type, so a window can check the workspace's files against their
    /// schemas again.
    pub(crate) fn types_generation(&self) -> u64 {
        self.types.generation()
    }

    fn changed(&mut self) {
        self.version += 1;
    }

    /// Whether a running plugin has calls whose answers haven't been
    /// taken yet.
    pub(crate) fn is_working(&self) -> bool {
        self.running.values().any(|running| running.outstanding > 0)
    }

    /// Whether a check for a newer release runs.
    pub(crate) fn is_checking(&self) -> bool {
        self.update_checks
            .values()
            .any(|check| *check == UpdateCheck::Checking)
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
        let library = self.app_data.join(LIBRARY);
        let host = if self.agent {
            PluginHost::shared(library)
        } else {
            PluginHost::new(library)
        };
        let host = Arc::new(host.map_err(|error| from_install(&error))?);
        self.host = Some(host.clone());
        let records = self.records.plugins.clone();
        if self.agent {
            for id in records.keys() {
                if let Ok((manifest, _)) = host.installed(id) {
                    self.manifests.insert(id.clone(), manifest);
                }
            }
            return Ok(host);
        }
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
                    if let Some(folder) = &record.folder {
                        self.watch(&id, folder, digest);
                    }
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
                        let revoked = lines_beyond(&manifest.capabilities, &granted);
                        (lines(&granted), revoked)
                    }
                    (None, Some(record)) => (lines(&record.approved), Vec::new()),
                    _ => (Vec::new(), Vec::new()),
                };
                let offer = self.updates.get(id).filter(|_| manifest.is_some());
                let update = match (offer, manifest) {
                    (Some(offer), Some(manifest)) => lines_beyond(
                        &offer.candidate.manifest().capabilities,
                        &manifest.capabilities,
                    ),
                    _ => Vec::new(),
                };
                let running = self.running.get(id);
                PluginRow {
                    id: id.clone(),
                    name: manifest.map_or_else(|| id.clone(), |manifest| manifest.name.clone()),
                    version: manifest.map(|manifest| manifest.version.to_string()),
                    folder: record.and_then(|record| record.folder.clone()),
                    release: record.and_then(|record| record.release.clone()),
                    newer: offer.map(|offer| offer.candidate.manifest().version.to_string()),
                    update_check: self.update_checks.get(id).cloned(),
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
        if let Some(running) = self.running.remove(id) {
            running.plugin.release_object_types();
        }
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
        let reader: Arc<dyn WorkspaceReads> = Arc::new(reader::WorkspaceReader(workspace.clone()));
        let objects: Arc<dyn WorkspaceObjects> =
            Arc::new(ObjectStore::new(workspace, self.types.clone()));
        let navigator = Arc::new(Navigator::new(id, self.openings.clone()));
        let navigation: Arc<dyn Navigation> = navigator.clone();
        let services = Services {
            workspace: Some(reader),
            objects: Some(objects),
            types: self.types.clone(),
            network: Some(self.network.clone()),
            router: Arc::downgrade(&self.router),
            clock: self.clock.clone(),
            navigation: Some(navigation),
            ai: self.ai.as_ref().map(|ai| ai.access(id)),
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
            navigator,
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
                panels: BTreeMap::new(),
                rows: HashMap::new(),
                drawn: 0,
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
        let Some(path) = folder.to_str() else {
            return PluginInstall::refused(shown, false, AppError::unsupported_path().message);
        };
        let host = match self.host() {
            Ok(host) => host,
            Err(error) => return PluginInstall::refused(shown, false, error.message),
        };
        match host.inspect(folder) {
            Ok(candidate) => PluginInstall::ready(path.to_owned(), false, candidate),
            Err(error) => PluginInstall::refused(shown, false, error.message),
        }
    }

    /// Read the release archive file `archive`, as `noura-plugin package`
    /// writes it, for the install dialog, without running any of its code.
    /// It goes through the same checks as a release downloaded from GitHub.
    pub(crate) fn review_archive(&mut self, archive: &Path) -> PluginInstall {
        let shown = archive.to_string_lossy().into_owned();
        let Some(path) = archive.to_str() else {
            return PluginInstall::refused(shown, false, AppError::unsupported_path().message);
        };
        let host = match self.host() {
            Ok(host) => host,
            Err(error) => return PluginInstall::refused(shown, false, error.message),
        };
        let bytes = match read_archive_file(archive) {
            Ok(bytes) => bytes,
            Err(problem) => return PluginInstall::refused(shown, false, problem),
        };
        match Archive::read(&bytes).and_then(|archive| host.inspect_archive(archive)) {
            Ok(candidate) => PluginInstall::ready(path.to_owned(), false, candidate),
            Err(error) => PluginInstall::refused(shown, false, error.message),
        }
    }

    /// Start reading the release at `address` on GitHub for the install
    /// dialog. An address that isn't a release on GitHub is refused at
    /// once, and nothing is fetched. Otherwise the dialog says the release
    /// is downloading, and the receiver gets the dialog to show once the
    /// download is read and compiled. None of the plugin's code runs.
    pub(crate) fn review_release(
        &mut self,
        address: &str,
    ) -> (PluginInstall, Option<Receiver<PluginInstall>>) {
        let shown = address.trim().to_owned();
        let release = match ReleaseUrl::parse(address) {
            Ok(release) => release,
            Err(error) => return (PluginInstall::refused(shown, true, error.message), None),
        };
        let host = match self.host() {
            Ok(host) => host,
            Err(error) => return (PluginInstall::refused(shown, true, error.message), None),
        };
        let page = release.page();
        let releases = self.releases.clone();
        let wake = self.wake.clone();
        let (sender, receiver) = channel();
        let downloading = page.clone();
        let spawned = std::thread::Builder::new()
            .name("plugin-download".into())
            .spawn(move || {
                let review = releases
                    .download(&release)
                    .and_then(|downloaded| {
                        let archive = Archive::read(&downloaded.archive)?;
                        let candidate = host.inspect_archive(archive)?;
                        Ok(PluginInstall::ready(downloaded.page, true, candidate))
                    })
                    .unwrap_or_else(|error| {
                        tracing::warn!(code = error.code.as_str(), "a plugin release was refused");
                        PluginInstall::refused(downloading, true, error.message)
                    });
                if sender.send(review).is_ok()
                    && let Some(wake) = wake
                {
                    wake();
                }
            });
        if let Err(error) = spawned {
            tracing::warn!(%error, "a plugin download didn't start");
            let problem = "Noura couldn't start the download.".to_owned();
            return (PluginInstall::refused(page, true, problem), None);
        }
        let waiting = PluginInstall {
            downloading: true,
            problem: None,
            ..PluginInstall::refused(page, true, String::new())
        };
        (waiting, Some(receiver))
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
        let source = Some(install.source.as_str());
        if install.from_github {
            self.install_candidate(candidate, None, source, false)
        } else if candidate.folder().is_some() {
            self.install_candidate(candidate, source, None, false)
        } else {
            // A release archive from a file holds one build, so there is
            // no folder to watch and no release to check.
            self.install_candidate(candidate, None, None, false)
        }
    }

    fn install_candidate(
        &mut self,
        candidate: &Candidate,
        folder: Option<&str>,
        release: Option<&str>,
        shipped: bool,
    ) -> Result<(), AppError> {
        let host = self.host()?;
        host.install(candidate)
            .map_err(|error| from_install(&error))?;
        let id = candidate.manifest().id.clone();
        let record = Record {
            folder: folder.map(str::to_owned),
            release: release.map(str::to_owned),
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
        if let Some(folder) = folder {
            self.watch(&id, folder, candidate.digest());
        }
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
            // The AI runs that relied on it end, and its files stay.
            if let Some(ai) = &self.ai {
                ai.plugin_disabled(id);
            }
            // What the other plugins show may come from this one's
            // commands, as a project's tasks come from Tasks. A plugin
            // that starts has them draw again once it registered.
            self.draw_again(Some(id));
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
        if let Some(ai) = &self.ai {
            ai.plugin_disabled(id);
        }
        self.watchers.remove(id);
        self.updates.remove(id);
        self.update_checks.remove(id);
        self.problems.remove(id);
        self.manifests.remove(id);
        self.save_records(|records| {
            records.plugins.remove(id);
        })?;
        let removed = host.uninstall(id).map_err(|error| from_install(&error));
        self.changed();
        removed
    }

    /// Approve what a waiting build adds, install it and run it. When the
    /// install fails, the installed build and your approvals stay as they
    /// were.
    pub(crate) fn approve_update(&mut self, id: &str) -> Result<(), AppError> {
        let Some(offer) = self.updates.get(id).cloned() else {
            return Err(no_update());
        };
        if !self.records.plugins.contains_key(id) {
            self.updates.remove(id);
            return Err(AppError::new(
                "plugin_not_installed",
                "This plugin isn't installed on this device.",
            ));
        }
        let added = self.added(id, &offer.candidate);
        match self.apply(id, &offer.candidate, &added, offer.release.as_deref()) {
            Ok(()) => {
                self.updates.remove(id);
                self.update_checks.remove(id);
                Ok(())
            }
            // A release from GitHub keeps waiting, so you can try again.
            Err(error) if offer.release.is_some() => Err(error),
            Err(error) => {
                self.updates.remove(id);
                self.failed_update(id, &error);
                Err(error)
            }
        }
    }

    /// Keep the installed build of the plugin `id` and drop the waiting
    /// one. Its folder offers a build again once it changes, and Check for
    /// updates offers a release again.
    pub(crate) fn decline_update(&mut self, id: &str) -> Result<(), AppError> {
        if self.updates.remove(id).is_none() {
            return Err(no_update());
        }
        self.changed();
        Ok(())
    }

    /// Start checking GitHub for a release of the plugin `id` newer than
    /// the installed one. The plugin's row says how it went once
    /// [`Self::poll`] takes the answer; a newer release waits there for
    /// you.
    pub(crate) fn check_update(&mut self, id: &str) -> Result<(), AppError> {
        let Some(record) = self.records.plugins.get(id) else {
            return Err(AppError::new(
                "plugin_not_installed",
                "This plugin isn't installed on this device.",
            ));
        };
        let Some(page) = record.release.clone() else {
            return Err(AppError::new(
                "plugin_not_from_github",
                "Only plugins installed from GitHub can check for updates there.",
            ));
        };
        if self.update_checks.get(id) == Some(&UpdateCheck::Checking) {
            return Ok(());
        }
        let release = ReleaseUrl::parse(&page)
            .map_err(|error| from_install(&error))?
            .latest();
        let host = self.host()?;
        let Some(installed) = self
            .manifests
            .get(id)
            .map(|manifest| manifest.version.clone())
        else {
            return Err(AppError::new(
                "plugin_not_installed",
                "This plugin isn't installed on this device.",
            ));
        };
        let releases = self.releases.clone();
        let sender = self.checks.0.clone();
        let wake = self.wake.clone();
        let plugin = id.to_owned();
        std::thread::Builder::new()
            .name("plugin-update-check".into())
            .spawn(move || {
                let found = releases.download(&release).and_then(|downloaded| {
                    let archive = Archive::read(&downloaded.archive)?;
                    let manifest = archive.manifest();
                    if manifest.id == plugin && manifest.version <= installed {
                        return Ok(Found::Newest);
                    }
                    let candidate = host.inspect_archive_update(&plugin, archive)?;
                    Ok(Found::Newer(Box::new(Offer {
                        candidate,
                        release: Some(downloaded.page),
                    })))
                });
                let checked = Checked { id: plugin, found };
                if sender.send(checked).is_ok()
                    && let Some(wake) = wake
                {
                    wake();
                }
            })
            .map_err(|error| {
                tracing::warn!(%error, "a check for plugin updates didn't start");
                AppError::new(
                    "plugin_check_failed",
                    "Noura couldn't start checking for updates.",
                )
            })?;
        self.update_checks
            .insert(id.to_owned(), UpdateCheck::Checking);
        self.changed();
        Ok(())
    }

    /// Show what a check for a newer release found. A newer release waits
    /// for you; anything else leaves the installed version as it is.
    fn take_check(&mut self, checked: Checked) {
        let id = checked.id;
        let Some(installed) = self
            .manifests
            .get(&id)
            .filter(|_| self.records.plugins.contains_key(&id))
            .map(|manifest| manifest.version.clone())
        else {
            self.update_checks.remove(&id);
            return;
        };
        let check = match checked.found {
            Ok(Found::Newer(offer)) if offer.candidate.manifest().version > installed => {
                self.updates.insert(id.clone(), *offer);
                None
            }
            Ok(_) => Some(UpdateCheck::Newest(installed.to_string())),
            Err(error) => {
                tracing::warn!(
                    code = error.code.as_str(),
                    "a check for plugin updates failed"
                );
                Some(UpdateCheck::Failed(error.message))
            }
        };
        match check {
            Some(check) => self.update_checks.insert(id, check),
            None => self.update_checks.remove(&id),
        };
        self.changed();
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
        self.router.insert(running.plugin.clone());
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
                        if let Err(error) = self.apply(&reload.id, &candidate, &[], None) {
                            self.failed_update(&reload.id, &error);
                        }
                    } else {
                        let offer = Offer {
                            candidate,
                            release: None,
                        };
                        self.updates.insert(reload.id.clone(), offer);
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
        while let Ok(checked) = self.checks.1.try_recv() {
            self.take_check(checked);
        }
        while let Ok(outcome) = self.outcomes.1.try_recv() {
            self.take(outcome);
        }
        let called: Vec<String> = self.called.try_iter().collect();
        for id in called {
            self.acts += 1;
            self.send(&id, Job::Redraw);
        }
        // A plugin's AI run wrote to its chat or streamed more of its
        // answer.
        let mut answered = self
            .ai
            .as_ref()
            .map(|ai| ai.take_redraws())
            .unwrap_or_default();
        // The providers or your consent changed: every plugin that chats
        // draws again.
        if self
            .ai
            .as_ref()
            .is_some_and(|ai| ai.take_settings_changed())
        {
            answered.extend(
                self.running
                    .iter()
                    .filter(|(_, running)| Capability::holds(&running.plugin.granted(), "ai"))
                    .map(|(id, _)| id.clone()),
            );
        }
        for id in answered {
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
        let started = outcome.registrations.is_some();
        if let Some(registrations) = outcome.registrations {
            running.registrations = registrations;
        }
        if let Some(views) = outcome.views {
            running.views = views;
            // Virtual lists ask for their rows again.
            for rows in running.rows.values_mut() {
                rows.again();
            }
            running.drawn += 1;
        }
        if let Some(fetched) = outcome.rows {
            running.take_rows(fetched);
        }
        if outcome.acted {
            self.acts += 1;
            // What it did may change any of its panels.
            for panel in running.panels.values_mut() {
                panel.drawn = Drawn::Nothing;
            }
        }
        if let Some(drawn) = outcome.panel {
            let panel = running
                .panels
                .entry(drawn.object)
                .or_insert_with(Panel::empty);
            if panel.requested.is_some() && panel.requested == drawn.revision {
                panel.requested = None;
            }
            panel.view = drawn.view;
            panel.drawn = drawn.revision.map_or(Drawn::AfterPress, Drawn::Revision);
        }
        if let Some(refused) = outcome.refused_types
            && !refused.is_empty()
        {
            self.problems.insert(outcome.id.clone(), refused.join(" "));
        }
        // A job that started before you revoked a capability may answer
        // after: what you revoked stays gone.
        let granted = running.plugin.granted();
        running
            .registrations
            .keep(|name| Capability::holds(&granted, name));
        let surfaces = running.registrations.surfaces();
        running
            .views
            .retain(|surface, _| surfaces.contains(surface));
        match outcome.failed {
            // The view where the event happened says why, until the
            // plugin draws it again.
            Some((Some(surface), message)) => {
                if running.views.contains_key(&surface) {
                    running.views.insert(surface, Err(message));
                }
            }
            Some((None, message)) => {
                self.problems.insert(outcome.id.clone(), message);
            }
            None => {}
        }
        // The plugin's health now, not as the job saw it: an answer from
        // before you started it again doesn't stop it a second time.
        if outcome.health.stopped && !running.failed && running.plugin.health().stopped {
            running.failed = true;
            running.registrations = Registrations::default();
            running.views.clear();
            self.router.remove(&outcome.id);
            self.problems.insert(
                outcome.id.clone(),
                "It failed too often and stopped for this session.".to_owned(),
            );
        }
        // The plugin registered its types and commands, so what the other
        // plugins show from its commands, as a project's tasks from Tasks,
        // may change.
        if started {
            self.draw_again(Some(&outcome.id));
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

    /// Install `candidate` over the plugin `id`, approve `added` for it,
    /// and run it in place of the old build when the plugin is running.
    /// What you approved for capabilities the new build no longer declares
    /// goes. A build from GitHub records `release`, its release page.
    /// Approvals change only once the new build installed.
    fn apply(
        &mut self,
        id: &str,
        candidate: &Candidate,
        added: &[Capability],
        release: Option<&str>,
    ) -> Result<(), AppError> {
        let host = self.host()?;
        host.update(candidate)
            .map_err(|error| from_install(&error))?;
        let declared = candidate.manifest().capabilities.clone();
        let saved = self.save_records(|records| {
            if let Some(record) = records.plugins.get_mut(id) {
                Capability::approve(&mut record.approved, added);
                record.approved = Capability::granted(&declared, &record.approved);
                if let Some(release) = release {
                    record.release = Some(release.to_owned());
                }
            }
        });
        if let Err(error) = saved {
            // The new build runs with what you approved before.
            tracing::warn!(code = %error.code, "the approvals of a new build weren't saved");
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

    /// Tell the plugin `plugin` about `event` in the view at `surface`,
    /// which you did in the window `origin`, and draw its views again once
    /// it has heard. An input value of the wrong form, such as a date that
    /// isn't `YYYY-MM-DD`, never reaches the plugin.
    fn event_at(&mut self, plugin: &str, surface: Surface, event: &Event, origin: WindowKey) {
        let shown = self
            .running
            .get(plugin)
            .is_some_and(|running| running.views.contains_key(&surface));
        if shown && well_formed_event(event) {
            self.send(plugin, Job::Event(surface, event.clone(), Some(origin)));
        }
    }

    /// Tell the plugin behind the rail page `page_id` about `event` there.
    pub(crate) fn send_event(&mut self, page_id: &str, event: &Event, origin: WindowKey) {
        if let Some((plugin, page)) = split_page_id(page_id) {
            let (plugin, page) = (plugin.to_owned(), page.to_owned());
            self.event_at(&plugin, Surface::Page(page), event, origin);
        }
    }

    /// The section the rail page `page_id` adds to the sidebar, when its
    /// plugin runs and drew one.
    pub(crate) fn sidebar_view(&self, page_id: &str) -> Option<PluginView> {
        let (plugin, page) = split_page_id(page_id)?;
        self.view_at(plugin, &Surface::Sidebar(page.to_owned()))
    }

    /// Tell the plugin behind the rail page `page_id` about `event` in the
    /// page's sidebar section.
    pub(crate) fn send_sidebar_event(&mut self, page_id: &str, event: &Event, origin: WindowKey) {
        if let Some((plugin, page)) = split_page_id(page_id) {
            let (plugin, page) = (plugin.to_owned(), page.to_owned());
            self.event_at(&plugin, Surface::Sidebar(page), event, origin);
        }
    }

    /// The rows in `range` of the virtual list `node` in the plugin page
    /// `page_id`, each `None` until the plugin returns it. The plugin
    /// fetches rows it hasn't returned yet on its own thread, in batches of
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
        let running = self
            .running
            .get_mut(plugin)
            .filter(|running| !running.failed)
            .ok_or_else(gone)?;
        let total = match running.views.get(&Surface::Page(page.to_owned())) {
            Some(Ok(view)) => virtual_list_rows(view.root(), node),
            _ => None,
        }
        .ok_or_else(|| format!("The page has no virtual list {node}."))?;
        let range = range.start.min(total)..range.end.min(total);
        let drawn = running.drawn;
        let rows = running
            .rows
            .entry((page.to_owned(), node.to_owned()))
            .or_default();
        if let Some(problem) = &rows.problem {
            return Err(problem.clone());
        }
        let first = range.start / ROW_BATCH * ROW_BATCH;
        let mut wanted = Vec::new();
        for start in (first..range.end).step_by(ROW_BATCH as usize) {
            if rows.asked.insert(start) {
                let count = ROW_BATCH.min(total - start);
                wanted.push(Job::Rows {
                    page: page.to_owned(),
                    node: node.to_owned(),
                    start,
                    count,
                    drawn,
                });
            }
        }
        let shown = range.map(|index| rows.get(index)).collect();
        let plugin = plugin.to_owned();
        for job in wanted {
            self.send(&plugin, job);
        }
        Ok(shown)
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

    /// Run the palette command `command_id`, which you ran in the window
    /// `origin`. Returns whether a running plugin offers it.
    pub(crate) fn run_command(&mut self, command_id: &str, origin: WindowKey) -> bool {
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
            self.send(&plugin, Job::Command(command, Some(origin)));
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

    /// Tell the plugin `plugin` about `event` in its Inbox section
    /// `section`.
    pub(crate) fn send_inbox(
        &mut self,
        plugin: &str,
        section: &str,
        event: &Event,
        origin: WindowKey,
    ) {
        let surface = Surface::InboxSection(section.to_owned());
        self.event_at(plugin, surface, event, origin);
    }

    /// The files changed, as the listing `listing` shows: each running
    /// plugin draws its views again, once per listing, and its properties
    /// panels the next time they show, since a panel may show other files
    /// than its object's, as a project's shows its tasks.
    pub(crate) fn files_changed(&mut self, listing: u64) {
        if listing <= self.listing_drawn {
            return;
        }
        self.listing_drawn = listing;
        self.draw_again(None);
    }

    /// Have each running plugin but `except` draw its views again, and
    /// its properties panels the next time they show.
    fn draw_again(&mut self, except: Option<&str>) {
        let ids: Vec<String> = self
            .running
            .keys()
            .filter(|id| Some(id.as_str()) != except)
            .cloned()
            .collect();
        for id in ids {
            if let Some(running) = self.running.get_mut(&id) {
                for panel in running.panels.values_mut() {
                    panel.drawn = Drawn::Nothing;
                }
            }
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

    /// Tell the plugin behind its page `section_id` in Settings about
    /// `event` there.
    pub(crate) fn send_settings(&mut self, section_id: &str, event: &Event, origin: WindowKey) {
        if let Some(plugin) = section_id.strip_prefix(SETTINGS_PREFIX) {
            let plugin = plugin.to_owned();
            self.event_at(&plugin, Surface::Settings, event, origin);
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

    /// The name of the installed plugin `id`, as its manifest gives it.
    pub(crate) fn plugin_name(&self, id: &str) -> Option<String> {
        self.manifests.get(id).map(|manifest| manifest.name.clone())
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

    /// A call for an AI tool outside Noura to the tool `name` of the
    /// plugin `plugin`, as [`Self::tool_call`] makes one. The plugin must
    /// still be on in the workspace's manifest on disk and still hold the
    /// `tools` capability in this device's approvals, which the app may
    /// have changed since this service started it. A plugin turned off
    /// since stops here, and one whose approvals changed runs with what
    /// you approve now.
    pub(crate) fn agent_tool_call(
        &mut self,
        plugin: &str,
        name: &str,
        arguments: &str,
    ) -> Result<ToolCall, AppError> {
        let shown = self
            .manifests
            .get(plugin)
            .map_or_else(|| plugin.to_owned(), |manifest| manifest.name.clone());
        let workspace = self
            .workspace
            .clone()
            .ok_or_else(|| AppError::new("no_workspace", "No workspace is open for this tool."))?;
        let manifest = workspace.engine().read_manifest().map_err(|error| {
            AppError::from_core(error, workspace.root(), crate::error::Access::ReadWorkspace)
        })?;
        if !manifest.enabled_plugins.iter().any(|id| id == plugin) {
            let message = if self.running.contains_key(plugin) {
                format!(
                    "You turned {shown} off in this workspace after this connection \
                     started, so Noura didn't run its tool. Nothing changed."
                )
            } else {
                format!("{shown} is off in this workspace, so Noura didn't run its tool.")
            };
            self.stop(plugin);
            return Err(AppError::new("plugin_disabled", message));
        }
        self.records = self.store.load();
        let Some(record) = self.records.plugins.get(plugin) else {
            self.stop(plugin);
            return Err(AppError::new(
                "plugin_not_installed",
                format!(
                    "{shown} isn't installed on this device anymore, so Noura didn't run its \
                     tool. Nothing changed."
                ),
            ));
        };
        if let Some(running) = self.running.get(plugin) {
            running.plugin.set_approved(&record.approved);
            if !Capability::holds(&running.plugin.granted(), "tools") {
                return Err(AppError::new(
                    "plugin_tools_revoked",
                    format!(
                        "You took back {shown}'s permission to offer tools on this device, \
                         so Noura didn't run its tool. Nothing changed."
                    ),
                ));
            }
        }
        self.tool_call(plugin, name, arguments)
    }

    /// Draw the plugin `id`'s views again, as after another caller ran it.
    pub(crate) fn redraw(&mut self, id: &str) {
        self.acts += 1;
        self.send(id, Job::Redraw);
    }

    /// Use the plugins in `folder` as the ones that ship with Noura: each
    /// subfolder that holds a plugin.
    pub(crate) fn set_shipped(&mut self, folder: PathBuf) {
        self.shipped = Some(folder);
        self.shipped_pending = self.shipped_folders();
        self.changed();
    }

    /// The properties panel of the object `object_id` of type
    /// `object_type`, whose file has the revision `revision`, as the
    /// running plugin that holds the type draws it. `None` when no running
    /// plugin holds the type, so the kernel's panel shows. The plugin draws
    /// on its own thread: until it answers, the panel it drew last shows,
    /// or an empty one.
    pub(crate) fn panel(
        &mut self,
        object_type: &str,
        object_id: &str,
        revision: &str,
    ) -> Option<PluginView> {
        let owner = self.types.owner(object_type)?;
        let running = self
            .running
            .get_mut(&owner)
            .filter(|running| !running.failed)?;
        let panel = running
            .panels
            .entry(object_id.to_owned())
            .or_insert_with(Panel::empty);
        if panel.current_for(revision) {
            return Some(panel.view.clone());
        }
        let view = panel.view.clone();
        if panel.requested.as_deref() != Some(revision) {
            panel.requested = Some(revision.to_owned());
            let job = Job::Panel(object_id.to_owned(), revision.to_owned());
            self.send(&owner, job);
        }
        Some(view)
    }

    /// Tell the plugin that holds `object_type` about `event` in the
    /// properties panel of the object `object_id`. It draws the panel and
    /// its views again once it has heard. An input value of the wrong form
    /// never reaches the plugin.
    pub(crate) fn send_panel(
        &mut self,
        object_type: &str,
        object_id: &str,
        event: &Event,
        origin: WindowKey,
    ) {
        let Some(owner) = self.types.owner(object_type) else {
            return;
        };
        if self.running.contains_key(&owner) && well_formed_event(event) {
            let job = Job::PanelEvent(object_id.to_owned(), event.clone(), Some(origin));
            self.send(&owner, job);
        }
    }

    /// How many times a running plugin ran code that may have written
    /// files: it handled an event, or ran a command for the palette,
    /// another plugin or a tool. Each window's open notes look at their
    /// files when this changes.
    pub(crate) fn acts(&self) -> u64 {
        self.acts
    }

    /// The plugins shipping with Noura that this device hasn't shown you
    /// yet, each read from its manifest. None of their code has run.
    pub(crate) fn shipped_review(&self) -> Vec<ShippedPlugin> {
        self.shipped_pending
            .iter()
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
        for (manifest, folder) in std::mem::take(&mut self.shipped_pending) {
            let id = manifest.id.clone();
            let installed = folder
                .to_str()
                .ok_or_else(AppError::unsupported_path)
                .and_then(|path| {
                    let candidate = host
                        .inspect(&folder)
                        .map_err(|error| from_install(&error))?;
                    self.install_candidate(&candidate, Some(path), None, true)
                });
            if let Err(error) = installed {
                tracing::warn!(code = %error.code, "a plugin shipping with Noura didn't install");
                self.problems.insert(id.clone(), error.message);
                let reviewed = self.save_records(|records| {
                    records.shipped_reviewed.insert(id);
                });
                if let Err(error) = reviewed {
                    self.shipped_pending = self.shipped_folders();
                    return Err(error);
                }
            }
        }
        self.shipped_pending = self.shipped_folders();
        self.changed();
        Ok(())
    }
}

impl Running {
    /// Keep the rows a plugin fetched for a virtual list, unless its views
    /// were drawn again since it was asked. A batch that breaks the list's
    /// rules stops the list.
    fn take_rows(&mut self, fetched: RowsFetched) {
        if fetched.drawn != self.drawn {
            return;
        }
        let Some(rows) = self.rows.get_mut(&(fetched.page, fetched.node)) else {
            return;
        };
        let checked = fetched
            .rows
            .and_then(|batch| check_rows(batch, fetched.count).map_err(|error| error.to_string()))
            .and_then(|batch| {
                // `check_rows` checks one batch; a row may not repeat the
                // ID of a row in another.
                match batch.iter().find(|row| rows.ids.contains(&row.id)) {
                    Some(row) => Err(ViewError::DuplicateRowId { id: row.id.clone() }.to_string()),
                    None => Ok(batch),
                }
            });
        match checked {
            Ok(batch) => {
                rows.ids.extend(batch.iter().map(|row| row.id.clone()));
                let end = fetched.start + fetched.count;
                rows.earlier
                    .retain(|index, _| !(fetched.start..end).contains(index));
                rows.rows.extend((fetched.start..).zip(batch));
            }
            Err(problem) => rows.problem = Some(problem),
        }
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
pub(super) fn virtual_list_in(node: &ViewNode) -> Option<&str> {
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

/// The bytes of the release archive file `archive`, refused past the size
/// a release download may have.
fn read_archive_file(archive: &Path) -> Result<Vec<u8>, String> {
    let unreadable = |error: std::io::Error| {
        tracing::warn!(%error, "a plugin archive file didn't read");
        "Noura couldn't read the archive.".to_owned()
    };
    let too_large = || {
        format!(
            "The archive is larger than {} MB.",
            plugin_host::MAX_ARCHIVE_BYTES >> 20
        )
    };
    let file = std::fs::File::open(archive).map_err(unreadable)?;
    let limit = plugin_host::MAX_ARCHIVE_BYTES as u64;
    if file.metadata().map_err(unreadable)?.len() > limit {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut std::io::Read::take(file, limit + 1), &mut bytes)
        .map_err(unreadable)?;
    if bytes.len() as u64 > limit {
        return Err(too_large());
    }
    Ok(bytes)
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
