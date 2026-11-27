//! A fake of OpenAI's authorization server and Responses API, on this
//! computer, for tests of Sign in with ChatGPT. It signs ID tokens with a
//! synthetic test key, rotates refresh tokens, records each revocation
//! and each Responses request, and answers with one short answer that
//! calls a tool. It runs on a thread of its own, so sync and async tests
//! share it. Only test builds have it.

use std::{
    collections::HashMap,
    io::{Read, Write},
    net::TcpStream as StdStream,
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use base64::{Engine as _, engine::general_purpose::URL_SAFE_NO_PAD};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};

use super::chatgpt::ChatGptEndpoints;

/// A synthetic RSA key made for these tests. It signs nothing else.
const KEY: &[u8] = include_bytes!("../../tests/fixtures/chatgpt-test-signing-key.pk8");
/// The client ID the fake issues at registration.
pub const CLIENT: &str = "oaiapp_test_client";
/// The account the fake signs you in to.
pub const SUBJECT: &str = "user_test_subject";
pub const EMAIL: &str = "you@example.com";

/// What the fake has seen, and how it answers.
#[derive(Debug, Default, Clone)]
pub struct FakeState {
    pub issuer: String,
    /// The nonce and PKCE challenge of the sign-in under way.
    pub nonce: String,
    pub challenge: String,
    /// The ID token's `audience`, `nonce` or `signature` to get wrong.
    pub wrong: Option<&'static str>,
    /// The scopes the token response grants.
    pub scope: String,
    /// The refresh token the fake accepts now, and each one it used.
    pub refresh: String,
    pub used: Vec<String>,
    pub refreshes: usize,
    /// The access token the fake accepts now, and how many it issued.
    pub access: String,
    pub issued: usize,
    /// How long each access token lasts, in seconds.
    pub expires_in: u64,
    /// Refuse every refresh, as after you disconnect the app in ChatGPT.
    pub refuse_refresh: bool,
    /// Whether the answer calls `add_task` after its text.
    pub tool_call: bool,
    /// Each revocation: the token and the client ID.
    pub revoked: Vec<(String, String)>,
    /// Each Responses request: its Authorization header and its body.
    pub responses: Vec<(String, Value)>,
}

/// The fake, running.
pub struct FakeOpenAi {
    pub endpoints: ChatGptEndpoints,
    state: Arc<Mutex<FakeState>>,
}

impl FakeOpenAi {
    /// Start the fake on a port of this computer.
    pub fn start() -> Self {
        let (ready, port) = std::sync::mpsc::channel();
        let state = Arc::new(Mutex::new(FakeState {
            scope: "openid profile email offline_access resource.invoke chatgpt.tokens.use.direct"
                .into(),
            expires_in: 3600,
            ..FakeState::default()
        }));
        let shared = state.clone();
        std::thread::Builder::new()
            .name("fake-openai".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .expect("a runtime for the fake");
                runtime.block_on(async move {
                    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
                    let _ = ready.send(listener.local_addr().expect("an address"));
                    loop {
                        let Ok((socket, _)) = listener.accept().await else {
                            return;
                        };
                        let state = shared.clone();
                        tokio::spawn(async move { serve(socket, state).await });
                    }
                });
            })
            .expect("the fake's thread");
        let address = port.recv().expect("the fake's address");
        let base = format!("http://{address}");
        state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .issuer
            .clone_from(&base);
        Self {
            endpoints: ChatGptEndpoints {
                issuer: base.clone(),
                api: format!("{base}/v1"),
            },
            state,
        }
    }

    /// What the fake has seen, and its answers to change.
    pub fn state(&self) -> MutexGuard<'_, FakeState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Act as the browser for the sign-in at `url`: note its nonce and
    /// challenge, send a forged callback with another state first, then
    /// come back with a code. Returns the status the real callback got.
    pub fn come_back(&self, url: &str) -> u16 {
        let asked = query(url);
        {
            let mut state = self.state();
            state.nonce = asked.get("nonce").cloned().unwrap_or_default();
            state.challenge = asked.get("code_challenge").cloned().unwrap_or_default();
        }
        let redirect = asked.get("redirect_uri").cloned().unwrap_or_default();
        let forged = get(&format!(
            "{redirect}?code=code_evil&state=forged&client_id=oaiapp_evil"
        ));
        assert_eq!(forged, 400, "a forged callback must fail");
        get(&format!(
            "{redirect}?code=code_1&state={}&client_id={CLIENT}\
             &scope=chatgpt.tokens.use.direct+email+offline_access+openid+profile+resource.invoke",
            asked.get("state").cloned().unwrap_or_default()
        ))
    }

    /// Act as the browser when you decline: come back with
    /// `access_denied`.
    pub fn decline(&self, url: &str) -> u16 {
        let asked = query(url);
        get(&format!(
            "{}?error=access_denied&state={}",
            asked.get("redirect_uri").cloned().unwrap_or_default(),
            asked.get("state").cloned().unwrap_or_default()
        ))
    }
}

