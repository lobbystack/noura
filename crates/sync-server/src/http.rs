//! The HTTP API of §26: the core routes, error bodies with their statuses,
//! and the `Noura-Restore-Generation` header on every response. The server
//! serves no `/v1` route.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, MatchedPath, Path, RawQuery, Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE, RETRY_AFTER};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use sync_protocol::codec::{Bytes, Decode, Encode, Nested, Text};
use sync_protocol::messages::{
    ChainAppendRequest, ChainsRequest, CreateInvitationRequest, CreateShareRequest,
    DeviceTokenRequest, EnrollRequest, ErrorBody, ExpiresAtResponse, LiveCursor, MAX_BODY,
    MEDIA_TYPE, OfferRequest, PullRequest, RESTORE_GENERATION_HEADER, RecoveryRequest, RevealBody,
    RewrapBody, ShareBody,
};
use sync_protocol::types::{Digest, Id, from_base64url, from_decimal};
use sync_store::devices::UNBOUND;
use sync_store::{
    Backend, Error, InvitationCreation, Refusal, Session, SessionRow, ShareCreation, Store,
};

/// A failed request: a refusal the protocol names, or a store failure.
pub struct ApiError(pub Error);

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        Self(error)
    }
}

pub(crate) fn refused(code: &'static str, message: &str) -> ApiError {
    ApiError(Error::Refused(Refusal::new(code, message)))
}

/// The cause of a store failure, carried from the handler to the request
/// log and never sent.
#[derive(Clone)]
pub(crate) struct InternalCause(pub(crate) String);

fn encoded(status: StatusCode, body: Vec<u8>) -> Response {
    let mut response = (status, body).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static(MEDIA_TYPE));
    response
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        match self.0 {
            Error::Refused(refusal) => {
                let status = StatusCode::from_u16(refusal.status())
                    .unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
                let body = refusal.body();
                let wait = body.retry_after_seconds;
                let mut response = encoded(status, body.to_bytes());
                // A `HEAD` carries no body, so its wait goes in the header.
                if let Some(seconds) = wait {
                    response
                        .headers_mut()
                        .insert(RETRY_AFTER, HeaderValue::from(seconds));
                }
                response
            }
            Error::Internal(cause) => {
                let body = ErrorBody {
                    code: Text("internal".into()),
                    message: Text("the server failed to finish the request".into()),
                    retry_after_seconds: None,
                    detail: None::<Bytes>,
                };
                let mut response = encoded(StatusCode::INTERNAL_SERVER_ERROR, body.to_bytes());
                response.extensions_mut().insert(InternalCause(cause));
                response
            }
        }
    }
}

fn ok(value: &impl Encode) -> Response {
    encoded(StatusCode::OK, value.to_bytes())
}

/// A route's path segments. A segment that doesn't percent-decode to
/// UTF-8 gets `bad_request` with the error body of §26, like any other
/// malformed segment.
pub struct Segments<T>(pub T);

impl<T, S> FromRequestParts<S> for Segments<T>
where
    Path<T>: FromRequestParts<S>,
    S: Send + Sync,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|Path(segments)| Segments(segments))
            .map_err(|_| refused("bad_request", "a path segment isn't UTF-8"))
    }
}

/// The signed-in device a bearer token names.
pub struct Auth(pub Session);

/// The signed-in device a bearer token names, with the token's hash.
pub struct AuthWithHash(pub Session, pub Digest);

/// The signed-in device a bearer token names, also when a link removed its
/// card: such a session keeps the right to read its account's chains (§7).
pub struct ChainReader(pub SessionRow);

/// A session from the device grant before its device enrolled, with the
/// token's hash: only the challenge and enrollment routes take one, so a
/// device code nobody enrolled reads nothing (§7).
pub struct Enrolling(pub Session, pub Digest);

/// Counts a request of `subject` against its rate limits (§26).
async fn admit<B: Backend>(
    parts: &Parts,
    state: &AppState<B>,
    subject: Id,
) -> Result<(), ApiError> {
    let (Some(route), Some(meter)) = (
        parts.extensions.get::<MatchedPath>(),
        parts.extensions.get::<crate::rate::Meter>(),
    ) else {
        return Ok(());
    };
    let limiter = state.store.backend().limiter();
    state
        .rate
        .admit(&*limiter, (&parts.method, route.as_str()), subject, meter)
        .await
}

