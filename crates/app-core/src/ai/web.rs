//! Web search for the AI: your Brave Search key in the operating system's
//! credential store, your web consent for each workspace, and the web
//! searches and page reads that plugins holding `ai` ask for.
//!
//! Web consent is its own choice, apart from a provider's consent: Noura
//! asks for it in a prompt it draws itself, keeps it on this device, and
//! asks nothing before each call. A search or a page read fails closed,
//! with nothing sent, until the key and the consent for the open
//! workspace are both there. The requests go out from here through
//! `local-core`'s web access boundary, so the plugin opens no connection
//! of its own, and nothing a plugin gets back holds the key or what Brave
//! Search or a site answered.

use std::{
    future::Future,
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use local_core::{
    BraveClient, CoreError, CredentialStore, WebClient, WebFetchInput, WebFoundation,
    WebSearchInput,
};
use plugin_host::{AiFailure, AiWebPage, AiWebResult};

use super::{Ai, lock};
use crate::AppError;

/// The version of what Noura tells you before the AI searches the web.
/// Consent holds for one version, so a new one asks again.
pub const WEB_POLICY_VERSION: &str = "2026-10";

/// The name of the web search provider, as Noura shows it.
pub const WEB_SEARCH_PROVIDER_NAME: &str = "Brave Search";

/// Where the web search settings and the web consents live, under the app
/// data folder, apart from the AI provider consents.
const SETTINGS_FILE: &str = "ai/web.json";
const CONSENTS_FILE: &str = "ai/web-consents.json";

/// Web search as Settings shows it. It holds whether the key is saved,
/// never the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchRow {
    /// Whether the Brave Search key is in the credential store.
    pub has_key: bool,
    /// Whether you allowed web search for the open workspace.
    pub consented: bool,
}

/// The question Noura asks before the AI may search the web and read
/// pages in the open workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebConsentPrompt {
    /// The provider that receives the searches: Brave Search.
    pub provider_name: String,
    /// The open workspace's name.
    pub workspace: String,
}

pub(super) struct Web {
    /// The key and consents, or why they can't be read.
    store: Result<WebFoundation, AppError>,
    /// Makes the provider: Brave Search, or a fake in tests.
    client: Mutex<Arc<dyn WebClient>>,
    /// Whether the prompt is open.
    asking: AtomicBool,
}

impl Web {
    pub(super) fn new(app_data: &Path, credentials: Arc<dyn CredentialStore>) -> Self {
        let store = WebFoundation::open_with(
            app_data.join(SETTINGS_FILE),
            app_data.join(CONSENTS_FILE),
            credentials,
        )
        .map_err(|error| {
            tracing::warn!(code = %error.code, "the web search settings can't be read");
            AppError::new(
                "web_settings_unreadable",
                "Noura can't read its web search settings on this computer, so it left them as \
                 they were.",
            )
        });
        Self {
            store,
            client: Mutex::new(Arc::new(BraveClient)),
            asking: AtomicBool::new(false),
        }
    }

    fn store(&self) -> Result<&WebFoundation, AppError> {
        self.store.as_ref().map_err(Clone::clone)
    }

    /// Close the prompt without an answer, as when its workspace closes.
    pub(super) fn close_prompt(&self) {
        self.asking.store(false, Ordering::SeqCst);
    }
}

impl Ai {
    /// Why Noura can't read its web search settings, if it can't.
    pub(crate) fn web_problem(&self) -> Option<String> {
        self.web
            .store
            .as_ref()
            .err()
            .map(|error| error.message.clone())
    }

    /// Web search as Settings shows it.
    pub(crate) fn web_row(&self) -> WebSearchRow {
        let Ok(store) = self.web.store() else {
            return WebSearchRow {
                has_key: false,
                consented: false,
            };
        };
        WebSearchRow {
            has_key: store.has_key(),
            consented: self.web_consented(store),
        }
    }

    /// Keep `key` as the Brave Search key, in the credential store.
    pub(crate) fn set_web_key(&self, key: &str) -> Result<(), AppError> {
        self.web.store()?.set_key(key).map_err(web_settings_error)?;
        self.changed();
        Ok(())
    }

