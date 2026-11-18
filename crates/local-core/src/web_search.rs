//! Web search and page reads for the AI: your Brave Search key, kept in
//! the operating system's credential store, outside the workspace. A
//! search or a page read fails closed, with nothing sent, until the key is
//! there. Adding the key is your say-so, so nothing asks per workspace.

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

/// The ID of the web search provider.
pub const WEB_SEARCH_PROVIDER: &str = "brave-search";

const SETTINGS_VERSION: u32 = 1;

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

#[derive(Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SettingsFile {
    version: u32,
    /// The credential store's reference to the Brave Search key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    brave_search_key: Option<String>,
}

/// The Brave Search key on this device.
pub struct WebFoundation {
    settings_path: PathBuf,
    /// The credential store's reference to the key, if there is one.
    key: Mutex<Option<String>>,
    credentials: Arc<dyn CredentialStore>,
    limits: WebAccessLimits,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl WebFoundation {
    /// Open the web settings at `settings_path`, with the key in the
    /// operating system's credential store.
    pub fn open(settings_path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with(settings_path, Arc::new(SystemCredentials))
    }

    /// [`WebFoundation::open`] with the key in `credentials`.
    ///
    /// A file Noura can't read moves aside, to the same name with
    /// `.unreadable` added, and Noura starts again without it: with no
    /// key, so web calls fail closed until you add it again. A file a newer version of Noura wrote stays where it
    /// is, and opening fails.
    pub fn open_with(
        settings_path: impl AsRef<Path>,
        credentials: Arc<dyn CredentialStore>,
    ) -> Result<Self> {
        let settings_path = settings_path.as_ref().to_owned();
        let settings = match read::<SettingsFile>(&settings_path)? {
            Read::Missing => SettingsFile::default(),
            Read::Value(file) if file.version > SETTINGS_VERSION => return Err(invalid()),
            Read::Value(file) if file.brave_search_key.as_deref().is_none_or(valid_reference) => {
                file
            }
            Read::Value(_) | Read::Unreadable => {
                set_aside(&settings_path)?;
                SettingsFile::default()
            }
        };
        Ok(Self {
            settings_path,
            key: Mutex::new(settings.brave_search_key),
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

    /// Search the web through `client`.
    pub async fn search(
        &self,
        client: &dyn WebClient,
        input: WebSearchInput,
    ) -> Result<WebSearchResponse> {
        self.access(client, "web_search")?.search(input).await
    }

    /// Read a page through `client`.
    pub async fn fetch(
        &self,
        client: &dyn WebClient,
        input: WebFetchInput,
    ) -> Result<WebFetchResponse> {
        self.access(client, "web_fetch")?.fetch(input).await
    }

    /// Fail as a web call would without the key, and succeed once it's
    /// there.
    pub fn check_ready(&self) -> Result<()> {
        self.key_reference("web_access").map(|_| ())
    }

    /// The web access boundary with a provider for your key, once the key
    /// is there. Without it, nothing reaches `client`.
    fn access(&self, client: &dyn WebClient, operation: &str) -> Result<WebAccessService> {
        let reference = self.key_reference(operation)?;
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

    /// The credential store's reference to the key, once it's there.
    fn key_reference(&self, operation: &str) -> Result<String> {
        let reference = lock(&self.key).clone().ok_or_else(|| {
            CoreError::new(
                "web_search_key_missing",
                ErrorCategory::Credential,
                "Web search needs a Brave Search key",
                operation,
            )
        })?;
        Ok(reference)
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
}

/// What reading a settings file found.
enum Read<T> {
    /// There is no file.
    Missing,
    Value(T),
    /// The file holds something that isn't the value.
    Unreadable,
}

/// What the file at `path` holds.
fn read<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Read<T>> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Read::Missing),
        Err(_) => return Err(storage_error("web_settings_load")),
    };
    Ok(serde_json::from_slice(&bytes).map_or(Read::Unreadable, Read::Value))
}

/// Move the unreadable file at `path` aside, to the same name with
/// `.unreadable` added, in place of an earlier one.
fn set_aside(path: &Path) -> Result<()> {
    let mut aside = path.as_os_str().to_owned();
    aside.push(".unreadable");
    std::fs::rename(path, &aside).map_err(|_| storage_error("web_settings_load"))?;
    let file = path.file_name().unwrap_or_default().to_string_lossy();
    tracing::warn!(%file, "Noura set aside web search settings it couldn't read");
    Ok(())
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
        WebFoundation::open_with(root.join("ai/web.json"), credentials.clone()).unwrap()
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

    #[tokio::test]
    async fn without_the_key_nothing_reaches_the_web() {
        let root = tempdir().unwrap();
        let credentials = Arc::new(MemoryCredentials::new());
        let web = open(root.path(), &credentials);
        let fake = Arc::new(FakeWeb::default());

        let error = web.search(&fake, query()).await.unwrap_err();
        assert_eq!(error.code, "web_search_key_missing");
        assert_eq!(
            web.check_ready().unwrap_err().code,
            "web_search_key_missing"
        );
        let error = web
            .fetch(
                &fake,
                WebFetchInput {
                    url: "https://garden.example.com/".into(),
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.code, "web_search_key_missing");
        assert!(fake.keys.lock().unwrap().is_empty());
        assert!(fake.searches.lock().unwrap().is_empty());

        web.set_key(KEY).unwrap();
        web.check_ready().unwrap();
        let found = web.search(&fake, query()).await.unwrap();
        assert_eq!(found.results[0].title, "Tomatoes");
        assert_eq!(*fake.keys.lock().unwrap(), [KEY.to_owned()]);
        assert_eq!(*fake.searches.lock().unwrap(), ["tomatoes".to_owned()]);
    }

    /// A file Noura can't read moves aside and Noura starts again without
    /// it, so web calls fail closed until you set them up again. A file
    /// from a newer version stays, and opening fails without its contents.
    #[test]
    fn unreadable_settings_move_aside_and_newer_ones_stay() {
        for (file, text, opens) in [
            ("ai/web.json", "not json BSA-secret", true),
            (
                "ai/web.json",
                r#"{"version":1,"braveSearchKey":"BSA-secret"}"#,
                true,
            ),
            ("ai/web.json", r#"{"version":9}"#, false),
        ] {
            let root = tempdir().unwrap();
            std::fs::create_dir_all(root.path().join("ai")).unwrap();
            let path = root.path().join(file);
            std::fs::write(&path, text).unwrap();
            let aside = root.path().join(format!("{file}.unreadable"));

            let opened = WebFoundation::open_with(
                root.path().join("ai/web.json"),
                Arc::new(MemoryCredentials::new()),
            );

            match opened {
                Ok(foundation) => {
                    assert!(opens, "{text} should stay where it is");
                    assert!(!path.exists(), "{text}");
                    assert_eq!(std::fs::read_to_string(&aside).unwrap(), text);
                    assert!(!foundation.has_key(), "{text}");
                    // The next change writes a fresh file.
                    foundation.set_key("BSA-new").unwrap();
                    let reopened = WebFoundation::open_with(
                        root.path().join("ai/web.json"),
                        Arc::new(MemoryCredentials::new()),
                    )
                    .unwrap();
                    assert!(reopened.has_key());
                }
                Err(error) => {
                    assert!(!opens, "{text} should move aside: {}", error.code);
                    assert_eq!(error.code, "web_settings_invalid");
                    assert!(!error.message.contains("BSA-secret"));
                    assert!(error.details.is_none());
                    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
                    assert!(!aside.exists());
                }
            }
        }
    }
}
