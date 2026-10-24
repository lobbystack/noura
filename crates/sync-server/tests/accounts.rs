//! Accounts at seam 4 (§7, §14, §29): sign-up and sign-in with a magic link
//! or a software passkey, a second passkey, the email allowlist, account
//! IDs from the server secret, tokens stored only as hashes, mail through
//! fake SMTP and Resend transports, the account pages' content security
//! policy, and the public origin.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use passkey_authenticator::{Authenticator, MemoryStore, UiHint, UserCheck, UserValidationMethod};
use passkey_client::{Client as PasskeyClient, DefaultClientData};
use passkey_types::ctap2::{Aaguid, Ctap2Error};
use passkey_types::webauthn::{CredentialCreationOptions, CredentialRequestOptions};
use reqwest::StatusCode;
use reqwest::header::HeaderMap;
use serde_json::{Value, json};
use sqlx::{Connection, Row, SqliteConnection};
use sync_server::accounts::mail::{Mail, MailError, MailTransport, Resend, SendFuture, Smtp};
use sync_server::{Accounts, Allowlist, PublicOrigin};
use sync_store::Store;
use sync_store_sqlite::{DATABASE_FILE, SqliteBackend};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

/// Keeps every message it sends.
#[derive(Clone, Default)]
struct Outbox(Arc<Mutex<Vec<Mail>>>);

impl MailTransport for Outbox {
    fn send<'a>(&'a self, mail: &'a Mail) -> SendFuture<'a> {
        self.0.lock().unwrap().push(mail.clone());
        Box::pin(async { Ok(()) })
    }
}

/// Fails every send, as a provider outage would.
struct Broken;

impl MailTransport for Broken {
    fn send<'a>(&'a self, _mail: &'a Mail) -> SendFuture<'a> {
        Box::pin(async { Err(MailError("the provider is down".into())) })
    }
}

impl Outbox {
    fn take(&self) -> Vec<Mail> {
        std::mem::take(&mut *self.0.lock().unwrap())
    }
}

/// A server with account pages, its data directory and an HTTP client
/// that behaves like a browser on those pages: it sends the page origin
/// with every post and follows no redirect.
struct Site {
    base: String,
    origin: String,
    store: Arc<Store<SqliteBackend>>,
    http: reqwest::Client,
    data: PathBuf,
    _dir: tempfile::TempDir,
}

struct Reply {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Reply {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or_else(|_| panic!("JSON: {}", self.body))
    }

    /// The web session cookie a response sets, as a request sends it.
    fn session_cookie(&self) -> Option<String> {
        self.headers
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|v| v.starts_with("noura_session=") && !v.contains("Max-Age=0"))
            .map(|v| v.split(';').next().unwrap().to_owned())
    }
}

async fn site(allowed: &str, mailer: Box<dyn MailTransport>) -> Site {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    site_in(dir, data, allowed, mailer).await
}

async fn site_in(
    dir: tempfile::TempDir,
    data: PathBuf,
    allowed: &str,
    mailer: Box<dyn MailTransport>,
) -> Site {
    sync_server::migrate(&data).await.expect("migrated");
    let store = Arc::new(
        sync_server::open_store(&sync_server::Config::new(
            data.clone(),
            "127.0.0.1:0".parse().unwrap(),
        ))
        .await
        .expect("the store opens"),
    );
    let allowlist = Allowlist::parse(allowed);
    let address = sync_server::spawn_with(store.clone(), "127.0.0.1:0", |origin| {
        Accounts::new(
            origin,
            allowlist,
            "Noura <noura@example.com>".into(),
            mailer,
            None,
        )
    })
    .await
    .expect("serving");
    let base = format!("http://localhost:{}", address.port());
    Site {
        origin: base.clone(),
        base,
        store,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        data,
        _dir: dir,
    }
}

impl Site {
    async fn send(&self, request: reqwest::RequestBuilder) -> Reply {
        let response = request.send().await.expect("the server answers");
        Reply {
            status: response.status(),
            headers: response.headers().clone(),
            body: response.text().await.expect("a body"),
        }
    }

    async fn get(&self, path: &str, cookie: Option<&str>) -> Reply {
        let mut request = self.http.get(format!("{}{path}", self.base));
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        self.send(request).await
    }

    async fn form(&self, path: &str, fields: &[(&str, &str)], cookie: Option<&str>) -> Reply {
        let body = url::form_urlencoded::Serializer::new(String::new())
            .extend_pairs(fields)
            .finish();
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header("origin", &self.origin)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(body);
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        self.send(request).await
    }

    async fn json(&self, path: &str, value: Value, cookie: Option<&str>) -> Reply {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header("origin", &self.origin)
            .header("content-type", "application/json")
            .body(value.to_string());
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        self.send(request).await
    }

