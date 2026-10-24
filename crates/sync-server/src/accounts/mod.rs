//! The account pages (§7, §29; ADR 0003): sign-up and sign-in with a
//! passkey or a magic link, more passkeys for a signed-in account, and
//! signing out. The pages are askama templates with plain forms, which
//! work without script for magic links. `account-web`, one WebAssembly
//! module the pages load, runs the passkey ceremonies against the JSON
//! routes under `/account/passkeys/`, and webauthn-rs checks them here.
//!
//! Sign-up accepts only allowlisted emails, and the server answers an
//! allowed and a refused email the same way: it mails a link only to an
//! allowed one. An account works only once its email confirms it: a magic
//! link signs up the email that opens it, and a passkey created at sign-up
//! waits for the link mailed to its email. Every page response carries a
//! content security policy that blocks inline scripts, and every form post
//! must come from the server's own origin.

pub mod config;
pub mod mail;
mod pages;

use std::path::PathBuf;
use std::sync::Arc;

use askama::Template;
use axum::Router;
use axum::body::Body;
use axum::extract::{Path, RawQuery, Request, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, COOKIE, LOCATION, ORIGIN,
    REFERRER_POLICY, SET_COOKIE, X_CONTENT_TYPE_OPTIONS, X_FRAME_OPTIONS,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use serde_json::{Value, json};
use sync_protocol::types::{Id, from_base64url, to_base64url};
use sync_store::{Backend, CeremonyKind, PasskeyRow, SignInToken, Store};
use webauthn_rs::prelude::{
    Passkey, PasskeyAuthentication, PasskeyRegistration, PublicKeyCredential,
    RegisterPublicKeyCredential, Uuid,
};
use webauthn_rs::{Webauthn, WebauthnBuilder};

pub use config::{Allowlist, PublicOrigin, normalize_email};
pub use mail::{Mail, MailError, MailTransport, NoMail};

use crate::http::{AppState, InternalCause};

/// The cookie that carries a web session's token.
pub const SESSION_COOKIE: &str = "noura_session";

/// The content security policy of every account page and asset: scripts
/// only from the server's own files, and WebAssembly for account-web, so no
/// inline script runs.
pub const CONTENT_POLICY: &str = "default-src 'none'; script-src 'self' 'wasm-unsafe-eval'; \
     connect-src 'self'; style-src 'self'; img-src 'self'; form-action 'self'; \
     base-uri 'none'; frame-ancestors 'none'";

/// The largest body an account route reads.
const MAX_ACCOUNT_BODY: usize = 64 * 1024;

/// The files of the account-web build that the server serves, beside the
/// two it embeds.
const BUILT_ASSETS: &[(&str, &str)] = &[
    ("account_web.js", "text/javascript; charset=utf-8"),
    ("account_web_bg.wasm", "application/wasm"),
    ("THIRD_PARTY_NOTICES.txt", "text/plain; charset=utf-8"),
];

/// The account pages' settings: the public origin, the allowlist, the
/// mail transport, the passkey relying party and the account-web build.
pub struct Accounts {
    origin: PublicOrigin,
    allowlist: Allowlist,
    mail_from: String,
    mailer: Box<dyn MailTransport>,
    webauthn: Option<Webauthn>,
    assets: Option<PathBuf>,
}

impl Accounts {
    /// The account pages at `origin`, whose host is the passkeys' relying
    /// party. An origin on an IP address can't hold passkeys, so its pages
    /// offer magic links alone.
    pub fn new(
        origin: PublicOrigin,
        allowlist: Allowlist,
        mail_from: String,
        mailer: Box<dyn MailTransport>,
        assets: Option<PathBuf>,
    ) -> Self {
        let webauthn = WebauthnBuilder::new(&origin.host(), origin.url())
            .and_then(|builder| builder.rp_name("Noura").build())
            .ok();
        Self {
            origin,
            allowlist,
            mail_from,
            mailer,
            webauthn,
            assets,
        }
    }

    /// Account pages for tests and local servers: no allowlist and no mail.
    pub fn local(origin: PublicOrigin) -> Self {
        Self::new(
            origin,
            Allowlist::default(),
            "Noura <noura@localhost>".into(),
            Box::new(NoMail),
            None,
        )
    }

    /// The server's public origin.
    pub fn origin(&self) -> &PublicOrigin {
        &self.origin
    }

    /// Whether the relying party for passkeys is set up.
    pub fn has_passkeys(&self) -> bool {
        self.webauthn.is_some()
    }

    fn link(&self, path: &str, token: &SignInToken) -> String {
        format!(
            "{}{path}?token={}",
            self.origin.as_str(),
            to_base64url(token)
        )
    }

    async fn send(&self, to: &str, subject: &str, text: String) -> Result<(), MailError> {
        let mail = Mail {
            from: self.mail_from.clone(),
            to: to.to_owned(),
            subject: subject.to_owned(),
            text,
        };
        self.mailer.send(&mail).await
    }
}

/// The account routes.
pub fn routes<B: Backend>() -> Router<AppState<B>> {
    Router::new()
        .route("/account", get(account_page::<B>))
        .route("/account/sign-in", get(sign_in_page))
        .route("/account/sign-up", get(sign_up_page))
        .route(
            "/account/email-link",
            get(email_link_page).post(email_link::<B>),
        )
        .route("/account/email-link/use", post(use_email_link::<B>))
        .route("/account/confirm", get(confirm_page).post(confirm::<B>))
        .route("/account/sign-out", post(sign_out::<B>))
        .route("/account/passkeys/sign-up/start", post(sign_up_start::<B>))
        .route(
            "/account/passkeys/sign-up/finish",
            post(sign_up_finish::<B>),
        )
        .route("/account/passkeys/sign-in/start", post(sign_in_start::<B>))
        .route(
            "/account/passkeys/sign-in/finish",
            post(sign_in_finish::<B>),
        )
        .route("/account/passkeys/add/start", post(add_start::<B>))
        .route("/account/passkeys/add/finish", post(add_finish::<B>))
        .route("/account/assets/{file}", get(asset::<B>))
        .layer(middleware::from_fn(page_headers))
}

/// Sets the content security policy and the other page headers on every
/// account response.
async fn page_headers(request: Request, next: Next) -> Response {
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(CONTENT_POLICY),
    );
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    headers.insert(X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    headers
        .entry(CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    response
}

fn html(status: StatusCode, page: &impl Template) -> Response {
    match page.render() {
        Ok(body) => (
            status,
            [(
                CONTENT_TYPE,
                HeaderValue::from_static("text/html; charset=utf-8"),
            )],
            body,
        )
            .into_response(),
        Err(error) => failure(error),
    }
}

fn message(status: StatusCode, heading: &str, text: &str) -> Response {
    html(status, &pages::Message { heading, text })
}

/// A store or template failure: a generic page, with the cause for the
/// request log.
fn failure(cause: impl std::fmt::Display) -> Response {
    let mut response = message(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Something went wrong",
        "The server couldn't finish. Try again in a moment.",
    );
    response
        .extensions_mut()
        .insert(InternalCause(cause.to_string()));
    response
}

fn json_reply(status: StatusCode, value: Value) -> Response {
    (
        status,
        [(CONTENT_TYPE, HeaderValue::from_static("application/json"))],
        value.to_string(),
    )
        .into_response()
}

fn json_error(status: StatusCode, code: &str, message: &str) -> Response {
    json_reply(status, json!({ "error": code, "message": message }))
}

fn json_failure(cause: impl std::fmt::Display) -> Response {
    let mut response = json_error(
        StatusCode::INTERNAL_SERVER_ERROR,
        "internal",
        "The server couldn't finish. Try again in a moment.",
    );
    response
        .extensions_mut()
        .insert(InternalCause(cause.to_string()));
    response
}

fn redirect(location: &str) -> Response {
    (
        StatusCode::SEE_OTHER,
        [(
            LOCATION,
            HeaderValue::from_str(location).unwrap_or(HeaderValue::from_static("/account")),
        )],
    )
        .into_response()
}

fn session_cookie(accounts: &Accounts, token: &SignInToken) -> HeaderValue {
    let secure = if accounts.origin.is_https() {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}={}; Path=/; HttpOnly; SameSite=Strict{secure}",
        to_base64url(token)
    ))
    .expect("a cookie of base64url is a header value")
}