    /// Forget the Brave Search key. The next search fails until you add
    /// one again.
    pub(crate) fn remove_web_key(&self) -> Result<(), AppError> {
        self.web.store()?.clear_key().map_err(web_settings_error)?;
        self.changed();
        Ok(())
    }

    /// Open the prompt that asks whether the AI may search the web in the
    /// open workspace.
    pub(crate) fn ask_web_consent(&self) -> Result<(), AppError> {
        self.workspace().ok_or_else(super::no_workspace)?;
        self.web.store()?;
        self.web.asking.store(true, Ordering::SeqCst);
        self.changed();
        Ok(())
    }

    /// The web consent question waiting for your answer, if one is.
    pub(crate) fn web_prompt(&self) -> Option<WebConsentPrompt> {
        if !self.web.asking.load(Ordering::SeqCst) {
            return None;
        }
        let workspace = self.workspace()?;
        Some(WebConsentPrompt {
            provider_name: WEB_SEARCH_PROVIDER_NAME.to_owned(),
            workspace: workspace.name(),
        })
    }

    /// Answer the web consent prompt. Allowing it stores your consent for
    /// the open workspace on this device; saying no stores nothing.
    pub(crate) fn answer_web_prompt(&self, allow: bool) -> Result<(), AppError> {
        if !self.web.asking.swap(false, Ordering::SeqCst) {
            return Ok(());
        }
        let answered = if allow {
            let workspace = self.workspace().ok_or_else(super::no_workspace)?;
            self.web
                .store()?
                .grant(workspace.id(), WEB_POLICY_VERSION)
                .map(|_| ())
                .map_err(web_settings_error)
        } else {
            Ok(())
        };
        self.changed();
        answered
    }

    /// Withdraw your web consent for the open workspace. The next web
    /// search or page read fails, and chats go on without them.
    pub(crate) fn revoke_web(&self) -> Result<(), AppError> {
        let workspace = self.workspace().ok_or_else(super::no_workspace)?;
        self.web
            .store()?
            .revoke(workspace.id())
            .map_err(web_settings_error)?;
        self.changed();
        Ok(())
    }

    /// Search the web and read pages through `client` from the next call
    /// on.
    pub(crate) fn set_web_client(&self, client: Arc<dyn WebClient>) {
        *lock(&self.web.client) = client;
    }

    /// Whether you consented to web search for the open workspace.
    fn web_consented(&self, store: &WebFoundation) -> bool {
        self.workspace().is_some_and(|workspace| {
            store
                .consent(workspace.id(), WEB_POLICY_VERSION)
                .ok()
                .flatten()
                .is_some()
        })
    }

    // What plugins reach.

    /// Whether the model may search the web and read pages in the open
    /// workspace.
    pub(super) fn web_ready(&self) -> bool {
        self.web
            .store()
            .is_ok_and(|store| store.has_key() && self.web_consented(store))
    }

    pub(super) fn web_search(
        &self,
        query: &str,
        max_results: Option<u32>,
    ) -> Result<Vec<AiWebResult>, AiFailure> {
        let workspace = self.workspace().ok_or_else(no_workspace)?;
        let store = self
            .web
            .store()
            .map_err(|_| web_failure_code("web_settings_invalid"))?;
        let client = lock(&self.web.client).clone();
        let input = WebSearchInput {
            query: query.to_owned(),
            max_results: max_results.map(|count| usize::try_from(count).unwrap_or(usize::MAX)),
        };
        let found = self
            .block_on(store.search(workspace.id(), WEB_POLICY_VERSION, client.as_ref(), input))
            .map_err(|error| web_failure(&error))?;
        Ok(found
            .results
            .into_iter()
            .map(|result| AiWebResult {
                title: result.title,
                url: result.url,
                snippet: result.snippet,
            })
            .collect())
    }

