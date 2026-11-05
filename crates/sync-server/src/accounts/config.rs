//! What the account pages run with: the server's public origin, the email
//! allowlist and the mail transport.

use std::collections::BTreeSet;

use url::Url;

/// The server's public origin in the form enrollment and recovery proofs
/// name (§7): lowercase scheme and host, the port only when it isn't the
/// default, no trailing slash. It must be HTTPS, except on a loopback host,
/// where plain HTTP serves development and tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicOrigin {
    url: Url,
    text: String,
}

impl PublicOrigin {
    /// Parses and checks an origin such as `https://sync.noura.app`.
    pub fn parse(text: &str) -> Result<Self, String> {
        let url = Url::parse(text.trim()).map_err(|_| format!("{text} isn't a URL"))?;
        if !matches!(url.scheme(), "https" | "http") {
            return Err(format!("{text} isn't an HTTPS origin"));
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || !matches!(url.path(), "" | "/")
        {
            return Err(format!(
                "{text} isn't an origin: it may hold only a scheme, a host and a port"
            ));
        }
        let host = url.host().ok_or_else(|| format!("{text} names no host"))?;
        if url.scheme() == "http" && !is_loopback(&host) {
            return Err(format!(
                "{text} isn't HTTPS; plain HTTP is allowed only on a loopback host"
            ));
        }
        let text = url.origin().ascii_serialization();
        Ok(Self { url, text })
    }

    /// The origin as proofs name it.
    pub fn as_str(&self) -> &str {
        &self.text
    }

    /// The origin as a URL.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// The host, which passkeys use as their relying party ID.
    pub fn host(&self) -> String {
        self.url.host_str().unwrap_or_default().to_owned()
    }

    /// Whether the origin is HTTPS, so cookies carry `Secure`.
    pub fn is_https(&self) -> bool {
        self.url.scheme() == "https"
    }
}

fn is_loopback(host: &url::Host<&str>) -> bool {
    match host {
        url::Host::Domain(name) => {
            let name = name.to_ascii_lowercase();
            name == "localhost" || name.ends_with(".localhost")
        }
        url::Host::Ipv4(address) => address.is_loopback(),
        url::Host::Ipv6(address) => address.is_loopback(),
    }
}

/// An email address as the server keeps it: trimmed, with its ASCII
/// letters lowercased and every other byte kept, the form account IDs
/// derive from (§14). Refuses text that can't be an address.
pub fn normalize_email(text: &str) -> Option<String> {
    let email = sync_protocol::ids::normalize_email(text.trim());
    let (local, domain) = email.split_once('@')?;
    let valid = !local.is_empty()
        && !domain.is_empty()
        && !domain.contains('@')
        && email.len() <= 254
        && !email.chars().any(|c| c.is_whitespace() || c.is_control());
    valid.then_some(email)
}

/// The email allowlist: the addresses that may sign up. An empty list
/// admits nobody.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Allowlist(BTreeSet<String>);

impl Allowlist {
    /// The allowlist from a comma-separated list of addresses.
    pub fn parse(text: &str) -> Self {
        Self(text.split(',').filter_map(normalize_email).collect())
    }

    /// Whether `email`, already normalized, may sign up.
    pub fn admits(&self, email: &str) -> bool {
        self.0.contains(email)
    }

    /// How many addresses the list holds.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the list admits nobody.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origins_are_https_except_on_loopback() {
        for good in [
            "https://sync.noura.app",
            "https://sync.noura.app/",
            "HTTPS://Sync.Noura.App:443",
            "http://localhost:8080",
            "http://127.0.0.1:3000",
            "http://[::1]:3000",
            "http://app.localhost",
        ] {
            PublicOrigin::parse(good).unwrap_or_else(|e| panic!("{good}: {e}"));
        }
        assert_eq!(
            PublicOrigin::parse("HTTPS://Sync.Noura.App:443/")
                .unwrap()
                .as_str(),
            "https://sync.noura.app"
        );
        for bad in [
            "http://sync.noura.app",
            "http://10.0.0.2:8080",
            "ftp://sync.noura.app",
            "https://sync.noura.app/account",
            "https://user@sync.noura.app",
            "https://sync.noura.app?x=1",
            "sync.noura.app",
        ] {
            assert!(PublicOrigin::parse(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn emails_keep_non_ascii_bytes() {
        assert_eq!(
            normalize_email("  Alice.Example@Example.COM "),
            Some("alice.example@example.com".into())
        );
        assert_eq!(
            normalize_email("ÅSA@Example.se"),
            Some("Åsa@example.se".into())
        );
        for bad in [
            "",
            "alice",
            "@example.com",
            "alice@",
            "a b@example.com",
            "a@b@c",
        ] {
            assert_eq!(normalize_email(bad), None, "{bad}");
        }
        let list = Allowlist::parse("Alice@Example.com, ,bob@example.com");
        assert!(list.admits("alice@example.com"));
        assert_eq!(list.len(), 2);
        assert!(Allowlist::parse("").is_empty());
    }
}
