use std::{
    collections::HashMap,
    io::Write,
    path::Path,
    sync::{LazyLock, Mutex},
};

use age::secrecy::ExposeSecret;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{ObjectKey, SigningIdentity, crypto::decode, identifier, invalid};
use crate::{CoreError, ErrorCategory, Result, WORKSPACE_MANIFEST_PATH};

/// Native credential-store boundary, replaceable by an in-memory implementation in tests.
pub trait SyncCredentials {
    fn read(&self, reference: &str) -> Result<Zeroizing<String>>;
    fn write(&self, reference: &str, value: &str) -> Result<()>;
    fn read_optional(&self, reference: &str) -> Result<Option<Zeroizing<String>>> {
        self.read(reference).map(Some)
    }
}

pub struct OsSyncCredentials;

fn sync_keychain_service() -> &'static str {
    option_env!("NOURA_SYNC_KEYCHAIN_SERVICE")
        .filter(|service| !service.trim().is_empty())
        .unwrap_or("org.noura.sync")
}

// Credentials stay native and zeroizing. Serialize the first lookup as well as
// writes, so concurrent editor opens and the sync loop cannot prompt together.
static SESSION_CREDENTIALS: LazyLock<SessionCredentials<KeychainCredentials>> =
    LazyLock::new(|| SessionCredentials::new(KeychainCredentials));

struct SessionCredentials<S> {
    store: S,
    values: Mutex<HashMap<String, Result<Option<Zeroizing<String>>>>>,
}

impl<S> SessionCredentials<S> {
    fn new(store: S) -> Self {
        Self {
            store,
            values: Mutex::new(HashMap::new()),
        }
    }
}

impl<S: SyncCredentials> SyncCredentials for SessionCredentials<S> {
    fn read_optional(&self, reference: &str) -> Result<Option<Zeroizing<String>>> {
        let mut values = self.values.lock().map_err(|_| credential_error())?;
        // Cache absence and access failures too: passive navigation must not
        // repeatedly retry a denied Keychain prompt. A successful explicit write
        // refreshes the entry; otherwise access is retried on the next launch.
        values
            .entry(reference.to_owned())
            .or_insert_with(|| self.store.read_optional(reference))
            .clone()
    }
    fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
        self.read_optional(reference)?.ok_or_else(credential_error)
    }
    fn write(&self, reference: &str, value: &str) -> Result<()> {
        let mut values = self.values.lock().map_err(|_| credential_error())?;
        let result = self.store.write(reference, value);
        // Publish only after Keychain confirms the write. Invalidate any old
        // cached value on failure rather than exposing an unconfirmed secret.
        values.insert(
            reference.to_owned(),
            result
                .clone()
                .map(|()| Some(Zeroizing::new(value.to_owned()))),
        );
        result
    }
}

impl SyncCredentials for OsSyncCredentials {
    fn read_optional(&self, reference: &str) -> Result<Option<Zeroizing<String>>> {
        SESSION_CREDENTIALS.read_optional(reference)
    }
    fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
        SESSION_CREDENTIALS.read(reference)
    }
    fn write(&self, reference: &str, value: &str) -> Result<()> {
        SESSION_CREDENTIALS.write(reference, value)
    }
}

struct KeychainCredentials;

// The legacy macOS keychain backend can display authorization UI even during
// background reads. Serialize this process-wide setting and restore its prior
// value; refusal remains a credential error, never permission to bypass Keychain.
fn without_keychain_prompt<T>(operation: impl FnOnce() -> Result<T>) -> Result<T> {
    #[cfg(target_os = "macos")]
    let _guard = {
        use security_framework::os::macos::keychain::SecKeychain;
        static INTERACTION: Mutex<()> = Mutex::new(());
        let lock = INTERACTION.lock().map_err(|_| credential_error())?;
        let interaction =
            if SecKeychain::user_interaction_allowed().map_err(|_| credential_error())? {
                Some(SecKeychain::disable_user_interaction().map_err(|_| credential_error())?)
            } else {
                None
            };
        (interaction, lock)
    };
    operation()
}

impl SyncCredentials for KeychainCredentials {
    fn read_optional(&self, reference: &str) -> Result<Option<Zeroizing<String>>> {
        without_keychain_prompt(|| {
            match keyring::Entry::new(sync_keychain_service(), reference)
                .and_then(|entry| entry.get_password())
            {
                Ok(value) => Ok(Some(Zeroizing::new(value))),
                Err(keyring::Error::NoEntry) => Ok(None),
                Err(_) => Err(credential_error()),
            }
        })
    }
    fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
        self.read_optional(reference)?.ok_or_else(credential_error)
    }
    fn write(&self, reference: &str, value: &str) -> Result<()> {
        without_keychain_prompt(|| {
            keyring::Entry::new(sync_keychain_service(), reference)
                .and_then(|entry| entry.set_password(value))
                .map_err(|_| credential_error())
        })
    }
}

