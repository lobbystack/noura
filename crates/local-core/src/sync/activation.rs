use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use super::{
    CheckpointBlobManifest, DeviceKeys, DocumentDescriptor, EncryptedCheckpoint, PolicyEnvelope,
    WorkspaceCapability, crypto::decode, identifier, invalid,
};
use crate::Result;

/// Atomic publication of a writer-created collaborative object.
#[derive(Clone, Debug, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectActivation {
    #[ts(type = "1 | 2")]
    pub version: u8,
    pub activation_id: String,
    pub workspace_id: String,
    pub policy_revision: String,
    pub covered_sequence: String,
    pub capability_digest: String,
    pub device_id: String,
    pub document: DocumentDescriptor,
    pub envelopes: Vec<PolicyEnvelope>,
    pub checkpoint: EncryptedCheckpoint,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub blobs: Vec<CheckpointBlobManifest>,
    pub signature: String,
}

impl ObjectActivation {
    #[allow(clippy::too_many_arguments)]
    pub fn sign(
        device: &DeviceKeys,
        capability: &WorkspaceCapability,
        policy_revision: &str,
        covered_sequence: &str,
        document: DocumentDescriptor,
        mut envelopes: Vec<PolicyEnvelope>,
        checkpoint: EncryptedCheckpoint,
        mut blobs: Vec<CheckpointBlobManifest>,
    ) -> Result<Self> {
        envelopes.sort_by(|left, right| left.device_id.cmp(&right.device_id));
        blobs.sort_by(|left, right| {
            (&left.object_id, &left.ciphertext_digest)
                .cmp(&(&right.object_id, &right.ciphertext_digest))
        });
        let mut result = Self {
            version: if blobs.is_empty() { 1 } else { 2 },
            activation_id: uuid::Uuid::new_v4().to_string(),
            workspace_id: capability.workspace_id.clone(),
            policy_revision: policy_revision.into(),
            covered_sequence: covered_sequence.into(),
            capability_digest: capability.digest()?,
            device_id: device.device_id().into(),
            document,
            envelopes,
            checkpoint,
            blobs,
            signature: String::new(),
        };
        result.validate(&device.signer().public_key())?;
        result.signature = device.signer().sign_bytes(&result.signing_bytes()?);
        Ok(result)
    }

    fn validate(&self, public_key: &str) -> Result<()> {
        identifier(&self.activation_id)?;
        identifier(&self.workspace_id)?;
        identifier(&self.device_id)?;
        identifier(&self.document.generation)?;
        super::transport::parse_cursor(&self.policy_revision)?;
        super::transport::parse_cursor(&self.covered_sequence)?;
        if !matches!(self.version, 1 | 2)
            || (self.version == 1 && !self.blobs.is_empty())
            || self.capability_digest.len() != 64
            || !self.capability_digest.bytes().all(is_lower_hex)
            || self.envelopes.is_empty()
            || self.envelopes.len() > 1000
            || self.blobs.len() > 1
        {
            return Err(invalid("sync_invalid_object_activation"));
        }
        let operation = &self.checkpoint.payload;
        if operation.workspace_id != self.workspace_id
            || operation.device_id != self.device_id
            || operation.policy_revision != self.policy_revision
            || operation.epoch != 1
            || self.checkpoint.generation != self.document.generation
            || self.checkpoint.covered_sequence != self.covered_sequence
        {
            return Err(invalid("sync_invalid_object_activation"));
        }
        self.checkpoint.verify(public_key)?;
        let mut previous = "";
        for envelope in &self.envelopes {
            if envelope.device_id.as_str() <= previous {
                return Err(invalid("sync_invalid_object_activation"));
            }
            envelope
                .to_key_envelope(&self.workspace_id, &operation.object_id, 1, &self.device_id)?
                .verify(public_key)?;
            previous = &envelope.device_id;
        }
        for blob in &self.blobs {
            if blob.object_id != operation.object_id
                || blob.epoch != 1
                || blob.ciphertext_size == 0
                || blob.ciphertext_size > super::blobs::MAX_BLOB_BYTES
                || blob.ciphertext_digest.len() != 64
                || !blob.ciphertext_digest.bytes().all(is_lower_hex)
            {
                return Err(invalid("sync_invalid_object_activation_blob"));
            }
        }
        Ok(())
    }

