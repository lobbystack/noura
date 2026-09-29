use super::*;
use crate::sync::{
    AccessMember, AccessObject, AccessPolicy, CheckpointContent, DocumentDescriptor,
    EncryptedCheckpoint, SyncCredentials, WorkspaceRole,
};
use zeroize::Zeroizing;
struct Credentials;
impl SyncCredentials for Credentials {
    fn read(&self, _: &str) -> Result<Zeroizing<String>> {
        Err(invalid("test_missing"))
    }
    fn write(&self, _: &str, _: &str) -> Result<()> {
        Ok(())
    }
}

fn copy_tree(source: &Path, destination: &Path) {
    std::fs::create_dir_all(destination).unwrap();
    for entry in std::fs::read_dir(source).unwrap() {
        let entry = entry.unwrap();
        let target = destination.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

struct Fixture {
    directory: tempfile::TempDir,
    engine: WorkspaceEngine,
    device: DeviceKeys,
    secrets: SyncSecrets,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let engine = WorkspaceEngine::create_with_app_data(
        directory.path().join("workspace"),
        "Collaboration",
        directory.path().join("app"),
    )
    .unwrap();
    engine.enable_sync_plugin().unwrap();
    let device = DeviceKeys::create(&Credentials).unwrap();
    let key = ObjectKey::generate();
    let policy = AccessPolicy::sign(
        &engine.manifest().id,
        "1",
        None,
        &device,
        vec![AccessMember {
            account_id: "owner".into(),
            role: WorkspaceRole::Owner,
        }],
        vec![AccessObject {
            object_id: "text".into(),
            epoch: 2,
            grants: vec![],
            envelopes: vec![],
            document: Some(DocumentDescriptor {
                generation: "generation-one".into(),
                mode: DocumentMode::Text,
            }),
        }],
    )
    .unwrap();
    engine
        .sync_accept_access_policy("0", None, &policy)
        .unwrap();
    let bytes = b"\xef\xbb\xbfA\xf0\x9f\x98\x80\r\nsecond\r\n";
    let content = CheckpointContent {
        version: 1,
        object_id: "text".into(),
        generation: "generation-one".into(),
        content_revision: Some(markdown::revision(bytes)),
        change: FileChange {
            version: 1,
            path: "note.md".into(),
            previous_path: None,
            base_revision: None,
            content: Some(STANDARD.encode(bytes)),
            accepted_revisions: None,
            blob: None,
        },
    };
    let checkpoint = EncryptedCheckpoint::seal(&device, &key, &policy, "0", &content).unwrap();
    let secrets = SyncSecrets {
        objects: BTreeMap::from([(("text".into(), 2), key)]),
        trusted_devices: BTreeMap::from([(
            device.device_id().into(),
            device.signer().public_key(),
        )]),
        authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
        ..Default::default()
    };
    engine
        .collaboration_install_checkpoint(
            &checkpoint,
            secrets.objects.get(&("text".into(), 2)).unwrap(),
            &device.signer().public_key(),
            &device,
            &secrets,
        )
        .unwrap();
    Fixture {
        directory,
        engine,
        device,
        secrets,
    }
}

fn managed_fixture() -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let engine = WorkspaceEngine::create_with_app_data(
        directory.path().join("workspace"),
        "Managed collaboration",
        directory.path().join("app"),
    )
    .unwrap();
    engine.enable_sync_plugin().unwrap();
    let device = DeviceKeys::create(&Credentials).unwrap();
    let key = ObjectKey::generate();
    let policy = AccessPolicy::sign(
        &engine.manifest().id,
        "1",
        None,
        &device,
        vec![AccessMember {
            account_id: "owner".into(),
            role: WorkspaceRole::Owner,
        }],
        vec![AccessObject {
            object_id: "note_01j00000000000000000000000".into(),
            epoch: 2,
            grants: vec![],
            envelopes: vec![],
            document: Some(DocumentDescriptor {
                generation: "generation-one".into(),
                mode: DocumentMode::Text,
            }),
        }],
    )
    .unwrap();
    engine
        .sync_accept_access_policy("0", None, &policy)
        .unwrap();
    let object = WorkspaceObject {
        id: "note_01j00000000000000000000000".into(),
        object_type: "note".into(),
        title: "Original".into(),
        body: "Body\n".into(),
        relative_path: "note.md".into(),
        revision: String::new(),
        created: Some("2026-09-07T00:00:00Z".into()),
        updated: Some("2026-09-07T00:00:00Z".into()),
        properties: BTreeMap::from([("status".into(), serde_json::Value::String("todo".into()))]),
    };
    let bytes = markdown::serialize_object(&object).unwrap();
    let content = CheckpointContent {
        version: 1,
        object_id: "note_01j00000000000000000000000".into(),
        generation: "generation-one".into(),
        content_revision: Some(markdown::revision(&bytes)),
        change: FileChange {
            version: 1,
            path: "note.md".into(),
            previous_path: None,
            base_revision: None,
            content: Some(STANDARD.encode(&bytes)),
            accepted_revisions: None,
            blob: None,
        },
    };
    let checkpoint = EncryptedCheckpoint::seal(&device, &key, &policy, "0", &content).unwrap();
    let secrets = SyncSecrets {
        objects: BTreeMap::from([(("note_01j00000000000000000000000".into(), 2), key)]),
        trusted_devices: BTreeMap::from([(
            device.device_id().into(),
            device.signer().public_key(),
        )]),
        authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
        ..Default::default()
    };
    engine
        .collaboration_install_checkpoint(
            &checkpoint,
            secrets
                .objects
                .get(&("note_01j00000000000000000000000".into(), 2))
                .unwrap(),
            &device.signer().public_key(),
            &device,
            &secrets,
        )
        .unwrap();
    Fixture {
        directory,
        engine,
        device,
        secrets,
    }
}

fn open(f: &Fixture) -> CollaborationSession {
    f.engine
        .collaboration_open(
            CollaborationOpenInput {
                relative_path: "note.md".into(),
            },
            &f.device,
            &f.secrets,
        )
        .unwrap()
        .unwrap()
}

fn edit(session: &CollaborationSession, text: &str) -> CollaborationSubmitInput {
    let (_, update) = TextDocument::restore(&session.update)
        .unwrap()
        .replace_text(text)
        .unwrap();
    CollaborationSubmitInput {
        session_id: session.session_id.clone(),
        generation: session.generation.clone(),
        batch_id: uuid::Uuid::new_v4().to_string(),
        updates: vec![update],
    }
}

#[test]
fn checkpoint_blob_round_trip_installs_large_text_without_inline_plaintext() {
    let directory = tempfile::tempdir().unwrap();
    let engine = WorkspaceEngine::create_with_app_data(
        directory.path().join("workspace"),
        "Large checkpoint",
        directory.path().join("app"),
    )
    .unwrap();
    let device = DeviceKeys::create(&Credentials).unwrap();
    let public = device.signer().public_key();
    let bytes = vec![b'a'; MAX_TEXT_BYTES];
    std::fs::write(engine.root.join("large.txt"), &bytes).unwrap();
    let mut capture_secrets = SyncSecrets {
        trusted_devices: BTreeMap::from([(device.device_id().into(), public.clone())]),
        authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
        ..Default::default()
    };
    engine
        .sync_capture_workspace(&device, &mut capture_secrets)
        .unwrap();
    let object_id = engine.sync_outbox().unwrap()[0].object_id.clone();
    let generation = "large-generation";
    let key = ObjectKey::generate();
    let (content, blob, mode) = engine
        .collaboration_prepare_checkpoint_content(&object_id, generation, &key)
        .unwrap();
    assert_eq!(mode, DocumentMode::Text);
    let blob = blob.expect("large checkpoint must use a blob");
    assert!(content.change.content.is_none());
    assert_eq!(content.change.blob.as_ref(), Some(&blob));
    let policy = AccessPolicy::sign(
        &engine.manifest().id,
        "1",
        None,
        &device,
        vec![AccessMember {
            account_id: "owner".into(),
            role: WorkspaceRole::Owner,
        }],
        vec![AccessObject {
            object_id: object_id.clone(),
            epoch: 2,
            grants: vec![],
            envelopes: vec![],
            document: Some(DocumentDescriptor {
                generation: generation.into(),
                mode: DocumentMode::Text,
            }),
        }],
    )
    .unwrap();
    engine
        .sync_accept_access_policy("0", None, &policy)
        .unwrap();
    let checkpoint = EncryptedCheckpoint::seal(&device, &key, &policy, "0", &content).unwrap();
    let secrets = SyncSecrets {
        objects: BTreeMap::from([((object_id.clone(), 2), key)]),
        trusted_devices: BTreeMap::from([(device.device_id().into(), public.clone())]),
        authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
        ..Default::default()
    };
    engine
        .collaboration_install_checkpoint(
            &checkpoint,
            secrets.objects.get(&(object_id.clone(), 2)).unwrap(),
            &public,
            &device,
            &secrets,
        )
        .unwrap();
    assert_eq!(std::fs::read(engine.root.join("large.txt")).unwrap(), bytes);
    assert!(
        engine
            .collaboration_generation_is_installed(&object_id, generation)
            .unwrap()
    );
}

#[test]
fn large_live_updates_use_durable_encrypted_blobs_and_apply_on_a_peer() {
    let mut f = fixture();
    f.secrets
        .historical_workspace_writers
        .insert("1".into(), BTreeSet::from([f.device.device_id().into()]));
    let peer_root = f.directory.path().join("peer");
    copy_tree(&f.engine.root, &peer_root);
    let peer = WorkspaceEngine::open_with_app_data(&peer_root, f.directory.path().join("peer-app"))
        .unwrap();
    let session = open(&f);
    let text = "large 😀\n".repeat(100_000);
    f.engine
        .collaboration_submit(edit(&session, &text), &f.device, &f.secrets)
        .unwrap();
    let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
    assert!(serde_json::to_vec(&operation).unwrap().len() < 1024 * 1024);
    let key = f.secrets.objects.get(&("text".into(), 2)).unwrap();
    let blob = f
        .engine
        .collaboration_operation_blob(&operation, key, &f.device.signer().public_key())
        .unwrap()
        .expect("large live update must use a blob");
    let mut source = f.engine.sync_blob_file(&blob, false).unwrap();
    blob.verify(&mut source).unwrap();
    source.rewind().unwrap();
    let mut destination = peer.sync_blob_file(&blob, true).unwrap();
    std::io::copy(&mut source, &mut destination).unwrap();
    destination.sync_all().unwrap();
    drop(destination);
    assert_eq!(
        peer.collaboration_apply_remote(
            &operation,
            key,
            &f.device.signer().public_key(),
            &f.secrets,
            "1",
        )
        .unwrap(),
        ApplyOutcome::Applied
    );
    assert_eq!(
        TextDocument::restore(&peer.collaboration_state("text").unwrap().unwrap().update)
            .unwrap()
            .text(),
        text
    );
}

#[test]
fn unsupported_text_rotates_into_attachment_mode_and_archives_old_operations() {
    let directory = tempfile::tempdir().unwrap();
    let engine = WorkspaceEngine::create_with_app_data(
        directory.path().join("workspace"),
        "Attachment checkpoint",
        directory.path().join("app"),
    )
    .unwrap();
    let device = DeviceKeys::create(&Credentials).unwrap();
    let public = device.signer().public_key();
    let bytes = vec![0xff; MAX_TEXT_BYTES + 1];
    std::fs::write(engine.root.join("binary.dat"), &bytes).unwrap();
    let mut capture_secrets = SyncSecrets {
        trusted_devices: BTreeMap::from([(device.device_id().into(), public.clone())]),
        authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
        ..Default::default()
    };
    engine
        .sync_capture_workspace(&device, &mut capture_secrets)
        .unwrap();
    let operation = engine.sync_outbox().unwrap()[0].clone();
    let object_id = operation.object_id.clone();
    let generation = "attachment-generation";
    let key = ObjectKey::generate();
    let (content, blob, mode) = engine
        .collaboration_prepare_checkpoint_content(&object_id, generation, &key)
        .unwrap();
    assert_eq!(mode, DocumentMode::Attachment);
    assert!(blob.is_some());
    assert!(content.change.content.is_none());
    let policy = AccessPolicy::sign(
        &engine.manifest().id,
        "1",
        None,
        &device,
        vec![AccessMember {
            account_id: "owner".into(),
            role: WorkspaceRole::Owner,
        }],
        vec![AccessObject {
            object_id: object_id.clone(),
            epoch: 2,
            grants: vec![],
            envelopes: vec![],
            document: Some(DocumentDescriptor {
                generation: generation.into(),
                mode,
            }),
        }],
    )
    .unwrap();
    engine
        .sync_accept_access_policy("0", None, &policy)
        .unwrap();
    let checkpoint = EncryptedCheckpoint::seal(&device, &key, &policy, "0", &content).unwrap();
    let mut secrets = SyncSecrets {
        objects: BTreeMap::from([((object_id.clone(), 2), key)]),
        trusted_devices: BTreeMap::from([(device.device_id().into(), public.clone())]),
        authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
        ..Default::default()
    };
    engine
        .collaboration_install_checkpoint(
            &checkpoint,
            secrets.objects.get(&(object_id.clone(), 2)).unwrap(),
            &public,
            &device,
            &secrets,
        )
        .unwrap();
    assert_eq!(
        std::fs::read(engine.root.join("binary.dat")).unwrap(),
        bytes
    );
    assert!(engine.sync_outbox().unwrap().is_empty());
    assert!(
        engine
            .collaboration_generation_is_installed(&object_id, generation)
            .unwrap()
    );
    assert!(
        engine
            .collaboration_open(
                CollaborationOpenInput {
                    relative_path: "binary.dat".into(),
                },
                &device,
                &secrets,
            )
            .unwrap()
            .is_none()
    );
    let mut edited = bytes;
    edited[0] = 0xfe;
    std::fs::write(engine.root.join("binary.dat"), &edited).unwrap();
    engine
        .sync_capture_workspace(&device, &mut secrets)
        .unwrap();
    let operations = engine.sync_outbox().unwrap();
    assert_eq!(operations.len(), 1);
    let operation = &operations[0];
    assert_eq!(operation.version, 2);
    assert_eq!(operation.generation.as_deref(), Some(generation));
    assert_eq!(operation.kind, Some(OperationKind::File));
    assert_eq!(operation.epoch, 2);
    let plaintext = operation
        .open(
            secrets.objects.get(&(object_id.clone(), 2)).unwrap(),
            &public,
        )
        .unwrap();
    let change: FileChange = serde_json::from_slice(&plaintext).unwrap();
    assert!(change.content.is_none());
    assert!(change.blob.is_some());
    engine.sync_acknowledge(operation, "1").unwrap();

    let inline_edit = vec![0xfd; 1024];
    std::fs::write(engine.root.join("binary.dat"), &inline_edit).unwrap();
    engine
        .sync_capture_workspace(&device, &mut secrets)
        .unwrap();
    let operations = engine.sync_outbox().unwrap();
    assert_eq!(operations.len(), 1);
    let operation = &operations[0];
    assert_eq!(operation.version, 2);
    assert_eq!(operation.generation.as_deref(), Some(generation));
    assert_eq!(operation.kind, Some(OperationKind::File));
    let plaintext = operation
        .open(
            secrets.objects.get(&(object_id.clone(), 2)).unwrap(),
            &public,
        )
        .unwrap();
    let change: FileChange = serde_json::from_slice(&plaintext).unwrap();
    assert!(change.blob.is_none());
    assert!(change.content.is_some());
}

#[test]
fn writer_object_activation_is_durable_and_installs_its_fresh_generation() {
    let directory = tempfile::tempdir().unwrap();
    let engine = WorkspaceEngine::create_with_app_data(
        directory.path().join("workspace"),
        "Object activation",
        directory.path().join("app"),
    )
    .unwrap();
    let device = DeviceKeys::create(&Credentials).unwrap();
    let public = device.signer().public_key();
    let policy = AccessPolicy::sign(
        &engine.manifest().id,
        "1",
        None,
        &device,
        vec![AccessMember {
            account_id: "writer".into(),
            role: WorkspaceRole::Owner,
        }],
        vec![],
    )
    .unwrap();
    engine
        .sync_accept_access_policy("0", None, &policy)
        .unwrap();
    engine
        .sync_record_access_authorization(
            "1",
            &BTreeSet::from([device.device_id().into()]),
            &BTreeSet::new(),
        )
        .unwrap();
    std::fs::write(engine.root.join("created.txt"), b"created by a writer").unwrap();
    let mut secrets = SyncSecrets {
        trusted_devices: BTreeMap::from([(device.device_id().into(), public.clone())]),
        authorized_workspace_writers: BTreeSet::from([device.device_id().into()]),
        historical_workspace_writers: BTreeMap::from([(
            "1".into(),
            BTreeSet::from([device.device_id().into()]),
        )]),
        ..Default::default()
    };
    engine
        .sync_capture_workspace(&device, &mut secrets)
        .unwrap();
    let operation = engine.sync_outbox().unwrap().pop().unwrap();
    let key = secrets
        .objects
        .get(&(operation.object_id.clone(), 1))
        .unwrap();
    let generation = "writer-generation";
    let (content, blob, mode) = engine
        .collaboration_prepare_checkpoint_content(&operation.object_id, generation, key)
        .unwrap();
    assert!(blob.is_none());
    let checkpoint = crate::sync::EncryptedCheckpoint::seal_activation(
        &device,
        key,
        &engine.manifest().id,
        "1",
        "0",
        &content,
    )
    .unwrap();
    let envelope = crate::sync::PolicyEnvelope::from(
        device
            .wrap_key(
                &engine.manifest().id,
                &operation.object_id,
                1,
                device.device_id(),
                &device.recipient(),
                key,
            )
            .unwrap(),
    );
    let capability =
        crate::sync::WorkspaceCapability::sign(&engine.manifest().id, &device).unwrap();
    let activation = crate::sync::ObjectActivation::sign(
        &device,
        &capability,
        "1",
        "0",
        DocumentDescriptor {
            generation: generation.into(),
            mode,
        },
        vec![envelope],
        checkpoint.clone(),
        vec![],
    )
    .unwrap();
    engine
        .sync_prepare_activation(&activation, &public)
        .unwrap();
    drop(engine);
    let engine = WorkspaceEngine::open_with_app_data(
        directory.path().join("workspace"),
        directory.path().join("app"),
    )
    .unwrap();
    assert_eq!(
        engine
            .sync_pending_activation()
            .unwrap()
            .unwrap()
            .digest()
            .unwrap(),
        activation.digest().unwrap()
    );
    engine
        .sync_abandon_activation(&activation.activation_id)
        .unwrap();
    assert_eq!(engine.sync_outbox().unwrap().len(), 1);
    engine
        .sync_prepare_activation(&activation, &public)
        .unwrap();
    let activation_status = engine.sync_status().unwrap().activation.unwrap();
    assert_eq!(activation_status.activation_id, activation.activation_id);
    assert_eq!(activation_status.object_id, operation.object_id);
    assert_eq!(activation_status.path.as_deref(), Some("created.txt"));
    assert!(!activation_status.installed);
    engine.sync_accept_activation(&activation, &public).unwrap();
    engine
        .collaboration_install_checkpoint(&checkpoint, key, &public, &device, &secrets)
        .unwrap();
    assert!(engine.sync_status().unwrap().activation.unwrap().installed);
    engine
        .sync_finish_activation(&activation.activation_id)
        .unwrap();
    assert!(engine.sync_status().unwrap().activation.is_none());
    assert!(engine.sync_pending_activation().unwrap().is_none());
    assert!(engine.sync_outbox().unwrap().is_empty());
    let session = engine
        .collaboration_open(
            CollaborationOpenInput {
                relative_path: "created.txt".into(),
            },
            &device,
            &secrets,
        )
        .unwrap()
        .unwrap();
    assert_eq!(session.generation, generation);
    assert_eq!(
        TextDocument::restore(&session.update).unwrap().text(),
        "created by a writer"
    );
    let invalid_presence = engine
        .collaboration_seal_presence(
            crate::sync::CollaborationPresenceInput {
                session_id: session.session_id.clone(),
                anchor: STANDARD.encode([1, 2, 3]),
                head: STANDARD.encode([4, 5, 6]),
            },
            "relay-session",
            1,
            &device,
            &secrets,
        )
        .unwrap_err();
    assert_eq!(invalid_presence.code, "collaboration_invalid_presence");
    let presence = engine
        .collaboration_seal_presence(
            crate::sync::CollaborationPresenceInput {
                session_id: session.session_id,
                // Yjs relative position for the end of root text `content`.
                anchor: "AQdjb250ZW50AA==".into(),
                head: "AQdjb250ZW50AA==".into(),
            },
            "relay-session",
            1,
            &device,
            &secrets,
        )
        .unwrap();
    assert_eq!(presence.object_id, operation.object_id);
    assert_eq!(presence.generation, generation);
    assert_eq!(presence.epoch, 1);
    assert_eq!(presence.session_id, "relay-session");
    presence.verify(&public).unwrap();

    let remote = DeviceKeys::create(&Credentials).unwrap();
    secrets
        .trusted_devices
        .insert(remote.device_id().into(), remote.signer().public_key());
    let remote_presence = crate::sync::EncryptedPresence::seal(
        &remote,
        secrets
            .objects
            .get(&(operation.object_id.clone(), 1))
            .unwrap(),
        &crate::sync::PresenceContext {
            workspace_id: &engine.manifest().id,
            object_id: &operation.object_id,
            generation,
            epoch: 1,
            session_id: "remote-session",
            sequence: 1,
        },
        &crate::sync::PresenceSelection {
            anchor: "AQdjb250ZW50AA==".into(),
            head: "AQdjb250ZW50AA==".into(),
        },
    )
    .unwrap();
    let mut events = engine.subscribe();
    engine
        .collaboration_receive_presence(&remote_presence, &secrets)
        .unwrap();
    let received = events.try_recv().unwrap();
    assert_eq!(received.event_type, "collaboration:presence");
    assert_eq!(
        received.payload["presence"][0]["deviceId"],
        remote.device_id()
    );
    engine
        .collaboration_receive_presence(&remote_presence, &secrets)
        .unwrap();
    assert!(events.try_recv().is_err());
    engine
        .collaboration_remove_presence(remote.device_id(), "remote-session")
        .unwrap();
    let removed = events.try_recv().unwrap();
    assert_eq!(removed.payload["presence"], serde_json::json!([]));
}

#[test]
fn closing_one_window_does_not_close_another_windows_lease() {
    let f = fixture();
    let left = open(&f);
    let right = open(&f);
    assert_ne!(left.session_id, right.session_id);
    f.engine.collaboration_close(&left.session_id).unwrap();
    f.engine
        .collaboration_submit(edit(&right, "another window"), &f.device, &f.secrets)
        .unwrap();
    assert_eq!(
        std::fs::read(f.directory.path().join("workspace/note.md")).unwrap(),
        "\u{feff}another window".as_bytes()
    );
}

#[test]
fn path_activity_matches_whether_open_returns_a_session() {
    let f = fixture();
    assert!(f.engine.collaboration_path_is_active("note.md").unwrap());
    assert!(
        !f.engine
            .collaboration_path_is_active("elsewhere.md")
            .unwrap()
    );
    assert!(
        f.engine
            .collaboration_open(
                CollaborationOpenInput {
                    relative_path: "elsewhere.md".into()
                },
                &f.device,
                &f.secrets
            )
            .unwrap()
            .is_none()
    );
    assert!(
        f.engine
            .collaboration_path_is_active("../escape.md")
            .is_err()
    );
}

#[test]
fn managed_body_and_metadata_commit_as_one_generation_bound_operation() {
    let mut f = managed_fixture();
    let session = open(&f);
    let result = f
        .engine
        .collaboration_update_object(
            "note_01j00000000000000000000000",
            ObjectPatch {
                title: Some("Renamed".into()),
                body: Some("Changed body 😀\n".into()),
                properties: BTreeMap::from([(
                    "priority".into(),
                    serde_json::Value::String("high".into()),
                )]),
                remove_properties: vec!["status".into()],
                expected_revision: session.revision,
            },
            &f.device,
            &f.secrets,
        )
        .unwrap();
    assert_eq!(result.value.title, "Renamed");
    assert_eq!(result.value.body, "Changed body 😀\n");
    assert_eq!(result.value.properties["priority"], "high");
    assert!(!result.value.properties.contains_key("status"));
    let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
    assert_eq!(operation.kind, Some(OperationKind::Metadata));
    assert_eq!(operation.generation.as_deref(), Some("generation-one"));
    let pending = f
        .engine
        .collaboration_state("note_01j00000000000000000000000")
        .unwrap()
        .unwrap();
    assert_eq!(pending.pending.len(), 1);
    assert!(!pending.pending[0].updates.is_empty());
    assert_eq!(pending.pending[0].metadata.len(), 3);
    f.engine.collaboration_acknowledge(&operation, "1").unwrap();
    let reopened = open(&f);
    assert_eq!(reopened.status, CollaborationStatus::Synced);
    assert_eq!(
        TextDocument::restore(&reopened.update).unwrap().text(),
        "Changed body 😀\n"
    );
    let remote = DeviceKeys::create(&Credentials).unwrap();
    f.secrets
        .trusted_devices
        .insert(remote.device_id().into(), remote.signer().public_key());
    f.secrets
        .historical_workspace_writers
        .insert("1".into(), BTreeSet::from([remote.device_id().into()]));
    let remote_change = CollaborativeTransaction {
        version: 1,
        object_id: "note_01j00000000000000000000000".into(),
        generation: "generation-one".into(),
        updates: Vec::new(),
        metadata: vec![CollaborativeMetadataPatch {
            field: "property:status".into(),
            expected: CollaborativeMetadataValue::Missing,
            value: CollaborativeMetadataValue::Value {
                value: serde_json::Value::String("done".into()),
            },
        }],
        format: None,
        lifecycle: None,
    };
    let key = f
        .secrets
        .objects
        .get(&("note_01j00000000000000000000000".into(), 2))
        .unwrap();
    let remote_operation = remote
        .signer()
        .seal_for_document(
            key,
            &f.engine.manifest().id,
            "note_01j00000000000000000000000",
            remote.device_id(),
            (2, "1", "generation-one", OperationKind::Metadata),
            &serde_json::to_vec(&remote_change).unwrap(),
        )
        .unwrap();
    assert_eq!(
        f.engine
            .collaboration_apply_remote(
                &remote_operation,
                key,
                &remote.signer().public_key(),
                &f.secrets,
                "2",
            )
            .unwrap(),
        ApplyOutcome::Applied
    );
    let bytes = std::fs::read(f.engine.root.join("note.md")).unwrap();
    let ParsedMarkdown::Managed(object) = markdown::parse_markdown("note.md", &bytes) else {
        panic!("managed object expected")
    };
    assert_eq!(object.properties["status"], "done");
}

#[test]
fn same_field_metadata_races_preserve_the_losing_operation_for_review() {
    let mut f = managed_fixture();
    let session = open(&f);
    f.engine
        .collaboration_update_object(
            "note_01j00000000000000000000000",
            ObjectPatch {
                title: None,
                body: None,
                properties: BTreeMap::from([(
                    "priority".into(),
                    serde_json::Value::String("high".into()),
                )]),
                remove_properties: Vec::new(),
                expected_revision: session.revision,
            },
            &f.device,
            &f.secrets,
        )
        .unwrap();
    let remote = DeviceKeys::create(&Credentials).unwrap();
    f.secrets
        .trusted_devices
        .insert(remote.device_id().into(), remote.signer().public_key());
    f.secrets
        .historical_workspace_writers
        .insert("1".into(), BTreeSet::from([remote.device_id().into()]));
    let change = CollaborativeTransaction {
        version: 1,
        object_id: "note_01j00000000000000000000000".into(),
        generation: "generation-one".into(),
        updates: Vec::new(),
        metadata: vec![CollaborativeMetadataPatch {
            field: "property:priority".into(),
            expected: CollaborativeMetadataValue::Missing,
            value: CollaborativeMetadataValue::Value {
                value: serde_json::Value::String("low".into()),
            },
        }],
        format: None,
        lifecycle: None,
    };
    let key = f
        .secrets
        .objects
        .get(&("note_01j00000000000000000000000".into(), 2))
        .unwrap();
    let operation = remote
        .signer()
        .seal_for_document(
            key,
            &f.engine.manifest().id,
            "note_01j00000000000000000000000",
            remote.device_id(),
            (2, "1", "generation-one", OperationKind::Metadata),
            &serde_json::to_vec(&change).unwrap(),
        )
        .unwrap();
    assert_eq!(
        f.engine
            .collaboration_apply_remote(
                &operation,
                key,
                &remote.signer().public_key(),
                &f.secrets,
                "1",
            )
            .unwrap(),
        ApplyOutcome::Conflict
    );
    let bytes = std::fs::read(f.engine.root.join("note.md")).unwrap();
    let ParsedMarkdown::Managed(object) = markdown::parse_markdown("note.md", &bytes) else {
        panic!("managed object expected")
    };
    assert_eq!(object.properties["priority"], "high");
    assert!(
        f.engine
            .sync_path(&format!(
                ".noura/sync/collaboration/note_01j00000000000000000000000/reviews/{}.metadata.json",
                operation.operation_id
            ))
            .unwrap()
            .exists()
    );
    assert_eq!(
        f.engine.sync_journal().unwrap().receipts[&operation.operation_id].outcome,
        ApplyOutcome::Conflict
    );
}

#[test]
fn managed_moves_are_generation_bound_and_advance_the_acknowledged_path() {
    let f = managed_fixture();
    let session = open(&f);
    let result = f
        .engine
        .collaboration_move_object(
            "note_01j00000000000000000000000",
            "moved/note.md",
            &session.revision,
            &f.device,
            &f.secrets,
        )
        .unwrap();
    assert_eq!(result.value.id, "note_01j00000000000000000000000");
    assert_eq!(result.value.relative_path, "moved/note.md");
    assert!(!f.engine.root.join("note.md").exists());
    assert!(f.engine.root.join("moved/note.md").exists());
    let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
    assert_eq!(operation.kind, Some(OperationKind::Metadata));
    assert_eq!(operation.generation.as_deref(), Some("generation-one"));
    let pending = f
        .engine
        .collaboration_state("note_01j00000000000000000000000")
        .unwrap()
        .unwrap();
    assert!(matches!(
        pending.pending[0].lifecycle,
        Some(CollaborativeLifecycleChange::Move { ref from, ref to, .. })
            if from == "note.md" && to == "moved/note.md"
    ));
    let reopened = f
        .engine
        .collaboration_open(
            CollaborationOpenInput {
                relative_path: "moved/note.md".into(),
            },
            &f.device,
            &f.secrets,
        )
        .unwrap()
        .unwrap();
    assert_eq!(reopened.object_id, "note_01j00000000000000000000000");
    f.engine.collaboration_acknowledge(&operation, "1").unwrap();
    let acknowledged = f
        .engine
        .collaboration_state("note_01j00000000000000000000000")
        .unwrap()
        .unwrap();
    assert!(acknowledged.pending.is_empty());
    assert_eq!(
        acknowledged.acknowledged.unwrap().path.as_deref(),
        Some("moved/note.md")
    );
}

#[test]
fn remote_moves_apply_and_destination_collisions_are_preserved_for_review() {
    for collision in [false, true] {
        let mut f = managed_fixture();
        let remote = DeviceKeys::create(&Credentials).unwrap();
        f.secrets
            .trusted_devices
            .insert(remote.device_id().into(), remote.signer().public_key());
        f.secrets
            .historical_workspace_writers
            .insert("1".into(), BTreeSet::from([remote.device_id().into()]));
        let state = f
            .engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        let change = CollaborativeTransaction {
            version: 1,
            object_id: state.object_id.clone(),
            generation: state.generation.clone(),
            updates: Vec::new(),
            metadata: Vec::new(),
            format: None,
            lifecycle: Some(CollaborativeLifecycleChange::Move {
                from: state.path.clone(),
                to: "remote/note.md".into(),
                expected_revision: state.revision.clone(),
            }),
        };
        let key = f
            .secrets
            .objects
            .get(&(state.object_id.clone(), 2))
            .unwrap();
        let operation = remote
            .signer()
            .seal_for_document(
                key,
                &f.engine.manifest().id,
                &state.object_id,
                remote.device_id(),
                (2, "1", &state.generation, OperationKind::Metadata),
                &serde_json::to_vec(&change).unwrap(),
            )
            .unwrap();
        if collision {
            std::fs::create_dir_all(f.engine.root.join("remote")).unwrap();
            std::fs::write(f.engine.root.join("remote/note.md"), b"occupied").unwrap();
        }
        let outcome = f
            .engine
            .collaboration_apply_remote(
                &operation,
                key,
                &remote.signer().public_key(),
                &f.secrets,
                "1",
            )
            .unwrap();
        if collision {
            assert_eq!(outcome, ApplyOutcome::Conflict);
            assert!(f.engine.root.join("note.md").exists());
            assert_eq!(
                std::fs::read(f.engine.root.join("remote/note.md")).unwrap(),
                b"occupied"
            );
            assert!(
                f.engine
                    .sync_path(&format!(
                        ".noura/sync/collaboration/{}/reviews/{}.metadata.json",
                        state.object_id, operation.operation_id
                    ))
                    .unwrap()
                    .exists()
            );
        } else {
            assert_eq!(outcome, ApplyOutcome::Applied);
            assert!(!f.engine.root.join("note.md").exists());
            assert!(f.engine.root.join("remote/note.md").exists());
            assert_eq!(
                f.engine
                    .collaboration_state(&state.object_id)
                    .unwrap()
                    .unwrap()
                    .path,
                "remote/note.md"
            );
        }
    }
}

#[test]
fn managed_move_recovers_every_durable_boundary_after_restart() {
    for boundary in 0..3 {
        let f = managed_fixture();
        let workspace = f.directory.path().join("workspace");
        let app_data = f.directory.path().join("app");
        let state = f
            .engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        f.engine.collaboration_set_mutation_fault(boundary);
        assert_eq!(
            f.engine
                .collaboration_move_object(
                    &state.object_id,
                    "recovered/note.md",
                    &state.revision,
                    &f.device,
                    &f.secrets,
                )
                .unwrap_err()
                .code,
            "collaboration_test_interrupted"
        );
        let device = f.device;
        let secrets = f.secrets;
        drop(f.engine);
        let engine = WorkspaceEngine::open_with_app_data(&workspace, &app_data).unwrap();
        engine
            .collaboration_recover("note_01j00000000000000000000000", &secrets)
            .unwrap();
        assert!(!workspace.join("note.md").exists());
        assert!(workspace.join("recovered/note.md").exists());
        let recovered = engine
            .collaboration_open(
                CollaborationOpenInput {
                    relative_path: "recovered/note.md".into(),
                },
                &device,
                &secrets,
            )
            .unwrap()
            .unwrap();
        assert_eq!(recovered.object_id, "note_01j00000000000000000000000");
        assert_eq!(engine.sync_outbox().unwrap().len(), 1);
    }
}

#[test]
fn managed_deletion_preserves_pending_drafts_until_tombstone_acknowledgment() {
    let f = managed_fixture();
    let session = open(&f);
    let edited = f
        .engine
        .collaboration_update_object(
            "note_01j00000000000000000000000",
            ObjectPatch {
                title: None,
                body: Some("Unsent draft\n".into()),
                properties: BTreeMap::new(),
                remove_properties: Vec::new(),
                expected_revision: session.revision,
            },
            &f.device,
            &f.secrets,
        )
        .unwrap();
    let deleted = f
        .engine
        .collaboration_delete_object(
            "note_01j00000000000000000000000",
            &edited.revision,
            &f.device,
            &f.secrets,
        )
        .unwrap();
    assert_eq!(deleted.value.body, "Unsent draft\n");
    assert!(!f.engine.root.join("note.md").exists());
    let operations = f.engine.sync_outbox().unwrap();
    assert_eq!(operations.len(), 2);
    let delete_operation = operations.last().unwrap();
    let key = f
        .secrets
        .objects
        .get(&("note_01j00000000000000000000000".into(), 2))
        .unwrap();
    let transaction: CollaborativeTransaction = serde_json::from_slice(
        &delete_operation
            .open(key, &f.device.signer().public_key())
            .unwrap(),
    )
    .unwrap();
    let Some(CollaborativeLifecycleChange::Delete {
        path, tombstone_id, ..
    }) = transaction.lifecycle
    else {
        panic!("delete lifecycle expected")
    };
    let trash =
        collaboration_trash_path("note_01j00000000000000000000000", &tombstone_id, &path).unwrap();
    assert!(f.engine.root.join(trash).exists());
    let state = f
        .engine
        .collaboration_state("note_01j00000000000000000000000")
        .unwrap()
        .unwrap();
    assert!(state.deleted);
    assert_eq!(state.pending.len(), 2);
    assert_eq!(
        TextDocument::restore(&state.update).unwrap().text(),
        "Unsent draft\n"
    );
    assert!(
        f.engine
            .collaboration_open(
                CollaborationOpenInput {
                    relative_path: "note.md".into(),
                },
                &f.device,
                &f.secrets,
            )
            .unwrap()
            .is_none()
    );
    f.engine
        .collaboration_acknowledge(&operations[0], "1")
        .unwrap();
    let pending_delete = f
        .engine
        .collaboration_state("note_01j00000000000000000000000")
        .unwrap()
        .unwrap();
    assert!(pending_delete.deleted);
    assert_eq!(pending_delete.pending.len(), 1);
    assert_eq!(
        TextDocument::restore(&pending_delete.acknowledged.unwrap().update)
            .unwrap()
            .text(),
        "Unsent draft\n"
    );
    f.engine
        .collaboration_acknowledge(delete_operation, "2")
        .unwrap();
    let acknowledged = f
        .engine
        .collaboration_state("note_01j00000000000000000000000")
        .unwrap()
        .unwrap();
    assert!(acknowledged.pending.is_empty());
    assert!(acknowledged.acknowledged.unwrap().deleted);
}

#[test]
fn remote_delete_conflicting_with_a_local_draft_enters_review() {
    let mut f = managed_fixture();
    let baseline = open(&f);
    f.engine
        .collaboration_update_object(
            "note_01j00000000000000000000000",
            ObjectPatch {
                title: None,
                body: Some("Retained locally\n".into()),
                properties: BTreeMap::new(),
                remove_properties: Vec::new(),
                expected_revision: baseline.revision.clone(),
            },
            &f.device,
            &f.secrets,
        )
        .unwrap();
    let remote = DeviceKeys::create(&Credentials).unwrap();
    f.secrets
        .trusted_devices
        .insert(remote.device_id().into(), remote.signer().public_key());
    f.secrets
        .historical_workspace_writers
        .insert("1".into(), BTreeSet::from([remote.device_id().into()]));
    let change = CollaborativeTransaction {
        version: 1,
        object_id: "note_01j00000000000000000000000".into(),
        generation: "generation-one".into(),
        updates: Vec::new(),
        metadata: Vec::new(),
        format: None,
        lifecycle: Some(CollaborativeLifecycleChange::Delete {
            path: "note.md".into(),
            expected_revision: baseline.revision,
            tombstone_id: uuid::Uuid::new_v4().to_string(),
        }),
    };
    let key = f
        .secrets
        .objects
        .get(&("note_01j00000000000000000000000".into(), 2))
        .unwrap();
    let operation = remote
        .signer()
        .seal_for_document(
            key,
            &f.engine.manifest().id,
            "note_01j00000000000000000000000",
            remote.device_id(),
            (2, "1", "generation-one", OperationKind::Metadata),
            &serde_json::to_vec(&change).unwrap(),
        )
        .unwrap();
    assert_eq!(
        f.engine
            .collaboration_apply_remote(
                &operation,
                key,
                &remote.signer().public_key(),
                &f.secrets,
                "1",
            )
            .unwrap(),
        ApplyOutcome::Conflict
    );
    assert!(f.engine.root.join("note.md").exists());
    let retained = f
        .engine
        .collaboration_state("note_01j00000000000000000000000")
        .unwrap()
        .unwrap();
    assert!(!retained.deleted);
    assert_eq!(
        TextDocument::restore(&retained.update).unwrap().text(),
        "Retained locally\n"
    );
    assert_eq!(retained.pending.len(), 1);
}

#[test]
fn remote_delete_tombstones_a_clean_document_and_replay_is_idempotent() {
    let mut f = managed_fixture();
    let baseline = open(&f);
    let remote = DeviceKeys::create(&Credentials).unwrap();
    f.secrets
        .trusted_devices
        .insert(remote.device_id().into(), remote.signer().public_key());
    f.secrets
        .historical_workspace_writers
        .insert("1".into(), BTreeSet::from([remote.device_id().into()]));
    let change = CollaborativeTransaction {
        version: 1,
        object_id: "note_01j00000000000000000000000".into(),
        generation: "generation-one".into(),
        updates: Vec::new(),
        metadata: Vec::new(),
        format: None,
        lifecycle: Some(CollaborativeLifecycleChange::Delete {
            path: "note.md".into(),
            expected_revision: baseline.revision,
            tombstone_id: uuid::Uuid::new_v4().to_string(),
        }),
    };
    let key = f
        .secrets
        .objects
        .get(&(change.object_id.clone(), 2))
        .unwrap();
    let operation = remote
        .signer()
        .seal_for_document(
            key,
            &f.engine.manifest().id,
            &change.object_id,
            remote.device_id(),
            (2, "1", &change.generation, OperationKind::Metadata),
            &serde_json::to_vec(&change).unwrap(),
        )
        .unwrap();
    assert_eq!(
        f.engine
            .collaboration_apply_remote(
                &operation,
                key,
                &remote.signer().public_key(),
                &f.secrets,
                "1",
            )
            .unwrap(),
        ApplyOutcome::Applied
    );
    assert!(!f.engine.root.join("note.md").exists());
    let state = f
        .engine
        .collaboration_state(&change.object_id)
        .unwrap()
        .unwrap();
    assert!(state.deleted);
    assert!(state.acknowledged.unwrap().deleted);
    assert_eq!(
        f.engine
            .collaboration_apply_remote(
                &operation,
                key,
                &remote.signer().public_key(),
                &f.secrets,
                "1",
            )
            .unwrap(),
        ApplyOutcome::Applied
    );
}

#[test]
fn managed_delete_recovers_every_durable_boundary_after_restart() {
    for boundary in 0..3 {
        let f = managed_fixture();
        let workspace = f.directory.path().join("workspace");
        let app_data = f.directory.path().join("app");
        let state = f
            .engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        f.engine.collaboration_set_mutation_fault(boundary);
        assert_eq!(
            f.engine
                .collaboration_delete_object(
                    &state.object_id,
                    &state.revision,
                    &f.device,
                    &f.secrets,
                )
                .unwrap_err()
                .code,
            "collaboration_test_interrupted"
        );
        let device = f.device;
        let secrets = f.secrets;
        drop(f.engine);
        let engine = WorkspaceEngine::open_with_app_data(&workspace, &app_data).unwrap();
        engine
            .collaboration_recover("note_01j00000000000000000000000", &secrets)
            .unwrap();
        assert!(!workspace.join("note.md").exists());
        let recovered = engine
            .collaboration_state("note_01j00000000000000000000000")
            .unwrap()
            .unwrap();
        assert!(recovered.deleted);
        assert_eq!(recovered.pending.len(), 1);
        assert!(
            engine
                .collaboration_open(
                    CollaborationOpenInput {
                        relative_path: "note.md".into(),
                    },
                    &device,
                    &secrets,
                )
                .unwrap()
                .is_none()
        );
        assert_eq!(engine.sync_outbox().unwrap().len(), 1);
    }
}

#[test]
fn external_managed_saves_translate_to_recoverable_native_transactions() {
    let mut f = managed_fixture();
    let path = f.engine.root.join("note.md");
    let original = std::fs::read(&path).unwrap();
    let ParsedMarkdown::Managed(mut external) = markdown::parse_markdown("note.md", &original)
    else {
        panic!("managed object expected")
    };
    external.title = "Changed outside".into();
    external.body = "External body 😀\n".into();
    external.properties.insert(
        "priority".into(),
        serde_json::Value::String("medium".into()),
    );
    let external_bytes = markdown::serialize_object(&external).unwrap();
    std::fs::write(&path, &external_bytes).unwrap();
    f.engine
        .sync_capture_workspace(&f.device, &mut f.secrets)
        .unwrap();
    let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
    assert_eq!(operation.kind, Some(OperationKind::Metadata));
    let snapshot: CollaborationExternalSnapshot = serde_json::from_slice(
        &std::fs::read(
            f.engine
                .sync_path(&format!(
                    ".noura/sync/collaboration/note_01j00000000000000000000000/external/{}.json",
                    operation.operation_id
                ))
                .unwrap(),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(STANDARD.decode(snapshot.bytes).unwrap(), external_bytes);
    let reopened = open(&f);
    assert_eq!(
        TextDocument::restore(&reopened.update).unwrap().text(),
        "External body 😀"
    );
    let materialized = std::fs::read(&path).unwrap();
    let ParsedMarkdown::Managed(materialized) = markdown::parse_markdown("note.md", &materialized)
    else {
        panic!("managed object expected")
    };
    assert_eq!(materialized.title, "Changed outside");
    assert_eq!(materialized.properties["priority"], "medium");
}

#[test]
fn invalid_external_managed_saves_and_deletes_are_preserved_for_review() {
    let mut f = managed_fixture();
    let path = f.engine.root.join("note.md");
    let invalid = b"---\nid: forged\ntype: note\ntitle: Forged\n---\n\nbytes";
    std::fs::write(&path, invalid).unwrap();
    f.engine
        .sync_capture_workspace(&f.device, &mut f.secrets)
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), invalid);
    assert!(f.engine.sync_outbox().unwrap().is_empty());
    let reviews = f
        .engine
        .sync_path(".noura/sync/collaboration/note_01j00000000000000000000000/reviews")
        .unwrap();
    assert_eq!(std::fs::read_dir(&reviews).unwrap().count(), 1);
    std::fs::remove_file(&path).unwrap();
    f.engine
        .sync_capture_workspace(&f.device, &mut f.secrets)
        .unwrap();
    assert!(!path.exists());
    assert_eq!(std::fs::read_dir(reviews).unwrap().count(), 2);
}

#[test]
fn external_raw_format_changes_preserve_bom_and_newline_intent() {
    let mut f = fixture();
    let path = f.engine.root.join("note.md");
    let external = "A😀\nsecond\n".as_bytes();
    std::fs::write(&path, external).unwrap();
    f.engine
        .sync_capture_workspace(&f.device, &mut f.secrets)
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), external);
    let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
    assert_eq!(operation.kind, Some(OperationKind::Metadata));
    let key = f.secrets.objects.get(&("text".into(), 2)).unwrap();
    let transaction: CollaborativeTransaction = serde_json::from_slice(
        &operation
            .open(key, &f.device.signer().public_key())
            .unwrap(),
    )
    .unwrap();
    assert!(transaction.updates.is_empty());
    assert!(transaction.metadata.is_empty());
    assert_eq!(
        transaction.format,
        Some(CollaborativeFormatPatch {
            expected: CollaborativeTextFormat {
                has_bom: true,
                uses_crlf: true,
            },
            value: CollaborativeTextFormat {
                has_bom: false,
                uses_crlf: false,
            },
        })
    );
    f.engine.collaboration_acknowledge(&operation, "1").unwrap();
    assert_eq!(open(&f).status, CollaborationStatus::Synced);
}

