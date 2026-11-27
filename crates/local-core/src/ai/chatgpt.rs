//! Sign in with ChatGPT, as OpenAI documents it for open-source apps that
//! run on your computer: <https://developers.openai.com/siwc/token-sharing-open-source>.
//!
//! The first sign-in registers Noura as a client of your ChatGPT account
//! with `dynamic_agent_client`, and OpenAI returns the client ID to use
//! from then on. The browser comes back to a listener on `127.0.0.1`, the
//! code is exchanged with PKCE and no secret, and the ID token is checked
//! against OpenAI's published keys. The tokens live only in the credential
//! store, as one record per account. Access tokens last an hour and
//! refresh tokens 30 days, replaced on each refresh. Requests go to the
//! public Responses API with the access token as a bearer token.

use std::{
    fmt,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use crate::{CoreError, ErrorCategory, Result};

/// The provider kind of a ChatGPT plan.
pub const CHATGPT_KIND: &str = "chatgpt";
/// The provider ID Noura gives your ChatGPT plan.
pub const CHATGPT_PROVIDER: &str = "chatgpt";
/// Where you review and limit what apps use of your ChatGPT plan.
pub const MANAGE_USAGE_URL: &str = "https://chatgpt.com/settings/usage";
/// The model a ChatGPT plan starts with when its catalog doesn't answer.
pub const CHATGPT_DEFAULT_MODEL: &str = "gpt-6-luna";

/// The client ID a first sign-in registers with. It's an entry point,
/// never the ID Noura keeps.
const DYNAMIC_CLIENT: &str = "dynamic_agent_client";
/// The name Noura registers under, the same on every installation.
const AGENT_NAME: &str = "Noura";
const SCOPES: &str =
    "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct";
/// The scope that lets requests use the plan.
const PLAN_SCOPE: &str = "chatgpt.tokens.use.direct";
/// The resource every grant names.
pub(crate) const RESOURCE: &str = "https://api.openai.com/v1";
const CALLBACK_PATH: &str = "/auth/callback";
/// The port OpenAI's examples use. Any other may follow it.
const PREFERRED_PORT: u16 = 1455;
/// How long before an access token expires Noura replaces it.
const REFRESH_MARGIN: u64 = 300;
/// How far a token's times may drift from this computer's clock.
const CLOCK_SKEW: u64 = 120;
const HTTP_TIMEOUT: Duration = Duration::from_secs(30);

/// Where Noura signs in and sends requests. Tests point them at a server
/// on this computer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChatGptEndpoints {
    /// The OpenID issuer, `https://auth.openai.com`.
    pub issuer: String,
    /// The API base, `https://api.openai.com/v1`.
    pub api: String,
}

impl Default for ChatGptEndpoints {
    fn default() -> Self {
        Self {
            issuer: "https://auth.openai.com".to_owned(),
            api: RESOURCE.to_owned(),
        }
    }
}

impl ChatGptEndpoints {
    fn at(&self, path: &str) -> String {
        format!("{}{path}", self.issuer.trim_end_matches('/'))
    }

    pub(crate) fn api_at(&self, path: &str) -> String {
        format!("{}/{path}", self.api.trim_end_matches('/'))
    }
}

/// What a ChatGPT sign-in keeps in the credential store: your account,
/// the client it registered, and its tokens. Nothing else holds it.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatGptCredential {
    pub email: Option<String>,
    pub issuer: String,
    /// The account, as the validated ID token names it.
    pub subject: String,
    /// The client ID OpenAI issued at registration, such as `oaiapp_…`.
    pub client_id: String,
    pub ext_agent_host_id: String,
    pub id_token: String,
    pub access_token: String,
    pub refresh_token: String,
    pub token_type: String,
    /// When the access token expires, in Unix seconds.
    pub expires_at: u64,
    pub scopes: Vec<String>,
    /// When Noura received these tokens, in Unix seconds.
    pub saved_at: u64,
}

impl fmt::Debug for ChatGptCredential {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatGptCredential")
            .field("subject", &self.subject)
            .field("client_id", &self.client_id)
            .field("expires_at", &self.expires_at)
            .finish_non_exhaustive()
    }
}

impl ChatGptCredential {
    /// The record as the credential store keeps it.
    pub(crate) fn to_secret(&self) -> Result<String> {
        serde_json::to_string(self).map_err(|_| failure("chatgpt_credential_invalid"))
    }

