//! Web search and page reads for the AI: your Brave Search key, kept in
//! the operating system's credential store, and your web consent for each
//! workspace, kept on this device apart from AI provider consent. Both
//! stay outside the workspace. A search or a page read fails closed, with
//! nothing sent, until the key and the consent are both there.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use serde::{Deserialize, Serialize};

use crate::{
    BraveSearch, CoreError, CredentialStore, ErrorCategory, Result, SystemCredentials,
    WebAccessLimits, WebAccessProvider, WebAccessProviderConfig, WebAccessService, WebFetchInput,
    WebFetchResponse, WebSearchInput, WebSearchResponse,
};

/// The ID of the web search provider web consent names.
pub const WEB_SEARCH_PROVIDER: &str = "brave-search";

const SETTINGS_VERSION: u32 = 1;
const CONSENT_VERSION: u32 = 1;

/// Makes the provider that searches with your key and reads pages:
/// Brave Search over this device's network, or a stand-in in tests.
pub trait WebClient: Send + Sync {
    /// The provider that searches with `key`.
    fn provider(&self, key: &str) -> Arc<dyn WebAccessProvider>;
}

/// Brave Search over HTTPS on this device's network.
pub struct BraveClient;

impl WebClient for BraveClient {
    fn provider(&self, key: &str) -> Arc<dyn WebAccessProvider> {
        Arc::new(BraveSearch::new(key))
    }
}

/// Your consent for the AI to search the web and read pages in one
/// workspace, under one version of what Noura told you.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WebConsentGrant {
    pub workspace_id: String,
    /// The provider that receives searches: [`WEB_SEARCH_PROVIDER`].
    pub provider_id: String,
    pub policy_version: String,
    pub granted_at: String,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    version: u32,
    /// The credential store's reference to the Brave Search key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    brave_search_key: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConsentFile {
    version: u32,
    grants: Vec<WebConsentGrant>,
}

