//! The collaboration sandbox (sync-v2.md §22): the Wasmtime instances every
//! remote `text_snapshot` and `text_update` passes through before it reaches
//! the editor or the file, so a hostile collaborator can't crash or freeze
//! Noura.
//!
//! The sandbox runs one guest module, built from
//! `crates/collab-sandbox-guest` and pinned by the BLAKE3 hash that
//! `sync-v2-collaboration.json` records, on `plugin-host`'s Wasmtime
//! configuration with fuel instead of epoch deadlines. It works in two
//! stages:
//!
//! 1. [`Sandbox::verdict`] judges a record by its bytes and two numbers from
//!    the verified log, in a fresh instance with a fuel budget of 1,000,000
//!    units plus 1,000 per byte and 512 MiB of memory. Fuel counts the same on
//!    every device, so every device reaches the same verdict.
//! 2. A [`Note`] holds a text-mode note's one yrs document and merges the
//!    records that passed, under a fuel allowance per writer.
//!
//! [`NoteHandle`] runs a [`Note`] off the calling thread, so the editor stays
//! responsive while a slow record decodes, and [`SandboxDocument`] hands it
//! to `editor-core` as the note's document. The sync client reaches the
//! note through a [`SharedNote`], whose calls wait without holding up the
//! editor, and the note's file follows the changes the document merged,
//! which [`SharedNote::take_merged_changes`] hands over. Each load of a
//! document inserts under a client ID from the device's
//! [`LoadReservation`].

mod allowance;
mod editor;
#[doc(hidden)]
pub mod fuzzing;
mod guest;
mod note;
pub mod op;
mod reservation;
mod session;
mod verdict;

use std::sync::Arc;

pub use allowance::{ALLOWANCE_FUEL, REFILL_PERIOD};
pub use editor::SandboxDocument;
pub use note::{
    ClientIds, DEFERRED_MEMORY_BYTES, DocumentError, Effects, Event, INTEGRATION_FUEL_LIMIT,
    INTEGRATION_MEMORY_BYTES, JournalRecord, LineageStatus, Loader, LocalUpdate, Note, Opened,
    Place, RemoteRecord, StopReason,
};
pub use plugin_host::Backend;
pub use reservation::{
    LOAD_BLOCK, LoadReservation, MemoryReservation, ReservationError, ReservationStore,
};
pub use session::{HandleError, NoteHandle, Opening, SharedNote};
pub use verdict::{
    Record, Unreadable, VERDICT_BASE_FUEL, VERDICT_FUEL_PER_BYTE, VERDICT_MEMORY_BYTES, Verdict,
    verdict_budget,
};
use wasmtime::{Engine, Module, OperatorCost};

/// The guest module the sandbox runs, built by
/// `crates/collab-sandbox-guest/build.sh`.
pub static GUEST_MODULE: &[u8] = include_bytes!("../guest/collab_sandbox_guest.wasm");

/// The BLAKE3 hash of [`GUEST_MODULE`], as `sync-v2-collaboration.json`
/// records it. The sandbox refuses to run any other module.
pub const GUEST_MODULE_HASH: &str =
    "474fc8ff67264921721d966cab76ebae7f5d0b24684b80c7c0be769b2cb43b6e";

/// The operators that cost no fuel. Every other operator costs one unit,
/// as `sync-v2-collaboration.json` records.
pub const FREE_OPERATORS: [&str; 8] = [
    "Block",
    "Drop",
    "Else",
    "End",
    "Loop",
    "Nop",
    "Return",
    "Unreachable",
];

/// The fuel each operator whose work grows with an operand costs per unit of
/// that work, as `sync-v2-collaboration.json` records.
pub const VARIABLE_COSTS: [(&str, u8); 16] = [
    ("array_copy_per_element", 1),
    ("array_fill_per_element", 1),
    ("array_init_data_per_element", 1),
    ("array_init_elem_per_element", 1),
    ("array_new_data_per_element", 1),
    ("array_new_default_per_element", 1),
    ("array_new_elem_per_element", 1),
    ("array_new_per_element", 1),
    ("memory_copy_per_byte", 1),
    ("memory_fill_per_byte", 1),
    ("memory_grow_per_page", 0),
    ("memory_init_per_byte", 1),
    ("table_copy_per_element", 1),
    ("table_fill_per_element", 1),
    ("table_grow_per_element", 1),
    ("table_init_per_element", 1),
];

