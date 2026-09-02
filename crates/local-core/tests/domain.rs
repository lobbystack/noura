use std::{collections::BTreeMap, time::Duration};

use local_core::{
    ConflictResolution, CreateObjectInput, ManagedConflictResolution, ManagedConflictResolveInput,
    ManagedDraftInput, ManagedDraftResult, ManifestUpdateInput, MarkdownLinkTarget, ObjectPatch,
    ParseStatus, ParsedMarkdown, RawConflictResolveInput, RawSaveInput, RawSaveResult, SearchInput,
    WorkspaceEngine, WorkspaceEntryKind, WorkspaceManifest, new_object_id, parse_markdown,
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
fn managed_draft_save_merges_disjoint_fields_and_snapshots_external_version() {
    let (workspace, app_data, engine) = engine();
    let created = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Ship beta".into(),
            body: "first opening\n\nsecond line detail".into(),
            relative_path: None,
            properties: BTreeMap::from([
                ("status".into(), serde_json::json!("todo")),
                ("priority".into(), serde_json::json!("medium")),
            ]),
        })
        .unwrap();
    let base = created.value;
    let external = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
    external
        .update_object(
            &base.id,
            ObjectPatch {
                title: None,
                body: Some("first opening\n\nsecond line detail from file".into()),
                properties: BTreeMap::from([("priority".into(), serde_json::json!("high"))]),
                remove_properties: Vec::new(),
                expected_revision: base.revision.clone(),
            },
        )
        .unwrap();
    let local = ManagedDraftInput {
        id: base.id.clone(),
        base_revision: base.revision.clone(),
        base_title: base.title.clone(),
        base_body: "first opening\n\nsecond line detail".into(),
        base_properties: base.properties.clone(),
        local_title: base.title.clone(),
        local_body: "first opening edited locally\n\nsecond line detail".into(),
        local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
    };
    let result = engine.save_managed_draft(local).unwrap();
    let ManagedDraftResult::Merged {
        current,
        title,
        body,
        properties,
    } = result
    else {
        panic!("expected merged draft, got {result:?}");
    };
    assert_eq!(title, "Ship beta");
    assert_eq!(
        body,
        "first opening edited locally\n\nsecond line detail from file",
    );
    assert_eq!(properties["status"], serde_json::json!("in-progress"));
    assert_eq!(properties["priority"], serde_json::json!("high"));
    assert_ne!(current.revision, base.revision);
    assert!(
        workspace
            .path()
            .join(".noura/history")
            .join(&base.id)
            .read_dir()
            .unwrap()
            .next()
            .is_some(),
        "external snapshot should be recoverable"
    );
}

