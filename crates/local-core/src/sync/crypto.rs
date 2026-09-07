use aes_gcm::{
    Aes256Gcm, KeyInit,
    aead::{Aead, AeadCore, OsRng, Payload},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use super::{identifier, invalid};
use crate::Result;

const MAX_CIPHERTEXT: usize = 1024 * 1024;

/// Transport envelope compatible with the server's version-one protocol.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptedOperation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub generation: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<OperationKind>,
    #[ts(type = "1 | 2")]
    pub version: u8,
    pub operation_id: String,
    pub workspace_id: String,
    pub object_id: String,
    pub device_id: String,
    #[ts(type = "number")]
    pub epoch: u64,
    pub policy_revision: String,
    pub nonce: String,
    pub ciphertext: String,
    pub signature: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "snake_case")]
pub enum OperationKind {
    Text,
    Metadata,
    File,
}

/// A native-only object key. No Debug, Serialize or IPC implementation.
pub struct ObjectKey(Zeroizing<[u8; 32]>);

impl ObjectKey {
    pub(crate) fn secret(&self) -> &[u8; 32] {
        &self.0
    }
    pub fn generate() -> Self {
        Self(Zeroizing::new(Aes256Gcm::generate_key(OsRng).into()))
    }

    /// Import from an authorized native credential or decrypted key envelope.
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(Zeroizing::new(bytes))
    }
}

/// A native-only signing identity. Store the seed in the operating system keychain.
pub struct SigningIdentity(SigningKey);

impl SigningIdentity {
    pub(crate) fn seed(&self) -> Zeroizing<[u8; 32]> {
        Zeroizing::new(self.0.to_bytes())
    }

    pub(crate) fn sign_bytes(&self, bytes: &[u8]) -> String {
        STANDARD.encode(self.0.sign(bytes).to_bytes())
    }
    pub fn generate() -> Self {
        Self(SigningKey::generate(&mut OsRng))
    }

    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self(SigningKey::from_bytes(seed))
    }

    pub fn public_key(&self) -> String {
        STANDARD.encode(self.0.verifying_key().as_bytes())
    }

    /// Seal once, then persist and retry this exact envelope until acknowledged.
    pub fn seal(
        &self,
        key: &ObjectKey,
        workspace_id: &str,
        object_id: &str,
        device_id: &str,
        epoch: u64,
        plaintext: &[u8],
    ) -> Result<EncryptedOperation> {
        self.seal_at_revision(
            key,
            workspace_id,
            object_id,
            device_id,
            (epoch, "0"),
            plaintext,
        )
    }

    pub fn seal_at_revision(
        &self,
        key: &ObjectKey,
        workspace_id: &str,
        object_id: &str,
        device_id: &str,
        authorization: (u64, &str),
        plaintext: &[u8],
    ) -> Result<EncryptedOperation> {
        self.seal_context(
            key,
            workspace_id,
            object_id,
            device_id,
            (authorization.0, authorization.1, None),
            plaintext,
        )
    }

    pub fn seal_for_document(
        &self,
        key: &ObjectKey,
        workspace_id: &str,
        object_id: &str,
        device_id: &str,
        authorization: (u64, &str, &str, OperationKind),
        plaintext: &[u8],
    ) -> Result<EncryptedOperation> {
        self.seal_context(
            key,
            workspace_id,
            object_id,
            device_id,
            (
                authorization.0,
                authorization.1,
                Some((authorization.2, authorization.3)),
            ),
            plaintext,
        )
    }

    fn seal_context(
        &self,
        key: &ObjectKey,
        workspace_id: &str,
        object_id: &str,
        device_id: &str,
        authorization: (u64, &str, Option<(&str, OperationKind)>),
        plaintext: &[u8],
    ) -> Result<EncryptedOperation> {
        let (epoch, policy_revision, document) = authorization;
        if let Some((generation, _)) = document {
            identifier(generation)?;
        }
        for id in [workspace_id, object_id, device_id] {
            identifier(id)?;
        }
        if epoch == 0 || epoch > 9_007_199_254_740_991 || plaintext.len() > MAX_CIPHERTEXT - 16 {
            return Err(invalid("sync_invalid_envelope"));
        }
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let mut op = EncryptedOperation {
            version: if document.is_some() { 2 } else { 1 },
            generation: document.map(|(generation, _)| generation.into()),
            kind: document.map(|(_, kind)| kind),
            operation_id: uuid::Uuid::new_v4().to_string(),
            workspace_id: workspace_id.into(),
            object_id: object_id.into(),
            device_id: device_id.into(),
            epoch,
            policy_revision: policy_revision.into(),
            nonce: STANDARD.encode(nonce),
            ciphertext: String::new(),
            signature: String::new(),
        };
        let aad = op.associated_data()?;
        let cipher = Aes256Gcm::new((&*key.0).into());
        op.ciphertext = STANDARD.encode(
            cipher
                .encrypt(
                    &nonce,
                    Payload {
                        msg: plaintext,
                        aad: &aad,
                    },
                )
                .map_err(|_| invalid("sync_encrypt_failed"))?,
        );
        op.signature = STANDARD.encode(self.0.sign(&op.signing_bytes()?).to_bytes());
        Ok(op)
    }

    /// Bind enrollment proof to one browser-authenticated challenge and origin.
    pub fn enrollment_proof(
        &self,
        origin: &str,
        account: &str,
        device: &str,
        challenge: &str,
    ) -> Result<String> {
        identifier(device)?;
        let bytes = serde_json::to_vec(&(
            "noura.device.enroll",
            origin,
            account,
            device,
            self.public_key(),
            challenge,
        ))
        .map_err(|_| invalid("sync_serialize_failed"))?;
        Ok(STANDARD.encode(self.0.sign(&bytes).to_bytes()))
    }
}