    pub(crate) fn from_secret(secret: &str) -> Result<Self> {
        serde_json::from_str(secret).map_err(|_| failure("chatgpt_credential_invalid"))
    }

    /// Whether the access token needs replacing before a request.
    pub(crate) fn expires_soon(&self, now: u64) -> bool {
        now + REFRESH_MARGIN >= self.expires_at
    }
}

/// The client this device registered for your account, kept beside the
/// provider settings so a later sign-in reuses it. None of it is secret.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ChatGptRegistration {
    /// This device's host ID, chosen once before its first sign-in.
    pub ext_agent_host_id: String,
    /// The client ID OpenAI issued, once a sign-in registered one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
    /// The account's email, to show and to hint the next sign-in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Whether you dismissed the notice that Noura uses your plan.
    #[serde(default)]
    pub notice_seen: bool,
}

impl ChatGptRegistration {
    /// A registration with a new host ID.
    pub fn new() -> Self {
        Self {
            ext_agent_host_id: format!("urn:uuid:{}", uuid::Uuid::new_v4()),
            ..Self::default()
        }
    }
}

/// A sign-in waiting for the browser to come back.
pub struct ChatGptSignIn {
    listener: TcpListener,
    redirect_uri: String,
    state: String,
    nonce: String,
    verifier: String,
    /// The issued client ID, when this device registered one before.
    client_id: Option<String>,
    host_id: String,
    endpoints: ChatGptEndpoints,
    url: String,
}

impl fmt::Debug for ChatGptSignIn {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ChatGptSignIn")
            .field("redirect_uri", &self.redirect_uri)
            .finish_non_exhaustive()
    }
}

impl ChatGptSignIn {
    /// Start a sign-in: listen on `127.0.0.1`, then build the address to
    /// open in the browser. With the client a sign-in on this device
    /// registered before, it signs in to that account again; without one,
    /// it registers a new client.
    pub async fn start(
        endpoints: ChatGptEndpoints,
        registration: &ChatGptRegistration,
    ) -> Result<Self> {
        let listener = match TcpListener::bind(("127.0.0.1", PREFERRED_PORT)).await {
            Ok(listener) => listener,
            Err(_) => TcpListener::bind(("127.0.0.1", 0))
                .await
                .map_err(|_| failure("chatgpt_sign_in_unavailable"))?,
        };
        let port = listener
            .local_addr()
            .map_err(|_| failure("chatgpt_sign_in_unavailable"))?
            .port();
        let redirect_uri = format!("http://127.0.0.1:{port}{CALLBACK_PATH}");
        let state = random_token()?;
        let nonce = random_token()?;
        let verifier = random_token()?;
        let challenge = URL_SAFE_NO_PAD.encode(ring::digest::digest(
            &ring::digest::SHA256,
            verifier.as_bytes(),
        ));
        let client_id = registration.client_id.clone();
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        query.append_pair("client_id", client_id.as_deref().unwrap_or(DYNAMIC_CLIENT));
        if client_id.is_none() {
            query.append_pair("agent_name_hint", AGENT_NAME);
        } else if let Some(email) = &registration.email {
            query.append_pair("login_hint", email);
        }
        query
            .append_pair("ext_agent_host_id", &registration.ext_agent_host_id)
            .append_pair("response_type", "code")
            .append_pair("redirect_uri", &redirect_uri)
            .append_pair("scope", SCOPES)
            .append_pair("resource", RESOURCE)
            .append_pair("state", &state)
            .append_pair("nonce", &nonce)
            .append_pair("code_challenge_method", "S256")
            .append_pair("code_challenge", &challenge);
        let url = format!(
            "{}?{}",
            endpoints.at("/api/accounts/authorize"),
            query.finish()
        );
        Ok(Self {
            listener,
            redirect_uri,
            state,
            nonce,
            verifier,
            client_id,
            host_id: registration.ext_agent_host_id.clone(),
            endpoints,
            url,
        })
    }

