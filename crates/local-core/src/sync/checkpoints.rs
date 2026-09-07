//! Signed current-state checkpoints. Plaintext metadata stays inside the encrypted payload.

use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zeroize::Zeroizing;

use super::{
    AccessPolicy, DeviceKeys, EncryptedOperation, FileChange, ObjectKey, crypto::decode,
    identifier, invalid, validate_file_change,
};
use crate::Result;

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptedCheckpoint {
    #[ts(type = "1")]
    pub version: u8,
    pub generation: String,
    pub covered_sequence: String,
    pub payload: EncryptedOperation,
    pub signature: String,
}

/// Contains only a current file snapshot, never a prior operation or CRDT history.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CheckpointContent {
    pub version: u8,
    pub object_id: String,
    pub generation: String,
    pub content_revision: Option<String>,
    pub change: FileChange,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessTransition {
    #[ts(type = "1")]
    pub version: u8,
    pub transition_id: String,
    pub covered_sequence: String,
    pub policy: AccessPolicy,
    pub checkpoints: Vec<EncryptedCheckpoint>,
    pub signature: String,
}

impl CheckpointContent {
    fn validate(&self) -> Result<()> {
        identifier(&self.object_id)?;
        identifier(&self.generation)?;
        validate_file_change(&self.change)?;
        if self.version != 1
            || self.change.base_revision.is_some()
            || self.change.previous_path.is_some()
            || self.change.accepted_revisions.is_some()
        {
            return Err(invalid("sync_invalid_checkpoint"));
        }
        let revision = if let Some(content) = &self.change.content {
            let bytes = Zeroizing::new(decode(content, 0, 1024 * 1024)?);
            Some(blake3::hash(&bytes).to_hex().to_string())
        } else {
            self.change.blob.as_ref().map(|blob| blob.revision.clone())
        };
        if self.content_revision != revision {
            return Err(invalid("sync_invalid_checkpoint_revision"));
        }
        Ok(())
    }
}

impl EncryptedCheckpoint {
    pub fn seal(
        device: &DeviceKeys,
        key: &ObjectKey,
        policy: &AccessPolicy,
        covered_sequence: &str,
        content: &CheckpointContent,
    ) -> Result<Self> {
        content.validate()?;
        policy.verify(&device.signer().public_key())?;
        super::transport::parse_cursor(covered_sequence)?;
        let object = policy
            .objects
            .iter()
            .find(|object| object.object_id == content.object_id)
            .ok_or_else(|| invalid("sync_invalid_checkpoint"))?;
        let plaintext = Zeroizing::new(
            serde_json::to_vec(content).map_err(|_| invalid("sync_serialize_failed"))?,
        );
        let payload = device.signer().seal_at_revision(
            key,
            &policy.workspace_id,
            &content.object_id,
            device.device_id(),
            (object.epoch, &policy.revision),
            &plaintext,
        )?;
        let mut result = Self {
            version: 1,
            generation: content.generation.clone(),
            covered_sequence: covered_sequence.into(),
            payload,
            signature: String::new(),
        };
        result.signature = device.signer().sign_bytes(&result.signing_bytes()?);
        Ok(result)
    }

    pub fn verify(&self, public_key: &str) -> Result<()> {
        if self.version != 1 {
            return Err(invalid("sync_invalid_checkpoint"));
        }
        identifier(&self.generation)?;
        super::transport::parse_cursor(&self.covered_sequence)?;
        self.payload.verify(public_key)?;
        verify(&self.signing_bytes()?, &self.signature, public_key)
    }

    /// The caller must match this checkpoint to a trusted committed transition before applying it.
    pub fn open(&self, key: &ObjectKey, public_key: &str) -> Result<CheckpointContent> {
        self.verify(public_key)?;
        let plaintext = self.payload.open(key, public_key)?;
        let result: CheckpointContent =
            serde_json::from_slice(&plaintext).map_err(|_| invalid("sync_invalid_checkpoint"))?;
        result.validate()?;
        if result.object_id != self.payload.object_id || result.generation != self.generation {
            return Err(invalid("sync_invalid_checkpoint"));
        }
        Ok(result)
    }

    fn signing_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&(
            "noura.sync.checkpoint",
            self.version,
            &self.generation,
            &self.covered_sequence,
            self.payload.digest()?,
        ))
        .map_err(|_| invalid("sync_serialize_failed"))
    }
    pub fn digest(&self) -> Result<String> {
        signed_digest(&self.signing_bytes()?, &self.signature)
    }
}