/// Refuses a session that holds no card yet.
fn bound(session: Session) -> Result<Session, ApiError> {
    if session.device == UNBOUND {
        return Err(refused(
            "forbidden",
            "enroll this device before the session reads or writes anything",
        ));
    }
    Ok(session)
}

impl<B: Backend> FromRequestParts<AppState<B>> for ChainReader {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState<B>,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts)?;
        let row = state.store.session_row(token).await?.0;
        bound(row.session)?;
        admit(parts, state, row.session.device).await?;
        Ok(ChainReader(row))
    }
}

pub(crate) fn bearer(parts: &Parts) -> Result<&str, ApiError> {
    parts
        .headers
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .ok_or_else(|| refused("unauthorized", "no valid session"))
}

impl<B: Backend> FromRequestParts<AppState<B>> for Auth {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState<B>,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts)?;
        let session = bound(state.store.session(token).await?)?;
        admit(parts, state, session.device).await?;
        Ok(Auth(session))
    }
}

impl<B: Backend> FromRequestParts<AppState<B>> for Enrolling {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState<B>,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts)?;
        let (session, hash) = state.store.session_and_hash(token).await?;
        // A session with no card yet counts against its account.
        let subject = if session.device == UNBOUND {
            session.account
        } else {
            session.device
        };
        admit(parts, state, subject).await?;
        Ok(Enrolling(session, hash))
    }
}

impl<B: Backend> FromRequestParts<AppState<B>> for AuthWithHash {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState<B>,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts)?;
        let (session, hash) = state.store.session_and_hash(token).await?;
        let session = bound(session)?;
        admit(parts, state, session.device).await?;
        Ok(AuthWithHash(session, hash))
    }
}

/// What every route reads: the store, and the account pages' settings.
pub struct AppState<B: Backend> {
    /// The store.
    pub store: Arc<Store<B>>,
    /// The account pages, sign-in and mail.
    pub accounts: Arc<crate::accounts::Accounts>,
    /// Where blob segments live (§21).
    pub blobs: Arc<dyn crate::blobs::SegmentStore>,
    /// The rate limits (§26).
    pub rate: Arc<crate::rate::RateGate>,
}

impl<B: Backend> Clone for AppState<B> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            accounts: self.accounts.clone(),
            blobs: self.blobs.clone(),
            rate: self.rate.clone(),
        }
    }
}

impl<B: Backend> axum::extract::FromRef<AppState<B>> for Arc<Store<B>> {
    fn from_ref(state: &AppState<B>) -> Self {
        state.store.clone()
    }
}

async fn body(body: Body) -> Result<Vec<u8>, ApiError> {
    axum::body::to_bytes(body, MAX_BODY)
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|error| {
            if crate::listen::is_late(&error) {
                late_body()
            } else {
                refused("too_large", "the body exceeds 1 MiB")
            }
        })
}

/// A body that didn't arrive in time. The server committed nothing and the
/// client sends it again later.
pub(crate) fn late_body() -> ApiError {
    ApiError(Error::internal("a request body took too long to arrive"))
}

fn decode<T: Decode>(bytes: &[u8]) -> Result<T, ApiError> {
    T::from_bytes(bytes).map_err(|e| {
        refused(
            e.kind.server_code(),
            &format!("the body doesn't decode: {e}"),
        )
    })
}

pub(crate) fn id(text: &str) -> Result<Id, ApiError> {
    Id::from_base64url(text)
        .ok()
        .filter(|id| !id.is_zero())
        .ok_or_else(|| refused("bad_request", "an ID isn't 22 characters of base64url"))
}

fn number(text: &str) -> Result<u64, ApiError> {
    from_decimal(text).map_err(|_| refused("bad_request", "a number isn't unsigned decimal"))
}