/// The query of the address Noura opens in the browser.
pub fn query(url: &str) -> HashMap<String, String> {
    form(url.split_once('?').map_or("", |(_, query)| query))
}

/// The status of a GET to `url` on this computer.
fn get(url: &str) -> u16 {
    let Ok(parsed) = url::Url::parse(url) else {
        return 0;
    };
    let address = format!(
        "{}:{}",
        parsed.host_str().unwrap_or("127.0.0.1"),
        parsed.port().unwrap_or(80)
    );
    let Ok(mut stream) = StdStream::connect(address) else {
        return 0;
    };
    let target = match parsed.query() {
        Some(query) => format!("{}?{query}", parsed.path()),
        None => parsed.path().to_owned(),
    };
    let request = format!("GET {target} HTTP/1.1\r\nhost: 127.0.0.1\r\nconnection: close\r\n\r\n");
    if stream.write_all(request.as_bytes()).is_err() {
        return 0;
    }
    let mut response = String::new();
    let _ = stream.read_to_string(&mut response);
    response
        .split(' ')
        .nth(1)
        .and_then(|status| status.parse().ok())
        .unwrap_or(0)
}

fn form(body: &str) -> HashMap<String, String> {
    url::form_urlencoded::parse(body.as_bytes())
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

async fn serve(mut socket: TcpStream, state: Arc<Mutex<FakeState>>) {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => return,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
        }
    };
    let head = String::from_utf8_lossy(&bytes[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap_or_default().to_owned();
    let headers: HashMap<String, String> = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.to_lowercase(), value.trim().to_owned()))
        .collect();
    let length: usize = headers
        .get("content-length")
        .and_then(|length| length.parse().ok())
        .unwrap_or(0);
    while bytes.len() < head_end + 4 + length {
        match socket.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => bytes.extend_from_slice(&chunk[..read]),
        }
    }
    let body = String::from_utf8_lossy(&bytes[head_end + 4..]).to_string();
    let mut parts = request_line.split(' ');
    let (method, path) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let (status, content_type, answer) = {
        let mut state = state.lock().unwrap_or_else(PoisonError::into_inner);
        route(method, path, &headers, &body, &mut state)
    };
    let response = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\n\
         connection: close\r\n\r\n{answer}",
        answer.len()
    );
    let _ = socket.write_all(response.as_bytes()).await;
    let _ = socket.shutdown().await;
}

fn route(
    method: &str,
    path: &str,
    headers: &HashMap<String, String>,
    body: &str,
    fake: &mut FakeState,
) -> (&'static str, &'static str, String) {
    let json_type = "application/json";
    let refuse = |error: &str| {
        (
            "400 Bad Request",
            json_type,
            json!({ "error": error }).to_string(),
        )
    };
    match (method, path) {
        ("GET", "/.well-known/openid-configuration") => (
            "200 OK",
            json_type,
            json!({
                "issuer": fake.issuer,
                "jwks_uri": format!("{}/jwks", fake.issuer),
                "revocation_endpoint": format!("{}/revoke", fake.issuer),
            })
            .to_string(),
        ),
        ("GET", "/jwks") => ("200 OK", json_type, jwks().to_string()),
        ("POST", "/api/accounts/oauth/token") => {
            let fields = form(body);
            match fields.get("grant_type").map(String::as_str) {
                Some("authorization_code") => {
                    let verifier = fields.get("code_verifier").cloned().unwrap_or_default();
                    let challenge = URL_SAFE_NO_PAD.encode(ring::digest::digest(
                        &ring::digest::SHA256,
                        verifier.as_bytes(),
                    ));
                    let good = fields.get("code").map(String::as_str) == Some("code_1")
                        && fields.get("client_id").map(String::as_str) == Some(CLIENT)
                        && fields.get("resource").map(String::as_str)
                            == Some("https://api.openai.com/v1")
                        && challenge == fake.challenge;
                    if !good {
                        return refuse("invalid_grant");
                    }
                    let id_token = id_token(fake);
                    (
                        "200 OK",
                        json_type,
                        tokens(fake, Some(id_token)).to_string(),
                    )
                }
                Some("refresh_token") => {
                    fake.refreshes += 1;
                    let given = fields.get("refresh_token").cloned().unwrap_or_default();
                    if fake.used.contains(&given) {
                        return refuse("refresh_token_reused");
                    }
                    if fake.refuse_refresh || given != fake.refresh {
                        return refuse("invalid_grant");
                    }
                    fake.used.push(given);
                    ("200 OK", json_type, tokens(fake, None).to_string())
                }
                _ => refuse("unsupported_grant_type"),
            }
        }
        ("POST", "/revoke") => {
            let fields = form(body);
            fake.revoked.push((
                fields.get("token").cloned().unwrap_or_default(),
                fields.get("client_id").cloned().unwrap_or_default(),
            ));
            ("200 OK", "text/plain", String::new())
        }
        ("GET", "/v1/models") => (
            "200 OK",
            json_type,
            json!({ "models": [
                { "slug": "gpt-6.2-sol", "display_name": "GPT-6.2 Sol", "visibility": "list" },
                { "slug": "internal", "display_name": "Internal", "visibility": "hide" },
            ] })
            .to_string(),
        ),
        ("POST", "/v1/responses") => {
            let authorization = headers.get("authorization").cloned().unwrap_or_default();
            let body: Value = serde_json::from_str(body).unwrap_or(Value::Null);
            fake.responses.push((authorization.clone(), body));
            if authorization != format!("Bearer {}", fake.access) {
                return (
                    "401 Unauthorized",
                    json_type,
                    json!({ "error": { "code": "invalid_api_key", "message": "x" } }).to_string(),
                );
            }
            ("200 OK", "text/event-stream", answer_events(fake.tool_call))
        }
        _ => ("404 Not Found", "text/plain", String::new()),
    }
}