#[test]
fn external_translation_recovers_each_durable_boundary_after_restart() {
    for boundary in 0..3 {
        let mut f = managed_fixture();
        let workspace = f.directory.path().join("workspace");
        let app_data = f.directory.path().join("app");
        let path = workspace.join("note.md");
        let bytes = std::fs::read(&path).unwrap();
        let ParsedMarkdown::Managed(mut external) = markdown::parse_markdown("note.md", &bytes)
        else {
            panic!("managed object expected")
        };
        external.title = format!("External boundary {boundary}");
        external.body = format!("Recovered {boundary}\n");
        std::fs::write(&path, markdown::serialize_object(&external).unwrap()).unwrap();
        f.engine.collaboration_set_mutation_fault(boundary);
        assert_eq!(
            f.engine
                .sync_capture_workspace(&f.device, &mut f.secrets)
                .unwrap_err()
                .code,
            "collaboration_test_interrupted"
        );
        let device = f.device;
        let mut secrets = f.secrets;
        drop(f.engine);
        let engine = WorkspaceEngine::open_with_app_data(&workspace, &app_data).unwrap();
        engine
            .sync_capture_workspace(&device, &mut secrets)
            .unwrap();
        assert_eq!(engine.sync_outbox().unwrap().len(), 1);
        let session = engine
            .collaboration_open(
                CollaborationOpenInput {
                    relative_path: "note.md".into(),
                },
                &device,
                &secrets,
            )
            .unwrap()
            .unwrap();
        assert_eq!(
            TextDocument::restore(&session.update).unwrap().text(),
            format!("Recovered {boundary}")
        );
    }
}

