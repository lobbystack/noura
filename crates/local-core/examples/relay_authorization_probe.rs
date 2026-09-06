//! Opt-in security regression probe using only disposable files and synthetic identities.
//! Exits unsuccessfully while an approved viewer can write through a malicious relay.
use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{Read, Write},
    net::TcpListener,
    time::Duration,
};

use base64::{Engine as _, engine::general_purpose::STANDARD};
use local_core::{Result, WorkspaceEngine, sync::*};
use zeroize::Zeroizing;

#[derive(Default)]
struct Credentials(RefCell<BTreeMap<String, String>>);
impl SyncCredentials for Credentials {
    fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
        self.0
            .borrow()
            .get(reference)
            .cloned()
            .map(Zeroizing::new)
            .ok_or_else(|| {
                local_core::CoreError::validation(
                    "missing",
                    "Synthetic credential missing",
                    "probe",
                )
            })
    }
    fn write(&self, reference: &str, value: &str) -> Result<()> {
        self.0.borrow_mut().insert(reference.into(), value.into());
        Ok(())
    }
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let credentials = Credentials::default();
    let owner = DeviceKeys::create(&credentials)?;
    let viewer = DeviceKeys::create(&credentials)?;
    let directory = tempfile::tempdir()?;
    let source = WorkspaceEngine::create_with_app_data(
        directory.path().join("source"),
        "Synthetic attack test",
        directory.path().join("app1"),
    )?;
    let workspace = source.manifest().id;
    let original = "Authorized owner content\n";
    let replacement = "Unauthorized viewer replacement\n";
    let key = ObjectKey::generate();
    std::fs::write(source.root().join("note.txt"), original)?;
    let captured = source
        .sync_capture_file("note.txt", &key, owner.signer(), owner.device_id(), 1)?
        .ok_or("missing capture")?;
    let policy = AccessPolicy::sign(
        &workspace,
        "1",
        None,
        &owner,
        vec![
            AccessMember {
                account_id: "owner_account".into(),
                role: WorkspaceRole::Owner,
            },
            AccessMember {
                account_id: "viewer_account".into(),
                role: WorkspaceRole::Viewer,
            },
        ],
        vec![AccessObject {
            object_id: captured.object_id.clone(),
            epoch: 1,
            grants: vec![],
            envelopes: vec![
                owner
                    .wrap_key(
                        &workspace,
                        &captured.object_id,
                        1,
                        viewer.device_id(),
                        &viewer.recipient(),
                        &key,
                    )?
                    .into(),
            ],
        }],
    )?;
    policy.verify(&owner.signer().public_key())?;
    let plaintext = captured.open(&key, &owner.signer().public_key())?;
    let operation = owner.signer().seal_at_revision(
        &key,
        &workspace,
        &captured.object_id,
        owner.device_id(),
        (1, "1"),
        &plaintext,
    )?;
    // The viewer legitimately has the content key and its own pinned signing key.
    // Nothing here steals another device's signing key or changes the owner policy.
    let envelope = &policy.objects[0].envelopes[0];
    let viewer_key = viewer.unwrap_key(
        &KeyEnvelope {
            workspace_id: workspace.clone(),
            object_id: operation.object_id.clone(),
            epoch: 1,
            device_id: viewer.device_id().into(),
            signing_device: owner.device_id().into(),
            wrapped_key: envelope.wrapped_key.clone(),
            signature: envelope.signature.clone(),
        },
        &owner.signer().public_key(),
    )?;
    let forged = viewer.signer().seal_at_revision(
        &viewer_key,
        &workspace,
        &operation.object_id,
        viewer.device_id(),
        (1, "1"),
        &serde_json::to_vec(&FileChange {
            version: 1,
            path: "note.txt".into(),
            previous_path: None,
            base_revision: Some(blake3::hash(original.as_bytes()).to_hex().to_string()),
            content: Some(STANDARD.encode(replacement)),
            accepted_revisions: None,
            blob: None,
        })?,
    )?;
    let target_path = directory.path().join("target");
    std::fs::create_dir(&target_path)?;
    std::fs::copy(
        source.root().join("workspace.yaml"),
        target_path.join("workspace.yaml"),
    )?;
    let target = WorkspaceEngine::open_with_app_data(&target_path, directory.path().join("app2"))?;
    target.sync_accept_access_policy("0", None, &policy)?;
    let mut secrets = SyncSecrets::default();
    secrets
        .objects
        .insert((operation.object_id.clone(), 1), key);
    secrets
        .trusted_devices
        .insert(owner.device_id().into(), owner.signer().public_key());
    secrets
        .trusted_devices
        .insert(viewer.device_id().into(), viewer.signer().public_key());
    secrets
        .authorized_workspace_writers
        .insert(owner.device_id().into());
    secrets
        .historical_workspace_writers
        .entry("1".into())
        .or_default()
        .insert(owner.device_id().into());
    let entries = [operation, forged]
        .into_iter()
        .enumerate()
        .map(|(index, op)| {
            let mut value = serde_json::to_value(op).expect("synthetic envelope serializes");
            value["sequence"] = (index + 1).to_string().into();
            value
        })
        .collect::<Vec<_>>();
    let response = serde_json::to_vec(
        &serde_json::json!({ "operations": entries, "cursor": "2", "accessRevision": "1", "hasMore": false }),
    )?;
    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    let relay = std::thread::spawn(move || -> std::io::Result<()> {
        let (mut stream, _) = listener.accept()?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let mut header = Vec::new();
        while !header.ends_with(b"\r\n\r\n") {
            let mut byte = [0];
            stream.read_exact(&mut byte)?;
            header.push(byte[0]);
            if header.len() > 8192 {
                return Err(std::io::Error::other("Oversized test request"));
            }
        }
        if !header.starts_with(b"GET /v1/workspaces/") {
            return Err(std::io::Error::other("Unexpected test request"));
        }
        write!(
            stream,
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            response.len()
        )?;
        stream.write_all(&response)
    });
    let transport = HttpSyncTransport::new(&format!("http://{address}"), "x".repeat(43))?;
    let result = transport.synchronize(&target, &secrets).await;
    relay.join().map_err(|_| "test relay panicked")??;
    let overwritten =
        std::fs::read_to_string(target_path.join("note.txt")).is_ok_and(|text| text == replacement);
    println!(
        "{}",
        serde_json::json!({ "viewerWriteApplied": overwritten, "transportSucceeded": result.is_ok(), "errorCode": result.as_ref().err().map(|error| &error.code) })
    );
    if overwritten {
        return Err(
            "release blocker: malicious relay applied a viewer-authored replacement".into(),
        );
    }
    match result {
        Err(error) if error.code == "sync_writer_not_authorized" => Ok(()),
        _ => Err(
            "probe requires an explicit writer-authorization rejection, not an unrelated failure"
                .into(),
        ),
    }
}