/// The backend the sandbox's own tests run on: the one
/// `NOURA_SANDBOX_BACKEND` names, `cranelift` when it's unset, or `pulley`
/// in a build with the `pulley` feature. `scripts/check-collab-sandbox.sh
/// backends` runs every test once per backend.
///
/// # Panics
///
/// When the variable names a backend this build doesn't run.
#[cfg(any(test, feature = "test-support"))]
pub fn test_backend() -> Backend {
    match std::env::var("NOURA_SANDBOX_BACKEND").as_deref() {
        Err(_) | Ok("" | "cranelift") => Backend::Cranelift,
        #[cfg(feature = "pulley")]
        Ok("pulley") => Backend::Pulley,
        Ok(other) => panic!(
            "NOURA_SANDBOX_BACKEND={other} names no backend this build runs: \
             use cranelift, or pulley with --features pulley"
        ),
    }
}

/// Why the sandbox can't start.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum SandboxError {
    /// The guest module isn't the one whose hash the fixture pins.
    #[error("the guest module's hash {found} isn't the pinned {pinned}")]
    ModuleHash {
        /// The pinned hash.
        pinned: String,
        /// The module's hash.
        found: String,
    },
    /// Wasmtime refused the configuration or the module.
    #[error("Wasmtime refused the sandbox: {0}")]
    Engine(String),
}

/// The engine and the compiled guest module, shared by every instance.
#[derive(Clone)]
pub struct Sandbox {
    inner: Arc<Inner>,
}

struct Inner {
    engine: Engine,
    module: Module,
}

impl Sandbox {
    /// Compiles the pinned guest module for `backend`.
    pub fn new(backend: Backend) -> Result<Self, SandboxError> {
        let found = blake3::hash(GUEST_MODULE).to_hex().to_string();
        if found != GUEST_MODULE_HASH {
            return Err(SandboxError::ModuleHash {
                pinned: GUEST_MODULE_HASH.to_owned(),
                found,
            });
        }
        Self::with_module(backend, GUEST_MODULE)
    }

    /// The sandbox on any module, for tests that stand in a module that
    /// misbehaves. The app runs only [`Self::new`]'s pinned module.
    pub(crate) fn with_module(backend: Backend, bytes: &[u8]) -> Result<Self, SandboxError> {
        let engine = engine(backend).map_err(|error| SandboxError::Engine(error.to_string()))?;
        let module =
            Module::new(&engine, bytes).map_err(|error| SandboxError::Engine(error.to_string()))?;
        Ok(Self {
            inner: Arc::new(Inner { engine, module }),
        })
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.inner.engine
    }

    pub(crate) fn module(&self) -> &Module {
        &self.inner.module
    }
}

/// `plugin-host`'s configuration for `backend`, with fuel on, the operator
/// costs the fixture records, and epoch interruption off, so no clock takes
/// part in a verdict.
fn engine(backend: Backend) -> wasmtime::Result<Engine> {
    let mut config = plugin_host::wasmtime_config(backend)?;
    config.epoch_interruption(false);
    config.consume_fuel(true);
    config.operator_cost(operator_costs());
    // A copy-on-write image rounds the data segments up to the target's page
    // size, and Wasmtime charges that rounded length: x86-64 would pay 4,096
    // units less than Apple silicon and Pulley. Copying the segments charges
    // their exact length everywhere.
    config.memory_init_cow(false);
    Engine::new(&config)
}

/// The operator cost table: one unit per operator, none for
/// [`FREE_OPERATORS`], and [`VARIABLE_COSTS`] for work that grows with an
/// operand. Built field by field, so a Wasmtime release that changes its
/// defaults changes nothing here.
pub fn operator_costs() -> OperatorCost {
    let mut table = serde_json::to_value(OperatorCost::new())
        .expect("Wasmtime's operator cost table serializes");
    let fields = table
        .as_object_mut()
        .expect("the operator cost table is a struct");
    for (name, cost) in fields.iter_mut() {
        if name == "variable" {
            let variable = cost
                .as_object_mut()
                .expect("the variable costs are a struct");
            for (field, value) in variable.iter_mut() {
                let (_, fuel) = VARIABLE_COSTS
                    .iter()
                    .find(|(known, _)| known == field)
                    .unwrap_or_else(|| {
                        panic!("Wasmtime has a variable cost {field} the fixture doesn't record")
                    });
                *value = (*fuel).into();
            }
        } else {
            *cost = u8::from(!FREE_OPERATORS.contains(&name.as_str())).into();
        }
    }
    serde_json::from_value(table).expect("the operator cost table deserializes")
}
