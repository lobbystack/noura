//! Requests to the hosts a caller may reach, such as the network hosts a
//! plugin declares. The boundary allows HTTPS only, checks the host
//! against the allowed list and every address it resolves to before a
//! connection opens, and follows redirects itself so it checks each hop
//! the same way. A transport sends one checked hop at a time and connects
//! only to the addresses the boundary hands it, so a host can't resolve to
//! another address between the check and the connection.

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use reqwest::header::{HeaderName, HeaderValue};
use url::{Host, Url};

use super::{
    WebAccessFuture, WebAccessLimits, parse_https_url, timeout_error, validate_redirect_target,
    validate_resolved_address,
};
use crate::{CoreError, ErrorCategory, Result};

const OPERATION: &str = "web_request";

/// The most headers one request may carry.
const MAX_HEADERS: usize = 64;

/// The most bytes a request's headers may hold, names and values.
const MAX_HEADER_BYTES: usize = 64 * 1024;

/// Headers the transport sets itself or that change how the connection
/// behaves, which a caller may not set.
const FORBIDDEN_HEADERS: [&str; 11] = [
    "connection",
    "content-length",
    "expect",
    "host",
    "keep-alive",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Headers that carry credentials, which a redirect to another host drops.
const CREDENTIAL_HEADERS: [&str; 2] = ["authorization", "cookie"];

/// An HTTP method a host request may use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebMethod {
    Get,
    Head,
    Post,
    Put,
    Patch,
    Delete,
}

impl WebMethod {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Head => "HEAD",
            Self::Post => "POST",
            Self::Put => "PUT",
            Self::Patch => "PATCH",
            Self::Delete => "DELETE",
        }
    }
}

/// A request to send to an allowed host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostRequest {
    pub method: WebMethod,
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// What the host answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostResponse {
    pub status: u16,
    /// The URL that answered, after any redirects.
    pub url: String,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// One hop of a request, after the boundary checked its URL and the
/// addresses of its host.
#[derive(Debug, Clone)]
pub struct TransportRequest {
    pub method: WebMethod,
    pub url: Url,
    /// The only addresses the transport may connect to, each checked.
    pub addresses: Vec<SocketAddr>,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub timeout: Duration,
    /// The transport stops reading and fails past this many body bytes.
    pub max_response_bytes: usize,
}

/// What one hop answered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportResponse {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

/// Resolves hosts and sends single hops for [`HostRequests`].
pub trait WebTransport: Send + Sync {
    /// The addresses `host` resolves to.
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> WebAccessFuture<'a, Vec<IpAddr>>;

    /// Send one hop, connecting only to `request.addresses`, without
    /// following redirects.
    fn send(&self, request: TransportRequest) -> WebAccessFuture<'_, TransportResponse>;
}

/// Sends requests to allowed hosts through a [`WebTransport`].
pub struct HostRequests {
    transport: Arc<dyn WebTransport>,
    limits: WebAccessLimits,
}

impl HostRequests {
    pub fn new(transport: Arc<dyn WebTransport>, limits: WebAccessLimits) -> Result<Self> {
        limits.validate(OPERATION)?;
        Ok(Self { transport, limits })
    }

    /// Requests over HTTPS on this device's network, with the default
    /// limits.
    pub fn https() -> Self {
        Self {
            transport: Arc::new(HttpsTransport),
            limits: WebAccessLimits::default(),
        }
    }

    /// Send `request` to one of `hosts`, following redirects to `hosts`.
    pub async fn send(&self, request: HostRequest, hosts: &[String]) -> Result<HostResponse> {
        let checked = check_request(request, hosts, self.limits)?;
        tokio::time::timeout(self.limits.request_timeout, self.follow(checked, hosts))
            .await
            .map_err(|_| timeout_error(OPERATION))?
    }

