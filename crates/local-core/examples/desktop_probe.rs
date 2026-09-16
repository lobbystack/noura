//! Integration probe for live browser-to-desktop sync.
//!
//! It runs in two modes over stdin JSON:
//!
//! * `bootstrap` (default): enrolls a native device, creates a workspace, uploads an
//!   encrypted note, and shares its object key with a browser device. This is the
//!   original single-invocation behavior and keeps its stdout fields.
//! * `pull`: reopens the workspace persisted by a prior `bootstrap`, pins the browser
//!   device, and downloads/decrypts the operation the browser authored.
//!
//! When `statePath` is supplied, `bootstrap` writes a **test-only** state file that
//! carries everything the `pull` run needs: the native signing seed, the age identity
//! secret, the bearer token, the workspace/object identifiers, and the device-wrapped
//! object key envelope. This file intentionally stores plaintext secrets in an ordinary
//! local file for integration testing. It is **not** a credential store: never point
//! `statePath` at a real credential location, never reuse it for real workspaces, and
//! delete it when the probe finishes. The file is written with owner-only permissions
//! and is never printed.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use age::secrecy::ExposeSecret;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::Signer as _;
use local_core::{
    CoreError, Result, WorkspaceEngine,
    sync::{
        AccessMember, AccessObject, AccessPolicy, DeviceKeys, HttpSyncTransport, KeyEnvelope,
        ObjectKey, PolicyEnvelope, SyncCredentials, SyncSecrets, WorkspaceRole,
    },
};
use reqwest::Method;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

const DEFAULT_TARGET_FILE: &str = "notes/native-note.md";

#[derive(Default)]
struct Memory(RefCell<BTreeMap<String, String>>);

impl SyncCredentials for Memory {
    fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
        self.0
            .borrow()
            .get(reference)
            .cloned()
            .map(Zeroizing::new)
            .ok_or_else(|| CoreError::validation("missing", "Test credential missing", "probe"))
    }
    fn write(&self, reference: &str, value: &str) -> Result<()> {
        self.0.borrow_mut().insert(reference.into(), value.into());
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Mode {
    #[default]
    Bootstrap,
    Pull,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    #[serde(default)]
    mode: Mode,
    origin: String,
    #[serde(default)]
    cookie: Option<String>,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    browser_device_id: Option<String>,
    #[serde(default)]
    browser_signing_public: Option<String>,
    #[serde(default)]
    browser_recipient: Option<String>,
    #[serde(default)]
    state_path: Option<PathBuf>,
    #[serde(default)]
    workspace_path: Option<PathBuf>,
    #[serde(default)]
    target_file: Option<String>,
}

/// Test-only persisted probe session. Contains plaintext secrets by design.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProbeState {
    version: u8,
    signing_seed: String,
    recipient_identity: String,
    device_id: String,
    token: String,
    workspace_id: String,
    workspace_path: String,
    app_data_path: String,
    object_id: String,
    epoch: u64,
    object_key_envelope: KeyEnvelope,
}

