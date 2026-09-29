use std::collections::BTreeMap;

use local_core::{CreateObjectInput, WorkspaceEngine};
use tempfile::tempdir;

fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Copy tests", app_data.path())
            .unwrap();
    (workspace, app_data, engine)
}

#[test]
fn copying_a_plain_file_keeps_its_bytes_and_the_original() {
    let (workspace, _app_data, engine) = engine();
    let bytes = b"%PDF-1.7\n\xff\xfe binary".to_vec();
    std::fs::create_dir_all(workspace.path().join("papers")).unwrap();
    std::fs::write(workspace.path().join("papers/scan.pdf"), &bytes).unwrap();
    std::fs::write(workspace.path().join("draft.md"), "# Draft\n\nText\r\n").unwrap();
    engine.reconcile().unwrap();

    engine
        .copy_file("papers/scan.pdf", "papers/scan 1.pdf")
        .unwrap();
    engine.copy_file("draft.md", "archive/Draft 1.md").unwrap();

    assert_eq!(
        std::fs::read(workspace.path().join("papers/scan.pdf")).unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read(workspace.path().join("papers/scan 1.pdf")).unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("archive/Draft 1.md")).unwrap(),
        "# Draft\n\nText\r\n"
    );
    let listed = engine
        .list_workspace_entries()
        .unwrap()
        .into_iter()
        .map(|entry| entry.relative_path)
        .collect::<Vec<_>>();
    assert!(listed.contains(&"papers/scan 1.pdf".to_owned()));
    assert!(listed.contains(&"archive/Draft 1.md".to_owned()));
}

#[test]
fn copying_a_note_creates_a_new_identity() {
    let (workspace, _app_data, engine) = engine();
    let mut properties = BTreeMap::new();
    properties.insert("tags".to_owned(), serde_json::json!(["plans"]));
    let original = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Launch brief".into(),
            body: "Ship it.\n".into(),
            relative_path: Some("Launch brief.md".into()),
            properties,
        })
        .unwrap()
        .value;

    engine
        .copy_file("Launch brief.md", "Launch brief 1.md")
        .unwrap();

    let objects = engine.query_objects(Some("note")).unwrap();
    assert_eq!(objects.len(), 2, "both notes stay listed with unique IDs");
    let copy = objects
        .iter()
        .find(|object| object.relative_path == "Launch brief 1.md")
        .unwrap();
    assert_ne!(copy.id, original.id);
    assert_eq!(copy.title, "Launch brief 1");
    let unchanged = engine.get_object(&original.id).unwrap().unwrap();
    let copy_bytes = std::fs::read_to_string(workspace.path().join("Launch brief 1.md")).unwrap();
    assert!(copy_bytes.ends_with("# Launch brief 1\n\nShip it.\n"));
    assert_eq!(copy.body.trim_end(), unchanged.body.trim_end());
    assert_eq!(
        copy.properties.get("tags"),
        Some(&serde_json::json!(["plans"]))
    );
    assert_eq!(unchanged.relative_path, "Launch brief.md");
    assert_eq!(unchanged.revision, original.revision);
    assert!(workspace.path().join("Launch brief 1.md").is_file());
}

#[test]
fn copying_never_overwrites_escapes_or_copies_folders() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(workspace.path().join("a.md"), "a").unwrap();
    std::fs::write(workspace.path().join("b.md"), "b").unwrap();
    std::fs::create_dir_all(workspace.path().join("folder")).unwrap();
    engine.reconcile().unwrap();

    assert_eq!(
        engine.copy_file("a.md", "b.md").unwrap_err().code,
        "path_exists"
    );
    assert!(engine.copy_file("a.md", "../outside.md").is_err());
    assert!(engine.copy_file("a.md", "/tmp/absolute.md").is_err());
    assert!(engine.copy_file("a.md", ".noura/a.md").is_err());
    assert!(engine.copy_file("../a.md", "c.md").is_err());
    assert!(engine.copy_file("missing.md", "c.md").is_err());
    assert_eq!(
        engine.copy_file("folder", "folder 1").unwrap_err().code,
        "file_not_found"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("b.md")).unwrap(),
        "b"
    );
    assert!(!workspace.path().join("c.md").exists());
    assert!(!workspace.path().join("folder 1").exists());
}

#[cfg(unix)]
#[test]
fn copying_refuses_symlinks() {
    let (workspace, _app_data, engine) = engine();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("secret.md"), "secret").unwrap();
    std::os::unix::fs::symlink(
        outside.path().join("secret.md"),
        workspace.path().join("link.md"),
    )
    .unwrap();
    assert!(engine.copy_file("link.md", "copy.md").is_err());
    assert!(!workspace.path().join("copy.md").exists());
}

#[test]
fn a_copied_note_survives_an_index_rebuild() {
    let (workspace, app_data, engine) = engine();
    let original = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Plan".into(),
            body: "Body".into(),
            relative_path: Some("Plan.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap()
        .value;
    engine.copy_file("Plan.md", "Plan 1.md").unwrap();
    let index_path = engine.index_path().to_owned();
    drop(engine);
    std::fs::remove_file(index_path).unwrap();
    let reopened = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
    let objects = reopened.query_objects(Some("note")).unwrap();
    assert_eq!(objects.len(), 2);
    assert!(objects.iter().any(|object| object.id == original.id));
}

#[test]
fn copying_reads_the_file_not_a_stale_index() {
    let (workspace, _app_data, engine) = engine();
    engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Plan".into(),
            body: "Old body".into(),
            relative_path: Some("Plan.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    let path = workspace.path().join("Plan.md");
    let edited = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Old body", "Edited outside noura");
    std::fs::write(&path, edited).unwrap();
    std::fs::write(
        workspace.path().join("broken.md"),
        "---\nid: [\n---\nBody\n",
    )
    .unwrap();

    engine.copy_file("Plan.md", "Plan 1.md").unwrap();
    engine.copy_file("broken.md", "broken 1.md").unwrap();

    let copy = std::fs::read_to_string(workspace.path().join("Plan 1.md")).unwrap();
    assert!(copy.contains("Edited outside noura"));
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("broken 1.md")).unwrap(),
        "---\nid: [\n---\nBody\n"
    );
}
