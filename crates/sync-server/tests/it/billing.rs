//! Billing through Polar (§20, §26 Billing) against a fake Polar on a
//! loopback port: the upgrade route signs the visitor in, then opens a
//! checkout for the plan's products under the account's ID; the portal
//! route opens a customer session; and webhook deliveries, signed here
//! under the Standard Webhooks scheme, set the account's plan once and
//! refuse forgeries. Nothing reaches Polar's real API.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use serde_json::{Value, json};
use sync_protocol::types::{Id, to_base64url};
use sync_server::Accounts;
use sync_server::billing::polar::{Polar, PolarConfig};
use sync_server::billing::webhooks::Secret;
use sync_store::Store;
use sync_store_sqlite::SqliteBackend;

const TOKEN: &str = "polar_oat_test_token";
const SECRET: &str = "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw";

/// One request the fake Polar received.
#[derive(Clone, Debug)]
struct Received {
    path: &'static str,
    authorization: String,
    version: String,
    body: Value,
}

#[derive(Clone, Default)]
struct FakePolar {
    received: Arc<Mutex<Vec<Received>>>,
}

impl FakePolar {
    fn record(&self, path: &'static str, headers: &HeaderMap, body: Value) {
        let header = |name: &str| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .unwrap_or_default()
                .to_owned()
        };
        self.received.lock().unwrap().push(Received {
            path,
            authorization: header("authorization"),
            version: header("polar-version"),
            body,
        });
    }

    fn last(&self, path: &str) -> Received {
        self.received
            .lock()
            .unwrap()
            .iter()
            .rev()
            .find(|r| r.path == path)
            .cloned()
            .expect("a request")
    }
}

/// A JSON answer.
type Answer = (StatusCode, [(&'static str, &'static str); 1], String);

fn answer(status: StatusCode, value: Value) -> Answer {
    (
        status,
        [("content-type", "application/json")],
        value.to_string(),
    )
}