#[test]
fn legacy_raw_autosave_cannot_overwrite_an_active_generation() {
    let f = fixture();
    let session = open(&f);
    let error = f
        .engine
        .save_raw_markdown(crate::RawSaveInput {
            relative_path: "note.md".into(),
            base_revision: session.revision,
            base_body: "A😀\nsecond\n".into(),
            local_body: "legacy overwrite".into(),
        })
        .unwrap_err();
    assert_eq!(error.code, "collaboration_transaction_required");
    assert!(f.engine.sync_outbox().unwrap().is_empty());
    assert_eq!(
        TextDocument::restore(&open(&f).update).unwrap().text(),
        "A😀\nsecond\n"
    );
    let replacement = FileChange {
        version: 1,
        path: "note.md".into(),
        previous_path: None,
        base_revision: Some(open(&f).revision),
        content: Some(STANDARD.encode(b"relay replacement")),
        accepted_revisions: None,
        blob: None,
    };
    let key = f.secrets.objects.get(&("text".into(), 2)).unwrap();
    let operation = f
        .device
        .signer()
        .seal_at_revision(
            key,
            &f.engine.manifest().id,
            "text",
            f.device.device_id(),
            (2, "1"),
            &serde_json::to_vec(&replacement).unwrap(),
        )
        .unwrap();
    assert_eq!(
        f.engine
            .sync_apply_file(&operation, key, &f.device.signer().public_key())
            .unwrap_err()
            .code,
        "collaboration_transaction_required"
    );
    assert_eq!(
        TextDocument::restore(&open(&f).update).unwrap().text(),
        "A😀\nsecond\n"
    );
}

