//! The rate limits of §26. Sign-in requests count per client IP in an outer
//! layer, before any handler runs. Sync traffic counts per device once a
//! session names the device, and a session with no card yet counts against
//! its account: requests and pushes before the handler, the bodies of pushes
//! and uploads as soon as they're read, and the responses of reads after the
//! handler. A read checks only that its least cost fits, so reads in flight
//! together can overdraw a budget by their responses.
//! Records on the realtime channel pass through none of this.
//!
//! Counters go through the store's [`Limiter`], so in PostgreSQL mode every
//! instance counts against one allowance.

use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::{HeaderMap, HeaderName, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sync_protocol::types::Id;
use sync_store::{Backend, Error, Limit, Limiter, Refusal, Taken};

use crate::http::{ApiError, AppState};

const MIB: u64 = 1024 * 1024;

/// What a read or upload costs at least, in bytes, so a stream of empty
/// answers can't run forever.
pub const MIN_CHARGE: u64 = 1024;

const fn per_minute(allowance: u64) -> Limit {
    Limit {
        allowance,
        window: Duration::from_secs(60),
    }
}

/// The limits of §26. Sync limits apply per device, or per account for a
/// session with no card yet, and the sign-in limit per client IP.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RateLimits {
    /// Pushes.
    pub pushes: Limit,
    /// Bytes of push bodies.
    pub push_bytes: Limit,
    /// Bytes of blob uploads: tus `PATCH`, and `HEAD` at what a read costs
    /// at least, since a client asks for its offset before each segment.
    pub upload_bytes: Limit,
    /// Bytes of pulls, chain and checkpoint fetches, policy and keys.
    pub read_bytes: Limit,
    /// Bytes of `live`, `manifest` and blob range reads.
    pub bootstrap_bytes: Limit,
    /// Other sync requests.
    pub requests: Limit,
    /// Sign-in requests.
    pub sign_in: Limit,
}

impl Default for RateLimits {
    fn default() -> Self {
        Self {
            pushes: Limit {
                allowance: 10,
                window: Duration::from_secs(1),
            },
            push_bytes: per_minute(64 * MIB),
            upload_bytes: per_minute(512 * MIB),
            read_bytes: per_minute(128 * MIB),
            bootstrap_bytes: per_minute(512 * MIB),
            requests: per_minute(600),
            sign_in: per_minute(30),
        }
    }
}

/// What a sync route counts against.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Traffic {
    Push,
    Upload,
    Read,
    Bootstrap,
    Other,
}

const WORKSPACE: &str = "/v2/workspaces/{w}";

/// The class of `method` on the route template `route`.
fn traffic(method: &Method, route: &str) -> Traffic {
    if route == "/v2/accounts/chains" && *method == Method::POST {
        return Traffic::Read;
    }
    let Some(rest) = route.strip_prefix(WORKSPACE) else {
        return Traffic::Other;
    };
    let domain = rest.strip_prefix("/domains/{d}");
    match (method.as_str(), rest, domain) {
        ("POST", "/ops", _) => Traffic::Push,
        ("PATCH" | "HEAD", "/blobs/{id}", _) => Traffic::Upload,
        ("GET", "/blobs/{id}", _) | ("GET", _, Some("/live" | "/manifest")) => Traffic::Bootstrap,
        ("POST", "/pull", _)
        | ("GET", "/policy" | "/keys" | "/keys/current", _)
        | (
            "GET",
            _,
            Some(
                "/chain/{device}/{counter}"
                | "/heads/{device}"
                | "/checkpoint"
                | "/checkpoints/{digest}"
                | "/checkpoint-chain"
                | "/epochs/{epoch}",
            ),
        ) => Traffic::Read,
        _ => Traffic::Other,
    }
}

/// Whether `method` on `route` is a sign-in request: a device grant, magic
/// link, passkey or reset code.
fn is_sign_in(method: &Method, route: &str) -> bool {
    *method == Method::POST
        && (route.starts_with("/account/passkeys/")
            || matches!(
                route,
                "/v2/device/authorize"
                    | "/v2/device/token"
                    | "/v2/account/reset-code"
                    | "/v2/account/reset"
                    | "/account/email-link"
                    | "/account/email-link/use"
                    | "/account/confirm"
                    | "/device"
                    | "/device/decide"
            ))
}

/// The bytes that stand for `ip` in a counter: an IPv4 address, or the /64
/// of an IPv6 address, which one host can fill alone.
fn address_key(ip: IpAddr) -> Vec<u8> {
    match ip {
        IpAddr::V4(ip) => ip.octets().to_vec(),
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(ip) => ip.octets().to_vec(),
            None => ip.octets()[..8].to_vec(),
        },
    }
}

