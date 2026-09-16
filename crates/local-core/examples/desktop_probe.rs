use std::{
    cell::RefCell,
    collections::BTreeMap,
    io::{Read, Write},
};

use age::secrecy::ExposeSecret;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::Signer as _;
use local_core::{
    CoreError, Result, WorkspaceEngine,
    sync::{
        AccessMember, AccessObject, AccessPolicy, DeviceKeys, HttpSyncTransport, ObjectKey,
        PolicyEnvelope, SyncCredentials, SyncSecrets, WorkspaceRole,
    },
};
use reqwest::Method;
use serde::Deserialize;
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
            .ok_or_else(|| CoreError::validation("missing", "Test credential missing", "probe"))
    }
    fn write(&self, reference: &str, value: &str) -> Result<()> {
        self.0.borrow_mut().insert(reference.into(), value.into());
        Ok(())
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Input {
    origin: String,
    cookie: String,
    account_id: String,
    browser_device_id: String,
    browser_signing_public: String,
    browser_recipient: String,
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

#[tokio::main]
async fn main() -> std::result::Result<(), Box<dyn std::error::Error>> {
    let mut raw = Zeroizing::new(String::new());
    std::io::stdin().read_to_string(&mut raw)?;
    let input: Input = serde_json::from_str(&raw)?;
    let origin = input.origin.clone();

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
        &[
            ("Origin", origin.as_str()),
            ("Cookie", input.cookie.as_str()),
        ],
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
    if session_account != input.account_id {
        return Err("input accountId does not match the authenticated session".into());
    }

    let proof_bytes = serde_json::to_vec(&(
        "noura.device.enroll.v2",
        origin.as_str(),
        input.account_id.as_str(),
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
        &[
            ("Origin", origin.as_str()),
            ("Cookie", input.cookie.as_str()),
        ],
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
    let directory = tempfile::TempDir::new()?;
    let engine = WorkspaceEngine::create_with_app_data(
        directory.path().join("workspace"),
        "Desktop to browser test",
        directory.path().join("app"),
    )?;
    let workspace = engine.manifest().id;
    eprintln!("desktop_probe: creating workspace {workspace}");
    transport.create_workspace(&workspace).await?;

    std::fs::create_dir_all(engine.root().join("notes"))?;
    std::fs::write(
        engine.root().join("notes/native-note.md"),
        b"# Native note\n\nWritten by the desktop probe and encrypted for the browser.\n",
    )?;
    let key = ObjectKey::generate();
    let operation = engine
        .sync_capture_file(
            "notes/native-note.md",
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

    let browser_public = decode_public(&input.browser_signing_public)?;
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
        if id == input.browser_device_id {
            if row_recipient != input.browser_recipient {
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
            account_id: input.account_id.clone(),
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

    let output = serde_json::json!({
        "workspace": workspace,
        "objectId": object_id,
        "epoch": 1,
        "deviceId": device.device_id(),
        "token": token,
        "file": "notes/native-note.md",
        "signingPublic": device.signer().public_key(),
        "recipient": device.recipient(),
    });
    println!("{}", serde_json::to_string(&output)?);
    std::io::stdout().flush()?;
    Ok(())
}
