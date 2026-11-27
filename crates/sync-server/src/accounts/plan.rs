//! The plan pages (§20, §26 Billing): the plan section of the account
//! page, `GET /account/upgrade`, which signs the visitor in and sends them
//! to the provider's checkout, `GET /account/billing`, which sends them to
//! the provider's portal, and `POST /billing/{provider}/webhook`.

use axum::body::Body;
use axum::extract::{Path, RawQuery, State};
use axum::http::header::ACCEPT_LANGUAGE;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use sync_protocol::messages::{AccountResponse, OwnedState, SubscriptionStatus};
use sync_protocol::types::Id;
use sync_store::Backend;

use super::{Accounts, cookie_value, failure, field, html, message, pages, redirect, signed_in};
use crate::billing::{BillingProvider, CheckoutRequest, PortalRequest, ProviderError};
use crate::http::{AppState, InternalCause};

/// The cookie that remembers which plan a signed-out visitor chose, until
/// they sign in and the account page sends them on.
pub const UPGRADE_COOKIE: &str = "noura_upgrade";

/// The largest webhook body the server reads.
const MAX_WEBHOOK_BODY: usize = 1024 * 1024;

/// The tiers the account page offers, each by its yearly and monthly plan.
const TIERS: &[(&str, &str, &str)] = &[
    ("standard_year", "standard_month", "Standard"),
    ("plus_year", "plus_month", "Plus"),
    ("plus_100gb_year", "plus_100gb_month", "Plus 100 GB"),
];

/// A plan's name on the account page.
pub fn plan_name(plan: &str) -> &'static str {
    match plan {
        "standard_year" => "Standard, yearly",
        "standard_month" => "Standard, monthly",
        "plus_year" => "Plus, yearly",
        "plus_month" => "Plus, monthly",
        "plus_100gb_year" => "Plus 100 GB, yearly",
        "plus_100gb_month" => "Plus 100 GB, monthly",
        _ => "Free",
    }
}

/// `bytes` in decimal units, as the plans are sold.
pub fn size(bytes: u64) -> String {
    for (unit, scale) in [("GB", 1e9), ("MB", 1e6), ("KB", 1e3)] {
        let value = bytes as f64 / scale;
        if value >= 1.0 {
            let text = format!("{value:.1}");
            let text = text.strip_suffix(".0").unwrap_or(&text);
            return format!("{text} {unit}");
        }
    }
    format!("{bytes} B")
}

/// The plan section's content for an account's report.
pub struct Section {
    pub plan_id: String,
    pub plan_name: &'static str,
    pub usage: String,
    pub past_due: bool,
    pub over_plan: String,
    pub offers: Vec<(&'static str, &'static str)>,
    pub manage: bool,
}

/// What the account page shows of `report` while `provider` sells plans.
pub fn section(provider: &dyn BillingProvider, report: &AccountResponse) -> Section {
    let used = report.used_bytes.saturating_add(report.history_bytes);
    let usage = match report.storage_bytes {
        Some(limit) => format!("{} of {} used", size(used), size(limit)),
        None => format!("{} used", size(used)),
    };
    let over = report
        .workspaces
        .0
        .iter()
        .filter(|w| w.state == OwnedState::OverPlan)
        .count();
    let over_plan = match over {
        0 => String::new(),
        1 => "1 workspace is read-only until you upgrade or choose which stay synced in Noura."
            .to_owned(),
        n => format!(
            "{n} workspaces are read-only until you upgrade or choose which stay synced in Noura."
        ),
    };
    let paid = matches!(
        report.status,
        SubscriptionStatus::Active | SubscriptionStatus::PastDue
    );
    let offers = if paid {
        Vec::new()
    } else {
        TIERS
            .iter()
            .filter_map(|(year, month, name)| {
                [*year, *month]
                    .into_iter()
                    .find(|plan| provider.sells(plan))
                    .map(|plan| (plan, *name))
            })
            .collect()
    };
    Section {
        plan_id: report.plan.as_str().to_owned(),
        plan_name: plan_name(report.plan.as_str()),
        usage,
        past_due: report.status == SubscriptionStatus::PastDue,
        over_plan,
        offers,
        manage: report.status != SubscriptionStatus::None,
    }
}

fn not_sold() -> Response {
    message(
        StatusCode::NOT_FOUND,
        "This plan isn't for sale here",
        "Check the link, or open your account page.",
    )
}

/// The page for a provider that failed, with the failure's kind for the
/// request log.
fn unavailable(what: &str, error: ProviderError) -> Response {
    let mut response = message(
        StatusCode::SERVICE_UNAVAILABLE,
        what,
        "Try again in a few minutes.",
    );
    response
        .extensions_mut()
        .insert(InternalCause(error.to_string()));
    response
}

fn upgrade_cookie(accounts: &Accounts, plan: Option<&str>) -> HeaderValue {
    let secure = if accounts.origin.is_https() {
        "; Secure"
    } else {
        ""
    };
    let value = match plan {
        Some(plan) => {
            format!(
                "{UPGRADE_COOKIE}={plan}; Path=/account; HttpOnly; SameSite=Lax; Max-Age=1800{secure}"
            )
        }
        None => {
            format!("{UPGRADE_COOKIE}=; Path=/account; HttpOnly; SameSite=Lax; Max-Age=0{secure}")
        }
    };
    HeaderValue::from_str(&value).expect("a plan ID is a header value")
}

/// The plan a signed-out visitor chose before they signed in, if any.
pub fn chosen_plan(headers: &HeaderMap) -> Option<String> {
    cookie_value(headers, UPGRADE_COOKIE).filter(|plan| sync_store::plans::is_paid(plan))
}

