use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use ts_rs::TS;

use super::{DeviceKeys, crypto::decode, identifier, invalid};
use crate::Result;

/// Owner-signed authorization to create collaborative generations in a workspace.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkspaceCapability {
    #[ts(type = "1")]
    pub version: u8,
    pub workspace_id: String,
    #[ts(type = "1")]
    pub collaboration_version: u8,
    #[ts(type = "1")]
    pub minimum_client_version: u8,
    #[ts(type = "1")]
    pub minimum_relay_version: u8,
    pub device_id: String,
    pub signature: String,
}

impl WorkspaceCapability {
    pub fn sign(workspace_id: &str, device: &DeviceKeys) -> Result<Self> {
        identifier(workspace_id)?;
        let mut result = Self {
            version: 1,
            workspace_id: workspace_id.into(),
            collaboration_version: 1,
            minimum_client_version: 1,
            minimum_relay_version: 1,
            device_id: device.device_id().into(),
            signature: String::new(),
        };
        result.signature = device.signer().sign_bytes(&result.signing_bytes()?);
        Ok(result)
    }

    pub fn verify(&self, public_key: &str) -> Result<()> {
        if self.version != 1
            || self.collaboration_version != 1
            || self.minimum_client_version != 1
            || self.minimum_relay_version != 1
        {
            return Err(invalid("sync_incompatible_collaboration_capability"));
        }
        identifier(&self.workspace_id)?;
        identifier(&self.device_id)?;
        let verifying = VerifyingKey::from_bytes(
            &decode(public_key, 32, 32)?
                .try_into()
                .map_err(|_| invalid("sync_invalid_public_key"))?,
        )
        .map_err(|_| invalid("sync_invalid_public_key"))?;
        let signature = Signature::from_slice(&decode(&self.signature, 64, 64)?)
            .map_err(|_| invalid("sync_invalid_signature"))?;
        verifying
            .verify_strict(&self.signing_bytes()?, &signature)
            .map_err(|_| invalid("sync_invalid_signature"))
    }

    fn signing_bytes(&self) -> Result<Vec<u8>> {
        serde_json::to_vec(&(
            "noura.sync.workspace-capability",
            self.version,
            &self.workspace_id,
            self.collaboration_version,
            self.minimum_client_version,
            self.minimum_relay_version,
            &self.device_id,
        ))
        .map_err(|_| invalid("sync_serialize_failed"))
    }

    pub fn digest(&self) -> Result<String> {
        let mut hash = Sha256::new();
        hash.update(self.signing_bytes()?);
        hash.update(super::crypto::decode(&self.signature, 64, 64)?);
        Ok(format!("{:x}", hash.finalize()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::{SigningIdentity, SyncCredentials};
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

    #[test]
    fn capability_binds_workspace_versions_and_owner_device() {
        let store = Memory::default();
        let device = DeviceKeys::create(&store).unwrap();
        let capability = WorkspaceCapability::sign("workspace", &device).unwrap();
        capability.verify(&device.signer().public_key()).unwrap();
        let mut changed = capability.clone();
        changed.workspace_id = "other".into();
        assert_eq!(
            changed
                .verify(&device.signer().public_key())
                .unwrap_err()
                .code,
            "sync_invalid_signature"
        );
        assert!(
            capability
                .verify(&SigningIdentity::generate().public_key())
                .is_err()
        );
    }
}