/// Signing and age recipient identities stay native and have no IPC serialization.
pub struct DeviceKeys {
    device_id: String,
    signer: SigningIdentity,
    recipient: age::x25519::Identity,
}

#[derive(Serialize, Deserialize, zeroize::Zeroize, zeroize::ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct StoredKeys {
    version: u8,
    signing_seed: String,
    recipient_identity: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct KeyEnvelope {
    pub workspace_id: String,
    pub object_id: String,
    pub epoch: u64,
    pub device_id: String,
    pub wrapped_key: String,
    pub signing_device: String,
    pub signature: String,
}

impl DeviceKeys {
    /// Create a fresh identity under a random, non-reused native credential reference.
    pub fn create(store: &impl SyncCredentials) -> Result<Self> {
        let keys = Self {
            device_id: format!("device_{}", uuid::Uuid::new_v4()),
            signer: SigningIdentity::generate(),
            recipient: age::x25519::Identity::generate(),
        };
        let mut record = StoredKeys {
            version: 1,
            signing_seed: STANDARD.encode(keys.signer.seed().as_slice()),
            recipient_identity: keys.recipient.to_string().expose_secret().to_owned(),
        };
        let encoded = Zeroizing::new(
            serde_json::to_string(&record).map_err(|_| invalid("sync_serialize_failed"))?,
        );
        use zeroize::Zeroize;
        record.signing_seed.zeroize();
        record.recipient_identity.zeroize();
        store.write(&keys.device_id, &encoded)?;
        Ok(keys)
    }

    pub fn load(store: &impl SyncCredentials, device_id: &str) -> Result<Self> {
        identifier(device_id)?;
        let encoded = store.read(device_id)?;
        let mut record: StoredKeys =
            serde_json::from_str(&encoded).map_err(|_| credential_error())?;
        if record.version != 1 {
            return Err(credential_error());
        }
        let seed = Zeroizing::new(decode(&record.signing_seed, 32, 32)?);
        let seed: &[u8; 32] = seed.as_slice().try_into().map_err(|_| credential_error())?;
        let signer = SigningIdentity::from_seed(seed);
        let recipient = record
            .recipient_identity
            .parse()
            .map_err(|_| credential_error())?;
        use zeroize::Zeroize;
        record.signing_seed.zeroize();
        record.recipient_identity.zeroize();
        Ok(Self {
            device_id: device_id.into(),
            signer,
            recipient,
        })
    }

    pub fn device_id(&self) -> &str {
        &self.device_id
    }
    pub fn signer(&self) -> &SigningIdentity {
        &self.signer
    }
    pub fn recipient(&self) -> String {
        self.recipient.to_public().to_string()
    }

    /// Encrypt a context-bound object key with age, then authenticate its routing statement.
    pub fn wrap_key(
        &self,
        workspace: &str,
        object: &str,
        epoch: u64,
        recipient_device: &str,
        recipient: &str,
        key: &ObjectKey,
    ) -> Result<KeyEnvelope> {
        for id in [workspace, object, recipient_device] {
            identifier(id)?;
        }
        if epoch == 0 || epoch > 9_007_199_254_740_991 {
            return Err(invalid("sync_invalid_epoch"));
        }
        let recipient: age::x25519::Recipient = recipient
            .parse()
            .map_err(|_| invalid("sync_invalid_recipient"))?;
        let encoded_key = Zeroizing::new(STANDARD.encode(key.secret()));
        let plaintext = Zeroizing::new(
            serde_json::to_vec(&(
                "noura.sync.object-key",
                1,
                workspace,
                object,
                epoch,
                encoded_key.as_str(),
            ))
            .map_err(|_| invalid("sync_serialize_failed"))?,
        );
        let ciphertext =
            age::encrypt(&recipient, &plaintext).map_err(|_| invalid("sync_key_wrap_failed"))?;
        let mut envelope = KeyEnvelope {
            workspace_id: workspace.into(),
            object_id: object.into(),
            epoch,
            device_id: recipient_device.into(),
            wrapped_key: STANDARD.encode(ciphertext),
            signing_device: self.device_id.clone(),
            signature: String::new(),
        };
        envelope.signature = self.signer.sign_bytes(&envelope.signing_bytes()?);
        Ok(envelope)
    }

