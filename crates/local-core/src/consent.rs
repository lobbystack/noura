use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Mutex,
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{CoreError, ErrorCategory, Result};

const CONSENT_STORE_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum AiConsentDataCategory {
    WorkspaceContent,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiConsentGrantInput {
    pub provider_id: String,
    pub policy_version: String,
    pub data_category: AiConsentDataCategory,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiConsentReadInput {
    pub provider_id: String,
    pub policy_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiConsentRevokeInput {
    pub provider_id: String,
    pub policy_version: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiConsentGrant {
    pub provider_id: String,
    pub policy_version: String,
    pub data_category: AiConsentDataCategory,
    pub granted_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AiConsentRevokeOutcome {
    pub provider_id: String,
    pub policy_version: String,
    pub revoked: bool,
    pub cancelled_operations: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct AiConsentKey {
    pub(crate) workspace_id: String,
    pub(crate) provider_id: String,
    pub(crate) policy_version: String,
    pub(crate) provider_fingerprint: String,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConsentFile {
    version: u32,
    grants: Vec<StoredAiConsentGrant>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredAiConsentGrant {
    workspace_id: String,
    provider_id: String,
    policy_version: String,
    #[serde(default)]
    provider_fingerprint: String,
    data_category: AiConsentDataCategory,
    granted_at: String,
}

pub(crate) struct AiConsentStore {
    path: PathBuf,
    grants: Mutex<HashMap<AiConsentKey, AiConsentGrant>>,
}

impl AiConsentStore {
    pub(crate) fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_owned();
        let grants = if path.exists() {
            let bytes =
                std::fs::read(&path).map_err(|_| consent_storage_error("ai_consent_load"))?;
            let file: ConsentFile =
                serde_json::from_slice(&bytes).map_err(|_| consent_parse_error())?;
            if !matches!(file.version, 1 | CONSENT_STORE_VERSION) {
                return Err(consent_parse_error());
            }
            // Version 1 grants were not tied to a concrete provider target. They
            // cannot authorize a later request, so retain no effective grants.
            if file.version == 1 {
                return Ok(Self {
                    path,
                    grants: Mutex::new(HashMap::new()),
                });
            }
            let mut grants = HashMap::new();
            for stored in file.grants {
                let StoredAiConsentGrant {
                    workspace_id,
                    provider_id,
                    policy_version,
                    provider_fingerprint,
                    data_category,
                    granted_at,
                } = stored;
                let key = AiConsentKey {
                    workspace_id,
                    provider_id: provider_id.clone(),
                    policy_version: policy_version.clone(),
                    provider_fingerprint,
                };
                validate_key(&key, "ai_consent_load")?;
                if grants
                    .insert(
                        key,
                        AiConsentGrant {
                            provider_id,
                            policy_version,
                            data_category,
                            granted_at,
                        },
                    )
                    .is_some()
                {
                    return Err(consent_parse_error());
                }
            }
            grants
        } else {
            HashMap::new()
        };
        Ok(Self {
            path,
            grants: Mutex::new(grants),
        })
    }

    pub(crate) fn read(&self, key: &AiConsentKey) -> Result<Option<AiConsentGrant>> {
        validate_key(key, "ai_consent_read")?;
        Ok(self
            .grants
            .lock()
            .map_err(|_| consent_storage_error("ai_consent_read"))?
            .get(key)
            .cloned())
    }

    pub(crate) fn grant(
        &self,
        key: AiConsentKey,
        input: AiConsentGrantInput,
    ) -> Result<AiConsentGrant> {
        validate_key(&key, "ai_consent_grant")?;
        let grant = AiConsentGrant {
            provider_id: input.provider_id,
            policy_version: input.policy_version,
            data_category: input.data_category,
            granted_at: crate::now_rfc3339(),
        };
        let mut grants = self
            .grants
            .lock()
            .map_err(|_| consent_storage_error("ai_consent_grant"))?;
        let mut next = grants.clone();
        next.insert(key, grant.clone());
        self.persist(&next, "ai_consent_grant")?;
        *grants = next;
        Ok(grant)
    }

    pub(crate) fn revoke_matching(
        &self,
        workspace_id: &str,
        provider_id: &str,
        policy_version: &str,
    ) -> Result<Vec<AiConsentKey>> {
        validate_input(
            workspace_id,
            provider_id,
            policy_version,
            "ai_consent_revoke",
        )?;
        let mut grants = self
            .grants
            .lock()
            .map_err(|_| consent_storage_error("ai_consent_revoke"))?;
        let keys = grants
            .keys()
            .filter(|key| {
                key.workspace_id == workspace_id
                    && key.provider_id == provider_id
                    && key.policy_version == policy_version
            })
            .cloned()
            .collect::<Vec<_>>();
        if keys.is_empty() {
            return Ok(keys);
        }
        let mut next = grants.clone();
        for key in &keys {
            next.remove(key);
        }
        self.persist(&next, "ai_consent_revoke")?;
        *grants = next;
        Ok(keys)
    }

    fn persist(
        &self,
        grants: &HashMap<AiConsentKey, AiConsentGrant>,
        operation: &str,
    ) -> Result<()> {
        let mut entries = grants
            .iter()
            .map(|(key, grant)| StoredAiConsentGrant {
                workspace_id: key.workspace_id.clone(),
                provider_id: grant.provider_id.clone(),
                policy_version: grant.policy_version.clone(),
                provider_fingerprint: key.provider_fingerprint.clone(),
                data_category: grant.data_category.clone(),
                granted_at: grant.granted_at.clone(),
            })
            .collect::<Vec<_>>();
        entries.sort_by(|left, right| {
            (
                &left.workspace_id,
                &left.provider_id,
                &left.policy_version,
                &left.provider_fingerprint,
            )
                .cmp(&(
                    &right.workspace_id,
                    &right.provider_id,
                    &right.policy_version,
                    &right.provider_fingerprint,
                ))
        });
        let bytes = serde_json::to_vec(&ConsentFile {
            version: CONSENT_STORE_VERSION,
            grants: entries,
        })
        .map_err(|_| consent_storage_error(operation))?;
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| consent_storage_error(operation))?;
        }
        crate::durable_settings::write(&self.path, &bytes)
            .map_err(|_| consent_storage_error(operation))?;
        Ok(())
    }
}

pub(crate) fn consent_key(
    workspace_id: &str,
    provider_id: String,
    policy_version: String,
    provider_fingerprint: String,
) -> AiConsentKey {
    AiConsentKey {
        workspace_id: workspace_id.into(),
        provider_id,
        policy_version,
        provider_fingerprint,
    }
}

fn validate_key(key: &AiConsentKey, operation: &str) -> Result<()> {
    validate_input(
        &key.workspace_id,
        &key.provider_id,
        &key.policy_version,
        operation,
    )?;
    if key.provider_fingerprint.len() != 64
        || !key
            .provider_fingerprint
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(CoreError::validation(
            "ai_consent_invalid",
            "The provider consent fingerprint is invalid",
            operation,
        ));
    }
    Ok(())
}

fn validate_input(
    workspace_id: &str,
    provider_id: &str,
    policy_version: &str,
    operation: &str,
) -> Result<()> {
    if !crate::valid_object_id(workspace_id, "workspace") {
        return Err(CoreError::validation(
            "invalid_workspace_id",
            "The workspace ID must be a lowercase stable workspace ID",
            operation,
        ));
    }
    for value in [provider_id, policy_version] {
        if value.is_empty()
            || value.len() > 128
            || value.chars().any(|character| character.is_control())
        {
            return Err(CoreError::validation(
                "ai_consent_invalid",
                "Provider IDs and policy versions must be non-empty plain text",
                operation,
            ));
        }
    }
    Ok(())
}

fn consent_parse_error() -> CoreError {
    CoreError::new(
        "consent_store_invalid",
        ErrorCategory::Parse,
        "Device-local AI consent settings are invalid",
        "ai_consent_load",
    )
}

fn consent_storage_error(operation: &str) -> CoreError {
    CoreError::new(
        "consent_store_error",
        ErrorCategory::Filesystem,
        "Device-local AI consent settings are unavailable",
        operation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn key() -> AiConsentKey {
        consent_key(
            "workspace_01j00000000000000000000000",
            "test-provider".into(),
            "2026-09".into(),
            "a".repeat(64),
        )
    }

    #[test]
    fn grants_are_atomically_persisted_and_reloaded() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("ai/consents.json");
        let store = AiConsentStore::open(&path).unwrap();
        let grant = store
            .grant(
                key(),
                AiConsentGrantInput {
                    provider_id: "test-provider".into(),
                    policy_version: "2026-09".into(),
                    data_category: AiConsentDataCategory::WorkspaceContent,
                },
            )
            .unwrap();
        drop(store);

        let reopened = AiConsentStore::open(path).unwrap();
        assert_eq!(reopened.read(&key()).unwrap(), Some(grant));
    }

    #[test]
    fn public_grants_redact_workspace_and_credential_references() {
        let grant = AiConsentGrant {
            provider_id: "test-provider".into(),
            policy_version: "2026-09".into(),
            data_category: AiConsentDataCategory::WorkspaceContent,
            granted_at: "2026-09-03T00:00:00Z".into(),
        };

        let value = serde_json::to_value(grant).unwrap();
        assert_eq!(
            value,
            serde_json::json!({
                "providerId": "test-provider",
                "policyVersion": "2026-09",
                "dataCategory": "workspace-content",
                "grantedAt": "2026-09-03T00:00:00Z"
            })
        );
    }

    #[test]
    fn malformed_persisted_consent_fails_without_returning_its_contents() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("consents.json");
        std::fs::write(&path, b"credential_secret_value").unwrap();

        let Err(error) = AiConsentStore::open(path) else {
            panic!("malformed consent data should be rejected");
        };
        assert_eq!(error.code, "consent_store_invalid");
        assert!(!error.message.contains("credential_secret_value"));
        assert!(error.details.is_none());
    }

    #[test]
    fn legacy_unfingerprinted_grants_are_discarded() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("consents.json");
        std::fs::write(
            &path,
            serde_json::json!({
                "version": 1,
                "grants": [{
                    "workspaceId": "workspace_01j00000000000000000000000",
                    "providerId": "test-provider",
                    "policyVersion": "2026-09",
                    "dataCategory": "workspace-content",
                    "grantedAt": "2026-09-03T00:00:00Z"
                }]
            })
            .to_string(),
        )
        .unwrap();

        let store = AiConsentStore::open(&path).unwrap();
        assert!(store.read(&key()).unwrap().is_none());
    }
}
