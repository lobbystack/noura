//! Finds the trial S7 hello plugins that `trial/build-plugins.sh` builds.
//!
//! Without the built plugins the trial tests pass with a note on stderr, so a
//! machine without Node or the wasm32-wasip2 target can still run the
//! workspace tests. CI sets `NOURA_S7_REQUIRE_PLUGINS=1`, which turns a missing
//! plugin into a failure.

#![allow(dead_code)]

use std::path::PathBuf;

use plugin_host::trial::Backend;

/// Every backend this build of the host supports.
pub fn backends() -> Vec<Backend> {
    vec![
        Backend::Cranelift,
        #[cfg(feature = "pulley")]
        Backend::Pulley,
    ]
}

pub struct TrialPlugin {
    pub name: &'static str,
    pub bytes: Vec<u8>,
}

/// Both hello plugins, Rust first, or `None` when they aren't built and CI
/// doesn't require them.
pub fn hello_plugins() -> Option<Vec<TrialPlugin>> {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("../../target"));
    let candidates = [
        (
            "Rust",
            "S7_RUST_PLUGIN",
            target_dir.join("wasm32-wasip2/release/s7_rust_hello.wasm"),
        ),
        (
            "TypeScript",
            "S7_TYPESCRIPT_PLUGIN",
            manifest_dir.join("../../sdk/typescript/s7-hello/dist/s7-hello.wasm"),
        ),
    ];

    let mut plugins = Vec::new();
    for (name, variable, default_path) in candidates {
        let path = std::env::var_os(variable)
            .map(PathBuf::from)
            .unwrap_or(default_path);
        match std::fs::read(&path) {
            Ok(bytes) => plugins.push(TrialPlugin { name, bytes }),
            Err(error) => {
                let message = format!(
                    "the {name} hello plugin isn't built at {} ({error}); run crates/plugin-host/trial/build-plugins.sh",
                    path.display()
                );
                if std::env::var_os("NOURA_S7_REQUIRE_PLUGINS").is_some_and(|value| value == "1") {
                    panic!("{message}");
                }
                eprintln!("skipping trial S7 test: {message}");
                return None;
            }
        }
    }
    Some(plugins)
}