impl EncryptedOperation {
    pub fn digest(&self) -> Result<String> {
        use sha2::{Digest, Sha256};
        self.validate()?;
        let mut hash = Sha256::new();
        hash.update(self.signing_bytes()?);
        hash.update(decode(&self.signature, 64, 64)?);
        Ok(format!("{:x}", hash.finalize()))
    }

    pub fn validate(&self) -> Result<()> {
        if !matches!(self.version, 1 | 2) || self.epoch == 0 || self.epoch > 9_007_199_254_740_991 {
            return Err(invalid("sync_invalid_envelope"));
        }
        match (self.version, self.generation.as_deref(), self.kind) {
            (1, None, None) => {}
            (2, Some(generation), Some(_)) => identifier(generation)?,
            _ => return Err(invalid("sync_invalid_envelope")),
        }
        super::transport::parse_cursor(&self.policy_revision)?;
        for id in [
            &self.workspace_id,
            &self.object_id,
            &self.device_id,
            &self.operation_id,
        ] {
            identifier(id)?;
        }
        decode(&self.nonce, 12, 12)?;
        decode(&self.ciphertext, 16, MAX_CIPHERTEXT)?;
        decode(&self.signature, 64, 64)?;
        Ok(())
    }

    /// Verify the sender against a previously trusted device public key.
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

    /// Authenticate all routing metadata before returning any plaintext.
    pub fn open(&self, key: &ObjectKey, trusted_public_key: &str) -> Result<Zeroizing<Vec<u8>>> {
        self.verify(trusted_public_key)?;
        let nonce: [u8; 12] = decode(&self.nonce, 12, 12)?
            .try_into()
            .map_err(|_| invalid("sync_invalid_nonce"))?;
        let ciphertext = decode(&self.ciphertext, 16, MAX_CIPHERTEXT)?;
        let aad = self.associated_data()?;
        Aes256Gcm::new((&*key.0).into())
            .decrypt(
                (&nonce).into(),
                Payload {
                    msg: &ciphertext,
                    aad: &aad,
                },
            )
            .map(Zeroizing::new)
            .map_err(|_| invalid("sync_decrypt_failed"))
    }

    fn associated_data(&self) -> Result<Vec<u8>> {
        let mut value = serde_json::to_value((
            "noura.sync.payload",
            self.version,
            &self.workspace_id,
            &self.object_id,
            &self.device_id,
            &self.operation_id,
            self.epoch,
            &self.policy_revision,
        ))
        .map_err(|_| invalid("sync_serialize_failed"))?;
        if self.version == 2 {
            let values = value
                .as_array_mut()
                .ok_or_else(|| invalid("sync_serialize_failed"))?;
            values.push(serde_json::json!(self.generation));
            values.push(serde_json::json!(self.kind));
        }
        serde_json::to_vec(&value).map_err(|_| invalid("sync_serialize_failed"))
    }

    fn signing_bytes(&self) -> Result<Vec<u8>> {
        let mut value = serde_json::to_value((
            "noura.sync.operation",
            self.version,
            &self.workspace_id,
            &self.object_id,
            &self.device_id,
            &self.operation_id,
            self.epoch,
            &self.policy_revision,
            &self.nonce,
            &self.ciphertext,
        ))
        .map_err(|_| invalid("sync_serialize_failed"))?;
        if self.version == 2 {
            let values = value
                .as_array_mut()
                .ok_or_else(|| invalid("sync_serialize_failed"))?;
            values.push(serde_json::json!(self.generation));
            values.push(serde_json::json!(self.kind));
        }
        serde_json::to_vec(&value).map_err(|_| invalid("sync_serialize_failed"))
    }
}

pub(crate) fn decode(value: &str, min: usize, max: usize) -> Result<Vec<u8>> {
    if value.len() > max.div_ceil(3) * 4 {
        return Err(invalid("sync_invalid_base64"));
    }
    let bytes = STANDARD
        .decode(value)
        .map_err(|_| invalid("sync_invalid_base64"))?;
    if bytes.len() < min || bytes.len() > max || STANDARD.encode(&bytes) != value {
        return Err(invalid("sync_invalid_base64"));
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encrypted_roundtrip_authenticates_metadata_and_key() {
        let signer = SigningIdentity::generate();
        let key = ObjectKey::generate();
        let op = signer
            .seal(
                &key,
                "workspace",
                "object",
                "device",
                1,
                b"private/path.md\nsecret",
            )
            .unwrap();
        assert_eq!(
            &**op.open(&key, &signer.public_key()).unwrap(),
            b"private/path.md\nsecret"
        );
        assert!(
            op.open(&ObjectKey::generate(), &signer.public_key())
                .is_err()
        );
        for field in [
            "workspaceId",
            "objectId",
            "deviceId",
            "operationId",
            "policyRevision",
            "nonce",
            "ciphertext",
        ] {
            let mut changed = serde_json::to_value(&op).unwrap();
            changed[field] = serde_json::json!("different");
            let changed: EncryptedOperation = serde_json::from_value(changed).unwrap();
            assert!(changed.open(&key, &signer.public_key()).is_err(), "{field}");
        }
        let serialized = serde_json::to_string(&op).unwrap();
        assert!(!serialized.contains("secret"));
        assert!(!serialized.contains("private/path"));
    }
}
