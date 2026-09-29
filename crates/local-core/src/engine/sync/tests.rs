use super::*;
use tempfile::TempDir;

#[test]
fn wrapped_keys_survive_index_rebuild_and_refuse_replacement_or_wrong_context() {
    use crate::sync::SyncCredentials;
    use std::cell::RefCell;
    use zeroize::Zeroizing;
    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, String>>);
    impl SyncCredentials for Memory {
        fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
            self.0
                .borrow()
                .get(reference)
                .cloned()
                .map(Zeroizing::new)
                .ok_or_else(|| invalid("missing"))
        }
        fn write(&self, reference: &str, value: &str) -> Result<()> {
            self.0.borrow_mut().insert(reference.into(), value.into());
            Ok(())
        }
    }
    let (_dir, engine) = engine();
    let store = Memory::default();
    let device = DeviceKeys::create(&store).unwrap();
    let key = ObjectKey::generate();
    let public = device.signer().public_key();
    let envelope = device
        .wrap_key(
            &engine.manifest().id,
            "object",
            1,
            device.device_id(),
            &device.recipient(),
            &key,
        )
        .unwrap();
    engine.sync_store_key(&envelope, &device, &public).unwrap();
    engine.sync_store_key(&envelope, &device, &public).unwrap();
    engine.rebuild_index().unwrap();
    let restored = engine.sync_load_key("object", 1, &device, &public).unwrap();
    let operation = device
        .signer()
        .seal(
            &key,
            &engine.manifest().id,
            "object",
            device.device_id(),
            1,
            b"secret",
        )
        .unwrap();
    assert_eq!(&**operation.open(&restored, &public).unwrap(), b"secret");
    let other = device
        .wrap_key(
            &engine.manifest().id,
            "object",
            1,
            device.device_id(),
            &device.recipient(),
            &ObjectKey::generate(),
        )
        .unwrap();
    assert!(engine.sync_store_key(&other, &device, &public).is_err());
    assert!(engine.sync_load_key("object", 2, &device, &public).is_err());
    assert!(
        engine
            .sync_load_key("../escape", 1, &device, &public)
            .is_err()
    );
    let bytes = std::fs::read(
        engine
            .root
            .join(key_path("object", 1, device.device_id()).unwrap()),
    )
    .unwrap();
    assert!(
        !String::from_utf8(bytes)
            .unwrap()
            .contains(&STANDARD.encode(key.secret()))
    );
    std::fs::write(engine.root.join("owned.md"), b"first owned file").unwrap();
    let captured = engine
        .sync_capture_owned_file("owned.md", &device, 1)
        .unwrap()
        .unwrap();
    let captured_key = engine
        .sync_load_key(&captured.object_id, 1, &device, &public)
        .unwrap();
    assert!(captured.open(&captured_key, &public).is_ok());
    let own_path = engine
        .root
        .join(key_path(&captured.object_id, 1, device.device_id()).unwrap());
    let original_key_bytes = std::fs::read(&own_path).unwrap();
    std::fs::write(engine.root.join("owned.md"), b"second owned file").unwrap();
    let second = engine
        .sync_capture_owned_file("owned.md", &device, 1)
        .unwrap()
        .unwrap();
    assert_eq!(captured.object_id, second.object_id);
    assert_eq!(std::fs::read(&own_path).unwrap(), original_key_bytes);
    assert!(second.open(&captured_key, &public).is_ok());
    std::fs::remove_file(&own_path).unwrap();
    std::fs::write(engine.root.join("owned.md"), b"third owned file").unwrap();
    assert_eq!(
        engine
            .sync_capture_owned_file("owned.md", &device, 1)
            .unwrap_err()
            .code,
        "sync_key_required"
    );
    assert!(!own_path.exists());
}

fn engine() -> (TempDir, WorkspaceEngine) {
    let dir = TempDir::new().unwrap();
    let engine = WorkspaceEngine::create_with_app_data(
        dir.path().join("workspace"),
        "Sync test",
        dir.path().join("app"),
    )
    .unwrap();
    (dir, engine)
}