    /// The address to open in the browser.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Wait up to `patience` for the browser to come back, then exchange
    /// the code and check the ID token and the plan scope.
    pub async fn finish(self, patience: Duration) -> Result<ChatGptCredential> {
        let callback =
            tokio::time::timeout(patience, wait_for_callback(&self.listener, &self.state))
                .await
                .map_err(|_| failure("chatgpt_sign_in_timed_out"))??;
        drop(self.listener);
        let client_id = match (&self.client_id, callback.client_id) {
            // A sign-in again keeps the client it registered.
            (Some(saved), Some(issued)) if *saved != issued => {
                return Err(failure("chatgpt_account_mismatch"));
            }
            (Some(saved), _) => saved.clone(),
            (None, Some(issued)) if issued != DYNAMIC_CLIENT => issued,
            (None, _) => return Err(failure("chatgpt_registration_incomplete")),
        };
        let http = client()?;
        let tokens = token_request(
            &http,
            &self.endpoints,
            &[
                ("grant_type", "authorization_code"),
                ("client_id", &client_id),
                ("code", &callback.code),
                ("code_verifier", &self.verifier),
                ("redirect_uri", &self.redirect_uri),
                ("resource", RESOURCE),
            ],
        )
        .await?;
        let id_token = tokens
            .id_token
            .clone()
            .ok_or_else(|| failure("chatgpt_id_token_invalid"))?;
        let claims =
            validate_id_token(&http, &self.endpoints, &id_token, &client_id, &self.nonce).await?;
        let scopes = scopes_of(tokens.scope.as_deref().or(callback.scope.as_deref()));
        if !scopes.iter().any(|scope| scope == PLAN_SCOPE) {
            return Err(failure("chatgpt_plan_not_granted"));
        }
        let now = now();
        Ok(ChatGptCredential {
            email: claims.email,
            issuer: self.endpoints.issuer.clone(),
            subject: claims.sub,
            client_id,
            ext_agent_host_id: self.host_id,
            id_token,
            access_token: tokens.access_token,
            refresh_token: tokens
                .refresh_token
                .ok_or_else(|| failure("chatgpt_sign_in_failed"))?,
            token_type: tokens.token_type.unwrap_or_else(|| "Bearer".to_owned()),
            expires_at: now + tokens.expires_in.unwrap_or(3600),
            scopes,
            saved_at: now,
        })
    }
}

/// What the browser brought back.
struct Callback {
    code: String,
    client_id: Option<String>,
    scope: Option<String>,
}

/// Answer the browser's requests until one comes back to the callback
/// with this attempt's `state`. A request with another state, as a page
/// could forge, gets an error page and changes nothing.
async fn wait_for_callback(listener: &TcpListener, state: &str) -> Result<Callback> {
    loop {
        let Ok((mut socket, _)) = listener.accept().await else {
            continue;
        };
        let Some(target) = request_target(&mut socket).await else {
            continue;
        };
        let Some(query) = target
            .strip_prefix(CALLBACK_PATH)
            .and_then(|rest| rest.strip_prefix('?'))
        else {
            respond(&mut socket, "404 Not Found", "Not found.").await;
            continue;
        };
        let pairs: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
            .map(|(name, value)| (name.into_owned(), value.into_owned()))
            .collect();
        let value = |name: &str| {
            pairs
                .iter()
                .find(|(key, _)| key == name)
                .map(|(_, value)| value.clone())
        };
        if value("state").as_deref() != Some(state) {
            respond(&mut socket, "400 Bad Request", PAGE_FAILED).await;
            continue;
        }
        if let Some(error) = value("error") {
            respond(&mut socket, "200 OK", PAGE_FAILED).await;
            let code = if error == "access_denied" {
                "chatgpt_sign_in_declined"
            } else {
                "chatgpt_sign_in_failed"
            };
            tracing::warn!(code, "ChatGPT sign-in came back with an error");
            return Err(failure(code));
        }
        let Some(code) = value("code") else {
            respond(&mut socket, "400 Bad Request", PAGE_FAILED).await;
            continue;
        };
        respond(&mut socket, "200 OK", PAGE_DONE).await;
        return Ok(Callback {
            code,
            client_id: value("client_id"),
            scope: value("scope"),
        });
    }
}

/// The page the browser shows once Noura has what it needs.
const PAGE_DONE: &str = "You're signed in. You can close this tab.";
/// The page the browser shows when the sign-in didn't finish.
const PAGE_FAILED: &str = "Sign-in didn't finish. Go back to Noura to try again.";

