//! Reading the start of a Markdown file, as a note embed does: at most the
//! bytes asked for, cut back to a whole character, through the same path
//! checks as a whole read.

use local_core::WorkspaceEngine;
use tempfile::tempdir;

fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Embeds", app_data.path()).unwrap();
    (workspace, app_data, engine)
}

#[test]
fn reads_at_most_the_bytes_asked_for_and_ends_on_a_whole_character() {
    let (workspace, _app_data, engine) = engine();
    // "é" takes two bytes, so 5 bytes end inside the third one.
    std::fs::write(workspace.path().join("long.md"), "éééé and more").unwrap();

    assert_eq!(engine.read_raw_markdown_start("long.md", 5).unwrap(), "éé");
    assert_eq!(engine.read_raw_markdown_start("long.md", 6).unwrap(), "ééé");
    assert_eq!(
        engine.read_raw_markdown_start("long.md", 1024).unwrap(),
        "éééé and more"
    );
}

#[test]
fn reads_like_a_whole_read_without_the_bom_and_crlf_line_breaks() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(
        workspace.path().join("windows.md"),
        b"\xEF\xBB\xBF# Title\r\nline\r\n",
    )
    .unwrap();

    assert_eq!(
        engine.read_raw_markdown_start("windows.md", 1024).unwrap(),
        engine.read_raw_markdown("windows.md").unwrap().body
    );
    assert_eq!(
        engine.read_raw_markdown_start("windows.md", 1024).unwrap(),
        "# Title\nline\n"
    );
}

#[test]
fn refuses_what_a_whole_read_refuses() {
    let (workspace, _app_data, engine) = engine();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.md"), "secret").unwrap();
    std::fs::write(workspace.path().join("picture.png"), "not markdown").unwrap();
    std::fs::write(workspace.path().join("bytes.md"), b"\xFF\xFE broken").unwrap();

    for path in ["../secret.md", "picture.png", "missing.md", "bytes.md"] {
        assert!(
            engine.read_raw_markdown_start(path, 1024).is_err(),
            "{path:?}"
        );
    }
    let absolute = outside.path().join("secret.md");
    assert!(
        engine
            .read_raw_markdown_start(absolute.to_str().unwrap(), 1024)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn refuses_a_symlink_that_leaves_the_workspace() {
    let (workspace, _app_data, engine) = engine();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.md"), "secret").unwrap();
    std::os::unix::fs::symlink(outside.path(), workspace.path().join("linked")).unwrap();

    assert!(
        engine
            .read_raw_markdown_start("linked/secret.md", 1024)
            .is_err()
    );
    assert!(engine.read_raw_markdown("linked/secret.md").is_err());
}
