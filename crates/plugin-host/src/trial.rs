//! Trial S7: a small host for the `noura-trial:s7` world in `trial/wit`.
//!
//! [`PluginHost`](crate::PluginHost) runs `noura:plugin@0.1.0` on the same
//! runtime. This host stays until the TypeScript author kit (ticket 34)
//! moves the TypeScript hello plugin onto `noura:plugin`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use wasmtime::component::{Component, Linker};
use wasmtime::{Engine, Store};

use crate::runtime::{self, EpochTicker, Failure, StoreLimiter, deadline_ticks};
pub use crate::runtime::{Backend, Limits, MAX_INSTANCES, MAX_MEMORIES, MAX_TABLES};

mod bindings {
    wasmtime::component::bindgen!({
        path: "trial/wit",
        world: "plugin",
        additional_derives: [PartialEq, Eq],
    });
}

use bindings::Plugin;
pub use bindings::noura_trial::s7::ui::{Node, NodeKind};

/// Something the trial host refused or a plugin failed to do.
#[derive(Debug, thiserror::Error)]
pub enum TrialError {
    #[error("the host could not start: {0}")]
    Engine(String),
    #[error("the plugin is not a valid component: {0}")]
    Compile(String),
    #[error("the plugin imports {import}, which the host doesn't provide")]
    ForbiddenImport { import: String },
    #[error("the compiled cache can't be written or loaded: {0}")]
    Cache(String),
    #[error("the plugin failed to start: {0}")]
    Instantiate(String),
    #[error("the plugin needs more than its {cap_bytes}-byte memory cap")]
    MemoryCap { cap_bytes: usize },
    #[error("the plugin ran past its {deadline:?} deadline")]
    Deadline { deadline: Duration },
    #[error("the plugin failed: {0}")]
    Call(String),
}

/// Runs trial plugins. Each instance gets its own store.
pub struct TrialHost {
    engine: Engine,
    limits: Limits,
    epochs: Arc<EpochTicker>,
}

/// A component this host compiled.
pub struct CompiledPlugin {
    component: Component,
}

impl CompiledPlugin {
    /// The names of the interfaces the component imports. The host refuses
    /// WASI files, sockets and HTTP, and links every other import as a stub
    /// that traps when called.
    pub fn imports(&self) -> Vec<String> {
        runtime::imports(&self.component)
    }
}

/// Compiled code that this host wrote to disk. Only
/// [`TrialHost::compile_to_cache`] creates one, so the host only loads paths
/// it wrote compiled code to. It doesn't check that the file still holds that
/// code.
pub struct CachedPlugin {
    path: PathBuf,
}

/// One running plugin with its own store. It keeps the host's epoch ticker
/// running, so its call deadline holds after the host is dropped.
pub struct PluginInstance {
    store: Store<StoreLimiter>,
    bindings: Plugin,
    call_deadline: Duration,
    _epochs: Arc<EpochTicker>,
}

impl TrialHost {
    pub fn new(backend: Backend) -> Result<Self, TrialError> {
        Self::with_limits(backend, Limits::default())
    }

    pub fn with_limits(backend: Backend, limits: Limits) -> Result<Self, TrialError> {
        let engine =
            runtime::engine(backend).map_err(|error| TrialError::Engine(error.to_string()))?;
        let epochs = EpochTicker::start(engine.clone())
            .map_err(|error| TrialError::Engine(error.to_string()))?;
        Ok(Self {
            engine,
            limits,
            epochs: Arc::new(epochs),
        })
    }

    /// Compiles a component from its bytes.
    pub fn compile(&self, component: &[u8]) -> Result<CompiledPlugin, TrialError> {
        let component = Component::new(&self.engine, component)
            .map_err(|error| TrialError::Compile(format!("{error:#}")))?;
        if let Some(import) = runtime::forbidden_import(&component) {
            return Err(TrialError::ForbiddenImport { import });
        }
        Ok(CompiledPlugin { component })
    }