/// A query's parameters. A repeated name is a bad request.
fn query(raw: Option<&str>) -> Result<HashMap<&str, &str>, ApiError> {
    let mut parameters = HashMap::new();
    for pair in raw.unwrap_or("").split('&').filter(|p| !p.is_empty()) {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        if parameters.insert(name, value).is_some() {
            return Err(refused("bad_request", "a query names a parameter twice"));
        }
    }
    Ok(parameters)
}

async fn limits<B: Backend>(State(store): State<Arc<Store<B>>>) -> Response {
    ok(&store.limits())
}

/// `GET /v2/ready`: whether this instance's store takes writes.
async fn ready<B: Backend>(State(store): State<Arc<Store<B>>>) -> Result<StatusCode, ApiError> {
    if store.ready().await {
        Ok(StatusCode::OK)
    } else {
        Err(ApiError(Error::internal("the store takes no write")))
    }
}

async fn list_workspaces<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
) -> Result<Response, ApiError> {
    Ok(ok(&store.workspaces(session).await?))
}

async fn create_workspace<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    request: Body,
) -> Result<Response, ApiError> {
    let bytes = body(request).await?;
    Ok(ok(&store.create_workspace(session, bytes).await?))
}

async fn policy<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let parameters = query(raw.as_deref())?;
    let after = parameters.get("after").map_or(Ok(0), |a| number(a))?;
    Ok(ok(&store.policy(session, id(&workspace)?, after).await?))
}

async fn keys<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let parameters = query(raw.as_deref())?;
    let after = parameters.get("after").map_or(Ok(0), |a| number(a))?;
    let recipient = parameters
        .get("recipient")
        .map_or(Ok(session.device), |r| id(r))?;
    Ok(ok(&store
        .keys(session, id(&workspace)?, after, recipient)
        .await?))
}

async fn current_keys<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let parameters = query(raw.as_deref())?;
    let recipient = parameters
        .get("recipient")
        .map_or(Ok(session.device), |r| id(r))?;
    Ok(ok(&store
        .current_keys(session, id(&workspace)?, recipient)
        .await?))
}

async fn rewrap<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, domain)): Segments<(String, String)>,
    request: Body,
) -> Result<Response, ApiError> {
    let request: RewrapBody = decode(&body(request).await?)?;
    store
        .rewrap(session, id(&workspace)?, id(&domain)?, request.epoch)
        .await?;
    Ok(empty())
}

async fn epochs<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, domain, epoch)): Segments<(String, String, String)>,
) -> Result<Response, ApiError> {
    Ok(ok(&store
        .epochs(session, id(&workspace)?, id(&domain)?, number(&epoch)?)
        .await?))
}

async fn push<B: Backend>(
    State(state): State<AppState<B>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
    request: Body,
) -> Result<Response, ApiError> {
    let workspace = id(&workspace)?;
    let bytes = body(request).await?;
    let limiter = state.store.backend().limiter();
    state
        .rate
        .take_push_body(&*limiter, session.device, bytes.len() as u64)
        .await?;
    Ok(ok(&state.store.push(session, workspace, bytes).await?))
}

async fn pull<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
    request: Body,
) -> Result<Response, ApiError> {
    let workspace = id(&workspace)?;
    let request: PullRequest = decode(&body(request).await?)?;
    Ok(ok(&store.pull(session, workspace, request).await?))
}

async fn chain<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, domain, device, counter)): Segments<(String, String, String, String)>,
) -> Result<Response, ApiError> {
    Ok(ok(&store
        .chain_record(
            session,
            id(&workspace)?,
            id(&domain)?,
            id(&device)?,
            number(&counter)?,
        )
        .await?))
}

async fn heads<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, domain, device)): Segments<(String, String, String)>,
) -> Result<Response, ApiError> {
    Ok(ok(&store
        .chain_head(session, id(&workspace)?, id(&domain)?, id(&device)?)
        .await?))
}

async fn checkpoint<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, domain)): Segments<(String, String)>,
) -> Result<Response, ApiError> {
    Ok(ok(&store
        .checkpoint(session, id(&workspace)?, id(&domain)?)
        .await?))
}

async fn checkpoint_by_digest<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, domain, checkpoint)): Segments<(String, String, String)>,
) -> Result<Response, ApiError> {
    Ok(ok(&store
        .checkpoint_by_digest(session, id(&workspace)?, id(&domain)?, digest(&checkpoint)?)
        .await?))
}

