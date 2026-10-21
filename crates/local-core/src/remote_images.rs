//! Remote images a note shows only when you ask: one request per load,
//! through the same address rules as the web access boundary. The limits
//! are today's app's: an `http` or `https` address without credentials, at
//! most 5 redirects, 20 seconds, an image content type and 20 MB. Every
//! address the request connects to, redirects included, must be public:
//! local, private and loopback addresses are refused before connecting.

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use url::{Host, Url};

use crate::{CoreError, ErrorCategory, Result};

/// The largest remote image a note loads, in bytes.
pub const MAX_REMOTE_IMAGE_BYTES: usize = 20 * 1024 * 1024;
/// How long one load may take, redirects included.
pub const REMOTE_IMAGE_TIMEOUT: Duration = Duration::from_secs(20);
/// How many redirects one load follows.
pub const MAX_REMOTE_IMAGE_REDIRECTS: usize = 5;

const OPERATION: &str = "remote_image_fetch";

/// A remote image's bytes and the content type the server gave them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteImage {
    pub bytes: Vec<u8>,
    pub content_type: String,
}

/// Fetches remote images. Each [`fetch`](Self::fetch) sends at most one
/// request chain and blocks until it ends.
#[derive(Debug, Clone, Default)]
pub struct RemoteImageFetcher {
    /// Lets tests serve images from a loopback server. Never set in a
    /// shipped build.
    allow_loopback: bool,
}

impl RemoteImageFetcher {
    pub fn new() -> Self {
        Self::default()
    }

    /// A fetcher that also reaches loopback addresses, for tests that serve
    /// images from this computer.
    #[cfg(feature = "test-hooks")]
    pub fn allowing_loopback() -> Self {
        Self {
            allow_loopback: true,
        }
    }

    /// Download the image at `address`.
    pub fn fetch(&self, address: &str) -> Result<RemoteImage> {
        let url = self.checked(address)?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| unavailable())?;
        runtime.block_on(self.fetch_url(url))
    }

    /// `address` as a URL a load may request: `http` or `https`, with no
    /// credentials, naming a public host.
    fn checked(&self, address: &str) -> Result<Url> {
        let url = Url::parse(address.trim()).map_err(|_| not_web_address())?;
        self.check(&url)?;
        Ok(url)
    }

    fn check(&self, url: &Url) -> Result<()> {
        if !matches!(url.scheme(), "http" | "https")
            || !url.username().is_empty()
            || url.password().is_some()
        {
            return Err(not_web_address());
        }
        let refused = match url.host().ok_or_else(not_web_address)? {
            Host::Domain(domain) => {
                let domain = domain.trim_end_matches('.').to_ascii_lowercase();
                let local = domain == "localhost" || domain.ends_with(".localhost");
                (local && !self.allow_loopback)
                    || domain == "local"
                    || domain.ends_with(".local")
                    || (!domain.contains('.') && !local)
            }
            Host::Ipv4(address) => !self.public(IpAddr::V4(address)),
            Host::Ipv6(address) => !self.public(IpAddr::V6(address)),
        };
        if refused {
            return Err(private_address());
        }
        Ok(())
    }

    /// Whether a load may connect to `address`.
    fn public(&self, address: IpAddr) -> bool {
        (self.allow_loopback && address.is_loopback())
            || crate::web_access::validate_resolved_address(address, OPERATION).is_ok()
    }

    async fn fetch_url(&self, url: Url) -> Result<RemoteImage> {
        let checker = self.clone();
        let redirects = reqwest::redirect::Policy::custom(move |attempt| {
            if attempt.previous().len() > MAX_REMOTE_IMAGE_REDIRECTS {
                attempt.error("too many redirects")
            } else if checker.check(attempt.url()).is_err() {
                attempt.error("a redirect leads to an address Noura doesn't load")
            } else {
                attempt.follow()
            }
        });
        let client = reqwest::Client::builder()
            .timeout(REMOTE_IMAGE_TIMEOUT)
            .redirect(redirects)
            .dns_resolver(Arc::new(PublicResolver(self.clone())))
            .build()
            .map_err(|_| unavailable())?;
        let mut response = client
            .get(url)
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    timed_out()
                } else {
                    unavailable()
                }
            })?
            .error_for_status()
            .map_err(|_| unavailable())?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(';').next())
            .map(|value| value.trim().to_ascii_lowercase())
            .filter(|value| value.starts_with("image/") && value.len() <= 64)
            .ok_or_else(not_an_image)?;
        if response
            .content_length()
            .is_some_and(|length| length > MAX_REMOTE_IMAGE_BYTES as u64)
        {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await.map_err(|_| unavailable())? {
            if bytes.len() + chunk.len() > MAX_REMOTE_IMAGE_BYTES {
                return Err(too_large());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(RemoteImage {
            bytes,
            content_type,
        })
    }
}

/// Resolves host names, refusing any that leads to an address a load may
/// not reach, so a public name can't point the request at this computer or
/// its network.
struct PublicResolver(RemoteImageFetcher);

impl reqwest::dns::Resolve for PublicResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let fetcher = self.0.clone();
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addresses: Vec<SocketAddr> =
                tokio::net::lookup_host((host.as_str(), 0)).await?.collect();
            if addresses.is_empty()
                || addresses
                    .iter()
                    .any(|address| !fetcher.public(address.ip()))
            {
                return Err(Box::new(private_address()) as Box<dyn std::error::Error + Send + Sync>);
            }
            Ok(Box::new(addresses.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

fn not_web_address() -> CoreError {
    CoreError::validation(
        "image_address_invalid",
        "The image address isn't a web address Noura loads",
        OPERATION,
    )
}

fn private_address() -> CoreError {
    CoreError::validation(
        "image_address_private",
        "Noura doesn't load images from this computer or its local network",
        OPERATION,
    )
}

fn not_an_image() -> CoreError {
    CoreError::validation("not_an_image", "That address isn't an image", OPERATION)
}

fn too_large() -> CoreError {
    CoreError::validation(
        "image_too_large",
        "The image is larger than 20 MB",
        OPERATION,
    )
}

fn unavailable() -> CoreError {
    let mut error = CoreError::new(
        "image_unavailable",
        ErrorCategory::Transient,
        "The image couldn't be downloaded",
        OPERATION,
    );
    error.retryable = true;
    error
}

fn timed_out() -> CoreError {
    let mut error = CoreError::new(
        "image_timeout",
        ErrorCategory::Transient,
        "The image took longer than 20 seconds to download",
        OPERATION,
    );
    error.retryable = true;
    error
}

#[cfg(test)]
mod tests {
    use super::RemoteImageFetcher;

    #[test]
    fn only_public_web_addresses_without_credentials_pass() {
        let fetcher = RemoteImageFetcher::new();
        for refused in [
            "ftp://example.com/a.png",
            "file:///etc/passwd",
            "https://user:secret@example.com/a.png",
            "https://user@example.com/a.png",
            "http://127.0.0.1/a.png",
            "http://localhost/a.png",
            "http://10.0.0.1/a.png",
            "http://192.168.1.1/a.png",
            "http://[::1]/a.png",
            "http://printer.local/a.png",
            "http://intranet/a.png",
            "not an address",
        ] {
            assert!(fetcher.checked(refused).is_err(), "{refused}");
        }
        for allowed in ["https://example.com/a.png", "http://93.184.215.14/a.png"] {
            assert!(fetcher.checked(allowed).is_ok(), "{allowed}");
        }
    }
}
