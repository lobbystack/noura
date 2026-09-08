use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, AeadCore, OsRng, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{DeviceKeys, ObjectKey, decode, identifier, invalid};
use crate::Result;

const MAX_RELATIVE_POSITION_BYTES: usize = 512;
const MAX_PRESENCE_CIPHERTEXT_BYTES: usize = 4096;
const MAX_SAFE_INTEGER: u64 = 9_007_199_254_740_991;

/// Relative Yjs selections cross IPC only long enough for the native client to encrypt them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationPresenceInput {
    pub session_id: String,
    pub anchor: String,
    pub head: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationPresenceMember {
    pub device_id: String,
    pub name: String,
    pub color: String,
    pub anchor: String,
    pub head: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CollaborationPresenceEvent {
    pub object_id: String,
    pub generation: String,
    pub presence: Vec<CollaborationPresenceMember>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PresenceSelection {
    pub anchor: String,
    pub head: String,
}

pub struct PresenceContext<'a> {
    pub workspace_id: &'a str,
    pub object_id: &'a str,
    pub generation: &'a str,
    pub epoch: u64,
    pub session_id: &'a str,
    pub sequence: u64,
}

/// Opaque transient presence. It is never written to the workspace or relay database.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptedPresence {
    #[ts(type = "1")]
    pub version: u8,
    pub workspace_id: String,
    pub object_id: String,
    pub generation: String,
    #[ts(type = "number")]
    pub epoch: u64,
    pub device_id: String,
    pub session_id: String,
    #[ts(type = "number")]
    pub sequence: u64,
    pub nonce: String,
    pub ciphertext: String,
    pub signature: String,
}

impl PresenceSelection {
    fn validate(&self) -> Result<()> {
        for value in [&self.anchor, &self.head] {
            decode(value, 1, MAX_RELATIVE_POSITION_BYTES)?;
        }
        Ok(())
    }
}

impl EncryptedPresence {
    pub fn seal(
        device: &DeviceKeys,
        key: &ObjectKey,
        context: &PresenceContext<'_>,
        selection: &PresenceSelection,
    ) -> Result<Self> {
        selection.validate()?;
        for value in [
            context.workspace_id,
            context.object_id,
            context.generation,
            device.device_id(),
            context.session_id,
        ] {
            identifier(value)?;
        }
        if context.epoch == 0
            || context.epoch > MAX_SAFE_INTEGER
            || context.sequence == 0
            || context.sequence > MAX_SAFE_INTEGER
        {
            return Err(invalid("collaboration_invalid_presence"));
        }
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let mut value = Self {
            version: 1,
            workspace_id: context.workspace_id.into(),
            object_id: context.object_id.into(),
            generation: context.generation.into(),
            epoch: context.epoch,
            device_id: device.device_id().into(),
            session_id: context.session_id.into(),
            sequence: context.sequence,
            nonce: STANDARD.encode(nonce),
            ciphertext: String::new(),
            signature: String::new(),
        };
        let plaintext = Zeroizing::new(
            serde_json::to_vec(selection).map_err(|_| invalid("sync_serialize_failed"))?,
        );
        value.ciphertext = STANDARD.encode(
            Aes256Gcm::new(key.secret().into())
                .encrypt(
                    &nonce,
                    Payload {
                        msg: &plaintext,
                        aad: &value.associated_data()?,
                    },
                )
                .map_err(|_| invalid("sync_encrypt_failed"))?,
        );
        value.signature = device.signer().sign_bytes(&value.signing_bytes()?);
        value.validate()?;
        Ok(value)
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != 1
            || self.epoch == 0
            || self.epoch > MAX_SAFE_INTEGER
            || self.sequence == 0
            || self.sequence > MAX_SAFE_INTEGER
        {
            return Err(invalid("collaboration_invalid_presence"));
        }
        for value in [
            &self.workspace_id,
            &self.object_id,
            &self.generation,
            &self.device_id,
            &self.session_id,
        ] {
            identifier(value)?;
        }
        decode(&self.nonce, 12, 12)?;
        decode(&self.ciphertext, 16, MAX_PRESENCE_CIPHERTEXT_BYTES)?;
        decode(&self.signature, 64, 64)?;
        Ok(())
    }

