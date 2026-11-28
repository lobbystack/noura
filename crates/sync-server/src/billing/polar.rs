//! Polar (<https://polar.sh>), the merchant of record that runs checkout,
//! tax, invoices, refunds and the customer portal, on API version 2026-10:
//!
//! - prices: `GET /v1/products/{id}`, whose fixed price and recurring
//!   interval the upgrade page shows before the checkout
//!   (<https://polar.sh/docs/api-reference/2026-10/products/get>);
//! - checkout: `POST /v1/checkouts/` with the plan's products, the account
//!   as `external_customer_id`, `success_url` and the customer's email
//!   (<https://polar.sh/docs/api-reference/2026-10/checkouts/create-checkout-session>);
//! - portal: `POST /v1/customer-sessions/` by `external_customer_id`, whose
//!   `customer_portal_url` signs the customer in for an hour
//!   (<https://polar.sh/docs/api-reference/2026-10/customer-sessions/create-customer-session>);
//! - webhooks: Standard Webhooks signatures, the `subscription.*` events
//!   whose payload is the subscription with its customer and product, and
//!   `order.paid`, whose payload is the order with its customer,
//!   subscription and billing address
//!   (<https://polar.sh/docs/integrate/webhooks/delivery>);
//! - cancellation: `PATCH /v1/subscriptions/{id}` with
//!   `cancel_at_period_end`, and revocation: `DELETE
//!   /v1/subscriptions/{id}`, which answers 403 for one already revoked
//!   (<https://polar.sh/docs/api-reference/2026-10/subscriptions/update>);
//! - refunds: `POST /v1/refunds/` with the order, a reason and an amount
//!   before tax; a refund doesn't end the subscription
//!   (<https://polar.sh/docs/api-reference/2026-10/refunds/create>).

use std::collections::BTreeMap;

use axum::http::HeaderMap;
use serde_json::{Value, json};
use sync_protocol::types::Id;
use sync_store::SubscriptionState;

use super::webhooks::{self, Secret};
use super::{
    BillingProvider, ChangeKind, CheckoutRequest, PaidOrder, PortalRequest, Price, ProviderError,
    ProviderFuture, SubscriptionChange, Terms, WebhookError, WebhookEvent,
};

/// Polar's production API.
pub const API: &str = "https://api.polar.sh";

/// Polar's sandbox API.
pub const SANDBOX_API: &str = "https://sandbox-api.polar.sh";

/// The API version every request and the webhook endpoint pin.
pub const API_VERSION: &str = "2026-10";

/// The metadata keys the server sets on a checkout, and reads back from a
/// product when its ID isn't in the mapping.
const ACCOUNT_KEY: &str = "noura_account";
const PLAN_KEY: &str = "noura_plan";

/// How `serve` reaches Polar.
#[derive(Clone)]
pub struct PolarConfig {
    /// The organization access token.
    pub access_token: String,
    /// The webhook endpoint's secret.
    pub webhook_secret: String,
    /// The API's origin, such as [`API`] or [`SANDBOX_API`].
    pub api: String,
    /// The product that sells each plan ID.
    pub products: BTreeMap<String, String>,
}

impl std::fmt::Debug for PolarConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PolarConfig")
            .field("api", &self.api)
            .field("products", &self.products)
            .finish_non_exhaustive()
    }
}

impl PolarConfig {
    /// Reads `NOURA_POLAR_PRODUCTS`: `plan=product` pairs separated by
    /// commas, each plan a paid plan ID of §20 at most once.
    pub fn parse_products(text: &str) -> Result<BTreeMap<String, String>, String> {
        let mut products = BTreeMap::new();
        for pair in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
            let (plan, product) = pair
                .split_once('=')
                .map(|(plan, product)| (plan.trim(), product.trim()))
                .filter(|(_, product)| !product.is_empty())
                .ok_or_else(|| format!("NOURA_POLAR_PRODUCTS holds {pair:?}, not plan=product"))?;
            if !sync_store::plans::is_paid(plan) {
                return Err(format!(
                    "NOURA_POLAR_PRODUCTS names {plan:?}, which isn't a plan ID"
                ));
            }
            if products
                .insert(plan.to_owned(), product.to_owned())
                .is_some()
            {
                return Err(format!("NOURA_POLAR_PRODUCTS names {plan:?} twice"));
            }
        }
        if products.is_empty() {
            return Err("NOURA_POLAR_PRODUCTS names no product".into());
        }
        Ok(products)
    }
}

