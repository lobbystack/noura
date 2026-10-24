//! One instance of the guest module: its store, with a memory cap and fuel,
//! and the calls that move bytes in and out of its linear memory.
//!
//! The instance gets no imports at all. A module that imports anything, WASI
//! files, sockets and HTTP included, fails to instantiate, so the guest sees
//! only the bytes the host writes into its memory.

use plugin_host::StoreLimiter;
use wasmtime::{Instance, Memory, Module, Store, Trap, TypedFunc, WasmParams, WasmResults};

use crate::Sandbox;

/// Why a call into the guest failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Failure {
    /// The call ran out of fuel.
    Fuel,
    /// The store hit its memory cap.
    Memory,
    /// The guest trapped, panicked or broke the call protocol.
    Trap(String),
}

pub(crate) struct Guest {
    store: Store<StoreLimiter>,
    instance: Instance,
    memory: Memory,
    /// The fuel the current operation started with.
    budget: u64,
}

impl Guest {
    /// A fresh instance whose store may hold `memory_cap` bytes, starting an
    /// operation that may use `fuel` units, instantiation included: Wasmtime
    /// charges the copy of the module's data segments.
    pub(crate) fn new(sandbox: &Sandbox, memory_cap: usize, fuel: u64) -> Result<Self, Failure> {
        Self::instantiate(sandbox.engine(), sandbox.module(), memory_cap, fuel)
    }

    pub(crate) fn instantiate(
        engine: &wasmtime::Engine,
        module: &Module,
        memory_cap: usize,
        fuel: u64,
    ) -> Result<Self, Failure> {
        let mut store = Store::new(engine, StoreLimiter::new(memory_cap));
        store.limiter(|limiter| limiter);
        store
            .set_fuel(fuel)
            .map_err(|error| Failure::Trap(error.to_string()))?;
        let instance = Instance::new(&mut store, module, &[]).map_err(|error| {
            if store.data().cap_hit() {
                Failure::Memory
            } else if error.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel) {
                Failure::Fuel
            } else {
                Failure::Trap(format!("{error:?}"))
            }
        })?;
        let memory = instance
            .get_memory(&mut store, "memory")
            .ok_or_else(|| Failure::Trap("the guest exports no memory".into()))?;
        Ok(Self {
            store,
            instance,
            memory,
            budget: fuel,
        })
    }

    /// Starts an operation that may use `fuel` units across its calls.
    pub(crate) fn begin(&mut self, fuel: u64) {
        self.budget = fuel;
        self.store.data_mut().clear_cap_hit();
        // Only a store without fuel consumption refuses this.
        let _ = self.store.set_fuel(fuel);
    }

    /// The fuel the current operation used so far.
    pub(crate) fn used(&self) -> u64 {
        self.budget
            .saturating_sub(self.store.get_fuel().unwrap_or(0))
    }

    fn func<P: WasmParams, R: WasmResults>(
        &mut self,
        name: &str,
    ) -> Result<TypedFunc<P, R>, Failure> {
        self.instance
            .get_typed_func::<P, R>(&mut self.store, name)
            .map_err(|error| Failure::Trap(format!("{name}: {error}")))
    }

    /// Calls the export `name`.
    pub(crate) fn call<P: WasmParams, R: WasmResults>(
        &mut self,
        name: &str,
        params: P,
    ) -> Result<R, Failure> {
        let func = self.func::<P, R>(name)?;
        func.call(&mut self.store, params).map_err(|error| {
            if self.store.data().cap_hit() {
                Failure::Memory
            } else if error.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel) {
                Failure::Fuel
            } else {
                Failure::Trap(format!("{error:?}"))
            }
        })
    }

    /// Copies `bytes` into memory the guest allocated, for a call that takes
    /// them.
    pub(crate) fn write(&mut self, bytes: &[u8]) -> Result<u32, Failure> {
        let len = u32::try_from(bytes.len()).map_err(|_| Failure::Memory)?;
        let ptr: u32 = self.call("alloc", len)?;
        self.memory
            .write(&mut self.store, ptr as usize, bytes)
            .map_err(|error| Failure::Trap(error.to_string()))?;
        Ok(ptr)
    }

    /// The output the last call left.
    pub(crate) fn output(&mut self) -> Result<Vec<u8>, Failure> {
        let ptr: u32 = self.call("out_ptr", ())?;
        let len: u32 = self.call("out_len", ())?;
        let mut out = vec![0; len as usize];
        self.memory
            .read(&self.store, ptr as usize, &mut out)
            .map_err(|error| Failure::Trap(error.to_string()))?;
        Ok(out)
    }

    /// The linear memory the instance holds, in bytes.
    pub(crate) fn memory_bytes(&self) -> usize {
        self.store.data().memory_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn try_instantiate(wat: &str) -> Result<Guest, Failure> {
        let sandbox = Sandbox::new(crate::test_backend()).expect("sandbox");
        let module = Module::new(sandbox.engine(), wat).expect("module compiles");
        Guest::instantiate(sandbox.engine(), &module, 1 << 20, 1_000_000)
    }

    #[test]
    fn a_module_importing_files_sockets_or_http_fails_to_instantiate() {
        for (module, name) in [
            ("wasi_snapshot_preview1", "path_open"),
            ("wasi_snapshot_preview1", "fd_read"),
            ("wasi_snapshot_preview1", "sock_accept"),
            ("wasi:filesystem/types@0.2.0", "read"),
            ("wasi:sockets/tcp@0.2.0", "connect"),
            ("wasi:http/outgoing-handler@0.2.0", "handle"),
        ] {
            let wat = format!(
                r#"(module (import "{module}" "{name}" (func)) (memory (export "memory") 1))"#
            );
            assert!(
                try_instantiate(&wat).is_err(),
                "{module} {name} instantiated"
            );
        }
    }

    #[test]
    fn a_module_without_imports_instantiates() {
        assert!(try_instantiate(r#"(module (memory (export "memory") 1))"#).is_ok());
    }
}
