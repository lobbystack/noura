//! Web search for the AI: your Brave Search key in the operating system's
//! credential store, and the web searches and page reads that plugins
//! holding `ai` ask for.
//!
//! Adding the key is your say-so, so Noura asks nothing per workspace or
//! per call. A search or a page read fails closed, with nothing sent,
//! until the key is there. Even then, a plugin's search or page read
//! goes out only to answer a call its model made: a run the plugin sent
//! completed asking for `web_search` with that query, or `web_fetch`
//! with that address. The requests go out from here through
//! `local-core`'s web access boundary, so the plugin opens no connection
//! of its own, and nothing a plugin gets back holds the key or what Brave
//! Search or a site answered.

use std::{
    future::Future,
    path::Path,
    sync::{Arc, Mutex},
};

use local_core::{
    BraveClient, CoreError, CredentialStore, WebClient, WebFetchInput, WebFoundation,
    WebSearchInput,
};
use plugin_host::{AiFailure, AiRunStatus, AiWebPage, AiWebResult};

use super::{Ai, lock};
use crate::AppError;

/// The tools whose calls a plugin's web searches and page reads answer.
const SEARCH_TOOL: &str = "web_search";
const FETCH_TOOL: &str = "web_fetch";

/// The name of the web search provider, as Noura shows it.
pub const WEB_SEARCH_PROVIDER_NAME: &str = "Brave Search";

/// Where the web search settings live, under the app data folder.
const SETTINGS_FILE: &str = "ai/web.json";

/// Web search as Settings shows it. It holds whether the key is saved,
/// never the key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchRow {
    /// Whether the Brave Search key is in the credential store.
    pub has_key: bool,
}

pub(super) struct Web {
    /// The key, or why it can't be read.
    store: Result<WebFoundation, AppError>,
    /// Makes the provider: Brave Search, or a fake in tests.
    client: Mutex<Arc<dyn WebClient>>,
}