    /// Only a previously trusted signer is accepted; never trust a key merely because the server returned it.
    pub fn unwrap_key(&self, envelope: &KeyEnvelope, trusted_signer: &str) -> Result<ObjectKey> {
        if envelope.device_id != self.device_id {
            return Err(invalid("sync_wrong_recipient"));
        }
        envelope.verify(trusted_signer)?;
        unwrap(&self.recipient, envelope)
    }

    pub(crate) fn recovery_secret(&self) -> Zeroizing<String> {
        Zeroizing::new(self.recipient.to_string().expose_secret().to_owned())
    }

    /// This ephemeral reader can unwrap old recipient envelopes, but has no old signing seed.
    pub(crate) fn recovery_reader(device_id: &str, secret: &str) -> Result<Self> {
        identifier(device_id)?;
        Ok(Self {
            device_id: device_id.into(),
            signer: SigningIdentity::generate(),
            recipient: secret
                .parse()
                .map_err(|_| invalid("sync_invalid_recovery_file"))?,
        })
    }

    /// Write a user-held recovery identity outside the workspace, without returning it over IPC.
    /// The recovery recipient must also receive encrypted object-key envelopes.
    pub fn export_recovery_identity(
        &self,
        destination: &Path,
        workspace_root: &Path,
    ) -> Result<String> {
        let parent = destination
            .parent()
            .ok_or_else(|| invalid("sync_invalid_recovery_path"))?
            .canonicalize()
            .map_err(|error| CoreError::io(error, "sync_recovery", None))?;
        let root = workspace_root
            .canonicalize()
            .map_err(|error| CoreError::io(error, "sync_recovery", None))?;
        if parent.starts_with(&root)
            || destination.starts_with(&root)
            || parent
                .ancestors()
                .any(|path| path.join(WORKSPACE_MANIFEST_PATH).exists())
        {
            return Err(invalid("sync_recovery_in_workspace"));
        }
        let name = destination
            .file_name()
            .ok_or_else(|| invalid("sync_invalid_recovery_path"))?;
        let destination = parent.join(name);
        let secret = self.recipient.to_string();
        let public = self.recipient.to_public().to_string();
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&destination)
            .map_err(|error| CoreError::io(error, "sync_recovery", None))?;
        file.write_all(secret.expose_secret().as_bytes())
            .and_then(|()| file.sync_all())
            .map_err(|error| CoreError::io(error, "sync_recovery", None))?;
        #[cfg(unix)]
        std::fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|error| CoreError::io(error, "sync_recovery", None))?;
        Ok(public)
    }

    /// Recover an object key using the user-selected identity file and a trusted signed envelope.
    pub fn recover_key(
        identity_file: &Path,
        envelope: &KeyEnvelope,
        trusted_signer: &str,
    ) -> Result<ObjectKey> {
        envelope.verify(trusted_signer)?;
        let metadata = std::fs::symlink_metadata(identity_file)
            .map_err(|error| CoreError::io(error, "sync_recovery", None))?;
        if !metadata.is_file() || metadata.len() > 1024 {
            return Err(invalid("sync_invalid_recovery_file"));
        }
        let secret = Zeroizing::new(
            std::fs::read_to_string(identity_file)
                .map_err(|error| CoreError::io(error, "sync_recovery", None))?,
        );
        let identity = secret
            .trim()
            .parse()
            .map_err(|_| invalid("sync_invalid_recovery_file"))?;
        unwrap(&identity, envelope)
    }
}

impl KeyEnvelope {
    /// Preserve recipient ciphertext when an approved administrator signs the next access policy.
    pub(crate) fn resign(&self, device: &DeviceKeys, trusted_signer: &str) -> Result<Self> {
        self.verify(trusted_signer)?;
        let mut result = self.clone();
        result.signing_device = device.device_id().into();
        result.signature = device.signer().sign_bytes(&result.signing_bytes()?);
        Ok(result)
    }
    fn signing_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&(
            "noura.sync.key",
            1,
            &self.workspace_id,
            &self.object_id,
            self.epoch,
            &self.signing_device,
            &self.device_id,
            &self.wrapped_key,
        ))
        .map_err(|_| invalid("sync_serialize_failed"))
    }

    pub fn verify(&self, trusted_signer: &str) -> Result<()> {
        for id in [
            &self.workspace_id,
            &self.object_id,
            &self.device_id,
            &self.signing_device,
        ] {
            identifier(id)?;
        }
        if self.epoch == 0 || self.epoch > 9_007_199_254_740_991 {
            return Err(invalid("sync_invalid_epoch"));
        }
        decode(&self.wrapped_key, 60, 4096)?;
        let public: [u8; 32] = decode(trusted_signer, 32, 32)?
            .try_into()
            .map_err(|_| invalid("sync_invalid_key"))?;
        let signature = Signature::from_slice(&decode(&self.signature, 64, 64)?)
            .map_err(|_| invalid("sync_invalid_signature"))?;
        VerifyingKey::from_bytes(&public)
            .map_err(|_| invalid("sync_invalid_key"))?
            .verify_strict(&self.signing_bytes()?, &signature)
            .map_err(|_| invalid("sync_invalid_signature"))
    }
}