    pub fn verify(&self, public_key: &str) -> Result<()> {
        self.validate(public_key)?;
        let key: [u8; 32] = decode(public_key, 32, 32)?
            .try_into()
            .map_err(|_| invalid("sync_invalid_key"))?;
        let signature = Signature::from_slice(&decode(&self.signature, 64, 64)?)
            .map_err(|_| invalid("sync_invalid_signature"))?;
        VerifyingKey::from_bytes(&key)
            .map_err(|_| invalid("sync_invalid_key"))?
            .verify_strict(&self.signing_bytes()?, &signature)
            .map_err(|_| invalid("sync_invalid_signature"))
    }

    fn signing_bytes(&self) -> Result<Vec<u8>> {
        let envelopes: Vec<_> = self
            .envelopes
            .iter()
            .map(|envelope| {
                if envelope.is_web() {
                    serde_json::json!([
                        &envelope.device_id,
                        &envelope.wrapped_key,
                        &envelope.signature,
                        "web",
                        envelope.recipient_public_key.as_deref(),
                        envelope.ephemeral_public_key.as_deref(),
                        envelope.salt.as_deref(),
                        envelope.nonce.as_deref(),
                    ])
                } else {
                    serde_json::json!([
                        &envelope.device_id,
                        &envelope.wrapped_key,
                        &envelope.signature
                    ])
                }
            })
            .collect();
        let value = if self.version == 1 {
            serde_json::json!([
                "noura.sync.object-activation",
                self.version,
                &self.activation_id,
                &self.workspace_id,
                &self.policy_revision,
                &self.covered_sequence,
                &self.capability_digest,
                &self.device_id,
                [&self.document.generation, self.document.mode],
                envelopes,
                self.checkpoint.digest()?,
            ])
        } else {
            let blobs: Vec<_> = self
                .blobs
                .iter()
                .map(|blob| {
                    serde_json::json!([
                        &blob.object_id,
                        blob.epoch,
                        &blob.ciphertext_digest,
                        blob.ciphertext_size
                    ])
                })
                .collect();
            serde_json::json!([
                "noura.sync.object-activation",
                self.version,
                &self.activation_id,
                &self.workspace_id,
                &self.policy_revision,
                &self.covered_sequence,
                &self.capability_digest,
                &self.device_id,
                [&self.document.generation, self.document.mode],
                envelopes,
                self.checkpoint.digest()?,
                blobs,
            ])
        };
        serde_json::to_vec(&value).map_err(|_| invalid("sync_serialize_failed"))
    }

    pub fn digest(&self) -> Result<String> {
        let mut hash = Sha256::new();
        hash.update(self.signing_bytes()?);
        hash.update(decode(&self.signature, 64, 64)?);
        Ok(format!("{:x}", hash.finalize()))
    }
}