    /// [`Self::send`] for a caller outside an async runtime, such as a
    /// plugin's call. Inside a runtime it sends from a thread of its own.
    pub fn send_blocking(&self, request: HostRequest, hosts: &[String]) -> Result<HostResponse> {
        let run = move || {
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .map_err(|error| {
                    tracing::warn!(%error, "a runtime for a web request didn't start");
                    request_failed()
                })?;
            runtime.block_on(self.send(request, hosts))
        };
        if tokio::runtime::Handle::try_current().is_ok() {
            std::thread::scope(|scope| {
                scope
                    .spawn(run)
                    .join()
                    .unwrap_or_else(|_| Err(request_failed()))
            })
        } else {
            run()
        }
    }

    async fn follow(&self, request: Checked, hosts: &[String]) -> Result<HostResponse> {
        let Checked {
            mut method,
            mut url,
            mut headers,
            mut body,
        } = request;
        let mut redirects = 0;
        loop {
            let addresses = self.addresses(&url).await?;
            let response = self
                .transport
                .send(TransportRequest {
                    method,
                    url: url.clone(),
                    addresses,
                    headers: headers.clone(),
                    body: body.clone(),
                    timeout: self.limits.request_timeout,
                    max_response_bytes: self.limits.max_response_bytes,
                })
                .await?;
            if response.body.len() > self.limits.max_response_bytes {
                return Err(response_too_large());
            }
            let location = is_redirect(response.status)
                .then(|| header(&response.headers, "location"))
                .flatten();
            let Some(location) = location else {
                return Ok(HostResponse {
                    status: response.status,
                    url: url.into(),
                    headers: response.headers,
                    body: response.body,
                });
            };
            let target = validate_redirect_target(
                &url,
                location,
                redirects,
                self.limits.max_redirects,
                OPERATION,
            )?;
            allowed(&target, hosts)?;
            redirects += 1;
            if response.status == 303 && method != WebMethod::Head
                || matches!(response.status, 301 | 302) && method == WebMethod::Post
            {
                method = WebMethod::Get;
                body.clear();
                headers.retain(|(name, _)| !name.eq_ignore_ascii_case("content-type"));
            }
            if target.host_str() != url.host_str() {
                headers.retain(|(name, _)| {
                    !CREDENTIAL_HEADERS
                        .iter()
                        .any(|credential| name.eq_ignore_ascii_case(credential))
                });
            }
            url = target;
        }
    }

    /// The addresses to connect to for `url`, once each is known to be
    /// public.
    async fn addresses(&self, url: &Url) -> Result<Vec<SocketAddr>> {
        let port = url.port_or_known_default().ok_or_else(invalid_url)?;
        let addresses = match url.host() {
            Some(Host::Domain(domain)) => self.transport.resolve(domain, port).await?,
            Some(Host::Ipv4(address)) => vec![IpAddr::V4(address)],
            Some(Host::Ipv6(address)) => vec![IpAddr::V6(address)],
            None => return Err(invalid_url()),
        };
        if addresses.is_empty() {
            return Err(CoreError::new(
                "web_host_unresolved",
                ErrorCategory::Provider,
                "The host's address couldn't be found",
                OPERATION,
            ));
        }
        for address in &addresses {
            validate_resolved_address(*address, OPERATION)?;
        }
        Ok(addresses
            .into_iter()
            .map(|address| SocketAddr::new(address, port))
            .collect())
    }
}

