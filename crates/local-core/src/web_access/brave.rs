//! Brave Search, the provider behind the AI's web search, with your own
//! key. A search is one GET to Brave Search's web search API through the
//! host request boundary, which lets it reach only Brave Search's host;
//! a page read goes to any public host through the same boundary. The key
//! travels only in the request's `X-Subscription-Token` header: no error,
//! log line or result holds it, nor what Brave Search answered when it
//! failed.

use std::sync::Arc;

use serde_json::Value;
use zeroize::Zeroizing;

use super::{
    HostRequest, HostRequests, WebAccessFuture, WebAccessProvider, WebFetchRequest,
    WebFetchResponse, WebMethod, WebSearchRequest, WebSearchResponse, WebSearchResult,
    WebTransport, host_requests::https_transport, pages, parse_https_url,
};
use crate::{CoreError, ErrorCategory, Result};

/// The host Brave Search's API answers on.
pub const BRAVE_SEARCH_HOST: &str = "api.search.brave.com";

/// Brave Search's web search endpoint.
const ENDPOINT: &str = "https://api.search.brave.com/res/v1/web/search";

const OPERATION: &str = "web_search";

/// Searches with Brave Search and reads pages, for one key.
pub struct BraveSearch {
    key: Zeroizing<String>,
    transport: Arc<dyn WebTransport>,
}

impl BraveSearch {
    /// Brave Search with `key`, over HTTPS on this device's network.
    pub fn new(key: &str) -> Self {
        Self::with_transport(key, https_transport())
    }

    /// Brave Search with `key`, over `transport`, as tests use a stand-in
    /// for the web.
    pub fn with_transport(key: &str, transport: Arc<dyn WebTransport>) -> Self {
        Self {
            key: Zeroizing::new(key.to_owned()),
            transport,
        }
    }

    async fn run_search(&self, request: WebSearchRequest) -> Result<WebSearchResponse> {
        let count = request.max_results.to_string();
        let url = url::Url::parse_with_params(
            ENDPOINT,
            [("q", request.query.as_str()), ("count", count.as_str())],
        )
        .map_err(|_| failed())?;
        let requests = HostRequests::new(self.transport.clone(), request.limits)?;
        let response = requests
            .send(
                HostRequest {
                    method: WebMethod::Get,
                    url: url.into(),
                    headers: vec![
                        ("accept".to_owned(), "application/json".to_owned()),
                        ("x-subscription-token".to_owned(), self.key.to_string()),
                    ],
                    body: Vec::new(),
                },
                &[BRAVE_SEARCH_HOST.to_owned()],
            )
            .await
            .map_err(|mut error| {
                error.operation = OPERATION.to_owned();
                error
            })?;
        if !(200..300).contains(&response.status) {
            return Err(refusal(response.status, &response.body));
        }
        let answer: Value = serde_json::from_slice(&response.body).map_err(|_| {
            tracing::warn!("Brave Search answered with something that isn't JSON");
            CoreError::new(
                "web_search_response_invalid",
                ErrorCategory::Provider,
                "Brave Search answered with something Noura can't read",
                OPERATION,
            )
        })?;
        let results = answer
            .pointer("/web/results")
            .and_then(Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .filter_map(result)
            .take(request.max_results)
            .collect();
        Ok(WebSearchResponse {
            results,
            response_bytes: response.body.len(),
        })
    }
}

impl WebAccessProvider for BraveSearch {
    fn search(&self, request: WebSearchRequest) -> WebAccessFuture<'_, WebSearchResponse> {
        Box::pin(self.run_search(request))
    }

    fn fetch(&self, request: WebFetchRequest) -> WebAccessFuture<'_, WebFetchResponse> {
        Box::pin(pages::fetch(self.transport.clone(), request))
    }
}