/// The account page's answer when the visitor chose a plan before signing
/// in: on to the checkout, with the choice forgotten.
pub fn resume_upgrade(accounts: &Accounts, plan: &str) -> Response {
    let mut response = redirect(&format!("/account/upgrade?plan={plan}"));
    response.headers_mut().append(
        axum::http::header::SET_COOKIE,
        upgrade_cookie(accounts, None),
    );
    response
}

/// `GET /account/upgrade?plan={plan}`: signs the visitor in first, then
/// sends them to the provider's checkout for the plan, or to the portal
/// when the account already pays.
pub async fn upgrade<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let accounts = &state.accounts;
    let Some(provider) = accounts.billing() else {
        return not_sold();
    };
    let query = query.unwrap_or_default();
    let Some(plan) = field(query.as_bytes(), "plan").filter(|plan| provider.sells(plan)) else {
        return not_sold();
    };
    let account = match signed_in(&state.store, &headers).await {
        Ok(Some((account, _))) => account,
        Ok(None) if field(query.as_bytes(), "continue").is_none() => {
            // A link from another site carries no strict cookie, so this page
            // asks again from the server's own origin.
            return html(
                StatusCode::OK,
                &pages::Upgrade {
                    plan: &plan,
                    name: plan_name(&plan),
                },
            );
        }
        Ok(None) => {
            let mut response = redirect("/account/sign-in");
            response.headers_mut().append(
                axum::http::header::SET_COOKIE,
                upgrade_cookie(accounts, Some(&plan)),
            );
            return response;
        }
        Err(error) => return failure(error),
    };
    match state.store.account_plan(account).await {
        Ok(Some(row)) if row.state.grants() => return redirect("/account/billing"),
        Ok(_) => {}
        Err(error) => return failure(error),
    }
    let email = match state.store.account_email(account).await {
        Ok(email) => email,
        Err(error) => return failure(error),
    };
    let offered = crate::billing::offered(provider, &plan);
    let plans: Vec<&str> = offered.iter().map(String::as_str).collect();
    let back = format!("{}/account", accounts.origin.as_str());
    let locale = crate::billing::checkout_locale(
        headers
            .get(ACCEPT_LANGUAGE)
            .and_then(|value| value.to_str().ok()),
    );
    let request = CheckoutRequest {
        account,
        plans: &plans,
        email: email.as_deref(),
        locale,
        success_url: &back,
        return_url: &back,
    };
    match provider.checkout_url(request).await {
        Ok(url) => redirect(&url),
        Err(error) => unavailable("Checkout isn't available right now", error),
    }
}

/// `GET /account/billing`: sends the signed-in account to the provider's
/// customer portal, where it changes its plan, its payment method and its
/// invoices.
pub async fn portal<B: Backend>(State(state): State<AppState<B>>, headers: HeaderMap) -> Response {
    let accounts = &state.accounts;
    let Some(provider) = accounts.billing() else {
        return redirect("/account");
    };
    let account = match signed_in(&state.store, &headers).await {
        Ok(Some((account, _))) => account,
        Ok(None) => return redirect("/account/sign-in"),
        Err(error) => return failure(error),
    };
    let row = match state.store.account_plan(account).await {
        Ok(row) => row,
        Err(error) => return failure(error),
    };
    let customer = row
        .as_ref()
        .filter(|row| row.provider == provider.name())
        .map(|row| row.customer.as_str());
    let back = format!("{}/account", accounts.origin.as_str());
    let request = PortalRequest {
        account,
        customer,
        return_url: &back,
    };
    match provider.portal_url(request).await {
        Ok(url) => redirect(&url),
        Err(ProviderError::NoCustomer) => redirect("/account"),
        Err(error) => unavailable("Billing isn't available right now", error),
    }
}

/// `POST /billing/{provider}/webhook`: checks a delivery's signature and
/// applies its event once. A forged or stale delivery gets 403, and one
/// the store couldn't apply gets 500, so the provider retries it.
pub async fn webhook<B: Backend>(
    State(state): State<AppState<B>>,
    Path(name): Path<String>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let Some(provider) = state.accounts.billing().filter(|p| p.name() == name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let Ok(body) = axum::body::to_bytes(body, MAX_WEBHOOK_BODY).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let now = state.store.clock().now();
    let event = match provider.verify_webhook(&headers, &body, now) {
        Ok(event) => event,
        Err(crate::billing::WebhookError::Rejected(rejection)) => {
            tracing::warn!(cause = %rejection, "a billing webhook failed its check");
            return StatusCode::FORBIDDEN.into_response();
        }
        Err(crate::billing::WebhookError::Malformed) => {
            return StatusCode::BAD_REQUEST.into_response();
        }
    };
    let account: Option<Id> = event.change.as_ref().map(|change| change.account);
    let change = event
        .change
        .map(|change| (change.account, change.row(provider.name())));
    match state
        .store
        .apply_billing_event(provider.name(), event.id, change)
        .await
    {
        Ok(applied) => {
            tracing::info!(event = %event.kind, applied, "billing event");
            if let Some(account) = account {
                state.rate.forget_plan(account);
            }
            StatusCode::ACCEPTED.into_response()
        }
        Err(error) => {
            let mut response = StatusCode::INTERNAL_SERVER_ERROR.into_response();
            response
                .extensions_mut()
                .insert(InternalCause(error.to_string()));
            response
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_decimal_units() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(999), "999 B");
        assert_eq!(size(12_345_678), "12.3 MB");
        assert_eq!(size(100_000_000), "100 MB");
        assert_eq!(size(1_000_000_000), "1 GB");
        assert_eq!(size(10_500_000_000), "10.5 GB");
    }
}
