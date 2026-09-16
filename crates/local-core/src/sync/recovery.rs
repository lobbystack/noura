//! User-held recovery kits. Secret bytes stay in native memory or the explicitly chosen backup file.
use std::{
    collections::BTreeMap,
    fs::{File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, Payload},
};
use ed25519_dalek::{Signature, VerifyingKey};
use hmac::Hmac;
use pbkdf2::pbkdf2;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use zeroize::{Zeroize, Zeroizing};

use super::*;
use crate::{CoreError, Result, WORKSPACE_MANIFEST_PATH, WorkspaceEngine};

const MAX_KIT: u64 = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RecoveryKit {
    version: u8,
    config: WorkspaceSyncConfig,
    identity: String,
    envelopes: Vec<KeyEnvelope>,
    signature: String,
}
impl Drop for RecoveryKit {
    fn drop(&mut self) {
        self.identity.zeroize();
    }
}
impl RecoveryKit {
    fn signing_bytes(&self) -> Result<Zeroizing<Vec<u8>>> {
        serde_json::to_vec(&(
            "noura.sync.recovery",
            self.version,
            &self.config,
            &self.identity,
            &self.envelopes,
        ))
        .map(Zeroizing::new)
        .map_err(|_| invalid("sync_invalid_recovery_file"))
    }
    fn reader(&self) -> Result<DeviceKeys> {
        self.config.validate(&self.config.workspace_id)?;
        if self.version != 1 || self.envelopes.len() > 1000 {
            return Err(invalid("sync_invalid_recovery_file"));
        }
        let public_key = self
            .config
            .trusted_devices
            .get(&self.config.device_id)
            .ok_or_else(|| invalid("sync_invalid_recovery_file"))?;
        let bytes: [u8; 32] = crypto::decode(public_key, 32, 32)?
            .try_into()
            .map_err(|_| invalid("sync_invalid_key"))?;
        VerifyingKey::from_bytes(&bytes)
            .map_err(|_| invalid("sync_invalid_key"))?
            .verify_strict(
                &self.signing_bytes()?,
                &Signature::from_slice(&crypto::decode(&self.signature, 64, 64)?)
                    .map_err(|_| invalid("sync_invalid_signature"))?,
            )
            .map_err(|_| invalid("sync_invalid_recovery_file"))?;
        let reader = DeviceKeys::recovery_reader(&self.config.device_id, &self.identity)?;
        if self.config.approved_recipients.get(&self.config.device_id) != Some(&reader.recipient())
        {
            return Err(invalid("sync_invalid_recovery_file"));
        }
        let mut seen = std::collections::BTreeSet::new();
        for envelope in &self.envelopes {
            if envelope.workspace_id != self.config.workspace_id
                || envelope.device_id != self.config.device_id
                || !seen.insert((&envelope.object_id, envelope.epoch))
            {
                return Err(invalid("sync_invalid_recovery_file"));
            }
            let trusted = self
                .config
                .trusted_devices
                .get(&envelope.signing_device)
                .ok_or_else(|| invalid("sync_untrusted_device"))?;
            reader.unwrap_key(envelope, trusted)?;
        }
        Ok(reader)
    }
    fn load(path: &Path) -> Result<Self> {
        let meta =
            std::fs::symlink_metadata(path).map_err(|_| invalid("sync_recovery_read_failed"))?;
        if !meta.is_file() || meta.len() > MAX_KIT {
            return Err(invalid("sync_invalid_recovery_file"));
        }
        let mut bytes = Zeroizing::new(Vec::new());
        File::open(path)
            .and_then(|file| file.take(MAX_KIT + 1).read_to_end(&mut bytes))
            .map_err(|_| invalid("sync_recovery_read_failed"))?;
        if bytes.len() as u64 > MAX_KIT {
            return Err(invalid("sync_invalid_recovery_file"));
        }
        serde_json::from_slice(&bytes).map_err(|_| invalid("sync_invalid_recovery_file"))
    }
}

impl WorkspaceSyncCoordinator {
    /// Export the current workspace's trusted identities and recipient-encrypted keys with its recovery identity.
    pub fn export_recovery_kit(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        destination: &Path,
    ) -> Result<()> {
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let device = DeviceKeys::load(store, &connection.device_id)?;
        Self::check_connection(&config, connection, &device)?;
        let secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        let envelopes = secrets
            .objects
            .keys()
            .map(|(object, epoch)| {
                engine
                    .sync_key_envelope(object, *epoch, device.device_id())?
                    .ok_or_else(|| invalid("sync_key_required"))
            })
            .collect::<Result<Vec<_>>>()?;
        let mut kit = RecoveryKit {
            version: 1,
            config,
            identity: device.recovery_secret()?.to_string(),
            envelopes,
            signature: String::new(),
        };
        kit.signature = device.signer().sign_bytes(&kit.signing_bytes()?);
        kit.reader()?;
        let bytes = Zeroizing::new(
            serde_json::to_vec(&kit).map_err(|_| invalid("sync_invalid_recovery_file"))?,
        );
        if bytes.len() as u64 > MAX_KIT {
            return Err(invalid("sync_recovery_too_large"));
        }
        write_backup(destination, engine.root(), &bytes)
    }