fn cleared_cookie(accounts: &Accounts) -> HeaderValue {
    let secure = if accounts.origin.is_https() {
        "; Secure"
    } else {
        ""
    };
    HeaderValue::from_str(&format!(
        "{SESSION_COOKIE}=; Path=/; HttpOnly; SameSite=Strict; Max-Age=0{secure}"
    ))
    .expect("a header value")
}

fn with_cookie(mut response: Response, cookie: HeaderValue) -> Response {
    response.headers_mut().append(SET_COOKIE, cookie);
    response
}

/// The web session token a request's cookie carries.
fn cookie_token(headers: &HeaderMap) -> Option<SignInToken> {
    headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .and_then(|(_, value)| from_base64url::<32>(value).ok())
}

/// The account a request's web session belongs to.
async fn signed_in<B: Backend>(
    store: &Store<B>,
    headers: &HeaderMap,
) -> sync_store::Result<Option<(Id, SignInToken)>> {
    let Some(token) = cookie_token(headers) else {
        return Ok(None);
    };
    Ok(store
        .web_session(&token)
        .await?
        .map(|account| (account, token)))
}

/// Whether a post comes from the server's own pages: browsers send the
/// `Origin` header with every form post and fetch, so a missing or foreign
/// one refuses the request.
fn same_origin(accounts: &Accounts, headers: &HeaderMap) -> bool {
    headers
        .get(ORIGIN)
        .and_then(|value| value.to_str().ok())
        .is_some_and(|origin| origin == accounts.origin.as_str())
}

