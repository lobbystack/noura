//! Native key-exchange integration probe. Only public identity metadata and ciphertext reach stdout.
use base64::{Engine as _, engine::general_purpose::STANDARD};
use local_core::{Result, WorkspaceEngine, sync::*};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{BufRead, Write},
};
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
            .ok_or_else(|| {
                local_core::CoreError::validation("missing", "Test credential missing", "test")
            })
    }
    fn write(&self, reference: &str, value: &str) -> Result<()> {
        self.0.borrow_mut().insert(reference.into(), value.into());
        Ok(())
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut lines = std::io::stdin().lock().lines();
    let input: serde_json::Value = serde_json::from_str(&lines.next().ok_or("input missing")??)?;
    let store = Memory::default();
    let owner = DeviceKeys::create(&store)?;
    let reader = DeviceKeys::create(&store)?;
    let peer = DeviceKeys::create(&store)?;
    let editor = DeviceKeys::create(&store)?;
    let viewer = DeviceKeys::create(&store)?;
    let account_for = |device: &DeviceKeys| {
        input[if device.device_id() == reader.device_id() {
            "readerAccount"
        } else if device.device_id() == editor.device_id() {
            "editorAccount"
        } else if device.device_id() == viewer.device_id() {
            "viewerAccount"
        } else {
            "ownerAccount"
        }]
        .as_str()
        .ok_or("account missing")
    };
    let directory = tempfile::TempDir::new()?;
    let engine = WorkspaceEngine::create_with_app_data(
        directory.path().join("workspace"),
        "Key test",
        directory.path().join("app"),
    )?;
    let workspace = engine.manifest().id;
    let key = ObjectKey::generate();
    let envelopes = [&owner, &reader, &editor, &viewer]
        .into_iter()
        .map(|device| {
            owner
                .wrap_key(
                    &workspace,
                    "object",
                    1,
                    device.device_id(),
                    &device.recipient(),
                    &key,
                )
                .map(PolicyEnvelope::from)
        })
        .collect::<Result<Vec<_>>>()?;
    let policy = AccessPolicy::sign(
        &workspace,
        "1",
        None,
        &owner,
        [&owner, &editor, &viewer]
            .into_iter()
            .map(|device| {
                Ok(AccessMember {
                    account_id: account_for(device)?.into(),
                    role: if device.device_id() == owner.device_id() {
                        WorkspaceRole::Owner
                    } else if device.device_id() == editor.device_id() {
                        WorkspaceRole::Editor
                    } else {
                        WorkspaceRole::Viewer
                    },
                })
            })
            .collect::<std::result::Result<Vec<_>, &str>>()?,
        vec![AccessObject {
            object_id: "object".into(),
            epoch: 1,
            grants: vec![ObjectGrant {
                account_id: input["readerAccount"]
                    .as_str()
                    .ok_or("reader missing")?
                    .into(),
                role: ObjectRole::Viewer,
            }],
            envelopes,
        }],
    )?;
    let operation = owner.signer().seal_at_revision(
        &key,
        &workspace,
        "object",
        owner.device_id(),
        (1, "1"),
        &serde_json::to_vec(&FileChange {
            version: 1,
            path: "shared.md".into(),
            previous_path: None,
            base_revision: None,
            content: Some(STANDARD.encode(b"private shared content")),
            accepted_revisions: None,
            blob: None,
        })?,
    )?;
    println!(
        "{}",
        serde_json::json!({"workspaceId":workspace,"owner":{"deviceId":owner.device_id(),"publicKey":owner.signer().public_key(),"recipient":owner.recipient()},"reader":{"deviceId":reader.device_id(),"publicKey":reader.signer().public_key(),"recipient":reader.recipient()},"peer":{"deviceId":peer.device_id(),"publicKey":peer.signer().public_key(),"recipient":peer.recipient()},"editor":{"deviceId":editor.device_id(),"publicKey":editor.signer().public_key(),"recipient":editor.recipient()},"viewer":{"deviceId":viewer.device_id(),"publicKey":viewer.signer().public_key(),"recipient":viewer.recipient()},"policy":policy,"operation":operation})
    );
    std::io::stdout().flush()?;
    let ready: serde_json::Value =
        serde_json::from_str(&lines.next().ok_or("server readiness missing")??)?;
    if ready["ready"] != true {
        return Err("server not ready".into());
    }
    let transport = HttpSyncTransport::new(
        input["origin"].as_str().ok_or("origin missing")?,
        input["token"].as_str().ok_or("token missing")?.into(),
    )?;
    let mut secrets = SyncSecrets::default();
    // Merely receiving a signer public key from /keys does not establish trust.
    let untrusted = transport
        .receive_keys(&engine, &reader, &mut secrets)
        .await
        .unwrap_err();
    assert_eq!(untrusted.code, "sync_untrusted_device");
    assert!(engine.sync_cursor()? == "0");
    secrets
        .trusted_devices
        .insert(owner.device_id().into(), owner.signer().public_key());
    secrets
        .authorized_workspace_writers
        .insert(owner.device_id().into());
    secrets
        .historical_workspace_writers
        .entry("1".into())
        .or_default()
        .insert(owner.device_id().into());
    engine.sync_accept_access_policy("0", None, &policy)?;
    transport
        .receive_keys(&engine, &reader, &mut secrets)
        .await?;
    let pass = transport.synchronize(&engine, &secrets).await?;
    assert_eq!(pass.downloaded, 1);
    assert_eq!(
        std::fs::read(engine.root().join("shared.md"))?,
        b"private shared content"
    );
    engine.rebuild_index()?;
    let restored = engine.sync_load_key("object", 1, &reader, &owner.signer().public_key())?;
    assert!(
        operation
            .open(&restored, &owner.signer().public_key())
            .is_ok()
    );
    let origin = input["origin"].as_str().ok_or("origin missing")?;
    let owner_connection = DeviceConnection {
        origin: origin.into(),
        device_id: owner.device_id().into(),
        account_id: input["ownerAccount"]
            .as_str()
            .ok_or("owner account missing")?
            .into(),
        token_reference: "probe_owner".into(),
    };
    let peer_connection = DeviceConnection {
        origin: origin.into(),
        device_id: peer.device_id().into(),
        account_id: input["ownerAccount"]
            .as_str()
            .ok_or("owner account missing")?
            .into(),
        token_reference: "probe_peer".into(),
    };
    store.write(
        "probe_owner",
        input["ownerToken"].as_str().ok_or("owner token missing")?,
    )?;
    store.write(
        "probe_peer",
        input["peerToken"].as_str().ok_or("peer token missing")?,
    )?;
    let owner_replica = WorkspaceEngine::create_sync_replica(
        directory.path().join("owner"),
        "Owner",
        &workspace,
        directory.path().join("owner_app"),
    )?;
    WorkspaceSyncCoordinator::enable(&owner_replica, &owner_connection, &store).await?;
    for approved in [&reader, &peer, &editor, &viewer] {
        let fingerprint = device_fingerprint(
            approved.device_id(),
            account_for(approved)?,
            &approved.signer().public_key(),
            &approved.recipient(),
        )?;
        WorkspaceSyncCoordinator::approve_device(
            &owner_replica,
            &owner_connection,
            &store,
            approved.device_id(),
            &fingerprint,
        )
        .await?;
    }
    WorkspaceSyncCoordinator::pass(&owner_replica, &owner_connection, &store)
        .await
        .map_err(|error| format!("owner initial pass: {error:?}"))?;
    assert_eq!(
        std::fs::read(owner_replica.root().join("shared.md"))?,
        b"private shared content"
    );
    std::fs::write(owner_replica.root().join("peer.txt"), b"first device edit")?;
    WorkspaceSyncCoordinator::pass(&owner_replica, &owner_connection, &store)
        .await
        .map_err(|error| format!("owner upload peer file: {error:?}"))?;
    let peer_replica = WorkspaceSyncCoordinator::join(
        directory.path().join("peer"),
        "Peer",
        &workspace,
        directory.path().join("peer_app"),
        &peer_connection,
        &store,
    )
    .await?;
    assert!(!peer_replica.sync_status()?.enabled);
    peer_replica.sync_pause(false)?;
    let untrusted = WorkspaceSyncCoordinator::pass(&peer_replica, &peer_connection, &store)
        .await
        .unwrap_err();
    assert_eq!(untrusted.code, "sync_untrusted_device");
    for approved in [&owner, &reader, &editor, &viewer] {
        let fingerprint = device_fingerprint(
            approved.device_id(),
            account_for(approved)?,
            &approved.signer().public_key(),
            &approved.recipient(),
        )?;
        WorkspaceSyncCoordinator::approve_device(
            &peer_replica,
            &peer_connection,
            &store,
            approved.device_id(),
            &fingerprint,
        )
        .await?;
    }
    WorkspaceSyncCoordinator::pass(&peer_replica, &peer_connection, &store)
        .await
        .map_err(|error| format!("peer initial pass: {error:?}"))?;
    assert_eq!(
        std::fs::read(peer_replica.root().join("peer.txt"))?,
        b"first device edit"
    );
    std::fs::write(peer_replica.root().join("peer.txt"), b"second device edit")?;
    WorkspaceSyncCoordinator::pass(&peer_replica, &peer_connection, &store)
        .await
        .map_err(|error| format!("peer edit pass: {error:?}"))?;
    WorkspaceSyncCoordinator::pass(&owner_replica, &owner_connection, &store)
        .await
        .map_err(|error| format!("owner receives peer edit: {error:?}"))?;
    assert_eq!(
        std::fs::read(owner_replica.root().join("peer.txt"))?,
        b"second device edit"
    );
    std::fs::write(owner_replica.root().join("peer.txt"), b"owner branch")?;
    std::fs::write(peer_replica.root().join("peer.txt"), b"peer branch")?;
    WorkspaceSyncCoordinator::pass(&owner_replica, &owner_connection, &store)
        .await
        .map_err(|error| format!("owner conflict branch: {error:?}"))?;
    WorkspaceSyncCoordinator::pass(&peer_replica, &peer_connection, &store)
        .await
        .map_err(|error| format!("peer conflict branch: {error:?}"))?;
    WorkspaceSyncCoordinator::pass(&owner_replica, &owner_connection, &store)
        .await
        .map_err(|error| format!("owner conflict receive: {error:?}"))?;
    let conflicts = WorkspaceSyncCoordinator::conflicts(&peer_replica, &peer_connection, &store)?;
    assert_eq!(conflicts.len(), 1);
    WorkspaceSyncCoordinator::resolve_conflict(
        &peer_replica,
        &peer_connection,
        &store,
        &ResolveSyncConflict {
            operation_id: conflicts[0].operation_id.clone(),
            current_revision: conflicts[0].current_revision.clone(),
            choice: SyncResolutionChoice::Remote,
        },
    )?;
    WorkspaceSyncCoordinator::pass(&peer_replica, &peer_connection, &store)
        .await
        .map_err(|error| format!("peer conflict resolution: {error:?}"))?;
    WorkspaceSyncCoordinator::pass(&owner_replica, &owner_connection, &store)
        .await
        .map_err(|error| format!("owner conflict resolution receive: {error:?}"))?;
    assert_eq!(
        std::fs::read(owner_replica.root().join("peer.txt"))?,
        b"owner branch"
    );
    assert_eq!(
        std::fs::read(peer_replica.root().join("peer.txt"))?,
        b"owner branch"
    );
    assert_eq!(owner_replica.sync_status()?.conflicts, 0);
    assert_eq!(peer_replica.sync_status()?.conflicts, 0);
    let attachment: Vec<u8> = (0..2 * 1024 * 1024 + 123)
        .map(|i| (i % 251) as u8)
        .collect();
    std::fs::write(
        owner_replica.root().join("private-attachment.bin"),
        &attachment,
    )?;
    let mut captured = owner_replica.sync_restore_secrets(
        &owner,
        &owner_replica
            .sync_configuration()?
            .ok_or("config missing")?
            .trusted_devices,
    )?;
    owner_replica.sync_capture_workspace(&owner, &mut captured)?;
    let attachment_op = owner_replica
        .sync_outbox()?
        .pop()
        .ok_or("attachment missing")?;
    let attachment_key = captured
        .objects
        .get(&(attachment_op.object_id.clone(), attachment_op.epoch))
        .ok_or("key missing")?;
    let change: FileChange =
        serde_json::from_slice(&attachment_op.open(attachment_key, &owner.signer().public_key())?)?;
    let blob = change.blob.ok_or("descriptor missing")?;
    // Simulate restart after a durable partial download, including a non-chunk-aligned offset.
    let partial_directory = peer_replica.root().join(".noura/sync/blobs");
    std::fs::create_dir_all(&partial_directory)?;
    let ciphertext = std::fs::read(
        owner_replica
            .root()
            .join(".noura/sync/blobs")
            .join(&blob.id),
    )?;
    std::fs::write(partial_directory.join(&blob.id), &ciphertext[..1_048_593])?;
    std::fs::File::open(partial_directory.join(&blob.id))?.sync_all()?;
    WorkspaceSyncCoordinator::pass(&owner_replica, &owner_connection, &store)
        .await
        .map_err(|error| format!("owner attachment upload: {error:?}"))?;
    WorkspaceSyncCoordinator::pass(&peer_replica, &peer_connection, &store)
        .await
        .map_err(|error| format!("peer attachment download: {error:?}"))?;
    assert_eq!(
        std::fs::read(peer_replica.root().join("private-attachment.bin"))?,
        attachment
    );
    peer_replica.rebuild_index()?;
    assert_eq!(
        std::fs::read(peer_replica.root().join("private-attachment.bin"))?,
        attachment
    );
    let recovery_path = directory.path().join("workspace-recovery.json");
    WorkspaceSyncCoordinator::export_recovery_kit(
        &owner_replica,
        &owner_connection,
        &store,
        &recovery_path,
    )?;
    let mut member_replicas = Vec::new();
    for (device, label) in [(&editor, "editor"), (&viewer, "viewer")] {
        let connection = DeviceConnection {
            origin: origin.into(),
            device_id: device.device_id().into(),
            account_id: account_for(device)?.into(),
            token_reference: label.into(),
        };
        store.write(
            label,
            input[format!("{label}Token")]
                .as_str()
                .ok_or("token missing")?,
        )?;
        let replica = WorkspaceSyncCoordinator::join(
            directory.path().join(label),
            label,
            &workspace,
            directory.path().join(format!("{label}_app")),
            &connection,
            &store,
        )
        .await?;
        for approved in [&owner, &reader, &peer, &editor, &viewer] {
            if approved.device_id() == device.device_id() {
                continue;
            }
            let fingerprint = device_fingerprint(
                approved.device_id(),
                account_for(approved)?,
                &approved.signer().public_key(),
                &approved.recipient(),
            )?;
            WorkspaceSyncCoordinator::approve_device(
                &replica,
                &connection,
                &store,
                approved.device_id(),
                &fingerprint,
            )
            .await?;
        }
        replica.sync_pause(false)?;
        WorkspaceSyncCoordinator::pass(&replica, &connection, &store)
            .await
            .map_err(|error| format!("{label} initial pass: {error:?}"))?;
        assert_eq!(
            std::fs::read(replica.root().join("private-attachment.bin"))?,
            attachment
        );
        member_replicas.push((replica, connection));
    }
    let (editor_replica, editor_connection) = &member_replicas[0];
    std::fs::write(
        editor_replica.root().join("editor-created.txt"),
        b"editor created this object",
    )?;
    WorkspaceSyncCoordinator::pass(editor_replica, editor_connection, &store)
        .await
        .map_err(|error| format!("editor create pass: {error:?}"))?;
    let (viewer_replica, viewer_connection) = &member_replicas[1];
    std::fs::write(
        viewer_replica.root().join("local-only.txt"),
        b"viewer local file",
    )?;
    WorkspaceSyncCoordinator::pass(viewer_replica, viewer_connection, &store)
        .await
        .map_err(|error| format!("viewer receive pass: {error:?}"))?;
    assert_eq!(
        std::fs::read(viewer_replica.root().join("editor-created.txt"))?,
        b"editor created this object"
    );
    assert!(viewer_replica.sync_outbox()?.is_empty());
    WorkspaceSyncCoordinator::pass(&owner_replica, &owner_connection, &store)
        .await
        .map_err(|error| format!("owner receives editor object: {error:?}"))?;
    assert_eq!(
        std::fs::read(owner_replica.root().join("editor-created.txt"))?,
        b"editor created this object"
    );
    assert!(!owner_replica.root().join("local-only.txt").exists());
    let clean_store = Memory::default();
    clean_store.write(peer.device_id(), &store.read(peer.device_id())?)?;
    clean_store.write(
        "probe_peer",
        input["peerToken"].as_str().ok_or("token missing")?,
    )?;
    assert!(clean_store.read(owner.device_id()).is_err());
    let recovered = WorkspaceSyncCoordinator::join(
        directory.path().join("recovered"),
        "Recovered",
        &workspace,
        directory.path().join("recovered_app"),
        &peer_connection,
        &clean_store,
    )
    .await?;
    WorkspaceSyncCoordinator::import_recovery_kit(
        &recovered,
        &peer_connection,
        &clean_store,
        &recovery_path,
    )
    .await?;
    assert!(!recovered.sync_status()?.enabled);
    assert_eq!(
        std::fs::read(recovered.root().join("private-attachment.bin"))?,
        attachment
    );
    // This object was created after the kit: its envelope is recovered from the old recipient's server inbox.
    assert_eq!(
        std::fs::read(recovered.root().join("editor-created.txt"))?,
        b"editor created this object"
    );
    std::fs::write(
        recovered.root().join("editor-created.txt"),
        b"external edit after recovery",
    )?;
    WorkspaceSyncCoordinator::import_recovery_kit(
        &recovered,
        &peer_connection,
        &clean_store,
        &recovery_path,
    )
    .await?;
    assert_eq!(
        std::fs::read(recovered.root().join("editor-created.txt"))?,
        b"external edit after recovery"
    );
    let invitation_id = ready["invitationId"].as_str().ok_or("invitation missing")?;
    let mut approval_config = owner_replica
        .sync_configuration()?
        .ok_or("configuration missing")?;
    approval_config
        .approved_recipients
        .remove(reader.device_id());
    approval_config.approved_accounts.remove(reader.device_id());
    owner_replica.sync_save_configuration(&approval_config)?;
    let rejected = WorkspaceSyncCoordinator::finalize_invitation(
        &owner_replica,
        &owner_connection,
        &store,
        invitation_id,
    )
    .await
    .unwrap_err();
    assert_eq!(rejected.code, "sync_device_approval_required");
    let fingerprint = device_fingerprint(
        reader.device_id(),
        account_for(&reader)?,
        &reader.signer().public_key(),
        &reader.recipient(),
    )?;
    WorkspaceSyncCoordinator::approve_invited_device(
        &owner_replica,
        &owner_connection,
        &store,
        invitation_id,
        reader.device_id(),
        &fingerprint,
    )
    .await?;
    let invitations =
        WorkspaceSyncCoordinator::invitations(&owner_replica, &owner_connection, &store).await?;
    let invitation = invitations
        .iter()
        .find(|item| item.id == invitation_id)
        .ok_or("invitation not returned")?;
    assert_eq!(invitation.status, SyncInvitationStatus::Accepted);
    WorkspaceSyncCoordinator::finalize_invitation(
        &owner_replica,
        &owner_connection,
        &store,
        invitation_id,
    )
    .await
    .map_err(|error| format!("finalize invitation: {error:?}"))?;
    let invitations =
        WorkspaceSyncCoordinator::invitations(&owner_replica, &owner_connection, &store).await?;
    assert_eq!(
        invitations
            .iter()
            .find(|item| item.id == invitation_id)
            .ok_or("completed invitation missing")?
            .status,
        SyncInvitationStatus::Completed
    );
    println!(
        "{}",
        serde_json::json!({"decrypted":true,"reloaded":true,"rejectedUntrustedSigner":true,"approvedCoordinatorExchange":true,"resolvedConflict":true,"attachmentExchange":true,"memberReplicas":true,"recoveryKit":true,"invitationFinalized":true})
    );
    Ok(())
}
