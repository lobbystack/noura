//! Trial S7: a small host for the `noura-trial:s7` world in `trial/wit`.
//!
//! Ticket 24 replaces this world with `noura:plugin@0.1.0`.

use std::path::{Path, PathBuf};
use std::sync::Arc;
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

/// Wasmtime stores one pointer per table element, so table elements count
/// toward the store's memory cap at this size.
const TABLE_ELEMENT_BYTES: usize = size_of::<usize>();

/// The most core instances, tables and linear memories one store may create.
/// The TypeScript hello plugin needs 11 core instances, 3 tables and 3
/// memories. Each memory reserves address space, and Wasmtime's defaults allow
/// 10,000 of each.
pub const MAX_INSTANCES: usize = 32;
/// See [`MAX_INSTANCES`].
pub const MAX_TABLES: usize = 8;
/// See [`MAX_INSTANCES`].
pub const MAX_MEMORIES: usize = 8;

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
    /// The most memory the instance's store may hold, in bytes: its linear
    /// memories plus its table elements.
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
        self.component
            .component_type()
            .imports(self.component.engine())
            .map(|(name, _)| name.to_string())
            .collect()
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
            epochs: Arc::new(epochs),
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
            .map_err(|error| failure(&store, error, call_deadline, TrialError::Instantiate))?;
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
            .map_err(|error| failure(&self.store, error, self.call_deadline, TrialError::Call))
    }

    /// Tells the plugin that the node with this id was pressed.
    pub fn handle_event(&mut self, node_id: &str) -> Result<(), TrialError> {
        self.start_call();
        self.bindings
            .call_handle_event(&mut self.store, node_id)
            .map_err(|error| failure(&self.store, error, self.call_deadline, TrialError::Call))
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

/// Caps the memory one store holds, its linear memories plus its table
/// elements, and counts what it holds.
///
/// A growth the limiter grants stays counted even if Wasmtime then fails it.
/// Wasmtime can report a failed growth that it never asked the limiter about,
/// so a refund could free budget the store still uses. Refusing growth past a
/// memory's or table's own maximum up front leaves allocation failures as the
/// only granted growth that fails, and counting those errs on the safe side.
struct StoreLimiter {
    cap_bytes: usize,
    memory_bytes: usize,
    table_bytes: usize,
    /// The cap refused a growth since the current call or instantiation began.
    cap_hit: bool,
}

impl StoreLimiter {
    fn new(cap_bytes: usize) -> Self {
        Self {
            cap_bytes,
            memory_bytes: 0,
            table_bytes: 0,
            cap_hit: false,
        }
    }

    /// Whether `growth` more bytes fit under the cap, noting a refusal.
    fn fits(&mut self, growth: usize) -> bool {
        let total = self
            .memory_bytes
            .saturating_add(self.table_bytes)
            .saturating_add(growth);
        if total > self.cap_bytes {
            self.cap_hit = true;
            return false;
        }
        true
    }
}

impl ResourceLimiter for StoreLimiter {
    fn memory_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if maximum.is_some_and(|maximum| desired > maximum) {
            return Ok(false);
        }
        let growth = desired.saturating_sub(current);
        if !self.fits(growth) {
            return Ok(false);
        }
        self.memory_bytes += growth;
        Ok(true)
    }

    fn table_growing(
        &mut self,
        current: usize,
        desired: usize,
        maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        if maximum.is_some_and(|maximum| desired > maximum) {
            return Ok(false);
        }
        let growth = desired
            .saturating_sub(current)
            .saturating_mul(TABLE_ELEMENT_BYTES);
        if !self.fits(growth) {
            return Ok(false);
        }
        self.table_bytes += growth;
        Ok(true)
    }

    fn instances(&self) -> usize {
        MAX_INSTANCES
    }

    fn tables(&self) -> usize {
        MAX_TABLES
    }

    fn memories(&self) -> usize {
        MAX_MEMORIES
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

#[cfg(test)]
mod tests {
    use super::*;

    const PAGE: usize = 64 << 10;

    #[test]
    fn a_failed_growth_wasmtime_never_asked_about_refunds_nothing() {
        let mut limiter = StoreLimiter::new(4 * PAGE);

        for pages in 1..=4 {
            assert!(
                limiter
                    .memory_growing((pages - 1) * PAGE, pages * PAGE, None)
                    .expect("growth")
            );
            // Wasmtime reports growth past a memory type's own limits this
            // way, without calling `memory_growing` first.
            limiter
                .memory_grow_failed(wasmtime::format_err!("exceeds the memory type's limits"))
                .expect("failure noted");
        }

        assert_eq!(limiter.memory_bytes, 4 * PAGE);
        assert!(
            !limiter
                .memory_growing(4 * PAGE, 5 * PAGE, None)
                .expect("growth")
        );
    }

    #[test]
    fn growth_past_a_declared_maximum_is_refused_without_counting() {
        let mut limiter = StoreLimiter::new(16 * PAGE);

        assert!(
            !limiter
                .memory_growing(PAGE, 3 * PAGE, Some(2 * PAGE))
                .expect("growth")
        );
        assert!(!limiter.table_growing(10, 20, Some(15)).expect("growth"));

        assert_eq!((limiter.memory_bytes, limiter.table_bytes), (0, 0));
        assert!(!limiter.cap_hit);
    }

    #[test]
    fn table_elements_and_linear_memory_share_the_cap() {
        let mut limiter = StoreLimiter::new(2 * PAGE);
        let elements_per_page = PAGE / TABLE_ELEMENT_BYTES;

        assert!(limiter.memory_growing(0, PAGE, None).expect("growth"));
        assert!(
            limiter
                .table_growing(0, elements_per_page, None)
                .expect("growth")
        );
        assert!(
            !limiter
                .table_growing(elements_per_page, elements_per_page + 1, None)
                .expect("growth")
        );

        assert!(limiter.cap_hit);
        assert_eq!(limiter.memory_bytes + limiter.table_bytes, 2 * PAGE);
    }
}