    /// Compiles a component and writes the compiled code to `path`. The host
    /// later runs whatever that file holds, so `path` must sit where only this
    /// process writes, such as a private temporary directory.
    pub fn compile_to_cache(
        &self,
        component: &[u8],
        path: &Path,
    ) -> Result<CachedPlugin, TrialError> {
        let compiled = self.compile(component)?;
        let code = compiled
            .component
            .serialize()
            .map_err(|error| TrialError::Cache(format!("{error:#}")))?;
        std::fs::write(path, code).map_err(|error| TrialError::Cache(error.to_string()))?;
        Ok(CachedPlugin {
            path: path.to_path_buf(),
        })
    }

    /// Loads compiled code that this host wrote. Wasmtime refuses code from
    /// another Wasmtime version or engine configuration.
    pub fn load_cached(&self, cached: &CachedPlugin) -> Result<CompiledPlugin, TrialError> {
        // SAFETY: Wasmtime runs deserialized code without verifying it.
        // `CachedPlugin` has no constructor besides `compile_to_cache`, so the
        // path names a file this host wrote compiled code to. Nothing here
        // stops another process from replacing that file before this load;
        // `compile_to_cache` requires a path only this process writes.
        let component = unsafe { Component::deserialize_file(&self.engine, &cached.path) }
            .map_err(|error| TrialError::Cache(format!("{error:#}")))?;
        Ok(CompiledPlugin { component })
    }

    /// Starts a plugin in a new store.
    pub fn instantiate(&self, compiled: &CompiledPlugin) -> Result<PluginInstance, TrialError> {
        let mut store = Store::new(&self.engine, StoreLimiter::new(self.limits.memory_bytes));
        store.limiter(|limiter| limiter);
        let call_deadline = self.limits.call_deadline;
        store.set_epoch_deadline(deadline_ticks(call_deadline));
        let mut linker = Linker::new(&self.engine);
        linker
            .define_unknown_imports_as_traps(&compiled.component)
            .map_err(|error| TrialError::Instantiate(format!("{error:?}")))?;
        let bindings = Plugin::instantiate(&mut store, &compiled.component, &linker)
            .map_err(|error| failure(&store, &error, call_deadline, TrialError::Instantiate))?;
        Ok(PluginInstance {
            store,
            bindings,
            call_deadline,
            _epochs: Arc::clone(&self.epochs),
        })
    }
}

impl PluginInstance {
    /// Asks the plugin for its UI nodes.
    pub fn render(&mut self) -> Result<Vec<Node>, TrialError> {
        self.start_call();
        self.bindings
            .call_render(&mut self.store)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, TrialError::Call))
    }

    /// Tells the plugin that the node with this id was pressed.
    pub fn handle_event(&mut self, node_id: &str) -> Result<(), TrialError> {
        self.start_call();
        self.bindings
            .call_handle_event(&mut self.store, node_id)
            .map_err(|error| failure(&self.store, &error, self.call_deadline, TrialError::Call))
    }

    /// The linear memory this instance's store holds, in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.store.data().memory_bytes
    }

    /// Gives the next call a fresh deadline, and forgets a growth the cap
    /// refused in an earlier call, so a later failure reports its own cause.
    fn start_call(&mut self) {
        self.store
            .set_epoch_deadline(deadline_ticks(self.call_deadline));
        self.store.data_mut().cap_hit = false;
    }
}

fn failure(
    store: &Store<StoreLimiter>,
    error: &wasmtime::Error,
    call_deadline: Duration,
    other: fn(String) -> TrialError,
) -> TrialError {
    match Failure::of(store.data(), error) {
        Failure::Deadline => TrialError::Deadline {
            deadline: call_deadline,
        },
        Failure::MemoryCap { cap_bytes } => TrialError::MemoryCap { cap_bytes },
        Failure::Other(message) => other(message),
    }
}
