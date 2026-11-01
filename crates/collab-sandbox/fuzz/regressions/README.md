# Fuzz regressions

Each file here is an input that crashed the sandbox under cargo-fuzz, kept so `crates/collab-sandbox/tests/fuzz.rs` replays it on every test run. The folder names the target that found it: `verdict`, `record` or `writer_record`.

The scheduled fuzz job copies the crashes it finds here and opens a pull request. To add one by hand, copy the file from `crates/collab-sandbox/fuzz/artifacts/<target>/` into the folder of its target.