/// The Brave Search key and web consents on this device.
pub struct WebFoundation {
    settings_path: PathBuf,
    consent_path: PathBuf,
    /// The credential store's reference to the key, if there is one.
    key: Mutex<Option<String>>,
    grants: Mutex<Vec<WebConsentGrant>>,
    credentials: Arc<dyn CredentialStore>,
    limits: WebAccessLimits,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl WebFoundation {
    /// Open the web settings at `settings_path` and the consents at
    /// `consent_path`, with the key in the operating system's credential
    /// store.
    pub fn open(settings_path: impl AsRef<Path>, consent_path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(settings_path, consent_path, Arc::new(SystemCredentials))
    }

    /// [`WebFoundation::open`] with the key in `credentials`.
    pub fn open_with(
        settings_path: impl AsRef<Path>,
        consent_path: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self> {
        let settings_path = settings_path.as_ref().to_owned();
        let consent_path = consent_path.as_ref().to_owned();
        let settings: SettingsFile = read(&settings_path)?.unwrap_or_default();
        if settings.version > SETTINGS_VERSION
            || settings
                .brave_search_key
                .as_deref()
                .is_some_and(|reference| !valid_reference(reference))
        {
            return Err(invalid());
        }
        let grants = match read::<ConsentFile>(&consent_path)? {
            None => Vec::new(),
            Some(file) if file.version == CONSENT_VERSION => file.grants,
            Some(_) => return Err(invalid()),
        };
        for grant in &grants {
            validate(
                &grant.workspace_id,
                &grant.policy_version,
                "web_consent_load",
            )?;
        }
        Ok(Self {
            settings_path,
            consent_path,
            key: Mutex::new(settings.brave_search_key),
            grants: Mutex::new(grants),
            credentials,
            limits: WebAccessLimits::default(),
        })
    }

    /// Whether the Brave Search key is in the credential store.
    pub fn has_key(&self) -> bool {
        lock(&self.key).is_some()
    }

    /// Keep `secret` as the Brave Search key, in the credential store. The
    /// settings keep only a new reference to it, and the key it replaces
    /// leaves the store.
    pub fn set_key(&self, secret: &str) -> Result<()> {
        let secret = secret.trim();
        if secret.is_empty() {
            return Err(CoreError::validation(
                "credential_empty",
                "A credential value is required",
                "web_key_set",
            ));
        }
        let mut key = lock(&self.key);
        let reference = format!("credential_{}", uuid::Uuid::new_v4());
        self.credentials.set(&reference, secret)?;
        if let Err(error) = self.write_settings(Some(&reference)) {
            let _ = self.credentials.delete(&reference);
            return Err(error);
        }
        if let Some(previous) = key.replace(reference) {
            let _ = self.credentials.delete(&previous);
        }
        Ok(())
    }

    /// Forget the Brave Search key: its reference leaves the settings and
    /// the key leaves the credential store.
    pub fn clear_key(&self) -> Result<()> {
        let mut key = lock(&self.key);
        let Some(reference) = key.clone() else {
            return Ok(());
        };
        self.write_settings(None)?;
        *key = None;
        self.credentials.delete(&reference)
    }

    /// Your web consent for the workspace `workspace_id` under
    /// `policy_version`, if you gave it.
    pub fn consent(
        &self,
        workspace_id: &str,
        policy_version: &str,
    ) -> Result<Option<WebConsentGrant>> {
        validate(workspace_id, policy_version, "web_consent_read")?;
        Ok(lock(&self.grants)
            .iter()
            .find(|grant| {
                grant.workspace_id == workspace_id && grant.policy_version == policy_version
            })
            .cloned())
    }

    /// Consent to the AI searching the web and reading pages in the
    /// workspace `workspace_id`, under `policy_version`.
    pub fn grant(&self, workspace_id: &str, policy_version: &str) -> Result<WebConsentGrant> {
        validate(workspace_id, policy_version, "web_consent_grant")?;
        let grant = WebConsentGrant {
            workspace_id: workspace_id.to_owned(),
            provider_id: WEB_SEARCH_PROVIDER.to_owned(),
            policy_version: policy_version.to_owned(),
            granted_at: crate::now_rfc3339(),
        };
        let mut grants = lock(&self.grants);
        let mut next: Vec<WebConsentGrant> = grants
            .iter()
            .filter(|known| known.workspace_id != workspace_id)
            .cloned()
            .collect();
        next.push(grant.clone());
        self.write_consents(&next)?;
        *grants = next;
        Ok(grant)
    }

    /// Withdraw your web consent for the workspace `workspace_id`, under
    /// every policy version. Returns whether there was one.
    pub fn revoke(&self, workspace_id: &str) -> Result<bool> {
        let mut grants = lock(&self.grants);
        let next: Vec<WebConsentGrant> = grants
            .iter()
            .filter(|known| known.workspace_id != workspace_id)
            .cloned()
            .collect();
        if next.len() == grants.len() {
            return Ok(false);
        }
        self.write_consents(&next)?;
        *grants = next;
        Ok(true)
    }

    /// Search the web for the workspace `workspace_id` through `client`.
    pub async fn search(
        &self,
        workspace_id: &str,
        policy_version: &str,
        client: &dyn WebClient,
        input: WebSearchInput,
    ) -> Result<WebSearchResponse> {
        self.access(workspace_id, policy_version, client, "web_search")?
            .search(input)
            .await
    }

    /// Read a page for the workspace `workspace_id` through `client`.
    pub async fn fetch(
        &self,
        workspace_id: &str,
        policy_version: &str,
        client: &dyn WebClient,
        input: WebFetchInput,
    ) -> Result<WebFetchResponse> {
        self.access(workspace_id, policy_version, client, "web_fetch")?
            .fetch(input)
            .await
    }

    /// The web access boundary with a provider for your key, once the key
    /// and your consent for the workspace are both there. Without either,
    /// nothing reaches `client`.
    fn access(
        &self,
        workspace_id: &str,
        policy_version: &str,
        client: &dyn WebClient,
        operation: &str,
    ) -> Result<WebAccessService> {
        let reference = lock(&self.key).clone().ok_or_else(|| {
            CoreError::new(
                "web_search_key_missing",
                ErrorCategory::Credential,
                "Web search needs a Brave Search key",
                operation,
            )
        })?;
        if self.consent(workspace_id, policy_version)?.is_none() {
            return Err(CoreError::new(
                "web_consent_required",
                ErrorCategory::Permission,
                "Web search needs your consent for this workspace",
                operation,
            ));
        }
        let key = zeroize::Zeroizing::new(self.credentials.get(&reference)?);
        let mut service = WebAccessService::new(self.limits)?;
        service.configure(
            WebAccessProviderConfig {
                id: WEB_SEARCH_PROVIDER.to_owned(),
                user_consent_granted: true,
            },
            client.provider(&key),
        )?;
        Ok(service)
    }

    fn write_settings(&self, reference: Option<&str>) -> Result<()> {
        write(
            &self.settings_path,
            &SettingsFile {
                version: SETTINGS_VERSION,
                brave_search_key: reference.map(str::to_owned),
            },
        )
    }

    fn write_consents(&self, grants: &[WebConsentGrant]) -> Result<()> {
        let mut grants = grants.to_vec();
        grants.sort_by(|left, right| left.workspace_id.cmp(&right.workspace_id));
        write(
            &self.consent_path,
            &ConsentFile {
                version: CONSENT_VERSION,
                grants,
            },
        )
    }
}

/// The value the file at `path` holds, or none when there is no file.
fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(storage_error("web_settings_load")),
    };
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|_| invalid())
}