async fn send_json(
    client: &reqwest::Client,
    method: Method,
    url: String,
    headers: &[(&str, &str)],
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> std::result::Result<serde_json::Value, Box<dyn std::error::Error>> {
    let mut request = client.request(method.clone(), url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    if let Some(token) = token {
        request = request.bearer_auth(token);
    }
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    let status = response.status();
    let text = response.text().await?;
    if !status.is_success() {
        let code = serde_json::from_str::<serde_json::Value>(&text)
            .ok()
            .and_then(|value| value["error"]["code"].as_str().map(str::to_owned))
            .unwrap_or_else(|| "unknown".into());
        return Err(format!("{method} request failed with {status} ({code})").into());
    }
    Ok(serde_json::from_str(&text)?)
}

fn decode_public(value: &str) -> std::result::Result<[u8; 32], Box<dyn std::error::Error>> {
    let bytes = STANDARD.decode(value)?;
    let bytes: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| "public key must decode to 32 bytes")?;
    Ok(bytes)
}

fn random_seed() -> [u8; 32] {
    use aes_gcm::aead::{OsRng, rand_core::RngCore};
    let mut bytes = [0_u8; 32];
    OsRng.fill_bytes(&mut bytes);
    bytes
}

fn write_state(
    path: &Path,
    state: &ProbeState,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = Zeroizing::new(serde_json::to_vec(state)?);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    file.write_all(bytes.as_slice())?;
    file.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

fn read_state(path: &Path) -> std::result::Result<ProbeState, Box<dyn std::error::Error>> {
    let bytes = Zeroizing::new(std::fs::read(path)?);
    Ok(serde_json::from_slice(&bytes)?)
}

fn snapshot_files(
    root: &Path,
) -> std::result::Result<BTreeMap<String, Vec<u8>>, Box<dyn std::error::Error>> {
    let mut files = BTreeMap::new();
    collect_files(root, root, &mut files)?;
    Ok(files)
}

fn collect_files(
    root: &Path,
    directory: &Path,
    files: &mut BTreeMap<String, Vec<u8>>,
) -> std::result::Result<(), Box<dyn std::error::Error>> {
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        let name = entry.file_name();
        if name.to_str() == Some(".noura") {
            continue;
        }
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_files(root, &path, files)?;
        } else if file_type.is_file() {
            let relative = path
                .strip_prefix(root)?
                .to_string_lossy()
                .replace('\\', "/");
            files.insert(relative, std::fs::read(&path)?);
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut raw = Zeroizing::new(String::new());
    std::io::stdin().read_to_string(&mut raw)?;
    let input: Input = serde_json::from_str(&raw)?;
    match input.mode {
        Mode::Bootstrap => run_bootstrap(input).await,
        Mode::Pull => run_pull(input).await,
    }
}

async fn run_bootstrap(input: Input) -> std::result::Result<(), Box<dyn std::error::Error>> {
    if input.state_path.is_some() && input.workspace_path.is_none() {
        return Err("workspacePath is required when statePath is set".into());
    }
    let cookie = input
        .cookie
        .as_deref()
        .ok_or("cookie is required in bootstrap mode")?;
    let account_id = input
        .account_id
        .as_deref()
        .ok_or("accountId is required in bootstrap mode")?;
    let browser_device_id = input
        .browser_device_id
        .as_deref()
        .ok_or("browserDeviceId is required in bootstrap mode")?;
    let browser_signing_public = input
        .browser_signing_public
        .as_deref()
        .ok_or("browserSigningPublic is required in bootstrap mode")?;
    let browser_recipient = input
        .browser_recipient
        .as_deref()
        .ok_or("browserRecipient is required in bootstrap mode")?;
    let origin = input.origin.clone();

    let (workspace_root, app_data, tempdir) = match input.workspace_path.as_deref() {
        Some(path) => {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("workspace");
            (
                path.to_path_buf(),
                path.with_file_name(format!("{name}-app-data")),
                None,
            )
        }
        None => {
            let directory = tempfile::TempDir::new()?;
            let root = directory.path().join("workspace");
            let app = directory.path().join("app");
            (root, app, Some(directory))
        }
    };

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(10))
        .timeout(std::time::Duration::from_secs(30))
        .build()?;

    let store = Memory::default();
    let seed = random_seed();
    let identity = age::x25519::Identity::generate();
    let device_id = format!("device_{}", uuid::Uuid::new_v4());
    let identity_secret = identity.to_string();
    let record = serde_json::json!({
        "version": 1,
        "signing_seed": STANDARD.encode(seed),
        "recipient_identity": identity_secret.expose_secret(),
        "construction": "age",
    });
    store.write(&device_id, &serde_json::to_string(&record)?)?;
    let device = DeviceKeys::load(&store, &device_id)?;
    let public_key = device.signer().public_key();
    let recipient = device.recipient();

    eprintln!("desktop_probe: requesting a device challenge");
    let challenge_body = send_json(
        &client,
        Method::POST,
        format!("{origin}/v1/device-challenges"),
        &[("Origin", origin.as_str()), ("Cookie", cookie)],
        None,
        Some(serde_json::json!({})),
    )
    .await?;
    let challenge = challenge_body["challenge"]
        .as_str()
        .ok_or("challenge missing")?
        .to_owned();
    let session_account = challenge_body["accountId"]
        .as_str()
        .ok_or("accountId missing")?;
    if session_account != account_id {
        return Err("input accountId does not match the authenticated session".into());
    }

    let proof_bytes = serde_json::to_vec(&(
        "noura.device.enroll.v2",
        origin.as_str(),
        account_id,
        device.device_id(),
        public_key.as_str(),
        recipient.as_str(),
        challenge.as_str(),
    ))?;
    let signing_key = ed25519_dalek::SigningKey::from_bytes(&seed);
    let proof = STANDARD.encode(signing_key.sign(&proof_bytes).to_bytes());

    eprintln!(
        "desktop_probe: enrolling native device {}",
        device.device_id()
    );
    let enrolled = send_json(
        &client,
        Method::POST,
        format!("{origin}/v1/devices"),
        &[("Origin", origin.as_str()), ("Cookie", cookie)],
        None,
        Some(serde_json::json!({
            "challenge": challenge,
            "deviceId": device.device_id(),
            "publicKey": public_key,
            "proof": proof,
            "encryptionRecipient": recipient,
        })),
    )
    .await?;
    let token = enrolled["token"]
        .as_str()
        .ok_or("token missing")?
        .to_owned();
    if enrolled["deviceId"].as_str() != Some(device.device_id()) {
        return Err("server returned a different device id".into());
    }

    let transport = HttpSyncTransport::new(&origin, token.clone())?;
    let engine = WorkspaceEngine::create_with_app_data(
        &workspace_root,
        "Desktop to browser test",
        &app_data,
    )?;
    let workspace = engine.manifest().id;
    eprintln!("desktop_probe: creating workspace {workspace}");
    transport.create_workspace(&workspace).await?;

    std::fs::create_dir_all(engine.root().join("notes"))?;
    std::fs::write(
        engine.root().join(DEFAULT_TARGET_FILE),
        b"# Native note\n\nWritten by the desktop probe and encrypted for the browser.\n",
    )?;
    let key = ObjectKey::generate();
    let operation = engine
        .sync_capture_file(
            DEFAULT_TARGET_FILE,
            &key,
            device.signer(),
            device.device_id(),
            1,
        )?
        .ok_or("capture produced no operation")?;
    let object_id = operation.object_id.clone();
    eprintln!("desktop_probe: captured object {object_id}");

    let created = send_json(
        &client,
        Method::POST,
        format!("{origin}/v1/workspaces/{workspace}/objects"),
        &[],
        Some(&token),
        Some(serde_json::json!({ "id": object_id })),
    )
    .await?;
    if created["epoch"].as_u64() != Some(1) {
        return Err("remote object is not at epoch 1".into());
    }

    let browser_public = decode_public(browser_signing_public)?;
    let device_list = send_json(
        &client,
        Method::GET,
        format!("{origin}/v1/devices"),
        &[],
        Some(&token),
        None,
    )
    .await?;
    let rows = device_list["devices"].as_array().ok_or("devices missing")?;
    let mut envelopes = Vec::new();
    let mut covered_browser = false;
    for row in rows {
        if row["revoked"].as_bool().unwrap_or(true) {
            continue;
        }
        let id = row["id"].as_str().ok_or("device id missing")?;
        let row_recipient = row["encryptionRecipient"]
            .as_str()
            .ok_or("enrolled device is missing an encryption recipient")?;
        if id == browser_device_id {
            if row_recipient != browser_recipient {
                return Err("browser recipient does not match the enrolled device".into());
            }
            let row_public = decode_public(
                row["publicKey"]
                    .as_str()
                    .ok_or("device public key missing")?,
            )?;
            if row_public != browser_public {
                return Err("browser signing key does not match the enrolled device".into());
            }
            covered_browser = true;
        }
        envelopes.push(PolicyEnvelope::from(device.wrap_key(
            &workspace,
            &object_id,
            1,
            id,
            row_recipient,
            &key,
        )?));
    }
    if envelopes.is_empty() {
        return Err("no active devices are enrolled for this account".into());
    }
    if !covered_browser {
        return Err("browser device is not enrolled for this account".into());
    }

    let policy = AccessPolicy::sign(
        &workspace,
        "1",
        None,
        &device,
        vec![AccessMember {
            account_id: account_id.to_owned(),
            role: WorkspaceRole::Owner,
        }],
        vec![AccessObject {
            document: None,
            object_id: object_id.clone(),
            epoch: 1,
            grants: vec![],
            envelopes,
        }],
    )?;
    eprintln!("desktop_probe: publishing access policy revision 1");
    transport
        .set_access(&policy, &device.signer().public_key())
        .await?;
    engine.sync_accept_access_policy("0", None, &policy)?;

    let mut secrets = SyncSecrets::default();
    secrets.objects.insert((object_id.clone(), 1), key);
    secrets
        .trusted_devices
        .insert(device.device_id().into(), device.signer().public_key());
    secrets
        .authorized_workspace_writers
        .insert(device.device_id().into());
    secrets
        .historical_workspace_writers
        .entry("0".into())
        .or_default()
        .insert(device.device_id().into());

    eprintln!("desktop_probe: synchronizing encrypted operation");
    let pass = transport.synchronize(&engine, &secrets).await?;
    eprintln!(
        "desktop_probe: uploaded {} downloaded {} conflicts {}",
        pass.uploaded, pass.downloaded, pass.conflicts
    );
    if pass.uploaded != 1 {
        return Err(format!(
            "expected exactly one uploaded operation, got {}",
            pass.uploaded
        )
        .into());
    }

    if let Some(state_path) = input.state_path.as_deref() {
        // Read the local device's own envelope from the policy we just built. The
        // engine may not have retained it, and only public metadata is persisted.
        let object_key_envelope = policy
            .objects
            .iter()
            .flat_map(|object| object.envelopes.iter())
            .find(|envelope| envelope.device_id == device.device_id())
            .map(|envelope| KeyEnvelope {
                workspace_id: workspace.clone(),
                object_id: object_id.clone(),
                epoch: 1,
                device_id: envelope.device_id.clone(),
                wrapped_key: envelope.wrapped_key.clone(),
                signing_device: device.device_id().to_owned(),
                signature: envelope.signature.clone(),
                construction: envelope.construction,
                recipient_public_key: envelope.recipient_public_key.clone(),
                ephemeral_public_key: envelope.ephemeral_public_key.clone(),
                salt: envelope.salt.clone(),
                nonce: envelope.nonce.clone(),
            })
            .ok_or("persisted object key envelope is missing")?;
        let state = ProbeState {
            version: 1,
            signing_seed: STANDARD.encode(seed),
            recipient_identity: identity_secret.expose_secret().to_owned(),
            device_id: device.device_id().to_owned(),
            token: token.clone(),
            workspace_id: workspace.clone(),
            workspace_path: engine.root().to_string_lossy().into_owned(),
            app_data_path: app_data.to_string_lossy().into_owned(),
            object_id: object_id.clone(),
            epoch: 1,
            object_key_envelope,
        };
        write_state(state_path, &state)?;
        eprintln!("desktop_probe: wrote test-only probe state");
    }

    let output = serde_json::json!({
        "workspace": workspace,
        "objectId": object_id,
        "epoch": 1,
        "deviceId": device.device_id(),
        "token": token,
        "file": DEFAULT_TARGET_FILE,
        "signingPublic": device.signer().public_key(),
        "recipient": device.recipient(),
    });
    println!("{}", serde_json::to_string(&output)?);
    std::io::stdout().flush()?;
    drop(tempdir);
    Ok(())
}

async fn run_pull(input: Input) -> std::result::Result<(), Box<dyn std::error::Error>> {
    let state_path = input
        .state_path
        .as_deref()
        .ok_or("statePath is required in pull mode")?;
    let browser_device_id = input
        .browser_device_id
        .as_deref()
        .ok_or("browserDeviceId is required in pull mode")?;
    let browser_signing_public = input
        .browser_signing_public
        .as_deref()
        .ok_or("browserSigningPublic is required in pull mode")?;
    let state = read_state(state_path)?;
    if state.version != 1 {
        return Err(format!("unsupported probe state version {}", state.version).into());
    }

    let store = Memory::default();
    let record = serde_json::json!({
        "version": 1,
        "signing_seed": state.signing_seed,
        "recipient_identity": state.recipient_identity,
        "construction": "age",
    });
    store.write(&state.device_id, &serde_json::to_string(&record)?)?;
    let device = DeviceKeys::load(&store, &state.device_id)?;

    let engine = WorkspaceEngine::open_with_app_data(&state.workspace_path, &state.app_data_path)?;
    if engine.manifest().id != state.workspace_id {
        return Err("probe state does not match the reopened workspace".into());
    }
    let transport = HttpSyncTransport::new(&input.origin, state.token.clone())?;
    let key = device.unwrap_key(&state.object_key_envelope, &device.signer().public_key())?;

    let mut secrets = SyncSecrets::default();
    secrets
        .objects
        .insert((state.object_id.clone(), state.epoch), key);
    secrets
        .trusted_devices
        .insert(state.device_id.clone(), device.signer().public_key());
    secrets.trusted_devices.insert(
        browser_device_id.to_owned(),
        browser_signing_public.to_owned(),
    );
    secrets
        .authorized_workspace_writers
        .insert(state.device_id.clone());
    secrets
        .authorized_workspace_writers
        .insert(browser_device_id.to_owned());
    let mut revisions = BTreeSet::new();
    revisions.insert("0".to_owned());
    revisions.insert("1".to_owned());
    revisions.insert(engine.sync_access_revision()?);
    for revision in revisions {
        let writers = secrets
            .historical_workspace_writers
            .entry(revision)
            .or_default();
        writers.insert(state.device_id.clone());
        writers.insert(browser_device_id.to_owned());
    }

    let before = snapshot_files(engine.root())?;
    eprintln!("desktop_probe: pulling encrypted operations");
    let pass = transport.synchronize(&engine, &secrets).await?;
    let after = snapshot_files(engine.root())?;
    let mut changed = Vec::new();
    for (path, bytes) in &after {
        if before.get(path).map(Vec::as_slice) != Some(bytes.as_slice()) {
            changed.push(path.clone());
        }
    }

    let target_file = input
        .target_file
        .clone()
        .unwrap_or_else(|| DEFAULT_TARGET_FILE.to_owned());
    let content = match std::fs::read(engine.root().join(&target_file)) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => text,
            Err(error) => STANDARD.encode(error.into_bytes()),
        },
        Err(_) => String::new(),
    };
    eprintln!(
        "desktop_probe: downloaded {} conflicts {}",
        pass.downloaded, pass.conflicts
    );

    let output = serde_json::json!({
        "downloaded": pass.downloaded,
        "files": changed,
        "targetFile": target_file,
        "content": content,
    });
    println!("{}", serde_json::to_string(&output)?);
    std::io::stdout().flush()?;
    Ok(())
}
