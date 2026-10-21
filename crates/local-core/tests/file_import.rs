use std::path::PathBuf;

use local_core::{CreateObjectInput, WorkspaceEngine};
use tempfile::tempdir;

fn new_engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Import tests", app_data.path())
            .unwrap();
    (workspace, app_data, engine)
}

fn listed(engine: &WorkspaceEngine) -> Vec<String> {
    engine
        .list_workspace_entries()
        .unwrap()
        .into_iter()
        .map(|entry| entry.relative_path)
        .collect()
}

#[test]
fn importing_copies_files_and_leaves_the_sources() {
    let (workspace, _app_data, engine) = new_engine();
    let outside = tempdir().unwrap();
    let bytes = b"%PDF-1.7\n\xff\xfe binary".to_vec();
    std::fs::write(outside.path().join("scan.pdf"), &bytes).unwrap();
    std::fs::write(outside.path().join("Plan.md"), "# Plan\r\n").unwrap();
    std::fs::create_dir(workspace.path().join("papers")).unwrap();

    let result = engine
        .import_external(
            &[
                outside.path().join("scan.pdf"),
                outside.path().join("Plan.md"),
            ],
            "papers",
        )
        .unwrap();

    assert_eq!(result.imported, ["papers/scan.pdf", "papers/Plan.md"]);
    assert_eq!(result.skipped, 0);
    assert_eq!(
        std::fs::read(workspace.path().join("papers/scan.pdf")).unwrap(),
        bytes
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("papers/Plan.md")).unwrap(),
        "# Plan\r\n"
    );
    assert!(outside.path().join("scan.pdf").exists());
    assert!(listed(&engine).contains(&"papers/Plan.md".to_owned()));
}

#[test]
fn a_taken_name_gets_the_next_number_and_nothing_is_overwritten() {
    let (workspace, _app_data, engine) = new_engine();
    let outside = tempdir().unwrap();
    std::fs::write(workspace.path().join("photo.png"), "original").unwrap();
    std::fs::write(workspace.path().join("photo 1.png"), "first copy").unwrap();
    std::fs::create_dir(workspace.path().join("Trip")).unwrap();
    std::fs::write(outside.path().join("photo.png"), "dropped").unwrap();
    std::fs::create_dir(outside.path().join("Trip")).unwrap();

    let result = engine
        .import_external(
            &[
                outside.path().join("photo.png"),
                outside.path().join("Trip"),
            ],
            "",
        )
        .unwrap();

    assert_eq!(result.imported, ["photo 2.png", "Trip 1"]);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("photo.png")).unwrap(),
        "original"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("photo 2.png")).unwrap(),
        "dropped"
    );
    assert!(workspace.path().join("Trip 1").is_dir());
}

#[test]
fn folders_copy_with_their_contents_and_skip_hidden_files_and_links() {
    let (workspace, _app_data, engine) = new_engine();
    let outside = tempdir().unwrap();
    let trip = outside.path().join("Trip");
    std::fs::create_dir_all(trip.join("Days/Monday")).unwrap();
    std::fs::write(trip.join("Days/Monday/notes.md"), "Beach\n").unwrap();
    std::fs::write(trip.join("map.jpg"), [0xff, 0xd8]).unwrap();
    std::fs::write(trip.join(".DS_Store"), "junk").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink("/etc/hosts", trip.join("hosts")).unwrap();

    let result = engine.import_external(&[trip], "").unwrap();

    assert_eq!(result.imported, ["Trip"]);
    assert_eq!(result.skipped, if cfg!(unix) { 2 } else { 1 });
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("Trip/Days/Monday/notes.md")).unwrap(),
        "Beach\n"
    );
    assert_eq!(
        std::fs::read(workspace.path().join("Trip/map.jpg")).unwrap(),
        [0xff, 0xd8]
    );
    assert!(!workspace.path().join("Trip/.DS_Store").exists());
    assert!(std::fs::symlink_metadata(workspace.path().join("Trip/hosts")).is_err());
    assert!(listed(&engine).contains(&"Trip/Days/Monday/notes.md".to_owned()));
}

#[test]
fn a_note_keeps_its_id_unless_the_workspace_already_uses_it() {
    let (workspace, _app_data, engine) = new_engine();
    let outside = tempdir().unwrap();
    let existing = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Brief".into(),
            body: "Ship it.\n".into(),
            relative_path: Some("Brief.md".into()),
            properties: Default::default(),
        })
        .unwrap()
        .value;
    // The same note dragged back in, plus a note from another workspace.
    std::fs::copy(
        workspace.path().join("Brief.md"),
        outside.path().join("Brief.md"),
    )
    .unwrap();
    let (other_workspace, _other_app_data, other) = new_engine();
    let foreign_note = other
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Foreign".into(),
            body: "Hello\n".into(),
            relative_path: Some("Foreign.md".into()),
            properties: Default::default(),
        })
        .unwrap()
        .value;
    let foreign = std::fs::read(other_workspace.path().join("Foreign.md")).unwrap();
    std::fs::write(outside.path().join("Foreign.md"), &foreign).unwrap();

    let result = engine
        .import_external(
            &[
                outside.path().join("Brief.md"),
                outside.path().join("Foreign.md"),
            ],
            "",
        )
        .unwrap();

    assert_eq!(result.imported, ["Brief 1.md", "Foreign.md"]);
    let objects = engine.query_objects(Some("note")).unwrap();
    let copy = objects
        .iter()
        .find(|object| object.relative_path == "Brief 1.md")
        .unwrap();
    assert_ne!(copy.id, existing.id);
    assert!(copy.body.contains("Ship it."));
    assert_eq!(
        std::fs::read(workspace.path().join("Foreign.md")).unwrap(),
        foreign
    );
    assert!(objects.iter().any(|object| object.id == foreign_note.id));
    assert_eq!(objects.len(), 3);
}

#[test]
fn an_import_checks_every_source_before_it_copies_any() {
    let (workspace, _app_data, engine) = new_engine();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("a.txt"), "a").unwrap();
    std::fs::create_dir(workspace.path().join("Docs")).unwrap();

    for sources in [
        vec![
            outside.path().join("a.txt"),
            workspace.path().parent().unwrap().to_owned(),
        ],
        vec![outside.path().join("a.txt"), workspace.path().join("Docs")],
        vec![outside.path().join("a.txt"), PathBuf::from("relative.txt")],
    ] {
        assert!(
            engine.import_external(&sources, "Docs").is_err(),
            "{sources:?}"
        );
    }

    assert!(!workspace.path().join("Docs/a.txt").exists());
    assert_eq!(
        std::fs::read_dir(workspace.path().join("Docs"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn imports_refuse_unsafe_destinations_and_sources() {
    let (workspace, _app_data, engine) = new_engine();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("a.txt"), "a").unwrap();
    std::fs::write(workspace.path().join("file.txt"), "f").unwrap();
    let source = [outside.path().join("a.txt")];

    for folder in ["../elsewhere", "/tmp", ".noura", "missing", "file.txt"] {
        assert!(engine.import_external(&source, folder).is_err(), "{folder}");
    }
    assert_eq!(
        engine
            .import_external(&[PathBuf::from("relative.txt")], "")
            .unwrap_err()
            .code,
        "absolute_path_required"
    );
    // A folder holding the workspace would copy itself forever.
    assert_eq!(
        engine
            .import_external(&[workspace.path().parent().unwrap().to_owned()], "")
            .unwrap_err()
            .code,
        "import_contains_workspace"
    );
    assert!(!workspace.path().join("a.txt").exists());
}