    /// Import into the selected workspace under a newly signed-in device. No old signing seed or session is restored.
    /// The workspace remains paused for the user to verify the new device and handle any required epoch rotation.
    pub async fn import_recovery_kit(
        engine: &WorkspaceEngine,
        connection: &DeviceConnection,
        store: &impl SyncCredentials,
        source: &Path,
    ) -> Result<()> {
        let kit = RecoveryKit::load(source)?;
        let reader = kit.reader()?;
        if kit.config.workspace_id != engine.manifest().id
            || kit.config.origin != connection.origin
            || kit.config.approved_accounts.get(&kit.config.device_id)
                != Some(&connection.account_id)
        {
            return Err(invalid("sync_recovery_context_mismatch"));
        }
        let device = DeviceKeys::load(store, &connection.device_id)?;
        let config = engine
            .sync_configuration()?
            .ok_or_else(|| invalid("sync_not_enabled"))?;
        let config = prepare_recovery_configuration(config, &kit.config, connection, &device)?;
        engine.sync_save_configuration(&config)?;
        let mut recovered = engine.sync_restore_secrets(&reader, &config.trusted_devices)?;
        for envelope in &kit.envelopes {
            let signer = config
                .trusted_devices
                .get(&envelope.signing_device)
                .ok_or_else(|| invalid("sync_untrusted_device"))?;
            let key = reader.unwrap_key(envelope, signer)?;
            if let Some(existing) = recovered
                .objects
                .get(&(envelope.object_id.clone(), envelope.epoch))
            {
                if existing.secret() != key.secret() {
                    return Err(invalid("sync_object_key_changed"));
                }
            } else {
                engine.sync_store_key(envelope, &reader, signer)?;
            }
            recovered
                .objects
                .insert((envelope.object_id.clone(), envelope.epoch), key);
        }
        let transport = connection.transport(store)?;
        // Same-account recovery can retrieve later envelopes addressed to the exported identity.
        // Signer pins still come from the kit/current local approval, never from the server response.
        transport
            .receive_keys(engine, &reader, &mut recovered)
            .await?;
        let own = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        for ((object, epoch), key) in &recovered.objects {
            if let Some(existing) = own.objects.get(&(object.clone(), *epoch)) {
                if existing.secret() != key.secret() {
                    return Err(invalid("sync_object_key_changed"));
                }
                continue;
            }
            let envelope = device.wrap_key(
                &config.workspace_id,
                object,
                *epoch,
                device.device_id(),
                &device.recipient(),
                key,
            )?;
            engine.sync_store_key(&envelope, &device, &device.signer().public_key())?;
        }
        let mut secrets = engine.sync_restore_secrets(&device, &config.trusted_devices)?;
        let mut access = transport.access_state(&config.workspace_id).await?;
        if access.revision() != engine.sync_access_revision()? {
            transport.refresh_access_policies(engine, &config).await?;
            access = transport.access_state(&config.workspace_id).await?;
        }
        access.authorize_writers(engine, &device, &config, &mut secrets)?;
        transport
            .receive_checkpoints(engine, &device, &secrets)
            .await?;
        loop {
            let result = transport
                .synchronize_readonly(engine, &secrets, false)
                .await?;
            if !result.has_more {
                break;
            }
        }
        Ok(())
    }
}

fn prepare_recovery_configuration(
    mut current: WorkspaceSyncConfig,
    backup: &WorkspaceSyncConfig,
    connection: &DeviceConnection,
    device: &DeviceKeys,
) -> Result<WorkspaceSyncConfig> {
    current.validate(&backup.workspace_id)?;
    if current.workspace_id != backup.workspace_id
        || current.origin != backup.origin
        || current.origin != connection.origin
        || backup.approved_accounts.get(&backup.device_id) != Some(&connection.account_id)
    {
        return Err(invalid("sync_recovery_context_mismatch"));
    }

    if current.device_id == connection.device_id {
        WorkspaceSyncCoordinator::check_connection(&current, connection, device)?;
    } else if current.device_id != backup.device_id {
        return Err(invalid("sync_recovery_context_mismatch"));
    }

    merge_pins(&mut current.trusted_devices, &backup.trusted_devices)?;
    merge_pins(
        &mut current.approved_recipients,
        &backup.approved_recipients,
    )?;
    merge_pins(&mut current.approved_accounts, &backup.approved_accounts)?;
    merge_pins(
        &mut current.trusted_devices,
        &BTreeMap::from([(device.device_id().to_owned(), device.signer().public_key())]),
    )?;
    merge_pins(
        &mut current.approved_recipients,
        &BTreeMap::from([(device.device_id().to_owned(), device.recipient())]),
    )?;
    merge_pins(
        &mut current.approved_accounts,
        &BTreeMap::from([(device.device_id().to_owned(), connection.account_id.clone())]),
    )?;
    current.device_id = connection.device_id.clone();
    current.enabled = false;
    current.validate(&backup.workspace_id)?;
    Ok(current)
}

