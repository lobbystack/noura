//! The `noura` binary's `--smoke` run in a package that lacks a plugin it
//! should ship. It says which and fails before it opens a window, so this
//! needs no display. `scripts/check-desktop-packages.sh` runs the passing
//! case from each installed package.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

/// Where a package on this system keeps the binary, and the resources
/// `--smoke` looks in, under the folder `root`.
fn package_layout(root: &Path) -> (PathBuf, PathBuf) {
    let binary = format!("noura{}", std::env::consts::EXE_SUFFIX);
    if cfg!(target_os = "macos") {
        let contents = root.join("Noura.app/Contents");
        (
            contents.join("MacOS").join(binary),
            contents.join("Resources"),
        )
    } else if cfg!(windows) {
        (root.join(binary), root.to_owned())
    } else {
        (
            root.join("usr/bin").join(binary),
            root.join("usr/lib/noura"),
        )
    }
}

/// A package under `root` holding this build's binary, with its resources
/// folder made.
fn package(root: &Path) -> (PathBuf, PathBuf) {
    let (binary, resources) = package_layout(root);
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::create_dir_all(&resources).unwrap();
    fs::copy(env!("CARGO_BIN_EXE_noura"), &binary).unwrap();
    (binary, resources)
}

fn smoke(binary: &Path) -> Output {
    Command::new(binary).arg("--smoke").output().unwrap()
}

fn plugins_line(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .find(|line| line.starts_with("Plugins:"))
        .unwrap_or_else(|| panic!("no plugins line in {output:?}"))
        .to_owned()
}

#[test]
fn a_package_without_plugins_fails_its_smoke_run_and_names_them() {
    let root = tempfile::tempdir().unwrap();
    let (binary, _) = package(root.path());

    let output = smoke(&binary);

    assert!(!output.status.success(), "{output:?}");
    let line = plugins_line(&output);
    for name in ["AI", "Calendar", "Projects", "Tasks"] {
        assert!(line.contains(name), "{line}");
    }
    assert!(
        !String::from_utf8_lossy(&output.stdout).contains("first window"),
        "{output:?}"
    );
}

#[test]
fn a_package_that_ships_a_broken_plugin_fails_its_smoke_run() {
    let root = tempfile::tempdir().unwrap();
    let (binary, resources) = package(root.path());
    let tasks = resources.join("plugins/tasks");
    fs::create_dir_all(&tasks).unwrap();
    fs::write(
        tasks.join("plugin.toml"),
        include_str!("../../../crates/plugins/tasks/plugin.toml"),
    )
    .unwrap();
    fs::write(tasks.join("tasks_plugin.wasm"), b"not a component").unwrap();

    let output = smoke(&binary);

    assert!(!output.status.success(), "{output:?}");
    let line = plugins_line(&output);
    assert!(line.contains("Tasks (tasks_plugin.wasm"), "{line}");
}