impl Web {
    pub(super) fn new(app_data: &Path, credentials: Arc<dyn CredentialStore>) -> Self {
        let store =
            WebFoundation::open_with(app_data.join(SETTINGS_FILE), credentials).map_err(|error| {
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
        }
    }

    fn store(&self) -> Result<&WebFoundation, AppError> {
        self.store.as_ref().map_err(Clone::clone)
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
            return WebSearchRow { has_key: false };
        };
        WebSearchRow {
            has_key: store.has_key(),
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

    /// Search the web and read pages through `client` from the next call
    /// on.
    pub(crate) fn set_web_client(&self, client: Arc<dyn WebClient>) {
        *lock(&self.web.client) = client;
    }

    // What plugins reach.

    /// Whether the model may search the web and read pages.
    pub(super) fn web_ready(&self) -> bool {
        self.web.store().is_ok_and(WebFoundation::has_key)
    }

    /// Search the web for `query` for the plugin `plugin`, whose model
    /// asked for that search.
    pub(super) fn web_search(
        &self,
        plugin: &str,
        query: &str,
        max_results: Option<u32>,
    ) -> Result<Vec<AiWebResult>, AiFailure> {
        let store = self.web_call(plugin, SEARCH_TOOL, "query", query)?;
        let client = lock(&self.web.client).clone();
        let input = WebSearchInput {
            query: query.to_owned(),
            max_results: max_results.map(|count| usize::try_from(count).unwrap_or(usize::MAX)),
        };
        let found = self
            .block_on(store.search(client.as_ref(), input))
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

    /// Read the page at `url` for the plugin `plugin`, whose model asked
    /// to read it.
    pub(super) fn web_fetch(&self, plugin: &str, url: &str) -> Result<AiWebPage, AiFailure> {
        let store = self.web_call(plugin, FETCH_TOOL, "url", url)?;
        let client = lock(&self.web.client).clone();
        let input = WebFetchInput {
            url: url.to_owned(),
        };
        let page = self
            .block_on(store.fetch(client.as_ref(), input))
            .map_err(|error| web_failure(&error))?;
        Ok(AiWebPage {
            url: page.url,
            content_type: page.content_type.unwrap_or_default(),
            text: page.content,
            truncated: page.truncated,
        })
    }

    /// Search the web for `query` for an Orb's run, whose model asked for
    /// it. Noura makes the call itself, so no plugin's run is involved.
    pub(super) fn kernel_web_search(
        &self,
        query: &str,
        max_results: Option<u32>,
    ) -> Result<Vec<AiWebResult>, AiFailure> {
        self.workspace().ok_or_else(no_workspace)?;
        let store = self
            .web
            .store()
            .map_err(|_| web_failure_code("web_settings_invalid"))?;
        store.check_ready().map_err(|error| web_failure(&error))?;
        let client = lock(&self.web.client).clone();
        let input = WebSearchInput {
            query: query.to_owned(),
            max_results: max_results.map(|count| usize::try_from(count).unwrap_or(usize::MAX)),
        };
        let found = self
            .block_on(store.search(client.as_ref(), input))
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

    /// Read the page at `url` for an Orb's run, whose model asked to read
    /// it.
    pub(super) fn kernel_web_fetch(&self, url: &str) -> Result<AiWebPage, AiFailure> {
        self.workspace().ok_or_else(no_workspace)?;
        let store = self
            .web
            .store()
            .map_err(|_| web_failure_code("web_settings_invalid"))?;
        store.check_ready().map_err(|error| web_failure(&error))?;
        let client = lock(&self.web.client).clone();
        let input = WebFetchInput {
            url: url.to_owned(),
        };
        let page = self
            .block_on(store.fetch(client.as_ref(), input))
            .map_err(|error| web_failure(&error))?;
        Ok(AiWebPage {
            url: page.url,
            content_type: page.content_type.unwrap_or_default(),
            text: page.content,
            truncated: page.truncated,
        })
    }

    /// The web settings, for a web call the plugin `plugin` makes in the
    /// open workspace to answer its model's call to `tool` with `argument`
    /// set to `value`. Without the key the call fails as it always does,
    /// and a call that answers no call of the model's fails with nothing
    /// sent.
    fn web_call(
        &self,
        plugin: &str,
        tool: &str,
        argument: &str,
        value: &str,
    ) -> Result<&WebFoundation, AiFailure> {
        self.workspace().ok_or_else(no_workspace)?;
        let store = self
            .web
            .store()
            .map_err(|_| web_failure_code("web_settings_invalid"))?;
        store.check_ready().map_err(|error| web_failure(&error))?;
        if !self.model_asked(plugin, tool, argument, value) {
            tracing::warn!(%plugin, %tool, "a plugin's web call answered no call of its model's");
            return Err(web_failure_code("web_call_not_requested"));
        }
        Ok(store)
    }

    /// Whether a run the plugin `plugin` sent completed asking for `tool`
    /// with `argument` set to `value`, so the call waits for its answer.
    fn model_asked(&self, plugin: &str, tool: &str, argument: &str, value: &str) -> bool {
        lock(&self.state).runs.values().any(|run| {
            run.plugin == plugin
                && run.status == AiRunStatus::Completed
                && run.tool_calls.iter().any(|call| {
                    call.name == tool
                        && serde_json::from_str::<serde_json::Value>(&call.arguments)
                            .ok()
                            .and_then(|arguments| {
                                arguments
                                    .get(argument)?
                                    .as_str()
                                    .map(|asked| asked == value)
                            })
                            .unwrap_or(false)
                })
        })
    }

    /// Run `future` on the service's runtime and wait for it, from a
    /// thread of its own when the caller is inside a runtime already.
    pub(super) fn block_on<F>(&self, future: F) -> F::Output
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
        "web_search_key_missing" => "Add a Brave Search key in Settings > AI to search the web.",
        "web_call_not_requested" => {
            "Noura searches the web or reads a page only when the model asks for that query or \
             address."
        }
        "web_search_key_rejected" => "Brave Search refused the key. Check it in Settings > AI.",
        "web_search_rate_limited" => "Your Brave Search limit was reached. Try again in a moment.",
        "web_search_query_empty" => "The search needs a query.",
        "web_search_query_too_large" => "The search query is too long.",
        "web_search_result_limit_invalid" => "Ask for 1 to 10 results.",
        "web_url_not_https" => "Noura reads only pages with an HTTPS address.",
        "web_url_invalid" => "The address isn't a full HTTPS address.",
        "web_url_unsafe_host" => "Noura doesn't read pages on this computer or a private network.",
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
        "credential_store_error" => "Couldn't read the Brave Search key from the credential store.",
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