async fn checkout(
    State(fake): State<FakePolar>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Answer {
    let body: Value = serde_json::from_slice(&body).expect("JSON");
    fake.record("/v1/checkouts/", &headers, body);
    answer(
        StatusCode::CREATED,
        json!({ "id": "chk-1", "url": "https://polar.test/checkout/chk-1" }),
    )
}

async fn customer_session(
    State(fake): State<FakePolar>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Answer {
    let body: Value = serde_json::from_slice(&body).expect("JSON");
    let known = body["customer_id"] == "cus-1";
    fake.record("/v1/customer-sessions/", &headers, body);
    if !known {
        return answer(StatusCode::UNPROCESSABLE_ENTITY, json!({}));
    }
    answer(
        StatusCode::CREATED,
        json!({ "customer_portal_url": "https://polar.test/portal/session-1" }),
    )
}

/// A fake Polar API on a loopback port, and its address.
async fn fake_polar() -> (FakePolar, String) {
    let fake = FakePolar::default();
    let app = axum::Router::new()
        .route("/v1/checkouts/", post(checkout))
        .route("/v1/customer-sessions/", post(customer_session))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bound");
    let address = listener.local_addr().expect("an address");
    tokio::spawn(async move { axum::serve(listener, app).await });
    (fake, format!("http://{address}"))
}

struct Site {
    base: String,
    store: Arc<Store<SqliteBackend>>,
    http: reqwest::Client,
    fake: Option<FakePolar>,
    _dir: tempfile::TempDir,
}

async fn site(billing: bool) -> Site {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    sync_server::migrate(&data).await.expect("migrated");
    let store = Arc::new(
        sync_server::open_store(&sync_server::Config::new(
            data,
            "127.0.0.1:0".parse().expect("an address"),
        ))
        .await
        .expect("the store opens")
        .with_billing(billing),
    );
    let (fake, api) = fake_polar().await;
    let products: BTreeMap<String, String> = PolarConfig::parse_products(
        "plus_year=prod-plus-year,plus_month=prod-plus-month,standard_year=prod-std-year",
    )
    .expect("products");
    let config = PolarConfig {
        access_token: TOKEN.into(),
        webhook_secret: SECRET.into(),
        api,
        products,
    };
    let address = sync_server::spawn_with(store.clone(), "127.0.0.1:0", move |origin| {
        let accounts = Accounts::local(origin);
        if billing {
            accounts.with_billing(Arc::new(Polar::new(config)))
        } else {
            accounts
        }
    })
    .await
    .expect("serving");
    Site {
        base: format!("http://localhost:{}", address.port()),
        store,
        http: reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("a client"),
        fake: billing.then_some(fake),
        _dir: dir,
    }
}

impl Site {
    /// A web session of a new account, as its cookie.
    async fn signed_in(&self, email: &str) -> (Id, String) {
        let link = self.store.issue_magic_link(email).await.expect("a link");
        let (account, token) = self
            .store
            .sign_in_with_magic_link(&link)
            .await
            .expect("used")
            .expect("a current link");
        (account, format!("noura_session={}", to_base64url(&token)))
    }

    async fn get(&self, path: &str, cookie: Option<&str>) -> reqwest::Response {
        let mut request = self.http.get(format!("{}{path}", self.base));
        if let Some(cookie) = cookie {
            request = request.header("cookie", cookie);
        }
        request.send().await.expect("an answer")
    }

    /// Delivers `event` as Polar would, signed with `secret` at `timestamp`.
    async fn deliver(&self, id: &str, timestamp: u64, event: &Value, secret: &str) -> StatusCode {
        let body = event.to_string();
        let signature = Secret::new(secret).signature(id, timestamp, body.as_bytes());
        self.http
            .post(format!("{}/billing/polar/webhook", self.base))
            .header("webhook-id", id)
            .header("webhook-timestamp", timestamp.to_string())
            .header("webhook-signature", signature)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await
            .expect("an answer")
            .status()
    }

    fn now(&self) -> u64 {
        self.store.clock().now()
    }
}

fn location(response: &reqwest::Response) -> String {
    response.headers()["location"]
        .to_str()
        .expect("text")
        .to_owned()
}

/// A `subscription.*` event for `account`, as Polar's 2026-10 payloads
/// carry it.
fn event(kind: &str, status: &str, account: Id, modified_at: &str) -> Value {
    json!({
        "type": kind,
        "timestamp": modified_at,
        "api_version": "2026-10",
        "data": {
            "id": "sub-1",
            "status": status,
            "customer_id": "cus-1",
            "product_id": "prod-plus-year",
            "created_at": "2026-11-01T00:00:00Z",
            "modified_at": modified_at,
            "current_period_end": "2027-11-01T00:00:00Z",
            "cancel_at_period_end": false,
            "metadata": { "noura_account": account.to_base64url() },
            "customer": { "id": "cus-1", "external_id": account.to_base64url() },
            "product": { "id": "prod-plus-year", "metadata": { "noura_plan": "plus_year" } },
        },
    })
}

#[tokio::test]
async fn an_upgrade_signs_in_then_opens_a_polar_checkout() {
    let site = site(true).await;
    let fake = site.fake.clone().expect("billing");

    // A link from elsewhere carries no session cookie: the page asks again
    // from the server's own origin.
    let page = site.get("/account/upgrade?plan=plus_year", None).await;
    assert_eq!(page.status(), StatusCode::OK);
    let text = page.text().await.expect("a body");
    assert!(text.contains("http-equiv=\"refresh\""));
    assert!(text.contains("/account/upgrade?plan=plus_year&amp;continue=1"));

    // Signed out for real: off to sign in, with the plan remembered.
    let again = site
        .get("/account/upgrade?plan=plus_year&continue=1", None)
        .await;
    assert_eq!(again.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&again), "/account/sign-in");
    let remembered = again.headers()["set-cookie"].to_str().expect("text");
    assert!(remembered.starts_with("noura_upgrade=plus_year;"));
    assert!(remembered.contains("HttpOnly"));

    // Once signed in, the account page sends the visitor on.
    let (account, session) = site.signed_in("buyer@example.com").await;
    let both = format!("{session}; noura_upgrade=plus_year");
    let resumed = site.get("/account", Some(&both)).await;
    assert_eq!(resumed.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&resumed), "/account/upgrade?plan=plus_year");
    assert!(
        resumed.headers()["set-cookie"]
            .to_str()
            .expect("text")
            .contains("Max-Age=0")
    );

    // The checkout offers both intervals of Plus, for the account's ID.
    let checkout = site
        .http
        .get(format!("{}/account/upgrade?plan=plus_year", site.base))
        .header("cookie", &session)
        .header("accept-language", "fr-CA,fr;q=0.9,en;q=0.5")
        .send()
        .await
        .expect("an answer");
    assert_eq!(checkout.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&checkout), "https://polar.test/checkout/chk-1");
    let sent = fake.last("/v1/checkouts/");
    assert_eq!(sent.authorization, format!("Bearer {TOKEN}"));
    assert_eq!(sent.version, "2026-10");
    assert_eq!(
        sent.body["products"],
        json!(["prod-plus-year", "prod-plus-month"])
    );
    assert_eq!(sent.body["external_customer_id"], account.to_base64url());
    assert_eq!(sent.body["customer_email"], "buyer@example.com");
    assert_eq!(sent.body["success_url"], format!("{}/account", site.base));
    assert_eq!(sent.body["locale"], "fr-CA");
    assert_eq!(sent.body["metadata"]["noura_plan"], "plus_year");

    // An English visitor gets Polar's default language, and a plan the
    // server doesn't sell gets a not-found page.
    site.get("/account/upgrade?plan=standard_year", Some(&session))
        .await;
    let sent = fake.last("/v1/checkouts/");
    assert_eq!(sent.body["products"], json!(["prod-std-year"]));
    assert!(sent.body.get("locale").is_none());
    for plan in ["plus_100gb_year", "gold", "free"] {
        let refused = site
            .get(&format!("/account/upgrade?plan={plan}"), Some(&session))
            .await;
        assert_eq!(refused.status(), StatusCode::NOT_FOUND, "{plan}");
    }

    // The account page shows the free plan and what it can become.
    let page = site.get("/account", Some(&session)).await;
    let text = page.text().await.expect("a body");
    assert!(text.contains("data-plan=\"free\""));
    assert!(text.contains("of 100 MB used"));
    assert!(text.contains("/account/upgrade?plan=standard_year"));
    assert!(text.contains("/account/upgrade?plan=plus_year"));
    assert!(!text.contains("plus_100gb"));
    assert!(!text.contains("Manage billing"));
}

