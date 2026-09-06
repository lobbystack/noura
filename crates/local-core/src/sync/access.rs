use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{DeviceKeys, KeyEnvelope, crypto::decode, identifier, invalid};
use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceRole {
    Owner,
    Admin,
    Editor,
    Viewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ObjectRole {
    Editor,
    Viewer,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessMember {
    pub account_id: String,
    pub role: WorkspaceRole,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectGrant {
    pub account_id: String,
    pub role: ObjectRole,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyEnvelope {
    pub device_id: String,
    pub wrapped_key: String,
    pub signature: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessObject {
    pub object_id: String,
    pub epoch: u64,
    pub grants: Vec<ObjectGrant>,
    pub envelopes: Vec<PolicyEnvelope>,
}

/// Signed authorization metadata; it contains no plaintext content keys or workspace paths.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessPolicy {
    pub version: u8,
    pub workspace_id: String,
    pub revision: String,
    pub previous_policy_digest: Option<String>,
    pub device_id: String,
    pub members: Vec<AccessMember>,
    pub objects: Vec<AccessObject>,
    pub signature: String,
}

impl From<KeyEnvelope> for PolicyEnvelope {
    fn from(value: KeyEnvelope) -> Self {
        Self {
            device_id: value.device_id,
            wrapped_key: value.wrapped_key,
            signature: value.signature,
        }
    }
}

impl AccessPolicy {
    /// Sign a complete proposed policy after the caller has approved its membership and recipient keys.
    /// The server enforces revision CAS, role authority, complete coverage, and mandatory epoch rotation.
    pub fn sign(
        workspace: &str,
        revision: &str,
        previous_policy_digest: Option<String>,
        device: &DeviceKeys,
        members: Vec<AccessMember>,
        objects: Vec<AccessObject>,
    ) -> Result<Self> {
        let mut policy = Self {
            version: 1,
            workspace_id: workspace.into(),
            revision: revision.into(),
            previous_policy_digest,
            device_id: device.device_id().into(),
            members,
            objects,
            signature: String::new(),
        };
        policy
            .members
            .sort_by(|a, b| a.account_id.cmp(&b.account_id));
        policy.objects.sort_by(|a, b| a.object_id.cmp(&b.object_id));
        for object in &mut policy.objects {
            object
                .grants
                .sort_by(|a, b| a.account_id.cmp(&b.account_id));
            object
                .envelopes
                .sort_by(|a, b| a.device_id.cmp(&b.device_id));
        }
        policy.validate(&device.signer().public_key())?;
        policy.signature = device.signer().sign_bytes(&policy.signing_bytes()?);
        Ok(policy)
    }

    pub fn verify(&self, trusted_signer: &str) -> Result<()> {
        self.validate(trusted_signer)?;
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

    pub fn digest(&self) -> Result<String> {
        let mut hash = Sha256::new();
        hash.update(self.signing_bytes()?);
        hash.update(decode(&self.signature, 64, 64)?);
        Ok(hash
            .finalize()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect())
    }

    fn validate(&self, trusted_signer: &str) -> Result<()> {
        identifier(&self.workspace_id)?;
        identifier(&self.device_id)?;
        let revision: u64 = self
            .revision
            .parse()
            .map_err(|_| invalid("sync_invalid_policy"))?;
        if self.version != 1
            || revision == 0
            || revision > i64::MAX as u64
            || revision.to_string() != self.revision
        {
            return Err(invalid("sync_invalid_policy"));
        }
        if self.previous_policy_digest.as_ref().is_some_and(|digest| {
            digest.len() != 64
                || !digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err(invalid("sync_invalid_policy"));
        }
        ordered(self.members.iter().map(|v| v.account_id.as_str()))?;
        ordered(self.objects.iter().map(|v| v.object_id.as_str()))?;
        if !self
            .members
            .iter()
            .any(|member| member.role == WorkspaceRole::Owner)
        {
            return Err(invalid("sync_owner_required"));
        }
        for object in &self.objects {
            if object.epoch == 0 || object.epoch > 9_007_199_254_740_991 {
                return Err(invalid("sync_invalid_epoch"));
            }
            ordered(object.grants.iter().map(|v| v.account_id.as_str()))?;
            ordered(object.envelopes.iter().map(|v| v.device_id.as_str()))?;
            for envelope in &object.envelopes {
                KeyEnvelope {
                    workspace_id: self.workspace_id.clone(),
                    object_id: object.object_id.clone(),
                    epoch: object.epoch,
                    device_id: envelope.device_id.clone(),
                    wrapped_key: envelope.wrapped_key.clone(),
                    signing_device: self.device_id.clone(),
                    signature: envelope.signature.clone(),
                }
                .verify(trusted_signer)?;
            }
        }
        Ok(())
    }

    fn signing_bytes(&self) -> Result<Vec<u8>> {
        let members: Vec<_> = self
            .members
            .iter()
            .map(|v| (&v.account_id, v.role))
            .collect();
        let objects: Vec<_> = self
            .objects
            .iter()
            .map(|v| {
                let grants: Vec<_> = v.grants.iter().map(|g| (&g.account_id, g.role)).collect();
                let envelopes: Vec<_> = v
                    .envelopes
                    .iter()
                    .map(|e| (&e.device_id, &e.wrapped_key, &e.signature))
                    .collect();
                (&v.object_id, v.epoch, grants, envelopes)
            })
            .collect();
        serde_json::to_vec(&(
            "noura.sync.access",
            self.version,
            &self.workspace_id,
            &self.revision,
            &self.previous_policy_digest,
            &self.device_id,
            members,
            objects,
        ))
        .map_err(|_| invalid("sync_serialize_failed"))
    }
}

fn ordered<'a>(values: impl Iterator<Item = &'a str>) -> Result<()> {
    let mut previous = None;
    for (index, value) in values.enumerate() {
        identifier(value)?;
        if index >= 1000 || previous.is_some_and(|last| last >= value) {
            return Err(invalid("sync_policy_order"));
        }
        previous = Some(value);
    }
    Ok(())
}