/// A request whose URL, host, headers and body passed the checks.
struct Checked {
    method: WebMethod,
    url: Url,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

fn check_request(
    request: HostRequest,
    hosts: &[String],
    limits: WebAccessLimits,
) -> Result<Checked> {
    let url = parse_https_url(&request.url, OPERATION)?;
    allowed(&url, hosts)?;
    if request.headers.len() > MAX_HEADERS
        || request
            .headers
            .iter()
            .map(|(name, value)| name.len() + value.len())
            .sum::<usize>()
            > MAX_HEADER_BYTES
    {
        return Err(invalid_header("The request carries too many headers"));
    }
    for (name, value) in &request.headers {
        let valid =
            HeaderName::from_bytes(name.as_bytes()).is_ok() && HeaderValue::from_str(value).is_ok();
        if !valid {
            return Err(invalid_header("A request header isn't valid"));
        }
        if FORBIDDEN_HEADERS
            .iter()
            .any(|forbidden| name.eq_ignore_ascii_case(forbidden))
            || name.to_ascii_lowercase().starts_with("proxy-")
        {
            return Err(invalid_header(format!(
                "The request may not set the {name} header"
            )));
        }
    }
    if request.body.len() > limits.max_response_bytes {
        return Err(CoreError::validation(
            "web_request_too_large",
            "The request body exceeds the allowed size",
            OPERATION,
        ));
    }
    Ok(Checked {
        method: request.method,
        url,
        headers: request.headers,
        body: request.body,
    })
}

/// Fails unless `url`'s host is one of `hosts`.
fn allowed(url: &Url, hosts: &[String]) -> Result<()> {
    let host = url.host_str().unwrap_or_default();
    if hosts
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(host))
    {
        Ok(())
    } else {
        Err(CoreError::new(
            "web_host_not_allowed",
            ErrorCategory::Permission,
            format!("The host {host} isn't one this request may reach"),
            OPERATION,
        ))
    }
}

fn is_redirect(status: u16) -> bool {
    matches!(status, 301 | 302 | 303 | 307 | 308)
}

fn header<'a>(headers: &'a [(String, String)], name: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(known, _)| known.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

fn invalid_header(message: impl Into<String>) -> CoreError {
    CoreError::validation("web_header_invalid", message, OPERATION)
}

fn invalid_url() -> CoreError {
    CoreError::validation(
        "web_url_invalid",
        "The web URL must be an absolute HTTPS URL without credentials or fragments",
        OPERATION,
    )
}

fn response_too_large() -> CoreError {
    CoreError::new(
        "web_response_limit_exceeded",
        ErrorCategory::Provider,
        "The web response exceeded the allowed size",
        OPERATION,
    )
}

fn request_failed() -> CoreError {
    let mut error = CoreError::new(
        "web_request_failed",
        ErrorCategory::Transient,
        "The web request failed",
        OPERATION,
    );
    error.retryable = true;
    error
}

/// Sends hops over HTTPS with reqwest and resolves hosts with the system
/// resolver.
struct HttpsTransport;

impl WebTransport for HttpsTransport {
    fn resolve<'a>(&'a self, host: &'a str, port: u16) -> WebAccessFuture<'a, Vec<IpAddr>> {
        Box::pin(async move {
            let addresses = tokio::net::lookup_host((host, port))
                .await
                .map_err(|error| {
                    tracing::warn!(%error, "a web request's host didn't resolve");
                    CoreError::new(
                        "web_host_unresolved",
                        ErrorCategory::Provider,
                        "The host's address couldn't be found",
                        OPERATION,
                    )
                })?;
            Ok(addresses.map(|address| address.ip()).collect())
        })
    }