    pub(super) fn web_fetch(&self, url: &str) -> Result<AiWebPage, AiFailure> {
        let workspace = self.workspace().ok_or_else(no_workspace)?;
        let store = self
            .web
            .store()
            .map_err(|_| web_failure_code("web_settings_invalid"))?;
        let client = lock(&self.web.client).clone();
        let input = WebFetchInput {
            url: url.to_owned(),
        };
        let page = self
            .block_on(store.fetch(workspace.id(), WEB_POLICY_VERSION, client.as_ref(), input))
            .map_err(|error| web_failure(&error))?;
        Ok(AiWebPage {
            url: page.url,
            content_type: page.content_type.unwrap_or_default(),
            text: page.content,
            truncated: page.truncated,
        })
    }

    /// Run `future` on the service's runtime and wait for it, from a
    /// thread of its own when the caller is inside a runtime already.
    fn block_on<F>(&self, future: F) -> F::Output
    where
        F: Future + Send,
        F::Output: Send,
    {
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| {
                scope
                    .spawn(|| self.runtime.block_on(future))
                    .join()
                    .expect("a web call's thread finishes")
            })
        } else {
            self.runtime.block_on(future)
        }
    }
}

fn no_workspace() -> AiFailure {
    web_failure_code("workspace_not_open")
}

fn web_failure_code(code: &str) -> AiFailure {
    AiFailure {
        code: code.to_owned(),
        message: web_message(code, None).to_owned(),
    }
}

/// A web failure as a plugin and the chat see it: a code and Noura's own
/// words, never the key or what Brave Search or a site answered.
fn web_failure(error: &CoreError) -> AiFailure {
    tracing::warn!(code = %error.code, operation = %error.operation, "a web call failed");
    AiFailure {
        code: super::error_code(error),
        message: web_message(&error.code, Some(error)).to_owned(),
    }
}

fn web_message<'a>(code: &str, error: Option<&'a CoreError>) -> &'a str {
    match code {
        "workspace_not_open" => "Open a workspace first.",
        "web_search_key_missing" => {
            "Add a Brave Search key in Settings, under Web search, to search the web."
        }
        "web_consent_required" => {
            "Allow web search for this workspace in Settings, under Web search."
        }
        "web_search_key_rejected" => {
            "Brave Search didn't accept the key. Check it in Settings, under Web search."
        }
        "web_search_rate_limited" => {
            "Brave Search's limit for your key was reached. Try again in a moment."
        }
        "web_search_query_empty" => "The search needs a query.",
        "web_search_query_too_large" => "The search query is too long.",
        "web_search_result_limit_invalid" => "Ask for 1 to 10 results.",
        "web_url_not_https" => "Noura reads only pages with an HTTPS address.",
        "web_url_invalid" => "The address isn't a full HTTPS address.",
        "web_url_unsafe_host" => {
            "Noura doesn't read pages on this computer or on a local or private network."
        }
        "web_redirect_limit_exceeded" | "web_redirect_invalid" => {
            "The page sent Noura to too many other addresses."
        }
        "web_content_type_unsupported" => "Noura reads only HTML and plain-text pages.",
        "web_response_limit_exceeded" => "The page is larger than Noura reads.",
        "web_page_unavailable" | "web_page_unreadable" => match error {
            // Noura's own words, which hold at most the site's status.
            Some(error) => error.message.as_str(),
            None => "Noura couldn't read the page.",
        },
        "web_request_timeout" => "The site took too long to answer. Try again.",
        "web_host_unresolved" => "Noura couldn't find the site.",
        "web_request_failed" => "Noura couldn't reach the site.",
        "credential_store_error" => {
            "Noura couldn't read the Brave Search key from this computer's credential store."
        }
        "web_settings_invalid" | "web_settings_error" => {
            "Noura can't read its web search settings on this computer."
        }
        "web_search_failed" | "web_search_response_invalid" | "web_host_not_allowed" => {
            "Brave Search couldn't answer. Try again."
        }
        _ => "Noura couldn't search the web.",
    }
}

fn web_settings_error(error: CoreError) -> AppError {
    tracing::warn!(code = %error.code, "a web search setting wasn't saved");
    let message = match error.code.as_str() {
        "credential_store_error" => {
            "Noura couldn't reach this computer's credential store for \
                                     the key."
        }
        "credential_empty" => "Enter the key.",
        _ => "Noura couldn't save the web search settings on this computer.",
    };
    AppError::new(&error.code, message)
}