/// A required query parameter.
fn required<'a>(parameters: &HashMap<&str, &'a str>, name: &str) -> Result<&'a str, ApiError> {
    parameters
        .get(name)
        .copied()
        .ok_or_else(|| refused("bad_request", "a required query parameter is missing"))
}

fn live_cursor(parameters: &HashMap<&str, &str>) -> Result<Option<LiveCursor>, ApiError> {
    parameters
        .get("after")
        .map(|text| {
            LiveCursor::from_query(text)
                .map_err(|_| refused("bad_request", "after isn't a live_cursor"))
        })
        .transpose()
}

async fn checkpoint_chain<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, domain)): Segments<(String, String)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let parameters = query(raw.as_deref())?;
    let from = digest(required(&parameters, "from")?)?;
    let to = digest(required(&parameters, "to")?)?;
    Ok(ok(&store
        .checkpoint_chain(session, id(&workspace)?, id(&domain)?, from, to)
        .await?))
}

async fn live<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, domain)): Segments<(String, String)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let parameters = query(raw.as_deref())?;
    let at = number(required(&parameters, "at")?)?;
    let rid = parameters.get("rid").map(|r| id(r)).transpose()?;
    let after = live_cursor(&parameters)?;
    Ok(ok(&store
        .live(session, id(&workspace)?, id(&domain)?, at, rid, after)
        .await?))
}

async fn manifest<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, domain)): Segments<(String, String)>,
    RawQuery(raw): RawQuery,
) -> Result<Response, ApiError> {
    let parameters = query(raw.as_deref())?;
    let (workspace, domain) = (id(&workspace)?, id(&domain)?);
    let at = number(required(&parameters, "at")?)?;
    if let Some(buckets) = parameters.get("buckets") {
        if *buckets != "all" {
            return Err(refused("bad_request", "buckets takes only all"));
        }
        return Ok(ok(&store
            .manifest_buckets(session, workspace, domain, at)
            .await?));
    }
    let level = u8::try_from(number(required(&parameters, "level")?)?)
        .map_err(|_| refused("bad_request", "level is 0 to 3"))?;
    let index = u16::try_from(number(required(&parameters, "index")?)?)
        .map_err(|_| refused("bad_request", "no such manifest node"))?;
    if level == 3 {
        let after = live_cursor(&parameters)?;
        return Ok(ok(&store
            .manifest_leaves(session, workspace, domain, at, index, after)
            .await?));
    }
    Ok(ok(&store
        .manifest_node(session, workspace, domain, at, level, index)
        .await?))
}

async fn append_chain<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    request: Body,
) -> Result<Response, ApiError> {
    let request: ChainAppendRequest = decode(&body(request).await?)?;
    let links = request.links.0.into_iter().map(|link| link.0).collect();
    Ok(ok(&store.append_chain(session, links).await?))
}

async fn read_chains<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    ChainReader(row): ChainReader,
    request: Body,
) -> Result<Response, ApiError> {
    let request: ChainsRequest = decode(&body(request).await?)?;
    Ok(ok(&store.read_chains_as(row, request).await?))
}

async fn challenge<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Enrolling(session, _): Enrolling,
) -> Result<Response, ApiError> {
    Ok(ok(&store.challenge(session).await?))
}

async fn recovery<B: Backend>(
    State(state): State<AppState<B>>,
    AuthWithHash(session, hash): AuthWithHash,
    request: Body,
) -> Result<Response, ApiError> {
    let request: RecoveryRequest = decode(&body(request).await?)?;
    state
        .store
        .prove_recovery(
            session,
            hash,
            state.accounts.origin().as_str(),
            request.recovery_proof.0,
        )
        .await?;
    Ok(encoded(StatusCode::OK, Vec::new()))
}

fn empty() -> Response {
    encoded(StatusCode::OK, Vec::new())
}

fn digest(text: &str) -> Result<Digest, ApiError> {
    Digest::from_base64url(text)
        .map_err(|_| refused("bad_request", "a digest isn't 43 characters of base64url"))
}