impl AccessTransition {
    pub fn sign(
        device: &DeviceKeys,
        policy: AccessPolicy,
        covered_sequence: String,
        mut checkpoints: Vec<EncryptedCheckpoint>,
    ) -> Result<Self> {
        checkpoints.sort_by(|a, b| a.payload.object_id.cmp(&b.payload.object_id));
        let mut result = Self {
            version: 1,
            transition_id: uuid::Uuid::new_v4().to_string(),
            covered_sequence,
            policy,
            checkpoints,
            signature: String::new(),
        };
        result.validate(&device.signer().public_key())?;
        result.signature = device.signer().sign_bytes(&result.signing_bytes()?);
        Ok(result)
    }
    fn validate(&self, public_key: &str) -> Result<()> {
        identifier(&self.transition_id)?;
        super::transport::parse_cursor(&self.covered_sequence)?;
        self.policy.verify(public_key)?;
        if self.version != 1 || self.checkpoints.len() > 1000 {
            return Err(invalid("sync_invalid_transition"));
        }
        let mut previous = "";
        for checkpoint in &self.checkpoints {
            checkpoint.verify(public_key)?;
            let op = &checkpoint.payload;
            if op.object_id.as_str() <= previous
                || op.workspace_id != self.policy.workspace_id
                || op.device_id != self.policy.device_id
                || op.policy_revision != self.policy.revision
                || checkpoint.covered_sequence != self.covered_sequence
                || !self.policy.objects.iter().any(|object| {
                    object.object_id == op.object_id
                        && object.epoch == op.epoch
                        && object
                            .document
                            .as_ref()
                            .is_none_or(|document| document.generation == checkpoint.generation)
                })
            {
                return Err(invalid("sync_invalid_transition"));
            }
            previous = &op.object_id;
        }
        Ok(())
    }
    pub fn verify(&self, public_key: &str) -> Result<()> {
        self.validate(public_key)?;
        verify(&self.signing_bytes()?, &self.signature, public_key)
    }
    fn signing_bytes(&self) -> Result<Vec<u8>> {
        let digests: Vec<_> = self
            .checkpoints
            .iter()
            .map(EncryptedCheckpoint::digest)
            .collect::<Result<_>>()?;
        serde_json::to_vec(&(
            "noura.sync.transition",
            self.version,
            &self.transition_id,
            &self.covered_sequence,
            self.policy.digest()?,
            digests,
        ))
        .map_err(|_| invalid("sync_serialize_failed"))
    }
    pub fn digest(&self) -> Result<String> {
        signed_digest(&self.signing_bytes()?, &self.signature)
    }
}

