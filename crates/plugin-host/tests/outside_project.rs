//! An outside Rust project (story 129): the template in
//! `crates/plugin-sdk/template` builds in a folder away from this
//! repository, as an author's copy does, and runs in the real host. The
//! guide holds the template's files as they are.

mod support;

use std::path::Path;
use std::process::Command;

use plugin_host::{Event, Services};
use support::plugins::{self, FakeWorkspace, built};

fn template(path: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../plugin-sdk/template")
            .join(path),
    )
    .unwrap()
}

#[test]
fn the_guide_shows_the_templates_files() {
    let guide = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/plugins/rust.md"),
    )
    .unwrap();
    for file in ["Cargo.toml", "plugin.toml", "src/lib.rs"] {
        assert!(guide.contains(&template(file)), "{file} differs");
    }
}

#[test]
fn the_template_builds_outside_the_repository_and_runs_in_the_host() {
    // The hello plugin stands for the wasm32-wasip2 target being there.
    if built("hello_plugin").is_none() {
        return;
    }
    let project = tempfile::tempdir().unwrap();
    let sdk = format!("{}/../plugin-sdk", env!("CARGO_MANIFEST_DIR")).replace('\\', "/");
    let manifest = template("Cargo.toml").replace("../noura/crates/plugin-sdk", &sdk);
    std::fs::write(project.path().join("Cargo.toml"), manifest).unwrap();
    std::fs::write(project.path().join("plugin.toml"), template("plugin.toml")).unwrap();
    std::fs::create_dir(project.path().join("src")).unwrap();
    std::fs::write(project.path().join("src/lib.rs"), template("src/lib.rs")).unwrap();

    let build = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "build",
            "--release",
            "--offline",
            "--target",
            "wasm32-wasip2",
        ])
        .current_dir(project.path())
        .env_remove("CARGO_TARGET_DIR")
        .output()
        .expect("cargo runs");
    assert!(
        build.status.success(),
        "{}",
        String::from_utf8_lossy(&build.stderr)
    );
    std::fs::create_dir(project.path().join("dist")).unwrap();
    std::fs::copy(
        project
            .path()
            .join("target/wasm32-wasip2/release/word_count.wasm"),
        project.path().join("dist/plugin.wasm"),
    )
    .unwrap();

    let library = tempfile::tempdir().unwrap();
    let host = plugins::host(library.path());
    host.install(&host.inspect(project.path()).unwrap())
        .unwrap();
    let loaded = host.load("word-count").unwrap();
    let workspace = FakeWorkspace::default();
    let plugin = host.start(
        &loaded,
        &loaded.manifest().capabilities,
        Services {
            workspace: Some(std::sync::Arc::new(workspace)),
            ..Services::default()
        },
    );
    assert_eq!(plugin.pages().unwrap()[0].id, "main");
    plugin
        .handle_event("main", &Event::Pressed("press".into()))
        .unwrap();
    let texts = plugins::texts(&plugin.render("main").unwrap());
    assert!(texts.contains(&"Presses: 1".to_owned()), "{texts:?}");
}