/// Polar as the server's payment provider.
pub struct Polar {
    http: reqwest::Client,
    config: PolarConfig,
    secret: Secret,
}

impl Polar {
    /// A provider on `config`.
    pub fn new(config: PolarConfig) -> Self {
        let secret = Secret::new(&config.webhook_secret);
        Self {
            http: reqwest::Client::new(),
            config,
            secret,
        }
    }

    fn endpoint(&self, path: &str) -> String {
        format!("{}{path}", self.config.api.trim_end_matches('/'))
    }

    /// Sends `body`, if any, to `path` and returns the answer's JSON, or
    /// the kind of failure. Polar's error bodies can repeat the customer's
    /// email, so they stay unread.
    async fn call(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, ProviderError> {
        let mut request = self
            .http
            .request(method, self.endpoint(path))
            .bearer_auth(&self.config.access_token)
            .header("Polar-Version", API_VERSION)
            .header("Accept", "application/json");
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|_| ProviderError::Unavailable)?;
        let status = response.status();
        if !status.is_success() {
            return Err(ProviderError::Status(status.as_u16()));
        }
        response
            .json()
            .await
            .map_err(|_| ProviderError::Unavailable)
    }

    async fn post(&self, path: &str, body: Value) -> Result<Value, ProviderError> {
        self.call(reqwest::Method::POST, path, Some(body)).await
    }

    /// The plan a subscription's or an order's product sells: the
    /// configured mapping, or the product's `metadata.noura_plan`, or its
    /// `external_id`, when that names a paid plan.
    fn plan_of(&self, data: &Value) -> Option<String> {
        let product = data["product_id"].as_str()?;
        if let Some((plan, _)) = self.config.products.iter().find(|(_, id)| *id == product) {
            return Some(plan.clone());
        }
        [
            &data["product"]["metadata"][PLAN_KEY],
            &data["product"]["external_id"],
        ]
        .into_iter()
        .filter_map(Value::as_str)
        .find(|plan| sync_store::plans::is_paid(plan))
        .map(str::to_owned)
    }

    /// The paid order an `order.paid` event's `data` reports, or `None`
    /// when it pays for no subscription, or names no account or no plan.
    fn order(&self, data: &Value) -> Option<PaidOrder> {
        let subscription = &data["subscription"];
        let account = account_of(&data["customer"], subscription)
            .or_else(|| account_of(&Value::Null, data))?;
        let plan = self.plan_of(data)?;
        let address = data["billing_address"]
            .as_object()
            .map(|_| &data["billing_address"])
            .unwrap_or(&data["customer"]["billing_address"]);
        let mut terms = terms_of(subscription, address);
        terms.amount = terms.amount.or_else(|| uint(&data["net_amount"]));
        let amount = |key: &str| uint(&data[key]).unwrap_or(0);
        Some(PaidOrder {
            account,
            plan,
            subscription: data["subscription_id"].as_str()?.to_owned(),
            order_id: data["id"].as_str()?.to_owned(),
            first: data["billing_reason"].as_str() == Some("subscription_create"),
            paid_at: [&data["modified_at"], &data["created_at"]]
                .into_iter()
                .find_map(timestamp)?,
            net: amount("net_amount"),
            tax: amount("tax_amount"),
            total: amount("total_amount"),
            currency: data["currency"].as_str()?.to_ascii_lowercase(),
            name: [&data["billing_name"], &data["customer"]["name"]]
                .into_iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .find(|name| !name.is_empty())
                .map(str::to_owned),
            address: address_lines(address),
            period_end: timestamp(&subscription["current_period_end"]),
            terms,
        })
    }

    /// The subscription a `subscription.*` event's `data` reports, or `None`
    /// when it names no account or no plan.
    fn change(
        &self,
        kind: ChangeKind,
        data: &Value,
        sent: Option<u64>,
    ) -> Option<SubscriptionChange> {
        let account = account_of(&data["customer"], data)?;
        let plan = self.plan_of(data)?;
        let state = match data["status"].as_str()? {
            "active" | "trialing" => SubscriptionState::Active,
            "past_due" => SubscriptionState::PastDue,
            _ => SubscriptionState::Ended,
        };
        let changed_at = [&data["modified_at"], &data["created_at"]]
            .into_iter()
            .find_map(timestamp)
            .or(sent)?;
        Some(SubscriptionChange {
            kind,
            account,
            plan,
            customer: data["customer_id"].as_str()?.to_owned(),
            subscription: data["id"].as_str()?.to_owned(),
            state,
            period_end: timestamp(&data["current_period_end"]),
            changed_at,
            terms: terms_of(data, &data["customer"]["billing_address"]),
        })
    }
}