fn merge_pins(
    target: &mut BTreeMap<String, String>,
    backup: &BTreeMap<String, String>,
) -> Result<()> {
    for (id, value) in backup {
        if target.get(id).is_some_and(|existing| existing != value) {
            return Err(invalid("sync_device_changed"));
        }
        target.insert(id.clone(), value.clone());
    }
    Ok(())
}
fn write_backup(destination: &Path, workspace: &Path, bytes: &[u8]) -> Result<()> {
    let parent = destination
        .parent()
        .ok_or_else(|| invalid("sync_invalid_recovery_path"))?
        .canonicalize()
        .map_err(|_| invalid("sync_invalid_recovery_path"))?;
    let root = workspace
        .canonicalize()
        .map_err(|_| invalid("sync_invalid_recovery_path"))?;
    if parent.starts_with(&root)
        || parent
            .ancestors()
            .any(|path| path.join(WORKSPACE_MANIFEST_PATH).exists())
    {
        return Err(invalid("sync_recovery_in_workspace"));
    }
    let destination = parent.join(
        destination
            .file_name()
            .ok_or_else(|| invalid("sync_invalid_recovery_path"))?,
    );
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&destination)
        .map_err(|_| invalid("sync_recovery_write_failed"))?;
    file.write_all(bytes)
        .and_then(|_| file.sync_all())
        .map_err(|_| invalid("sync_recovery_write_failed"))?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|file| file.sync_all())
        .map_err(|_| invalid("sync_recovery_write_failed"))?;
    Ok(())
}

/// Exact browser recovery-kit format discriminator, matching the browser client.
pub const BROWSER_RECOVERY_FORMAT: &str = "noura.browser-recovery-kit";

/// Supported browser recovery-kit format version.
pub const BROWSER_RECOVERY_VERSION: u8 = 1;

/// Supported browser recovery-kit key-derivation function identifier.
pub const BROWSER_RECOVERY_KDF: &str = "pbkdf2-sha256";

/// Minimum PBKDF2-HMAC-SHA256 iteration count accepted from a browser kit.
pub const BROWSER_RECOVERY_MIN_ITERATIONS: u32 = 310_000;

/// Maximum decoded browser recovery-kit ciphertext bytes accepted on import.
pub const BROWSER_RECOVERY_MAX_CIPHERTEXT: usize = 4 * 1024 * 1024;

/// Maximum number of object bindings accepted from a browser recovery kit.
const MAX_BROWSER_BINDING_OBJECTS: usize = 100_000;

/// Domain string the browser client binds as AES-GCM additional authenticated data.
const BROWSER_RECOVERY_AAD_DOMAIN: &str = "noura.browser-recovery-kit.aad.v1";

/// Domain string binding the browser at-rest wrapped key bundle.
const BROWSER_BUNDLE_DOMAIN: &str = "noura.browser-sync.bundle";

/// Browser at-rest wrapped key bundle version.
const BROWSER_BUNDLE_VERSION: u8 = 1;

/// Browser at-rest wrapped key bundle key-derivation function identifier.
const BROWSER_BUNDLE_KDF: &str = "pbkdf2-sha256";

/// Length in bytes of a browser PBKDF2 salt.
const BROWSER_SALT_LENGTH: usize = 32;

/// Length in bytes of a browser AES-256-GCM nonce.
const BROWSER_NONCE_LENGTH: usize = 12;

/// Length in bytes of a browser Ed25519 seed, X25519 secret, or public key.
const BROWSER_SECRET_LENGTH: usize = 32;

/// Smallest decoded ciphertext that still contains a 16-byte AES-GCM tag.
const BROWSER_MIN_CIPHERTEXT: usize = 16;