/// The request target of the GET the browser sent, such as
/// `/auth/callback?code=…`.
async fn request_target(socket: &mut TcpStream) -> Option<String> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 2048];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let read = tokio::time::timeout(Duration::from_secs(10), socket.read(&mut chunk))
            .await
            .ok()?
            .ok()?;
        if read == 0 || bytes.len() > 16 * 1024 {
            return None;
        }
        bytes.extend_from_slice(&chunk[..read]);
    }
    let head = String::from_utf8_lossy(&bytes);
    let line = head.lines().next()?;
    let mut parts = line.split(' ');
    (parts.next()? == "GET").then_some(())?;
    parts.next().map(str::to_owned)
}

async fn respond(socket: &mut TcpStream, status: &str, text: &str) {
    let body = format!(
        "<!doctype html><meta charset=utf-8><title>Noura</title>\
         <body style=\"font-family:system-ui;margin:4rem\"><p>{text}</p>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: text/html; charset=utf-8\r\n\
         cache-control: no-store\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
        body.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: Option<String>,
    id_token: Option<String>,
    token_type: Option<String>,
    expires_in: Option<u64>,
    scope: Option<String>,
}

/// Replace `credential`'s access token and its rotating refresh token.
/// A refresh token OpenAI no longer takes fails with
/// `provider_signed_out`, which means signing in again.
pub(crate) async fn refresh(
    endpoints: &ChatGptEndpoints,
    credential: &ChatGptCredential,
) -> Result<ChatGptCredential> {
    let http = client()?;
    let tokens = token_request(
        &http,
        endpoints,
        &[
            ("grant_type", "refresh_token"),
            ("client_id", &credential.client_id),
            ("refresh_token", &credential.refresh_token),
            ("resource", RESOURCE),
        ],
    )
    .await?;
    let now = now();
    let mut next = credential.clone();
    next.access_token = tokens.access_token;
    if let Some(refresh_token) = tokens.refresh_token {
        next.refresh_token = refresh_token;
    }
    if let Some(id_token) = tokens.id_token {
        next.id_token = id_token;
    }
    if let Some(token_type) = tokens.token_type {
        next.token_type = token_type;
    }
    if tokens.scope.is_some() {
        next.scopes = scopes_of(tokens.scope.as_deref());
    }
    next.expires_at = now + tokens.expires_in.unwrap_or(3600);
    next.saved_at = now;
    Ok(next)
}

/// End the renewable session at OpenAI. Returns whether OpenAI confirmed
/// it; a network failure or a server error leaves it unconfirmed.
pub(crate) async fn revoke(endpoints: &ChatGptEndpoints, credential: &ChatGptCredential) -> bool {
    let Ok(http) = client() else {
        return false;
    };
    let endpoint = match discovery(&http, endpoints).await {
        Ok(discovered) => discovered.revocation_endpoint,
        Err(_) => None,
    }
    .unwrap_or_else(|| endpoints.at("/api/accounts/oauth/revoke"));
    if !allowed_url(&endpoint) {
        return false;
    }
    let body = form(&[
        ("token", &credential.refresh_token),
        ("token_type_hint", "refresh_token"),
        ("client_id", &credential.client_id),
    ]);
    for attempt in 0..3 {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(500 * attempt)).await;
        }
        match http
            .post(&endpoint)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body.clone())
            .send()
            .await
        {
            Ok(response) if response.status().is_success() => return true,
            Ok(response) if !response.status().is_server_error() => {
                tracing::warn!(
                    status = response.status().as_u16(),
                    "ChatGPT sign-out wasn't confirmed"
                );
                return false;
            }
            _ => {}
        }
    }
    false
}

/// The models your ChatGPT plan offers, as `(slug, name)`, in OpenAI's
/// order.
pub(crate) async fn models(
    endpoints: &ChatGptEndpoints,
    access_token: &str,
) -> Result<Vec<(String, String)>> {
    let http = client()?;
    let response = http
        .get(endpoints.api_at("models"))
        .bearer_auth(access_token)
        .send()
        .await
        .map_err(|_| failure("chatgpt_models_unavailable"))?;
    if !response.status().is_success() {
        return Err(failure("chatgpt_models_unavailable"));
    }
    let body: Value = response
        .json()
        .await
        .map_err(|_| failure("chatgpt_models_unavailable"))?;
    Ok(body
        .get("models")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|model| model.get("visibility").and_then(Value::as_str) == Some("list"))
        .filter_map(|model| {
            let slug = model.get("slug")?.as_str()?;
            let name = model
                .get("display_name")
                .and_then(Value::as_str)
                .unwrap_or(slug);
            Some((slug.to_owned(), name.to_owned()))
        })
        .collect())
}