fn seal(
    engine: &WorkspaceEngine,
    signer: &SigningIdentity,
    key: &ObjectKey,
    change: &FileChange,
) -> EncryptedOperation {
    signer
        .seal(
            key,
            &engine.manifest().id,
            "file_a",
            "device_a",
            1,
            &serde_json::to_vec(change).unwrap(),
        )
        .unwrap()
}

fn change(path: &str, content: Option<&[u8]>, base: Option<&[u8]>) -> FileChange {
    FileChange {
        version: 1,
        path: path.into(),
        previous_path: None,
        content: content.map(|v| STANDARD.encode(v)),
        base_revision: base.map(markdown::revision),
        accepted_revisions: None,
        blob: None,
    }
}

#[test]
fn duplicate_canonical_ids_block_capture_even_before_cataloging() {
    let (_dir, engine) = engine();
    let note = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Original".into(),
            body: "canonical".into(),
            relative_path: Some("original.md".into()),
            properties: BTreeMap::new(),
        })
        .unwrap()
        .value;
    let bytes = std::fs::read(engine.root.join("original.md")).unwrap();
    std::fs::write(engine.root.join("duplicate.md"), &bytes).unwrap();
    let key = ObjectKey::generate();
    let signer = SigningIdentity::generate();
    assert_eq!(
        engine
            .sync_capture_file("original.md", &key, &signer, "device", 1)
            .unwrap_err()
            .code,
        "sync_duplicate_identity"
    );
    assert!(engine.sync_outbox().unwrap().is_empty());
    let incoming = signer
        .seal(
            &key,
            &engine.manifest().id,
            &note.id,
            "device",
            1,
            &serde_json::to_vec(&change("remote.md", Some(&bytes), None)).unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine
            .sync_apply_file(&incoming, &key, &signer.public_key())
            .unwrap(),
        ApplyOutcome::Conflict
    );
    assert!(!engine.root.join("remote.md").exists());
    assert_eq!(
        std::fs::read(engine.root.join("original.md")).unwrap(),
        bytes
    );
}

#[cfg(unix)]
#[test]
fn identity_scan_does_not_reinterpret_literal_backslashes_as_directories() {
    let (_dir, engine) = engine();
    std::fs::write(engine.root.join("folder\\note.md"), b"literal filename").unwrap();
    let key = ObjectKey::generate();
    let signer = SigningIdentity::generate();
    let operation = seal(
        &engine,
        &signer,
        &key,
        &change("incoming.md", Some(b"new"), None),
    );
    assert_eq!(
        engine
            .sync_apply_file(&operation, &key, &signer.public_key())
            .unwrap_err()
            .code,
        "sync_unsupported_path"
    );
    assert!(!engine.root.join("incoming.md").exists());
    assert_eq!(engine.sync_cursor().unwrap(), "0");
}

#[cfg(unix)]
#[test]
fn sync_paths_reject_special_files_and_canonical_internal_targets() {
    let (_dir, engine) = engine();
    assert!(
        std::process::Command::new("mkfifo")
            .arg(engine.root.join("pipe.md"))
            .status()
            .unwrap()
            .success()
    );
    assert_eq!(
        engine.sync_path("pipe.md").unwrap_err().code,
        "sync_unsupported_path"
    );
    assert_eq!(
        engine
            .sync_file_path(".noura/sync/state.json")
            .unwrap_err()
            .code,
        "sync_unsafe_path"
    );
    assert_eq!(
        engine.sync_file_path("workspace.yaml").unwrap(),
        engine.root.join("workspace.yaml")
    );
}