fn cross_origin() -> Response {
    message(
        StatusCode::FORBIDDEN,
        "Request refused",
        "This form only works from this server's own pages.",
    )
}

async fn read_body(body: Body) -> Option<Vec<u8>> {
    axum::body::to_bytes(body, MAX_ACCOUNT_BODY)
        .await
        .ok()
        .map(|bytes| bytes.to_vec())
}

/// One field of a form body or a query string.
fn field(encoded: &[u8], name: &str) -> Option<String> {
    url::form_urlencoded::parse(encoded)
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
}

fn token_of(text: Option<String>) -> Option<SignInToken> {
    text.and_then(|t| from_base64url::<32>(&t).ok())
}

fn user_handle(account: Id) -> Uuid {
    Uuid::from_bytes(account.0)
}

async fn sign_in_page() -> Response {
    html(StatusCode::OK, &pages::SignIn)
}

async fn sign_up_page() -> Response {
    html(StatusCode::OK, &pages::SignUp)
}

async fn account_page<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
) -> Response {
    let store = &state.store;
    let (account, _) = match signed_in(store, &headers).await {
        Ok(Some(found)) => found,
        Ok(None) => return redirect("/account/sign-in"),
        Err(error) => return failure(error),
    };
    let email = match store.account_email(account).await {
        Ok(Some(email)) => email,
        Ok(None) => return redirect("/account/sign-in"),
        Err(error) => return failure(error),
    };
    let passkeys = match store.passkeys(account).await {
        Ok(passkeys) => passkeys.len(),
        Err(error) => return failure(error),
    };
    html(
        StatusCode::OK,
        &pages::Account {
            email: &email,
            passkeys,
        },
    )
}

/// `POST /account/email-link`: mails a magic link to an allowed email, and
/// answers every email the same way.
async fn email_link<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let accounts = &state.accounts;
    if !same_origin(accounts, &headers) {
        return cross_origin();
    }
    let Some(body) = read_body(body).await else {
        return message(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Request refused",
            "The form is too large.",
        );
    };
    let Some(email) = field(&body, "email").as_deref().and_then(normalize_email) else {
        return message(
            StatusCode::BAD_REQUEST,
            "Check the address",
            "Enter an email address, such as you@example.com.",
        );
    };
    if accounts.allowlist.admits(&email) {
        let token = match state.store.issue_magic_link(&email).await {
            Ok(token) => token,
            Err(error) => return failure(error),
        };
        let text = format!(
            "Use this link to sign in to Noura. It works once, within 10 minutes.\n\n{}\n\nIf you didn't ask to sign in, ignore this email.\n",
            accounts.link("/account/email-link", &token)
        );
        if let Err(error) = accounts.send(&email, "Sign in to Noura", text).await {
            // A link that never reached its email leaves nothing usable.
            if let Err(revoke) = state.store.revoke_magic_link(&token).await {
                return failure(revoke);
            }
            tracing::error!(cause = %error, "a sign-in email failed to send");
        }
    }
    html(StatusCode::OK, &pages::CheckEmail { email: &email })
}

