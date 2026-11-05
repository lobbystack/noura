//! A started plugin that several threads may call, and the router that
//! carries one plugin's command calls to another.
//!
//! [`RunningPlugin`] starts its store on its first call and again after a
//! call fails, since a store that trapped can't run again. A plugin that
//! fails [`MAX_FAILURES`] times within [`FAILURE_WINDOW`] stops for the
//! session until [`RunningPlugin::restart`]. Calls into one plugin take
//! turns. A command call from another plugin waits at most the call
//! deadline for its turn, and a call that would come back into a plugin
//! waiting for it fails at once, so a cycle never hangs.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use wasmtime::Engine;
use wasmtime::component::Component;

use crate::{
    Capability, Command, Event, InboxSection, Limits, Node, Page, PluginError, PluginManifest,
    Services, Tool, VirtualRow,
    instance::{CallError, Grants, Owner, PluginInstance, Registration},
    object_types::next_instance,
    runtime::EpochTicker,
};

/// How many failures stop a plugin for the session, when they come within
/// [`FAILURE_WINDOW`] of each other.
pub const MAX_FAILURES: usize = 3;

/// See [`MAX_FAILURES`].
pub const FAILURE_WINDOW: Duration = Duration::from_secs(60);

/// Where a plugin draws a view.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Surface {
    /// A page in the rail, by the plugin's page ID.
    Page(String),
    /// The section a page adds to the sidebar while it shows, by the
    /// plugin's page ID.
    Sidebar(String),
    /// A section in the Inbox, by the plugin's section ID.
    InboxSection(String),
    /// The plugin's page in Settings.
    Settings,
}

impl Surface {
    /// The capability a plugin needs to draw here.
    pub fn capability(&self) -> &'static str {
        match self {
            Self::Page(_) | Self::Sidebar(_) => "views",
            Self::InboxSection(_) => "inbox-sections",
            Self::Settings => "settings",
        }
    }
}

/// Whether a plugin keeps failing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Health {
    /// Its failures within the last [`FAILURE_WINDOW`].
    pub recent_failures: usize,
    /// It failed [`MAX_FAILURES`] times within [`FAILURE_WINDOW`] and runs
    /// no more calls until [`RunningPlugin::restart`].
    pub stopped: bool,
}

/// A plugin's store, when it has one. A call holds it while it runs.
type Slot = Option<PluginInstance>;

/// A plugin's recent failures. It has its own lock, which no call holds
/// while the plugin runs, so reading a plugin's health never waits for
/// its call.
#[derive(Default)]
struct Failures {
    /// When each recent failure happened, oldest first.
    at: Vec<Instant>,
    stopped: bool,
}

/// A started plugin. Each call runs its code in the plugin's own store,
/// with only the capabilities the plugin declares and you approved. The
/// object types it registers stay its own across new stores until it is
/// dropped or loses the `object-types` capability.
pub struct RunningPlugin {
    manifest: PluginManifest,
    /// Holds the object types the plugin registers.
    token: u64,
    component: Component,
    engine: Engine,
    epochs: Arc<EpochTicker>,
    limits: Limits,
    grants: Grants,
    services: Services,
    slot: Mutex<Slot>,
    failures: Mutex<Failures>,
}

impl RunningPlugin {
    pub(crate) fn new(
        manifest: PluginManifest,
        component: Component,
        engine: Engine,
        epochs: Arc<EpochTicker>,
        limits: Limits,
        approved: &[Capability],
        services: Services,
    ) -> Self {
        let grants = Arc::new(RwLock::new(Capability::granted(
            &manifest.capabilities,
            approved,
        )));
        Self {
            manifest,
            token: next_instance(),
            component,
            engine,
            epochs,
            limits,
            grants,
            services,
            slot: Mutex::new(None),
            failures: Mutex::new(Failures::default()),
        }
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }

    /// What the plugin may use now: what it declares and you approved.
    pub fn granted(&self) -> Vec<Capability> {
        self.grants.read().clone()
    }

    /// Use `approved` from the plugin's next call on, as after you revoke
    /// a capability. Without `object-types`, the plugin's types are no
    /// longer its own.
    pub fn set_approved(&self, approved: &[Capability]) {
        let granted = Capability::granted(&self.manifest.capabilities, approved);
        if !Capability::holds(&granted, "object-types") {
            self.services.types.release(self.token);
        }
        *self.grants.write() = granted;
    }

    /// Whether the plugin keeps failing. This doesn't wait for a call
    /// that is running.
    pub fn health(&self) -> Health {
        let mut failures = self.failures.lock();
        failures.forget_old();
        Health {
            recent_failures: failures.at.len(),
            stopped: failures.stopped,
        }
    }

    /// Let a stopped plugin run again, in a new store, with its failures
    /// forgotten. A stopped plugin runs no call, so this waits for none.
    pub fn restart(&self) {
        let mut slot = self.slot.lock();
        *slot = None;
        *self.failures.lock() = Failures::default();
    }

    pub fn pages(&self) -> Result<Vec<Page>, PluginError> {
        self.call(PluginInstance::pages)
    }

    pub fn render(&self, page: &str) -> Result<Vec<Node>, PluginError> {
        self.call(|instance| instance.render(page))
    }

    /// Rows `start` up to `start + count` of the virtual list `node` in a
    /// page's view, as the plugin returned them. See [`crate::check_rows`]
    /// for the checks before drawing them.
    pub fn rows(
        &self,
        page: &str,
        node: &str,
        start: u32,
        count: u32,
    ) -> Result<Vec<VirtualRow>, PluginError> {
        self.call(|instance| instance.rows(page, node, start, count))
    }

    pub fn handle_event(&self, page: &str, event: &Event) -> Result<(), PluginError> {
        self.call(|instance| instance.handle_event(page, event))
    }

    /// The nodes of the section the page `page` adds to the sidebar: none
    /// for no section.
    pub fn render_sidebar(&self, page: &str) -> Result<Vec<Node>, PluginError> {
        self.call(|instance| instance.render_sidebar(page))
    }

    pub fn commands(&self) -> Result<Vec<Command>, PluginError> {
        self.call(PluginInstance::commands)
    }

    /// Run the command `id`. The inner result is the command's own
    /// answer or the message it failed with.
    pub fn run_command(
        &self,
        id: &str,
        input: &str,
    ) -> Result<Result<String, String>, PluginError> {
        self.call(|instance| instance.run_command(id, input))
    }

    pub fn inbox_sections(&self) -> Result<Vec<InboxSection>, PluginError> {
        self.call(PluginInstance::inbox_sections)
    }

    pub fn settings_page(&self) -> Result<Option<String>, PluginError> {
        self.call(PluginInstance::settings_page)
    }

    pub fn tools(&self) -> Result<Vec<Tool>, PluginError> {
        self.call(PluginInstance::tools)
    }

    /// Run the tool `name` with JSON `arguments`. The inner result is the
    /// tool's own answer or the message it failed with.
    pub fn call_tool(
        &self,
        name: &str,
        arguments: &str,
    ) -> Result<Result<String, String>, PluginError> {
        self.call(|instance| instance.call_tool(name, arguments))
    }

    /// Ask the plugin for its object types and register each one in the
    /// workspace's registry. Needs the `object-types` capability. Each type
    /// comes back with whether it registered: the kernel's own types, a
    /// type another plugin holds and an invalid schema each fail with a
    /// structured error, and the other types register all the same. Types
    /// it registered before and no longer lists stay its own.
    pub fn register_object_types(&self) -> Result<Vec<Registration>, PluginError> {
        self.call(PluginInstance::register_object_types)
    }