    fn database(&self) -> PathBuf {
        self.data.join(DATABASE_FILE)
    }

    /// Follows a mailed link: opens its page, then posts the token that
    /// page's form carries, as the button does.
    async fn follow(&self, link: &str) -> Reply {
        let path = link
            .strip_prefix(&self.base)
            .expect("a link to this server");
        let page = self.get(path, None).await;
        assert_eq!(page.status, StatusCode::OK);
        let action = between(&page.body, "action=\"", "\"");
        let token = between(&page.body, "name=\"token\" value=\"", "\"");
        // Opening the page used nothing: a mail scanner that fetches links
        // leaves the link working.
        self.form(&action, &[("token", &token)], None).await
    }
}

fn between(text: &str, start: &str, end: &str) -> String {
    let from = text
        .find(start)
        .unwrap_or_else(|| panic!("{start} in {text}"))
        + start.len();
    let to = text[from..].find(end).expect("an end") + from;
    text[from..to].to_owned()
}

/// The link in a message.
fn link_in(mail: &Mail) -> String {
    mail.text
        .split_whitespace()
        .find(|word| word.starts_with("http"))
        .expect("a link")
        .to_owned()
}

/// Every BLOB and TEXT value in every table of a database, by table and
/// column.
async fn every_value(database: &Path) -> Vec<(String, String, Vec<u8>)> {
    let mut connection =
        SqliteConnection::connect(&format!("sqlite://{}?mode=ro", database.display()))
            .await
            .expect("the database opens");
    let tables: Vec<String> = sqlx::query("SELECT name FROM sqlite_master WHERE type = 'table'")
        .fetch_all(&mut connection)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect();
    let mut values = Vec::new();
    for table in tables {
        let columns: Vec<String> = sqlx::query(sqlx::AssertSqlSafe(format!(
            "PRAGMA table_info(\"{table}\")"
        )))
        .fetch_all(&mut connection)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>("name"))
        .collect();
        for column in columns {
            let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
                "SELECT CAST(\"{column}\" AS BLOB) FROM \"{table}\""
            )))
            .fetch_all(&mut connection)
            .await
            .unwrap();
            for row in rows {
                if let Some(value) = row.get::<Option<Vec<u8>>, _>(0) {
                    values.push((table.clone(), column.clone(), value));
                }
            }
        }
    }
    values
}

async fn columns(database: &Path, table: &str) -> Vec<String> {
    let mut connection =
        SqliteConnection::connect(&format!("sqlite://{}?mode=ro", database.display()))
            .await
            .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "PRAGMA table_info(\"{table}\")"
    )))
    .fetch_all(&mut connection)
    .await
    .unwrap()
    .iter()
    .map(|r| r.get::<String, _>("name"))
    .collect()
}

async fn count(database: &Path, table: &str) -> i64 {
    let mut connection =
        SqliteConnection::connect(&format!("sqlite://{}?mode=ro", database.display()))
            .await
            .unwrap();
    sqlx::query(sqlx::AssertSqlSafe(format!(
        "SELECT COUNT(*) FROM \"{table}\""
    )))
    .fetch_one(&mut connection)
    .await
    .unwrap()
    .get::<i64, _>(0)
}

/// Whether HTML holds an attribute such as `onclick=`.
fn has_inline_handler(html: &str) -> bool {
    html.match_indices(" on").any(|(at, _)| {
        let name: String = html[at + 3..]
            .chars()
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        !name.is_empty() && html[at + 3 + name.len()..].starts_with('=')
    })
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// A software passkey: an authenticator that approves every request with
/// user verification.
struct Approve;

#[async_trait::async_trait]
impl UserValidationMethod for Approve {
    type PasskeyItem = passkey_types::Passkey;

    async fn check_user<'a>(
        &self,
        _hint: UiHint<'a, Self::PasskeyItem>,
        presence: bool,
        verification: bool,
    ) -> Result<UserCheck, Ctap2Error> {
        Ok(UserCheck {
            presence,
            verification,
        })
    }

    fn is_presence_enabled(&self) -> bool {
        true
    }

    fn is_verification_enabled(&self) -> Option<bool> {
        Some(true)
    }
}

type Passkey = PasskeyClient<
    MemoryStore,
    Approve,
    passkey_crypto::AvailableBackend,
    public_suffix::PublicSuffixList,
    (),
>;

fn passkey() -> Passkey {
    PasskeyClient::new(Authenticator::new(
        Aaguid::new_empty(),
        MemoryStore::new(),
        Approve,
        passkey_crypto::AvailableBackend,
    ))
    .allows_insecure_localhost(true)
}

