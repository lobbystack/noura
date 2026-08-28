use std::{collections::BTreeMap, time::Duration};

use local_core::{
    CreateObjectInput, ObjectPatch, ParsedMarkdown, SearchInput, WorkspaceEngine, parse_markdown,
};
use tempfile::tempdir;

fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Domain tests", app_data.path())
            .unwrap();
    (workspace, app_data, engine)
}

#[test]
fn notes_support_create_update_move_and_trash() {
    let (workspace, _app_data, engine) = engine();
    let created = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Lifecycle".into(),
            body: "First".into(),
            relative_path: None,
            properties: BTreeMap::new(),
        })
        .unwrap();
    let updated = engine
        .update_object(
            &created.value.id,
            ObjectPatch {
                title: Some("Renamed".into()),
                body: Some("Second".into()),
                properties: BTreeMap::from([("custom".into(), serde_json::json!(true))]),
                remove_properties: Vec::new(),
                expected_revision: created.revision,
            },
        )
        .unwrap();
    let moved = engine
        .move_object(&updated.value.id, "archive/note.md", &updated.revision)
        .unwrap();
    let deleted = engine
        .delete_object(&moved.value.id, &moved.revision)
        .unwrap();
    assert_eq!(deleted.value.id, created.value.id);
    assert!(!workspace.path().join("archive/note.md").exists());
    assert!(
        workspace
            .path()
            .join(".noura/trash")
            .read_dir()
            .unwrap()
            .next()
            .is_some()
    );
}

#[test]
fn idless_markdown_requires_explicit_adoption() {
    let (workspace, _app_data, engine) = engine();
    let path = workspace.path().join("draft.md");
    std::fs::write(&path, "# Draft\n\nText\n").unwrap();
    engine.reconcile().unwrap();
    assert!(engine.query_objects(Some("note")).unwrap().is_empty());
    assert_eq!(
        engine
            .search(&SearchInput {
                query: "Draft".into(),
                ..Default::default()
            })
            .unwrap()
            .len(),
        1
    );
    let bytes = std::fs::read(&path).unwrap();
    let revision = blake3::hash(&bytes).to_hex().to_string();
    let adopted = engine
        .adopt_markdown("draft.md", "note", &revision)
        .unwrap();
    assert!(matches!(
        parse_markdown("draft.md", &std::fs::read(path).unwrap()),
        ParsedMarkdown::Managed(_)
    ));
    assert_eq!(adopted.value.title, "Draft");
}

#[test]
fn tasks_projects_calendar_and_kanban_metadata_share_files() {
    let (_workspace, _app_data, engine) = engine();
    let project = engine
        .create_object(CreateObjectInput {
            object_type: "project".into(),
            title: "Launch".into(),
            body: String::new(),
            relative_path: None,
            properties: BTreeMap::new(),
        })
        .unwrap();
    let task = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Ship".into(),
            body: "Checklist".into(),
            relative_path: None,
            properties: BTreeMap::from([
                ("project".into(), serde_json::json!(project.value.id)),
                ("due".into(), serde_json::json!("2026-09-04")),
                ("kanban_order".into(), serde_json::json!("a0")),
            ]),
        })
        .unwrap();
    assert_eq!(task.value.properties["status"], "todo");
    assert_eq!(task.value.properties["priority"], "medium");
    assert_eq!(
        engine.calendar("2026-09-01", "2026-10-01").unwrap()[0].source_id,
        task.value.id
    );
    let completed = engine
        .update_object(
            &task.value.id,
            ObjectPatch {
                title: None,
                body: None,
                properties: BTreeMap::from([
                    ("status".into(), serde_json::json!("done")),
                    ("kanban_order".into(), serde_json::json!("z0")),
                ]),
                remove_properties: Vec::new(),
                expected_revision: task.revision,
            },
        )
        .unwrap();
    assert_eq!(completed.value.properties["status"], "done");
    engine
        .delete_object(&project.value.id, &project.revision)
        .unwrap();
    assert!(engine.get_object(&task.value.id).unwrap().is_some());
}

#[test]
fn duplicate_ids_are_indexed_but_block_identity_reads() {
    let (workspace, _app_data, engine) = engine();
    let note = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "One".into(),
            body: String::new(),
            relative_path: Some("one.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    std::fs::copy(
        workspace.path().join("one.md"),
        workspace.path().join("two.md"),
    )
    .unwrap();
    engine.reconcile().unwrap();
    let error = engine.get_object(&note.value.id).unwrap_err();
    assert_eq!(error.code, "identity_conflict");
}

#[test]
fn reconciliation_repairs_an_external_save() {
    let (workspace, _app_data, engine) = engine();
    let note = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Watch".into(),
            body: "Before".into(),
            relative_path: Some("watch.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    let path = workspace.path().join("watch.md");
    let content = std::fs::read_to_string(&path)
        .unwrap()
        .replace("Before", "After");
    std::fs::write(&path, content).unwrap();
    engine.reconcile().unwrap();
    assert_eq!(
        engine.get_object(&note.value.id).unwrap().unwrap().body,
        "After"
    );
}

#[test]
fn watcher_poll_accepts_an_empty_batch_and_keeps_the_workspace_usable() {
    let (_workspace, _app_data, engine) = engine();
    let changes = engine
        .poll_external_changes(Duration::from_millis(25))
        .unwrap();
    assert!(changes.is_empty());
    assert_eq!(engine.state().phase, local_core::WorkspacePhase::Ready);
}

#[test]
fn path_traversal_is_rejected_before_a_write() {
    let (_workspace, _app_data, engine) = engine();
    let error = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Unsafe".into(),
            body: String::new(),
            relative_path: Some("../outside.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap_err();
    assert_eq!(error.code, "unsafe_path");
}