#[test]
fn restart_retains_ordered_ciphertext_and_external_edits() {
    let (dir, engine) = engine();
    let key = ObjectKey::generate();
    let signer = SigningIdentity::generate();
    std::fs::write(engine.root.join("note.md"), b"first").unwrap();
    let first = engine
        .sync_capture_file("note.md", &key, &signer, "device", 1)
        .unwrap()
        .unwrap();
    std::fs::write(engine.root.join("note.md"), b"external edit").unwrap();
    let second = engine
        .sync_capture_file("note.md", &key, &signer, "device", 1)
        .unwrap()
        .unwrap();
    assert_eq!(first.object_id, second.object_id);
    let root = engine.root.clone();
    drop(engine);
    let reopened = WorkspaceEngine::open_with_app_data(root, dir.path().join("app")).unwrap();
    assert_eq!(
        reopened.sync_outbox().unwrap(),
        vec![first.clone(), second.clone()]
    );
    reopened.sync_acknowledge(&first, "1").unwrap();
    assert_eq!(reopened.sync_outbox().unwrap(), vec![second]);
    assert!(
        reopened
            .sync_capture_file("note.md", &key, &signer, "device", 1)
            .unwrap()
            .is_none()
    );
}

#[test]
fn cursor_requires_durable_apply_and_replay_does_not_overwrite_external_edit() {
    let (_dir, engine) = engine();
    let key = ObjectKey::generate();
    let signer = SigningIdentity::generate();
    let op = seal(
        &engine,
        &signer,
        &key,
        &change("note.md", Some(b"remote"), None),
    );
    assert!(
        engine
            .sync_checkpoint("0", "1", std::slice::from_ref(&op))
            .is_err()
    );
    assert_eq!(engine.sync_cursor().unwrap(), "0");
    assert_eq!(
        engine
            .sync_apply_file(&op, &key, &signer.public_key())
            .unwrap(),
        ApplyOutcome::Applied
    );
    std::fs::write(engine.root.join("note.md"), b"external edit").unwrap();
    assert_eq!(
        engine
            .sync_apply_file(&op, &key, &signer.public_key())
            .unwrap(),
        ApplyOutcome::Applied
    );
    assert_eq!(
        std::fs::read(engine.root.join("note.md")).unwrap(),
        b"external edit"
    );
    engine.sync_checkpoint("0", "1", &[op]).unwrap();
    assert_eq!(engine.sync_cursor().unwrap(), "1");
    assert!(engine.sync_checkpoint("0", "2", &[]).is_err());
}

#[test]
fn delete_versus_external_edit_preserves_both_versions_and_completes_page() {
    let (_dir, engine) = engine();
    let key = ObjectKey::generate();
    let signer = SigningIdentity::generate();
    let first = seal(
        &engine,
        &signer,
        &key,
        &change("note.md", Some(b"base"), None),
    );
    engine
        .sync_apply_file(&first, &key, &signer.public_key())
        .unwrap();
    std::fs::write(engine.root.join("note.md"), b"edited locally").unwrap();
    let deletion = seal(
        &engine,
        &signer,
        &key,
        &change("note.md", None, Some(b"base")),
    );
    assert_eq!(
        engine
            .sync_apply_file(&deletion, &key, &signer.public_key())
            .unwrap(),
        ApplyOutcome::Conflict
    );
    assert_eq!(
        std::fs::read(engine.root.join("note.md")).unwrap(),
        b"edited locally"
    );
    assert!(
        engine
            .root
            .join(format!(
                ".noura/sync/conflicts/{}.json",
                deletion.operation_id
            ))
            .is_file()
    );
    engine
        .sync_checkpoint("0", "2", &[first, deletion])
        .unwrap();
}