#[test]
fn managed_draft_save_updates_an_unchanged_canonical_file() {
    let (workspace, _app_data, engine) = engine();
    let base = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Original".into(),
            body: "base body".into(),
            relative_path: None,
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap()
        .value;

    let result = engine
        .save_managed_draft(ManagedDraftInput {
            id: base.id.clone(),
            base_revision: base.revision.clone(),
            base_title: base.title.clone(),
            base_body: base.body.clone(),
            base_properties: base.properties.clone(),
            local_title: "Renamed".into(),
            local_body: "edited body".into(),
            local_properties: BTreeMap::from([
                ("status".into(), serde_json::json!("in-progress")),
                ("priority".into(), serde_json::json!("high")),
            ]),
        })
        .unwrap();
    let ManagedDraftResult::Unchanged { current } = result else {
        panic!("expected clean-base save, got {result:?}");
    };

    assert_eq!(current.title, "Renamed");
    assert_eq!(current.body, "edited body");
    assert_eq!(current.properties["status"], "in-progress");
    assert_eq!(current.properties["priority"], "high");
    assert_ne!(current.revision, base.revision);
    assert!(workspace.path().join(&current.relative_path).exists());
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

#[test]
fn workspace_entries_classify_visible_workspace_content() {
    let (workspace, _app_data, engine) = engine();
    let managed = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Managed".into(),
            body: String::new(),
            relative_path: Some("docs/managed.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    std::fs::create_dir(workspace.path().join("archive")).unwrap();
    std::fs::write(workspace.path().join("docs/draft.md"), "# Draft\n\nBody\n").unwrap();
    std::fs::write(
        workspace.path().join("docs/broken.md"),
        "---\nid: incomplete\n# Broken\n",
    )
    .unwrap();
    std::fs::write(workspace.path().join("docs/image.png"), [0_u8, 1, 2]).unwrap();

    let entries = engine.list_workspace_entries().unwrap();
    let summary = entries
        .iter()
        .map(|entry| {
            (
                entry.relative_path.as_str(),
                entry.kind,
                entry.parse_status,
                entry.object_type.as_deref(),
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(
        summary,
        vec![
            ("archive", WorkspaceEntryKind::Folder, None, None),
            ("docs", WorkspaceEntryKind::Folder, None, None),
            (
                "docs/broken.md",
                WorkspaceEntryKind::File,
                Some(ParseStatus::Malformed),
                None,
            ),
            (
                "docs/draft.md",
                WorkspaceEntryKind::File,
                Some(ParseStatus::Unmanaged),
                None,
            ),
            (
                "docs/image.png",
                WorkspaceEntryKind::File,
                Some(ParseStatus::Binary),
                None,
            ),
            (
                "docs/managed.md",
                WorkspaceEntryKind::File,
                Some(ParseStatus::Managed),
                Some("note"),
            ),
        ]
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| entry.relative_path == "docs/managed.md")
            .and_then(|entry| entry.object_id.as_deref()),
        Some(managed.value.id.as_str())
    );
}

#[test]
fn workspace_entries_apply_manifest_ignores_and_reserved_paths() {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Visibility", app_data.path())
            .unwrap();
    drop(engine);
    let manifest_path = workspace.path().join("workspace.yaml");
    let mut manifest: WorkspaceManifest =
        serde_yaml_ng::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
    manifest.ignore = vec!["ignored/".into()];
    std::fs::write(&manifest_path, serde_yaml_ng::to_string(&manifest).unwrap()).unwrap();
    std::fs::create_dir_all(workspace.path().join("ignored")).unwrap();
    std::fs::write(workspace.path().join("ignored/hidden.txt"), "hidden").unwrap();
    std::fs::create_dir_all(workspace.path().join(".git")).unwrap();
    std::fs::write(workspace.path().join(".git/config"), "hidden").unwrap();
    std::fs::write(workspace.path().join("visible.txt"), "visible").unwrap();
    let engine = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();

    let paths = engine
        .list_workspace_entries()
        .unwrap()
        .into_iter()
        .map(|entry| entry.relative_path)
        .collect::<Vec<_>>();

    assert_eq!(paths, vec!["visible.txt"]);
}

#[test]
fn external_move_preserves_managed_identity_in_workspace_entries() {
    let (workspace, _app_data, engine) = engine();
    let note = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Movable".into(),
            body: String::new(),
            relative_path: Some("before.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    std::fs::create_dir(workspace.path().join("after")).unwrap();
    std::fs::rename(
        workspace.path().join("before.md"),
        workspace.path().join("after/moved.md"),
    )
    .unwrap();

    engine.reconcile().unwrap();
    let entries = engine.list_workspace_entries().unwrap();

    assert_eq!(
        (
            entries
                .iter()
                .any(|entry| entry.relative_path == "before.md"),
            entries
                .iter()
                .find(|entry| entry.relative_path == "after/moved.md")
                .and_then(|entry| entry.object_id.as_deref()),
        ),
        (false, Some(note.value.id.as_str()))
    );
}

#[test]
fn external_delete_disappears_from_entries_and_index() {
    let (workspace, _app_data, engine) = engine();
    let note = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Temporary".into(),
            body: String::new(),
            relative_path: Some("temporary.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap();
    std::fs::remove_file(workspace.path().join("temporary.md")).unwrap();

    engine.reconcile().unwrap();

    assert_eq!(
        (
            engine
                .list_workspace_entries()
                .unwrap()
                .iter()
                .any(|entry| entry.relative_path == "temporary.md"),
            engine.get_object(&note.value.id).unwrap().is_some(),
        ),
        (false, false)
    );
}

#[test]
fn non_managed_markdown_query_is_stable_across_index_rebuild() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(workspace.path().join("draft.md"), "# Draft\n\nBody\n").unwrap();
    let before = engine.list_non_managed_markdown().unwrap();

    engine.rebuild_index().unwrap();
    let after = engine.list_non_managed_markdown().unwrap();

    assert_eq!(after, before);
}

#[cfg(unix)]
#[test]
fn workspace_entries_skip_symlinks() {
    use std::os::unix::fs::symlink;

    let (workspace, _app_data, engine) = engine();
    let outside = tempdir().unwrap();
    std::fs::write(outside.path().join("outside.txt"), "outside").unwrap();
    symlink(outside.path(), workspace.path().join("shortcut")).unwrap();

    let entries = engine.list_workspace_entries().unwrap();

    assert!(
        !entries
            .iter()
            .any(|entry| entry.relative_path == "shortcut")
    );
}

#[test]
fn overlapping_managed_property_edits_conflict_without_writing() {
    let (_workspace, app_data, engine) = engine();
    let created = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Conflict".into(),
            body: "body".into(),
            relative_path: None,
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap();
    let base = created.value;
    let external = WorkspaceEngine::open_with_app_data(engine.root(), app_data.path()).unwrap();
    external
        .update_object(
            &base.id,
            ObjectPatch {
                title: None,
                body: None,
                properties: BTreeMap::from([("status".into(), serde_json::json!("done"))]),
                remove_properties: Vec::new(),
                expected_revision: base.revision.clone(),
            },
        )
        .unwrap();
    let input = ManagedDraftInput {
        id: base.id.clone(),
        base_revision: base.revision.clone(),
        base_title: base.title.clone(),
        base_body: base.body.clone(),
        base_properties: base.properties.clone(),
        local_title: base.title.clone(),
        local_body: base.body.clone(),
        local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
    };
    let result = engine.save_managed_draft(input).unwrap();
    assert!(matches!(result, ManagedDraftResult::Conflict { .. }));
}

#[test]
fn raw_markdown_save_preserves_crlf_and_bom_and_reindexes() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(
        workspace.path().join("scratch.md"),
        b"\xEF\xBB\xBF# Scratch\r\n\r\nlorem\r\n",
    )
    .unwrap();
    engine.reconcile().unwrap();
    let base = engine.read_raw_markdown("scratch.md").unwrap();
    assert!(base.has_bom);
    assert!(base.uses_crlf);
    assert!(base.body.starts_with("# Scratch\n"));
    let saved = engine
        .save_raw_markdown(RawSaveInput {
            relative_path: "scratch.md".into(),
            base_revision: base.revision.clone(),
            base_body: base.body.clone(),
            local_body: "# Scratch\n\nlorem\n\nmore\n".into(),
        })
        .unwrap();
    let RawSaveResult::Saved { current, .. } = saved else {
        panic!("expected saved raw markdown, got {saved:?}");
    };
    assert!(current.uses_crlf);
    assert!(current.has_bom);
    assert_eq!(current.body, "# Scratch\n\nlorem\n\nmore\n");
    let bytes = std::fs::read(workspace.path().join("scratch.md")).unwrap();
    assert_eq!(bytes, b"\xEF\xBB\xBF# Scratch\r\n\r\nlorem\r\n\r\nmore\r\n");
}

#[test]
fn raw_markdown_rejects_traversal_and_non_markdown_paths() {
    let (_workspace, _app_data, engine) = engine();
    assert!(engine.read_raw_markdown("../outside.md").is_err());
    assert!(engine.read_raw_markdown("notes/file.txt").is_err());
    assert!(engine.read_raw_markdown(".noura/history/notes.md").is_err());
}

#[test]
fn markdown_link_fragments_resolve_as_unresolved() {
    let (workspace, _app_data, engine) = engine();
    std::fs::create_dir_all(workspace.path().join("notes")).unwrap();
    std::fs::write(workspace.path().join("notes/target.md"), "# Target\n").unwrap();
    engine.reconcile().unwrap();

    for fragment_only in ["#heading", "|alias", "| alias"] {
        let resolved = engine
            .resolve_markdown_link("notes/source.md", fragment_only)
            .unwrap();
        assert!(
            matches!(resolved, MarkdownLinkTarget::Unresolved),
            "expected `{fragment_only}` to be unresolved, got {resolved:?}"
        );
    }

    // Real files keep resolving with their fragments intact.
    let resolved = engine
        .resolve_markdown_link("notes/source.md", "target#heading")
        .unwrap();
    assert!(matches!(resolved, MarkdownLinkTarget::Markdown { .. }));
}

#[test]
fn markdown_targets_resolve_relative_files_and_reject_workspace_escape() {
    let (workspace, _app_data, engine) = engine();
    std::fs::create_dir_all(workspace.path().join("notes")).unwrap();
    std::fs::write(workspace.path().join("notes/target.md"), "# Target\n").unwrap();
    std::fs::write(workspace.path().join("image.png"), b"png").unwrap();
    engine.reconcile().unwrap();
    let markdown = engine
        .resolve_markdown_link("notes/source.md", "target")
        .unwrap();
    assert!(matches!(markdown, MarkdownLinkTarget::Markdown { .. }));
    let (relative, bytes) = engine
        .read_local_asset("notes/source.md", "../image.png", 1024)
        .unwrap();
    assert_eq!((relative, bytes), ("image.png".into(), b"png".to_vec()));
    assert!(
        engine
            .read_local_asset("notes/source.md", "../../outside.png", 1024)
            .is_err()
    );
}

#[test]
fn raw_markdown_save_reports_managed_identity_after_repair() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(workspace.path().join("repair.md"), "---\nid: broken\n---\n").unwrap();
    engine.reconcile().unwrap();
    let base = engine.read_raw_markdown("repair.md").unwrap();
    let id = new_object_id("note");
    let repaired = format!("---\nid: {id}\ntype: note\n---\n\n# Repaired\n");
    let result = engine
        .save_raw_markdown(RawSaveInput {
            relative_path: "repair.md".into(),
            base_revision: base.revision,
            base_body: base.body,
            local_body: repaired,
        })
        .unwrap();
    let RawSaveResult::Saved { managed_object, .. } = result else {
        panic!("expected repaired raw Markdown to save");
    };
    assert_eq!(managed_object.map(|object| object.id), Some(id));
}

#[test]
fn raw_conflict_resolution_snapshots_each_side() {
    let (workspace, app_data, engine) = engine();
    std::fs::write(workspace.path().join("diary.md"), "day one\n").unwrap();
    engine.reconcile().unwrap();
    let base = engine.read_raw_markdown("diary.md").unwrap();
    let external = WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
    std::fs::write(
        external.root().join("diary.md"),
        "day one\nday two external\n",
    )
    .unwrap();
    external.reconcile().unwrap();
    let saved = engine
        .save_raw_markdown(RawSaveInput {
            relative_path: "diary.md".into(),
            base_revision: base.revision.clone(),
            base_body: base.body.clone(),
            local_body: "day one\nday two local\n".into(),
        })
        .unwrap();
    let RawSaveResult::Conflict { current, .. } = saved else {
        panic!("expected raw conflict, got {saved:?}");
    };
    let adopted = engine
        .resolve_raw_conflict(RawConflictResolveInput {
            relative_path: "diary.md".into(),
            current_revision: current.revision.clone(),
            local_body: "day one\nday two local\n".into(),
            resolution: ConflictResolution::ReplaceExternal,
        })
        .unwrap();
    assert_eq!(adopted.current.body, "day one\nday two local\n");
    let history = workspace.path().join(".noura/history");
    assert!(history.read_dir().unwrap().next().is_some());
}

#[test]
fn managed_conflict_keeps_current_revisions_until_resolution() {
    let (_workspace, _app_data, engine) = engine();
    let created = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Pinned".into(),
            body: "body".into(),
            relative_path: None,
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap();
    let base = created.value;
    let external =
        WorkspaceEngine::open_with_app_data(engine.root(), tempdir().unwrap().path()).unwrap();
    external
        .update_object(
            &base.id,
            ObjectPatch {
                title: None,
                body: None,
                properties: BTreeMap::from([("status".into(), serde_json::json!("done"))]),
                remove_properties: Vec::new(),
                expected_revision: base.revision.clone(),
            },
        )
        .unwrap();
    let input = ManagedDraftInput {
        id: base.id.clone(),
        base_revision: base.revision.clone(),
        base_title: base.title.clone(),
        base_body: base.body.clone(),
        base_properties: base.properties.clone(),
        local_title: base.title.clone(),
        local_body: base.body.clone(),
        local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
    };
    let result = engine.save_managed_draft(input).unwrap();
    assert!(matches!(result, ManagedDraftResult::Conflict { .. }));
}

#[test]
fn managed_conflict_restore_recreates_an_externally_deleted_file() {
    let (workspace, _app_data, engine) = engine();
    let original = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Recover me".into(),
            body: "original body".into(),
            relative_path: Some("tasks/recover-me.md".into()),
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap()
        .value;
    std::fs::remove_file(workspace.path().join(&original.relative_path)).unwrap();
    engine.reconcile().unwrap();

    let restored = engine
        .resolve_managed_conflict(ManagedConflictResolveInput {
            id: original.id.clone(),
            current_revision: original.revision,
            relative_path: original.relative_path.clone(),
            created: original.created.clone(),
            local_title: "Recovered task".into(),
            local_body: "draft survived deletion".into(),
            local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
            resolution: ManagedConflictResolution::ReplaceExternal,
        })
        .unwrap();

    assert_eq!(restored.id, original.id);
    assert_eq!(restored.relative_path, original.relative_path);
    assert_eq!(restored.created, original.created);
    assert_eq!(restored.body, "draft survived deletion");
    assert!(workspace.path().join(&restored.relative_path).is_file());
    assert_eq!(engine.get_object(&restored.id).unwrap(), Some(restored));
}

#[test]
fn managed_conflict_restore_rejects_invalid_created_timestamp() {
    let (workspace, _app_data, engine) = engine();
    let original = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Validate my clock".into(),
            body: "original body".into(),
            relative_path: Some("tasks/recover-timestamp.md".into()),
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap()
        .value;
    std::fs::remove_file(workspace.path().join(&original.relative_path)).unwrap();
    engine.reconcile().unwrap();

    let error = engine
        .resolve_managed_conflict(ManagedConflictResolveInput {
            id: original.id,
            current_revision: original.revision,
            relative_path: original.relative_path,
            created: Some("yesterday".into()),
            local_title: "Recovered task".into(),
            local_body: "draft survived deletion".into(),
            local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
            resolution: ManagedConflictResolution::ReplaceExternal,
        })
        .unwrap_err();

    assert_eq!(error.code, "invalid_timestamp");
}

#[test]
fn managed_conflict_restore_rejects_reserved_directories() {
    let (workspace, _app_data, engine) = engine();
    let original = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Recover me".into(),
            body: "original body".into(),
            relative_path: Some("tasks/recover-reserved.md".into()),
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap()
        .value;
    std::fs::remove_file(workspace.path().join(&original.relative_path)).unwrap();
    engine.reconcile().unwrap();

    let error = engine
        .resolve_managed_conflict(ManagedConflictResolveInput {
            id: original.id,
            current_revision: original.revision,
            relative_path: ".noura/recovered.md".into(),
            created: original.created,
            local_title: "Recovered task".into(),
            local_body: "draft survived deletion".into(),
            local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
            resolution: ManagedConflictResolution::ReplaceExternal,
        })
        .unwrap_err();

    assert_eq!(error.code, "reserved_path");
}

#[test]
fn managed_conflict_restore_requires_markdown_extension() {
    let (workspace, _app_data, engine) = engine();
    let original = engine
        .create_object(CreateObjectInput {
            object_type: "task".into(),
            title: "Recover me".into(),
            body: "original body".into(),
            relative_path: Some("tasks/recover-extension.md".into()),
            properties: BTreeMap::from([("status".into(), serde_json::json!("todo"))]),
        })
        .unwrap()
        .value;
    std::fs::remove_file(workspace.path().join(&original.relative_path)).unwrap();
    engine.reconcile().unwrap();

    let error = engine
        .resolve_managed_conflict(ManagedConflictResolveInput {
            id: original.id,
            current_revision: original.revision,
            relative_path: "tasks/recovered.txt".into(),
            created: original.created,
            local_title: "Recovered task".into(),
            local_body: "draft survived deletion".into(),
            local_properties: BTreeMap::from([("status".into(), serde_json::json!("in-progress"))]),
            resolution: ManagedConflictResolution::ReplaceExternal,
        })
        .unwrap_err();

    assert_eq!(error.code, "unsupported_extension");
}

#[test]
fn plugin_state_is_namespaced_per_plugin_and_per_workspace() {
    let (workspace, app_data, engine) = engine();
    engine
        .plugin_state_set("tasks", "view", serde_json::json!("board"))
        .unwrap();
    engine
        .plugin_state_set("calendar", "view", serde_json::json!("week"))
        .unwrap();
    assert_eq!(
        engine.plugin_state_get("tasks", "view").unwrap(),
        Some(serde_json::json!("board"))
    );
    assert_eq!(
        engine.plugin_state_get("calendar", "view").unwrap(),
        Some(serde_json::json!("week"))
    );
    assert_eq!(engine.plugin_state_get("tasks", "missing").unwrap(), None);
    assert!(engine.plugin_state_delete("tasks", "view").unwrap());
    assert_eq!(engine.plugin_state_get("tasks", "view").unwrap(), None);

    // Reopening the same workspace on the same device shares local state.
    let same_device =
        WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
    assert_eq!(
        same_device.plugin_state_get("calendar", "view").unwrap(),
        Some(serde_json::json!("week"))
    );

    // A different device keeps its own disposable index.
    let other_device =
        WorkspaceEngine::open_with_app_data(workspace.path(), tempdir().unwrap().path()).unwrap();
    assert_eq!(
        other_device.plugin_state_get("calendar", "view").unwrap(),
        None
    );
}

#[test]
fn plugin_state_rejects_invalid_plugin_ids_and_accepts_hyphenated_ids() {
    let (_workspace, _app_data, engine) = engine();
    for (plugin_id, key) in [
        ("", "view"),
        ("Tasks", "view"),
        ("1tasks", "view"),
        ("tasks with spaces", "view"),
        ("tasks", ""),
        ("tasks", "\n"),
    ] {
        assert!(
            engine
                .plugin_state_set(plugin_id, key, serde_json::json!("x"))
                .is_err(),
            "expected rejection for {plugin_id:?} / {key:?}"
        );
    }
    let key = "key/with:symbols";
    engine
        .plugin_state_set("tasks-v2", key, serde_json::json!({"a": 1}))
        .unwrap();
    assert_eq!(
        engine.plugin_state_get("tasks-v2", key).unwrap(),
        Some(serde_json::json!({"a": 1}))
    );
}

#[test]
fn manifest_update_rewrites_enabled_plugins_durably_and_in_memory() {
    let (workspace, _app_data, engine) = engine();
    let before = engine.read_manifest().unwrap();
    let updated = engine
        .manifest_update(ManifestUpdateInput {
            enabled_plugins: Some(vec!["tasks".into(), "notes".into(), "tasks".into()]),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(
        updated.enabled_plugins,
        vec!["notes".to_owned(), "tasks".to_owned()]
    );
    assert_ne!(updated.updated, before.updated);
    assert_eq!(updated.id, before.id);

    let on_disk = std::fs::read_to_string(workspace.path().join("workspace.yaml")).unwrap();
    assert!(on_disk.contains("enabled_plugins:"));
    assert!(on_disk.contains("- notes"));
    assert_eq!(
        engine.read_manifest().unwrap().enabled_plugins,
        updated.enabled_plugins
    );

    let error = engine
        .manifest_update(ManifestUpdateInput {
            enabled_plugins: Some(vec!["calendar".into()]),
            expected_updated: Some(before.updated),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error.code, "manifest_conflict");
}

#[test]
fn manifest_update_rejects_invalid_fields_and_leaves_no_op_patches_untouched() {
    let (_workspace, _app_data, engine) = engine();
    let unchanged = engine
        .manifest_update(ManifestUpdateInput::default())
        .unwrap();
    let error = engine
        .manifest_update(ManifestUpdateInput {
            enabled_plugins: Some(vec!["".into()]),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error.code, "invalid_plugin_id");
    let error = engine
        .manifest_update(ManifestUpdateInput {
            name: Some("   ".into()),
            ..Default::default()
        })
        .unwrap_err();
    assert_eq!(error.code, "workspace_name_required");
    assert_eq!(
        engine.read_manifest().unwrap().enabled_plugins,
        unchanged.enabled_plugins
    );
}

#[test]
fn manifest_read_reflects_external_edits_to_workspace_yaml() {
    let (workspace, _app_data, engine) = engine();
    let bytes = std::fs::read_to_string(workspace.path().join("workspace.yaml")).unwrap();
    assert!(bytes.contains("name: Domain tests"));
    std::fs::write(
        workspace.path().join("workspace.yaml"),
        bytes.replace("name: Domain tests", "name: External name"),
    )
    .unwrap();
    assert_eq!(engine.read_manifest().unwrap().name, "External name");
    // The in-memory snapshot follows the file after a durable update.
    engine
        .manifest_update(ManifestUpdateInput {
            name: Some("Renamed".into()),
            ..Default::default()
        })
        .unwrap();
    assert_eq!(engine.read_manifest().unwrap().name, "Renamed");
}
