//! Trial S7: a small host for the `noura-trial:s7` world in `trial/wit`.
//!
//! Ticket 24 replaces this world with `noura:plugin@0.1.0`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::Duration;

use wasmtime::component::{Component, Linker};
use wasmtime::{Config, Engine, ResourceLimiter, Store, Trap};

mod bindings {
    wasmtime::component::bindgen!({
        path: "trial/wit",
        world: "plugin",
        additional_derives: [PartialEq, Eq],
    });
}

use bindings::Plugin;
pub use bindings::noura_trial::s7::ui::{Node, NodeKind};

/// Tables hold function references. This bounds them the way the memory cap
/// bounds linear memory.
const MAX_TABLE_ELEMENTS: usize = 1 << 20;

/// How often the host advances the engine's epoch, which bounds how precisely
/// a call deadline fires.
const EPOCH_TICK: Duration = Duration::from_millis(10);

/// WASI interfaces the host never links. Plugins reach files and the network
/// only through the host's own capability interfaces.
const FORBIDDEN_IMPORTS: [&str; 3] = ["wasi:filesystem/", "wasi:sockets/", "wasi:http/"];

/// The code generator that turns a component into code the host runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Backend {
    /// Native code from Cranelift.
    Cranelift,
    /// Bytecode for Wasmtime's Pulley interpreter, which Cranelift also
    /// compiles. The host never runs native code it generated.
    #[cfg(feature = "pulley")]
    Pulley,
}

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

/// What one plugin instance may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most linear memory the instance's store may hold, in bytes.
    pub memory_bytes: usize,
    /// How long one call into the plugin may run.
    pub call_deadline: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 256 << 20,
            call_deadline: Duration::from_secs(1),
        }
    }
}

/// Runs trial plugins. Each instance gets its own store.
pub struct TrialHost {
    engine: Engine,
    limits: Limits,
    _epochs: EpochTicker,
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
        self.component
            .component_type()
            .imports(self.component.engine())
            .map(|(name, _)| name.to_string())
            .collect()
    }
}

/// Compiled code that this host wrote to disk. Only
/// [`TrialHost::compile_to_cache`] creates one, so the host never loads
/// compiled code from anywhere else.
pub struct CachedPlugin {
    path: PathBuf,
}

/// One running plugin with its own store.
pub struct PluginInstance {
    store: Store<StoreLimiter>,
    bindings: Plugin,
    call_deadline: Duration,
}

impl TrialHost {
    pub fn new(backend: Backend) -> Result<Self, TrialError> {
        Self::with_limits(backend, Limits::default())
    }

    pub fn with_limits(backend: Backend, limits: Limits) -> Result<Self, TrialError> {
        let mut config = Config::new();
        config.wasm_component_model(true);
        config.epoch_interruption(true);
        match backend {
            Backend::Cranelift => {}
            #[cfg(feature = "pulley")]
            Backend::Pulley => {
                let target = match (
                    cfg!(target_pointer_width = "64"),
                    cfg!(target_endian = "big"),
                ) {
                    (true, false) => "pulley64",
                    (true, true) => "pulley64be",
                    (false, false) => "pulley32",
                    (false, true) => "pulley32be",
                };
                config
                    .target(target)
                    .map_err(|error| TrialError::Engine(error.to_string()))?;
            }
        }
        let engine = Engine::new(&config).map_err(|error| TrialError::Engine(error.to_string()))?;
        let epochs = EpochTicker::start(engine.clone())
            .map_err(|error| TrialError::Engine(error.to_string()))?;
        Ok(Self {
            engine,
            limits,
            _epochs: epochs,
        })
    }

    /// Compiles a component from its bytes.
    pub fn compile(&self, component: &[u8]) -> Result<CompiledPlugin, TrialError> {
        let component = Component::new(&self.engine, component)
            .map_err(|error| TrialError::Compile(format!("{error:#}")))?;
        let compiled = CompiledPlugin { component };
        if let Some(import) = compiled.imports().into_iter().find(|import| {
            FORBIDDEN_IMPORTS
                .iter()
                .any(|prefix| import.starts_with(prefix))
        }) {
            return Err(TrialError::ForbiddenImport { import });
        }
        Ok(compiled)
    }