/// An address in a proxy's header: an IP, with or without a port.
fn parse_address(text: &str) -> Option<IpAddr> {
    let text = text.trim();
    text.parse()
        .ok()
        .or_else(|| text.parse::<SocketAddr>().ok().map(|a| a.ip()))
}

/// What a read's response owes its allowance once its size is known. The
/// session extractors fill it; the layer around the route settles it.
#[derive(Clone, Default)]
pub(crate) struct Meter(Arc<Mutex<Option<Owed>>>);

/// A counter's key and its limit.
type Owed = (Vec<u8>, Limit);

impl Meter {
    fn set(&self, key: Vec<u8>, limit: Limit) {
        *self.0.lock().unwrap_or_else(PoisonError::into_inner) = Some((key, limit));
    }

    fn settle(&self) -> Option<Owed> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

/// The server's rate limits and where it reads the client IP.
#[derive(Clone, Debug)]
pub struct RateGate {
    /// `None` counts nothing, for tests.
    limits: Option<RateLimits>,
    trusted_header: Option<HeaderName>,
}

fn over(what: &str, retry_after: Duration) -> Error {
    let seconds = u64::try_from(retry_after.as_millis().div_ceil(1000)).unwrap_or(u64::MAX);
    Error::Refused(Refusal::new("rate_limited", what).retry_after(seconds.max(1)))
}

fn key(class: &str, subject: &[u8]) -> Vec<u8> {
    [b"rate:", class.as_bytes(), b":", subject].concat()
}

async fn take(limiter: &dyn Limiter, key: &[u8], cost: u64, limit: Limit) -> Result<(), Error> {
    match limiter.take(key, cost, limit).await? {
        Taken::Allowed => Ok(()),
        Taken::Refused { retry_after } => Err(over("over a rate limit", retry_after)),
    }
}

/// Counts `cost` whatever the allowance has left, so a response already
/// sent can overdraw the allowance and hold back the next one.
async fn charge(limiter: &dyn Limiter, key: &[u8], cost: u64, limit: Limit) -> Result<(), Error> {
    let limit = Limit {
        allowance: u64::MAX,
        ..limit
    };
    take(limiter, key, cost, limit).await
}

impl RateGate {
    /// A gate at `limits` that reads the client IP from `trusted_header`
    /// when one is given, and from the connection otherwise.
    pub fn new(limits: RateLimits, trusted_header: Option<HeaderName>) -> Self {
        Self {
            limits: Some(limits),
            trusted_header,
        }
    }

    /// A gate that counts nothing.
    pub fn unlimited() -> Self {
        Self {
            limits: None,
            trusted_header: None,
        }
    }

    /// The client's IP: the last address in the trusted header when one is
    /// configured and holds one, which is the one that proxy added, or
    /// else the connection's peer.
    fn client_ip(&self, headers: &HeaderMap, peer: Option<IpAddr>) -> Option<IpAddr> {
        self.trusted_header
            .as_ref()
            .and_then(|name| headers.get_all(name).iter().next_back())
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.rsplit(',').next())
            .and_then(parse_address)
            .or(peer)
    }

    /// Counts a request of `subject`, a device or an account with no card
    /// yet. A read also gets its response's size counted, through `meter`.
    pub(crate) async fn admit(
        &self,
        limiter: &dyn Limiter,
        request: (&Method, &str),
        subject: Id,
        meter: &Meter,
    ) -> Result<(), ApiError> {
        let Some(limits) = &self.limits else {
            return Ok(());
        };
        let (method, route) = request;
        match traffic(method, route) {
            Traffic::Push => take(limiter, &key("push", &subject.0), 1, limits.pushes).await?,
            Traffic::Upload => {
                let key = key("upload", &subject.0);
                take(limiter, &key, MIN_CHARGE, limits.upload_bytes).await?;
            }
            class @ (Traffic::Read | Traffic::Bootstrap) => {
                let (class, limit) = if class == Traffic::Read {
                    ("read", limits.read_bytes)
                } else {
                    ("bootstrap", limits.bootstrap_bytes)
                };
                let key = key(class, &subject.0);
                take(limiter, &key, MIN_CHARGE, limit).await?;
                meter.set(key, limit);
            }
            Traffic::Other => take(limiter, &key("other", &subject.0), 1, limits.requests).await?,
        }
        Ok(())
    }

