//! One store of a running plugin: the capability interfaces linked into
//! it, the deadline and memory cap of each call, and calls into its
//! exports. [`crate::RunningPlugin`] starts a new store after a call fails.

use std::sync::{Arc, Weak};
use std::time::{Duration, Instant};

use local_core::{CoreError, HostRequest, HostRequests, WebMethod};
use parking_lot::RwLock;
use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Engine, Store, UpdateDeadline};

use crate::{
    Capability, EntryKind, Limits, Listed, Router, ServiceError, WorkspacePath, WorkspaceReads,
    runtime::{EpochTicker, Failure, StoreLimiter, deadline_ticks},
};

mod bindings {
    wasmtime::component::bindgen!({
        path: "../plugin-sdk/wit",
        world: "plugin",
        additional_derives: [PartialEq, Eq],
    });
}

use bindings::Plugin;
pub use bindings::exports::noura::plugin::commands::Command;
pub use bindings::exports::noura::plugin::inbox::Section as InboxSection;
pub use bindings::exports::noura::plugin::tools::Tool;
pub use bindings::exports::noura::plugin::view::Page;
use bindings::exports::noura::plugin::{
    commands as commands_export, inbox as inbox_export, settings as settings_export,
    tools as tools_export, view as view_export,
};
pub(crate) use bindings::noura::plugin::types::Error as CallError;
pub use bindings::noura::plugin::ui::{Event, Node, NodeKind};
use bindings::noura::plugin::{command_calls, network, types, workspace};

/// The longest one call may take in all, time spent waiting for the
/// network or for another plugin included. The call deadline counts only
/// the plugin's own work, so this bounds a plugin that waits in a loop.
const MAX_CALL_TIME: Duration = Duration::from_secs(120);

/// The capabilities a running plugin may use now. Revoking one changes
/// this, and the plugin's next call sees the change.
pub(crate) type Grants = Arc<RwLock<Vec<Capability>>>;

/// Why a call into a running plugin failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    /// The plugin doesn't hold the capability the call needs: its
    /// manifest doesn't declare it, or you revoked it. The host made no
    /// call.
    #[error("the plugin doesn't hold the {capability} capability")]
    CapabilityNotDeclared { capability: &'static str },
    #[error("the plugin failed to start: {0}")]
    Instantiate(String),
    #[error("the plugin needs more than its {cap_bytes}-byte memory cap")]
    MemoryCap { cap_bytes: usize },
    #[error("the plugin ran past its {deadline:?} deadline")]
    Deadline { deadline: Duration },
    #[error("the plugin failed: {0}")]
    Call(String),
    /// Another call into the plugin was running and didn't finish in
    /// time, or the call came back into a plugin waiting for it.
    #[error("the plugin {plugin} is busy")]
    Busy { plugin: String },
    /// The plugin failed too often and stopped for this session.
    #[error("the plugin failed too often and stopped")]
    Stopped,
    /// The plugin's component doesn't export the interface the call needs.
    #[error("the plugin doesn't export {interface}")]
    NotExported { interface: &'static str },
}

impl PluginError {
    /// Whether the plugin itself failed: it trapped, ran past its deadline
    /// or outgrew its memory cap. The host then starts a new store for its
    /// next call, and counts the failure.
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            Self::Instantiate(_) | Self::MemoryCap { .. } | Self::Deadline { .. } | Self::Call(_)
        )
    }
}

/// The services a plugin's capability interfaces call. The app provides
/// them; the host checks each call against the plugin's capabilities
/// first.
#[derive(Clone, Default)]
pub struct Services {
    /// Reads the open workspace, for `workspace-read`.
    pub workspace: Option<Arc<dyn WorkspaceReads>>,
    /// Sends requests through `local-core`'s web access boundary, for
    /// `network`.
    pub network: Option<Arc<HostRequests>>,
    /// Finds the plugins whose commands a plugin calls, for the targets of
    /// `commands`.
    pub router: Weak<Router>,
}

/// How long the current call has run.
#[derive(Clone, Copy)]
struct Clock {
    started: Instant,
    /// Time spent waiting for the network or another plugin, which the
    /// deadline doesn't count.
    waited: Duration,
    deadline: Duration,
}

impl Clock {
    fn start(deadline: Duration) -> Self {
        Self {
            started: Instant::now(),
            waited: Duration::ZERO,
            deadline,
        }
    }

    /// The plugin's own time in the call so far.
    fn used(&self) -> Duration {
        self.started.elapsed().saturating_sub(self.waited)
    }
}

/// What a plugin's store holds: its memory cap, the capabilities it may
/// use, the services they call, and the call in progress.
pub(crate) struct HostState {
    limiter: StoreLimiter,
    grants: Grants,
    services: Services,
    /// The plugins whose calls wait for this one, this plugin last.
    chain: Vec<String>,
    clock: Clock,
}

