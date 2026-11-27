//! The rate limits of §26. Sign-in requests count per client IP in an outer
//! layer, before any handler runs. Sync traffic counts per device once a
//! session names the device, and a session with no card yet counts against
//! its account: requests and pushes before the handler, the bodies of pushes
//! and uploads as soon as they're read, and the responses of reads after the
//! handler. A read checks only that its least cost fits, so reads in flight
//! together can overdraw a budget by their responses.
//! Records on the realtime channel pass through none of this, but the gate
//! holds what limits the channels themselves: how many one device opens and
//! how long a silent one stays.
//!
//! On a server that sells plans, each account also has a daily budget in
//! each direction from its own plan (§26, Transfer budgets): push and
//! upload bodies count up, the responses of reads count down.
//!
//! Counters go through the store's [`Limiter`], so in PostgreSQL mode every
//! instance counts against one allowance.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, MatchedPath, Request, State};
use axum::http::{HeaderMap, HeaderName, Method};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use sync_protocol::types::Id;
use sync_store::{Backend, Error, Limit, Limiter, PlanLimits, Refusal, Store, Taken};

use crate::http::{ApiError, AppState};

const MIB: u64 = 1024 * 1024;

/// What a read or upload costs at least, in bytes, so a stream of empty
/// answers can't run forever.
pub const MIN_CHARGE: u64 = 1024;

/// The length of a transfer budget's window: one UTC day, since windows
/// start at multiples of their length from the Unix epoch.
pub const DAY: Duration = Duration::from_secs(86_400);

/// How long the gate keeps an account's plan before it reads it again.
const PLAN_TTL: Duration = Duration::from_secs(60);

/// Accounts whose plans the gate keeps before it forgets them all.
const PLAN_CACHE: usize = 65_536;

const fn per_day(allowance: u64) -> Limit {
    Limit {
        allowance,
        window: DAY,
    }
}

/// What one account may transfer each UTC day (§26, Transfer budgets).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DailyBudget {
    /// The account.
    pub account: Id,
    /// Bytes it may push and upload.
    pub up: Option<u64>,
    /// Bytes it may read.
    pub down: Option<u64>,
}

impl DailyBudget {
    /// The budget `limits` give `account`.
    pub fn of(account: Id, limits: &PlanLimits) -> Self {
        Self {
            account,
            up: limits.upload_per_day,
            down: limits.download_per_day,
        }
    }
}

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

/// What a read's response owes its allowances once its size is known: the
/// device's and, on a server that sells plans, the account's day. The
/// session extractors fill it; the layer around the route settles it.
#[derive(Clone, Default)]
pub(crate) struct Meter(Arc<Mutex<Vec<Owed>>>);

/// A counter's key and its limit.
type Owed = (Vec<u8>, Limit);

impl Meter {
    fn add(&self, key: Vec<u8>, limit: Limit) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((key, limit));
    }

    fn settle(&self) -> Vec<Owed> {
        std::mem::take(&mut *self.0.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// What holds one realtime channel open (§23).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelLimits {
    /// How many channels one device holds to one workspace on an instance.
    /// A client holds one, and one more while it reconnects.
    pub per_device: usize,
    /// How long a channel stays open while its client sends nothing, not
    /// even a ping.
    pub idle: Duration,
}

impl Default for ChannelLimits {
    fn default() -> Self {
        Self {
            per_device: 8,
            idle: Duration::from_secs(60),
        }
    }
}

/// The channels open on this instance, by device and workspace.
type OpenChannels = Arc<Mutex<HashMap<(Id, Id), usize>>>;

/// One open channel's place in the count, given back when it drops.
pub(crate) struct ChannelSlot {
    open: OpenChannels,
    key: (Id, Id),
}

impl Drop for ChannelSlot {
    fn drop(&mut self) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = open.get_mut(&self.key) {
            *count -= 1;
            if *count == 0 {
                open.remove(&self.key);
            }
        }
    }
}

/// The plans the gate read lately, by account, with when it read each.
type PlanCache = Arc<Mutex<HashMap<Id, (PlanLimits, Instant)>>>;

/// The server's rate limits and where it reads the client IP.
#[derive(Clone, Debug)]
pub struct RateGate {
    /// `None` counts nothing, for tests.
    limits: Option<RateLimits>,
    trusted_header: Option<HeaderName>,
    channels: ChannelLimits,
    open: OpenChannels,
    plans: PlanCache,
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

/// Takes `cost` from the account's day in one direction, when the budget
/// sets one.
async fn take_day(
    limiter: &dyn Limiter,
    direction: &str,
    budget: Option<DailyBudget>,
    cost: u64,
) -> Result<(), Error> {
    let Some(budget) = budget else {
        return Ok(());
    };
    let allowance = if direction == "day-up" {
        budget.up
    } else {
        budget.down
    };
    let Some(allowance) = allowance else {
        return Ok(());
    };
    match limiter
        .take(&key(direction, &budget.account.0), cost, per_day(allowance))
        .await?
    {
        Taken::Allowed => Ok(()),
        Taken::Refused { retry_after } => Err(over("over the day's transfer budget", retry_after)),
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
            channels: ChannelLimits::default(),
            open: OpenChannels::default(),
            plans: PlanCache::default(),
        }
    }

