//! The version of a note's local asset, as a note checks it before reading
//! the asset again: its size and modification time, through the same path
//! checks as a read, without reading the file.

use local_core::WorkspaceEngine;
use tempfile::tempdir;

fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Assets", app_data.path()).unwrap();
    (workspace, app_data, engine)
}

#[test]
fn names_the_asset_a_read_would_read_with_its_size_and_modification_time() {
    let (workspace, _app_data, engine) = engine();
    std::fs::create_dir_all(workspace.path().join("notes")).unwrap();
    let image = workspace.path().join("image.png");
    std::fs::write(&image, b"png").unwrap();

    let (path, len, modified) = engine
        .local_asset_version("notes/source.md", "../image.png")
        .unwrap();
    assert_eq!((path.as_str(), len), ("image.png", 3));
    assert_eq!(modified, image.metadata().unwrap().modified().ok());
    let (read, _) = engine
        .read_local_asset("notes/source.md", "../image.png", 1024)
        .unwrap();
    assert_eq!(read, path);

    // Another write gives another version.
    std::fs::write(&image, b"a new png").unwrap();
    let (_, len, _) = engine
        .local_asset_version("notes/source.md", "../image.png")
        .unwrap();
    assert_eq!(len, 9);
}

#[test]
fn refuses_what_a_read_refuses() {
    let (workspace, _app_data, engine) = engine();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.png"), "secret").unwrap();
    std::fs::write(workspace.path().join("note.md"), "# Note\n").unwrap();
    std::fs::create_dir_all(workspace.path().join("folder.png")).unwrap();
    let absolute = outside.path().join("secret.png");

    for target in [
        "../secret.png",
        "../../secret.png",
        absolute.to_str().unwrap(),
        "note.md",
        "missing.png",
        "folder.png",
    ] {
        assert!(
            engine.local_asset_version("source.md", target).is_err(),
            "{target:?}"
        );
    }
}

#[cfg(unix)]
#[test]
fn refuses_a_symlink_that_leaves_the_workspace() {
    let (workspace, _app_data, engine) = engine();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.png"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), workspace.path().join("linked")).unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.png"),
        workspace.path().join("pic.png"),
    )
    .unwrap();

    for target in ["linked/secret.png", "pic.png"] {
        assert!(
            engine.local_asset_version("source.md", target).is_err(),
            "{target:?}"
        );
        assert!(
            engine.read_local_asset("source.md", target, 1024).is_err(),
            "{target:?}"
        );
    }
}