async fn email_link_page(RawQuery(query): RawQuery) -> Response {
    let token = field(query.unwrap_or_default().as_bytes(), "token").unwrap_or_default();
    html(
        StatusCode::OK,
        &pages::Continue {
            heading: "Sign in to Noura",
            action: "/account/email-link/use",
            token: &token,
            button: "Continue",
        },
    )
}

fn expired_link() -> Response {
    message(
        StatusCode::BAD_REQUEST,
        "This link doesn't work anymore",
        "A link works once, within 10 minutes of being sent. Ask for a new one.",
    )
}

/// `POST /account/email-link/use`: uses a magic link once, signing up its
/// email when it has no account yet, and starts a web session.
async fn use_email_link<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let accounts = &state.accounts;
    if !same_origin(accounts, &headers) {
        return cross_origin();
    }
    let Some(token) = read_body(body)
        .await
        .and_then(|b| token_of(field(&b, "token")))
    else {
        return expired_link();
    };
    match state.store.sign_in_with_magic_link(&token).await {
        Ok(Some((_, session))) => {
            with_cookie(redirect("/account"), session_cookie(accounts, &session))
        }
        Ok(None) => expired_link(),
        Err(error) => failure(error),
    }
}

async fn confirm_page(RawQuery(query): RawQuery) -> Response {
    let token = field(query.unwrap_or_default().as_bytes(), "token").unwrap_or_default();
    html(
        StatusCode::OK,
        &pages::Continue {
            heading: "Confirm your email",
            action: "/account/confirm",
            token: &token,
            button: "Confirm and sign in",
        },
    )
}

/// `POST /account/confirm`: confirms the email of a passkey sign-up once,
/// which makes the account and its passkey work, and starts a web session.
async fn confirm<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let accounts = &state.accounts;
    if !same_origin(accounts, &headers) {
        return cross_origin();
    }
    let Some(token) = read_body(body)
        .await
        .and_then(|b| token_of(field(&b, "token")))
    else {
        return expired_link();
    };
    match state.store.confirm_passkey(&token).await {
        Ok(Some((_, session))) => {
            with_cookie(redirect("/account"), session_cookie(accounts, &session))
        }
        Ok(None) => expired_link(),
        Err(error) => failure(error),
    }
}

/// `POST /account/sign-out`: ends the web session.
async fn sign_out<B: Backend>(State(state): State<AppState<B>>, headers: HeaderMap) -> Response {
    let accounts = &state.accounts;
    if !same_origin(accounts, &headers) {
        return cross_origin();
    }
    if let Some(token) = cookie_token(&headers)
        && let Err(error) = state.store.end_web_session(&token).await
    {
        return failure(error);
    }
    with_cookie(redirect("/account/sign-in"), cleared_cookie(accounts))
}

/// A JSON passkey request's body, after the origin check.
async fn json_body(
    accounts: &Accounts,
    headers: &HeaderMap,
    body: Body,
) -> Result<Value, Response> {
    if !same_origin(accounts, headers) {
        return Err(json_error(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Passkeys work only from this server's own pages.",
        ));
    }
    if accounts.webauthn.is_none() {
        return Err(json_error(
            StatusCode::NOT_FOUND,
            "passkeys_unavailable",
            "This server's address can't hold passkeys. Use an email link.",
        ));
    }
    let bytes = read_body(body).await.ok_or_else(|| {
        json_error(
            StatusCode::PAYLOAD_TOO_LARGE,
            "too_large",
            "The request is too large.",
        )
    })?;
    serde_json::from_slice(&bytes).map_err(|_| {
        json_error(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "The request isn't JSON.",
        )
    })
}

fn json_email(value: &Value) -> Result<String, Response> {
    value["email"]
        .as_str()
        .and_then(normalize_email)
        .ok_or_else(|| {
            json_error(
                StatusCode::BAD_REQUEST,
                "email_invalid",
                "Enter an email address, such as you@example.com.",
            )
        })
}

fn json_ceremony(value: &Value) -> Option<Id> {
    value["ceremony"]
        .as_str()
        .and_then(|text| Id::from_base64url(text).ok())
}

fn expired_ceremony() -> Response {
    json_error(
        StatusCode::BAD_REQUEST,
        "ceremony_expired",
        "That took too long. Start again.",
    )
}

fn passkey_refused() -> Response {
    json_error(
        StatusCode::BAD_REQUEST,
        "passkey_invalid",
        "The passkey didn't check out for this server. Try again.",
    )
}