fn write(path: &Path, value: &impl Serialize) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|_| storage_error("web_settings_save"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|_| storage_error("web_settings_save"))?;
    }
    crate::durable_settings::write(path, &bytes).map_err(|_| storage_error("web_settings_save"))
}

fn valid_reference(reference: &str) -> bool {
    reference
        .strip_prefix("credential_")
        .is_some_and(|rest| uuid::Uuid::parse_str(rest).is_ok())
}

fn validate(workspace_id: &str, policy_version: &str, operation: &str) -> Result<()> {
    if !crate::valid_object_id(workspace_id, "workspace") {
        return Err(CoreError::validation(
            "invalid_workspace_id",
            "The workspace ID must be a lowercase stable workspace ID",
            operation,
        ));
    }
    if policy_version.is_empty()
        || policy_version.len() > 128
        || policy_version.chars().any(char::is_control)
    {
        return Err(CoreError::validation(
            "web_consent_invalid",
            "Policy versions must be non-empty plain text",
            operation,
        ));
    }
    Ok(())
}

fn invalid() -> CoreError {
    CoreError::new(
        "web_settings_invalid",
        ErrorCategory::Parse,
        "Device-local web search settings are invalid",
        "web_settings_load",
    )
}

fn storage_error(operation: &str) -> CoreError {
    CoreError::new(
        "web_settings_error",
        ErrorCategory::Filesystem,
        "Device-local web search settings are unavailable",
        operation,
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::tempdir;

    use super::*;
    use crate::{
        MemoryCredentials, WebAccessFuture, WebFetchRequest, WebSearchRequest, WebSearchResult,
    };

    const A: &str = "workspace_01jbcdefghjkmnpqrstvwxyz0a";
    const B: &str = "workspace_01jbcdefghjkmnpqrstvwxyz0b";
    const POLICY: &str = "2026-10";
    const KEY: &str = "BSA-secret-0123456789";

    /// A web provider that records the key it was made with and each
    /// search, and finds one result.
    #[derive(Default)]
    struct FakeWeb {
        keys: Mutex<Vec<String>>,
        searches: Mutex<Vec<String>>,
    }

    struct Provider(Arc<FakeWeb>);

    impl WebClient for Arc<FakeWeb> {
        fn provider(&self, key: &str) -> Arc<dyn WebAccessProvider> {
            self.keys.lock().unwrap().push(key.to_owned());
            Arc::new(Provider(self.clone()))
        }
    }

    impl WebAccessProvider for Provider {
        fn search(&self, request: WebSearchRequest) -> WebAccessFuture<'_, WebSearchResponse> {
            self.0.searches.lock().unwrap().push(request.query);
            Box::pin(async {
                Ok(WebSearchResponse {
                    results: vec![WebSearchResult {
                        title: "Tomatoes".into(),
                        url: "https://garden.example.com/".into(),
                        snippet: "Grow them.".into(),
                    }],
                    response_bytes: 64,
                })
            })
        }

        fn fetch(&self, _request: WebFetchRequest) -> WebAccessFuture<'_, WebFetchResponse> {
            self.0.searches.lock().unwrap().push("fetch".into());
            Box::pin(async { unreachable!("no test reads a page") })
        }
    }

    fn open(root: &Path, credentials: &Arc<MemoryCredentials>) -> WebFoundation {
        WebFoundation::open_with(
            root.join("ai/web.json"),
            root.join("ai/web-consents.json"),
            credentials.clone(),
        )
        .unwrap()
    }

    fn query() -> WebSearchInput {
        WebSearchInput {
            query: "tomatoes".into(),
            max_results: None,
        }
    }

    #[test]
    fn the_key_goes_to_the_credential_store_and_the_settings_keep_a_reference() {
        let root = tempdir().unwrap();
        let credentials = Arc::new(MemoryCredentials::new());
        let web = open(root.path(), &credentials);
        assert!(!web.has_key());

        web.set_key("first-key").unwrap();
        web.set_key(&format!("  {KEY} ")).unwrap();

        assert!(web.has_key());
        assert_eq!(credentials.secrets(), [KEY.to_owned()]);
        let settings = std::fs::read_to_string(root.path().join("ai/web.json")).unwrap();
        assert!(settings.contains("credential_"), "{settings}");
        assert!(!settings.contains(KEY) && !settings.contains("first-key"));
        drop(web);

        let web = open(root.path(), &credentials);
        assert!(web.has_key());
        web.clear_key().unwrap();
        assert!(!web.has_key());
        assert!(credentials.secrets().is_empty());
        assert!(!open(root.path(), &credentials).has_key());
        assert_eq!(web.set_key("   ").unwrap_err().code, "credential_empty");
    }

    #[test]
    fn consent_holds_for_one_workspace_on_this_device_apart_from_provider_consent() {
        let root = tempdir().unwrap();
        let credentials = Arc::new(MemoryCredentials::new());
        let web = open(root.path(), &credentials);

        let grant = web.grant(A, POLICY).unwrap();

        assert_eq!(grant.provider_id, WEB_SEARCH_PROVIDER);
        assert_eq!(web.consent(A, POLICY).unwrap(), Some(grant.clone()));
        assert_eq!(web.consent(B, POLICY).unwrap(), None);
        assert_eq!(web.consent(A, "2027-01").unwrap(), None);
        assert!(!root.path().join("ai/consents.json").exists());
        drop(web);

        let web = open(root.path(), &credentials);
        assert_eq!(web.consent(A, POLICY).unwrap(), Some(grant));
        web.grant(B, POLICY).unwrap();
        assert!(web.revoke(A).unwrap());
        assert!(!web.revoke(A).unwrap());
        assert_eq!(web.consent(A, POLICY).unwrap(), None);
        assert!(web.consent(B, POLICY).unwrap().is_some());
        drop(web);
        assert_eq!(
            open(root.path(), &credentials).consent(A, POLICY).unwrap(),
            None
        );
    }

    #[tokio::test]
    async fn without_the_key_or_consent_nothing_reaches_the_web() {
        let root = tempdir().unwrap();
        let credentials = Arc::new(MemoryCredentials::new());
        let web = open(root.path(), &credentials);
        let fake = Arc::new(FakeWeb::default());

        web.grant(A, POLICY).unwrap();
        let error = web.search(A, POLICY, &fake, query()).await.unwrap_err();
        assert_eq!(error.code, "web_search_key_missing");

        web.revoke(A).unwrap();
        web.set_key(KEY).unwrap();
        let error = web.search(A, POLICY, &fake, query()).await.unwrap_err();
        assert_eq!(error.code, "web_consent_required");
        let error = web
            .fetch(
                A,
                POLICY,
                &fake,
                WebFetchInput {
                    url: "https://garden.example.com/".into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "web_consent_required");
        assert!(fake.keys.lock().unwrap().is_empty());
        assert!(fake.searches.lock().unwrap().is_empty());

        web.grant(A, POLICY).unwrap();
        let found = web.search(A, POLICY, &fake, query()).await.unwrap();
        assert_eq!(found.results[0].title, "Tomatoes");
        assert_eq!(*fake.keys.lock().unwrap(), [KEY.to_owned()]);
        assert_eq!(*fake.searches.lock().unwrap(), ["tomatoes".to_owned()]);

        // Consent in one workspace doesn't reach another.
        let error = web.search(B, POLICY, &fake, query()).await.unwrap_err();
        assert_eq!(error.code, "web_consent_required");
        assert_eq!(fake.searches.lock().unwrap().len(), 1);
    }

    #[test]
    fn malformed_settings_fail_without_their_contents() {
        for (file, text) in [
            ("ai/web.json", "not json BSA-secret"),
            (
                "ai/web.json",
                r#"{"version":1,"braveSearchKey":"BSA-secret"}"#,
            ),
            (
                "ai/web-consents.json",
                r#"{"version":1,"grants":[{"workspaceId":"../x","providerId":"brave-search","policyVersion":"1","grantedAt":"t"}]}"#,
            ),
            ("ai/web-consents.json", r#"{"version":9,"grants":[]}"#),
        ] {
            let root = tempdir().unwrap();
            std::fs::create_dir_all(root.path().join("ai")).unwrap();
            std::fs::write(root.path().join(file), text).unwrap();

            let Err(error) = WebFoundation::open_with(
                root.path().join("ai/web.json"),
                root.path().join("ai/web-consents.json"),
                Arc::new(MemoryCredentials::new()),
            ) else {
                panic!("{text} should be refused");
            };

            assert!(
                matches!(
                    error.code.as_str(),
                    "web_settings_invalid" | "invalid_workspace_id"
                ),
                "{}",
                error.code
            );
            assert!(!error.message.contains("BSA-secret"));
            assert!(error.details.is_none());
        }
    }
}