fn unwrap(identity: &age::x25519::Identity, envelope: &KeyEnvelope) -> Result<ObjectKey> {
    let plaintext = Zeroizing::new(
        age::decrypt(identity, &decode(&envelope.wrapped_key, 60, 4096)?)
            .map_err(|_| invalid("sync_key_unwrap_failed"))?,
    );
    let (domain, version, workspace, object, epoch, encoded): (&str, u8, &str, &str, u64, &str) =
        serde_json::from_slice(&plaintext).map_err(|_| invalid("sync_invalid_wrapped_key"))?;
    if domain != "noura.sync.object-key"
        || version != 1
        || workspace != envelope.workspace_id
        || object != envelope.object_id
        || epoch != envelope.epoch
    {
        return Err(invalid("sync_invalid_wrapped_key"));
    }
    let bytes = Zeroizing::new(decode(encoded, 32, 32)?);
    Ok(ObjectKey::from_bytes(
        bytes
            .as_slice()
            .try_into()
            .map_err(|_| invalid("sync_invalid_key"))?,
    ))
}

fn credential_error() -> CoreError {
    CoreError::new(
        "sync_credentials_unavailable",
        ErrorCategory::Credential,
        "Sync credentials are unavailable in the operating system credential store",
        "sync",
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::HashMap};

    #[derive(Default)]
    struct Memory(RefCell<HashMap<String, Zeroizing<String>>>);
    impl SyncCredentials for Memory {
        fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
            self.0
                .borrow()
                .get(reference)
                .cloned()
                .ok_or_else(credential_error)
        }
        fn write(&self, reference: &str, value: &str) -> Result<()> {
            self.0
                .borrow_mut()
                .insert(reference.into(), Zeroizing::new(value.into()));
            Ok(())
        }
    }

    #[test]
    fn keys_reload_from_credentials_and_only_the_named_recipient_can_unwrap() {
        let store = Memory::default();
        let owner = DeviceKeys::create(&store).unwrap();
        let recipient = DeviceKeys::create(&store).unwrap();
        let stranger = DeviceKeys::create(&store).unwrap();
        let key = ObjectKey::generate();
        let envelope = owner
            .wrap_key(
                "workspace",
                "object",
                1,
                recipient.device_id(),
                &recipient.recipient(),
                &key,
            )
            .unwrap();
        let restored = DeviceKeys::load(&store, recipient.device_id()).unwrap();
        let unwrapped = restored
            .unwrap_key(&envelope, &owner.signer().public_key())
            .unwrap();
        let op = owner
            .signer()
            .seal(
                &key,
                "workspace",
                "object",
                owner.device_id(),
                1,
                b"private payload",
            )
            .unwrap();
        assert_eq!(
            &**op.open(&unwrapped, &owner.signer().public_key()).unwrap(),
            b"private payload"
        );
        assert!(
            stranger
                .unwrap_key(&envelope, &owner.signer().public_key())
                .is_err()
        );
        assert!(
            restored
                .unwrap_key(&envelope, &stranger.signer().public_key())
                .is_err()
        );
        let mut changed = envelope;
        changed.epoch = 2;
        assert!(
            restored
                .unwrap_key(&changed, &owner.signer().public_key())
                .is_err()
        );
    }

    #[test]
    fn recovery_identity_is_exported_outside_workspace_without_overwriting_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let workspace = dir.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let store = Memory::default();
        let owner = DeviceKeys::create(&store).unwrap();
        let path = dir.path().join("recovery.txt");
        assert!(
            owner
                .export_recovery_identity(&workspace.join("secret.txt"), &workspace)
                .is_err()
        );
        let other_workspace = dir.path().join("other-workspace");
        std::fs::create_dir(&other_workspace).unwrap();
        std::fs::create_dir(other_workspace.join(".noura")).unwrap();
        std::fs::write(
            other_workspace.join(WORKSPACE_MANIFEST_PATH),
            b"id: another",
        )
        .unwrap();
        assert!(
            owner
                .export_recovery_identity(&other_workspace.join("secret.txt"), &workspace)
                .is_err()
        );
        let recipient = owner.export_recovery_identity(&path, &workspace).unwrap();
        assert!(owner.export_recovery_identity(&path, &workspace).is_err());
        let key = ObjectKey::generate();
        let envelope = owner
            .wrap_key(
                "workspace",
                "object",
                1,
                "recovery_device",
                &recipient,
                &key,
            )
            .unwrap();
        let recovered =
            DeviceKeys::recover_key(&path, &envelope, &owner.signer().public_key()).unwrap();
        let op = owner
            .signer()
            .seal(
                &key,
                "workspace",
                "object",
                owner.device_id(),
                1,
                b"recoverable",
            )
            .unwrap();
        assert_eq!(
            &**op.open(&recovered, &owner.signer().public_key()).unwrap(),
            b"recoverable"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}