fn verify(bytes: &[u8], signature: &str, public_key: &str) -> Result<()> {
    let key: [u8; 32] = decode(public_key, 32, 32)?
        .try_into()
        .map_err(|_| invalid("sync_invalid_key"))?;
    let signature = Signature::from_slice(&decode(signature, 64, 64)?)
        .map_err(|_| invalid("sync_invalid_signature"))?;
    VerifyingKey::from_bytes(&key)
        .map_err(|_| invalid("sync_invalid_key"))?
        .verify_strict(bytes, &signature)
        .map_err(|_| invalid("sync_invalid_signature"))
}
fn signed_digest(bytes: &[u8], signature: &str) -> Result<String> {
    let mut hash = Sha256::new();
    hash.update(bytes);
    hash.update(decode(signature, 64, 64)?);
    Ok(format!("{:x}", hash.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_two_fixture_binds_policy_and_text_generation() {
        use crate::sync::collaboration::{CollaborativeChange, TextDocument};
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/collaboration-v2.json"
        ))
        .unwrap();
        let public = value["publicKey"].as_str().unwrap();
        let transition: AccessTransition =
            serde_json::from_value(value["transition"].clone()).unwrap();
        transition.verify(public).unwrap();
        assert_eq!(transition.policy.version, 2);
        let operation: EncryptedOperation =
            serde_json::from_value(value["liveOperation"].clone()).unwrap();
        let change: CollaborativeChange = serde_json::from_slice(
            &operation
                .open(&ObjectKey::from_bytes([7; 32]), public)
                .unwrap(),
        )
        .unwrap();
        change.validate().unwrap();
        let base = TextDocument::fresh_generation("current content\n", &change.generation).unwrap();
        assert_eq!(
            base.apply(&change.updates).unwrap().text(),
            "current content 😀\n"
        );
        let mut altered = operation;
        altered.generation = Some("stale".into());
        assert!(
            altered
                .open(&ObjectKey::from_bytes([7; 32]), public)
                .is_err()
        );
    }

    #[test]
    fn transition_journal_survives_restart_and_rebuild_without_discarding_edits() {
        use crate::{
            WorkspaceEngine,
            sync::{AccessMember, SyncCredentials, WorkspaceRole},
        };
        struct Credentials;
        impl SyncCredentials for Credentials {
            fn read(&self, _: &str) -> Result<Zeroizing<String>> {
                Err(invalid("test_missing"))
            }
            fn write(&self, _: &str, _: &str) -> Result<()> {
                Ok(())
            }
        }
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("workspace");
        let app = directory.path().join("app");
        let engine = WorkspaceEngine::create_with_app_data(&root, "Test", &app).unwrap();
        let device = DeviceKeys::create(&Credentials).unwrap();
        let public_key = device.signer().public_key();
        let policy = AccessPolicy::sign(
            &engine.manifest().id,
            "1",
            None,
            &device,
            vec![AccessMember {
                account_id: "owner".into(),
                role: WorkspaceRole::Owner,
            }],
            vec![],
        )
        .unwrap();
        let transition = AccessTransition::sign(&device, policy, "0".into(), vec![]).unwrap();
        let operation = device
            .signer()
            .seal(
                &ObjectKey::generate(),
                &engine.manifest().id,
                "object",
                device.device_id(),
                1,
                b"pending edit",
            )
            .unwrap();
        engine.sync_enqueue(&operation, &public_key).unwrap();
        engine
            .sync_prepare_transition(&transition, &public_key)
            .unwrap();
        engine
            .sync_prepare_transition(&transition, &public_key)
            .unwrap();
        drop(engine);
        let reopened = WorkspaceEngine::open_with_app_data(&root, &app).unwrap();
        reopened.rebuild_index().unwrap();
        assert_eq!(
            reopened
                .sync_pending_transition()
                .unwrap()
                .unwrap()
                .digest()
                .unwrap(),
            transition.digest().unwrap()
        );
        assert_eq!(reopened.sync_outbox().unwrap(), vec![operation]);
        let other =
            AccessTransition::sign(&device, transition.policy.clone(), "0".into(), vec![]).unwrap();
        assert_eq!(
            reopened
                .sync_prepare_transition(&other, &public_key)
                .unwrap_err()
                .code,
            "sync_transition_pending"
        );
    }

    fn fixture() -> (AccessTransition, String, String) {
        let value: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/checkpoint-v1.json"
        ))
        .unwrap();
        (
            serde_json::from_value(value["transition"].clone()).unwrap(),
            value["publicKey"].as_str().unwrap().into(),
            value["digest"].as_str().unwrap().into(),
        )
    }

    #[test]
    fn shared_fixture_verifies_and_decrypts_only_current_content() {
        let (transition, public_key, digest) = fixture();
        transition.verify(&public_key).unwrap();
        assert_eq!(transition.digest().unwrap(), digest);
        let content = transition.checkpoints[0]
            .open(&ObjectKey::from_bytes([7; 32]), &public_key)
            .unwrap();
        assert_eq!(content.change.path, "private/note.txt");
        assert_eq!(
            decode(content.change.content.as_ref().unwrap(), 0, 1024).unwrap(),
            b"current content\n"
        );
        assert!(
            transition.checkpoints[0]
                .open(&ObjectKey::from_bytes([8; 32]), &public_key)
                .is_err()
        );
    }

    #[test]
    fn signatures_reject_replayed_generation_boundary_and_policy() {
        let (transition, public_key, _) = fixture();
        for field in ["generation", "coveredSequence"] {
            let mut value = serde_json::to_value(&transition).unwrap();
            value["checkpoints"][0][field] = serde_json::json!("5");
            let changed: AccessTransition = serde_json::from_value(value).unwrap();
            assert!(changed.verify(&public_key).is_err(), "{field}");
        }
        let mut changed = transition;
        changed.policy.objects[0].epoch += 1;
        assert!(changed.verify(&public_key).is_err());
    }

    #[test]
    fn checkpoint_content_rejects_history_revision_mismatch_and_traversal() {
        let (transition, public_key, _) = fixture();
        let content = transition.checkpoints[0]
            .open(&ObjectKey::from_bytes([7; 32]), &public_key)
            .unwrap();
        let mut changed = content.clone();
        changed.content_revision = Some("0".repeat(64));
        assert!(changed.validate().is_err());
        let mut changed = content.clone();
        changed.change.base_revision = content.content_revision.clone();
        assert!(changed.validate().is_err());
        let mut changed = content;
        changed.change.path = "../outside.txt".into();
        assert!(changed.validate().is_err());
    }
}