impl HostState {
    fn holds(&self, name: &str) -> bool {
        Capability::holds(&self.grants.read(), name)
    }

    /// The workspace service, when the plugin holds `workspace-read`.
    fn workspace(&self) -> Result<&dyn WorkspaceReads, types::Error> {
        if !self.holds("workspace-read") {
            return Err(types::Error::CapabilityNotDeclared(
                "workspace-read".to_owned(),
            ));
        }
        self.services
            .workspace
            .as_deref()
            .ok_or_else(|| types::Error::Failed("No workspace is open.".to_owned()))
    }

    /// Run `wait`, which waits for something outside the plugin, without
    /// counting the time toward the call's deadline.
    fn waiting<R>(&mut self, wait: impl FnOnce(&mut Self) -> R) -> R {
        let started = Instant::now();
        let result = wait(self);
        self.clock.waited += started.elapsed();
        result
    }
}

impl types::Host for HostState {}

impl bindings::noura::plugin::ui::Host for HostState {}

impl workspace::Host for HostState {
    fn list_entries(
        &mut self,
    ) -> Result<Vec<Result<workspace::Entry, types::Error>>, types::Error> {
        let listed = self.workspace()?.list_entries().map_err(wit_error)?;
        Ok(listed
            .into_iter()
            .map(|listed| match listed {
                Listed::Entry(entry) => Ok(workspace::Entry {
                    path: entry.path.as_str().to_owned(),
                    kind: match entry.kind {
                        EntryKind::File => workspace::EntryKind::File,
                        EntryKind::Folder => workspace::EntryKind::Folder,
                    },
                    object_id: entry.object_id,
                    object_type: entry.object_type,
                    revision: entry.revision,
                }),
                Listed::Unsupported => Err(types::Error::UnsupportedPath),
            })
            .collect())
    }

    fn read_file(&mut self, path: String) -> Result<workspace::File, types::Error> {
        let workspace = self.workspace()?;
        let path = WorkspacePath::new(&path).map_err(wit_error)?;
        let file = workspace.read_file(&path).map_err(wit_error)?;
        Ok(workspace::File {
            path: file.path.as_str().to_owned(),
            bytes: file.bytes,
            revision: file.revision,
        })
    }

    fn read_object(&mut self, id: String) -> Result<workspace::Object, types::Error> {
        let object = self.workspace()?.read_object(&id).map_err(wit_error)?;
        Ok(workspace::Object {
            id: object.id,
            object_type: object.object_type,
            title: object.title,
            path: object.path.as_str().to_owned(),
            body: object.body,
            properties: object.properties,
            revision: object.revision,
        })
    }
}

impl network::Host for HostState {
    fn fetch(&mut self, request: network::Request) -> Result<network::Response, types::Error> {
        let hosts = {
            let grants = self.grants.read();
            if !Capability::holds(&grants, "network") {
                return Err(types::Error::CapabilityNotDeclared("network".to_owned()));
            }
            Capability::hosts(&grants)
        };
        let Some(requests) = self.services.network.clone() else {
            return Err(types::Error::Failed(
                "Noura can't send network requests here.".to_owned(),
            ));
        };
        let request = HostRequest {
            method: match request.method {
                network::Method::Get => WebMethod::Get,
                network::Method::Head => WebMethod::Head,
                network::Method::Post => WebMethod::Post,
                network::Method::Put => WebMethod::Put,
                network::Method::Patch => WebMethod::Patch,
                network::Method::Delete => WebMethod::Delete,
            },
            url: request.url,
            headers: request
                .headers
                .into_iter()
                .map(|header| (header.name, header.value))
                .collect(),
            body: request.body,
        };
        let response = self
            .waiting(|_| requests.send_blocking(request, &hosts))
            .map_err(network_error)?;
        Ok(network::Response {
            status: response.status,
            url: response.url,
            headers: response
                .headers
                .into_iter()
                .map(|(name, value)| network::Header { name, value })
                .collect(),
            body: response.body,
        })
    }
}

impl command_calls::Host for HostState {
    fn call(
        &mut self,
        plugin: String,
        command: String,
        input: String,
    ) -> Result<String, types::Error> {
        {
            let grants = self.grants.read();
            if !Capability::holds(&grants, "commands") {
                return Err(types::Error::CapabilityNotDeclared("commands".to_owned()));
            }
            if !Capability::allows_target(&grants, &plugin, &command) {
                return Err(types::Error::TargetNotDeclared(format!(
                    "{plugin}/{command}"
                )));
            }
        }
        let Some(router) = self.services.router.upgrade() else {
            return Err(types::Error::TargetUnavailable(format!(
                "{plugin}/{command}"
            )));
        };
        let chain = self.chain.clone();
        self.waiting(|_| router.call(&chain, &plugin, &command, &input))
    }
}

