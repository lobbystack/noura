//! A running plugin: its own store, the capability interfaces linked into
//! it, and calls into its exports.

use std::sync::Arc;
use std::time::Duration;

use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Engine, Store};

use crate::{
    Capability, EntryKind, Limits, Listed, ServiceError, WorkspacePath, WorkspaceReads,
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
pub use bindings::exports::noura::plugin::view::{Event, Page};
pub use bindings::noura::plugin::ui::{Node, NodeKind};
use bindings::noura::plugin::{types, workspace};

/// Why a call into a running plugin failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PluginError {
    /// The manifest doesn't declare the capability the call needs, so the
    /// host made no call.
    #[error("the plugin's manifest doesn't declare the {capability} capability")]
    CapabilityNotDeclared { capability: &'static str },
    #[error("the plugin failed to start: {0}")]
    Instantiate(String),
    #[error("the plugin needs more than its {cap_bytes}-byte memory cap")]
    MemoryCap { cap_bytes: usize },
    #[error("the plugin ran past its {deadline:?} deadline")]
    Deadline { deadline: Duration },
    #[error("the plugin failed: {0}")]
    Call(String),
}

/// What a plugin's store holds: its memory cap, the capabilities it may
/// use and the services they call.
pub(crate) struct HostState {
    limiter: StoreLimiter,
    granted: Vec<Capability>,
    workspace: Option<Arc<dyn WorkspaceReads>>,
}

impl HostState {
    fn holds(&self, name: &str) -> bool {
        self.granted
            .iter()
            .any(|capability| capability.name() == name)
    }

    /// The workspace service, when the plugin holds `workspace-read`.
    fn workspace(&self) -> Result<&dyn WorkspaceReads, types::Error> {
        if !self.holds("workspace-read") {
            return Err(types::Error::CapabilityNotDeclared(
                "workspace-read".to_owned(),
            ));
        }
        self.workspace
            .as_deref()
            .ok_or_else(|| types::Error::Failed("No workspace is open.".to_owned()))
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

fn wit_error(error: ServiceError) -> types::Error {
    match error {
        ServiceError::UnsupportedPath => types::Error::UnsupportedPath,
        ServiceError::InvalidPath(path) => types::Error::InvalidPath(path),
        ServiceError::NotFound(what) => types::Error::NotFound(what),
        ServiceError::Failed(message) => types::Error::Failed(message),
    }
}

/// One running plugin with its own store. It keeps the host's epoch ticker
/// running, so its call deadline holds after the host is dropped.
pub struct PluginInstance {
    store: Store<HostState>,
    bindings: Plugin,
    call_deadline: Duration,
    _epochs: Arc<EpochTicker>,
}

impl PluginInstance {
    /// Start `component` in a new store that may use `granted`.
    pub(crate) fn start(
        engine: &Engine,
        epochs: &Arc<EpochTicker>,
        limits: Limits,
        component: &Component,
        granted: Vec<Capability>,
        workspace: Option<Arc<dyn WorkspaceReads>>,
    ) -> Result<Self, PluginError> {
        let state = HostState {
            limiter: StoreLimiter::new(limits.memory_bytes),
            granted,
            workspace,
        };
        let mut store = Store::new(engine, state);
        store.limiter(|state| &mut state.limiter);
        let call_deadline = limits.call_deadline;
        store.set_epoch_deadline(deadline_ticks(call_deadline));
        let mut linker = Linker::new(engine);
        Plugin::add_to_linker::<HostState, HasSelf<HostState>>(&mut linker, |state| state)
            .map_err(|error| PluginError::Instantiate(format!("{error:?}")))?;
        // Imports the host doesn't provide, such as the WASI interfaces the
        // standard library pulls in for printing, trap when called.
        linker
            .define_unknown_imports_as_traps(component)
            .map_err(|error| PluginError::Instantiate(format!("{error:?}")))?;
        let bindings = Plugin::instantiate(&mut store, component, &linker)
            .map_err(|error| failure(&store, &error, call_deadline, PluginError::Instantiate))?;
        Ok(Self {
            store,
            bindings,
            call_deadline,
            _epochs: Arc::clone(epochs),
        })
    }

    /// The pages the plugin adds to the rail. Needs the `views`
    /// capability.
    pub fn pages(&mut self) -> Result<Vec<Page>, PluginError> {
        self.views()?;
        self.start_call();
        self.bindings
            .noura_plugin_view()
            .call_pages(&mut self.store)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, PluginError::Call))
    }

    /// The nodes of a page's view, as the plugin returned them. See
    /// [`crate::View::from_nodes`] for the checks before drawing them.
    pub fn render(&mut self, page: &str) -> Result<Vec<Node>, PluginError> {
        self.views()?;
        self.start_call();
        self.bindings
            .noura_plugin_view()
            .call_render(&mut self.store, page)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, PluginError::Call))
    }

    /// Tell the plugin about `event` in a page's view.
    pub fn handle_event(&mut self, page: &str, event: &Event) -> Result<(), PluginError> {
        self.views()?;
        self.start_call();
        self.bindings
            .noura_plugin_view()
            .call_handle_event(&mut self.store, page, event)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, PluginError::Call))
    }

    fn views(&self) -> Result<(), PluginError> {
        if self.store.data().holds("views") {
            Ok(())
        } else {
            Err(PluginError::CapabilityNotDeclared {
                capability: "views",
            })
        }
    }

    /// Gives the next call a fresh deadline, and forgets a growth the cap
    /// refused in an earlier call, so a later failure reports its own cause.
    fn start_call(&mut self) {
        self.store
            .set_epoch_deadline(deadline_ticks(self.call_deadline));
        self.store.data_mut().limiter.cap_hit = false;
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
