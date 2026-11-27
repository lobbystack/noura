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
    /// Whether refunds fail with 500, as an outage would.
    refunds_fail: Arc<std::sync::atomic::AtomicBool>,
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

    fn count(&self, path: &str) -> usize {
        self.received
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.path == path)
            .count()
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

async fn cancel_subscription(
    State(fake): State<FakePolar>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Answer {
    let mut body: Value = serde_json::from_slice(&body).expect("JSON");
    body["id"] = json!(id);
    fake.record("PATCH /v1/subscriptions", &headers, body);
    answer(
        StatusCode::OK,
        json!({ "id": id, "cancel_at_period_end": true }),
    )
}

async fn revoke_subscription(
    State(fake): State<FakePolar>,
    axum::extract::Path(id): axum::extract::Path<String>,
    headers: HeaderMap,
) -> Answer {
    let first = fake.count("DELETE /v1/subscriptions") == 0;
    fake.record("DELETE /v1/subscriptions", &headers, json!({ "id": id }));
    if first {
        answer(StatusCode::OK, json!({ "id": id, "status": "canceled" }))
    } else {
        // Polar answers 403 for a subscription it already revoked.
        answer(
            StatusCode::FORBIDDEN,
            json!({ "error": "AlreadyCanceledSubscription" }),
        )
    }
}

async fn create_refund(
    State(fake): State<FakePolar>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Answer {
    let body: Value = serde_json::from_slice(&body).expect("JSON");
    if fake.refunds_fail.load(std::sync::atomic::Ordering::SeqCst) {
        fake.record("failed refund", &headers, body);
        return answer(StatusCode::INTERNAL_SERVER_ERROR, json!({}));
    }
    fake.record("/v1/refunds/", &headers, body);
    answer(
        StatusCode::CREATED,
        json!({ "id": "ref-1", "status": "pending" }),
    )
}

async fn product(axum::extract::Path(id): axum::extract::Path<String>) -> Answer {
    let (amount, interval) = match id.as_str() {
        "prod-plus-year" => (9_600, "year"),
        "prod-plus-month" => (1_000, "month"),
        "prod-std-year" => (4_800, "year"),
        _ => return answer(StatusCode::NOT_FOUND, json!({})),
    };
    answer(
        StatusCode::OK,
        json!({
            "id": id,
            "recurring_interval": interval,
            "prices": [
                { "amount_type": "fixed", "is_archived": true, "price_amount": 1, "price_currency": "usd" },
                { "amount_type": "fixed", "is_archived": false, "price_amount": amount, "price_currency": "usd" },
            ],
        }),
    )
}

/// A fake Polar API on a loopback port, and its address.
async fn fake_polar() -> (FakePolar, String) {
    let fake = FakePolar::default();
    let app = axum::Router::new()
        .route("/v1/checkouts/", post(checkout))
        .route("/v1/customer-sessions/", post(customer_session))
        .route("/v1/products/{id}", axum::routing::get(product))
        .route(
            "/v1/subscriptions/{id}",
            axum::routing::patch(cancel_subscription).delete(revoke_subscription),
        )
        .route("/v1/refunds/", post(create_refund))
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
    config: PolarConfig,
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
    let kept = config.clone();
    let address = sync_server::spawn_with(store.clone(), "127.0.0.1:0", move |origin| {
        let accounts = Accounts::local(origin);
        if billing {
            accounts.with_billing(
                Arc::new(Polar::new(config)),
                sync_server::billing::Merchant::example(),
            )
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
        config: kept,
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

    /// Posts a form from the server's own pages.
    async fn post_form(
        &self,
        path: &str,
        cookie: &str,
        form: &str,
        accept_language: Option<&str>,
    ) -> reqwest::Response {
        let mut request = self
            .http
            .post(format!("{}{path}", self.base))
            .header("cookie", cookie)
            .header("origin", &self.base)
            .header("content-type", "application/x-www-form-urlencoded")
            .body(form.to_owned());
        if let Some(lang) = accept_language {
            request = request.header("accept-language", lang);
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

    /// One pass of the plan jobs, with this site's Polar.
    async fn plan_jobs(&self) -> sync_server::maintenance::PlanJobs {
        let accounts =
            Accounts::local(sync_server::PublicOrigin::parse(&self.base).expect("an origin"))
                .with_billing(
                    Arc::new(Polar::new(self.config.clone())),
                    sync_server::billing::Merchant::example(),
                );
        sync_server::maintenance::plan_jobs(&self.store, &accounts)
            .await
            .expect("the jobs run")
    }

    /// A paying account on `plus_year` billed to `country` and `state`,
    /// whose subscription and first order started `days_ago` days ago, as
    /// Polar's events report them.
    async fn paying(&self, email: &str, country: &str, state: &str, days_ago: u64) -> (Id, String) {
        let (account, session) = self.signed_in(email).await;
        let now = self.now();
        let start = now - days_ago * DAY;
        let end = start + 365 * DAY;
        let time = |t: u64| {
            jiff::Timestamp::from_second(i64::try_from(t).expect("a time"))
                .expect("a time")
                .to_string()
        };
        let subscription = json!({
            "id": "sub-1",
            "status": "active",
            "amount": 9_600,
            "currency": "usd",
            "recurring_interval": "year",
            "current_period_start": time(start),
            "current_period_end": time(end),
            "started_at": time(start),
            "cancel_at_period_end": false,
            "customer_id": "cus-1",
            "product_id": "prod-plus-year",
            "created_at": time(start),
            "modified_at": time(start),
            "metadata": {},
            "customer": {
                "id": "cus-1",
                "external_id": account.to_base64url(),
                "billing_address": { "country": country, "state": state },
            },
            "product": { "id": "prod-plus-year", "metadata": { "noura_plan": "plus_year" } },
        });
        let active = json!({
            "type": "subscription.active",
            "timestamp": time(start),
            "data": subscription,
        });
        assert_eq!(
            self.deliver(&format!("{email}-active"), now, &active, SECRET)
                .await,
            StatusCode::ACCEPTED
        );
        let order = json!({
            "type": "order.paid",
            "timestamp": time(start),
            "data": {
                "id": "ord-1",
                "created_at": time(start),
                "modified_at": time(start),
                "billing_reason": "subscription_create",
                "net_amount": 9_600,
                "tax_amount": 0,
                "total_amount": 9_600,
                "currency": "usd",
                "billing_address": { "country": country, "state": state },
                "product_id": "prod-plus-year",
                "subscription_id": "sub-1",
                "metadata": {},
                "customer": { "id": "cus-1", "external_id": account.to_base64url() },
                "subscription": subscription,
            },
        });
        assert_eq!(
            self.deliver(&format!("{email}-paid"), now, &order, SECRET)
                .await,
            StatusCode::ACCEPTED
        );
        // The contract went in the queue; these tests read what follows.
        for queued in self.store.billing_mail().await.expect("queue") {
            self.store.sent_billing_mail(queued.id).await.expect("sent");
        }
        (account, session)
    }

    async fn plan(&self, account: Id) -> String {
        self.store
            .account_report(account)
            .await
            .expect("report")
            .plan
            .as_str()
            .to_owned()
    }

    async fn page(&self, path: &str, session: &str) -> String {
        self.get(path, Some(session))
            .await
            .text()
            .await
            .expect("a body")
    }

    /// The subjects of the queued mail.
    async fn queued(&self) -> Vec<String> {
        self.store
            .billing_mail()
            .await
            .expect("queue")
            .into_iter()
            .map(|queued| queued.mail.subject)
            .collect()
    }
}

const DAY: u64 = 86_400;

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

    // Signed in, the page shows both intervals' prices, the renewal terms
    // and the two consents.
    let consent = site
        .get("/account/upgrade?plan=plus_year", Some(&session))
        .await;
    assert_eq!(consent.status(), StatusCode::OK);
    let text = consent.text().await.expect("a body");
    assert!(text.contains("Plus, yearly: USD 96.00 a year, plus tax"));
    assert!(text.contains("Plus, monthly: USD 10.00 a month, plus tax"));
    assert!(text.contains("renews automatically at the price in effect until you cancel"));
    assert!(text.contains("billing@example.com"));
    assert!(text.contains("name=\"renewal\" value=\"yes\" required"));
    assert!(text.contains("during the 14-day withdrawal period"));
    assert!(
        fake.received
            .lock()
            .unwrap()
            .iter()
            .all(|r| r.path != "/v1/checkouts/")
    );

    // Without both boxes, the page asks again and records nothing.
    let missing = site
        .post_form(
            "/account/upgrade",
            &session,
            "plan=plus_year&renewal=yes",
            None,
        )
        .await;
    assert_eq!(missing.status(), StatusCode::BAD_REQUEST);
    assert!(
        missing
            .text()
            .await
            .expect("a body")
            .contains("Check both boxes")
    );
    assert_eq!(
        site.store.latest_consent(account).await.expect("read"),
        None
    );
    let foreign = site
        .http
        .post(format!("{}/account/upgrade", site.base))
        .header("cookie", &session)
        .header("origin", "https://evil.example")
        .body("plan=plus_year&renewal=yes&immediate=yes")
        .send()
        .await
        .expect("an answer");
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);

    // With both, the consent is recorded and the checkout offers both
    // intervals of Plus, for the account's ID.
    let checkout = site
        .post_form(
            "/account/upgrade",
            &session,
            "plan=plus_year&renewal=yes&immediate=yes",
            Some("fr-CA,fr;q=0.9,en;q=0.5"),
        )
        .await;
    assert_eq!(checkout.status(), StatusCode::SEE_OTHER);
    let consent = site
        .store
        .latest_consent(account)
        .await
        .expect("read")
        .expect("recorded");
    assert_eq!(
        (
            consent.plan.as_str(),
            consent.version.as_str(),
            consent.immediate,
            consent.renewal
        ),
        ("plus_year", "2026-11-27", true, true)
    );
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
    site.post_form(
        "/account/upgrade",
        &session,
        "plan=standard_year&renewal=yes&immediate=yes",
        None,
    )
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
    assert!(
        text.contains("Example Inc., 1 Example Street"),
        "the merchant's contact"
    );

    // French, from the browser or from the switch.
    let french = site
        .http
        .get(format!("{}/account", site.base))
        .header("cookie", &session)
        .header("accept-language", "fr-CA,fr;q=0.9")
        .send()
        .await
        .expect("an answer")
        .text()
        .await
        .expect("a body");
    assert!(french.contains("<html lang=\"fr\">"));
    assert!(french.contains("Votre compte"));
    assert!(french.contains("Gratuit"));
    assert!(french.contains("utilisés sur 100 Mo"));
    assert!(french.contains("Passer à Plus"));
    let french_consent = site
        .http
        .get(format!("{}/account/upgrade?plan=plus_year", site.base))
        .header("cookie", &session)
        .header("accept-language", "fr-CA")
        .send()
        .await
        .expect("an answer")
        .text()
        .await
        .expect("a body");
    assert!(french_consent.contains("Plus, annuel : 96,00 USD par année, plus taxes"));
    assert!(french_consent.contains("pendant le délai de rétractation de 14 jours"));
    assert!(french_consent.contains("Continuer vers le paiement"));
    let switch = site
        .get("/account/language?to=en&back=/account", Some(&session))
        .await;
    assert_eq!(location(&switch), "/account");
    let chosen = switch.headers()["set-cookie"]
        .to_str()
        .expect("text")
        .to_owned();
    assert!(chosen.starts_with("noura_lang=en;"));
    let english = site
        .http
        .get(format!("{}/account", site.base))
        .header("cookie", format!("{session}; noura_lang=en"))
        .header("accept-language", "fr-CA")
        .send()
        .await
        .expect("an answer")
        .text()
        .await
        .expect("a body");
    assert!(english.contains("Your account"));
    let elsewhere = site
        .get(
            "/account/language?to=fr&back=https://evil.example",
            Some(&session),
        )
        .await;
    assert_eq!(location(&elsewhere), "/account");
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

/// An `order.paid` event for `account`'s subscription `sub-1`, billed to
/// an address in `country` and `state`.
fn paid(account: Id, order: &str, reason: &str, country: &str, state: &str) -> Value {
    json!({
        "type": "order.paid",
        "timestamp": "2026-11-27T10:00:00Z",
        "api_version": "2026-10",
        "data": {
            "id": order,
            "created_at": "2026-11-27T09:59:00Z",
            "modified_at": "2026-11-27T09:59:00Z",
            "status": "paid",
            "billing_reason": reason,
            "net_amount": 9_600,
            "tax_amount": 1_438,
            "total_amount": 11_038,
            "currency": "usd",
            "billing_name": "Ana Tremblay",
            "billing_address": {
                "line1": "1 rue Principale", "city": "Québec", "state": state,
                "postal_code": "G1A 1A1", "country": country,
            },
            "product_id": "prod-plus-year",
            "subscription_id": "sub-1",
            "metadata": {},
            "customer": { "id": "cus-1", "external_id": account.to_base64url() },
            "subscription": {
                "id": "sub-1", "amount": 9_600, "currency": "usd",
                "recurring_interval": "year",
                "current_period_start": "2026-11-27T09:59:00Z",
                "current_period_end": "2027-11-27T09:59:00Z",
                "started_at": "2026-11-27T09:59:00Z",
                "cancel_at_period_end": false,
                "metadata": {},
            },
        },
    })
}

#[tokio::test]
async fn the_first_payment_mails_the_contract() {
    let site = site(true).await;
    let (account, _) = site.signed_in("ana@example.com").await;
    let now = site.now();
    site.store
        .record_consent(
            account,
            sync_store::ConsentRow {
                accepted_at: 1_795_773_000,
                plan: "plus_year".into(),
                version: "2026-11-27".into(),
                immediate: true,
                renewal: true,
            },
        )
        .await
        .expect("recorded");

    // A first payment from Québec: the contract, French first.
    let first = paid(account, "ord-1", "subscription_create", "CA", "CA-QC");
    assert_eq!(
        site.deliver("evt-o1", now, &first, SECRET).await,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        site.deliver("evt-o1", now, &first, SECRET).await,
        StatusCode::ACCEPTED
    );
    let queue = site.store.billing_mail().await.expect("queue");
    assert_eq!(queue.len(), 1, "one contract, whatever the retries");
    let mail = &queue[0].mail;
    assert_eq!(mail.recipient, "ana@example.com");
    assert_eq!(
        mail.subject,
        "Votre contrat Noura Sync / Your Noura Sync contract"
    );
    let (french, english) = mail.body.split_once("* * *").expect("both languages");
    for line in [
        "Client : Ana Tremblay",
        "Adresse de facturation : 1 rue Principale, Québec QC G1A 1A1, CA",
        "Date : 27 novembre 2026",
        "Forfait : Plus, annuel",
        "Prix : 96,00 USD par année, avant taxes",
        "Taxes : 14,38 USD",
        "Total facturé : 110,38 USD",
        "Prochain renouvellement : 27 novembre 2027.",
        "Tarif mensuel : 8,00 USD",
        "Fin de la période en cours : 27 novembre 2027",
        "durée indéterminée",
        "sans indemnité",
        "Votre consentement : le 27 novembre 2026",
        "Conditions : https://example.com/fr/terms/",
        "Vendeur : Example Inc., 1 Example Street",
    ] {
        assert!(french.contains(line), "{line}\n{french}");
    }
    for line in [
        "Customer: Ana Tremblay",
        "Price: USD 96.00 a year, before tax",
        "Monthly rate: USD 8.00",
        "To cancel, sign in at http://localhost:",
        "Terms: https://example.com/terms/",
    ] {
        assert!(english.contains(line), "{line}\n{english}");
    }
    assert_eq!(
        site.store.latest_order(account).await.expect("read"),
        Some(sync_store::OrderRow {
            subscription: "sub-1".into(),
            order_id: "ord-1".into(),
            net: 9_600,
            currency: "usd".into(),
            paid_at: 1_795_773_540,
        })
    );

    // A renewal stores its order and mails no contract; elsewhere than in
    // Québec, the contract has one language and no Québec lines.
    let id = queue[0].id;
    site.store.sent_billing_mail(id).await.expect("sent");
    let renewal = paid(account, "ord-2", "subscription_cycle", "US", "US-CA");
    site.deliver("evt-o2", now, &renewal, SECRET).await;
    assert!(site.store.billing_mail().await.expect("queue").is_empty());
    assert_eq!(
        site.store
            .latest_order(account)
            .await
            .expect("read")
            .map(|row| row.order_id),
        Some("ord-2".into())
    );
    let (other, _) = site.signed_in("lee@example.com").await;
    let contract = paid(other, "ord-3", "subscription_create", "US", "US-CA");
    site.deliver("evt-o3", now, &contract, SECRET).await;
    let queue = site.store.billing_mail().await.expect("queue");
    assert_eq!(queue[0].mail.subject, "Your Noura Sync contract");
    assert!(!queue[0].mail.body.contains("Monthly rate"));
    assert!(!queue[0].mail.body.contains("Your consent"));
    assert!(
        queue[0]
            .mail
            .body
            .contains("stays active until the end of the period")
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
        ("NOURA_MERCHANT_NAME", "Example Inc."),
        ("NOURA_MERCHANT_ADDRESS", "1 Example Street"),
        ("NOURA_MERCHANT_EMAIL", "billing@example.com"),
        ("NOURA_MERCHANT_PHONE", "+1 418 555 0100"),
    ];
    for (settings, says) in [
        (&full[..1], "together"),
        (&full[1..3], "together"),
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
                full[3],
                full[4],
                full[5],
                full[6],
                ("NOURA_POLAR_API", "http://api.polar.sh"),
            ][..],
            "HTTPS",
        ),
        (&full[..6], "NOURA_MERCHANT_PHONE"),
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

#[tokio::test]
async fn quebec_cancels_today_and_gets_the_unused_part_back() {
    let site = site(true).await;
    let fake = site.fake.clone().expect("billing");
    let (account, session) = site.paying("qc@example.com", "CA", "CA-QC", 73).await;
    assert_eq!(site.plan(account).await, "plus_year");

    // After 14 days: cancel, and no refund button.
    let page = site.page("/account", &session).await;
    assert!(page.contains("href=\"/account/cancel\""));
    assert!(!page.contains("/account/refund"));
    assert!(!page.contains("/account/withdraw"));

    // The page offers today, with 292 of 365 days back, or the period's end.
    let ask = site.page("/account/cancel", &session).await;
    assert!(ask.contains("get USD 76.80 back, plus tax"), "{ask}");
    assert!(ask.contains("value=\"today\""));
    assert!(ask.contains("value=\"end\""));

    let done = site
        .post_form("/account/cancel", &session, "when=today", None)
        .await;
    assert_eq!(done.status(), StatusCode::OK);
    assert!(
        done.text()
            .await
            .expect("a body")
            .contains("Your plan is cancelled")
    );
    let refund = fake.last("/v1/refunds/");
    assert_eq!(refund.body["order_id"], "ord-1");
    assert_eq!(refund.body["amount"], 7_680);
    assert_eq!(refund.body["reason"], "customer_request");
    assert_eq!(fake.last("DELETE /v1/subscriptions").body["id"], "sub-1");
    assert_eq!(fake.count("PATCH /v1/subscriptions"), 0);
    assert_eq!(
        site.plan(account).await,
        "free",
        "the refund ends the period"
    );
    assert!(site.store.refunds_owed().await.expect("owed").is_empty());

    // The confirmation is in French first, then English.
    let queue = site.store.billing_mail().await.expect("queue");
    assert_eq!(queue.len(), 1);
    let mail = &queue[0].mail;
    assert_eq!(mail.recipient, "qc@example.com");
    assert_eq!(
        mail.subject,
        "Votre forfait Noura est résilié / Your Noura plan is cancelled"
    );
    assert!(mail.body.contains("Nous vous remboursons 76,80 USD"));
    assert!(mail.body.contains("We're refunding USD 76.80 plus the tax"));

    // The pages no longer offer to cancel.
    let page = site.page("/account", &session).await;
    assert!(!page.contains("/account/cancel"));
    assert_eq!(
        location(&site.get("/account/cancel", Some(&session)).await),
        "/account"
    );
}

#[tokio::test]
async fn elsewhere_a_cancellation_ends_with_the_period() {
    let site = site(true).await;
    let fake = site.fake.clone().expect("billing");
    let (account, session) = site.paying("on@example.com", "CA", "CA-ON", 30).await;
    let ask = site.page("/account/cancel", &session).await;
    assert!(ask.contains("stays active until"));
    assert!(!ask.contains("value=\"today\""));

    // Asking for today outside Québec still ends with the period.
    let done = site
        .post_form("/account/cancel", &session, "when=today", None)
        .await;
    assert!(
        done.text()
            .await
            .expect("a body")
            .contains("It stays active until")
    );
    assert_eq!(
        fake.last("PATCH /v1/subscriptions").body["cancel_at_period_end"],
        true
    );
    assert_eq!(fake.count("/v1/refunds/"), 0);
    assert_eq!(fake.count("DELETE /v1/subscriptions"), 0);
    assert_eq!(site.plan(account).await, "plus_year");
    let page = site.page("/account", &session).await;
    assert!(page.contains("Your plan ends on"));
    assert!(!page.contains("/account/cancel"));
    let subjects = site.queued().await;
    assert_eq!(subjects.len(), 1);
    assert!(subjects[0].starts_with("Your Noura plan ends on"));

    // A cross-site post changes nothing.
    let (_, other) = site.paying("on2@example.com", "CA", "CA-ON", 30).await;
    let foreign = site
        .http
        .post(format!("{}/account/cancel", site.base))
        .header("cookie", &other)
        .header("origin", "https://evil.example")
        .body("when=end")
        .send()
        .await
        .expect("an answer");
    assert_eq!(foreign.status(), StatusCode::FORBIDDEN);
    assert_eq!(fake.count("PATCH /v1/subscriptions"), 1);
}

#[tokio::test]
async fn a_withdrawal_in_the_eu_refunds_everything_and_ends_the_plan() {
    let site = site(true).await;
    let fake = site.fake.clone().expect("billing");
    let (account, session) = site.paying("fr@example.com", "FR", "", 3).await;
    let page = site.page("/account", &session).await;
    assert!(page.contains("Withdraw from contract here"));
    assert!(
        !page.contains("/account/refund"),
        "withdrawal replaces the refund"
    );

    let ask = site.page("/account/withdraw", &session).await;
    assert!(ask.contains("everything you paid, USD 96.00 plus tax"));
    let french = site
        .http
        .get(format!("{}/account/withdraw", site.base))
        .header("cookie", format!("{session}; noura_lang=fr"))
        .send()
        .await
        .expect("an answer")
        .text()
        .await
        .expect("a body");
    assert!(french.contains("Se rétracter du contrat"));
    assert!(french.contains("Confirmer la rétractation"));

    site.post_form("/account/withdraw", &session, "when=today", None)
        .await;
    assert_eq!(fake.last("/v1/refunds/").body["amount"], 9_600);
    assert_eq!(fake.count("DELETE /v1/subscriptions"), 1);
    assert_eq!(site.plan(account).await, "free");
    assert_eq!(site.queued().await, ["We received your withdrawal"]);
    let page = site.page("/account", &session).await;
    assert!(!page.contains("Withdraw from contract here"));

    // After 14 days the button is gone.
    let (_, late) = site.paying("late@example.com", "DE", "", 15).await;
    assert!(
        !site
            .page("/account", &late)
            .await
            .contains("/account/withdraw")
    );
    assert_eq!(
        location(&site.get("/account/withdraw", Some(&late)).await),
        "/account"
    );
}

#[tokio::test]
async fn a_refund_ends_the_plan_at_once_and_the_jobs_retry_what_failed() {
    let site = site(true).await;
    let fake = site.fake.clone().expect("billing");
    let (account, session) = site.paying("us@example.com", "US", "US-CA", 2).await;
    assert!(
        site.page("/account", &session)
            .await
            .contains("Refund last payment")
    );
    assert!(
        site.page("/account/refund", &session)
            .await
            .contains("You get USD 96.00 back")
    );

    // Polar's refunds fail for a while: the plan still ends at once, and
    // the store keeps the refund it owes.
    fake.refunds_fail
        .store(true, std::sync::atomic::Ordering::SeqCst);
    site.post_form("/account/refund", &session, "when=today", None)
        .await;
    assert_eq!(site.plan(account).await, "free");
    assert_eq!(fake.count("DELETE /v1/subscriptions"), 1);
    let owed = site.store.refunds_owed().await.expect("owed");
    assert_eq!(owed.len(), 1);
    assert_eq!((owed[0].amount, owed[0].revoke), (9_600, false));
    assert_eq!(site.queued().await, ["Your Noura refund"]);
    assert_eq!(site.plan_jobs().await.settled, 0);
    assert_eq!(site.store.refunds_owed().await.expect("owed").len(), 1);

    // Once Polar answers, the next pass pays it, once.
    fake.refunds_fail
        .store(false, std::sync::atomic::Ordering::SeqCst);
    assert_eq!(site.plan_jobs().await.settled, 1);
    assert_eq!(fake.count("/v1/refunds/"), 1);
    assert_eq!(fake.last("/v1/refunds/").body["amount"], 9_600);
    assert!(site.store.refunds_owed().await.expect("owed").is_empty());
    assert_eq!(site.plan_jobs().await.settled, 0);
    assert_eq!(fake.count("/v1/refunds/"), 1);

    // A second post finds nothing to refund.
    let again = site
        .post_form("/account/refund", &session, "when=today", None)
        .await;
    assert_eq!(location(&again), "/account");
    assert_eq!(fake.count("/v1/refunds/"), 1);
}

#[tokio::test]
async fn yearly_plans_get_two_reminders_and_price_changes_two_notices() {
    let site = site(true).await;
    let (_, _) = site.paying("year@example.com", "US", "US-NY", 280).await;
    let advance = |days: u64| {
        site.store
            .clock()
            .advance(std::time::Duration::from_secs(days * DAY))
    };

    // 85 days before the renewal: the first reminder, once.
    assert_eq!(site.plan_jobs().await.reminders, 1);
    assert_eq!(site.plan_jobs().await.reminders, 0);
    let queue = site.store.billing_mail().await.expect("queue");
    assert_eq!(queue.len(), 1);
    let mail = &queue[0].mail;
    assert_eq!(mail.recipient, "year@example.com");
    assert!(mail.subject.starts_with("Your Noura plan renews on"));
    assert!(mail.body.contains("Your Plus, yearly plan renews on"));
    assert!(mail.body.contains("for USD 96.00 plus tax"));
    assert!(
        mail.body
            .contains("choose Cancel plan, or email billing@example.com")
    );

    // 54 days before, nothing; 44 days before, the second; nothing more
    // after it.
    advance(30);
    assert_eq!(site.plan_jobs().await.reminders, 0);
    advance(10);
    assert_eq!(site.plan_jobs().await.reminders, 1);
    advance(20);
    assert_eq!(site.plan_jobs().await.reminders, 0);
    assert_eq!(site.queued().await.len(), 2);

    // A price change of Plus, yearly, 40 days out: a notice now, and a
    // reminder in the last 30 days.
    let effective =
        jiff::Timestamp::from_second(i64::try_from(site.now() + 40 * DAY).expect("a time"))
            .expect("a time")
            .to_zoned(jiff::tz::TimeZone::UTC)
            .date()
            .to_string();
    let announce = |effective: &str| sync_server::billing::Announcement {
        plan: "plus_year".into(),
        effective: effective.into(),
        old_en: "USD 96 a year".into(),
        new_en: "USD 108 a year".into(),
        old_fr: "96 $ US par année".into(),
        new_fr: "108 $ US par année".into(),
    };
    let soon = jiff::Timestamp::from_second(i64::try_from(site.now() + 10 * DAY).expect("a time"))
        .expect("a time")
        .to_zoned(jiff::tz::TimeZone::UTC)
        .date()
        .to_string();
    let refused = sync_server::billing::announce_price(&site.store, announce(&soon))
        .await
        .expect_err("too soon");
    assert!(refused.contains("at least 30 days"), "{refused}");
    sync_server::billing::announce_price(&site.store, announce(&effective))
        .await
        .expect("announced");
    assert_eq!(site.plan_jobs().await.price_notices, 1);
    assert_eq!(site.plan_jobs().await.price_notices, 0);
    let queue = site.store.billing_mail().await.expect("queue");
    let notice = &queue.last().expect("a notice").mail;
    assert!(
        notice
            .subject
            .starts_with("The price of your Noura plan changes on")
    );
    assert!(notice.body.contains("Now: USD 96 a year"));
    assert!(notice.body.contains("USD 108 a year"));
    assert!(
        notice
            .body
            .contains("refuse the change by cancelling your plan, at no cost")
    );

    // A subscriber refusing the change may end the plan today.
    advance(15);
    assert_eq!(site.plan_jobs().await.price_notices, 1);
    let queue = site.store.billing_mail().await.expect("queue");
    assert!(
        queue
            .last()
            .expect("a reminder")
            .mail
            .subject
            .starts_with("Reminder: the price of your Noura plan changes on")
    );
    let (_, refusing) = site.paying("refuse@example.com", "US", "US-NY", 100).await;
    let ask = site.page("/account/cancel", &refusing).await;
    assert!(ask.contains("value=\"today\""), "{ask}");
}

/// `billing announce-price` stores a change 30 days ahead or more, and
/// refuses one too soon or for a plan the server doesn't sell.
#[test]
fn the_binary_announces_a_price_change() {
    let dir = tempfile::tempdir().expect("temporary directory");
    let data = dir.path().join("data");
    assert!(crate::support::run(&["migrate"], &data).status.success());
    let date = |days: i64| {
        jiff::Timestamp::now()
            .checked_add(jiff::SignedDuration::from_hours(24 * days))
            .expect("a time")
            .to_zoned(jiff::tz::TimeZone::UTC)
            .date()
            .to_string()
    };
    let announce = |plan: &str, effective: &str| {
        std::process::Command::new(crate::support::BIN)
            .args(["billing", "announce-price", "--data"])
            .arg(&data)
            .args(["--plan", plan, "--effective", effective])
            .args(["--old-en", "USD 96 a year", "--new-en", "USD 108 a year"])
            .args([
                "--old-fr",
                "96 $ US par année",
                "--new-fr",
                "108 $ US par année",
            ])
            .output()
            .expect("the binary runs")
    };
    let soon = announce("plus_year", &date(5));
    assert!(!soon.status.success());
    assert!(String::from_utf8_lossy(&soon.stderr).contains("at least 30 days"));
    let unknown = announce("gold", &date(60));
    assert!(String::from_utf8_lossy(&unknown.stderr).contains("isn't a paid plan ID"));
    let done = announce("plus_year", &date(60));
    assert!(
        done.status.success(),
        "{}",
        String::from_utf8_lossy(&done.stderr)
    );
    assert!(String::from_utf8_lossy(&done.stderr).contains("announced price change 1"));
}

/// A visit to the account page, where every sign-in lands, counts as
/// activity: it clears an idle owner's warnings, and records the page's
/// language for the mail about plans.
#[tokio::test]
async fn a_sign_in_on_the_account_pages_keeps_an_idle_account() {
    let site = site(true).await;
    let (account, session) = site.signed_in("idle@example.com").await;
    let month = sync_server::maintenance::month_of(site.now());
    site.store
        .note_activity(account, month - 10)
        .await
        .expect("noted");
    site.store
        .set_inactivity_warnings(account, 1)
        .await
        .expect("warned");
    let page = site
        .http
        .get(format!("{}/account", site.base))
        .header("cookie", &session)
        .header("accept-language", "fr-CA")
        .send()
        .await
        .expect("an answer");
    assert_eq!(page.status(), StatusCode::OK);
    let owner = site.store.inactivity(account).await.expect("read");
    assert_eq!((owner.last_month, owner.warnings), (month, 0));
    assert_eq!(
        site.store
            .account_lang(account)
            .await
            .expect("read")
            .as_deref(),
        Some("fr")
    );
}
