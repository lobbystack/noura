use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::{DeviceKeys, KeyConstruction, KeyEnvelope, crypto::decode, identifier, invalid};
use crate::Result;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum WorkspaceRole {
    Owner,
    Admin,
    Editor,
    Viewer,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum ObjectRole {
    Editor,
    Viewer,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessMember {
    pub account_id: String,
    pub role: WorkspaceRole,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ObjectGrant {
    pub account_id: String,
    pub role: ObjectRole,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PolicyEnvelope {
    pub device_id: String,
    pub wrapped_key: String,
    pub signature: String,
    #[ts(inline)]
    #[serde(default, skip_serializing_if = "KeyConstruction::is_age")]
    pub construction: KeyConstruction,
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recipient_public_key: Option<String>,
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ephemeral_public_key: Option<String>,
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub salt: Option<String>,
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentDescriptor {
    pub generation: String,
    pub mode: DocumentMode,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum DocumentMode {
    Text,
    Attachment,
}

#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessObject {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub document: Option<DocumentDescriptor>,
    pub object_id: String,
    #[ts(type = "number")]
    pub epoch: u64,
    pub grants: Vec<ObjectGrant>,
    pub envelopes: Vec<PolicyEnvelope>,
}

/// Signed authorization metadata; it contains no plaintext content keys or workspace paths.
#[derive(Clone, Debug, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AccessPolicy {
    #[ts(type = "1 | 2")]
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
            construction: value.construction,
            recipient_public_key: value.recipient_public_key,
            ephemeral_public_key: value.ephemeral_public_key,
            salt: value.salt,
            nonce: value.nonce,
        }
    }
}

impl PolicyEnvelope {
    /// Rebuild the discriminated envelope for policy signature verification or unwrap.
    pub(crate) fn to_key_envelope(
        &self,
        workspace_id: &str,
        object_id: &str,
        epoch: u64,
        signing_device: &str,
    ) -> Result<KeyEnvelope> {
        if self.construction == KeyConstruction::Web
            && (self.recipient_public_key.is_none()
                || self.ephemeral_public_key.is_none()
                || self.salt.is_none()
                || self.nonce.is_none())
        {
            return Err(invalid("sync_invalid_key_envelope"));
        }
        Ok(KeyEnvelope {
            workspace_id: workspace_id.into(),
            object_id: object_id.into(),
            epoch,
            device_id: self.device_id.clone(),
            wrapped_key: self.wrapped_key.clone(),
            signing_device: signing_device.into(),
            signature: self.signature.clone(),
            construction: self.construction,
            recipient_public_key: self.recipient_public_key.clone(),
            ephemeral_public_key: self.ephemeral_public_key.clone(),
            salt: self.salt.clone(),
            nonce: self.nonce.clone(),
        })
    }

    /// True when this envelope uses the browser `noura.sync.key.web` construction.
    pub(crate) fn is_web(&self) -> bool {
        self.construction == KeyConstruction::Web
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
            version: if objects.iter().any(|object| object.document.is_some()) {
                2
            } else {
                1
            },
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
        if !matches!(self.version, 1 | 2)
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
            if let Some(document) = &object.document {
                identifier(&document.generation)?;
                if self.version != 2 {
                    return Err(invalid("sync_invalid_policy"));
                }
            } else if self.version == 2 {
                return Err(invalid("sync_document_required"));
            }
            if object.epoch == 0 || object.epoch > 9_007_199_254_740_991 {
                return Err(invalid("sync_invalid_epoch"));
            }
            ordered(object.grants.iter().map(|v| v.account_id.as_str()))?;
            ordered(object.envelopes.iter().map(|v| v.device_id.as_str()))?;
            for envelope in &object.envelopes {
                envelope
                    .to_key_envelope(
                        &self.workspace_id,
                        &object.object_id,
                        object.epoch,
                        &self.device_id,
                    )?
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
                    .map(|e| {
                        if e.is_web() {
                            serde_json::json!([
                                &e.device_id,
                                &e.wrapped_key,
                                &e.signature,
                                "web",
                                e.recipient_public_key.as_deref(),
                                e.ephemeral_public_key.as_deref(),
                                e.salt.as_deref(),
                                e.nonce.as_deref(),
                            ])
                        } else {
                            serde_json::json!([&e.device_id, &e.wrapped_key, &e.signature])
                        }
                    })
                    .collect();
                if self.version == 2 {
                    serde_json::json!([
                        &v.object_id,
                        v.epoch,
                        grants,
                        envelopes,
                        v.document.as_ref().map(|d| (&d.generation, d.mode))
                    ])
                } else {
                    serde_json::json!([&v.object_id, v.epoch, grants, envelopes])
                }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{ObjectKey, SyncCredentials};
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use std::{cell::RefCell, collections::BTreeMap};
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct Memory(RefCell<BTreeMap<String, Zeroizing<String>>>);
    impl SyncCredentials for Memory {
        fn read(&self, reference: &str) -> Result<Zeroizing<String>> {
            self.0
                .borrow()
                .get(reference)
                .cloned()
                .ok_or_else(|| invalid("test_missing"))
        }
        fn write(&self, reference: &str, value: &str) -> Result<()> {
            self.0
                .borrow_mut()
                .insert(reference.into(), Zeroizing::new(value.into()));
            Ok(())
        }
    }

    #[test]
    fn browser_policy_envelopes_verify_and_bind_the_web_signing_tuple() {
        let store = Memory::default();
        let device = DeviceKeys::create_browser(&store).unwrap();
        let envelope = PolicyEnvelope::from(
            device
                .wrap_key(
                    "workspace",
                    "object",
                    1,
                    device.device_id(),
                    &device.recipient(),
                    &ObjectKey::generate(),
                )
                .unwrap(),
        );
        assert!(envelope.is_web());
        let policy = AccessPolicy::sign(
            "workspace",
            "1",
            None,
            &device,
            vec![AccessMember {
                account_id: "account".into(),
                role: WorkspaceRole::Owner,
            }],
            vec![AccessObject {
                document: None,
                object_id: "object".into(),
                epoch: 1,
                grants: vec![],
                envelopes: vec![envelope.clone()],
            }],
        )
        .unwrap();
        policy.verify(&device.signer().public_key()).unwrap();

        // The web envelope's browser fields are covered by the policy signing tuple,
        // so the age and web tuples remain distinct.
        let base = policy.signing_bytes().unwrap();
        let mut changed = envelope;
        changed.salt = Some(STANDARD.encode([0x03_u8; 32]));
        let mut tampered = policy;
        tampered.objects[0].envelopes[0] = changed;
        assert_ne!(tampered.signing_bytes().unwrap(), base);
        assert!(tampered.verify(&device.signer().public_key()).is_err());
    }
}