    /// Give up the object types the plugin holds, as when it turns off.
    /// Another start of it registers them again.
    pub fn release_object_types(&self) {
        self.services.types.release(self.token);
    }

    /// The types the plugin holds, sorted.
    pub fn object_types(&self) -> Vec<String> {
        let types = &self.services.types;
        types
            .types_of(&self.manifest.id)
            .into_iter()
            .filter(|object_type| types.held_by(object_type, self.token))
            .collect()
    }

    /// The nodes of the properties panel of the object `object`, one of
    /// the plugin's types. Needs the `object-types` capability.
    pub fn render_panel(&self, object: &str) -> Result<Vec<Node>, PluginError> {
        self.call(|instance| instance.render_panel(object))
    }

    /// Tell the plugin about `event` in the properties panel of the object
    /// `object`.
    pub fn handle_panel_event(&self, object: &str, event: &Event) -> Result<(), PluginError> {
        self.call(|instance| instance.handle_panel_event(object, event))
    }

    /// The nodes of the view at `surface`.
    pub fn render_surface(&self, surface: &Surface) -> Result<Vec<Node>, PluginError> {
        self.call(|instance| match surface {
            Surface::Page(page) => instance.render(page),
            Surface::Sidebar(page) => instance.render_sidebar(page),
            Surface::InboxSection(section) => instance.render_inbox_section(section),
            Surface::Settings => instance.render_settings(),
        })
    }

    /// Tell the plugin about `event` in the view at `surface`.
    pub fn handle_surface_event(
        &self,
        surface: &Surface,
        event: &Event,
    ) -> Result<(), PluginError> {
        self.call(|instance| match surface {
            Surface::Page(page) | Surface::Sidebar(page) => instance.handle_event(page, event),
            Surface::InboxSection(section) => instance.handle_inbox_event(section, event),
            Surface::Settings => instance.handle_settings_event(event),
        })
    }

    /// A call that nothing else waits for: it takes its turn however long
    /// the plugin's other calls take.
    fn call<R>(
        &self,
        call: impl FnOnce(&mut PluginInstance) -> Result<R, PluginError>,
    ) -> Result<R, PluginError> {
        let mut slot = self.slot.lock();
        self.run(&mut slot, vec![self.manifest.id.clone()], call)
    }

    /// A call that the plugins in `chain` wait for. It fails as busy when
    /// it doesn't get its turn within the call deadline.
    fn call_for(
        &self,
        chain: &[String],
        call: impl FnOnce(&mut PluginInstance) -> Result<Offered, PluginError>,
    ) -> Result<Offered, PluginError> {
        let Some(mut slot) = self.slot.try_lock_for(self.limits.call_deadline) else {
            return Err(PluginError::Busy {
                plugin: self.manifest.id.clone(),
            });
        };
        let mut chain = chain.to_vec();
        chain.push(self.manifest.id.clone());
        self.run(&mut slot, chain, call)
    }

    fn run<R>(
        &self,
        slot: &mut Slot,
        chain: Vec<String>,
        call: impl FnOnce(&mut PluginInstance) -> Result<R, PluginError>,
    ) -> Result<R, PluginError> {
        if self.failures.lock().stopped {
            return Err(PluginError::Stopped);
        }
        let instance = match &mut *slot {
            Some(instance) => instance,
            empty => match PluginInstance::start(
                &self.engine,
                &self.epochs,
                self.limits,
                &self.component,
                Owner {
                    plugin: &self.manifest.id,
                    token: self.token,
                },
                self.grants.clone(),
                self.services.clone(),
            ) {
                Ok(instance) => empty.insert(instance),
                Err(error) => {
                    self.failed(&error);
                    return Err(error);
                }
            },
        };
        instance.begin(chain);
        let result = call(instance);
        if let Err(error) = &result
            && error.is_failure()
        {
            // A store that trapped can't run again.
            *slot = None;
            self.failed(error);
        }
        result
    }

