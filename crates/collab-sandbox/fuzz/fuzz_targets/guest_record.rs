//! cargo-fuzz target: the guest's verdict and document code built natively,
//! on the records `collab_sandbox::fuzzing::record` builds from each input.
//! libFuzzer sees the guest's coverage here, which it can't inside
//! Wasmtime. A panic here is a trap in the sandbox, or a debug assertion
//! the app's module leaves out. Crashes go to `regressions/record`, which
//! the sandbox's tests and the guest's replay.

#![no_main]

use libfuzzer_sys::fuzz_target;

mod native;

fuzz_target!(|data: &[u8]| native::record(data));
