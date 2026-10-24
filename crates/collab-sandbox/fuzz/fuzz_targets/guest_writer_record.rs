//! cargo-fuzz target: the guest's code built natively, on the records
//! `collab_sandbox::fuzzing::writer_record` builds from each input. Crashes
//! go to `regressions/writer_record`, which the sandbox itself replays.

#![no_main]

use libfuzzer_sys::fuzz_target;

mod native {
    include!("native.rs");
}

fuzz_target!(|data: &[u8]| native::record(&collab_sandbox::fuzzing::writer_yrs(data)));