/// `POST /account/passkeys/sign-up/start`: a registration ceremony for an
/// email. Every email gets one, so the answer reveals nothing about the
/// allowlist.
async fn sign_up_start<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let accounts = &state.accounts;
    let value = match json_body(accounts, &headers, body).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let email = match json_email(&value) {
        Ok(email) => email,
        Err(response) => return response,
    };
    let account = match state.store.account_id(&email) {
        Ok(account) => account,
        Err(error) => return json_failure(error),
    };
    let webauthn = accounts.webauthn.as_ref().expect("checked in json_body");
    let (options, registration) =
        match webauthn.start_passkey_registration(user_handle(account), &email, &email, None) {
            Ok(started) => started,
            Err(error) => return json_failure(error),
        };
    let state_bytes = match serde_json::to_vec(&registration) {
        Ok(bytes) => bytes,
        Err(error) => return json_failure(error),
    };
    match state
        .store
        .begin_ceremony(
            CeremonyKind::SignUp,
            Some(account),
            Some(email),
            state_bytes,
        )
        .await
    {
        Ok(ceremony) => json_reply(
            StatusCode::OK,
            json!({ "ceremony": ceremony.to_base64url(), "options": options }),
        ),
        Err(error) => json_failure(error),
    }
}

/// `POST /account/passkeys/sign-up/finish`: checks the new passkey and, for
/// an allowed email, holds it and mails the link that confirms the email.
/// Every email gets the same answer.
async fn sign_up_finish<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let accounts = &state.accounts;
    let value = match json_body(accounts, &headers, body).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(id) = json_ceremony(&value) else {
        return expired_ceremony();
    };
    let Ok(credential) =
        serde_json::from_value::<RegisterPublicKeyCredential>(value["credential"].clone())
    else {
        return passkey_refused();
    };
    let row = match state.store.finish_ceremony(id, CeremonyKind::SignUp).await {
        Ok(Some(row)) => row,
        Ok(None) => return expired_ceremony(),
        Err(error) => return json_failure(error),
    };
    let Ok(registration) = serde_json::from_slice::<PasskeyRegistration>(&row.state) else {
        return json_failure("a stored ceremony doesn't parse");
    };
    let webauthn = accounts.webauthn.as_ref().expect("checked in json_body");
    let Ok(passkey) = webauthn.finish_passkey_registration(&credential, &registration) else {
        return passkey_refused();
    };
    let email = row.email.unwrap_or_default();
    if accounts.allowlist.admits(&email) {
        let bytes = match serde_json::to_vec(&passkey) {
            Ok(bytes) => bytes,
            Err(error) => return json_failure(error),
        };
        let token = match state
            .store
            .hold_passkey(&email, passkey.cred_id().to_vec(), bytes)
            .await
        {
            Ok(token) => token,
            Err(error) => return json_failure(error),
        };
        let text = format!(
            "Confirm your email to finish signing up for Noura with your new passkey. The link works once, within 10 minutes.\n\n{}\n\nIf you didn't sign up, ignore this email.\n",
            accounts.link("/account/confirm", &token)
        );
        if let Err(error) = accounts
            .send(&email, "Confirm your Noura account", text)
            .await
        {
            if let Err(drop) = state.store.drop_held_passkey(&token).await {
                return json_failure(drop);
            }
            tracing::error!(cause = %error, "a sign-up email failed to send");
        }
    }
    json_reply(StatusCode::OK, json!({ "status": "check_email" }))
}

/// The passkeys of an account, as webauthn-rs reads them.
async fn account_passkeys<B: Backend>(
    store: &Store<B>,
    account: Id,
) -> Result<Vec<Passkey>, Response> {
    let rows = store.passkeys(account).await.map_err(json_failure)?;
    rows.iter()
        .map(|row| serde_json::from_slice::<Passkey>(&row.passkey))
        .collect::<Result<_, _>>()
        .map_err(|e| json_failure(format!("a stored passkey doesn't parse: {e}")))
}