/// Maximum base64 characters accepted for the kit ciphertext before decoding.
const BROWSER_RECOVERY_MAX_CIPHERTEXT_CHARS: usize =
    BROWSER_RECOVERY_MAX_CIPHERTEXT.div_ceil(3) * 4 + 8;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BrowserRecoveryKit {
    format: String,
    version: u8,
    created_at: String,
    kdf: String,
    iterations: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserRecoveryPayload {
    bundle: BrowserWrappedBundle,
    binding: BrowserBinding,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserWrappedBundle {
    version: u8,
    id: String,
    device_id: String,
    kdf: String,
    iterations: u32,
    salt: String,
    nonce: String,
    ciphertext: String,
    signing_public: String,
    recipient_public: String,
}

/// Decrypted browser bundle payload. Zeroized on drop; native import never persists it.
#[derive(Deserialize, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserBundlePayload {
    signing_seed: String,
    x25519_secret: String,
    token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserBinding {
    version: u8,
    local_workspace_id: String,
    workspace_id: String,
    revision: String,
    object_id: String,
    objects: BTreeMap<String, BrowserBoundObject>,
    pinned_signers: BTreeMap<String, String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserBoundObject {
    path: String,
    #[serde(default)]
    local_object_id: Option<String>,
    epoch: u64,
    policy_revision: String,
    key: BrowserBoundKey,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BrowserBoundKey {
    device_id: String,
    wrapped_key: String,
    signature: String,
    construction: String,
    recipient_public_key: String,
    ephemeral_public_key: String,
    salt: String,
    nonce: String,
}

/// One browser-bound object recovered from a browser recovery kit.
pub struct BrowserRecoveredObject {
    /// Stable identifier of the remote sync object.
    pub object_id: String,
    /// Positive safe-integer key epoch.
    pub epoch: u64,
    /// Canonical-path anchor recorded at bind time.
    pub path: String,
    /// Stable local object ID, when the binding recorded one.
    pub local_object_id: Option<String>,
    /// Access-policy revision the object was bound at.
    pub policy_revision: String,
}

/// Result of importing a browser recovery kit on a native client.
///
/// The recovered keys and binding metadata are enough to seed native sync
/// secrets. The browser device identity itself is **not** adopted: native import
/// only recovers the object keys the device was entitled to, using the bundle's
/// X25519 secret as a reader. Only [`Self::keys`] holds plaintext key material.
pub struct BrowserRecoveryImport {
    /// Device identifier the browser kit was bound to.
    pub device_id: String,
    /// Stable id of the local browser workspace the binding belonged to.
    pub local_workspace_id: String,
    /// Remote workspace identifier.
    pub workspace_id: String,
    /// Access-policy revision persisted in the binding.
    pub revision: String,
    /// Primary sync object id from the binding.
    pub primary_object_id: String,
    /// Pinned signer public keys by device id, base64.
    pub pinned_signers: BTreeMap<String, String>,
    /// Per-object binding metadata, without plaintext keys.
    pub objects: Vec<BrowserRecoveredObject>,
    /// Recovered plaintext object keys by object id. Never persist these directly.
    pub keys: BTreeMap<String, ObjectKey>,
}

fn browser_invalid() -> CoreError {
    invalid("sync_invalid_recovery_file")
}

fn browser_passphrase_rejected() -> CoreError {
    invalid("sync_recovery_passphrase_rejected")
}

/// The browser kit's AES-GCM additional authenticated data.
fn browser_kit_aad() -> Result<Vec<u8>> {
    serde_json::to_vec(&(
        BROWSER_RECOVERY_AAD_DOMAIN,
        BROWSER_RECOVERY_FORMAT,
        BROWSER_RECOVERY_VERSION,
    ))
    .map_err(|_| browser_invalid())
}

/// The browser at-rest bundle's AES-GCM additional authenticated data.
fn browser_bundle_aad(bundle: &BrowserWrappedBundle) -> Result<Vec<u8>> {
    serde_json::to_vec(&(
        BROWSER_BUNDLE_DOMAIN,
        bundle.version,
        &bundle.id,
        &bundle.device_id,
        &bundle.kdf,
        bundle.iterations,
        &bundle.salt,
        &bundle.nonce,
        &bundle.signing_public,
        &bundle.recipient_public,
    ))
    .map_err(|_| browser_invalid())
}

/// Derive an AES-256-GCM key-encryption key with PBKDF2-HMAC-SHA256.
fn browser_kek(passphrase: &str, salt: &[u8], iterations: u32) -> Result<Zeroizing<[u8; 32]>> {
    let mut kek = Zeroizing::new([0_u8; 32]);
    pbkdf2::<Hmac<Sha256>>(passphrase.as_bytes(), salt, iterations, &mut *kek)
        .map_err(|_| browser_invalid())?;
    Ok(kek)
}

/// Import a browser recovery kit with both the recovery and device passphrases.
///
/// A browser recovery kit needs **two** secrets: the recovery passphrase that
/// decrypts the kit itself, and the browser device passphrase that decrypts the
/// inner `WrappedKeyBundle`. This recovers the object keys the browser device
/// held; it does not adopt the browser device identity, its signing seed, or its
/// bearer token. All secret material is held in native memory and zeroized.
pub fn import_browser_recovery_kit(
    kit_json: &str,
    recovery_passphrase: &str,
    device_passphrase: &str,
) -> Result<BrowserRecoveryImport> {
    if recovery_passphrase.is_empty() || device_passphrase.is_empty() {
        return Err(browser_invalid());
    }
    let kit: BrowserRecoveryKit = serde_json::from_str(kit_json).map_err(|_| browser_invalid())?;
    import_browser_recovery(&kit, recovery_passphrase, device_passphrase)
}

fn import_browser_recovery(
    kit: &BrowserRecoveryKit,
    recovery_passphrase: &str,
    device_passphrase: &str,
) -> Result<BrowserRecoveryImport> {
    if kit.format != BROWSER_RECOVERY_FORMAT
        || kit.version != BROWSER_RECOVERY_VERSION
        || kit.kdf != BROWSER_RECOVERY_KDF
        || kit.created_at.is_empty()
        || kit.iterations < BROWSER_RECOVERY_MIN_ITERATIONS
        || kit.ciphertext.len() > BROWSER_RECOVERY_MAX_CIPHERTEXT_CHARS
    {
        return Err(browser_invalid());
    }

    let salt = decode(&kit.salt, BROWSER_SALT_LENGTH, BROWSER_SALT_LENGTH)?;
    let nonce: [u8; BROWSER_NONCE_LENGTH] =
        decode(&kit.nonce, BROWSER_NONCE_LENGTH, BROWSER_NONCE_LENGTH)?
            .try_into()
            .map_err(|_| browser_invalid())?;
    let ciphertext = decode(
        &kit.ciphertext,
        BROWSER_MIN_CIPHERTEXT,
        BROWSER_RECOVERY_MAX_CIPHERTEXT,
    )?;

    let aad = browser_kit_aad()?;
    let kek = browser_kek(recovery_passphrase, &salt, kit.iterations)?;
    let plaintext = Zeroizing::new(
        Aes256Gcm::new((&*kek).into())
            .decrypt(
                (&nonce).into(),
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| browser_passphrase_rejected())?,
    );

    let payload: BrowserRecoveryPayload =
        serde_json::from_slice(&plaintext[..]).map_err(|_| browser_invalid())?;
    validate_browser_bundle(&payload.bundle)?;
    let x25519_secret = open_browser_bundle(&payload.bundle, device_passphrase)?;
    build_browser_recovery(&payload.binding, &payload.bundle, &x25519_secret)
}

/// Validate the browser wrapped-bundle metadata and encodings before decrypting.
fn validate_browser_bundle(bundle: &BrowserWrappedBundle) -> Result<()> {
    if bundle.version != BROWSER_BUNDLE_VERSION
        || bundle.kdf != BROWSER_BUNDLE_KDF
        || bundle.iterations < BROWSER_RECOVERY_MIN_ITERATIONS
    {
        return Err(browser_invalid());
    }
    identifier(&bundle.id)?;
    identifier(&bundle.device_id)?;
    decode(&bundle.salt, BROWSER_SALT_LENGTH, BROWSER_SALT_LENGTH)?;
    decode(&bundle.nonce, BROWSER_NONCE_LENGTH, BROWSER_NONCE_LENGTH)?;
    decode(
        &bundle.signing_public,
        BROWSER_SECRET_LENGTH,
        BROWSER_SECRET_LENGTH,
    )?;
    decode(
        &bundle.recipient_public,
        BROWSER_SECRET_LENGTH,
        BROWSER_SECRET_LENGTH,
    )?;
    decode(
        &bundle.ciphertext,
        BROWSER_MIN_CIPHERTEXT,
        BROWSER_RECOVERY_MAX_CIPHERTEXT,
    )?;
    Ok(())
}

/// Open the inner browser bundle and return only its X25519 reader secret.
fn open_browser_bundle(
    bundle: &BrowserWrappedBundle,
    device_passphrase: &str,
) -> Result<Zeroizing<[u8; 32]>> {
    let salt = decode(&bundle.salt, BROWSER_SALT_LENGTH, BROWSER_SALT_LENGTH)?;
    let nonce: [u8; BROWSER_NONCE_LENGTH] =
        decode(&bundle.nonce, BROWSER_NONCE_LENGTH, BROWSER_NONCE_LENGTH)?
            .try_into()
            .map_err(|_| browser_invalid())?;
    let ciphertext = decode(
        &bundle.ciphertext,
        BROWSER_MIN_CIPHERTEXT,
        BROWSER_RECOVERY_MAX_CIPHERTEXT,
    )?;
    let aad = browser_bundle_aad(bundle)?;
    let kek = browser_kek(device_passphrase, &salt, bundle.iterations)?;
    let plaintext = Zeroizing::new(
        Aes256Gcm::new((&*kek).into())
            .decrypt(
                (&nonce).into(),
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map_err(|_| browser_passphrase_rejected())?,
    );
    let payload: BrowserBundlePayload =
        serde_json::from_slice(&plaintext[..]).map_err(|_| browser_invalid())?;
    let signing_seed = decode(
        &payload.signing_seed,
        BROWSER_SECRET_LENGTH,
        BROWSER_SECRET_LENGTH,
    )?;
    if signing_seed.as_slice() == [0_u8; BROWSER_SECRET_LENGTH] {
        return Err(browser_invalid());
    }
    let secret: [u8; BROWSER_SECRET_LENGTH] = decode(
        &payload.x25519_secret,
        BROWSER_SECRET_LENGTH,
        BROWSER_SECRET_LENGTH,
    )?
    .try_into()
    .map_err(|_| browser_invalid())?;
    Ok(Zeroizing::new(secret))
}

/// Unwrap every self-wrapped binding key and collect the recovered metadata.
fn build_browser_recovery(
    binding: &BrowserBinding,
    bundle: &BrowserWrappedBundle,
    x25519_secret: &Zeroizing<[u8; 32]>,
) -> Result<BrowserRecoveryImport> {
    if binding.version != BROWSER_RECOVERY_VERSION
        || binding.objects.is_empty()
        || binding.objects.len() > MAX_BROWSER_BINDING_OBJECTS
        || !binding.objects.contains_key(&binding.object_id)
    {
        return Err(browser_invalid());
    }
    identifier(&binding.workspace_id)?;
    identifier(&binding.local_workspace_id)?;
    let recipient = decode(
        &bundle.recipient_public,
        BROWSER_SECRET_LENGTH,
        BROWSER_SECRET_LENGTH,
    )?;

    let mut keys = BTreeMap::new();
    let mut objects = Vec::with_capacity(binding.objects.len());
    for (object_id, object) in &binding.objects {
        identifier(object_id)?;
        if object.key.construction != "web"
            || object.key.device_id != bundle.device_id
            || object.epoch == 0
            || object.epoch > sync_key_envelope::MAX_EPOCH
        {
            return Err(browser_invalid());
        }
        if decode(
            &object.key.recipient_public_key,
            BROWSER_SECRET_LENGTH,
            BROWSER_SECRET_LENGTH,
        )? != recipient
        {
            return Err(browser_invalid());
        }
        let signer = binding
            .pinned_signers
            .get(&object.key.device_id)
            .ok_or_else(|| invalid("sync_untrusted_device"))?;
        let signer: [u8; BROWSER_SECRET_LENGTH] =
            decode(signer, BROWSER_SECRET_LENGTH, BROWSER_SECRET_LENGTH)?
                .try_into()
                .map_err(|_| browser_invalid())?;
        let envelope = sync_key_envelope::WebKeyEnvelope {
            workspace_id: binding.workspace_id.clone(),
            object_id: object_id.clone(),
            epoch: object.epoch,
            signing_device: object.key.device_id.clone(),
            device_id: object.key.device_id.clone(),
            recipient_public_key: object.key.recipient_public_key.clone(),
            ephemeral_public_key: object.key.ephemeral_public_key.clone(),
            salt: object.key.salt.clone(),
            nonce: object.key.nonce.clone(),
            wrapped_key: object.key.wrapped_key.clone(),
            signature: object.key.signature.clone(),
        };
        let key = sync_key_envelope::unwrap_key(&envelope, **x25519_secret, signer)
            .map_err(|error| invalid(error.code()))?;
        keys.insert(object_id.clone(), ObjectKey::from_bytes(*key));
        objects.push(BrowserRecoveredObject {
            object_id: object_id.clone(),
            epoch: object.epoch,
            path: object.path.clone(),
            local_object_id: object.local_object_id.clone(),
            policy_revision: object.policy_revision.clone(),
        });
    }

    Ok(BrowserRecoveryImport {
        device_id: bundle.device_id.clone(),
        local_workspace_id: binding.local_workspace_id.clone(),
        workspace_id: binding.workspace_id.clone(),
        revision: binding.revision.clone(),
        primary_object_id: binding.object_id.clone(),
        pinned_signers: binding.pinned_signers.clone(),
        objects,
        keys,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, String>>);
    impl SyncCredentials for Memory {
        fn read(&self, name: &str) -> Result<Zeroizing<String>> {
            self.0
                .borrow()
                .get(name)
                .cloned()
                .map(Zeroizing::new)
                .ok_or_else(|| invalid("test_missing"))
        }
        fn write(&self, name: &str, value: &str) -> Result<()> {
            self.0.borrow_mut().insert(name.into(), value.into());
            Ok(())
        }
    }
    #[test]
    fn recovery_kit_binds_context_and_keys_and_never_overwrites_or_enters_a_workspace() {
        let directory = tempfile::TempDir::new().unwrap();
        let engine = WorkspaceEngine::create_with_app_data(
            directory.path().join("workspace"),
            "Recovery",
            directory.path().join("app"),
        )
        .unwrap();
        let credentials = Memory::default();
        let device = DeviceKeys::create(&credentials).unwrap();
        let connection = DeviceConnection {
            origin: "http://127.0.0.1:1900".into(),
            device_id: device.device_id().into(),
            account_id: "account".into(),
            token_reference: "test".into(),
        };
        let config = WorkspaceSyncConfig {
            version: 1,
            workspace_id: engine.manifest().id,
            origin: connection.origin.clone(),
            device_id: device.device_id().into(),
            enabled: false,
            trusted_devices: BTreeMap::from([(
                device.device_id().into(),
                device.signer().public_key(),
            )]),
            approved_recipients: BTreeMap::from([(device.device_id().into(), device.recipient())]),
            approved_accounts: BTreeMap::from([(device.device_id().into(), "account".into())]),
        };
        engine.sync_save_configuration(&config).unwrap();
        std::fs::write(engine.root().join("file.txt"), b"private recovery bytes").unwrap();
        engine
            .sync_capture_owned_file("file.txt", &device, 1)
            .unwrap();
        let path = directory.path().join("recovery.json");
        assert!(
            WorkspaceSyncCoordinator::export_recovery_kit(
                &engine,
                &connection,
                &credentials,
                &engine.root().join("leak.json")
            )
            .is_err()
        );
        WorkspaceSyncCoordinator::export_recovery_kit(&engine, &connection, &credentials, &path)
            .unwrap();
        assert!(
            WorkspaceSyncCoordinator::export_recovery_kit(
                &engine,
                &connection,
                &credentials,
                &path
            )
            .is_err()
        );
        let kit = RecoveryKit::load(&path).unwrap();
        let reader = kit.reader().unwrap();
        assert_eq!(reader.recipient(), device.recipient());
        assert_eq!(kit.envelopes.len(), 1);

        let replacement = DeviceKeys::create(&credentials).unwrap();
        let replacement_connection = DeviceConnection {
            origin: connection.origin.clone(),
            device_id: replacement.device_id().into(),
            account_id: connection.account_id.clone(),
            token_reference: "replacement-token".into(),
        };
        let recovered = prepare_recovery_configuration(
            config.clone(),
            &kit.config,
            &replacement_connection,
            &replacement,
        )
        .unwrap();
        assert_eq!(recovered.device_id, replacement.device_id());
        assert!(!recovered.enabled);
        assert_eq!(
            recovered.trusted_devices.get(replacement.device_id()),
            Some(&replacement.signer().public_key())
        );
        assert_eq!(
            recovered.approved_recipients.get(replacement.device_id()),
            Some(&replacement.recipient())
        );
        assert_eq!(
            recovered.approved_accounts.get(replacement.device_id()),
            Some(&connection.account_id)
        );
        assert!(recovered.trusted_devices.contains_key(device.device_id()));
        let mut wrong_account = replacement_connection.clone();
        wrong_account.account_id = "other-account".into();
        assert!(
            prepare_recovery_configuration(
                config.clone(),
                &kit.config,
                &wrong_account,
                &replacement
            )
            .is_err()
        );
        assert!(
            !std::fs::read_to_string(&path)
                .unwrap()
                .contains("private recovery bytes")
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        for field in ["identity", "signature"] {
            let mut corrupted = serde_json::to_value(&kit).unwrap();
            corrupted[field] = serde_json::json!("changed");
            let corrupted: RecoveryKit = serde_json::from_value(corrupted).unwrap();
            assert!(corrupted.reader().is_err());
        }
        let mut corrupted = serde_json::to_value(&kit).unwrap();
        corrupted["config"]["workspaceId"] = serde_json::json!("other");
        assert!(
            serde_json::from_value::<RecoveryKit>(corrupted)
                .unwrap()
                .reader()
                .is_err()
        );
        #[cfg(unix)]
        {
            let link = directory.path().join("link.json");
            std::os::unix::fs::symlink(&path, &link).unwrap();
            assert!(RecoveryKit::load(&link).is_err());
        }
    }

    const NATIVE_RECOVERY_FIXTURE: &str =
        include_str!("../../../../docs/workspace-format/fixtures/native-recovery-v1.json");

    /// Regenerates the shared fixture. This is test-only and never ships the identity secret in a
    /// production build; the fixture's recovery identity is a public age test vector.
    #[test]
    #[ignore = "regenerates docs/workspace-format/fixtures/native-recovery-v1.json"]
    fn regenerate_native_recovery_fixture() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        use std::collections::BTreeMap;

        let workspace = "workspace_recovery";
        let device_id = "device_recovery";
        let account = "account_recovery";
        let origin = "https://sync.noura.example";
        let identity = "AGE-SECRET-KEY-1GQ9778VQXMMJVE8SK7J6VT8UJ4HDQAJUVSFCWCM02D8GEWQ72PVQ2Y5J33";
        let device = DeviceKeys::from_test_parts(device_id, [0x11_u8; 32], identity).unwrap();
        let config = WorkspaceSyncConfig {
            version: 1,
            workspace_id: workspace.into(),
            origin: origin.into(),
            device_id: device_id.into(),
            enabled: false,
            trusted_devices: BTreeMap::from([(device_id.into(), device.signer().public_key())]),
            approved_recipients: BTreeMap::from([(device_id.into(), device.recipient())]),
            approved_accounts: BTreeMap::from([(device_id.into(), account.into())]),
        };
        let keys = [
            ("object_one", 1_u64, [0x21_u8; 32]),
            ("object_two", 2_u64, [0x42_u8; 32]),
        ];
        let envelopes = keys
            .iter()
            .map(|(object, epoch, secret)| {
                device
                    .wrap_key(
                        workspace,
                        object,
                        *epoch,
                        device.device_id(),
                        &device.recipient(),
                        &ObjectKey::from_bytes(*secret),
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        let mut kit = RecoveryKit {
            version: 1,
            config,
            identity: device.recovery_secret().unwrap().to_string(),
            envelopes,
            signature: String::new(),
        };
        kit.signature = device.signer().sign_bytes(&kit.signing_bytes().unwrap());
        // Prove the regenerated fixture satisfies the native reader before it is written.
        kit.reader().unwrap();

        let object_keys = keys
            .iter()
            .map(|(object, epoch, secret)| {
                serde_json::json!({
                    "object_id": object,
                    "epoch": epoch,
                    "key": STANDARD.encode(secret),
                })
            })
            .collect::<Vec<_>>();
        let document = serde_json::json!({
            "format": "noura.sync.recovery",
            "domain": "noura.sync.recovery",
            "version": 1,
            "recovery": {
                "version": kit.version,
                "config": &kit.config,
                "envelopes": &kit.envelopes,
                "signature": &kit.signature,
            },
            "recovery_identity": &kit.identity,
            "recovery_recipient": device.recipient(),
            "object_keys": object_keys,
        });
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../docs/workspace-format/fixtures/native-recovery-v1.json");
        std::fs::write(
            path,
            format!("{}\n", serde_json::to_string_pretty(&document).unwrap()),
        )
        .unwrap();
    }

    #[test]
    fn native_recovery_fixture_verifies_and_unwraps_its_own_object() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let fixture: serde_json::Value = serde_json::from_str(NATIVE_RECOVERY_FIXTURE).unwrap();
        assert_eq!(fixture["domain"], "noura.sync.recovery");
        assert_eq!(fixture["format"], "noura.sync.recovery");
        assert_eq!(fixture["version"], 1);

        // The public recovery object is signed over the identity secret, so reassemble the full
        // object before handing it to the native reader.
        let mut object = fixture["recovery"].as_object().unwrap().clone();
        object.insert("identity".into(), fixture["recovery_identity"].clone());
        let kit: RecoveryKit = serde_json::from_value(serde_json::Value::Object(object)).unwrap();
        let reader = kit.reader().unwrap();
        assert_eq!(
            reader.recipient(),
            fixture["recovery_recipient"].as_str().unwrap()
        );

        let expected = fixture["object_keys"].as_array().unwrap();
        assert_eq!(expected.len(), kit.envelopes.len());
        for vector in expected {
            let object_id = vector["object_id"].as_str().unwrap();
            let epoch = vector["epoch"].as_u64().unwrap();
            let encoded = vector["key"].as_str().unwrap();
            let envelope = kit
                .envelopes
                .iter()
                .find(|envelope| envelope.object_id == object_id && envelope.epoch == epoch)
                .expect("fixture envelope must exist");
            let signer = kit
                .config
                .trusted_devices
                .get(&envelope.signing_device)
                .unwrap();
            let key = reader.unwrap_key(envelope, signer).unwrap();
            assert_eq!(STANDARD.encode(key.secret()), encoded, "{object_id}");
        }
    }

    const BROWSER_RECOVERY_FIXTURE: &str =
        include_str!("../../../../docs/workspace-format/fixtures/browser-recovery-v1.json");

    fn browser_recovery_fixture() -> serde_json::Value {
        serde_json::from_str(BROWSER_RECOVERY_FIXTURE).unwrap()
    }

    fn browser_recovery_kit(fixture: &serde_json::Value) -> String {
        serde_json::to_string(&fixture["kit"]).unwrap()
    }

    #[test]
    fn browser_recovery_fixture_imports_and_matches_object_keys() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let fixture = browser_recovery_fixture();
        assert!(fixture["test_only"].as_bool().unwrap());
        assert!(fixture["note"].as_str().is_some());
        let kit = browser_recovery_kit(&fixture);
        let result = import_browser_recovery_kit(
            &kit,
            fixture["recovery_passphrase"].as_str().unwrap(),
            fixture["device_passphrase"].as_str().unwrap(),
        )
        .unwrap();

        let expected = &fixture["expected"];
        assert_eq!(result.device_id, expected["device_id"].as_str().unwrap());
        assert_eq!(
            result.workspace_id,
            expected["workspace_id"].as_str().unwrap()
        );
        assert_eq!(
            result.local_workspace_id,
            expected["local_workspace_id"].as_str().unwrap()
        );
        assert_eq!(
            result.primary_object_id,
            expected["primary_object_id"].as_str().unwrap()
        );
        assert_eq!(result.revision, expected["revision"].as_str().unwrap());

        let vectors = expected["object_keys"].as_array().unwrap();
        assert_eq!(result.keys.len(), vectors.len());
        for vector in vectors {
            let object_id = vector["object_id"].as_str().unwrap();
            let epoch = vector["epoch"].as_u64().unwrap();
            let key = result
                .keys
                .get(object_id)
                .unwrap_or_else(|| panic!("missing key for {object_id}"));
            assert_eq!(
                STANDARD.encode(key.secret()),
                vector["key"].as_str().unwrap(),
                "{object_id}"
            );
            let entry = result
                .objects
                .iter()
                .find(|entry| entry.object_id == object_id)
                .expect("recovered metadata must exist");
            assert_eq!(entry.epoch, epoch, "{object_id}");
        }
    }

    #[test]
    fn browser_recovery_rejects_wrong_secrets_and_tampering() {
        use base64::{Engine as _, engine::general_purpose::STANDARD};

        let fixture = browser_recovery_fixture();
        let passphrase = fixture["recovery_passphrase"].as_str().unwrap();
        let device = fixture["device_passphrase"].as_str().unwrap();
        let kit = browser_recovery_kit(&fixture);

        assert_eq!(
            import_browser_recovery_kit(&kit, "wrong recovery passphrase", device)
                .err()
                .expect("wrong recovery passphrase must fail")
                .code,
            "sync_recovery_passphrase_rejected"
        );
        assert_eq!(
            import_browser_recovery_kit(&kit, passphrase, "wrong device passphrase")
                .err()
                .expect("wrong device passphrase must fail")
                .code,
            "sync_recovery_passphrase_rejected"
        );
        assert!(import_browser_recovery_kit(&kit, "", device).is_err());
        assert!(import_browser_recovery_kit(&kit, passphrase, "").is_err());

        let mut tampered = fixture.clone();
        let mut ciphertext = STANDARD
            .decode(tampered["kit"]["ciphertext"].as_str().unwrap())
            .unwrap();
        let last = ciphertext.len() - 1;
        ciphertext[last] ^= 0x01;
        tampered["kit"]["ciphertext"] = serde_json::json!(STANDARD.encode(ciphertext));
        assert_eq!(
            import_browser_recovery_kit(&browser_recovery_kit(&tampered), passphrase, device)
                .err()
                .expect("tampered ciphertext must fail")
                .code,
            "sync_recovery_passphrase_rejected"
        );

        let mut wrong_version = fixture.clone();
        wrong_version["kit"]["version"] = serde_json::json!(2);
        assert_eq!(
            import_browser_recovery_kit(&browser_recovery_kit(&wrong_version), passphrase, device)
                .err()
                .expect("unknown version must fail")
                .code,
            "sync_invalid_recovery_file"
        );

        let mut weak = fixture.clone();
        weak["kit"]["iterations"] = serde_json::json!(BROWSER_RECOVERY_MIN_ITERATIONS - 1);
        assert_eq!(
            import_browser_recovery_kit(&browser_recovery_kit(&weak), passphrase, device)
                .err()
                .expect("below-minimum iterations must fail")
                .code,
            "sync_invalid_recovery_file"
        );

        let mut wrong_kdf = fixture.clone();
        wrong_kdf["kit"]["kdf"] = serde_json::json!("scrypt");
        assert_eq!(
            import_browser_recovery_kit(&browser_recovery_kit(&wrong_kdf), passphrase, device)
                .err()
                .expect("unknown kdf must fail")
                .code,
            "sync_invalid_recovery_file"
        );

        let mut wrong_format = fixture.clone();
        wrong_format["kit"]["format"] = serde_json::json!("noura.other-kit");
        assert_eq!(
            import_browser_recovery_kit(&browser_recovery_kit(&wrong_format), passphrase, device)
                .err()
                .expect("unknown format must fail")
                .code,
            "sync_invalid_recovery_file"
        );
    }
}
