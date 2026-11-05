//! cargo-fuzz target: the guest's verdict and document code built natively,
//! on the records `collab_sandbox::fuzzing::record` builds from each input.
//! libFuzzer sees the guest's coverage here, which it can't inside
//! Wasmtime, and a panic here is a trap in the sandbox. Crashes go to
//! `regressions/record`, which the sandbox itself replays.

#![no_main]

use libfuzzer_sys::fuzz_target;

mod native {
    include!("native.rs");
}

fuzz_target!(|data: &[u8]| native::record(data));