#[test]
fn durable_save_retry_restart_and_sqlite_rebuild_preserve_pending_edits() {
    let f = fixture();
    let session = open(&f);
    let input = edit(&session, "A😀 changed\nsecond\n");
    let receipt = f
        .engine
        .collaboration_submit(input.clone(), &f.device, &f.secrets)
        .unwrap();
    assert_eq!(
        f.engine
            .collaboration_submit(input, &f.device, &f.secrets)
            .unwrap()
            .revision,
        receipt.revision
    );
    assert_eq!(f.engine.sync_outbox().unwrap().len(), 1);
    assert_eq!(
        std::fs::read(f.directory.path().join("workspace/note.md")).unwrap(),
        "\u{feff}A😀 changed\r\nsecond\r\n".as_bytes()
    );
    // A restart closes the first engine before reopening the workspace;
    // Windows refuses to replace an index.sqlite that another handle holds.
    drop(session);
    drop(f.engine);
    let reopened = WorkspaceEngine::open_with_app_data(
        f.directory.path().join("workspace"),
        f.directory.path().join("app"),
    )
    .unwrap();
    reopened.rebuild_index().unwrap();
    let restored = reopened
        .collaboration_open(
            CollaborationOpenInput {
                relative_path: "note.md".into(),
            },
            &f.device,
            &f.secrets,
        )
        .unwrap()
        .unwrap();
    assert_eq!(restored.revision, receipt.revision);
    assert_eq!(
        TextDocument::restore(&restored.update).unwrap().text(),
        "A😀 changed\nsecond\n"
    );
    assert_eq!(reopened.sync_outbox().unwrap().len(), 1);
}

