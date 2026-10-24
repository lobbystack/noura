//! The HTTP API of §26: the core routes, error bodies with their statuses,
//! and the `Noura-Restore-Generation` header on every response. The server
//! serves no `/v1` route.

use std::collections::HashMap;
use std::sync::Arc;

use axum::Router;
use axum::body::Body;
use axum::extract::{FromRequestParts, MatchedPath, Path, RawQuery, Request, State};
use axum::http::header::{AUTHORIZATION, CONTENT_TYPE};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use sync_protocol::codec::{Bytes, Decode, Encode, Text};
use sync_protocol::messages::{
    ChainAppendRequest, ChainsRequest, DeviceTokenRequest, EnrollRequest, ErrorBody, MAX_BODY,
    MEDIA_TYPE, OfferRequest, PullRequest, RESTORE_GENERATION_HEADER, RecoveryRequest, RevealBody,
    RewrapBody,
};
use sync_protocol::types::{Digest, Id, from_decimal};
use sync_store::{Backend, Error, Refusal, Session, Store};

/// A failed request: a refusal the protocol names, or a store failure.
pub struct ApiError(pub Error);

impl From<Error> for ApiError {
    fn from(error: Error) -> Self {
        Self(error)
    }
}

fn refused(code: &'static str, message: &str) -> ApiError {
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
                encoded(status, refusal.body().to_bytes())
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
pub struct ChainReader(pub Session);

impl<B: Backend> FromRequestParts<AppState<B>> for ChainReader {
    type Rejection = ApiError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState<B>,
    ) -> Result<Self, Self::Rejection> {
        let token = bearer(parts)?;
        Ok(ChainReader(
            state.store.session_reading_chains(token).await?,
        ))
    }
}

fn bearer(parts: &Parts) -> Result<&str, ApiError> {
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
        Ok(Auth(state.store.session(token).await?))
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
        Ok(AuthWithHash(session, hash))
    }
}

/// What every route reads: the store, and the account pages' settings.
pub struct AppState<B: Backend> {
    /// The store.
    pub store: Arc<Store<B>>,
    /// The account pages, sign-in and mail.
    pub accounts: Arc<crate::accounts::Accounts>,
}

impl<B: Backend> Clone for AppState<B> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            accounts: self.accounts.clone(),
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
        .map_err(|_| refused("too_large", "the body exceeds 1 MiB"))
}

fn decode<T: Decode>(bytes: &[u8]) -> Result<T, ApiError> {
    T::from_bytes(bytes).map_err(|e| {
        refused(
            e.kind.server_code(),
            &format!("the body doesn't decode: {e}"),
        )
    })
}

fn id(text: &str) -> Result<Id, ApiError> {
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
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
    Segments(workspace): Segments<String>,
    request: Body,
) -> Result<Response, ApiError> {
    let workspace = id(&workspace)?;
    let bytes = body(request).await?;
    Ok(ok(&store.push(session, workspace, bytes).await?))
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
    ChainReader(session): ChainReader,
    request: Body,
) -> Result<Response, ApiError> {
    let request: ChainsRequest = decode(&body(request).await?)?;
    Ok(ok(&store.read_chains(session, request).await?))
}

async fn challenge<B: Backend>(
    State(store): State<Arc<Store<B>>>,
    Auth(session): Auth,
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
    AuthWithHash(session, hash): AuthWithHash,
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

async fn not_found() -> ApiError {
    refused("not_found", "no such route")
}

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
    tracing::info!(request = %id, route = %route, status = response.status().as_u16(), "request");
    response
}

/// The routes of §26 that the server serves so far, and the account pages.
pub fn router<B: Backend>(state: AppState<B>) -> Router {
    let generation = state.store.restore_generation();
    Router::new()
        .route("/v2/limits", get(limits::<B>))
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
        .route(
            "/v2/workspaces",
            get(list_workspaces::<B>).post(create_workspace::<B>),
        )
        .route("/v2/workspaces/{w}/policy", get(policy::<B>))
        .route("/v2/workspaces/{w}/keys", get(keys::<B>))
        .route("/v2/workspaces/{w}/keys/current", get(current_keys::<B>))
        .route("/v2/workspaces/{w}/domains/{d}/rewrap", post(rewrap::<B>))
        .route("/v2/workspaces/{w}/ops", post(push::<B>))
        .route("/v2/workspaces/{w}/pull", post(pull::<B>))
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
        .fallback(not_found)
        .method_not_allowed_fallback(not_found)
        .with_state(state)
        .layer(middleware::from_fn(move |request: Request, next: Next| {
            stamp_and_log(generation, request, next)
        }))
}
