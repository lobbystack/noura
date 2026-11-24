# Fuzz regressions

Each file here is an input that crashed the sandbox under cargo-fuzz. `crates/collab-sandbox/tests/fuzz.rs` replays every file through the sandbox on each test run, and `crates/collab-sandbox-guest/tests/it/fuzz.rs` replays those in `record` and `writer_record` through the guest's code built natively, where yrs's debug assertions stay on. The folder names the target that found it: `verdict`, `record` or `writer_record`. A `guest_record` or `guest_writer_record` crash goes in the folder of the sandbox target that builds the same records.

The scheduled fuzz job copies the crashes it finds here and opens a pull request. To add one by hand, copy the file from `crates/collab-sandbox/fuzz/artifacts/<target>/` into the folder of its target.