#[test]
fn acknowledgements_advance_only_the_verified_baseline_and_keep_later_drafts() {
    let f = fixture();
    let first_session = open(&f);
    f.engine
        .collaboration_submit(edit(&first_session, "first draft\n"), &f.device, &f.secrets)
        .unwrap();
    let second_session = open(&f);
    f.engine
        .collaboration_submit(
            edit(&second_session, "second draft\n"),
            &f.device,
            &f.secrets,
        )
        .unwrap();
    let pending = f.engine.sync_outbox().unwrap();
    assert_eq!(pending.len(), 2);

    f.engine.sync_acknowledge(&pending[0], "1").unwrap();
    let state = f.engine.collaboration_state("text").unwrap().unwrap();
    let baseline = state.acknowledged.as_ref().unwrap();
    assert_eq!(baseline.sequence, "1");
    assert_eq!(
        TextDocument::restore(&baseline.update).unwrap().text(),
        "first draft\n"
    );
    assert_eq!(
        TextDocument::restore(&state.update).unwrap().text(),
        "second draft\n"
    );
    assert_eq!(state.pending.len(), 1);
    assert_eq!(state.pending[0].operation_id, pending[1].operation_id);

    f.engine.sync_acknowledge(&pending[1], "2").unwrap();
    let state = f.engine.collaboration_state("text").unwrap().unwrap();
    assert!(state.pending.is_empty());
    assert_eq!(
        TextDocument::restore(&state.acknowledged.as_ref().unwrap().update)
            .unwrap()
            .text(),
        "second draft\n"
    );
    assert!(f.engine.sync_outbox().unwrap().is_empty());
}