#[test]
fn interrupted_move_replays_after_destination_write_and_after_source_removal() {
    for remove_source in [false, true] {
        let (_dir, engine) = engine();
        let key = ObjectKey::generate();
        let signer = SigningIdentity::generate();
        let first = seal(
            &engine,
            &signer,
            &key,
            &change("old.md", Some(b"base"), None),
        );
        engine
            .sync_apply_file(&first, &key, &signer.public_key())
            .unwrap();
        let mut moved = change("new.md", Some(b"base"), Some(b"base"));
        moved.previous_path = Some("old.md".into());
        let op = seal(&engine, &signer, &key, &moved);
        // Simulate a process interruption after canonical write, before receipt.
        engine
            .sync_write_once(&format!(".noura/sync/inbox/{}.json", op.operation_id), &op)
            .unwrap();
        std::fs::write(engine.root.join("new.md"), b"base").unwrap();
        if remove_source {
            std::fs::remove_file(engine.root.join("old.md")).unwrap();
        }
        assert_eq!(
            engine
                .sync_apply_file(&op, &key, &signer.public_key())
                .unwrap(),
            ApplyOutcome::Applied
        );
        assert!(!engine.root.join("old.md").exists());
        assert_eq!(std::fs::read(engine.root.join("new.md")).unwrap(), b"base");
    }
}

#[test]
fn unsafe_paths_and_unbound_updates_cannot_modify_workspace_files() {
    let (_dir, engine) = engine();
    let key = ObjectKey::generate();
    let signer = SigningIdentity::generate();
    for path in [
        "../escape.md",
        "/absolute.md",
        ".noura/keys",
        "a\\b.md",
        "C:/file",
        "a/../b",
        "a//b",
    ] {
        let op = seal(&engine, &signer, &key, &change(path, Some(b"bad"), None));
        assert!(
            engine
                .sync_apply_file(&op, &key, &signer.public_key())
                .is_err(),
            "{path}"
        );
    }
    std::fs::write(engine.root.join("private.txt"), b"private").unwrap();
    let op = seal(
        &engine,
        &signer,
        &key,
        &change("private.txt", None, Some(b"private")),
    );
    assert_eq!(
        engine
            .sync_apply_file(&op, &key, &signer.public_key())
            .unwrap(),
        ApplyOutcome::Conflict
    );
    assert_eq!(
        std::fs::read(engine.root.join("private.txt")).unwrap(),
        b"private"
    );
}

#[cfg(unix)]
#[test]
fn dangling_and_parent_symlinks_block_incoming_writes() {
    let (dir, engine) = engine();
    let key = ObjectKey::generate();
    let signer = SigningIdentity::generate();
    std::os::unix::fs::symlink(dir.path().join("missing"), engine.root.join("link.md")).unwrap();
    std::os::unix::fs::symlink(dir.path(), engine.root.join("outside")).unwrap();
    for path in ["link.md", "outside/escape.md"] {
        let op = seal(&engine, &signer, &key, &change(path, Some(b"bad"), None));
        assert!(
            engine
                .sync_apply_file(&op, &key, &signer.public_key())
                .is_err()
        );
    }
    assert!(!dir.path().join("escape.md").exists());
}

#[test]
fn failed_canonical_write_leaves_inbox_and_cursor_without_receipt() {
    let (_dir, engine) = engine();
    let key = ObjectKey::generate();
    let signer = SigningIdentity::generate();
    std::fs::write(engine.root.join("parent"), b"not a directory").unwrap();
    let op = seal(
        &engine,
        &signer,
        &key,
        &change("parent/note.md", Some(b"remote"), None),
    );
    assert!(
        engine
            .sync_apply_file(&op, &key, &signer.public_key())
            .is_err()
    );
    assert!(
        engine
            .root
            .join(format!(".noura/sync/inbox/{}.json", op.operation_id))
            .exists()
    );
    assert!(engine.sync_checkpoint("0", "1", &[op]).is_err());
    assert_eq!(engine.sync_cursor().unwrap(), "0");
}

#[test]
fn malformed_journal_is_rejected_instead_of_resetting_sync_state() {
    let (_dir, engine) = engine();
    std::fs::create_dir_all(engine.root.join(".noura/sync")).unwrap();
    std::fs::write(engine.root.join(STATE_PATH), b"{broken").unwrap();
    assert!(engine.sync_cursor().is_err());
}