async fn device_authorize<B: Backend>(
    State(state): State<AppState<B>>,
) -> Result<Response, ApiError> {
    let uri = format!("{}/device", state.accounts.origin().as_str());
    Ok(ok(&state.store.authorize_device(&uri).await?))
}

async fn device_token<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    request: Body,
) -> Result<Response, ApiError> {
    let request: DeviceTokenRequest = decode(&body(request).await?)?;
    Ok(ok(&store
        .poll_device_token(request.device_code.as_str())
        .await?))
}

async fn enroll<B: Backend>(
    State(state): State<AppState<B>>,
    Enrolling(session, hash): Enrolling,
    request: Body,
) -> Result<Response, ApiError> {
    let request: EnrollRequest = decode(&body(request).await?)?;
    state
        .store
        .enroll_device(
            session,
            hash,
            state.accounts.origin().as_str(),
            request.enroll.0.to_bytes(),
        )
        .await?;
    Ok(empty())
}

async fn devices<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
) -> Result<Response, ApiError> {
    Ok(ok(&store.devices(session).await?))
}

async fn end_device<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(device): Segments<String>,
) -> Result<Response, ApiError> {
    store.end_device_sessions(session, id(&device)?).await?;
    Ok(empty())
}

async fn post_approval_request<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    request: Body,
) -> Result<Response, ApiError> {
    store
        .post_approval_request(session, body(request).await?)
        .await?;
    Ok(empty())
}

async fn approval_requests<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
) -> Result<Response, ApiError> {
    Ok(ok(&store.own_approval_requests(session).await?))
}

async fn post_offer<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    request: Body,
) -> Result<Response, ApiError> {
    let request: OfferRequest = decode(&body(request).await?)?;
    store.post_offer(session, request.offer.0).await?;
    Ok(empty())
}

async fn offers<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
) -> Result<Response, ApiError> {
    Ok(ok(&store.offers(session).await?))
}

async fn reveal<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(offer): Segments<String>,
    request: Body,
) -> Result<Response, ApiError> {
    let request: RevealBody = decode(&body(request).await?)?;
    store.reveal(session, digest(&offer)?, request.nc).await?;
    Ok(empty())
}

async fn read_reveal<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(offer): Segments<String>,
) -> Result<Response, ApiError> {
    Ok(ok(&store.read_reveal(session, digest(&offer)?).await?))
}

async fn create_share<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
    request: Body,
) -> Result<Response, ApiError> {
    let workspace = id(&workspace)?;
    let request: CreateShareRequest = decode(&body(request).await?)?;
    let expires_at = store
        .create_share(
            session,
            workspace,
            ShareCreation {
                token_hash: request.token_hash,
                domain: request.domain,
                expires_in_days: request.expires_in_days,
                share: request.share.0,
            },
        )
        .await?;
    Ok(ok(&ExpiresAtResponse { expires_at }))
}

async fn list_shares<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
) -> Result<Response, ApiError> {
    Ok(ok(&store.shares(session, id(&workspace)?).await?))
}

async fn update_share<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, token_hash)): Segments<(String, String)>,
    request: Body,
) -> Result<Response, ApiError> {
    let workspace = id(&workspace)?;
    let token_hash = digest(&token_hash)?;
    let request: ShareBody = decode(&body(request).await?)?;
    store
        .update_share(session, workspace, token_hash, request.share.0)
        .await?;
    Ok(empty())
}

async fn revoke_share<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, token_hash)): Segments<(String, String)>,
) -> Result<Response, ApiError> {
    store
        .revoke_share(session, id(&workspace)?, digest(&token_hash)?)
        .await?;
    Ok(empty())
}

/// `GET /v2/shares/{token}`, which takes no session: the token alone names
/// the link, and the snapshot reads only with the fragment's keys.
async fn read_share<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Segments(token): Segments<String>,
) -> Result<Response, ApiError> {
    let token =
        from_base64url::<16>(&token).map_err(|_| refused("not_found", "no such share link"))?;
    let share = store.read_share(token).await?;
    Ok(ok(&ShareBody {
        share: Nested(share),
    }))
}