    pub fn verify(&self, public_key: &str) -> Result<()> {
        self.validate()?;
        let bytes: [u8; 32] = decode(public_key, 32, 32)?
            .try_into()
            .map_err(|_| invalid("sync_invalid_key"))?;
        let verifier = VerifyingKey::from_bytes(&bytes).map_err(|_| invalid("sync_invalid_key"))?;
        let signature = Signature::from_slice(&decode(&self.signature, 64, 64)?)
            .map_err(|_| invalid("sync_invalid_signature"))?;
        verifier
            .verify_strict(&self.signing_bytes()?, &signature)
            .map_err(|_| invalid("sync_invalid_signature"))
    }

    pub fn open(&self, key: &ObjectKey, trusted_public_key: &str) -> Result<PresenceSelection> {
        self.verify(trusted_public_key)?;
        let nonce: [u8; 12] = decode(&self.nonce, 12, 12)?
            .try_into()
            .map_err(|_| invalid("sync_invalid_nonce"))?;
        let ciphertext = decode(&self.ciphertext, 16, MAX_PRESENCE_CIPHERTEXT_BYTES)?;
        let plaintext = Zeroizing::new(
            Aes256Gcm::new(key.secret().into())
                .decrypt(
                    (&nonce).into(),
                    Payload {
                        msg: &ciphertext,
                        aad: &self.associated_data()?,
                    },
                )
                .map_err(|_| invalid("sync_decrypt_failed"))?,
        );
        let selection: PresenceSelection = serde_json::from_slice(&plaintext)
            .map_err(|_| invalid("collaboration_invalid_presence"))?;
        selection.validate()?;
        Ok(selection)
    }

    fn associated_data(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&(
            "noura.sync.presence.payload",
            self.version,
            &self.workspace_id,
            &self.object_id,
            &self.generation,
            self.epoch,
            &self.device_id,
            &self.session_id,
            self.sequence,
        ))
        .map_err(|_| invalid("sync_serialize_failed"))
    }

    pub fn signing_bytes(&self) -> Result<Vec<u8>> {
        self.validate_without_signature()?;
        serde_json::to_vec(&(
            "noura.sync.presence",
            self.version,
            &self.workspace_id,
            &self.object_id,
            &self.generation,
            self.epoch,
            &self.device_id,
            &self.session_id,
            self.sequence,
            &self.nonce,
            &self.ciphertext,
        ))
        .map_err(|_| invalid("sync_serialize_failed"))
    }

    fn validate_without_signature(&self) -> Result<()> {
        let mut value = self.clone();
        value.signature = STANDARD.encode([0_u8; 64]);
        value.validate()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SyncCredentials;
    use std::{collections::BTreeMap, sync::Mutex};

    #[derive(Default)]
    struct Credentials(Mutex<BTreeMap<String, String>>);
    impl SyncCredentials for Credentials {
        fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
            self.0
                .lock()
                .unwrap()
                .get(reference)
                .cloned()
                .map(Zeroizing::new)
                .ok_or_else(|| invalid("missing"))
        }
        fn write(&self, reference: &str, value: &str) -> Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(reference.into(), value.into());
            Ok(())
        }
    }

    #[test]
    fn presence_is_context_bound_signed_and_encrypted() {
        let device = DeviceKeys::create(&Credentials::default()).unwrap();
        let key = ObjectKey::generate();
        let selection = PresenceSelection {
            anchor: STANDARD.encode([1, 2, 3]),
            head: STANDARD.encode([4, 5, 6]),
        };
        let value = EncryptedPresence::seal(
            &device,
            &key,
            &PresenceContext {
                workspace_id: "workspace",
                object_id: "object",
                generation: "generation",
                epoch: 2,
                session_id: "session",
                sequence: 1,
            },
            &selection,
        )
        .unwrap();
        assert_eq!(
            value.open(&key, &device.signer().public_key()).unwrap(),
            selection
        );
        assert!(!value.ciphertext.contains(&selection.anchor));
        for changed in [
            EncryptedPresence {
                sequence: 2,
                ..value.clone()
            },
            EncryptedPresence {
                generation: "other".into(),
                ..value.clone()
            },
            EncryptedPresence {
                session_id: "other".into(),
                ..value.clone()
            },
        ] {
            assert!(changed.verify(&device.signer().public_key()).is_err());
        }
        assert!(
            value
                .open(&ObjectKey::generate(), &device.signer().public_key())
                .is_err()
        );
    }
}