fn rotate_checkpoint(f: &mut Fixture, checkpoint_text: &[u8]) -> EncryptedCheckpoint {
    let previous = f.engine.sync_access_policy().unwrap().unwrap();
    let previous_digest = previous.digest().unwrap();
    let policy = AccessPolicy::sign(
        &f.engine.manifest().id,
        "2",
        Some(previous_digest.clone()),
        &f.device,
        previous.members,
        vec![AccessObject {
            object_id: "text".into(),
            epoch: 3,
            grants: vec![],
            envelopes: vec![],
            document: Some(DocumentDescriptor {
                generation: "generation-two".into(),
                mode: DocumentMode::Text,
            }),
        }],
    )
    .unwrap();
    f.engine
        .sync_accept_access_policy("1", Some(&previous_digest), &policy)
        .unwrap();
    let key = ObjectKey::generate();
    let checkpoint = EncryptedCheckpoint::seal(
        &f.device,
        &key,
        &policy,
        "0",
        &CheckpointContent {
            version: 1,
            object_id: "text".into(),
            generation: "generation-two".into(),
            content_revision: Some(markdown::revision(checkpoint_text)),
            change: FileChange {
                version: 1,
                path: "note.md".into(),
                previous_path: None,
                base_revision: None,
                content: Some(STANDARD.encode(checkpoint_text)),
                accepted_revisions: None,
                blob: None,
            },
        },
    )
    .unwrap();
    f.secrets.objects.insert(("text".into(), 3), key);
    checkpoint
}