fn is_lower_hex(byte: u8) -> bool {
    byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{CheckpointContent, DocumentMode, FileChange, ObjectKey, SyncCredentials};
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::{cell::RefCell, collections::BTreeMap};
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, String>>);
    impl SyncCredentials for Memory {
        fn read(&self, key: &str) -> Result<Zeroizing<String>> {
            self.0
                .borrow()
                .get(key)
                .cloned()
                .map(Zeroizing::new)
                .ok_or_else(|| invalid("test_missing"))
        }
        fn write(&self, key: &str, value: &str) -> Result<()> {
            self.0.borrow_mut().insert(key.into(), value.into());
            Ok(())
        }
    }

    fn activation_content() -> CheckpointContent {
        let bytes = b"fresh collaborative object";
        CheckpointContent {
            version: 1,
            object_id: "object".into(),
            generation: "generation".into(),
            content_revision: Some(blake3::hash(bytes).to_hex().to_string()),
            change: FileChange {
                version: 1,
                path: "new.txt".into(),
                previous_path: None,
                base_revision: None,
                content: Some(STANDARD.encode(bytes)),
                accepted_revisions: None,
                blob: None,
            },
        }
    }

    #[test]
    fn activation_binds_capability_policy_checkpoint_recipients_and_blobs() {
        let store = Memory::default();
        let device = DeviceKeys::create(&store).unwrap();
        let capability = WorkspaceCapability::sign("workspace", &device).unwrap();
        let key = ObjectKey::generate();
        let checkpoint = EncryptedCheckpoint::seal_activation(
            &device,
            &key,
            "workspace",
            "3",
            "8",
            &activation_content(),
        )
        .unwrap();
        let envelope = PolicyEnvelope::from(
            device
                .wrap_key(
                    "workspace",
                    "object",
                    1,
                    device.device_id(),
                    &device.recipient(),
                    &key,
                )
                .unwrap(),
        );
        let activation = ObjectActivation::sign(
            &device,
            &capability,
            "3",
            "8",
            DocumentDescriptor {
                generation: "generation".into(),
                mode: DocumentMode::Text,
            },
            vec![envelope],
            checkpoint,
            vec![],
        )
        .unwrap();
        activation.verify(&device.signer().public_key()).unwrap();
        assert_eq!(activation.version, 1);
        let mut altered = activation.clone();
        altered.policy_revision = "4".into();
        assert_eq!(
            altered
                .verify(&device.signer().public_key())
                .unwrap_err()
                .code,
            "sync_invalid_object_activation"
        );
    }

    #[test]
    fn activation_signs_and_verifies_mixed_age_and_web_envelopes() {
        let store = Memory::default();
        let device = DeviceKeys::create(&store).unwrap();
        let capability = WorkspaceCapability::sign("workspace", &device).unwrap();
        let key = ObjectKey::generate();
        let checkpoint = EncryptedCheckpoint::seal_activation(
            &device,
            &key,
            "workspace",
            "3",
            "8",
            &activation_content(),
        )
        .unwrap();
        let age = PolicyEnvelope::from(
            device
                .wrap_key(
                    "workspace",
                    "object",
                    1,
                    device.device_id(),
                    &device.recipient(),
                    &key,
                )
                .unwrap(),
        );
        let web = PolicyEnvelope::from(
            device
                .wrap_key(
                    "workspace",
                    "object",
                    1,
                    "device_browser",
                    &sync_key_envelope::encode_recipient([0x07_u8; 32]),
                    &key,
                )
                .unwrap(),
        );
        assert!(web.is_web());
        let activation = ObjectActivation::sign(
            &device,
            &capability,
            "3",
            "8",
            DocumentDescriptor {
                generation: "generation".into(),
                mode: DocumentMode::Text,
            },
            vec![web, age],
            checkpoint,
            vec![],
        )
        .unwrap();
        activation.verify(&device.signer().public_key()).unwrap();
        assert!(
            activation
                .envelopes
                .iter()
                .any(|envelope| envelope.is_web())
        );

        // A tampered browser field fails envelope verification before the outer signature.
        let mut tampered = activation.clone();
        tampered
            .envelopes
            .iter_mut()
            .find(|envelope| envelope.is_web())
            .unwrap()
            .salt = Some(STANDARD.encode([0x09_u8; 32]));
        assert_eq!(
            tampered
                .verify(&device.signer().public_key())
                .unwrap_err()
                .code,
            "sync_invalid_signature"
        );
    }

    /// Pin the activation signing tuple for a browser recipient against the shared
    /// fixture so the native and server byte sequences cannot drift apart.
    #[test]
    fn activation_web_signing_tuple_matches_the_shared_fixture() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/activation-v1.json"
        ))
        .unwrap();
        let vector = &fixture["vectors"][0];
        let activation: ObjectActivation =
            serde_json::from_value(vector["activation"].clone()).unwrap();
        assert!(
            activation
                .envelopes
                .iter()
                .any(|envelope| envelope.is_web())
        );
        assert_eq!(
            String::from_utf8(activation.signing_bytes().unwrap()).unwrap(),
            vector["expected_signing_bytes"].as_str().unwrap()
        );
    }
}