async fn create_invitation<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
    request: Body,
) -> Result<Response, ApiError> {
    let workspace = id(&workspace)?;
    let request: CreateInvitationRequest = decode(&body(request).await?)?;
    let expires_at = store
        .create_invitation(
            session,
            workspace,
            InvitationCreation {
                invitation: request.invitation,
                token_hash: request.token_hash,
                role: request.role,
                invite_context: request.invite_context.0,
            },
        )
        .await?;
    Ok(ok(&ExpiresAtResponse { expires_at }))
}

async fn list_invitations<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
) -> Result<Response, ApiError> {
    Ok(ok(&store.invitations(session, id(&workspace)?).await?))
}

async fn revoke_invitation<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments((workspace, invitation)): Segments<(String, String)>,
) -> Result<Response, ApiError> {
    store
        .revoke_invitation(session, id(&workspace)?, id(&invitation)?)
        .await?;
    Ok(empty())
}

fn invite_token(token: &str) -> Result<[u8; 16], ApiError> {
    from_base64url::<16>(token).map_err(|_| {
        refused(
            "invitation_invalid",
            "the invitation is revoked, expired, used or unknown",
        )
    })
}

/// `GET /v2/invitations/{token}`, which takes no session: the token alone
/// names the invitation, and its context opens only with the fragment's
/// secret.
async fn read_invitation<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Segments(token): Segments<String>,
) -> Result<Response, ApiError> {
    Ok(ok(&store.invitation_context(invite_token(&token)?).await?))
}

async fn accept_invitation<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(token): Segments<String>,
) -> Result<Response, ApiError> {
    store
        .accept_invitation(session, invite_token(&token)?)
        .await?;
    Ok(empty())
}

async fn pending_devices<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
) -> Result<Response, ApiError> {
    Ok(ok(&store.pending_devices(session, id(&workspace)?).await?))
}

async fn missing_blobs<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
) -> Result<Response, ApiError> {
    Ok(ok(&store.missing_blobs(session, id(&workspace)?).await?))
}

async fn close_workspace<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
) -> Result<Response, ApiError> {
    store.close_workspace(session, id(&workspace)?).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn account_reset<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    request: Body,
) -> Result<Response, ApiError> {
    let mut key = b"account-reset:".to_vec();
    key.extend_from_slice(&session.account.0);
    let limiter = store.backend().limiter();
    if let sync_store::Taken::Refused { retry_after } =
        sync_store::Limiter::take(&*limiter, &key, 1, crate::accounts::RESET_ATTEMPTS).await?
    {
        return Err(crate::accounts::too_many_resets(retry_after));
    }
    let request = decode(&body(request).await?)?;
    store.reset_account(session, request).await?;
    Ok(StatusCode::NO_CONTENT.into_response())
}

async fn not_found() -> ApiError {
    refused("not_found", "no such route")
}

const READY_ROUTE: &str = "/v2/ready";

fn request_id() -> String {
    let bytes: [u8; 8] = sync_protocol::crypto::random_array().unwrap_or_default();
    sync_protocol::types::to_hex(&bytes)
}

/// Stamps the restore generation on every response and logs each request
/// by its route template, request ID and status, plus the cause of a store
/// failure.
async fn stamp_and_log(generation: u64, request: Request, next: Next) -> Response {
    let id = request_id();
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or_else(|| "unmatched".to_owned(), |p| p.as_str().to_owned());
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(RESTORE_GENERATION_HEADER, HeaderValue::from(generation));
    if let Some(InternalCause(cause)) = response.extensions_mut().remove::<InternalCause>() {
        tracing::error!(request = %id, route = %route, cause = %cause, "store failure");
    }
    // A health check every few seconds would fill the log with nothing.
    if route != READY_ROUTE || !response.status().is_success() {
        tracing::info!(request = %id, route = %route, status = response.status().as_u16(), "request");
    }
    response
}

