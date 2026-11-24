//! cargo-fuzz target: the guest's code built natively, on the records
//! `collab_sandbox::fuzzing::writer_record` builds from each input. Crashes
//! go to `regressions/writer_record`, which the sandbox's tests and the
//! guest's replay.

#![no_main]

use libfuzzer_sys::fuzz_target;

mod native;

fuzz_target!(|data: &[u8]| native::record(&native::inputs::writer_yrs(data)));