#[tokio::test]
async fn webhooks_set_the_plan_once_and_refuse_forgeries() {
    let site = site(true).await;
    let fake = site.fake.clone().expect("billing");
    let (account, session) = site.signed_in("payer@example.com").await;
    let now = site.now();

    let active = event(
        "subscription.active",
        "active",
        account,
        "2026-11-20T10:00:00Z",
    );
    assert_eq!(
        site.deliver("evt-1", now, &active, SECRET).await,
        StatusCode::ACCEPTED
    );
    let report = site.store.account_report(account).await.expect("report");
    assert_eq!(report.plan.as_str(), "plus_year");
    assert_eq!(report.owned_workspaces, 10);
    assert_eq!(report.period_end, Some(1_825_027_200));

    // A retried delivery under the same ID changes nothing, whatever it
    // carries.
    let revoked = event(
        "subscription.revoked",
        "canceled",
        account,
        "2026-11-21T10:00:00Z",
    );
    assert_eq!(
        site.deliver("evt-1", now, &revoked, SECRET).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        site.store
            .account_report(account)
            .await
            .expect("report")
            .plan
            .as_str(),
        "plus_year"
    );

    // Forgeries and stale deliveries change nothing.
    assert_eq!(
        site.deliver("evt-2", now, &revoked, "whsec_c29tZW9uZSBlbHNl")
            .await,
        StatusCode::FORBIDDEN
    );
    assert_eq!(
        site.deliver("evt-2", now - 600, &revoked, SECRET).await,
        StatusCode::FORBIDDEN
    );
    let unsigned = site
        .http
        .post(format!("{}/billing/polar/webhook", site.base))
        .body(revoked.to_string())
        .send()
        .await
        .expect("an answer");
    assert_eq!(unsigned.status(), StatusCode::FORBIDDEN);
    let other = site
        .http
        .post(format!("{}/billing/stripe/webhook", site.base))
        .send()
        .await
        .expect("an answer");
    assert_eq!(other.status(), StatusCode::NOT_FOUND);
    assert_eq!(
        site.store
            .account_report(account)
            .await
            .expect("report")
            .plan
            .as_str(),
        "plus_year"
    );

    // A paying account goes to the portal, from the page and from upgrade.
    let page = site.get("/account", Some(&session)).await;
    let text = page.text().await.expect("a body");
    assert!(text.contains("Plus, yearly"));
    assert!(text.contains("Manage billing"));
    assert!(!text.contains("Upgrade to"));
    let upgrade = site
        .get("/account/upgrade?plan=plus_month", Some(&session))
        .await;
    assert_eq!(location(&upgrade), "/account/billing");
    let portal = site.get("/account/billing", Some(&session)).await;
    assert_eq!(portal.status(), StatusCode::SEE_OTHER);
    assert_eq!(location(&portal), "https://polar.test/portal/session-1");
    let sent = fake.last("/v1/customer-sessions/");
    assert_eq!(sent.body["customer_id"], "cus-1");
    assert_eq!(sent.body["return_url"], format!("{}/account", site.base));

    // A failed renewal keeps the plan; the end of the retries takes it.
    let due = event(
        "subscription.past_due",
        "past_due",
        account,
        "2026-11-22T10:00:00Z",
    );
    assert_eq!(
        site.deliver("evt-3", now, &due, SECRET).await,
        StatusCode::ACCEPTED
    );
    let page = site.get("/account", Some(&session)).await;
    assert!(
        page.text()
            .await
            .expect("a body")
            .contains("Your last payment failed")
    );
    let ended = event(
        "subscription.revoked",
        "canceled",
        account,
        "2026-12-13T10:00:00Z",
    );
    assert_eq!(
        site.deliver("evt-4", now, &ended, SECRET).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        site.store
            .account_report(account)
            .await
            .expect("report")
            .plan
            .as_str(),
        "free"
    );
}

