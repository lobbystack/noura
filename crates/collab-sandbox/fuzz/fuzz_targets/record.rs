//! cargo-fuzz target: `collab_sandbox::fuzzing::record` on each input. The
//! scheduled fuzz job runs it, and `crates/collab-sandbox/tests/fuzz.rs`
//! replays every crash it kept from `regressions/record`.

#![no_main]

use std::sync::OnceLock;

use collab_sandbox::{Backend, Sandbox};
use libfuzzer_sys::fuzz_target;

fn sandbox() -> &'static Sandbox {
    static SANDBOX: OnceLock<Sandbox> = OnceLock::new();
    SANDBOX.get_or_init(|| Sandbox::new(Backend::Cranelift).expect("the sandbox starts"))
}

fuzz_target!(|data: &[u8]| {
    let _ = collab_sandbox::fuzzing::record(sandbox(), data);
});
