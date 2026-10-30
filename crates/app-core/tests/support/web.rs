//! A fake web provider for the AI's web search: it records the key it was
//! made with and every search and page read it gets, and answers from a
//! script. Nothing leaves the computer.

use std::sync::{Arc, Mutex};

use app_core::web_client::{
    WebAccessFuture, WebAccessProvider, WebClient, WebFetchRequest, WebFetchResponse,
    WebSearchRequest, WebSearchResponse, WebSearchResult,
};
use local_core::{CoreError, ErrorCategory};

/// What a failing search's error carries: what Brave Search answered and
/// the key, neither of which may reach a plugin, a chat file or the log.
pub const RAW_WEB_FAILURE: &str =
    "HTTP 422 {\"error\":{\"code\":\"SUBSCRIPTION_TOKEN_INVALID\"}} X-Subscription-Token: BSA-raw";

/// What the fake web provider got, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WebCall {
    Search(String),
    Fetch(String),
}

#[derive(Default)]
pub struct FakeWeb {
    keys: Mutex<Vec<String>>,
    calls: Mutex<Vec<WebCall>>,
}

impl FakeWeb {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// The keys providers were made with.
    pub fn keys(&self) -> Vec<String> {
        self.keys.lock().unwrap().clone()
    }

    /// Every search and page read so far.
    pub fn calls(&self) -> Vec<WebCall> {
        self.calls.lock().unwrap().clone()
    }
}

/// A client that makes `web`'s provider.
pub struct FakeWebClient(pub Arc<FakeWeb>);

impl WebClient for FakeWebClient {
    fn provider(&self, key: &str) -> Arc<dyn WebAccessProvider> {
        self.0.keys.lock().unwrap().push(key.to_owned());
        Arc::new(Provider(self.0.clone()))
    }
}

struct Provider(Arc<FakeWeb>);

/// A provider's failure with `code`, carrying the raw answer a careless
/// adapter might pass along.
fn raw_failure(code: &str, category: ErrorCategory) -> CoreError {
    let mut error = CoreError::new(code, category, RAW_WEB_FAILURE, "web_search");
    error.details = Some(serde_json::json!({ "body": RAW_WEB_FAILURE }));
    error
}

impl WebAccessProvider for Provider {
    /// Finds two results for any query, and fails for the queries
    /// `rejected key` and `rate limited` as Brave Search does.
    fn search(&self, request: WebSearchRequest) -> WebAccessFuture<'_, WebSearchResponse> {
        self.0
            .calls
            .lock()
            .unwrap()
            .push(WebCall::Search(request.query.clone()));
        Box::pin(async move {
            match request.query.as_str() {
                "rejected key" => Err(raw_failure(
                    "web_search_key_rejected",
                    ErrorCategory::Credential,
                )),
                "rate limited" => Err(raw_failure(
                    "web_search_rate_limited",
                    ErrorCategory::Transient,
                )),
                query => Ok(WebSearchResponse {
                    results: vec![
                        WebSearchResult {
                            title: format!("{query} guide"),
                            url: "https://garden.example.com/tomatoes".into(),
                            snippet: "Water and feed tomatoes weekly.".into(),
                        },
                        WebSearchResult {
                            title: "Pruning".into(),
                            url: "https://notes.example.com/prune".into(),
                            snippet: "Pinch the suckers.".into(),
                        },
                    ],
                    response_bytes: 512,
                }),
            }
        })
    }

    fn fetch(&self, request: WebFetchRequest) -> WebAccessFuture<'_, WebFetchResponse> {
        let url = request.url.to_string();
        self.0
            .calls
            .lock()
            .unwrap()
            .push(WebCall::Fetch(url.clone()));
        Box::pin(async move {
            Ok(WebFetchResponse {
                url,
                status: 200,
                content_type: Some("text/html".into()),
                content: "Growing tomatoes\n\nPlant them in full sun.".into(),
                response_bytes: 2048,
                truncated: false,
            })
        })
    }
}