async fn token_request(
    http: &reqwest::Client,
    endpoints: &ChatGptEndpoints,
    fields: &[(&str, &str)],
) -> Result<TokenResponse> {
    let response = http
        .post(endpoints.at("/api/accounts/oauth/token"))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(form(fields))
        .send()
        .await
        .map_err(|_| retryable(failure("chatgpt_unreachable")))?;
    let status = response.status();
    if status.is_success() {
        return response
            .json()
            .await
            .map_err(|_| failure("chatgpt_sign_in_failed"));
    }
    let body: Value = response.json().await.unwrap_or(Value::Null);
    let error = body
        .get("error")
        .and_then(Value::as_str)
        .unwrap_or_default();
    tracing::warn!(
        status = status.as_u16(),
        error = error
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_')
            .then_some(error),
        "a ChatGPT token request failed"
    );
    let unusable = matches!(
        error,
        "invalid_grant"
            | "invalid_refresh_token"
            | "token_expired"
            | "refresh_token_expired"
            | "refresh_token_invalidated"
            | "refresh_token_reused"
    );
    if unusable {
        Err(failure(super::failures::PROVIDER_SIGNED_OUT))
    } else if status.is_server_error() {
        Err(retryable(failure("chatgpt_unreachable")))
    } else {
        Err(failure("chatgpt_sign_in_failed"))
    }
}

#[derive(Deserialize)]
struct Discovery {
    jwks_uri: Option<String>,
    revocation_endpoint: Option<String>,
}

async fn discovery(http: &reqwest::Client, endpoints: &ChatGptEndpoints) -> Result<Discovery> {
    http.get(endpoints.at("/.well-known/openid-configuration"))
        .send()
        .await
        .map_err(|_| failure("chatgpt_unreachable"))?
        .json()
        .await
        .map_err(|_| failure("chatgpt_id_token_invalid"))
}

/// The claims Noura keeps from a valid ID token.
#[derive(Debug)]
struct Claims {
    sub: String,
    email: Option<String>,
}

/// Check `token`'s RS256 signature against OpenAI's published keys, then
/// its issuer, audience, expiry and nonce.
async fn validate_id_token(
    http: &reqwest::Client,
    endpoints: &ChatGptEndpoints,
    token: &str,
    client_id: &str,
    nonce: &str,
) -> Result<Claims> {
    let invalid = || failure("chatgpt_id_token_invalid");
    let mut parts = token.split('.');
    let (Some(header), Some(payload), Some(signature), None) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return Err(invalid());
    };
    let decode = |part: &str| URL_SAFE_NO_PAD.decode(part).map_err(|_| invalid());
    let header: Value = serde_json::from_slice(&decode(header)?).map_err(|_| invalid())?;
    if header.get("alg").and_then(Value::as_str) != Some("RS256") {
        return Err(invalid());
    }
    let kid = header.get("kid").and_then(Value::as_str);
    let jwks_uri = discovery(http, endpoints)
        .await?
        .jwks_uri
        .unwrap_or_else(|| endpoints.at("/.well-known/jwks.json"));
    if !allowed_url(&jwks_uri) {
        return Err(invalid());
    }
    let jwks: Value = http
        .get(&jwks_uri)
        .send()
        .await
        .map_err(|_| failure("chatgpt_unreachable"))?
        .json()
        .await
        .map_err(|_| invalid())?;
    let keys = jwks
        .get("keys")
        .and_then(Value::as_array)
        .ok_or_else(invalid)?;
    let key = keys
        .iter()
        .filter(|key| key.get("kty").and_then(Value::as_str) == Some("RSA"))
        .find(|key| kid.is_none() || key.get("kid").and_then(Value::as_str) == kid)
        .ok_or_else(invalid)?;
    let component = |name: &str| {
        key.get(name)
            .and_then(Value::as_str)
            .ok_or_else(invalid)
            .and_then(decode)
    };
    let public = ring::signature::RsaPublicKeyComponents {
        n: component("n")?,
        e: component("e")?,
    };
    let signed = &token[..token.len() - signature.len() - 1];
    public
        .verify(
            &ring::signature::RSA_PKCS1_2048_8192_SHA256,
            signed.as_bytes(),
            &decode(signature)?,
        )
        .map_err(|_| invalid())?;
    let claims: Value = serde_json::from_slice(&decode(payload)?).map_err(|_| invalid())?;
    let now = now();
    let issuer_ok = claims.get("iss").and_then(Value::as_str) == Some(endpoints.issuer.as_str());
    let audience_ok = match claims.get("aud") {
        Some(Value::String(audience)) => audience == client_id,
        Some(Value::Array(audiences)) => audiences.iter().any(|audience| audience == client_id),
        _ => false,
    };
    let fresh = claims
        .get("exp")
        .and_then(Value::as_u64)
        .is_some_and(|exp| exp + CLOCK_SKEW > now);
    let nonce_ok = claims.get("nonce").and_then(Value::as_str) == Some(nonce);
    let sub = claims
        .get("sub")
        .and_then(Value::as_str)
        .filter(|sub| !sub.is_empty());
    match (issuer_ok, audience_ok, fresh, nonce_ok, sub) {
        (true, true, true, true, Some(sub)) => Ok(Claims {
            sub: sub.to_owned(),
            email: claims
                .get("email")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        _ => Err(invalid()),
    }
}

fn scopes_of(scope: Option<&str>) -> Vec<String> {
    scope
        .unwrap_or_default()
        .split([' ', '+'])
        .filter(|scope| !scope.is_empty())
        .map(str::to_owned)
        .collect()
}

fn form(fields: &[(&str, &str)]) -> String {
    let mut form = url::form_urlencoded::Serializer::new(String::new());
    for (name, value) in fields {
        form.append_pair(name, value);
    }
    form.finish()
}

/// Whether Noura sends a request to `url`: HTTPS, or HTTP on this
/// computer, as a test's server is.
fn allowed_url(url: &str) -> bool {
    let Ok(url) = url::Url::parse(url) else {
        return false;
    };
    match url.scheme() {
        "https" => url.host().is_some(),
        "http" => {
            matches!(url.host(), Some(url::Host::Ipv4(address)) if address.is_loopback())
                || url.host_str() == Some("localhost")
        }
        _ => false,
    }
}

fn client() -> Result<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(HTTP_TIMEOUT)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| failure("chatgpt_unreachable"))
}