    /// Takes a push body of `bytes` from `device`'s push budget.
    pub(crate) async fn take_push_body(
        &self,
        limiter: &dyn Limiter,
        device: Id,
        bytes: u64,
    ) -> Result<(), ApiError> {
        let Some(limits) = &self.limits else {
            return Ok(());
        };
        let key = key("push-bytes", &device.0);
        Ok(take(limiter, &key, bytes, limits.push_bytes).await?)
    }

    /// Takes an upload of `bytes` from `device`'s upload budget, which
    /// [`Self::admit`] already took [`MIN_CHARGE`] from.
    pub(crate) async fn take_upload(
        &self,
        limiter: &dyn Limiter,
        device: Id,
        bytes: u64,
    ) -> Result<(), ApiError> {
        let Some(limits) = &self.limits else {
            return Ok(());
        };
        let key = key("upload", &device.0);
        let rest = bytes.saturating_sub(MIN_CHARGE);
        Ok(take(limiter, &key, rest, limits.upload_bytes).await?)
    }
}

/// How a refused sign-in request reads on `route`.
fn refused_sign_in(route: &str, retry_after: Duration) -> Response {
    if route.starts_with("/v2/") {
        ApiError(over("too many sign-in requests", retry_after)).into_response()
    } else {
        crate::accounts::too_many_sign_ins(route, retry_after)
    }
}