#[tokio::test]
async fn without_billing_no_plan_route_answers() {
    let site = site(false).await;
    let (_, session) = site.signed_in("someone@example.com").await;
    let upgrade = site
        .get("/account/upgrade?plan=plus_year", Some(&session))
        .await;
    assert_eq!(upgrade.status(), StatusCode::NOT_FOUND);
    let portal = site.get("/account/billing", Some(&session)).await;
    assert_eq!(location(&portal), "/account");
    let page = site.get("/account", Some(&session)).await;
    assert!(!page.text().await.expect("a body").contains("Plan"));
    let webhook = site
        .http
        .post(format!("{}/billing/polar/webhook", site.base))
        .send()
        .await
        .expect("an answer");
    assert_eq!(webhook.status(), StatusCode::NOT_FOUND);
}

/// The binary takes Polar's settings all together or not at all, maps only
/// paid plan IDs, talks to Polar only over HTTPS, and never prints a secret.
#[test]
fn the_binary_refuses_a_partial_or_unknown_polar_setting() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    assert!(crate::support::run(&["migrate"], &data).status.success());
    let full = [
        ("NOURA_POLAR_ACCESS_TOKEN", "polar_oat_secret_value"),
        ("NOURA_POLAR_WEBHOOK_SECRET", "whsec_secret_value"),
        ("NOURA_POLAR_PRODUCTS", "plus_year=prod-1"),
    ];
    for (settings, says) in [
        (&full[..1], "together"),
        (&full[1..], "together"),
        (
            &[
                full[0],
                full[1],
                ("NOURA_POLAR_PRODUCTS", "enterprise=prod-1"),
            ][..],
            "isn't a plan ID",
        ),
        (
            &[
                full[0],
                full[1],
                full[2],
                ("NOURA_POLAR_API", "http://api.polar.sh"),
            ][..],
            "HTTPS",
        ),
    ] {
        let output = std::process::Command::new(crate::support::BIN)
            .arg("serve")
            .arg("--data")
            .arg(&data)
            .arg("--listen")
            .arg("127.0.0.1:0")
            .envs(settings.iter().copied())
            .output()
            .expect("the binary runs");
        assert!(!output.status.success());
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(stderr.contains(says), "{stderr}");
        assert!(!stderr.contains("secret_value"), "{stderr}");
    }
}