fn wit_error(error: ServiceError) -> types::Error {
    match error {
        ServiceError::UnsupportedPath => types::Error::UnsupportedPath,
        ServiceError::InvalidPath(path) => types::Error::InvalidPath(path),
        ServiceError::NotFound(what) => types::Error::NotFound(what),
        ServiceError::Failed(message) => types::Error::Failed(message),
    }
}

/// A refused or failed network request as the plugin sees it. The web
/// access boundary's messages hold no raw system details.
fn network_error(error: CoreError) -> types::Error {
    match error.code.as_str() {
        "web_url_not_https"
        | "web_url_invalid"
        | "web_url_unsafe_host"
        | "web_host_not_allowed"
        | "web_redirect_limit_exceeded"
        | "web_redirect_invalid"
        | "web_header_invalid" => types::Error::NetworkRefused(error.message),
        _ => types::Error::Failed(error.message),
    }
}

/// The interfaces a plugin's component exports. Each is optional: a plugin
/// that doesn't export one registers nothing under it.
struct Exports {
    view: Option<view_export::Guest>,
    commands: Option<commands_export::Guest>,
    inbox: Option<inbox_export::Guest>,
    settings: Option<settings_export::Guest>,
    tools: Option<tools_export::Guest>,
}

/// One store of a running plugin. It keeps the host's epoch ticker
/// running, so its call deadline holds after the host is dropped.
pub(crate) struct PluginInstance {
    store: Store<HostState>,
    exports: Exports,
    call_deadline: Duration,
    _epochs: Arc<EpochTicker>,
}

/// Loads the exported interface whose indices `new` looks up: `None` when
/// the component doesn't export it, an error when it exports it with the
/// wrong types.
macro_rules! export {
    ($module:ident, $pre:expr, $store:expr, $instance:expr) => {
        match $module::GuestIndices::new($pre) {
            Ok(indices) => Some(indices.load(&mut *$store, $instance)?),
            Err(_) => None,
        }
    };
}

