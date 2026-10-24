//! The version of a workspace file, as a viewer checks it before reading
//! the file again: its size and modification time, through the same path
//! checks as `read_file`, without reading the file.

use local_core::WorkspaceEngine;
use tempfile::tempdir;

fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Viewers", app_data.path())
            .unwrap();
    (workspace, app_data, engine)
}

#[test]
fn gives_the_size_and_modification_time_and_changes_when_the_file_is_replaced() {
    let (workspace, _app_data, engine) = engine();
    std::fs::create_dir_all(workspace.path().join("Docs")).unwrap();
    let file = workspace.path().join("Docs/scan.pdf");
    std::fs::write(&file, b"%PDF-1.4").unwrap();

    let (len, modified) = engine.file_version("Docs/scan.pdf").unwrap();
    assert_eq!(len, 8);
    assert_eq!(modified, file.metadata().unwrap().modified().ok());

    std::fs::write(&file, b"%PDF-1.7 replaced").unwrap();
    let (len, _) = engine.file_version("Docs/scan.pdf").unwrap();
    assert_eq!(len, 17);
}

#[test]
fn refuses_what_a_read_refuses() {
    let (workspace, _app_data, engine) = engine();
    std::fs::create_dir_all(workspace.path().join("folder.png")).unwrap();

    for (path, code) in [
        ("../outside.png", "unsafe_path"),
        ("/etc/hosts", "absolute_path"),
        (".noura/workspace.yaml", "reserved_path"),
        ("folder.png", "file_not_found"),
        ("missing.png", "file_not_found"),
    ] {
        let error = engine.file_version(path).unwrap_err();
        assert_eq!(error.code, code, "{path}");
    }
}

#[cfg(unix)]
#[test]
fn refuses_links_in_and_out_of_the_workspace() {
    use std::os::unix::fs::symlink;

    let (workspace, _app_data, engine) = engine();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.png"), "secret").unwrap();
    symlink(
        outside.path().join("secret.png"),
        workspace.path().join("pic.png"),
    )
    .unwrap();
    symlink(outside.path(), workspace.path().join("Shortcut")).unwrap();

    for path in ["pic.png", "Shortcut/secret.png"] {
        let error = engine.file_version(path).unwrap_err();
        assert_eq!(error.code, "symlink_escape", "{path}");
    }
}
