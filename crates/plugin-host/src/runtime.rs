//! The Wasmtime setup every plugin runs on: the engine and its code
//! generator, each store's memory cap, the epoch ticker behind call
//! deadlines, and the WASI interfaces the host never links. Trial S7
//! measured these; the plugin host and the trial host share them.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use wasmtime::component::Component;
use wasmtime::{Config, Engine, ResourceLimiter, Trap};

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

/// What one plugin instance may use.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The most memory the instance's store may hold, in bytes: its linear
    /// memories plus its table elements.
    pub memory_bytes: usize,
    /// How long one call into the plugin may run, counting only the
    /// plugin's own work.
    pub call_deadline: Duration,
    /// How long one call may take in all, time spent waiting for the
    /// network or for another plugin included. This bounds a plugin that
    /// waits in a loop. The host checks it when the call deadline passes,
    /// so it fires no sooner than that.
    pub max_call_time: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            memory_bytes: 256 << 20,
            call_deadline: Duration::from_secs(1),
            max_call_time: Duration::from_secs(120),
        }
    }
}

/// An engine for components on `backend`, with epoch interruption for call
/// deadlines.
pub(crate) fn engine(backend: Backend) -> wasmtime::Result<Engine> {
    let mut config = wasmtime_config(backend)?;
    config.epoch_interruption(true);
    Engine::new(&config)
}

/// The Wasmtime configuration every guest Noura runs starts from: the
/// component model and the code generator for `backend`. Plugins add epoch
/// interruption for their call deadlines; the collaboration sandbox adds fuel
/// instead, so no clock decides its verdicts.
pub fn wasmtime_config(backend: Backend) -> wasmtime::Result<Config> {
    let mut config = Config::new();
    config.wasm_component_model(true);
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
            config.target(target)?;
        }
    }
    Ok(config)
}

/// The names of the interfaces `component` imports.
pub(crate) fn imports(component: &Component) -> Vec<String> {
    component
        .component_type()
        .imports(component.engine())
        .map(|(name, _)| name.to_string())
        .collect()
}

/// The first import of `component` that the host never provides: WASI's
/// filesystem, sockets or HTTP.
pub(crate) fn forbidden_import(component: &Component) -> Option<String> {
    imports(component).into_iter().find(|import| {
        FORBIDDEN_IMPORTS
            .iter()
            .any(|prefix| import.starts_with(prefix))
    })
}

/// Epoch ticks for a deadline, plus one because the first tick can come at
/// any moment after the call starts.
pub(crate) fn deadline_ticks(deadline: Duration) -> u64 {
    let ticks = deadline.as_nanos().div_ceil(EPOCH_TICK.as_nanos());
    u64::try_from(ticks).unwrap_or(u64::MAX - 1) + 1
}

/// Why a call into a plugin failed: its deadline passed, its store hit the
/// memory cap, or something else, described by `error`.
pub(crate) enum Failure {
    Deadline,
    MemoryCap { cap_bytes: usize },
    Other(String),
}

impl Failure {
    pub(crate) fn of(limiter: &StoreLimiter, error: &wasmtime::Error) -> Self {
        if error.downcast_ref::<Trap>() == Some(&Trap::Interrupt) {
            Self::Deadline
        } else if limiter.cap_hit {
            Self::MemoryCap {
                cap_bytes: limiter.cap_bytes,
            }
        } else {
            Self::Other(format!("{error:?}"))
        }
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
pub struct StoreLimiter {
    pub(crate) cap_bytes: usize,
    pub(crate) memory_bytes: usize,
    table_bytes: usize,
    /// The cap refused a growth since the current call or instantiation began.
    pub(crate) cap_hit: bool,
}

impl StoreLimiter {
    /// A limiter that lets a store hold at most `cap_bytes`.
    pub fn new(cap_bytes: usize) -> Self {
        Self {
            cap_bytes,
            memory_bytes: 0,
            table_bytes: 0,
            cap_hit: false,
        }
    }

    /// Whether the cap refused a growth since [`Self::clear_cap_hit`].
    pub fn cap_hit(&self) -> bool {
        self.cap_hit
    }

    /// Forgets earlier refusals, as before a new call.
    pub fn clear_cap_hit(&mut self) {
        self.cap_hit = false;
    }

    /// The linear memory the store holds, in bytes.
    pub fn memory_bytes(&self) -> usize {
        self.memory_bytes
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
pub(crate) struct EpochTicker {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl EpochTicker {
    pub(crate) fn start(engine: Engine) -> std::io::Result<Self> {
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("plugin-epochs".into())
            .spawn(move || {
                // A busy machine can wake this thread late, so each pass
                // catches up: a tick stands for 10 ms of real time, which
                // deadlines count on.
                let started = Instant::now();
                let mut ticks = 0;
                while let Err(RecvTimeoutError::Timeout) = stopped.recv_timeout(EPOCH_TICK) {
                    let due = started.elapsed().as_nanos() / EPOCH_TICK.as_nanos();
                    while ticks < due {
                        engine.increment_epoch();
                        ticks += 1;
                    }
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