/// Creates a credential for the server's registration options, as a
/// browser at `origin` would.
async fn create(passkey: &mut Passkey, origin: &str, options: &Value) -> Value {
    let options: CredentialCreationOptions =
        serde_json::from_value(options.clone()).expect("creation options");
    let origin = url::Url::parse(origin).unwrap();
    let credential = passkey
        .register(&origin, options, DefaultClientData)
        .await
        .expect("the passkey registers");
    serde_json::to_value(credential).unwrap()
}

/// Signs the server's authentication options, as a browser at `origin`
/// would.
async fn assert(passkey: &mut Passkey, origin: &str, options: &Value) -> Value {
    let options: CredentialRequestOptions =
        serde_json::from_value(options.clone()).expect("request options");
    let origin = url::Url::parse(origin).unwrap();
    let credential = passkey
        .authenticate(&origin, options, DefaultClientData)
        .await
        .expect("the passkey signs");
    serde_json::to_value(credential).unwrap()
}

/// Signs up `email` with a passkey and confirms the emailed link. Returns
/// the web session cookie.
async fn passkey_sign_up(site: &Site, outbox: &Outbox, key: &mut Passkey, email: &str) -> String {
    let started = site
        .json(
            "/account/passkeys/sign-up/start",
            json!({ "email": email }),
            None,
        )
        .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let started = started.json();
    let credential = create(key, &site.origin, &started["options"]).await;
    let finished = site
        .json(
            "/account/passkeys/sign-up/finish",
            json!({ "ceremony": started["ceremony"], "credential": credential }),
            None,
        )
        .await;
    assert_eq!(finished.status, StatusCode::OK, "{}", finished.body);
    assert_eq!(finished.json()["status"], "check_email");
    let mail = outbox.take().pop().expect("a confirmation mail");
    assert_eq!(mail.to, email.to_ascii_lowercase());
    let confirmed = site.follow(&link_in(&mail)).await;
    assert_eq!(
        confirmed.status,
        StatusCode::SEE_OTHER,
        "{}",
        confirmed.body
    );
    assert_eq!(confirmed.headers["location"], "/account");
    confirmed.session_cookie().expect("a session cookie")
}

/// Signs in to `email` with `key`, returning the reply of the finish step.
async fn passkey_sign_in(site: &Site, key: &mut Passkey, email: &str) -> Reply {
    let started = site
        .json(
            "/account/passkeys/sign-in/start",
            json!({ "email": email }),
            None,
        )
        .await;
    if started.status != StatusCode::OK {
        return started;
    }
    let started = started.json();
    let credential = assert(key, &site.origin, &started["options"]).await;
    site.json(
        "/account/passkeys/sign-in/finish",
        json!({ "ceremony": started["ceremony"], "credential": credential }),
        None,
    )
    .await
}

