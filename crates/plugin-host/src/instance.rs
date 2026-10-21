//! A running plugin: its own store, the capability interfaces linked into
//! it, and calls into its exports.

use std::sync::Arc;
use std::time::Duration;

use wasmtime::component::{Component, HasSelf, Linker};
use wasmtime::{Engine, Store};

use crate::{
    Capability, EntryKind, Limits, Listed, NewObject, ObjectChanges, ObjectRecord, ObjectTypes,
    PropertyProblem, ServiceError, TypeError, WorkspaceObjects, WorkspacePath, WorkspaceReads,
    object_types::{next_instance, type_of_id},
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
pub use bindings::exports::noura::plugin::object_types::ObjectType;
pub use bindings::exports::noura::plugin::view::Page;
use bindings::exports::noura::plugin::{object_types as types_export, view as view_export};
pub use bindings::noura::plugin::ui::{Event, Node, NodeKind};
use bindings::noura::plugin::{objects, types, workspace};

/// The capability behind object types, their panels and object writes.
const OBJECT_TYPES: &str = "object-types";

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
    /// The plugin's component doesn't export the interface the call needs.
    #[error("the plugin doesn't export {interface}")]
    NotExported { interface: &'static str },
}

/// The services a plugin's capability interfaces call. The app provides
/// them; the host checks each call against the plugin's capabilities
/// first.
#[derive(Clone, Default)]
pub struct Services {
    /// Reads the open workspace, for `workspace-read`.
    pub workspace: Option<Arc<dyn WorkspaceReads>>,
    /// Writes objects of the plugin's own types, for `object-types`.
    pub objects: Option<Arc<dyn WorkspaceObjects>>,
    /// The object types the workspace's running plugins hold.
    pub types: ObjectTypes,
}

/// What a plugin's store holds: its memory cap, the capabilities it may
/// use and the services they call.
pub(crate) struct HostState {
    limiter: StoreLimiter,
    granted: Vec<Capability>,
    services: Services,
    /// The plugin's ID, which owns the types it registers.
    plugin: String,
    /// This running instance, whose stop releases its types.
    instance: u64,
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
        self.services
            .workspace
            .as_deref()
            .ok_or_else(|| types::Error::Failed("No workspace is open.".to_owned()))
    }

    /// The object service, when the plugin holds `object-types` and
    /// registered `object_type`. A type the plugin doesn't hold, a note
    /// for one, gets the capability error: the capability covers the
    /// plugin's own types only.
    fn objects(&self, object_type: &str) -> Result<&dyn WorkspaceObjects, types::Error> {
        let refused = || types::Error::CapabilityNotDeclared(OBJECT_TYPES.to_owned());
        if !self.holds(OBJECT_TYPES) || !self.services.types.held_by(object_type, self.instance) {
            return Err(refused());
        }
        self.services
            .objects
            .as_deref()
            .ok_or_else(|| types::Error::Failed("No workspace is open.".to_owned()))
    }

    /// The object service for the object with the stable ID `id`, whose
    /// type is its prefix.
    fn objects_for(&self, id: &str) -> Result<&dyn WorkspaceObjects, types::Error> {
        let object_type = type_of_id(id).unwrap_or_default();
        self.objects(object_type)
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

impl objects::Host for HostState {
    fn create(&mut self, object: objects::NewObject) -> Result<objects::Object, types::Error> {
        let service = self.objects(&object.object_type)?;
        let path = object
            .path
            .as_deref()
            .map(WorkspacePath::new)
            .transpose()
            .map_err(wit_error)?;
        let new = NewObject {
            object_type: object.object_type,
            title: object.title,
            path,
            body: object.body,
            properties: json_object(&object.properties)?,
        };
        service.create(new).map(wit_object).map_err(wit_error)
    }

    fn get(&mut self, id: String) -> Result<objects::Object, types::Error> {
        self.objects_for(&id)?
            .get(&id)
            .map(wit_object)
            .map_err(wit_error)
    }

    fn list(&mut self, object_type: String) -> Result<Vec<objects::Object>, types::Error> {
        let listed = self
            .objects(&object_type)?
            .list(&object_type)
            .map_err(wit_error)?;
        Ok(listed.into_iter().map(wit_object).collect())
    }

    fn update(
        &mut self,
        id: String,
        expected_revision: String,
        changes: objects::Changes,
    ) -> Result<objects::Object, types::Error> {
        let service = self.objects_for(&id)?;
        let changes = ObjectChanges {
            body: changes.body,
            properties: json_object(&changes.properties)?,
            remove: changes.remove,
        };
        service
            .update(&id, &expected_revision, changes)
            .map(wit_object)
            .map_err(wit_error)
    }

    fn move_to(
        &mut self,
        id: String,
        expected_revision: String,
        path: String,
    ) -> Result<objects::Object, types::Error> {
        let service = self.objects_for(&id)?;
        let path = WorkspacePath::new(&path).map_err(wit_error)?;
        service
            .move_to(&id, &expected_revision, &path)
            .map(wit_object)
            .map_err(wit_error)
    }

    fn delete(&mut self, id: String, expected_revision: String) -> Result<(), types::Error> {
        self.objects_for(&id)?
            .delete(&id, &expected_revision)
            .map_err(wit_error)
    }
}

/// The JSON object `text` holds, or why it isn't one.
fn json_object(text: &str) -> Result<serde_json::Map<String, serde_json::Value>, types::Error> {
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    match serde_json::from_str(text) {
        Ok(serde_json::Value::Object(map)) => Ok(map),
        _ => Err(types::Error::Failed(
            "Properties must be a JSON object.".to_owned(),
        )),
    }
}

fn wit_object(object: ObjectRecord) -> objects::Object {
    objects::Object {
        id: object.id,
        object_type: object.object_type,
        title: object.title,
        path: object.path.as_str().to_owned(),
        body: object.body,
        properties: object.properties,
        revision: object.revision,
        problem: object.problem.map(wit_problem),
    }
}

fn wit_problem(problem: PropertyProblem) -> types::PropertyError {
    types::PropertyError {
        property: problem.property,
        code: problem.code,
    }
}

fn wit_error(error: ServiceError) -> types::Error {
    match error {
        ServiceError::UnsupportedPath => types::Error::UnsupportedPath,
        ServiceError::InvalidPath(path) => types::Error::InvalidPath(path),
        ServiceError::NotFound(what) => types::Error::NotFound(what),
        ServiceError::Failed(message) => types::Error::Failed(message),
        ServiceError::RevisionConflict(id) => types::Error::RevisionConflict(id),
        ServiceError::IdentityConflict(id) => types::Error::IdentityConflict(id),
        ServiceError::InvalidProperty(problem) => {
            types::Error::InvalidProperty(wit_problem(problem))
        }
        ServiceError::AlreadyExists(path) => types::Error::AlreadyExists(path),
    }
}

/// The interfaces a plugin's component exports. Each is optional: a
/// plugin that doesn't export one adds nothing under it.
struct Exports {
    view: Option<view_export::Guest>,
    object_types: Option<types_export::Guest>,
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

/// What registering one of a plugin's types gave: the type's name and
/// whether it is the plugin's now.
pub type Registration = (String, Result<(), TypeError>);

/// One running plugin with its own store. It keeps the host's epoch ticker
/// running, so its call deadline holds after the host is dropped. Dropping
/// it releases the object types it registered.
pub struct PluginInstance {
    store: Store<HostState>,
    exports: Exports,
    call_deadline: Duration,
    _epochs: Arc<EpochTicker>,
}

impl PluginInstance {
    /// Start `component` in a new store that may use `granted`, as the
    /// plugin `plugin`.
    pub(crate) fn start(
        engine: &Engine,
        epochs: &Arc<EpochTicker>,
        limits: Limits,
        component: &Component,
        plugin: &str,
        granted: Vec<Capability>,
        services: Services,
    ) -> Result<Self, PluginError> {
        let state = HostState {
            limiter: StoreLimiter::new(limits.memory_bytes),
            granted,
            services,
            plugin: plugin.to_owned(),
            instance: next_instance(),
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
        let exports = (|| -> wasmtime::Result<Exports> {
            let pre = linker.instantiate_pre(component)?;
            let instance = pre.instantiate(&mut store)?;
            let store = &mut store;
            Ok(Exports {
                view: export!(view_export, &pre, store, &instance),
                object_types: export!(types_export, &pre, store, &instance),
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

    /// The pages the plugin adds to the rail. Needs the `views`
    /// capability.
    pub fn pages(&mut self) -> Result<Vec<Page>, PluginError> {
        let view = self.view()?;
        self.start_call();
        view.call_pages(&mut self.store)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, PluginError::Call))
    }

    /// The nodes of a page's view, as the plugin returned them. See
    /// [`crate::View::from_nodes`] for the checks before drawing them.
    pub fn render(&mut self, page: &str) -> Result<Vec<Node>, PluginError> {
        let view = self.view()?;
        self.start_call();
        view.call_render(&mut self.store, page)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, PluginError::Call))
    }

    /// Tell the plugin about `event` in a page's view.
    pub fn handle_event(&mut self, page: &str, event: &Event) -> Result<(), PluginError> {
        let view = self.view()?;
        self.start_call();
        view.call_handle_event(&mut self.store, page, event)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, PluginError::Call))
    }

    /// Ask the plugin for its object types and register each one in the
    /// workspace's registry for this instance. Needs the `object-types`
    /// capability. Each type comes back with whether it registered: the
    /// kernel's own types, a type another running plugin holds and an
    /// invalid schema each fail with a structured error, and the other
    /// types register all the same.
    pub fn register_object_types(&mut self) -> Result<Vec<Registration>, PluginError> {
        let export = self.object_types_export()?;
        self.start_call();
        let declared = export
            .call_types(&mut self.store)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, PluginError::Call))?;
        let state = self.store.data();
        Ok(declared
            .into_iter()
            .map(|declared| {
                let registered = state.services.types.register(
                    &state.plugin,
                    state.instance,
                    &declared.name,
                    &declared.schema,
                );
                (declared.name, registered)
            })
            .collect())
    }

    /// The types this instance registered, sorted.
    pub fn object_types(&self) -> Vec<String> {
        let state = self.store.data();
        state
            .services
            .types
            .types_of(&state.plugin)
            .into_iter()
            .filter(|object_type| state.services.types.held_by(object_type, state.instance))
            .collect()
    }

    /// The nodes of the properties panel of the object `object`, as the
    /// plugin returned them. Needs the `object-types` capability.
    pub fn render_panel(&mut self, object: &str) -> Result<Vec<Node>, PluginError> {
        let export = self.object_types_export()?;
        self.start_call();
        export
            .call_render_panel(&mut self.store, object)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, PluginError::Call))
    }

    /// Tell the plugin about `event` in the properties panel of the object
    /// `object`.
    pub fn handle_panel_event(&mut self, object: &str, event: &Event) -> Result<(), PluginError> {
        let export = self.object_types_export()?;
        self.start_call();
        export
            .call_handle_panel_event(&mut self.store, object, event)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, PluginError::Call))
    }

    fn needs(&self, capability: &'static str) -> Result<(), PluginError> {
        if self.store.data().holds(capability) {
            Ok(())
        } else {
            Err(PluginError::CapabilityNotDeclared { capability })
        }
    }

    fn view(&self) -> Result<view_export::Guest, PluginError> {
        self.needs("views")?;
        self.exports.view.clone().ok_or(PluginError::NotExported {
            interface: "noura:plugin/view",
        })
    }

    fn object_types_export(&self) -> Result<types_export::Guest, PluginError> {
        self.needs(OBJECT_TYPES)?;
        self.exports
            .object_types
            .clone()
            .ok_or(PluginError::NotExported {
                interface: "noura:plugin/object-types",
            })
    }

    /// Gives the next call a fresh deadline, and forgets a growth the cap
    /// refused in an earlier call, so a later failure reports its own cause.
    fn start_call(&mut self) {
        self.store
            .set_epoch_deadline(deadline_ticks(self.call_deadline));
        self.store.data_mut().limiter.cap_hit = false;
    }
}

impl Drop for PluginInstance {
    fn drop(&mut self) {
        let state = self.store.data();
        state.services.types.release(state.instance);
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