/// The account a customer's `external_id`, or else the `metadata` of
/// `holder`, names.
fn account_of(customer: &Value, holder: &Value) -> Option<Id> {
    [&customer["external_id"], &holder["metadata"][ACCOUNT_KEY]]
        .into_iter()
        .filter_map(Value::as_str)
        .find_map(|text| Id::from_base64url(text).ok())
}

/// A non-negative JSON integer.
fn uint(value: &Value) -> Option<u64> {
    value.as_u64()
}

/// A subscription's terms, with the country and region of `address`.
fn terms_of(subscription: &Value, address: &Value) -> Terms {
    let text = |value: &Value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_owned)
    };
    Terms {
        period_start: timestamp(&subscription["current_period_start"]),
        started_at: timestamp(&subscription["started_at"])
            .or_else(|| timestamp(&subscription["created_at"])),
        amount: uint(&subscription["amount"]),
        currency: text(&subscription["currency"]).map(|c| c.to_ascii_lowercase()),
        interval: text(&subscription["recurring_interval"]),
        canceling: subscription["cancel_at_period_end"].as_bool() == Some(true),
        country: text(&address["country"]).map(|c| c.to_ascii_uppercase()),
        region: text(&address["state"]).map(|state| super::region_code(&state)),
    }
}

/// An address's lines, as an invoice prints them.
fn address_lines(address: &Value) -> Vec<String> {
    let part = |key: &str| {
        address[key]
            .as_str()
            .map(str::trim)
            .filter(|text| !text.is_empty())
    };
    let mut lines: Vec<String> = [part("line1"), part("line2")]
        .into_iter()
        .flatten()
        .map(str::to_owned)
        .collect();
    let state = part("state").map(super::region_code);
    let city: Vec<&str> = [part("city"), state.as_deref(), part("postal_code")]
        .into_iter()
        .flatten()
        .collect();
    if !city.is_empty() {
        lines.push(city.join(" "));
    }
    if let Some(country) = part("country") {
        lines.push(country.to_owned());
    }
    lines
}

/// An RFC 3339 time in JSON, in Unix seconds.
fn timestamp(value: &Value) -> Option<u64> {
    let time: jiff::Timestamp = value.as_str()?.parse().ok()?;
    u64::try_from(time.as_second()).ok()
}

/// The kind of change a Polar event type reports, for the `subscription.*`
/// events this server reads.
fn kind_of(event: &str) -> Option<ChangeKind> {
    Some(match event {
        "subscription.created"
        | "subscription.active"
        | "subscription.uncanceled"
        | "subscription.resumed" => ChangeKind::Active,
        "subscription.updated" => ChangeKind::Updated,
        "subscription.canceled" => ChangeKind::Canceled,
        "subscription.past_due" => ChangeKind::PaymentFailed,
        "subscription.revoked" | "subscription.paused" => ChangeKind::Ended,
        _ => return None,
    })
}

