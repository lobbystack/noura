//! Signature checks of the Standard Webhooks specification
//! (<https://github.com/standard-webhooks/standard-webhooks/blob/main/spec/standard-webhooks.md>),
//! which Polar signs its deliveries under: `webhook-id`, `webhook-timestamp`
//! and `webhook-signature` headers, and an HMAC-SHA256 of
//! `{id}.{timestamp}.{body}` per signature, base64-encoded after `v1,`.

use axum::http::HeaderMap;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, Mac};
use sha2::Sha256;

/// How far a delivery's timestamp may lie from the server's clock.
pub const TOLERANCE_SECONDS: u64 = 5 * 60;

/// Why a delivery fails its check.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum Rejection {
    /// A header is missing or doesn't parse.
    #[error("a webhook header is missing or malformed")]
    Headers,
    /// The timestamp lies more than [`TOLERANCE_SECONDS`] from the clock.
    #[error("the webhook's timestamp is outside the tolerance")]
    Stale,
    /// No signature matches.
    #[error("no webhook signature matches")]
    Signature,
}

/// A webhook secret's keys. A Standard Webhooks secret is `whsec_` followed
/// by the key in base64. Polar's endpoints made before 8 September 2026 key
/// the HMAC with the UTF-8 bytes of the whole secret instead, so the check
/// tries both, as Polar's SDKs do.
#[derive(Clone)]
pub struct Secret {
    keys: Vec<Vec<u8>>,
}

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    /// The keys `text` names.
    pub fn new(text: &str) -> Self {
        let mut keys = Vec::new();
        if let Some(encoded) = text.strip_prefix("whsec_")
            && let Ok(key) = STANDARD.decode(encoded)
            && !key.is_empty()
        {
            keys.push(key);
        }
        keys.push(text.as_bytes().to_vec());
        Self { keys }
    }

    /// The signature `key` gives a delivery, as base64.
    fn sign(key: &[u8], content: &[u8]) -> Option<Hmac<Sha256>> {
        let mut mac = Hmac::<Sha256>::new_from_slice(key).ok()?;
        mac.update(content);
        Some(mac)
    }

    /// The `webhook-signature` header a delivery of `body` gets under this
    /// secret's first key, for tests and fixtures.
    pub fn signature(&self, id: &str, timestamp: u64, body: &[u8]) -> String {
        let content = signed_content(id, timestamp, body);
        let mac = Self::sign(&self.keys[0], &content).expect("HMAC takes any key length");
        format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()))
    }
}

fn signed_content(id: &str, timestamp: u64, body: &[u8]) -> Vec<u8> {
    [
        id.as_bytes(),
        b".",
        timestamp.to_string().as_bytes(),
        b".",
        body,
    ]
    .concat()
}

fn header<'a>(headers: &'a HeaderMap, name: &str) -> Result<&'a str, Rejection> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or(Rejection::Headers)
}

/// Checks a delivery of `body` with `headers` against `secret` at `now`, in
/// Unix seconds, and returns its `webhook-id`, which names the event across
/// retries.
pub fn verify(
    secret: &Secret,
    headers: &HeaderMap,
    body: &[u8],
    now: u64,
) -> Result<String, Rejection> {
    let id = header(headers, "webhook-id")?;
    let timestamp: u64 = header(headers, "webhook-timestamp")?
        .parse()
        .map_err(|_| Rejection::Headers)?;
    if timestamp.abs_diff(now) > TOLERANCE_SECONDS {
        return Err(Rejection::Stale);
    }
    let signatures = header(headers, "webhook-signature")?;
    let content = signed_content(id, timestamp, body);
    for candidate in signatures.split(' ') {
        let Some(encoded) = candidate.strip_prefix("v1,") else {
            continue;
        };
        let Ok(expected) = STANDARD.decode(encoded) else {
            continue;
        };
        for key in &secret.keys {
            // `verify_slice` compares in constant time.
            if let Some(mac) = Secret::sign(key, &content)
                && mac.verify_slice(&expected).is_ok()
            {
                return Ok(id.to_owned());
            }
        }
    }
    Err(Rejection::Signature)
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn headers(id: &str, timestamp: u64, signature: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert(
            "webhook-timestamp",
            HeaderValue::from_str(&timestamp.to_string()).unwrap(),
        );
        headers.insert(
            "webhook-signature",
            HeaderValue::from_str(signature).unwrap(),
        );
        headers
    }

    /// The example of the Standard Webhooks specification: its secret, ID,
    /// timestamp and payload give the signature it shows.
    #[test]
    fn the_specification_example_verifies() {
        let secret = Secret::new("whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw");
        let body = br#"{"test": 2432232314}"#;
        let id = "msg_p5jXN8AQM9LWM0D4loKWxJek";
        let timestamp = 1_614_265_330;
        assert_eq!(
            secret.signature(id, timestamp, body),
            "v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE="
        );
        let headers = headers(
            id,
            timestamp,
            "v1,bm9wZQ== v1,g0hM9SsE+OTPJTGt/tmIKtSyZlE3uFJELVlNIOLJ1OE=",
        );
        assert_eq!(verify(&secret, &headers, body, timestamp), Ok(id.into()));
        assert_eq!(
            verify(&secret, &headers, br#"{"test": 1}"#, timestamp),
            Err(Rejection::Signature)
        );
    }

    #[test]
    fn a_delivery_outside_five_minutes_is_stale() {
        let secret = Secret::new("whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw");
        let body = b"{}";
        let signature = secret.signature("msg", 1_000_000, body);
        let headers = headers("msg", 1_000_000, &signature);
        assert!(verify(&secret, &headers, body, 1_000_000 + 300).is_ok());
        assert_eq!(
            verify(&secret, &headers, body, 1_000_000 + 301),
            Err(Rejection::Stale)
        );
        assert_eq!(
            verify(&secret, &headers, body, 1_000_000 - 301),
            Err(Rejection::Stale)
        );
        let mut missing = headers.clone();
        missing.remove("webhook-id");
        assert_eq!(
            verify(&secret, &missing, body, 1_000_000),
            Err(Rejection::Headers)
        );
    }

    /// An endpoint from before Polar adopted the specification keys the
    /// HMAC with the whole secret's bytes.
    #[test]
    fn a_legacy_polar_secret_verifies_too() {
        let text = "whsec_legacy-polar-secret";
        let body = b"{}";
        let mut mac = Hmac::<Sha256>::new_from_slice(text.as_bytes()).unwrap();
        mac.update(&signed_content("msg", 10, body));
        let signature = format!("v1,{}", STANDARD.encode(mac.finalize().into_bytes()));
        assert!(
            verify(
                &Secret::new(text),
                &headers("msg", 10, &signature),
                body,
                10
            )
            .is_ok()
        );
        let other = Secret::new("whsec_another-secret");
        assert_eq!(
            verify(&other, &headers("msg", 10, &signature), body, 10),
            Err(Rejection::Signature)
        );
    }
}