/// The routes of §26 that the server serves so far, and the account pages.
pub fn router<B: Backend>(state: AppState<B>) -> Router {
    let generation = state.store.restore_generation();
    let gate = state.clone();
    Router::new()
        .route("/v2/limits", get(limits::<B>))
        .route("/v2/ready", get(ready::<B>))
        .route("/v2/accounts/chain", post(append_chain::<B>))
        .route("/v2/accounts/chains", post(read_chains::<B>))
        .route("/v2/devices/challenge", post(challenge::<B>))
        .route("/v2/device/authorize", post(device_authorize::<B>))
        .route("/v2/device/token", post(device_token::<B>))
        .route("/v2/devices", get(devices::<B>).post(enroll::<B>))
        .route(
            "/v2/devices/{device}",
            axum::routing::delete(end_device::<B>),
        )
        .route(
            "/v2/approval-requests",
            get(approval_requests::<B>).post(post_approval_request::<B>),
        )
        .route("/v2/approvals", get(offers::<B>).post(post_offer::<B>))
        .route(
            "/v2/approvals/{offer}/reveal",
            get(read_reveal::<B>).post(reveal::<B>),
        )
        .route("/v2/recovery", post(recovery::<B>))
        .merge(crate::accounts::routes::<B>())
        .merge(crate::tus::routes::<B>())
        .route(
            "/v2/workspaces",
            get(list_workspaces::<B>).post(create_workspace::<B>),
        )
        .route(
            "/v2/workspaces/{w}",
            axum::routing::delete(close_workspace::<B>),
        )
        .route("/v2/account/reset", post(account_reset::<B>))
        .route("/v2/workspaces/{w}/policy", get(policy::<B>))
        .route("/v2/workspaces/{w}/keys", get(keys::<B>))
        .route("/v2/workspaces/{w}/keys/current", get(current_keys::<B>))
        .route("/v2/workspaces/{w}/domains/{d}/rewrap", post(rewrap::<B>))
        .route("/v2/workspaces/{w}/ops", post(push::<B>))
        .route(
            "/v2/workspaces/{w}/shares",
            get(list_shares::<B>).post(create_share::<B>),
        )
        .route(
            "/v2/workspaces/{w}/shares/{token_hash}",
            axum::routing::put(update_share::<B>).delete(revoke_share::<B>),
        )
        .route("/v2/shares/{token}", get(read_share::<B>))
        .route(
            "/v2/workspaces/{w}/invitations",
            get(list_invitations::<B>).post(create_invitation::<B>),
        )
        .route(
            "/v2/workspaces/{w}/invitations/{id}",
            axum::routing::delete(revoke_invitation::<B>),
        )
        .route("/v2/invitations/{token}", get(read_invitation::<B>))
        .route(
            "/v2/invitations/{token}/accept",
            post(accept_invitation::<B>),
        )
        .route(
            "/v2/workspaces/{w}/pending-devices",
            get(pending_devices::<B>),
        )
        .route("/v2/workspaces/{w}/missing-blobs", get(missing_blobs::<B>))
        .route("/v2/workspaces/{w}/pull", post(pull::<B>))
        .route(
            "/v2/workspaces/{w}/realtime",
            get(crate::realtime::open::<B>),
        )
        .route(
            "/v2/workspaces/{w}/domains/{d}/epochs/{epoch}",
            get(epochs::<B>),
        )
        .route(
            "/v2/workspaces/{w}/domains/{d}/chain/{device}/{counter}",
            get(chain::<B>),
        )
        .route(
            "/v2/workspaces/{w}/domains/{d}/heads/{device}",
            get(heads::<B>),
        )
        .route(
            "/v2/workspaces/{w}/domains/{d}/checkpoint",
            get(checkpoint::<B>),
        )
        .route(
            "/v2/workspaces/{w}/domains/{d}/checkpoints/{digest}",
            get(checkpoint_by_digest::<B>),
        )
        .route(
            "/v2/workspaces/{w}/domains/{d}/checkpoint-chain",
            get(checkpoint_chain::<B>),
        )
        .route("/v2/workspaces/{w}/domains/{d}/live", get(live::<B>))
        .route(
            "/v2/workspaces/{w}/domains/{d}/manifest",
            get(manifest::<B>),
        )
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .with_state(state)
        .layer(middleware::from_fn_with_state(
            gate,
            crate::rate::layer::<B>,
        ))
        .layer(middleware::from_fn(move |request: Request, next: Next| {
            stamp_and_log(generation, request, next)
        }))
}