/// `POST /account/passkeys/sign-in/start`: an authentication ceremony for
/// the passkeys of an email's account.
async fn sign_in_start<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let accounts = &state.accounts;
    let value = match json_body(accounts, &headers, body).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let email = match json_email(&value) {
        Ok(email) => email,
        Err(response) => return response,
    };
    let account = match state.store.account_id(&email) {
        Ok(account) => account,
        Err(error) => return json_failure(error),
    };
    let passkeys = match account_passkeys(&state.store, account).await {
        Ok(passkeys) => passkeys,
        Err(response) => return response,
    };
    if passkeys.is_empty() {
        return json_error(
            StatusCode::NOT_FOUND,
            "no_passkey",
            "No passkey signs in to that address. Use an email link instead.",
        );
    }
    let webauthn = accounts.webauthn.as_ref().expect("checked in json_body");
    let (options, authentication) = match webauthn.start_passkey_authentication(&passkeys) {
        Ok(started) => started,
        Err(error) => return json_failure(error),
    };
    let state_bytes = match serde_json::to_vec(&authentication) {
        Ok(bytes) => bytes,
        Err(error) => return json_failure(error),
    };
    match state
        .store
        .begin_ceremony(CeremonyKind::SignIn, Some(account), None, state_bytes)
        .await
    {
        Ok(ceremony) => json_reply(
            StatusCode::OK,
            json!({ "ceremony": ceremony.to_base64url(), "options": options }),
        ),
        Err(error) => json_failure(error),
    }
}

/// `POST /account/passkeys/sign-in/finish`: checks the assertion, moves the
/// passkey's counter and starts a web session.
async fn sign_in_finish<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let accounts = &state.accounts;
    let value = match json_body(accounts, &headers, body).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let Some(id) = json_ceremony(&value) else {
        return expired_ceremony();
    };
    let Ok(credential) = serde_json::from_value::<PublicKeyCredential>(value["credential"].clone())
    else {
        return passkey_refused();
    };
    let row = match state.store.finish_ceremony(id, CeremonyKind::SignIn).await {
        Ok(Some(row)) => row,
        Ok(None) => return expired_ceremony(),
        Err(error) => return json_failure(error),
    };
    let Some(account) = row.account else {
        return expired_ceremony();
    };
    let Ok(authentication) = serde_json::from_slice::<PasskeyAuthentication>(&row.state) else {
        return json_failure("a stored ceremony doesn't parse");
    };
    let webauthn = accounts.webauthn.as_ref().expect("checked in json_body");
    let Ok(result) = webauthn.finish_passkey_authentication(&credential, &authentication) else {
        return passkey_refused();
    };
    let stored = match state.store.passkey(result.cred_id().to_vec()).await {
        Ok(Some(stored)) if stored.account == account => stored,
        Ok(_) => return passkey_refused(),
        Err(error) => return json_failure(error),
    };
    let Ok(mut passkey) = serde_json::from_slice::<Passkey>(&stored.passkey) else {
        return json_failure("a stored passkey doesn't parse");
    };
    if passkey.update_credential(&result) == Some(true) {
        let bytes = match serde_json::to_vec(&passkey) {
            Ok(bytes) => bytes,
            Err(error) => return json_failure(error),
        };
        if let Err(error) = state
            .store
            .update_passkey(stored.credential_id, bytes)
            .await
        {
            return json_failure(error);
        }
    }
    match state.store.start_web_session(account).await {
        Ok(session) => with_cookie(
            json_reply(StatusCode::OK, json!({ "status": "signed_in" })),
            session_cookie(accounts, &session),
        ),
        Err(error) => json_failure(error),
    }
}

fn not_signed_in() -> Response {
    json_error(StatusCode::UNAUTHORIZED, "unauthorized", "Sign in first.")
}

/// `POST /account/passkeys/add/start`: a registration ceremony for another
/// passkey of the signed-in account.
async fn add_start<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let accounts = &state.accounts;
    if let Err(response) = json_body(accounts, &headers, body).await {
        return response;
    }
    let account = match signed_in(&state.store, &headers).await {
        Ok(Some((account, _))) => account,
        Ok(None) => return not_signed_in(),
        Err(error) => return json_failure(error),
    };
    let email = match state.store.account_email(account).await {
        Ok(Some(email)) => email,
        Ok(None) => return not_signed_in(),
        Err(error) => return json_failure(error),
    };
    let existing = match account_passkeys(&state.store, account).await {
        Ok(passkeys) => passkeys.iter().map(|p| p.cred_id().clone()).collect(),
        Err(response) => return response,
    };
    let webauthn = accounts.webauthn.as_ref().expect("checked in json_body");
    let (options, registration) = match webauthn.start_passkey_registration(
        user_handle(account),
        &email,
        &email,
        Some(existing),
    ) {
        Ok(started) => started,
        Err(error) => return json_failure(error),
    };
    let state_bytes = match serde_json::to_vec(&registration) {
        Ok(bytes) => bytes,
        Err(error) => return json_failure(error),
    };
    match state
        .store
        .begin_ceremony(CeremonyKind::AddPasskey, Some(account), None, state_bytes)
        .await
    {
        Ok(ceremony) => json_reply(
            StatusCode::OK,
            json!({ "ceremony": ceremony.to_base64url(), "options": options }),
        ),
        Err(error) => json_failure(error),
    }
}