/// Keeps every message it sends.
#[derive(Clone, Default)]
struct Outbox(Arc<Mutex<Vec<sync_server::accounts::mail::Mail>>>);

impl sync_server::accounts::mail::MailTransport for Outbox {
    fn send<'a>(
        &'a self,
        mail: &'a sync_server::accounts::mail::Mail,
    ) -> sync_server::accounts::mail::SendFuture<'a> {
        self.0.lock().unwrap().push(mail.clone());
        Box::pin(async { Ok(()) })
    }
}

/// While the server sells plans, a disposable address gets a page that
/// says so, and each client IP starts only its day's new accounts; the
/// answer is the same either way, and existing accounts still sign in.
#[tokio::test]
async fn sign_up_refuses_disposable_addresses_and_counts_per_client() {
    use sync_server::accounts::Deliveries;
    use sync_server::accounts::signups::SignUpGuard;

    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    sync_server::migrate(&data).await.expect("migrated");
    let store = Arc::new(
        sync_server::open_store(&sync_server::Config::new(
            data,
            "127.0.0.1:0".parse().expect("an address"),
        ))
        .await
        .expect("the store opens")
        .with_billing(true),
    );
    let outbox = Outbox::default();
    let deliveries = Deliveries::default();
    let (mailer, counted) = (outbox.clone(), deliveries.clone());
    let address = sync_server::spawn_limited(
        store.clone(),
        "127.0.0.1:0",
        move |origin| {
            sync_server::Accounts::new(
                origin,
                sync_server::Allowlist::parse("*"),
                "Noura <noura@example.com>".into(),
                Box::new(mailer),
                None,
            )
            .with_deliveries(counted)
            .with_sign_up_guard(SignUpGuard::new(Some(2), "blocked.test\n"))
        },
        Arc::new(sync_server::blobs::Objects::memory()),
        sync_server::RateGate::new(sync_server::RateLimits::default(), None),
    )
    .await
    .expect("serving");
    let base = format!("http://localhost:{}", address.port());
    let http = reqwest::Client::new();
    let ask = async |email: &str| {
        let response = http
            .post(format!("{base}/account/email-link"))
            .header("origin", &base)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(format!("email={}", email.replace('@', "%40")))
            .send()
            .await
            .expect("an answer");
        let status = response.status();
        let text = response.text().await.expect("a body");
        deliveries.idle().await;
        (status, text)
    };

    for email in ["someone@mailinator.com", "someone@mail.blocked.test"] {
        let (status, text) = ask(email).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{email}");
        assert!(text.contains("disposable email"), "{text}");
    }

    // An account that exists signs in whatever the day's count.
    let link = store
        .issue_magic_link("member@example.com")
        .await
        .expect("a link");
    store
        .sign_in_with_magic_link(&link)
        .await
        .expect("used")
        .expect("signed up");
    for email in ["one@example.com", "two@example.com", "three@example.com"] {
        let (status, _) = ask(email).await;
        assert_eq!(status, StatusCode::OK);
    }
    let (status, _) = ask("member@example.com").await;
    assert_eq!(status, StatusCode::OK);
    let sent: Vec<String> = outbox
        .0
        .lock()
        .unwrap()
        .iter()
        .map(|m| m.to.clone())
        .collect();
    assert_eq!(
        sent,
        vec!["one@example.com", "two@example.com", "member@example.com"],
        "the third new account waits for tomorrow"
    );
}