/// A result of Brave Search's answer, with its title and snippet as plain
/// text. A result without an HTTPS address on a public host is left out.
fn result(item: &Value) -> Option<WebSearchResult> {
    let url = item.get("url").and_then(Value::as_str)?;
    let url = parse_https_url(url, OPERATION).ok()?;
    let text = |field: &str| {
        item.get(field)
            .and_then(Value::as_str)
            .map(pages::plain_line)
            .unwrap_or_default()
    };
    Some(WebSearchResult {
        title: text("title"),
        url: url.into(),
        snippet: text("description"),
    })
}

/// What Brave Search refusing with `status` means, in public words. The
/// body only tells a rejected key from another refusal; none of it
/// reaches the error or the log.
fn refusal(status: u16, body: &[u8]) -> CoreError {
    let code = serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|answer| {
            answer
                .pointer("/error/code")
                .and_then(Value::as_str)
                .map(str::to_owned)
        })
        .unwrap_or_default();
    tracing::warn!(status, "Brave Search refused a search");
    let key_rejected = matches!(status, 401 | 403) || status == 422 && code.contains("TOKEN");
    if key_rejected {
        return CoreError::new(
            "web_search_key_rejected",
            ErrorCategory::Credential,
            "Brave Search didn't accept the key",
            OPERATION,
        );
    }
    if status == 429 {
        let mut error = CoreError::new(
            "web_search_rate_limited",
            ErrorCategory::Transient,
            "Brave Search's limit for the key was reached",
            OPERATION,
        );
        error.retryable = true;
        return error;
    }
    let mut error = failed();
    error.retryable = status >= 500;
    error
}