/// `POST /account/passkeys/add/finish`: checks the new passkey and adds it
/// to the signed-in account.
async fn add_finish<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let accounts = &state.accounts;
    let value = match json_body(accounts, &headers, body).await {
        Ok(value) => value,
        Err(response) => return response,
    };
    let account = match signed_in(&state.store, &headers).await {
        Ok(Some((account, _))) => account,
        Ok(None) => return not_signed_in(),
        Err(error) => return json_failure(error),
    };
    let Some(id) = json_ceremony(&value) else {
        return expired_ceremony();
    };
    let Ok(credential) =
        serde_json::from_value::<RegisterPublicKeyCredential>(value["credential"].clone())
    else {
        return passkey_refused();
    };
    let row = match state
        .store
        .finish_ceremony(id, CeremonyKind::AddPasskey)
        .await
    {
        Ok(Some(row)) if row.account == Some(account) => row,
        Ok(_) => return expired_ceremony(),
        Err(error) => return json_failure(error),
    };
    let Ok(registration) = serde_json::from_slice::<PasskeyRegistration>(&row.state) else {
        return json_failure("a stored ceremony doesn't parse");
    };
    let webauthn = accounts.webauthn.as_ref().expect("checked in json_body");
    let Ok(passkey) = webauthn.finish_passkey_registration(&credential, &registration) else {
        return passkey_refused();
    };
    let bytes = match serde_json::to_vec(&passkey) {
        Ok(bytes) => bytes,
        Err(error) => return json_failure(error),
    };
    match state
        .store
        .add_passkey(PasskeyRow {
            credential_id: passkey.cred_id().to_vec(),
            account,
            passkey: bytes,
        })
        .await
    {
        Ok(true) => json_reply(StatusCode::OK, json!({ "status": "added" })),
        Ok(false) => json_error(
            StatusCode::CONFLICT,
            "passkey_taken",
            "That passkey already signs in to an account.",
        ),
        Err(error) => json_failure(error),
    }
}

const ACCOUNT_CSS: &str = include_str!("../../assets/account.css");
const ACCOUNT_JS: &str = include_str!("../../assets/account.js");

/// `GET /account/assets/{file}`: the page style and module loader, which
/// the server embeds, and the account-web build with its notices.
async fn asset<B: Backend>(State(state): State<AppState<B>>, Path(file): Path<String>) -> Response {
    let reply = |content_type: &'static str, body: Vec<u8>| {
        (
            StatusCode::OK,
            [
                (CONTENT_TYPE, HeaderValue::from_static(content_type)),
                (CACHE_CONTROL, HeaderValue::from_static("no-cache")),
            ],
            body,
        )
            .into_response()
    };
    match file.as_str() {
        "account.css" => return reply("text/css; charset=utf-8", ACCOUNT_CSS.into()),
        "account.js" => return reply("text/javascript; charset=utf-8", ACCOUNT_JS.into()),
        _ => {}
    }
    let found = BUILT_ASSETS.iter().find(|(name, _)| *name == file);
    let (Some((name, content_type)), Some(dir)) = (found, state.accounts.assets.as_ref()) else {
        return (StatusCode::NOT_FOUND, "no such file").into_response();
    };
    match tokio::fs::read(dir.join(name)).await {
        Ok(bytes) => reply(content_type, bytes),
        Err(_) => (StatusCode::NOT_FOUND, "no such file").into_response(),
    }
}

/// Deletes sign-in rows 10 minutes after the store issued them, twice a
/// minute, until the server stops.
pub fn spawn_expiry<B: Backend>(store: Arc<Store<B>>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(30));
        loop {
            tick.tick().await;
            if let Err(error) = store.delete_expired().await {
                tracing::error!(cause = %error, "deleting expired sign-in rows failed");
            }
        }
    })
}