/// A token response with a new access token and a new refresh token.
fn tokens(fake: &mut FakeState, id_token: Option<String>) -> Value {
    fake.issued += 1;
    fake.access = format!("access_{}", fake.issued);
    fake.refresh = format!("refresh_{}", fake.issued);
    let mut tokens = json!({
        "access_token": fake.access,
        "refresh_token": fake.refresh,
        "token_type": "Bearer",
        "expires_in": fake.expires_in,
        "scope": fake.scope,
        "earliest_refresh_at": 0,
    });
    if let Some(id_token) = id_token {
        tokens["id_token"] = json!(id_token);
    }
    tokens
}

fn key_pair() -> ring::signature::RsaKeyPair {
    ring::signature::RsaKeyPair::from_pkcs8(KEY).expect("the test key")
}

fn jwks() -> Value {
    let public = ring::rsa::PublicKeyComponents::<Vec<u8>>::from(key_pair().public());
    json!({ "keys": [
        { "kty": "RSA", "kid": "other", "n": URL_SAFE_NO_PAD.encode([1u8; 256]), "e": "AQAB" },
        { "kty": "RSA", "kid": "test-key", "alg": "RS256", "use": "sig",
          "n": URL_SAFE_NO_PAD.encode(&public.n), "e": URL_SAFE_NO_PAD.encode(&public.e) },
    ] })
}

/// An ID token signed with the test key, wrong in the way `fake` says.
fn id_token(fake: &FakeState) -> String {
    let header = URL_SAFE_NO_PAD.encode(json!({ "alg": "RS256", "kid": "test-key" }).to_string());
    let now = super::chatgpt::now();
    let audience = if fake.wrong == Some("audience") {
        "oaiapp_someone_else"
    } else {
        CLIENT
    };
    let nonce = if fake.wrong == Some("nonce") {
        "another"
    } else {
        fake.nonce.as_str()
    };
    let claims = json!({
        "iss": fake.issuer,
        "aud": audience,
        "sub": SUBJECT,
        "email": EMAIL,
        "iat": now,
        "exp": now + 3600,
        "nonce": nonce,
    });
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signed = format!("{header}.{payload}");
    let key = key_pair();
    let mut signature = vec![0u8; key.public().modulus_len()];
    key.sign(
        &ring::signature::RSA_PKCS1_SHA256,
        &ring::rand::SystemRandom::new(),
        signed.as_bytes(),
        &mut signature,
    )
    .expect("a signature");
    if fake.wrong == Some("signature") {
        signature[10] ^= 0xff;
    }
    format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
}

/// One short answer: text, then, with `tool_call`, a call to `add_task`
/// in Noura's namespace.
fn answer_events(tool_call: bool) -> String {
    let mut events = vec![
        json!({ "type": "response.created", "response": { "id": "resp_1", "status": "in_progress" } }),
        json!({ "type": "response.output_text.delta", "delta": "Adding " }),
        json!({ "type": "response.output_text.delta", "delta": "it." }),
    ];
    if tool_call {
        events.push(json!({ "type": "response.output_item.done", "item": {
            "type": "function_call", "call_id": "call_1", "name": "add_task",
            "namespace": "noura", "arguments": "{\"title\":\"Plant\"}",
        } }));
    }
    events.push(json!({ "type": "response.completed", "response": {
        "id": "resp_1", "status": "completed",
        "usage": { "input_tokens": 11, "output_tokens": 7, "total_tokens": 18 },
    } }));
    events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap_or("message")
            )
        })
        .collect()
}