#[test]
fn generation_rotation_rebases_non_overlapping_drafts_as_fresh_updates() {
    let mut f = fixture();
    let session = open(&f);
    f.engine
        .collaboration_submit(
            edit(&session, "LOCAL\nA😀\nsecond\n"),
            &f.device,
            &f.secrets,
        )
        .unwrap();
    let old_operation = f.engine.sync_outbox().unwrap()[0].operation_id.clone();
    let checkpoint = rotate_checkpoint(&mut f, b"A\xf0\x9f\x98\x80\r\nsecond\r\nREMOTE\r\n");
    f.engine
        .collaboration_install_checkpoint(
            &checkpoint,
            f.secrets.objects.get(&("text".into(), 3)).unwrap(),
            &f.device.signer().public_key(),
            &f.device,
            &f.secrets,
        )
        .unwrap();
    let state = f.engine.collaboration_state("text").unwrap().unwrap();
    assert_eq!(state.generation, "generation-two");
    assert_eq!(
        TextDocument::restore(&state.update).unwrap().text(),
        "LOCAL\nA😀\nsecond\nREMOTE\n"
    );
    assert_eq!(state.pending.len(), 1);
    let outbox = f.engine.sync_outbox().unwrap();
    assert_eq!(outbox.len(), 1);
    assert_ne!(outbox[0].operation_id, old_operation);
    assert_eq!(outbox[0].generation.as_deref(), Some("generation-two"));
    f.engine
        .collaboration_install_checkpoint(
            &checkpoint,
            f.secrets.objects.get(&("text".into(), 3)).unwrap(),
            &f.device.signer().public_key(),
            &f.device,
            &f.secrets,
        )
        .unwrap();
    assert_eq!(f.engine.sync_outbox().unwrap(), outbox);
}

#[test]
fn generation_rotation_preserves_overlapping_drafts_for_review() {
    let mut f = fixture();
    let session = open(&f);
    f.engine
        .collaboration_submit(edit(&session, "local\n"), &f.device, &f.secrets)
        .unwrap();
    let before = std::fs::read(f.directory.path().join("workspace/note.md")).unwrap();
    let pending = f.engine.sync_outbox().unwrap();
    let checkpoint = rotate_checkpoint(&mut f, b"remote\n");
    assert_eq!(
        f.engine
            .collaboration_install_checkpoint(
                &checkpoint,
                f.secrets.objects.get(&("text".into(), 3)).unwrap(),
                &f.device.signer().public_key(),
                &f.device,
                &f.secrets,
            )
            .unwrap_err()
            .code,
        "collaboration_needs_review"
    );
    assert_eq!(
        std::fs::read(f.directory.path().join("workspace/note.md")).unwrap(),
        before
    );
    assert_eq!(f.engine.sync_outbox().unwrap(), pending);
    assert_eq!(
        f.engine
            .collaboration_state("text")
            .unwrap()
            .unwrap()
            .generation,
        "generation-one"
    );
}

#[test]
fn viewers_stale_generations_and_competing_external_bytes_are_rejected() {
    let mut f = fixture();
    let session = open(&f);
    let mut input = edit(&session, "draft");
    input.generation = "stale".into();
    assert_eq!(
        f.engine
            .collaboration_submit(input, &f.device, &f.secrets)
            .unwrap_err()
            .code,
        "collaboration_stale_generation"
    );
    f.secrets.authorized_workspace_writers.clear();
    assert_eq!(
        f.engine
            .collaboration_submit(edit(&session, "draft"), &f.device, &f.secrets)
            .unwrap_err()
            .code,
        "sync_writer_not_authorized"
    );
    f.secrets
        .authorized_workspace_writers
        .insert(f.device.device_id().into());
    std::fs::write(
        f.directory.path().join("workspace/note.md"),
        "external bytes",
    )
    .unwrap();
    assert_eq!(
        f.engine
            .collaboration_submit(edit(&session, "draft"), &f.device, &f.secrets)
            .unwrap_err()
            .code,
        "collaboration_external_change"
    );
    assert_eq!(
        std::fs::read_to_string(f.directory.path().join("workspace/note.md")).unwrap(),
        "external bytes"
    );
    assert!(f.engine.sync_outbox().unwrap().is_empty());
}