#[cfg(test)]
mod session_tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    #[derive(Default)]
    struct Store {
        reads: AtomicUsize,
        denied: AtomicBool,
        value: Mutex<Option<Zeroizing<String>>>,
    }
    impl SyncCredentials for Store {
        fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
            self.read_optional(reference)?.ok_or_else(credential_error)
        }
        fn read_optional(&self, _: &str) -> Result<Option<Zeroizing<String>>> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.denied.load(Ordering::SeqCst) {
                return Err(credential_error());
            }
            Ok(self.value.lock().unwrap().clone())
        }
        fn write(&self, _: &str, value: &str) -> Result<()> {
            if self.denied.load(Ordering::SeqCst) {
                return Err(credential_error());
            }
            *self.value.lock().unwrap() = Some(Zeroizing::new(value.to_owned()));
            Ok(())
        }
    }

    #[test]
    fn concurrent_navigation_reads_keychain_only_once() {
        let cache = SessionCredentials::new(Store::default());
        *cache.store.value.lock().unwrap() = Some(Zeroizing::new("secret".into()));
        std::thread::scope(|scope| {
            for _ in 0..16 {
                scope.spawn(|| assert_eq!(&*cache.read("device").unwrap(), "secret"));
            }
        });
        assert_eq!(cache.store.reads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn denied_requests_do_not_prompt_again_on_every_read() {
        let cache = SessionCredentials::new(Store::default());
        cache.store.denied.store(true, Ordering::SeqCst);
        for _ in 0..10 {
            assert_eq!(
                cache.read("device").unwrap_err().code,
                "sync_credentials_unavailable"
            );
        }
        assert_eq!(cache.store.reads.load(Ordering::SeqCst), 1);
        cache.store.denied.store(false, Ordering::SeqCst);
        cache.write("device", "new secret").unwrap();
        assert_eq!(&*cache.read("device").unwrap(), "new secret");
    }

    #[test]
    fn absence_is_cached_and_sign_in_and_sign_out_refresh_the_cache() {
        let cache = SessionCredentials::new(Store::default());
        for _ in 0..10 {
            assert!(cache.read_optional("connection").unwrap().is_none());
        }
        assert_eq!(cache.store.reads.load(Ordering::SeqCst), 1);
        cache.write("connection", "connected").unwrap();
        assert_eq!(&*cache.read("connection").unwrap(), "connected");
        assert_eq!(
            &**cache.store.value.lock().unwrap().as_ref().unwrap(),
            "connected"
        );
        cache.write("connection", "null").unwrap();
        assert_eq!(&*cache.read("connection").unwrap(), "null");
    }

    #[test]
    fn failed_writes_never_publish_the_new_secret() {
        let cache = SessionCredentials::new(Store::default());
        cache.write("device", "old secret").unwrap();
        cache.store.denied.store(true, Ordering::SeqCst);
        assert!(cache.write("device", "unconfirmed secret").is_err());
        assert!(cache.read("device").is_err());
        assert_eq!(
            &**cache.store.value.lock().unwrap().as_ref().unwrap(),
            "old secret"
        );
        assert_eq!(cache.store.reads.load(Ordering::SeqCst), 0);
    }
}

#[cfg(all(test, target_os = "macos"))]
mod noninteractive_keychain_tests {
    use super::*;
    use security_framework::os::macos::keychain::SecKeychain;

    #[test]
    fn operations_disable_prompts_and_restore_state_after_success_and_failure() {
        let original = SecKeychain::user_interaction_allowed().unwrap();
        for fail in [false, true] {
            let result = without_keychain_prompt(|| {
                assert!(!SecKeychain::user_interaction_allowed().unwrap());
                if fail {
                    Err(credential_error())
                } else {
                    Ok(())
                }
            });
            assert_eq!(result.is_err(), fail);
            assert_eq!(SecKeychain::user_interaction_allowed().unwrap(), original);
        }
    }
}