/// The layer around every route: it counts sign-in requests per client IP,
/// offers the session extractors a [`Meter`], and counts what a read
/// returned.
pub(crate) async fn layer<B: Backend>(
    State(state): State<AppState<B>>,
    mut request: Request,
    next: Next,
) -> Response {
    let limiter = state.store.backend().limiter();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(String::new, |p| p.as_str().to_owned());
    if let Some(limits) = &state.rate.limits
        && is_sign_in(request.method(), &route)
    {
        let peer = request
            .extensions()
            .get::<ConnectInfo<SocketAddr>>()
            .map(|ConnectInfo(peer)| peer.ip());
        let ip = state.rate.client_ip(request.headers(), peer);
        let bytes = ip.map(address_key).unwrap_or_default();
        // A keyed hash keeps addresses out of the counters (§29). The
        // account ID function is the server's keyed hash of a string.
        let subject = match state.store.account_id(&format!("rate limit {bytes:?}")) {
            Ok(subject) => subject,
            Err(error) => return ApiError(error).into_response(),
        };
        match limiter
            .take(&key("sign-in", &subject.0), 1, limits.sign_in)
            .await
        {
            Ok(Taken::Allowed) => {}
            Ok(Taken::Refused { retry_after }) => return refused_sign_in(&route, retry_after),
            Err(error) => return ApiError(error).into_response(),
        }
    }
    let meter = Meter::default();
    request.extensions_mut().insert(meter.clone());
    let response = next.run(request).await;
    if let Some((key, limit)) = meter.settle() {
        let size = axum::body::HttpBody::size_hint(response.body())
            .exact()
            .unwrap_or(0);
        let rest = size.saturating_sub(MIN_CHARGE);
        if rest > 0
            && let Err(error) = charge(&*limiter, &key, rest, limit).await
        {
            tracing::warn!("can't count a response: {error}");
        }
    }
    response
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use axum::http::HeaderValue;
    use sync_store::{BoxFut, Result};

    use super::*;

    /// The end of each key's window and what it used.
    type Counts = HashMap<(Vec<u8>, u64), (u64, u64)>;

    /// A limiter on a clock the test moves, so the worked example of §28
    /// runs in no time.
    #[derive(Default)]
    struct VirtualClock {
        now: Mutex<u64>,
        used: Mutex<Counts>,
    }

    impl VirtualClock {
        fn elapsed(&self) -> Duration {
            Duration::from_millis(*self.now.lock().expect("now"))
        }

        fn wait(&self, wait: Duration) {
            *self.now.lock().expect("now") += u64::try_from(wait.as_millis()).expect("millis");
        }
    }

    impl Limiter for VirtualClock {
        fn take<'a>(&'a self, key: &'a [u8], cost: u64, limit: Limit) -> BoxFut<'a, Result<Taken>> {
            Box::pin(async move {
                let now = *self.now.lock().expect("now");
                let ends = limit.window_ends(now);
                let mut used = self.used.lock().expect("used");
                let count = used
                    .entry((key.to_vec(), limit.window_ms()))
                    .or_insert((ends, 0));
                if count.0 != ends {
                    *count = (ends, 0);
                }
                match count.1.checked_add(cost) {
                    Some(sum) if sum <= limit.allowance => {
                        count.1 = sum;
                        Ok(Taken::Allowed)
                    }
                    _ => Ok(Taken::Refused {
                        retry_after: Duration::from_millis(ends - now),
                    }),
                }
            })
        }

        fn prune(&self) -> BoxFut<'_, Result<u64>> {
            Box::pin(async { Ok(0) })
        }
    }

    /// A device that waits out each refusal, as the client does.
    struct Device {
        gate: RateGate,
        clock: VirtualClock,
        id: Id,
    }

    impl Device {
        fn new() -> Self {
            Self {
                gate: RateGate::new(RateLimits::default(), None),
                clock: VirtualClock::default(),
                id: Id([7; 16]),
            }
        }

        /// Waits out a refusal of `attempt`, and tries again.
        async fn until_taken(&self, attempt: impl AsyncFn() -> std::result::Result<(), ApiError>) {
            while let Err(ApiError(error)) = attempt().await {
                let Error::Refused(refusal) = error else {
                    panic!("a store failure");
                };
                let seconds = refusal.body().retry_after_seconds.expect("a wait");
                self.clock.wait(Duration::from_secs(seconds));
            }
        }

        /// A read of `route` that returns `bytes`.
        async fn read(&self, method: Method, route: &str, bytes: u64) {
            let meter = Meter::default();
            self.until_taken(async || {
                self.gate
                    .admit(&self.clock, (&method, route), self.id, &meter)
                    .await
            })
            .await;
            let (key, limit) = meter.settle().expect("a read");
            charge(&self.clock, &key, bytes.saturating_sub(MIN_CHARGE), limit)
                .await
                .map_err(ApiError)
                .ok()
                .expect("counted");
        }

        /// A request with no response worth counting.
        async fn request(&self, method: Method, route: &str) {
            let meter = Meter::default();
            self.until_taken(async || {
                self.gate
                    .admit(&self.clock, (&method, route), self.id, &meter)
                    .await
            })
            .await;
        }

        async fn upload(&self, bytes: u64) {
            let route = "/v2/workspaces/{w}/blobs/{id}";
            let meter = Meter::default();
            self.until_taken(async || {
                self.gate
                    .admit(&self.clock, (&Method::PATCH, route), self.id, &meter)
                    .await
            })
            .await;
            self.until_taken(async || self.gate.take_upload(&self.clock, self.id, bytes).await)
                .await;
        }

        async fn push(&self, bytes: u64) -> std::result::Result<(), ApiError> {
            let route = "/v2/workspaces/{w}/ops";
            let meter = Meter::default();
            self.gate
                .admit(&self.clock, (&Method::POST, route), self.id, &meter)
                .await?;
            self.gate.take_push_body(&self.clock, self.id, bytes).await
        }
    }

    /// One 2 MiB attachment's blob in §28, in segments of at most 1 MiB.
    const BLOB: u64 = 2_163_269;
    const SEGMENTS: [u64; 3] = [MIB, MIB, BLOB - 2 * MIB];
    const ATTACHMENTS: u32 = 5_000;
    const ROUTE_LIVE: &str = "/v2/workspaces/{w}/domains/{d}/live";
    const ROUTE_BLOB: &str = "/v2/workspaces/{w}/blobs/{id}";

    #[tokio::test]
    async fn the_join_of_section_28_finishes_within_the_defaults_in_about_20_minutes() {
        let device = Device::new();
        device
            .read(Method::GET, "/v2/workspaces/{w}/policy", 760_000)
            .await;
        for _ in 0..3 {
            device
                .read(Method::POST, "/v2/accounts/chains", 850_000)
                .await;
        }
        device
            .read(Method::GET, "/v2/workspaces/{w}/keys/current", 2_000)
            .await;
        device
            .read(
                Method::GET,
                "/v2/workspaces/{w}/domains/{d}/epochs/{epoch}",
                107_000,
            )
            .await;
        for _ in 0..339 + 4 {
            device.read(Method::GET, ROUTE_LIVE, MIB).await;
        }
        for _ in 0..ATTACHMENTS {
            for bytes in SEGMENTS {
                device.read(Method::GET, ROUTE_BLOB, bytes).await;
            }
        }
        let minutes = device.clock.elapsed().as_secs_f64() / 60.0;
        assert!(
            (19.0..=22.0).contains(&minutes),
            "the join took {minutes} minutes"
        );
    }

    #[tokio::test]
    async fn the_upload_of_section_28_finishes_within_the_defaults_in_about_20_minutes() {
        let device = Device::new();
        for _ in 0..ATTACHMENTS {
            device
                .request(Method::POST, "/v2/workspaces/{w}/blobs")
                .await;
            for bytes in SEGMENTS {
                device.request(Method::HEAD, ROUTE_BLOB).await;
                device.upload(bytes).await;
            }
        }
        let minutes = device.clock.elapsed().as_secs_f64() / 60.0;
        assert!(
            (19.0..=22.0).contains(&minutes),
            "the upload took {minutes} minutes"
        );
    }

    #[tokio::test]
    async fn uploads_leave_the_push_budget_alone() {
        let device = Device::new();
        for _ in 0..512 {
            device.upload(MIB).await;
        }
        assert!(device.push(MIB).await.is_ok());
    }

    #[test]
    fn every_route_of_the_rate_table_lands_in_its_class() {
        let w = "/v2/workspaces/{w}";
        for (method, route, class) in [
            (Method::POST, format!("{w}/ops"), Traffic::Push),
            (Method::PATCH, format!("{w}/blobs/{{id}}"), Traffic::Upload),
            (Method::GET, format!("{w}/blobs/{{id}}"), Traffic::Bootstrap),
            (
                Method::GET,
                format!("{w}/domains/{{d}}/live"),
                Traffic::Bootstrap,
            ),
            (
                Method::GET,
                format!("{w}/domains/{{d}}/manifest"),
                Traffic::Bootstrap,
            ),
            (Method::POST, format!("{w}/pull"), Traffic::Read),
            (Method::GET, format!("{w}/policy"), Traffic::Read),
            (Method::GET, format!("{w}/keys"), Traffic::Read),
            (Method::GET, format!("{w}/keys/current"), Traffic::Read),
            (
                Method::GET,
                format!("{w}/domains/{{d}}/checkpoint"),
                Traffic::Read,
            ),
            (
                Method::GET,
                format!("{w}/domains/{{d}}/checkpoint-chain"),
                Traffic::Read,
            ),
            (
                Method::GET,
                format!("{w}/domains/{{d}}/checkpoints/{{digest}}"),
                Traffic::Read,
            ),
            (
                Method::GET,
                format!("{w}/domains/{{d}}/chain/{{device}}/{{counter}}"),
                Traffic::Read,
            ),
            (
                Method::GET,
                format!("{w}/domains/{{d}}/heads/{{device}}"),
                Traffic::Read,
            ),
            (
                Method::GET,
                format!("{w}/domains/{{d}}/epochs/{{epoch}}"),
                Traffic::Read,
            ),
            (
                Method::POST,
                "/v2/accounts/chains".to_owned(),
                Traffic::Read,
            ),
            (Method::POST, format!("{w}/blobs"), Traffic::Other),
            (Method::HEAD, format!("{w}/blobs/{{id}}"), Traffic::Upload),
            (Method::GET, format!("{w}/realtime"), Traffic::Other),
            (Method::GET, w.to_owned(), Traffic::Other),
            (Method::GET, "/v2/workspaces".to_owned(), Traffic::Other),
        ] {
            assert_eq!(traffic(&method, &route), class, "{method} {route}");
        }
    }

    #[test]
    fn a_trusted_header_names_the_client_only_when_configured() {
        let peer = Some(IpAddr::from([10, 0, 0, 1]));
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-for",
            HeaderValue::from_static("6.6.6.6, 203.0.113.9"),
        );
        let trusted = RateGate::new(
            RateLimits::default(),
            Some(HeaderName::from_static("x-forwarded-for")),
        );
        assert_eq!(
            trusted.client_ip(&headers, peer),
            Some(IpAddr::from([203, 0, 113, 9]))
        );
        assert_eq!(
            RateGate::new(RateLimits::default(), None).client_ip(&headers, peer),
            peer
        );
        assert_eq!(trusted.client_ip(&HeaderMap::new(), peer), peer);
    }

    #[test]
    fn an_ipv6_host_counts_as_its_64() {
        let one: IpAddr = "2001:db8:1:2::1".parse().expect("an address");
        let two: IpAddr = "2001:db8:1:2:ffff::9".parse().expect("an address");
        let three: IpAddr = "2001:db8:1:3::1".parse().expect("an address");
        assert_eq!(address_key(one), address_key(two));
        assert_ne!(address_key(one), address_key(three));
        let mapped: IpAddr = "::ffff:203.0.113.9".parse().expect("an address");
        assert_eq!(address_key(mapped), vec![203, 0, 113, 9]);
    }
}
