//! Every input the scheduled fuzz job kept in
//! `crates/collab-sandbox/fuzz/regressions`, through the guest's code built
//! natively, as the guest fuzz targets run it. Built this way, the guest
//! keeps yrs's debug assertions and its own overflow checks, which the
//! app's module leaves out, so an input can fail here and pass in the
//! sandbox, where `crates/collab-sandbox/tests/fuzz.rs` replays it.

use std::path::PathBuf;

#[path = "../../../collab-sandbox/fuzz/fuzz_targets/native.rs"]
mod native;

/// The inputs kept for `target`, by name.
fn kept(target: &str) -> Vec<(PathBuf, Vec<u8>)> {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../collab-sandbox/fuzz/regressions")
        .join(target);
    let mut inputs: Vec<(PathBuf, Vec<u8>)> = std::fs::read_dir(&dir)
        .unwrap_or_else(|error| panic!("{}: {error}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.is_file() && path.extension().is_none_or(|ext| ext != "md"))
        .map(|path| {
            let bytes = std::fs::read(&path).unwrap();
            (path, bytes)
        })
        .collect();
    inputs.sort();
    inputs
}

#[test]
fn kept_fuzz_inputs_pass_natively() {
    let mut replayed = 0;
    for (path, bytes) in kept("record") {
        eprintln!("replaying {}", path.display());
        native::record(&bytes);
        replayed += 1;
    }
    for (path, bytes) in kept("writer_record") {
        eprintln!("replaying {}", path.display());
        native::record(&native::inputs::writer_yrs(&bytes));
        replayed += 1;
    }
    assert!(replayed > 0, "no kept input");
}