fn failed() -> CoreError {
    CoreError::new(
        "web_search_failed",
        ErrorCategory::Provider,
        "Brave Search couldn't answer",
        OPERATION,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::web_access::{
        TransportResponse, WebAccessLimits, WebAccessProviderConfig, WebAccessService,
        WebSearchInput,
        test_server::{FakeServer, redirect},
    };

    const KEY: &str = "BSA-test-key-0123456789";

    fn brave(server: &Arc<FakeServer>) -> WebAccessService {
        let mut service = WebAccessService::new(WebAccessLimits::default()).unwrap();
        service
            .configure(
                WebAccessProviderConfig {
                    id: "brave-search".into(),
                    user_consent_granted: true,
                },
                Arc::new(BraveSearch::with_transport(KEY, server.clone())),
            )
            .unwrap();
        service
    }

    fn json(status: u16, body: serde_json::Value) -> TransportResponse {
        TransportResponse {
            status,
            headers: vec![("content-type".into(), "application/json".into())],
            body: body.to_string().into_bytes(),
        }
    }

    const SEARCH: &str = "https://api.search.brave.com/res/v1/web/search?q=tomato+care&count=2";

    fn search(query: &str, max_results: usize) -> WebSearchInput {
        WebSearchInput {
            query: query.into(),
            max_results: Some(max_results),
        }
    }

    #[tokio::test]
    async fn a_search_sends_the_query_and_key_and_reads_titles_addresses_and_snippets() {
        let server = FakeServer::new(vec![(
            SEARCH,
            json(
                200,
                serde_json::json!({
                    "type": "search",
                    "query": { "original": "tomato care" },
                    "web": { "results": [
                        {
                            "title": "Tomato <strong>care</strong> guide",
                            "url": "https://garden.example.com/tomatoes",
                            "description": "Water &amp; feed <strong>tomatoes</strong> weekly.",
                            "extra": "ignored"
                        },
                        {
                            "title": "Plain page",
                            "url": "http://old.example.com/",
                            "description": "Not over HTTPS"
                        },
                        {
                            "title": "No address"
                        },
                        {
                            "title": "Pruning",
                            "url": "https://notes.example.com/prune",
                            "description": "Pinch the suckers."
                        },
                        {
                            "title": "One too many",
                            "url": "https://garden.example.com/more",
                            "description": "Past the count."
                        }
                    ]}
                }),
            ),
        )]);

        let response = brave(&server)
            .search(search("  tomato care ", 2))
            .await
            .unwrap();

        assert_eq!(
            response.results,
            [
                WebSearchResult {
                    title: "Tomato care guide".into(),
                    url: "https://garden.example.com/tomatoes".into(),
                    snippet: "Water & feed tomatoes weekly.".into(),
                },
                WebSearchResult {
                    title: "Pruning".into(),
                    url: "https://notes.example.com/prune".into(),
                    snippet: "Pinch the suckers.".into(),
                },
            ]
        );
        let hops = server.hops();
        assert_eq!(hops.len(), 1);
        assert_eq!(hops[0].url.as_str(), SEARCH);
        assert_eq!(hops[0].method, WebMethod::Get);
        assert!(
            hops[0]
                .headers
                .contains(&("x-subscription-token".to_owned(), KEY.to_owned()))
        );
        assert!(
            hops[0]
                .headers
                .contains(&("accept".to_owned(), "application/json".to_owned()))
        );
    }

    #[tokio::test]
    async fn an_answer_without_web_results_finds_nothing() {
        let server = FakeServer::new(vec![(
            SEARCH,
            json(200, serde_json::json!({ "type": "search" })),
        )]);

        let response = brave(&server)
            .search(search("tomato care", 2))
            .await
            .unwrap();

        assert!(response.results.is_empty());
    }

    #[tokio::test]
    async fn refusals_reach_the_caller_as_public_errors_without_the_key_or_the_answer() {
        let raw = format!("raw detail echoing {KEY}");
        for (status, body, code, retryable) in [
            (
                401,
                serde_json::json!({ "error": { "detail": raw } }),
                "web_search_key_rejected",
                false,
            ),
            (
                422,
                serde_json::json!({
                    "type": "ErrorResponse",
                    "error": { "code": "SUBSCRIPTION_TOKEN_INVALID", "detail": raw, "status": 422 }
                }),
                "web_search_key_rejected",
                false,
            ),
            (
                429,
                serde_json::json!({
                    "type": "ErrorResponse",
                    "error": { "code": "RATE_LIMITED", "detail": raw, "status": 429 }
                }),
                "web_search_rate_limited",
                true,
            ),
            (
                422,
                serde_json::json!({ "error": { "code": "VALIDATION", "detail": raw } }),
                "web_search_failed",
                false,
            ),
            (
                503,
                serde_json::json!({ "error": { "detail": raw } }),
                "web_search_failed",
                true,
            ),
        ] {
            let server = FakeServer::new(vec![(SEARCH, json(status, body))]);

            let error = brave(&server)
                .search(search("tomato care", 2))
                .await
                .unwrap_err();

            assert_eq!(error.code, code, "{status}");
            assert_eq!(error.retryable, retryable, "{status}");
            let shown = serde_json::to_string(&error).unwrap();
            assert!(!shown.contains(KEY), "{shown}");
            assert!(!shown.contains("raw detail"), "{shown}");
        }
    }

    #[tokio::test]
    async fn an_answer_that_isnt_json_fails_without_its_text() {
        let server = FakeServer::new(vec![(
            SEARCH,
            TransportResponse {
                status: 200,
                headers: Vec::new(),
                body: b"<html>captive portal secret</html>".to_vec(),
            },
        )]);

        let error = brave(&server)
            .search(search("tomato care", 2))
            .await
            .unwrap_err();

        assert_eq!(error.code, "web_search_response_invalid");
        assert!(!serde_json::to_string(&error).unwrap().contains("secret"));
    }

    #[tokio::test]
    async fn a_search_reaches_only_brave_search() {
        let server = FakeServer::new(vec![(
            SEARCH,
            redirect(302, "https://garden.example.com/collect"),
        )]);

        let error = brave(&server)
            .search(search("tomato care", 2))
            .await
            .unwrap_err();

        assert_eq!(error.code, "web_host_not_allowed");
        assert_eq!(server.hops().len(), 1);
    }
}