impl PluginInstance {
    /// Start `component` in a new store that may use what `grants` holds.
    pub(crate) fn start(
        engine: &Engine,
        epochs: &Arc<EpochTicker>,
        limits: Limits,
        component: &Component,
        grants: Grants,
        services: Services,
    ) -> Result<Self, PluginError> {
        let call_deadline = limits.call_deadline;
        let state = HostState {
            limiter: StoreLimiter::new(limits.memory_bytes),
            grants,
            services,
            chain: Vec::new(),
            clock: Clock::start(call_deadline),
        };
        let mut store = Store::new(engine, state);
        store.limiter(|state| &mut state.limiter);
        store.set_epoch_deadline(deadline_ticks(call_deadline));
        // The deadline counts the plugin's own work: when it passes, time
        // spent waiting for the network or another plugin buys more ticks.
        store.epoch_deadline_callback(|store| {
            let clock = store.data().clock;
            let used = clock.used();
            if used >= clock.deadline || clock.started.elapsed() >= MAX_CALL_TIME {
                Ok(UpdateDeadline::Interrupt)
            } else {
                Ok(UpdateDeadline::Continue(deadline_ticks(
                    clock.deadline - used,
                )))
            }
        });
        let mut linker = Linker::new(engine);
        Plugin::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)
            .map_err(|error| PluginError::Instantiate(format!("{error:?}")))?;
        // Imports the host doesn't provide, such as the WASI interfaces the
        // standard library pulls in for printing, trap when called.
        linker
            .define_unknown_imports_as_traps(component)
            .map_err(|error| PluginError::Instantiate(format!("{error:?}")))?;
        let exports = (|| -> wasmtime::Result<Exports> {
            let pre = linker.instantiate_pre(component)?;
            let instance = pre.instantiate(&mut store)?;
            let store = &mut store;
            Ok(Exports {
                view: export!(view_export, &pre, store, &instance),
                commands: export!(commands_export, &pre, store, &instance),
                inbox: export!(inbox_export, &pre, store, &instance),
                settings: export!(settings_export, &pre, store, &instance),
                tools: export!(tools_export, &pre, store, &instance),
            })
        })()
        .map_err(|error| failure(&store, &error, call_deadline, PluginError::Instantiate))?;
        Ok(Self {
            store,
            exports,
            call_deadline,
            _epochs: Arc::clone(epochs),
        })
    }

    /// Start a call made on behalf of `chain`, the plugins waiting for it:
    /// a fresh deadline, and no memory-cap refusal left from an earlier
    /// call, so a failure reports its own cause.
    pub(crate) fn begin(&mut self, chain: Vec<String>) {
        let deadline = self.call_deadline;
        let state = self.store.data_mut();
        state.chain = chain;
        state.clock = Clock::start(deadline);
        state.limiter.cap_hit = false;
        self.store.set_epoch_deadline(deadline_ticks(deadline));
    }

    /// Fails unless the plugin holds `capability`.
    fn needs(&self, capability: &'static str) -> Result<(), PluginError> {
        if self.store.data().holds(capability) {
            Ok(())
        } else {
            Err(PluginError::CapabilityNotDeclared { capability })
        }
    }

    fn failed(&self, error: &wasmtime::Error) -> PluginError {
        failure(&self.store, error, self.call_deadline, PluginError::Call)
    }

    fn view(&self) -> Result<view_export::Guest, PluginError> {
        self.needs("views")?;
        self.exports.view.clone().ok_or(PluginError::NotExported {
            interface: "noura:plugin/view",
        })
    }

    fn commands_export(&self) -> Result<commands_export::Guest, PluginError> {
        self.needs("commands")?;
        self.exports
            .commands
            .clone()
            .ok_or(PluginError::NotExported {
                interface: "noura:plugin/commands",
            })
    }

    fn inbox(&self) -> Result<inbox_export::Guest, PluginError> {
        self.needs("inbox-sections")?;
        self.exports.inbox.clone().ok_or(PluginError::NotExported {
            interface: "noura:plugin/inbox",
        })
    }

    fn settings(&self) -> Result<settings_export::Guest, PluginError> {
        self.needs("settings")?;
        self.exports
            .settings
            .clone()
            .ok_or(PluginError::NotExported {
                interface: "noura:plugin/settings",
            })
    }

    fn tools_export(&self) -> Result<tools_export::Guest, PluginError> {
        self.needs("tools")?;
        self.exports.tools.clone().ok_or(PluginError::NotExported {
            interface: "noura:plugin/tools",
        })
    }

    pub(crate) fn pages(&mut self) -> Result<Vec<Page>, PluginError> {
        let view = self.view()?;
        view.call_pages(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn render(&mut self, page: &str) -> Result<Vec<Node>, PluginError> {
        let view = self.view()?;
        view.call_render(&mut self.store, page)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn handle_event(&mut self, page: &str, event: &Event) -> Result<(), PluginError> {
        let view = self.view()?;
        view.call_handle_event(&mut self.store, page, event)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn commands(&mut self) -> Result<Vec<Command>, PluginError> {
        let commands = self.commands_export()?;
        commands
            .call_commands(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn run_command(
        &mut self,
        id: &str,
        input: &str,
    ) -> Result<Result<String, String>, PluginError> {
        let commands = self.commands_export()?;
        commands
            .call_run(&mut self.store, id, input)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn inbox_sections(&mut self) -> Result<Vec<InboxSection>, PluginError> {
        let inbox = self.inbox()?;
        inbox
            .call_sections(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn render_inbox_section(&mut self, section: &str) -> Result<Vec<Node>, PluginError> {
        let inbox = self.inbox()?;
        inbox
            .call_render_section(&mut self.store, section)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn handle_inbox_event(
        &mut self,
        section: &str,
        event: &Event,
    ) -> Result<(), PluginError> {
        let inbox = self.inbox()?;
        inbox
            .call_handle_section_event(&mut self.store, section, event)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn settings_page(&mut self) -> Result<Option<String>, PluginError> {
        let settings = self.settings()?;
        settings
            .call_page(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn render_settings(&mut self) -> Result<Vec<Node>, PluginError> {
        let settings = self.settings()?;
        settings
            .call_render_settings(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn handle_settings_event(&mut self, event: &Event) -> Result<(), PluginError> {
        let settings = self.settings()?;
        settings
            .call_handle_settings_event(&mut self.store, event)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn tools(&mut self) -> Result<Vec<Tool>, PluginError> {
        let tools = self.tools_export()?;
        tools
            .call_tools(&mut self.store)
            .map_err(|error| self.failed(&error))
    }

    pub(crate) fn call_tool(
        &mut self,
        name: &str,
        arguments: &str,
    ) -> Result<Result<String, String>, PluginError> {
        let tools = self.tools_export()?;
        tools
            .call_call_tool(&mut self.store, name, arguments)
            .map_err(|error| self.failed(&error))
    }
}

fn failure(
    store: &Store<HostState>,
    error: &wasmtime::Error,
    call_deadline: Duration,
    other: fn(String) -> PluginError,
) -> PluginError {
    match Failure::of(&store.data().limiter, error) {
        Failure::Deadline => PluginError::Deadline {
            deadline: call_deadline,
        },
        Failure::MemoryCap { cap_bytes } => PluginError::MemoryCap { cap_bytes },
        Failure::Other(message) => other(message),
    }
}