    fn send(&self, request: TransportRequest) -> WebAccessFuture<'_, TransportResponse> {
        Box::pin(async move {
            let failed = |error: reqwest::Error| {
                tracing::warn!(%error, "a web request failed");
                if error.is_timeout() {
                    timeout_error(OPERATION)
                } else {
                    request_failed()
                }
            };
            let mut builder = reqwest::Client::builder()
                .https_only(true)
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .timeout(request.timeout);
            // Connect only to the addresses the boundary checked.
            if let Some(Host::Domain(domain)) = request.url.host() {
                builder = builder.resolve_to_addrs(domain, &request.addresses);
            }
            let client = builder.build().map_err(failed)?;
            let method = reqwest::Method::from_bytes(request.method.as_str().as_bytes())
                .map_err(|_| request_failed())?;
            let mut outgoing = client.request(method, request.url);
            for (name, value) in request.headers {
                outgoing = outgoing.header(name, value);
            }
            if !request.body.is_empty() {
                outgoing = outgoing.body(request.body);
            }
            let mut response = outgoing.send().await.map_err(failed)?;
            let status = response.status().as_u16();
            let headers = response
                .headers()
                .iter()
                .filter_map(|(name, value)| {
                    Some((name.as_str().to_owned(), value.to_str().ok()?.to_owned()))
                })
                .collect();
            let mut body = Vec::new();
            while let Some(chunk) = response.chunk().await.map_err(failed)? {
                if body.len() + chunk.len() > request.max_response_bytes {
                    return Err(response_too_large());
                }
                body.extend_from_slice(&chunk);
            }
            Ok(TransportResponse {
                status,
                headers,
                body,
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// Stands in for a host's server: it answers from a script and
    /// records each address lookup and each hop it was sent.
    #[derive(Default)]
    struct FakeServer {
        /// Each host and the addresses it resolves to.
        dns: Vec<(&'static str, Vec<IpAddr>)>,
        /// Each URL and what it answers.
        answers: Vec<(&'static str, TransportResponse)>,
        lookups: Mutex<Vec<String>>,
        hops: Mutex<Vec<TransportRequest>>,
    }

    impl FakeServer {
        fn hops(&self) -> Vec<TransportRequest> {
            self.hops.lock().unwrap().clone()
        }
    }

    impl WebTransport for FakeServer {
        fn resolve<'a>(&'a self, host: &'a str, _port: u16) -> WebAccessFuture<'a, Vec<IpAddr>> {
            self.lookups.lock().unwrap().push(host.to_owned());
            let addresses = self
                .dns
                .iter()
                .find(|(known, _)| *known == host)
                .map(|(_, addresses)| addresses.clone())
                .unwrap_or_default();
            Box::pin(async move { Ok(addresses) })
        }

        fn send(&self, request: TransportRequest) -> WebAccessFuture<'_, TransportResponse> {
            let answer = self
                .answers
                .iter()
                .find(|(url, _)| *url == request.url.as_str())
                .map(|(_, answer)| answer.clone())
                .unwrap_or(TransportResponse {
                    status: 404,
                    headers: Vec::new(),
                    body: Vec::new(),
                });
            self.hops.lock().unwrap().push(request);
            Box::pin(async move { Ok(answer) })
        }
    }

    const PUBLIC: &str = "93.184.215.14";

    fn ok(body: &str) -> TransportResponse {
        TransportResponse {
            status: 200,
            headers: vec![("content-type".into(), "text/plain".into())],
            body: body.as_bytes().to_vec(),
        }
    }

    fn redirect(status: u16, location: &str) -> TransportResponse {
        TransportResponse {
            status,
            headers: vec![("location".into(), location.into())],
            body: Vec::new(),
        }
    }

    fn server(answers: Vec<(&'static str, TransportResponse)>) -> Arc<FakeServer> {
        Arc::new(FakeServer {
            dns: vec![
                ("api.example.com", vec![PUBLIC.parse().unwrap()]),
                ("tiles.example.com", vec![PUBLIC.parse().unwrap()]),
                ("evil.example.net", vec![PUBLIC.parse().unwrap()]),
                ("intranet.example.com", vec!["10.0.0.5".parse().unwrap()]),
                (
                    "mixed.example.com",
                    vec![PUBLIC.parse().unwrap(), "127.0.0.1".parse().unwrap()],
                ),
            ],
            answers,
            ..FakeServer::default()
        })
    }

    fn hosts(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| (*name).to_owned()).collect()
    }

    fn get(url: &str) -> HostRequest {
        HostRequest {
            method: WebMethod::Get,
            url: url.into(),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    fn requests(server: &Arc<FakeServer>) -> HostRequests {
        HostRequests::new(server.clone(), WebAccessLimits::default()).unwrap()
    }

    #[tokio::test]
    async fn a_request_to_an_allowed_host_connects_only_to_its_checked_addresses() {
        let server = server(vec![("https://api.example.com/forecast", ok("sunny"))]);

        let response = requests(&server)
            .send(
                get("https://api.example.com/forecast"),
                &hosts(&["api.example.com"]),
            )
            .await
            .unwrap();

        assert_eq!(response.status, 200);
        assert_eq!(response.body, b"sunny");
        assert_eq!(response.url, "https://api.example.com/forecast");
        let hops = server.hops();
        assert_eq!(hops.len(), 1);
        assert_eq!(
            hops[0].addresses,
            [SocketAddr::new(PUBLIC.parse().unwrap(), 443)]
        );
    }

    #[tokio::test]
    async fn other_hosts_plain_http_and_local_names_fail_before_any_lookup_or_connection() {
        let server = server(Vec::new());
        let requests = requests(&server);
        let allowed = hosts(&["api.example.com", "localhost"]);

        for (url, code) in [
            ("https://evil.example.net/", "web_host_not_allowed"),
            ("http://api.example.com/", "web_url_not_https"),
            ("https://localhost/", "web_url_unsafe_host"),
            ("https://10.0.0.5/", "web_url_unsafe_host"),
            ("https://user@api.example.com/", "web_url_invalid"),
        ] {
            let error = requests.send(get(url), &allowed).await.unwrap_err();
            assert_eq!(error.code, code, "{url}");
        }
        assert!(server.lookups.lock().unwrap().is_empty());
        assert!(server.hops().is_empty());
    }

    #[tokio::test]
    async fn an_allowed_host_that_resolves_to_a_private_address_fails_before_connecting() {
        let server = server(Vec::new());
        let requests = requests(&server);

        for host in ["intranet.example.com", "mixed.example.com"] {
            let error = requests
                .send(get(&format!("https://{host}/")), &hosts(&[host]))
                .await
                .unwrap_err();
            assert_eq!(error.code, "web_url_unsafe_host", "{host}");
        }
        assert!(server.hops().is_empty());
    }

    #[tokio::test]
    async fn a_redirect_to_a_host_that_isnt_allowed_fails_before_connecting_to_it() {
        let server = server(vec![(
            "https://api.example.com/start",
            redirect(302, "https://evil.example.net/steal"),
        )]);

        let error = requests(&server)
            .send(
                get("https://api.example.com/start"),
                &hosts(&["api.example.com"]),
            )
            .await
            .unwrap_err();

        assert_eq!(error.code, "web_host_not_allowed");
        let hops: Vec<String> = server
            .hops()
            .iter()
            .map(|hop| hop.url.to_string())
            .collect();
        assert_eq!(hops, ["https://api.example.com/start"]);
        assert_eq!(*server.lookups.lock().unwrap(), ["api.example.com"]);
    }

    #[tokio::test]
    async fn redirects_to_allowed_hosts_are_followed_and_checked_at_every_hop() {
        let server = server(vec![
            (
                "https://api.example.com/start",
                redirect(303, "https://tiles.example.com/next"),
            ),
            (
                "https://tiles.example.com/next",
                redirect(307, "http://tiles.example.com/plain"),
            ),
        ]);
        let mut request = get("https://api.example.com/start");
        request.method = WebMethod::Post;
        request.body = b"payload".to_vec();
        request.headers = vec![("Authorization".into(), "Bearer secret".into())];

        let error = requests(&server)
            .send(request, &hosts(&["api.example.com", "tiles.example.com"]))
            .await
            .unwrap_err();

        assert_eq!(error.code, "web_url_not_https");
        let hops = server.hops();
        assert_eq!(hops.len(), 2);
        // A 303 turns the request into a GET without a body, and the
        // credentials stay with the host they were meant for.
        assert_eq!(hops[1].method, WebMethod::Get);
        assert!(hops[1].body.is_empty());
        assert!(hops[1].headers.is_empty());
    }

    #[tokio::test]
    async fn redirects_stop_at_the_limit() {
        let server = server(vec![(
            "https://api.example.com/loop",
            redirect(302, "/loop"),
        )]);

        let error = requests(&server)
            .send(
                get("https://api.example.com/loop"),
                &hosts(&["api.example.com"]),
            )
            .await
            .unwrap_err();

        assert_eq!(error.code, "web_redirect_limit_exceeded");
        assert_eq!(
            server.hops().len(),
            WebAccessLimits::default().max_redirects + 1
        );
    }

    #[tokio::test]
    async fn headers_the_transport_owns_are_refused() {
        let server = server(Vec::new());
        let requests = requests(&server);

        for (name, value) in [
            ("Host", "evil.example.net"),
            ("Content-Length", "5"),
            ("Proxy-Authorization", "x"),
            ("Bad Name", "x"),
            ("X-Ok", "line\r\nbreak"),
        ] {
            let mut request = get("https://api.example.com/");
            request.headers = vec![(name.into(), value.into())];
            let error = requests
                .send(request, &hosts(&["api.example.com"]))
                .await
                .unwrap_err();
            assert_eq!(error.code, "web_header_invalid", "{name}");
        }
        assert!(server.hops().is_empty());
    }

    #[tokio::test]
    async fn an_oversized_response_is_refused() {
        let limits = WebAccessLimits::default();
        let server = server(vec![(
            "https://api.example.com/big",
            ok(&"x".repeat(limits.max_response_bytes + 1)),
        )]);

        let error = requests(&server)
            .send(
                get("https://api.example.com/big"),
                &hosts(&["api.example.com"]),
            )
            .await
            .unwrap_err();

        assert_eq!(error.code, "web_response_limit_exceeded");
    }

    #[test]
    fn a_blocking_send_works_inside_and_outside_a_runtime() {
        let server = server(vec![("https://api.example.com/", ok("hi"))]);
        let requests = requests(&server);
        let allowed = hosts(&["api.example.com"]);

        let outside = requests
            .send_blocking(get("https://api.example.com/"), &allowed)
            .unwrap();
        let runtime = tokio::runtime::Runtime::new().unwrap();
        let inside = runtime
            .block_on(async { requests.send_blocking(get("https://api.example.com/"), &allowed) });

        assert_eq!(outside.body, b"hi");
        assert_eq!(inside.unwrap().body, b"hi");
    }

    /// A request through the real transport to `url`, which only
    /// `address` may answer.
    fn pinned(url: &str, address: SocketAddr) -> TransportRequest {
        TransportRequest {
            method: WebMethod::Get,
            url: Url::parse(url).unwrap(),
            addresses: vec![address],
            headers: Vec::new(),
            body: Vec::new(),
            timeout: Duration::from_secs(5),
            max_response_bytes: 1024,
        }
    }

    #[tokio::test]
    async fn the_https_transport_connects_only_to_the_checked_address() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        // A `.invalid` name never resolves, so a connection that reaches
        // the listener came from the address the boundary checked.
        let url = format!("https://pinned.invalid:{}/", address.port());
        let accepted = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            drop(stream);
        });

        let result = HttpsTransport.send(pinned(&url, address)).await;

        assert!(result.is_err(), "no TLS server answered");
        tokio::time::timeout(Duration::from_secs(5), accepted)
            .await
            .expect("the transport connected to the checked address")
            .unwrap();
    }

    #[tokio::test]
    async fn the_https_transport_refuses_plain_http_without_connecting() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let url = format!("http://pinned.invalid:{}/", address.port());

        let result = HttpsTransport.send(pinned(&url, address)).await;

        assert!(result.is_err());
        let accepted = tokio::time::timeout(Duration::from_millis(200), listener.accept()).await;
        assert!(accepted.is_err(), "the transport connected over plain HTTP");
    }
}