async fn account_page(site: &Site, cookie: &str) -> Reply {
    let page = site.get("/account", Some(cookie)).await;
    assert_eq!(page.status, StatusCode::OK, "{}", page.body);
    page
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_magic_link_signs_up_once_and_expires_after_ten_minutes() {
    let outbox = Outbox::default();
    let site = site("alice@example.com", Box::new(outbox.clone())).await;

    // Sign up: the link goes to the address, works once, and signs you in.
    let reply = site
        .form(
            "/account/email-link",
            &[("email", " Alice@Example.COM ")],
            None,
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(reply.body.contains("Check your email"));
    let mail = outbox.take().pop().expect("a sign-in mail");
    assert_eq!(mail.to, "alice@example.com");
    assert_eq!(mail.subject, "Sign in to Noura");
    let link = link_in(&mail);
    assert!(link.starts_with(&format!("{}/account/email-link?token=", site.base)));
    assert_eq!(count(&site.database(), "magic_links").await, 1);
    let used = site.follow(&link).await;
    assert_eq!(used.status, StatusCode::SEE_OTHER);
    let cookie = used.session_cookie().expect("a session");
    assert!(
        account_page(&site, &cookie)
            .await
            .body
            .contains("alice@example.com")
    );
    // The server deleted the link's row when it was used, and it works once.
    assert_eq!(count(&site.database(), "magic_links").await, 0);
    let again = site.follow(&link).await;
    assert_eq!(again.status, StatusCode::BAD_REQUEST);
    assert!(again.body.contains("work anymore"), "{}", again.body);

    // A later link signs in to the same account.
    site.form(
        "/account/email-link",
        &[("email", "alice@example.com")],
        None,
    )
    .await;
    let cookie = site
        .follow(&link_in(&outbox.take().pop().unwrap()))
        .await
        .session_cookie()
        .unwrap();
    assert!(
        account_page(&site, &cookie)
            .await
            .body
            .contains("alice@example.com")
    );
    assert_eq!(count(&site.database(), "account_emails").await, 1);

    // A link expires 10 minutes after the server issued it.
    site.form(
        "/account/email-link",
        &[("email", "alice@example.com")],
        None,
    )
    .await;
    let late = link_in(&outbox.take().pop().unwrap());
    site.store.clock().advance(Duration::from_secs(601));
    assert_eq!(site.follow(&late).await.status, StatusCode::BAD_REQUEST);

    // The server deletes an unused link's row 10 minutes after it issued it.
    site.form(
        "/account/email-link",
        &[("email", "alice@example.com")],
        None,
    )
    .await;
    outbox.take();
    assert_eq!(count(&site.database(), "magic_links").await, 1);
    site.store.clock().advance(Duration::from_secs(599));
    assert_eq!(site.store.delete_expired().await.unwrap(), 0);
    site.store.clock().advance(Duration::from_secs(1));
    assert_eq!(site.store.delete_expired().await.unwrap(), 1);
    assert_eq!(count(&site.database(), "magic_links").await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_allowlist_admits_only_its_emails_and_answers_every_email_alike() {
    let outbox = Outbox::default();
    let site = site("alice@example.com", Box::new(outbox.clone())).await;
    let allowed = site
        .form(
            "/account/email-link",
            &[("email", "alice@example.com")],
            None,
        )
        .await;
    let refused = site
        .form(
            "/account/email-link",
            &[("email", "mallory@example.com")],
            None,
        )
        .await;
    assert_eq!(allowed.status, refused.status);
    assert_eq!(
        allowed.body.replace("alice@example.com", "EMAIL"),
        refused.body.replace("mallory@example.com", "EMAIL")
    );
    let sent = outbox.take();
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].to, "alice@example.com");

    // A passkey sign-up answers alike too, and mails only the allowed email.
    let mut key = passkey();
    for email in ["mallory@example.com", "alice@example.com"] {
        let started = site
            .json(
                "/account/passkeys/sign-up/start",
                json!({ "email": email }),
                None,
            )
            .await;
        assert_eq!(started.status, StatusCode::OK);
        let started = started.json();
        let credential = create(&mut key, &site.origin, &started["options"]).await;
        let finished = site
            .json(
                "/account/passkeys/sign-up/finish",
                json!({ "ceremony": started["ceremony"], "credential": credential }),
                None,
            )
            .await;
        assert_eq!(
            (finished.status, finished.json()),
            (StatusCode::OK, json!({ "status": "check_email" }))
        );
    }
    let sent = outbox.take();
    assert_eq!(
        sent.iter().map(|m| m.to.as_str()).collect::<Vec<_>>(),
        ["alice@example.com"]
    );
    assert_eq!(count(&site.database(), "account_emails").await, 0);

    // An empty allowlist admits nobody.
    let outbox = Outbox::default();
    let closed = self::site("", Box::new(outbox.clone())).await;
    let reply = closed
        .form(
            "/account/email-link",
            &[("email", "alice@example.com")],
            None,
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert!(outbox.take().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_passkey_signs_up_after_its_email_confirms_and_signs_in_again() {
    let outbox = Outbox::default();
    let site = site("bob@example.com", Box::new(outbox.clone())).await;
    let mut laptop = passkey();

    // Before the email confirms it, the account doesn't work.
    let started = site
        .json(
            "/account/passkeys/sign-up/start",
            json!({ "email": "Bob@Example.com" }),
            None,
        )
        .await
        .json();
    let credential = create(&mut laptop, &site.origin, &started["options"]).await;
    site.json(
        "/account/passkeys/sign-up/finish",
        json!({ "ceremony": started["ceremony"], "credential": credential }),
        None,
    )
    .await;
    let mail = outbox.take().pop().expect("a confirmation");
    assert_eq!(mail.subject, "Confirm your Noura account");
    let early = passkey_sign_in(&site, &mut laptop, "bob@example.com").await;
    assert_eq!(early.status, StatusCode::NOT_FOUND);
    assert_eq!(early.json()["error"], "no_passkey");
    assert_eq!(count(&site.database(), "account_emails").await, 0);

    // The link confirms the email once and signs you in.
    let confirmed = site.follow(&link_in(&mail)).await;
    let cookie = confirmed.session_cookie().expect("a session");
    let page = account_page(&site, &cookie).await;
    assert!(page.body.contains("bob@example.com"));
    assert!(page.body.contains("You have 1 passkey."));
    assert_eq!(
        site.follow(&link_in(&mail)).await.status,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(count(&site.database(), "pending_passkeys").await, 0);

    // The passkey signs in again, with a new web session.
    let signed_in = passkey_sign_in(&site, &mut laptop, "bob@example.com").await;
    assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
    let second = signed_in.session_cookie().expect("a session");
    assert_ne!(second, cookie);
    account_page(&site, &second).await;

    // A ceremony works once.
    let started = site
        .json(
            "/account/passkeys/sign-in/start",
            json!({ "email": "bob@example.com" }),
            None,
        )
        .await
        .json();
    let credential = assert(&mut laptop, &site.origin, &started["options"]).await;
    let body = json!({ "ceremony": started["ceremony"], "credential": credential });
    let first = site
        .json("/account/passkeys/sign-in/finish", body.clone(), None)
        .await;
    assert_eq!(first.status, StatusCode::OK);
    let replay = site
        .json("/account/passkeys/sign-in/finish", body, None)
        .await;
    assert_eq!(replay.json()["error"], "ceremony_expired");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_second_passkey_signs_in_like_the_first() {
    let outbox = Outbox::default();
    let site = site("carol@example.com", Box::new(outbox.clone())).await;
    let mut laptop = passkey();
    let cookie = passkey_sign_up(&site, &outbox, &mut laptop, "carol@example.com").await;

    let mut phone = passkey();
    let started = site
        .json("/account/passkeys/add/start", json!({}), Some(&cookie))
        .await;
    assert_eq!(started.status, StatusCode::OK, "{}", started.body);
    let started = started.json();
    let credential = create(&mut phone, &site.origin, &started["options"]).await;
    let added = site
        .json(
            "/account/passkeys/add/finish",
            json!({ "ceremony": started["ceremony"], "credential": credential }),
            Some(&cookie),
        )
        .await;
    assert_eq!(added.json()["status"], "added");
    assert!(
        account_page(&site, &cookie)
            .await
            .body
            .contains("You have 2 passkeys.")
    );

    for key in [&mut laptop, &mut phone] {
        let signed_in = passkey_sign_in(&site, key, "carol@example.com").await;
        assert_eq!(signed_in.status, StatusCode::OK, "{}", signed_in.body);
        let session = signed_in.session_cookie().unwrap();
        assert!(
            account_page(&site, &session)
                .await
                .body
                .contains("carol@example.com")
        );
    }

    // Adding a passkey needs a web session.
    let refused = site
        .json("/account/passkeys/add/start", json!({}), None)
        .await;
    assert_eq!(refused.status, StatusCode::UNAUTHORIZED);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn passkeys_bind_to_the_public_origin() {
    let outbox = Outbox::default();
    let site = site("dan@example.com", Box::new(outbox.clone())).await;
    let mut key = passkey();
    let started = site
        .json(
            "/account/passkeys/sign-up/start",
            json!({ "email": "dan@example.com" }),
            None,
        )
        .await
        .json();
    // A page at another origin creates the credential.
    let elsewhere = "http://localhost:1";
    let credential = create(&mut key, elsewhere, &started["options"]).await;
    let finished = site
        .json(
            "/account/passkeys/sign-up/finish",
            json!({ "ceremony": started["ceremony"], "credential": credential }),
            None,
        )
        .await;
    assert_eq!(finished.status, StatusCode::BAD_REQUEST);
    assert_eq!(finished.json()["error"], "passkey_invalid");
    assert!(outbox.take().is_empty());

    // Every post must come from the server's own pages.
    let foreign = site
        .http
        .post(format!("{}/account/email-link", site.base))
        .header("origin", "https://evil.example")
        .header("content-type", "application/x-www-form-urlencoded")
        .body("email=dan%40example.com");
    assert_eq!(site.send(foreign).await.status, StatusCode::FORBIDDEN);
    let missing = site
        .http
        .post(format!("{}/account/passkeys/sign-up/start", site.base))
        .body(json!({ "email": "dan@example.com" }).to_string());
    assert_eq!(site.send(missing).await.status, StatusCode::FORBIDDEN);
    assert!(outbox.take().is_empty());
}

#[test]
fn the_public_origin_is_https_except_on_loopback() {
    let bound = "127.0.0.1:8080".parse().unwrap();
    let anywhere = "0.0.0.0:8080".parse().unwrap();
    for (configured, address, expected) in [
        (
            Some("https://sync.example.com"),
            anywhere,
            Ok("https://sync.example.com"),
        ),
        (
            Some("http://localhost:8080"),
            bound,
            Ok("http://localhost:8080"),
        ),
        (None, bound, Ok("http://localhost:8080")),
        (Some("http://sync.example.com"), anywhere, Err(())),
        (Some("http://192.168.1.4:8080"), anywhere, Err(())),
        (None, anywhere, Err(())),
    ] {
        let origin = sync_server::public_origin(configured, address);
        assert_eq!(
            origin.as_ref().map(PublicOrigin::as_str).map_err(|_| ()),
            expected,
            "{configured:?} on {address}"
        );
    }
}

#[test]
fn serve_refuses_a_plain_http_origin_away_from_loopback() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let migrated = std::process::Command::new(env!("CARGO_BIN_EXE_sync-server"))
        .args(["migrate", "--data"])
        .arg(&data)
        .output()
        .unwrap();
    assert!(migrated.status.success());
    let served = std::process::Command::new(env!("CARGO_BIN_EXE_sync-server"))
        .args([
            "serve",
            "--listen",
            "127.0.0.1:0",
            "--origin",
            "http://sync.example.com",
            "--data",
        ])
        .arg(&data)
        .output()
        .unwrap();
    assert!(!served.status.success());
    let stderr = String::from_utf8_lossy(&served.stderr);
    assert!(stderr.contains("isn't HTTPS"), "{stderr}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_database_holds_tokens_only_as_hashes_and_no_session_expiry() {
    let outbox = Outbox::default();
    let site = site("erin@example.com", Box::new(outbox.clone())).await;
    let mut key = passkey();
    let passkey_cookie = passkey_sign_up(&site, &outbox, &mut key, "erin@example.com").await;
    let link_cookie = {
        site.form(
            "/account/email-link",
            &[("email", "erin@example.com")],
            None,
        )
        .await;
        site.follow(&link_in(&outbox.take().pop().unwrap()))
            .await
            .session_cookie()
            .unwrap()
    };
    // An unused link stays in the database until it expires.
    site.form(
        "/account/email-link",
        &[("email", "erin@example.com")],
        None,
    )
    .await;
    let pending_link = link_in(&outbox.take().pop().unwrap());
    // A device session, as ticket 47's device grant will create one.
    let device_token: [u8; 32] = sync_protocol::crypto::random_array().unwrap();
    let account = site.store.account_id("erin@example.com").unwrap();
    site.store
        .create_session(
            device_token,
            sync_store::Session::new(account, sync_protocol::types::Id([7; 16])),
        )
        .await
        .unwrap();

    let tokens: Vec<Vec<u8>> = [&passkey_cookie, &link_cookie]
        .iter()
        .map(|c| c.trim_start_matches("noura_session=").to_owned())
        .chain([pending_link
            .split("token=")
            .nth(1)
            .expect("a token")
            .to_owned()])
        .map(|text| {
            sync_protocol::types::from_base64url::<32>(&text)
                .expect("a 32-byte token")
                .to_vec()
        })
        .chain([device_token.to_vec()])
        .collect();
    let values = every_value(&site.database()).await;
    for token in &tokens {
        let text = sync_protocol::types::to_base64url(token);
        let hash = sync_protocol::crypto::hash(token);
        for (table, column, value) in &values {
            assert!(
                !contains(value, token) && !contains(value, text.as_bytes()),
                "{table}.{column} holds a token"
            );
        }
        assert!(
            values
                .iter()
                .any(|(_, column, value)| column == "token_hash" && value == &hash.0),
            "the token's hash is stored"
        );
    }
    for table in ["sessions", "web_sessions"] {
        let columns = columns(&site.database(), table).await;
        assert!(
            !columns
                .iter()
                .any(|c| c.contains("expir") || c.contains("time") || c.contains("created")),
            "{table} has {columns:?}"
        );
    }

    // Signing out of the pages ends the web session.
    let out = site
        .form("/account/sign-out", &[], Some(&link_cookie))
        .await;
    assert_eq!(out.status, StatusCode::SEE_OTHER);
    assert_eq!(out.headers["location"], "/account/sign-in");
    let page = site.get("/account", Some(&link_cookie)).await;
    assert_eq!(page.status, StatusCode::SEE_OTHER);
    account_page(&site, &passkey_cookie).await;
    assert_eq!(count(&site.database(), "web_sessions").await, 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_account_id_derives_from_its_email_under_the_server_secret() {
    let outbox = Outbox::default();
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    let first = site_in(
        dir,
        data.clone(),
        "frank@example.com",
        Box::new(outbox.clone()),
    )
    .await;
    first
        .form(
            "/account/email-link",
            &[("email", "Frank@Example.com")],
            None,
        )
        .await;
    first.follow(&link_in(&outbox.take().pop().unwrap())).await;
    let secret: [u8; 32] = std::fs::read(data.join(sync_server::data::SECRET_FILE))
        .unwrap()
        .try_into()
        .unwrap();
    let expected = sync_protocol::ids::account_id(
        &sync_protocol::types::SecretKey(secret),
        "frank@example.com",
    );
    let stored = every_value(&first.database())
        .await
        .into_iter()
        .find(|(table, column, _)| table == "account_emails" && column == "account")
        .map(|(_, _, value)| value)
        .expect("the account row");
    assert_eq!(stored, expected.0);

    // After a restore that lost the account, signing up again with that
    // email under the same secret gets the same ID.
    let restored_dir = tempfile::tempdir().unwrap();
    let restored_data = restored_dir.path().join("data");
    std::fs::create_dir_all(&restored_data).unwrap();
    std::fs::write(restored_data.join(sync_server::data::SECRET_FILE), secret).unwrap();
    let second = site_in(
        restored_dir,
        restored_data,
        "frank@example.com",
        Box::new(outbox.clone()),
    )
    .await;
    second
        .form(
            "/account/email-link",
            &[("email", "frank@EXAMPLE.com")],
            None,
        )
        .await;
    second.follow(&link_in(&outbox.take().pop().unwrap())).await;
    let again = every_value(&second.database())
        .await
        .into_iter()
        .find(|(table, column, _)| table == "account_emails" && column == "account")
        .map(|(_, _, value)| value)
        .unwrap();
    assert_eq!(again, expected.0);
    // The secret lives outside the database: no row holds it.
    for (table, column, value) in every_value(&second.database()).await {
        assert!(
            !contains(&value, &secret),
            "{table}.{column} holds the secret"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_send_leaves_no_usable_link() {
    let site = site("gina@example.com", Box::new(Broken)).await;
    let reply = site
        .form(
            "/account/email-link",
            &[("email", "gina@example.com")],
            None,
        )
        .await;
    assert_eq!(reply.status, StatusCode::OK);
    assert_eq!(count(&site.database(), "magic_links").await, 0);

    let mut key = passkey();
    let started = site
        .json(
            "/account/passkeys/sign-up/start",
            json!({ "email": "gina@example.com" }),
            None,
        )
        .await
        .json();
    let credential = create(&mut key, &site.origin, &started["options"]).await;
    let finished = site
        .json(
            "/account/passkeys/sign-up/finish",
            json!({ "ceremony": started["ceremony"], "credential": credential }),
            None,
        )
        .await;
    assert_eq!(finished.status, StatusCode::OK);
    assert_eq!(count(&site.database(), "pending_passkeys").await, 0);
}

/// A fake SMTP server that takes messages over plain SMTP and keeps their
/// data.
async fn fake_smtp() -> (u16, Arc<Mutex<Vec<String>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let received = Arc::new(Mutex::new(Vec::new()));
    let kept = received.clone();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                return;
            };
            let kept = kept.clone();
            tokio::spawn(async move {
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                write.write_all(b"220 fake ESMTP\r\n").await.unwrap();
                let mut data: Option<String> = None;
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Some(body) = data.as_mut() {
                        if line == "." {
                            kept.lock().unwrap().push(std::mem::take(body));
                            data = None;
                            write.write_all(b"250 queued\r\n").await.unwrap();
                        } else {
                            body.push_str(&line);
                            body.push('\n');
                        }
                        continue;
                    }
                    let verb = line
                        .split_whitespace()
                        .next()
                        .unwrap_or("")
                        .to_ascii_uppercase();
                    let answer: &[u8] = match verb.as_str() {
                        "EHLO" | "HELO" => b"250 fake\r\n",
                        "DATA" => {
                            data = Some(String::new());
                            b"354 go ahead\r\n"
                        }
                        "QUIT" => {
                            write.write_all(b"221 bye\r\n").await.unwrap();
                            return;
                        }
                        _ => b"250 ok\r\n",
                    };
                    write.write_all(answer).await.unwrap();
                }
            });
        }
    });
    (port, received)
}

/// A fake Resend endpoint that keeps each request's JSON and answers 200,
/// or 500 when `fail` is set.
async fn fake_resend(fail: bool) -> (String, Arc<Mutex<Vec<(String, Value)>>>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/emails", listener.local_addr().unwrap());
    let received = Arc::new(Mutex::new(Vec::new()));
    let kept = received.clone();
    let app = axum::Router::new().route(
        "/emails",
        axum::routing::post(move |headers: axum::http::HeaderMap, body: String| {
            let kept = kept.clone();
            async move {
                let auth = headers["authorization"].to_str().unwrap().to_owned();
                kept.lock()
                    .unwrap()
                    .push((auth, serde_json::from_str(&body).unwrap()));
                if fail {
                    (axum::http::StatusCode::INTERNAL_SERVER_ERROR, "down")
                } else {
                    (axum::http::StatusCode::OK, "{\"id\":\"1\"}")
                }
            }
        }),
    );
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (url, received)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn configuration_picks_smtp_or_resend_and_a_failed_send_leaves_no_link() {
    // SMTP through lettre.
    let (port, received) = fake_smtp().await;
    let smtp = Smtp::from_url(&format!("smtp://127.0.0.1:{port}")).unwrap();
    let site = site("hana@example.com", Box::new(smtp)).await;
    site.form(
        "/account/email-link",
        &[("email", "hana@example.com")],
        None,
    )
    .await;
    let message = received
        .lock()
        .unwrap()
        .pop()
        .expect("SMTP got the message");
    assert!(message.contains("Subject: Sign in to Noura"), "{message}");
    // lettre sends the text as quoted-printable, which mail apps decode.
    assert!(message.contains("Content-Transfer-Encoding: quoted-printable"));
    let decoded = message.replace("=\n", "").replace("=3D", "=");
    let link = decoded
        .split_whitespace()
        .find(|w| w.starts_with(&format!("{}/account/email-link?token=", site.base)))
        .expect("the link")
        .to_owned();
    assert_eq!(
        site.follow(&link).await.status,
        StatusCode::SEE_OTHER,
        "{message}"
    );

    // Resend's HTTPS API.
    let (endpoint, received) = fake_resend(false).await;
    let resend = Resend::new("re_test_key".into(), Some(endpoint));
    let site = self::site("hana@example.com", Box::new(resend)).await;
    site.form(
        "/account/email-link",
        &[("email", "hana@example.com")],
        None,
    )
    .await;
    let (auth, body) = received
        .lock()
        .unwrap()
        .pop()
        .expect("Resend got the message");
    assert_eq!(auth, "Bearer re_test_key");
    assert_eq!(body["to"], json!(["hana@example.com"]));
    assert_eq!(body["subject"], "Sign in to Noura");
    let text = body["text"].as_str().unwrap();
    let link = text
        .split_whitespace()
        .find(|w| w.starts_with("http"))
        .unwrap();
    assert_eq!(site.follow(link).await.status, StatusCode::SEE_OTHER);

    // A send that fails leaves no link behind, with either transport.
    let (endpoint, _) = fake_resend(true).await;
    let site = self::site(
        "hana@example.com",
        Box::new(Resend::new("re_test_key".into(), Some(endpoint))),
    )
    .await;
    site.form(
        "/account/email-link",
        &[("email", "hana@example.com")],
        None,
    )
    .await;
    assert_eq!(count(&site.database(), "magic_links").await, 0);
    let closed = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = closed.local_addr().unwrap().port();
    drop(closed);
    let site = self::site(
        "hana@example.com",
        Box::new(Smtp::from_url(&format!("smtp://127.0.0.1:{port}")).unwrap()),
    )
    .await;
    site.form(
        "/account/email-link",
        &[("email", "hana@example.com")],
        None,
    )
    .await;
    assert_eq!(count(&site.database(), "magic_links").await, 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_pages_are_plain_forms_under_a_policy_that_blocks_inline_scripts() {
    let outbox = Outbox::default();
    let site = site("ivy@example.com", Box::new(outbox.clone())).await;
    site.form("/account/email-link", &[("email", "ivy@example.com")], None)
        .await;
    let link = link_in(&outbox.take().pop().unwrap());
    let cookie = site.follow(&link).await.session_cookie().unwrap();
    let path = link.strip_prefix(&site.base).unwrap().to_owned();
    for (path, cookie) in [
        ("/account/sign-in", None),
        ("/account/sign-up", None),
        ("/account", Some(cookie.as_str())),
        (path.as_str(), None),
        ("/account/confirm?token=x", None),
    ] {
        let page = site.get(path, cookie).await;
        assert_eq!(page.status, StatusCode::OK, "{path}");
        let policy = page.headers["content-security-policy"].to_str().unwrap();
        assert!(
            policy.contains("script-src 'self' 'wasm-unsafe-eval'"),
            "{policy}"
        );
        assert!(!policy.contains("unsafe-inline"), "{policy}");
        assert!(policy.contains("default-src 'none'"));
        assert_eq!(page.headers["x-content-type-options"], "nosniff");
        // Every script loads from a file; no element carries an inline
        // handler.
        let mut rest = page.body.as_str();
        while let Some(at) = rest.find("<script") {
            let tag_end = rest[at..].find('>').unwrap() + at;
            assert!(
                rest[at..tag_end].contains(" src=\"/account/assets/"),
                "{path}: inline script"
            );
            assert!(
                rest[tag_end + 1..].starts_with("</script>"),
                "{path}: script body"
            );
            rest = &rest[tag_end..];
        }
        assert!(
            !has_inline_handler(&page.body),
            "{path} holds an inline handler"
        );
        // The email forms post without script.
        assert!(page.body.contains("<form") || path.starts_with("/account/confirm"));
    }
    let sign_in = site.get("/account/sign-in", None).await.body;
    assert!(sign_in.contains("<form method=\"post\" action=\"/account/email-link\">"));
    for asset in ["account.css", "account.js"] {
        let reply = site.get(&format!("/account/assets/{asset}"), None).await;
        assert_eq!(reply.status, StatusCode::OK, "{asset}");
        assert!(reply.headers.contains_key("content-security-policy"));
    }
    assert_eq!(
        site.get("/account/assets/..%2Fsecret", None).await.status,
        StatusCode::NOT_FOUND
    );
}
