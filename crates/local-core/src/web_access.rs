use std::{
    future::Future,
    net::{IpAddr, Ipv4Addr},
    pin::Pin,
    sync::Arc,
    time::Duration,
};

use url::{Host, Url};

use crate::{CoreError, ErrorCategory, Result};

const MAX_SEARCH_QUERY_BYTES: usize = 4 * 1024;
const MAX_SEARCH_RESULTS: usize = 20;
const MAX_REDIRECTS: usize = 5;
const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
const MAX_CONTENT_BYTES: usize = 2 * 1024 * 1024;
const MAX_REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchInput {
    pub query: String,
    pub max_results: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchResult {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchResponse {
    pub results: Vec<WebSearchResult>,
    pub response_bytes: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebFetchInput {
    pub url: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebFetchResponse {
    pub url: String,
    pub status: u16,
    pub content_type: Option<String>,
    pub content: String,
    pub response_bytes: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WebAccessLimits {
    pub max_search_results: usize,
    pub max_redirects: usize,
    pub max_response_bytes: usize,
    pub max_content_bytes: usize,
    pub request_timeout: Duration,
}

impl Default for WebAccessLimits {
    fn default() -> Self {
        Self {
            max_search_results: 10,
            max_redirects: 3,
            max_response_bytes: 5 * 1024 * 1024,
            max_content_bytes: 1024 * 1024,
            request_timeout: Duration::from_secs(15),
        }
    }
}

impl WebAccessLimits {
    pub fn validate(self, operation: &str) -> Result<()> {
        if self.max_search_results == 0 || self.max_search_results > MAX_SEARCH_RESULTS {
            return Err(limit_error(operation));
        }
        if self.max_redirects > MAX_REDIRECTS
            || self.max_response_bytes == 0
            || self.max_response_bytes > MAX_RESPONSE_BYTES
            || self.max_content_bytes == 0
            || self.max_content_bytes > self.max_response_bytes
            || self.max_content_bytes > MAX_CONTENT_BYTES
            || self.request_timeout.is_zero()
            || self.request_timeout > MAX_REQUEST_TIMEOUT
        {
            return Err(limit_error(operation));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebSearchRequest {
    pub query: String,
    pub max_results: usize,
    pub limits: WebAccessLimits,
}

#[derive(Debug, Clone)]
pub struct WebFetchRequest {
    pub url: Url,
    pub limits: WebAccessLimits,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebAccessProviderConfig {
    pub id: String,
    pub user_consent_granted: bool,
}

pub type WebAccessFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T>> + Send + 'a>>;

/// Providers must validate each resolved address and redirect with this module's helpers.
pub trait WebAccessProvider: Send + Sync {
    fn search(&self, request: WebSearchRequest) -> WebAccessFuture<'_, WebSearchResponse>;

    fn fetch(&self, request: WebFetchRequest) -> WebAccessFuture<'_, WebFetchResponse>;
}

struct ConfiguredWebAccessProvider {
    provider: Arc<dyn WebAccessProvider>,
}

/// Native-only web access boundary. It has no provider or credential discovery behavior.
#[derive(Default)]
pub struct WebAccessService {
    provider: Option<ConfiguredWebAccessProvider>,
    limits: WebAccessLimits,
}

impl WebAccessService {
    pub fn new(limits: WebAccessLimits) -> Result<Self> {
        limits.validate("web_access_configure")?;
        Ok(Self {
            provider: None,
            limits,
        })
    }

    pub fn configure(
        &mut self,
        config: WebAccessProviderConfig,
        provider: Arc<dyn WebAccessProvider>,
    ) -> Result<()> {
        if config.id.trim().is_empty() {
            return Err(CoreError::validation(
                "web_provider_invalid",
                "A web access provider ID is required",
                "web_access_configure",
            ));
        }
        if !config.user_consent_granted {
            return Err(CoreError::new(
                "web_access_consent_required",
                ErrorCategory::Permission,
                "Web access requires explicit user consent",
                "web_access_configure",
            ));
        }
        self.provider = Some(ConfiguredWebAccessProvider { provider });
        Ok(())
    }

    pub async fn search(&self, input: WebSearchInput) -> Result<WebSearchResponse> {
        let provider = self.provider.as_ref().ok_or_else(unavailable_error)?;
        let request = validate_search_input(input, self.limits)?;
        let max_results = request.max_results;
        let response = tokio::time::timeout(
            self.limits.request_timeout,
            provider.provider.search(request),
        )
        .await
        .map_err(|_| timeout_error("web_search"))??;
        validate_search_response(response, max_results, self.limits, "web_search")
    }

    pub async fn fetch(&self, input: WebFetchInput) -> Result<WebFetchResponse> {
        let provider = self.provider.as_ref().ok_or_else(unavailable_error)?;
        let request = WebFetchRequest {
            url: parse_https_url(&input.url, "web_fetch")?,
            limits: self.limits,
        };
        let response = tokio::time::timeout(
            self.limits.request_timeout,
            provider.provider.fetch(request),
        )
        .await
        .map_err(|_| timeout_error("web_fetch"))??;
        validate_fetch_response(response, self.limits, "web_fetch")
    }
}

pub fn parse_https_url(value: &str, operation: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| invalid_url_error(operation))?;
    validate_https_url(url, operation)
}

pub fn validate_redirect_target(
    current: &Url,
    location: &str,
    redirects_followed: usize,
    max_redirects: usize,
    operation: &str,
) -> Result<Url> {
    if redirects_followed >= max_redirects {
        return Err(CoreError::new(
            "web_redirect_limit_exceeded",
            ErrorCategory::Provider,
            "The web request exceeded its redirect limit",
            operation,
        ));
    }
    let current = validate_https_url(current.clone(), operation)?;
    let target = current.join(location).map_err(|_| {
        CoreError::new(
            "web_redirect_invalid",
            ErrorCategory::Validation,
            "The web redirect target is invalid",
            operation,
        )
    })?;
    validate_https_url(target, operation)
}

/// Reject a DNS answer before a provider connects, including on every redirect hop.
pub fn validate_resolved_address(address: IpAddr, operation: &str) -> Result<()> {
    let unsafe_address = match address {
        IpAddr::V4(address) => ipv4_is_local_or_private(address),
        IpAddr::V6(address) => ipv6_is_local_or_private(address),
    };
    if unsafe_address {
        return Err(CoreError::validation(
            "web_url_unsafe_host",
            "Web access does not permit local or private network hosts",
            operation,
        ));
    }
    Ok(())
}

fn validate_search_input(
    input: WebSearchInput,
    limits: WebAccessLimits,
) -> Result<WebSearchRequest> {
    let query = input.query.trim();
    if query.is_empty() {
        return Err(CoreError::validation(
            "web_search_query_empty",
            "A web search query is required",
            "web_search",
        ));
    }
    if query.len() > MAX_SEARCH_QUERY_BYTES {
        return Err(CoreError::validation(
            "web_search_query_too_large",
            "The web search query exceeds the allowed size",
            "web_search",
        ));
    }
    let max_results = input.max_results.unwrap_or(limits.max_search_results);
    if max_results == 0 || max_results > limits.max_search_results {
        return Err(CoreError::validation(
            "web_search_result_limit_invalid",
            "The requested web search result limit is not allowed",
            "web_search",
        ));
    }
    Ok(WebSearchRequest {
        query: query.to_owned(),
        max_results,
        limits,
    })
}

fn validate_search_response(
    mut response: WebSearchResponse,
    max_results: usize,
    limits: WebAccessLimits,
    operation: &str,
) -> Result<WebSearchResponse> {
    validate_response_bytes(response.response_bytes, limits, operation)?;
    if response.results.len() > max_results {
        return Err(CoreError::new(
            "web_search_response_limit_exceeded",
            ErrorCategory::Provider,
            "The web search response exceeded the allowed result limit",
            operation,
        ));
    }
    let mut content_bytes = 0_usize;
    for result in &mut response.results {
        let url = parse_https_url(&result.url, operation)?;
        result.url = url.into();
        content_bytes = content_bytes
            .saturating_add(result.title.len())
            .saturating_add(result.url.len())
            .saturating_add(result.snippet.len());
    }
    validate_content_bytes(content_bytes, limits, operation)?;
    Ok(response)
}

fn validate_fetch_response(
    mut response: WebFetchResponse,
    limits: WebAccessLimits,
    operation: &str,
) -> Result<WebFetchResponse> {
    validate_response_bytes(response.response_bytes, limits, operation)?;
    validate_content_bytes(response.content.len(), limits, operation)?;
    if response.response_bytes < response.content.len() {
        return Err(CoreError::new(
            "web_response_invalid",
            ErrorCategory::Provider,
            "The web provider reported an invalid response size",
            operation,
        ));
    }
    response.url = parse_https_url(&response.url, operation)?.into();
    Ok(response)
}

fn validate_response_bytes(bytes: usize, limits: WebAccessLimits, operation: &str) -> Result<()> {
    if bytes > limits.max_response_bytes {
        return Err(CoreError::new(
            "web_response_limit_exceeded",
            ErrorCategory::Provider,
            "The web response exceeded the allowed size",
            operation,
        ));
    }
    Ok(())
}

fn validate_content_bytes(bytes: usize, limits: WebAccessLimits, operation: &str) -> Result<()> {
    if bytes > limits.max_content_bytes {
        return Err(CoreError::new(
            "web_content_limit_exceeded",
            ErrorCategory::Provider,
            "The web content exceeded the allowed size",
            operation,
        ));
    }
    Ok(())
}

fn validate_https_url(url: Url, operation: &str) -> Result<Url> {
    if url.scheme() != "https" {
        return Err(CoreError::validation(
            "web_url_not_https",
            "Web access only permits HTTPS URLs",
            operation,
        ));
    }
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() {
        return Err(invalid_url_error(operation));
    }
    if url.port() == Some(0) {
        return Err(invalid_url_error(operation));
    }
    let host = url.host().ok_or_else(|| invalid_url_error(operation))?;
    if host_is_local_or_private(host) {
        return Err(CoreError::validation(
            "web_url_unsafe_host",
            "Web access does not permit local or private network hosts",
            operation,
        ));
    }
    Ok(url)
}

fn host_is_local_or_private(host: Host<&str>) -> bool {
    match host {
        Host::Domain(domain) => {
            let domain = domain.trim_end_matches('.');
            domain.eq_ignore_ascii_case("localhost")
                || domain.ends_with(".localhost")
                || domain.eq_ignore_ascii_case("local")
                || domain.ends_with(".local")
                || !domain.contains('.')
        }
        Host::Ipv4(address) => ipv4_is_local_or_private(address),
        Host::Ipv6(address) => ipv6_is_local_or_private(address),
    }
}

fn ipv4_is_local_or_private(address: Ipv4Addr) -> bool {
    let [first, second, ..] = address.octets();
    first == 0
        || first == 10
        || first == 100 && (64..=127).contains(&second)
        || first == 127
        || first == 169 && second == 254
        || first == 172 && (16..=31).contains(&second)
        || first == 192 && (second == 0 || second == 168)
        || first == 198 && (second == 18 || second == 19 || second == 51)
        || first == 203 && second == 0
        || first >= 224
}

fn ipv6_is_local_or_private(address: std::net::Ipv6Addr) -> bool {
    let segments = address.segments();
    if address.is_unspecified() || address.is_loopback() || segments[0] & 0xfe00 == 0xfc00 {
        return true;
    }
    if segments[0] & 0xffc0 == 0xfe80 || segments[0] & 0xffc0 == 0xfec0 {
        return true;
    }
    if segments[0] & 0xff00 == 0xff00 || (segments[0] == 0x2001 && segments[1] == 0x0db8) {
        return true;
    }
    let embedded_ipv4 = segments[..6].iter().all(|segment| *segment == 0)
        || segments[..5].iter().all(|segment| *segment == 0) && segments[5] == 0xffff;
    let mapped_ipv4 = embedded_ipv4.then(|| {
        Ipv4Addr::new(
            (segments[6] >> 8) as u8,
            segments[6] as u8,
            (segments[7] >> 8) as u8,
            segments[7] as u8,
        )
    });
    mapped_ipv4.is_some_and(ipv4_is_local_or_private)
}

fn unavailable_error() -> CoreError {
    CoreError::new(
        "web_access_unavailable",
        ErrorCategory::Provider,
        "Web access is unavailable until a provider is explicitly configured with user consent",
        "web_access",
    )
}

fn timeout_error(operation: &str) -> CoreError {
    let mut error = CoreError::new(
        "web_request_timeout",
        ErrorCategory::Transient,
        "The web request exceeded its timeout",
        operation,
    );
    error.retryable = true;
    error
}

fn invalid_url_error(operation: &str) -> CoreError {
    CoreError::validation(
        "web_url_invalid",
        "The web URL must be an absolute HTTPS URL without credentials or fragments",
        operation,
    )
}

fn limit_error(operation: &str) -> CoreError {
    CoreError::validation(
        "web_limits_invalid",
        "The web access limits are invalid or exceed the allowed maximums",
        operation,
    )
}

#[cfg(test)]
mod tests {
    use std::{sync::Mutex, time::Duration};

    use super::*;

    struct FetchProvider {
        response: WebFetchResponse,
        request: Mutex<Option<WebFetchRequest>>,
    }

    impl WebAccessProvider for FetchProvider {
        fn search(&self, _request: WebSearchRequest) -> WebAccessFuture<'_, WebSearchResponse> {
            Box::pin(async { unreachable!("search is not called by this test") })
        }

        fn fetch(&self, request: WebFetchRequest) -> WebAccessFuture<'_, WebFetchResponse> {
            *self.request.lock().unwrap() = Some(request);
            Box::pin(async { Ok(self.response.clone()) })
        }
    }

    struct SlowProvider;

    impl WebAccessProvider for SlowProvider {
        fn search(&self, _request: WebSearchRequest) -> WebAccessFuture<'_, WebSearchResponse> {
            Box::pin(async { unreachable!("search is not called by this test") })
        }

        fn fetch(&self, _request: WebFetchRequest) -> WebAccessFuture<'_, WebFetchResponse> {
            Box::pin(async {
                tokio::time::sleep(Duration::from_millis(20)).await;
                Ok(fetch_response("ok", 2))
            })
        }
    }

    fn fetch_response(content: &str, response_bytes: usize) -> WebFetchResponse {
        WebFetchResponse {
            url: "https://example.com/article".into(),
            status: 200,
            content_type: Some("text/plain".into()),
            content: content.into(),
            response_bytes,
        }
    }

    fn consented_config() -> WebAccessProviderConfig {
        WebAccessProviderConfig {
            id: "test-provider".into(),
            user_consent_granted: true,
        }
    }

    fn configured_service(provider: Arc<dyn WebAccessProvider>) -> WebAccessService {
        let mut service = WebAccessService::default();
        service.configure(consented_config(), provider).unwrap();
        service
    }

    #[tokio::test]
    async fn access_fails_closed_until_a_consented_provider_is_configured() {
        let error = WebAccessService::default()
            .fetch(WebFetchInput {
                url: "https://example.com".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "web_access_unavailable");
    }

    #[test]
    fn configuration_requires_explicit_user_consent() {
        let provider = Arc::new(FetchProvider {
            response: fetch_response("ok", 2),
            request: Mutex::new(None),
        });
        let mut service = WebAccessService::default();
        let error = service
            .configure(
                WebAccessProviderConfig {
                    id: "test-provider".into(),
                    user_consent_granted: false,
                },
                provider,
            )
            .unwrap_err();
        assert_eq!(error.code, "web_access_consent_required");
    }

    #[test]
    fn non_https_and_local_targets_are_rejected() {
        for target in [
            "http://example.com",
            "file:///etc/passwd",
            "data:text/plain,secret",
            "https://localhost",
            "https://api.localhost",
            "https://printer.local",
            "https://intranet",
            "https://127.0.0.1",
            "https://10.0.0.1",
            "https://169.254.1.1",
            "https://[::1]",
            "https://[fc00::1]",
            "https://[fe80::1]",
            "https://[::ffff:127.0.0.1]",
        ] {
            assert!(parse_https_url(target, "test").is_err(), "{target}");
        }
    }

    #[test]
    fn strict_https_parsing_allows_public_targets_only() {
        assert_eq!(
            parse_https_url("https://example.com/path?query=value", "test")
                .unwrap()
                .as_str(),
            "https://example.com/path?query=value"
        );
        assert!(parse_https_url("https://8.8.8.8/", "test").is_ok());
        assert!(parse_https_url("https://user@example.com", "test").is_err());
        assert!(parse_https_url("https://example.com/#fragment", "test").is_err());
    }

    #[test]
    fn private_dns_answers_are_rejected_before_connecting() {
        assert_eq!(
            validate_resolved_address("192.168.1.1".parse().unwrap(), "test")
                .unwrap_err()
                .code,
            "web_url_unsafe_host"
        );
        assert_eq!(
            validate_resolved_address("fe80::1".parse().unwrap(), "test")
                .unwrap_err()
                .code,
            "web_url_unsafe_host"
        );
        assert!(validate_resolved_address("8.8.8.8".parse().unwrap(), "test").is_ok());
    }

    #[test]
    fn redirects_are_resolved_then_validated_at_every_hop() {
        let current = parse_https_url("https://example.com/start", "test").unwrap();
        assert_eq!(
            validate_redirect_target(&current, "/next", 0, 1, "test")
                .unwrap()
                .as_str(),
            "https://example.com/next"
        );
        assert_eq!(
            validate_redirect_target(&current, "http://example.com", 0, 1, "test")
                .unwrap_err()
                .code,
            "web_url_not_https"
        );
        assert_eq!(
            validate_redirect_target(&current, "//127.0.0.1", 0, 1, "test")
                .unwrap_err()
                .code,
            "web_url_unsafe_host"
        );
        assert_eq!(
            validate_redirect_target(&current, "/next", 1, 1, "test")
                .unwrap_err()
                .code,
            "web_redirect_limit_exceeded"
        );
    }

    #[tokio::test]
    async fn providers_receive_bounded_requests_and_oversized_outputs_are_rejected() {
        let provider = Arc::new(FetchProvider {
            response: fetch_response("12345", 5),
            request: Mutex::new(None),
        });
        let service = configured_service(provider.clone());
        let response = service
            .fetch(WebFetchInput {
                url: "https://example.com/article".into(),
            })
            .await
            .unwrap();
        assert_eq!(response.content, "12345");
        let request = provider.request.lock().unwrap().take().unwrap();
        assert_eq!(request.limits, WebAccessLimits::default());

        let oversized = configured_service(Arc::new(FetchProvider {
            response: fetch_response("ok", WebAccessLimits::default().max_response_bytes + 1),
            request: Mutex::new(None),
        }));
        let error = oversized
            .fetch(WebFetchInput {
                url: "https://example.com/article".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "web_response_limit_exceeded");
    }

    #[tokio::test]
    async fn provider_calls_cannot_exceed_the_configured_timeout() {
        let limits = WebAccessLimits {
            request_timeout: Duration::from_millis(1),
            ..WebAccessLimits::default()
        };
        let mut service = WebAccessService::new(limits).unwrap();
        service
            .configure(consented_config(), Arc::new(SlowProvider))
            .unwrap();
        let error = service
            .fetch(WebFetchInput {
                url: "https://example.com/article".into(),
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "web_request_timeout");
    }
}
