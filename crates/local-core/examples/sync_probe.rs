//! Integration-test client. The fixed test keys here MUST NOT be used for real workspaces.
use std::io::Read;

use local_core::{
    WORKSPACE_MANIFEST_PATH, WorkspaceEngine,
    sync::{HttpSyncTransport, ObjectKey, SigningIdentity, SyncSecrets},
};
use serde::Deserialize;

#[derive(Deserialize)]
struct Input {
    origin: String,
    token: String,
    device: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = zeroize::Zeroizing::new(String::new());
    std::io::stdin().read_to_string(&mut input)?;
    let input: Input = serde_json::from_str(&input)?;
    let signer = SigningIdentity::from_seed(&[7; 32]);
    let key = ObjectKey::from_bytes([9; 32]);
    let transport = HttpSyncTransport::new(&input.origin, input.token)?;
    let dir = tempfile::TempDir::new()?;
    let first = WorkspaceEngine::create_with_app_data(
        dir.path().join("first"),
        "Native sync test",
        dir.path().join("app1"),
    )?;
    transport.create_workspace(&first.manifest().id).await?;
    let sentinel = "native-plaintext-sentinel-private-path";
    std::fs::write(first.root().join("private-note.md"), sentinel)?;
    let op = first
        .sync_capture_file("private-note.md", &key, &signer, &input.device, 1)?
        .ok_or("no capture")?;
    let mut secrets = SyncSecrets::default();
    secrets.objects.insert((op.object_id.clone(), 1), key);
    secrets
        .trusted_devices
        .insert(input.device.clone(), signer.public_key());
    secrets
        .authorized_workspace_writers
        .insert(input.device.clone());
    secrets
        .historical_workspace_writers
        .entry("0".into())
        .or_default()
        .insert(input.device.clone());
    let first_root = first.root().to_owned();
    drop(first);
    let first = WorkspaceEngine::open_with_app_data(&first_root, dir.path().join("app1"))?;
    let uploaded = transport.synchronize(&first, &secrets).await?;
    assert_eq!(uploaded.uploaded, 1);
    let mut peers = Vec::new();
    for n in [2, 3] {
        let path = dir.path().join(format!("peer{n}"));
        std::fs::create_dir_all(path.join(".noura"))?;
        std::fs::copy(
            first.root().join(WORKSPACE_MANIFEST_PATH),
            path.join(WORKSPACE_MANIFEST_PATH),
        )?;
        let peer = WorkspaceEngine::open_with_app_data(&path, dir.path().join(format!("app{n}")))?;
        let pass = transport.synchronize(&peer, &secrets).await?;
        assert_eq!(pass.downloaded, 1);
        assert_eq!(
            std::fs::read_to_string(path.join("private-note.md"))?,
            sentinel
        );
        peers.push(peer);
    }
    std::fs::write(peers[0].root().join("private-note.md"), "offline peer edit")?;
    std::fs::write(
        first.root().join("private-note.md"),
        "remote second revision",
    )?;
    first.sync_capture_file(
        "private-note.md",
        secrets
            .objects
            .get(&(op.object_id.clone(), 1))
            .ok_or("missing key")?,
        &signer,
        &input.device,
        1,
    )?;
    transport.synchronize(&first, &secrets).await?;
    let conflict = transport.synchronize(&peers[0], &secrets).await?;
    assert_eq!(conflict.conflicts, 1);
    assert_eq!(
        std::fs::read_to_string(peers[0].root().join("private-note.md"))?,
        "offline peer edit"
    );
    let applied = transport.synchronize(&peers[1], &secrets).await?;
    assert_eq!(applied.conflicts, 0);
    assert_eq!(
        std::fs::read_to_string(peers[1].root().join("private-note.md"))?,
        "remote second revision"
    );
    let cursor = peers[1].sync_cursor()?;
    peers[1].rebuild_index()?;
    assert_eq!(peers[1].sync_cursor()?, cursor);
    println!(
        "{}",
        serde_json::json!({"workspace":first.manifest().id,"clients":3,"conflicts":conflict.conflicts,"cursor":cursor})
    );
    Ok(())
}