#[test]
fn lost_ack_recovery_finishes_each_durable_boundary_and_rejects_tampering() {
    for boundary in 0..3 {
        let f = fixture();
        let prior = f.engine.collaboration_state("text").unwrap().unwrap();
        let session = open(&f);
        let input = edit(&session, "recovered 😀\n");
        let change = CollaborativeChange {
            version: 1,
            object_id: "text".into(),
            generation: session.generation,
            updates: input.updates,
        };
        let key = f.secrets.objects.get(&("text".into(), 2)).unwrap();
        let operation = f
            .device
            .signer()
            .seal_for_document(
                key,
                &f.engine.manifest().id,
                "text",
                f.device.device_id(),
                (2, "1", &change.generation, OperationKind::Text),
                &serde_json::to_vec(&change).unwrap(),
            )
            .unwrap();
        let candidate = TextDocument::restore(&prior.update)
            .unwrap()
            .apply(&change.updates)
            .unwrap();
        let bytes = render(
            &prior.path,
            &STANDARD.decode(&prior.materialized).unwrap(),
            "text",
            &candidate.text(),
        )
        .unwrap();
        let next = DocumentState {
            revision: markdown::revision(&bytes),
            materialized: STANDARD.encode(&bytes),
            update: candidate.state().unwrap(),
            pending: vec![PendingDocumentChange {
                operation_id: operation.operation_id.clone(),
                updates: change.updates.clone(),
                metadata: Vec::new(),
                format: None,
                lifecycle: None,
            }],
            ..prior.clone()
        };
        let intent = DocumentIntent {
            version: 1,
            base_revision: prior.revision,
            next,
            operation,
            outgoing: true,
            batch_id: None,
            batch_digest: None,
            external_snapshot: None,
            previous_path: None,
        };
        f.engine
            .sync_write(&intent_path("text").unwrap(), &Some(&intent))
            .unwrap();
        if boundary >= 1 {
            f.engine
                .sync_write_file_checked("note.md", &bytes, Some(&intent.base_revision))
                .unwrap();
        }
        if boundary >= 2 {
            f.engine
                .sync_write(&state_path("text").unwrap(), &intent.next)
                .unwrap();
        }
        f.engine
            .collaboration_flush(&session.session_id, &f.secrets)
            .unwrap();
        assert_eq!(
            std::fs::read(f.directory.path().join("workspace/note.md")).unwrap(),
            bytes
        );
        assert_eq!(f.engine.sync_outbox().unwrap().len(), 1);
        // A forged intent cannot change canonical metadata or a candidate's text.
        let mut forged = intent;
        forged.next.materialized = STANDARD.encode(b"forged");
        forged.next.revision = markdown::revision(b"forged");
        f.engine
            .sync_write(&intent_path("text").unwrap(), &Some(forged))
            .unwrap();
        assert!(
            f.engine
                .collaboration_flush(&session.session_id, &f.secrets)
                .is_err()
        );
        assert_eq!(
            std::fs::read(f.directory.path().join("workspace/note.md")).unwrap(),
            bytes
        );
    }
}

const MANAGED_ID: &str = "note_01j00000000000000000000000";

fn body_patch(body: &str, expected_revision: &str) -> ObjectPatch {
    ObjectPatch {
        title: None,
        body: Some(body.into()),
        properties: BTreeMap::new(),
        remove_properties: Vec::new(),
        expected_revision: expected_revision.into(),
    }
}

fn disable_sync_plugin(engine: &WorkspaceEngine) {
    let mut plugins = engine.read_manifest().unwrap().enabled_plugins;
    plugins.retain(|id| id != crate::SYNC_PLUGIN_ID);
    engine
        .manifest_update(ManifestUpdateInput {
            enabled_plugins: Some(plugins),
            ..Default::default()
        })
        .unwrap();
}

fn connection() -> crate::sync::DeviceConnection {
    crate::sync::DeviceConnection {
        origin: "https://sync.example.com".into(),
        device_id: "device".into(),
        account_id: "account".into(),
        token_reference: "token".into(),
    }
}

#[test]
fn disabled_plugin_allows_local_writes_to_collaborative_documents() {
    let mut f = managed_fixture();
    f.engine.rebuild_index().unwrap();
    let revision = f.engine.get_object(MANAGED_ID).unwrap().unwrap().revision;
    assert_eq!(
        f.engine
            .update_object(MANAGED_ID, body_patch("Offline body\n", &revision))
            .unwrap_err()
            .code,
        "collaboration_transaction_required"
    );
    disable_sync_plugin(&f.engine);
    f.engine
        .update_object(MANAGED_ID, body_patch("Offline body\n", &revision))
        .unwrap();
    assert!(f.engine.sync_outbox().unwrap().is_empty());
    // Turning the plugin back on captures the offline edit like an external one.
    f.engine.enable_sync_plugin().unwrap();
    f.engine
        .sync_capture_workspace(&f.device, &mut f.secrets)
        .unwrap();
    let operation = f.engine.sync_outbox().unwrap().pop().unwrap();
    assert_eq!(operation.kind, Some(OperationKind::Metadata));
    let reopened = open(&f);
    assert_eq!(
        TextDocument::restore(&reopened.update)
            .unwrap()
            .text()
            .trim_end(),
        "Offline body"
    );
}

#[test]
fn route_helpers_bypass_collaboration_when_disabled() {
    let f = managed_fixture();
    f.engine.rebuild_index().unwrap();
    let revision = f.engine.get_object(MANAGED_ID).unwrap().unwrap().revision;
    let signed_out = crate::sync::route_update_object(
        &f.engine,
        || Ok(None),
        &Credentials,
        MANAGED_ID,
        body_patch("Routed\n", &revision),
        None,
    )
    .unwrap_err();
    assert_eq!(signed_out.code, "sync_sign_in_required");
    disable_sync_plugin(&f.engine);
    let no_connection = || -> Result<Option<crate::sync::DeviceConnection>> {
        panic!("a local write must not ask for the device connection")
    };
    let updated = crate::sync::route_update_object(
        &f.engine,
        no_connection,
        &Credentials,
        MANAGED_ID,
        body_patch("Routed\n", &revision),
        Some("note"),
    )
    .unwrap();
    // Mutations return the canonical parse of the written file.
    assert_eq!(updated.value.body, "Routed");
    let moved = crate::sync::route_move_object(
        &f.engine,
        no_connection,
        &Credentials,
        MANAGED_ID,
        "moved.md",
        &updated.revision,
    )
    .unwrap();
    crate::sync::route_delete_object(
        &f.engine,
        no_connection,
        &Credentials,
        MANAGED_ID,
        &moved.revision,
    )
    .unwrap();
    crate::sync::route_create_object(
        &f.engine,
        no_connection,
        &Credentials,
        crate::CreateObjectInput {
            object_type: "note".into(),
            title: "Local".into(),
            body: String::new(),
            relative_path: None,
            properties: BTreeMap::new(),
        },
    )
    .unwrap();
    assert!(f.engine.sync_outbox().unwrap().is_empty());
}

#[test]
fn coordinator_rejects_when_plugin_disabled() {
    let f = fixture();
    disable_sync_plugin(&f.engine);
    let connection = connection();
    let errors = [
        crate::sync::WorkspaceSyncCoordinator::conflicts(&f.engine, &connection, &Credentials)
            .unwrap_err(),
        crate::sync::WorkspaceSyncCoordinator::collaboration_open(
            &f.engine,
            &connection,
            &Credentials,
            CollaborationOpenInput {
                relative_path: "note.md".into(),
            },
        )
        .unwrap_err(),
        crate::sync::WorkspaceSyncCoordinator::collaboration_flush(
            &f.engine,
            &connection,
            &Credentials,
            "session",
        )
        .unwrap_err(),
    ];
    for error in errors {
        assert_eq!(error.code, "sync_plugin_disabled");
        assert_eq!(
            error.details,
            Some(serde_json::json!({ "pluginId": "sync" }))
        );
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let pass = runtime
        .block_on(crate::sync::WorkspaceSyncCoordinator::pass(
            &f.engine,
            &connection,
            &Credentials,
        ))
        .unwrap_err();
    assert_eq!(pass.code, "sync_plugin_disabled");
    let enable = runtime
        .block_on(crate::sync::WorkspaceSyncCoordinator::enable(
            &f.engine,
            &connection,
            &Credentials,
        ))
        .unwrap_err();
    assert_eq!(enable.code, "sync_plugin_disabled");
}