impl BillingProvider for Polar {
    fn name(&self) -> &'static str {
        "polar"
    }

    fn sells(&self, plan: &str) -> bool {
        self.config.products.contains_key(plan)
    }

    fn price<'a>(&'a self, plan: &'a str) -> ProviderFuture<'a, Option<Price>> {
        Box::pin(async move {
            let Some(product) = self.config.products.get(plan) else {
                return Ok(None);
            };
            let path = format!("/v1/products/{product}");
            let answer = self.call(reqwest::Method::GET, &path, None).await?;
            let Some(interval) = answer["recurring_interval"].as_str() else {
                return Ok(None);
            };
            let fixed = answer["prices"].as_array().and_then(|prices| {
                prices
                    .iter()
                    .find(|price| price["amount_type"] == "fixed" && price["is_archived"] != true)
            });
            Ok(fixed.and_then(|price| {
                Some(Price {
                    amount: price["price_amount"].as_u64()?,
                    currency: price["price_currency"].as_str()?.to_ascii_lowercase(),
                    interval: interval.to_owned(),
                })
            }))
        })
    }

    fn checkout_url<'a>(&'a self, request: CheckoutRequest<'a>) -> ProviderFuture<'a, String> {
        Box::pin(async move {
            let products: Vec<&String> = request
                .plans
                .iter()
                .filter_map(|plan| self.config.products.get(*plan))
                .collect();
            if products.is_empty() {
                return Err(ProviderError::Status(404));
            }
            let account = request.account.to_base64url();
            let mut body = json!({
                "products": products,
                "external_customer_id": account,
                "success_url": request.success_url,
                "return_url": request.return_url,
                "metadata": { ACCOUNT_KEY: account, PLAN_KEY: request.plans[0] },
            });
            if let Some(email) = request.email {
                body["customer_email"] = json!(email);
            }
            if let Some(locale) = request.locale {
                body["locale"] = json!(locale);
            }
            let answer = self.post("/v1/checkouts/", body).await?;
            answer["url"]
                .as_str()
                .map(str::to_owned)
                .ok_or(ProviderError::Unavailable)
        })
    }

    fn portal_url<'a>(&'a self, request: PortalRequest<'a>) -> ProviderFuture<'a, String> {
        Box::pin(async move {
            let body = match request.customer {
                Some(customer) => json!({
                    "customer_id": customer,
                    "return_url": request.return_url,
                }),
                None => json!({
                    "external_customer_id": request.account.to_base64url(),
                    "return_url": request.return_url,
                }),
            };
            let answer = match self.post("/v1/customer-sessions/", body).await {
                Err(ProviderError::Status(404 | 422)) => return Err(ProviderError::NoCustomer),
                other => other?,
            };
            answer["customer_portal_url"]
                .as_str()
                .map(str::to_owned)
                .ok_or(ProviderError::Unavailable)
        })
    }

    fn cancel<'a>(&'a self, subscription: &'a str) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            let path = format!("/v1/subscriptions/{subscription}");
            let body = json!({ "cancel_at_period_end": true });
            match self.call(reqwest::Method::PATCH, &path, Some(body)).await {
                // Already canceled, or no longer active: nothing renews.
                Ok(_) | Err(ProviderError::Status(403)) => Ok(()),
                Err(error) => Err(error),
            }
        })
    }

    fn revoke<'a>(&'a self, subscription: &'a str) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            let path = format!("/v1/subscriptions/{subscription}");
            match self.call(reqwest::Method::DELETE, &path, None).await {
                // Already revoked.
                Ok(_) | Err(ProviderError::Status(403)) => Ok(()),
                Err(error) => Err(error),
            }
        })
    }

    fn refund<'a>(&'a self, order: &'a str, amount: u64) -> ProviderFuture<'a, ()> {
        Box::pin(async move {
            let body = json!({
                "order_id": order,
                "reason": "customer_request",
                "amount": amount,
            });
            match self.post("/v1/refunds/", body).await {
                // The payment processor already refunded the payment.
                Ok(_) | Err(ProviderError::Status(409)) => Ok(()),
                Err(error) => Err(error),
            }
        })
    }

    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        now: u64,
    ) -> Result<WebhookEvent, WebhookError> {
        let id = webhooks::verify(&self.secret, headers, body, now)?;
        let payload: Value = serde_json::from_slice(body).map_err(|_| WebhookError::Malformed)?;
        let event = payload["type"]
            .as_str()
            .ok_or(WebhookError::Malformed)?
            .to_owned();
        let data = &payload["data"];
        let reads = kind_of(&event).is_some() || event == "order.paid";
        if reads && !data.is_object() {
            return Err(WebhookError::Malformed);
        }
        let change = kind_of(&event)
            .and_then(|kind| self.change(kind, data, timestamp(&payload["timestamp"])));
        let order = (event == "order.paid").then(|| self.order(data)).flatten();
        Ok(WebhookEvent {
            id,
            kind: event,
            change,
            order,
        })
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn polar() -> Polar {
        Polar::new(PolarConfig {
            access_token: "polar_oat_test".into(),
            webhook_secret: "whsec_MfKQ9r8GKYqrTwjUPD8ILPZIo2LaLaSw".into(),
            api: "http://127.0.0.1:1".into(),
            products: PolarConfig::parse_products(
                "plus_year=prod-plus-year,standard_month=prod-std",
            )
            .expect("products"),
        })
    }

    fn signed(polar: &Polar, id: &str, now: u64, body: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert("webhook-id", HeaderValue::from_str(id).unwrap());
        headers.insert(
            "webhook-timestamp",
            HeaderValue::from_str(&now.to_string()).unwrap(),
        );
        let signature = polar.secret.signature(id, now, body.as_bytes());
        headers.insert(
            "webhook-signature",
            HeaderValue::from_str(&signature).unwrap(),
        );
        headers
    }

    fn subscription(event: &str, status: &str, product: &str, metadata_plan: &str) -> String {
        json!({
            "type": event,
            "timestamp": "2026-11-27T10:00:00Z",
            "api_version": "2026-10",
            "data": {
                "id": "sub-1",
                "status": status,
                "customer_id": "cus-1",
                "product_id": product,
                "modified_at": "2026-11-27T09:59:00Z",
                "current_period_end": "2027-11-27T09:59:00Z",
                "cancel_at_period_end": false,
                "metadata": {},
                "customer": { "id": "cus-1", "external_id": Id([5; 16]).to_base64url() },
                "product": { "id": product, "metadata": { "noura_plan": metadata_plan } },
            },
        })
        .to_string()
    }

    #[test]
    fn the_product_mapping_names_paid_plans_once() {
        assert!(PolarConfig::parse_products("plus_year=a, plus_month = b").is_ok());
        assert!(PolarConfig::parse_products("free=a").is_err());
        assert!(PolarConfig::parse_products("gold=a").is_err());
        assert!(PolarConfig::parse_products("plus_year=a,plus_year=b").is_err());
        assert!(PolarConfig::parse_products("plus_year").is_err());
        assert!(PolarConfig::parse_products("").is_err());
    }

    #[test]
    fn subscription_events_map_onto_the_stores_states() {
        let polar = polar();
        let now = 1_795_000_000;
        for (event, status, kind, state) in [
            (
                "subscription.active",
                "active",
                ChangeKind::Active,
                SubscriptionState::Active,
            ),
            (
                "subscription.past_due",
                "past_due",
                ChangeKind::PaymentFailed,
                SubscriptionState::PastDue,
            ),
            (
                "subscription.canceled",
                "active",
                ChangeKind::Canceled,
                SubscriptionState::Active,
            ),
            (
                "subscription.revoked",
                "canceled",
                ChangeKind::Ended,
                SubscriptionState::Ended,
            ),
            (
                "subscription.updated",
                "unpaid",
                ChangeKind::Updated,
                SubscriptionState::Ended,
            ),
        ] {
            let body = subscription(event, status, "prod-plus-year", "");
            let parsed = polar
                .verify_webhook(&signed(&polar, "evt-1", now, &body), body.as_bytes(), now)
                .expect("verified");
            assert_eq!(parsed.id, "evt-1");
            let change = parsed.change.expect("a change");
            assert_eq!((change.kind, change.state), (kind, state), "{event}");
            assert_eq!(change.account, Id([5; 16]));
            assert_eq!(change.plan, "plus_year");
            assert_eq!(change.customer, "cus-1");
            assert_eq!(change.subscription, "sub-1");
            assert_eq!(change.changed_at, 1_795_773_540);
            assert_eq!(change.period_end, Some(1_827_309_540));
        }
    }

    #[test]
    fn a_subscription_reports_its_terms_and_billing_region() {
        let polar = polar();
        let now = 1_795_000_000;
        let mut payload: Value = serde_json::from_str(&subscription(
            "subscription.active",
            "active",
            "prod-plus-year",
            "",
        ))
        .expect("JSON");
        let data = &mut payload["data"];
        data["amount"] = json!(9_600);
        data["currency"] = json!("USD");
        data["recurring_interval"] = json!("year");
        data["current_period_start"] = json!("2026-11-27T09:59:00Z");
        data["started_at"] = json!("2026-11-27T09:59:00Z");
        data["cancel_at_period_end"] = json!(true);
        data["customer"]["billing_address"] = json!({ "country": "CA", "state": "CA-QC" });
        let body = payload.to_string();
        let change = polar
            .verify_webhook(&signed(&polar, "e", now, &body), body.as_bytes(), now)
            .expect("verified")
            .change
            .expect("a change");
        assert_eq!(
            change.terms,
            Terms {
                period_start: Some(1_795_773_540),
                started_at: Some(1_795_773_540),
                amount: Some(9_600),
                currency: Some("usd".into()),
                interval: Some("year".into()),
                canceling: true,
                country: Some("CA".into()),
                region: Some("QC".into()),
            }
        );
        let row = change.row("polar");
        assert!(row.in_quebec() && row.yearly() && row.canceling);
    }

    #[test]
    fn a_paid_order_reports_its_amounts_customer_and_address() {
        let polar = polar();
        let now = 1_795_000_000;
        let account = Id([5; 16]);
        let body = json!({
            "type": "order.paid",
            "timestamp": "2026-11-27T10:00:00Z",
            "data": {
                "id": "ord-1",
                "created_at": "2026-11-27T09:58:00Z",
                "modified_at": "2026-11-27T09:59:00Z",
                "status": "paid",
                "billing_reason": "subscription_create",
                "net_amount": 9_600,
                "tax_amount": 1_438,
                "total_amount": 11_038,
                "currency": "usd",
                "billing_name": "Ana Tremblay",
                "billing_address": {
                    "line1": "1 rue Principale", "line2": null, "city": "Québec",
                    "state": "QC", "postal_code": "G1A 1A1", "country": "CA",
                },
                "product_id": "prod-plus-year",
                "subscription_id": "sub-1",
                "customer": { "id": "cus-1", "external_id": account.to_base64url(), "name": "Ana" },
                "subscription": {
                    "id": "sub-1", "amount": 9_600, "currency": "usd",
                    "recurring_interval": "year",
                    "current_period_start": "2026-11-27T09:59:00Z",
                    "current_period_end": "2027-11-27T09:59:00Z",
                    "started_at": "2026-11-27T09:59:00Z",
                    "cancel_at_period_end": false,
                },
            },
        })
        .to_string();
        let order = polar
            .verify_webhook(&signed(&polar, "e", now, &body), body.as_bytes(), now)
            .expect("verified")
            .order
            .expect("an order");
        assert_eq!(order.account, account);
        assert_eq!(order.plan, "plus_year");
        assert_eq!(
            (order.subscription.as_str(), order.order_id.as_str()),
            ("sub-1", "ord-1")
        );
        assert!(order.first);
        assert_eq!((order.net, order.tax, order.total), (9_600, 1_438, 11_038));
        assert_eq!(order.paid_at, 1_795_773_540);
        assert_eq!(order.name.as_deref(), Some("Ana Tremblay"));
        assert_eq!(
            order.address,
            ["1 rue Principale", "Québec QC G1A 1A1", "CA"]
        );
        assert_eq!(order.period_end, Some(1_827_309_540));
        assert_eq!(order.terms.region.as_deref(), Some("QC"));
        assert_eq!(order.terms.interval.as_deref(), Some("year"));
        assert_eq!(
            order.row(),
            sync_store::OrderRow {
                subscription: "sub-1".into(),
                order_id: "ord-1".into(),
                net: 9_600,
                currency: "usd".into(),
                paid_at: 1_795_773_540,
            }
        );
    }

    #[test]
    fn a_product_outside_the_mapping_falls_back_to_its_metadata() {
        let polar = polar();
        let now = 1_795_000_000;
        let body = subscription(
            "subscription.active",
            "active",
            "prod-new",
            "plus_100gb_year",
        );
        let change = polar
            .verify_webhook(&signed(&polar, "e", now, &body), body.as_bytes(), now)
            .expect("verified")
            .change
            .expect("a change");
        assert_eq!(change.plan, "plus_100gb_year");
        // Metadata that names no paid plan changes nothing.
        let body = subscription("subscription.active", "active", "prod-new", "free");
        let event = polar
            .verify_webhook(&signed(&polar, "e", now, &body), body.as_bytes(), now)
            .expect("verified");
        assert_eq!(event.change, None);
    }

    #[test]
    fn other_events_verify_and_change_nothing_and_forgeries_fail() {
        let polar = polar();
        let now = 1_795_000_000;
        let body = json!({ "type": "checkout.created", "data": {} }).to_string();
        let event = polar
            .verify_webhook(&signed(&polar, "e", now, &body), body.as_bytes(), now)
            .expect("verified");
        assert_eq!(
            (event.kind.as_str(), event.change, event.order),
            ("checkout.created", None, None)
        );
        let forged = subscription("subscription.active", "active", "prod-plus-year", "");
        assert!(matches!(
            polar.verify_webhook(&signed(&polar, "e", now, &body), forged.as_bytes(), now),
            Err(WebhookError::Rejected(webhooks::Rejection::Signature))
        ));
        let garbage = "not json";
        assert_eq!(
            polar.verify_webhook(&signed(&polar, "e", now, garbage), garbage.as_bytes(), now),
            Err(WebhookError::Malformed)
        );
    }
}