    /// Note a failure, and stop the plugin when it keeps failing.
    fn failed(&self, error: &PluginError) {
        let id = &self.manifest.id;
        tracing::warn!(plugin = id, %error, "a plugin call failed");
        let mut failures = self.failures.lock();
        failures.forget_old();
        failures.at.push(Instant::now());
        if failures.at.len() >= MAX_FAILURES {
            tracing::warn!(plugin = id, "a plugin kept failing and stopped");
            failures.stopped = true;
        }
    }
}

impl Drop for RunningPlugin {
    fn drop(&mut self) {
        self.services.types.release(self.token);
    }
}

impl Failures {
    fn forget_old(&mut self) {
        self.at.retain(|failed| failed.elapsed() < FAILURE_WINDOW);
    }
}

/// What a command call found in its target.
enum Offered {
    /// The target ran the command: its answer or its own failure.
    Ran(Result<String, String>),
    /// The target doesn't offer the command to other plugins.
    No,
}

/// Hears which plugin ran a command another plugin called, or failed
/// while running one, so the app can draw that plugin's views again and
/// learn whether it stopped.
pub type CalledListener = Arc<dyn Fn(&str) + Send + Sync>;

/// The running plugins whose commands other plugins may call.
#[derive(Default)]
pub struct Router {
    plugins: RwLock<BTreeMap<String, Arc<RunningPlugin>>>,
    called: RwLock<Option<CalledListener>>,
}

impl Router {
    /// Let other plugins call `plugin`'s commands, in place of any plugin
    /// with its ID.
    pub fn insert(&self, plugin: Arc<RunningPlugin>) {
        self.plugins.write().insert(plugin.id().to_owned(), plugin);
    }

    /// Stop routing calls to the plugin `id`, as when it turns off.
    pub fn remove(&self, id: &str) {
        self.plugins.write().remove(id);
    }

    /// Call `listener` with a plugin's ID after it runs a command another
    /// plugin called, or fails while running one.
    pub fn on_called(&self, listener: CalledListener) {
        *self.called.write() = Some(listener);
    }

    /// Run `command` of `plugin` for the plugins in `chain`, the caller
    /// last. The caller's capabilities were checked already.
    pub(crate) fn call(
        &self,
        chain: &[String],
        plugin: &str,
        command: &str,
        input: &str,
    ) -> Result<String, CallError> {
        let target = format!("{plugin}/{command}");
        if chain.iter().any(|waiting| waiting == plugin) {
            return Err(CallError::Busy(plugin.to_owned()));
        }
        let Some(running) = self.plugins.read().get(plugin).cloned() else {
            return Err(CallError::TargetUnavailable(target));
        };
        let outcome = running.call_for(chain, |instance| {
            let offers = instance
                .commands()?
                .iter()
                .any(|offered| offered.id == command && offered.plugins);
            if !offers {
                return Ok(Offered::No);
            }
            instance.run_command(command, input).map(Offered::Ran)
        });
        let ran = match &outcome {
            Ok(Offered::Ran(_)) => true,
            Err(error) => error.is_failure(),
            Ok(Offered::No) => false,
        };
        if ran && let Some(listener) = self.called.read().clone() {
            listener(plugin);
        }
        match outcome {
            Ok(Offered::Ran(answer)) => answer.map_err(CallError::Failed),
            Ok(Offered::No)
            | Err(PluginError::CapabilityNotDeclared { .. } | PluginError::Stopped) => {
                Err(CallError::TargetUnavailable(target))
            }
            Err(PluginError::Busy { plugin }) => Err(CallError::Busy(plugin)),
            // The target's own failure, such as a trap's message, stays
            // in the local log: the caller is another plugin's code.
            Err(error) => {
                tracing::warn!(%error, target, "a plugin failed running another plugin's call");
                Err(CallError::Failed(format!(
                    "The {plugin} plugin failed while running {command}."
                )))
            }
        }
    }
}