    /// A gate that counts nothing.
    pub fn unlimited() -> Self {
        Self {
            limits: None,
            trusted_header: None,
            channels: ChannelLimits::default(),
            open: OpenChannels::default(),
            plans: PlanCache::default(),
        }
    }

    /// This gate with other limits on realtime channels.
    pub fn with_channels(mut self, channels: ChannelLimits) -> Self {
        self.channels = channels;
        self
    }

    /// How long a silent realtime channel stays open.
    pub(crate) fn channel_idle(&self) -> Duration {
        self.channels.idle
    }

    /// Takes a place for a device's channel to a workspace, or `rate_limited`
    /// when it holds as many as it may.
    pub(crate) fn open_channel(&self, device: Id, workspace: Id) -> Result<ChannelSlot, Error> {
        let key = (device, workspace);
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let count = open.entry(key).or_default();
        if *count >= self.channels.per_device {
            return Err(over(
                "too many realtime channels for this device",
                self.channels.idle,
            ));
        }
        *count += 1;
        Ok(ChannelSlot {
            open: self.open.clone(),
            key,
        })
    }

    /// Counts one presence record of `device` against `per_second`, across
    /// its channels and, in PostgreSQL mode, across instances.
    pub(crate) async fn take_presence(
        limiter: &dyn Limiter,
        device: Id,
        per_second: u32,
    ) -> Result<(), Error> {
        let limit = Limit {
            allowance: u64::from(per_second),
            window: Duration::from_secs(1),
        };
        take(limiter, &key("presence", &device.0), 1, limit).await
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
        budget: Option<DailyBudget>,
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
                take_day(limiter, "day-down", budget, MIN_CHARGE).await?;
                meter.add(key, limit);
                if let Some(budget) = budget
                    && let Some(down) = budget.down
                {
                    meter.add(self::key("day-down", &budget.account.0), per_day(down));
                }
            }
            Traffic::Other => take(limiter, &key("other", &subject.0), 1, limits.requests).await?,
        }
        Ok(())
    }

    /// Takes a push body of `bytes` from `device`'s push budget and from
    /// the account's day.
    pub(crate) async fn take_push_body(
        &self,
        limiter: &dyn Limiter,
        device: Id,
        budget: Option<DailyBudget>,
        bytes: u64,
    ) -> Result<(), ApiError> {
        let Some(limits) = &self.limits else {
            return Ok(());
        };
        let key = key("push-bytes", &device.0);
        take(limiter, &key, bytes, limits.push_bytes).await?;
        Ok(take_day(limiter, "day-up", budget, bytes).await?)
    }

    /// Takes an upload of `bytes` from `device`'s upload budget, which
    /// [`Self::admit`] already took [`MIN_CHARGE`] from, and from the
    /// account's day.
    pub(crate) async fn take_upload(
        &self,
        limiter: &dyn Limiter,
        device: Id,
        budget: Option<DailyBudget>,
        bytes: u64,
    ) -> Result<(), ApiError> {
        let Some(limits) = &self.limits else {
            return Ok(());
        };
        let key = key("upload", &device.0);
        let rest = bytes.saturating_sub(MIN_CHARGE);
        take(limiter, &key, rest, limits.upload_bytes).await?;
        Ok(take_day(limiter, "day-up", budget, bytes).await?)
    }

    /// The daily budget of `account` on a server that sells plans and
    /// counts traffic, from a plan the gate read within the last minute.
    pub(crate) async fn budget<B: Backend>(
        &self,
        store: &Store<B>,
        account: Id,
    ) -> Result<Option<DailyBudget>, Error> {
        if self.limits.is_none() || !store.billing() {
            return Ok(None);
        }
        let now = Instant::now();
        let cached = self
            .plans
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&account)
            .filter(|(_, read)| now.duration_since(*read) < PLAN_TTL)
            .map(|(limits, _)| *limits);
        let limits = match cached {
            Some(limits) => limits,
            None => {
                let limits = store.plan_limits(account).await?;
                let mut plans = self.plans.lock().unwrap_or_else(PoisonError::into_inner);
                if plans.len() >= PLAN_CACHE {
                    plans.clear();
                }
                plans.insert(account, (limits, now));
                limits
            }
        };
        Ok(Some(DailyBudget::of(account, &limits)))
    }

    /// What `account` pushed and uploaded, and read, so far today.
    pub(crate) async fn today(limiter: &dyn Limiter, account: Id) -> Result<(u64, u64), Error> {
        // The window's length, not the allowance, names the counter.
        let up = limiter
            .used(&key("day-up", &account.0), per_day(u64::MAX))
            .await?;
        let down = limiter
            .used(&key("day-down", &account.0), per_day(u64::MAX))
            .await?;
        Ok((up, down))
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
    let owed = meter.settle();
    if !owed.is_empty() {
        let size = axum::body::HttpBody::size_hint(response.body())
            .exact()
            .unwrap_or(0);
        let rest = size.saturating_sub(MIN_CHARGE);
        for (key, limit) in owed {
            if rest > 0
                && let Err(error) = charge(&*limiter, &key, rest, limit).await
            {
                tracing::warn!("can't count a response: {error}");
            }
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

        fn used<'a>(&'a self, key: &'a [u8], limit: Limit) -> BoxFut<'a, Result<u64>> {
            Box::pin(async move {
                let now = *self.now.lock().expect("now");
                let ends = limit.window_ends(now);
                Ok(self
                    .used
                    .lock()
                    .expect("used")
                    .get(&(key.to_vec(), limit.window_ms()))
                    .filter(|count| count.0 == ends)
                    .map_or(0, |count| count.1))
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
        budget: Option<DailyBudget>,
    }

    impl Device {
        fn new() -> Self {
            Self {
                gate: RateGate::new(RateLimits::default(), None),
                clock: VirtualClock::default(),
                id: Id([7; 16]),
                budget: None,
            }
        }

        /// A device of an account with `limits`' daily budget.
        fn on_plan(limits: &PlanLimits) -> Self {
            Self {
                budget: Some(DailyBudget::of(Id([8; 16]), limits)),
                ..Self::new()
            }
        }

        /// How long this device has waited so far.
        fn waited(&self) -> Duration {
            self.clock.elapsed()
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
                    .admit(&self.clock, (&method, route), self.id, self.budget, &meter)
                    .await
            })
            .await;
            let owed = meter.settle();
            assert!(!owed.is_empty(), "a read");
            for (key, limit) in owed {
                charge(&self.clock, &key, bytes.saturating_sub(MIN_CHARGE), limit)
                    .await
                    .map_err(ApiError)
                    .ok()
                    .expect("counted");
            }
        }

        /// A request with no response worth counting.
        async fn request(&self, method: Method, route: &str) {
            let meter = Meter::default();
            self.until_taken(async || {
                self.gate
                    .admit(&self.clock, (&method, route), self.id, self.budget, &meter)
                    .await
            })
            .await;
        }

        async fn upload(&self, bytes: u64) {
            let route = "/v2/workspaces/{w}/blobs/{id}";
            let meter = Meter::default();
            self.until_taken(async || {
                self.gate
                    .admit(
                        &self.clock,
                        (&Method::PATCH, route),
                        self.id,
                        self.budget,
                        &meter,
                    )
                    .await
            })
            .await;
            self.until_taken(async || {
                self.gate
                    .take_upload(&self.clock, self.id, self.budget, bytes)
                    .await
            })
            .await;
        }

        async fn push(&self, bytes: u64) -> std::result::Result<(), ApiError> {
            let route = "/v2/workspaces/{w}/ops";
            let meter = Meter::default();
            self.gate
                .admit(
                    &self.clock,
                    (&Method::POST, route),
                    self.id,
                    self.budget,
                    &meter,
                )
                .await?;
            self.gate
                .take_push_body(&self.clock, self.id, self.budget, bytes)
                .await
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

    #[tokio::test]
    async fn a_free_account_uploads_a_gigabyte_a_day() {
        let free = sync_store::plans::limits(sync_store::plans::FREE).expect("free");
        let device = Device::on_plan(&free);
        // 953 segments of 1 MiB fit 1 GB, and the 954th waits for the next
        // UTC day.
        for _ in 0..953 {
            device.upload(MIB).await;
        }
        assert!(device.waited() < DAY);
        device.upload(MIB).await;
        assert!(device.waited() >= DAY - Duration::from_secs(60));
        let (up, _) = RateGate::today(&device.clock, Id([8; 16]))
            .await
            .expect("today");
        assert_eq!(up, MIB);
    }

    #[tokio::test]
    async fn reads_count_down_against_the_day_and_pushes_up() {
        let device = Device {
            budget: Some(DailyBudget {
                account: Id([8; 16]),
                up: Some(128 * 1024),
                down: Some(10 * MIB),
            }),
            ..Device::new()
        };
        device
            .read(Method::GET, "/v2/workspaces/{w}/blobs/{id}", MIB)
            .await;
        assert!(device.push(64 * 1024).await.is_ok(), "a push");
        let (up, down) = RateGate::today(&device.clock, Id([8; 16]))
            .await
            .expect("today");
        assert_eq!((up, down), (64 * 1024, MIB));
        // A push over the day's budget gets `rate_limited` until midnight.
        let Err(ApiError(Error::Refused(refusal))) = device.push(128 * 1024).await else {
            panic!("refused");
        };
        assert_eq!(refusal.code, "rate_limited");
        let wait = refusal.retry_after_seconds.expect("a wait");
        assert!(wait > 60 && wait <= DAY.as_secs(), "{wait}");
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