    /// Compiles a component and writes the compiled code to `path`.
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
        // SAFETY: Wasmtime runs deserialized code without verifying it. The
        // file came from `compile_to_cache`, and `CachedPlugin` has no other
        // constructor, so it holds only output this host compiled.
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
            .map_err(|error| failure(&store, error, call_deadline, TrialError::Instantiate))?;
        Ok(PluginInstance {
            store,
            bindings,
            call_deadline,
        })
    }
}

impl PluginInstance {
    /// Asks the plugin for its UI nodes.
    pub fn render(&mut self) -> Result<Vec<Node>, TrialError> {
        self.store
            .set_epoch_deadline(deadline_ticks(self.call_deadline));
        self.bindings
            .call_render(&mut self.store)
            .map_err(|error| failure(&self.store, error, self.call_deadline, TrialError::Call))
    }

    /// Tells the plugin that the node with this id was pressed.
    pub fn handle_event(&mut self, node_id: &str) -> Result<(), TrialError> {
        self.store
            .set_epoch_deadline(deadline_ticks(self.call_deadline));
        self.bindings
            .call_handle_event(&mut self.store, node_id)
            .map_err(|error| failure(&self.store, error, self.call_deadline, TrialError::Call))
    }

    /// The linear memory this instance's store holds, in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.store.data().memory_bytes
    }
}

/// Epoch ticks for a deadline, plus one because the first tick can come at
/// any moment after the call starts.
fn deadline_ticks(deadline: Duration) -> u64 {
    let ticks = deadline.as_nanos().div_ceil(EPOCH_TICK.as_nanos());
    u64::try_from(ticks).unwrap_or(u64::MAX - 1) + 1
}

fn failure(
    store: &Store<StoreLimiter>,
    error: wasmtime::Error,
    call_deadline: Duration,
    other: fn(String) -> TrialError,
) -> TrialError {
    let limiter = store.data();
    if error.downcast_ref::<Trap>() == Some(&Trap::Interrupt) {
        TrialError::Deadline {
            deadline: call_deadline,
        }
    } else if limiter.cap_hit {
        TrialError::MemoryCap {
            cap_bytes: limiter.cap_bytes,
        }
    } else {
        other(format!("{error:?}"))
    }
}

/// Caps the total linear memory of one store and counts what it holds.
struct StoreLimiter {
    cap_bytes: usize,
    memory_bytes: usize,
    last_growth: usize,
    cap_hit: bool,
}

impl StoreLimiter {
    fn new(cap_bytes: usize) -> Self {
        Self {
            cap_bytes,
            memory_bytes: 0,
            last_growth: 0,
            cap_hit: false,
        }
    }
}

impl ResourceLimiter for StoreLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        let growth = desired.saturating_sub(current);
        let total = self.memory_bytes.saturating_add(growth);
        if total > self.cap_bytes {
            self.cap_hit = true;
            return Ok(false);
        }
        self.memory_bytes = total;
        self.last_growth = growth;
        Ok(true)
    }

    fn memory_grow_failed(&mut self, _error: wasmtime::Error) -> wasmtime::Result<()> {
        self.memory_bytes -= self.last_growth;
        self.last_growth = 0;
        Ok(())
    }

    fn table_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= MAX_TABLE_ELEMENTS)
    }
}

/// Advances the engine's epoch every tick until it is dropped.
struct EpochTicker {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl EpochTicker {
    fn start(engine: Engine) -> std::io::Result<Self> {
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("plugin-epochs".into())
            .spawn(move || {
                while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(EPOCH_TICK) {
                    engine.increment_epoch();
                }
            })?;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

impl Drop for EpochTicker {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