/// 32 random bytes, base64url without padding.
fn random_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| failure("chatgpt_sign_in_unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

fn failure(code: &str) -> CoreError {
    let category = match code {
        "chatgpt_unreachable" => ErrorCategory::Transient,
        "chatgpt_credential_invalid" => ErrorCategory::Credential,
        _ => ErrorCategory::Provider,
    };
    CoreError::new(
        code,
        category,
        "ChatGPT sign-in didn't finish",
        "chatgpt_sign_in",
    )
}

fn retryable(mut error: CoreError) -> CoreError {
    error.retryable = true;
    error
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_credential_never_shows_its_tokens_when_printed() {
        let credential = ChatGptCredential {
            email: Some("you@example.com".into()),
            issuer: "https://auth.openai.com".into(),
            subject: "user_1".into(),
            client_id: "oaiapp_1".into(),
            ext_agent_host_id: "urn:uuid:1".into(),
            id_token: "id-secret".into(),
            access_token: "access-secret".into(),
            refresh_token: "refresh-secret".into(),
            token_type: "Bearer".into(),
            expires_at: 10_000,
            scopes: vec![PLAN_SCOPE.into()],
            saved_at: 6_400,
        };
        let printed = format!("{credential:?}");
        for secret in [
            "id-secret",
            "access-secret",
            "refresh-secret",
            "you@example.com",
        ] {
            assert!(!printed.contains(secret), "{printed}");
        }
        assert!(credential.expires_soon(9_800));
        assert!(!credential.expires_soon(9_000));
        let reread = ChatGptCredential::from_secret(&credential.to_secret().unwrap()).unwrap();
        assert_eq!(reread, credential);
    }

    #[test]
    fn only_https_and_this_computer_are_allowed() {
        assert!(allowed_url("https://auth.openai.com/.well-known/jwks.json"));
        assert!(allowed_url("http://127.0.0.1:9/keys"));
        assert!(!allowed_url("http://auth.openai.com/keys"));
        assert!(!allowed_url("file:///etc/passwd"));
    }

    #[test]
    fn scopes_split_on_spaces_and_plus_signs() {
        assert_eq!(
            scopes_of(Some("openid+chatgpt.tokens.use.direct email")),
            ["openid", "chatgpt.tokens.use.direct", "email"]
        );
    }
}
