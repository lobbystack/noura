//! Selling plans (docs/workspace-format/sync-v2.md §20, §26 Billing): one
//! payment provider behind [`BillingProvider`], the routes that send a
//! signed-in account to its checkout and portal, and the webhook route that
//! turns the provider's events into stored subscriptions.
//!
//! Everything outside a provider's module speaks in the server's own terms:
//! plan IDs from the store's plan table, and [`SubscriptionChange`]s. A
//! provider maps its product IDs to plan IDs in its configuration, so no
//! edit in its dashboard changes a limit. The server reads the provider's
//! secrets only from its environment and never logs them, nor the checkout
//! and portal links it creates.

pub mod polar;
pub mod webhooks;

use std::future::Future;
use std::pin::Pin;

use axum::http::HeaderMap;
use sync_protocol::types::Id;
use sync_store::{AccountPlanRow, SubscriptionState};

/// Who sells the plans, as the account pages and the mail about plans
/// name them: the legal name, the street address, an email and a phone
/// number, and the terms in each language (§26, Billing).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Merchant {
    /// The legal name.
    pub name: String,
    /// The street address of the place of business.
    pub address: String,
    /// The email for support, billing and withdrawals.
    pub email: String,
    /// The phone number.
    pub phone: String,
    /// The terms in English.
    pub terms_en: String,
    /// The terms in French.
    pub terms_fr: String,
}

impl Merchant {
    /// The terms' address in `lang`.
    pub fn terms(&self, lang: crate::accounts::lang::Lang) -> &str {
        lang.pick(&self.terms_en, &self.terms_fr)
    }

    /// One line with every way to reach the merchant.
    pub fn contact(&self) -> String {
        format!(
            "{}, {} · {} · {}",
            self.name, self.address, self.email, self.phone
        )
    }

    /// A merchant for tests and examples.
    pub fn example() -> Self {
        Self {
            name: "Example Inc.".into(),
            address: "1 Example Street, Québec QC G1A 1A1, Canada".into(),
            email: "billing@example.com".into(),
            phone: "+1 418 555 0100".into(),
            terms_en: "https://example.com/terms/".into(),
            terms_fr: "https://example.com/fr/terms/".into(),
        }
    }
}

/// A boxed future a provider returns.
pub type ProviderFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, ProviderError>> + Send + 'a>>;

/// Why a provider call failed. The server logs only the kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum ProviderError {
    /// The provider couldn't be reached, or its answer didn't parse.
    #[error("the payment provider can't be reached")]
    Unavailable,
    /// The provider answered with this HTTP status.
    #[error("the payment provider answered with status {0}")]
    Status(u16),
    /// The provider holds no customer for the account.
    #[error("the payment provider holds no customer for the account")]
    NoCustomer,
}

/// What a checkout needs: whom it's for and what they buy.
#[derive(Clone, Debug)]
pub struct CheckoutRequest<'a> {
    /// The account, which the provider keeps as its external customer ID.
    pub account: Id,
    /// The plan IDs to offer, the chosen one first.
    pub plans: &'a [&'a str],
    /// The account's email, to fill in the form.
    pub email: Option<&'a str>,
    /// The checkout's language, as a BCP 47 tag, when not English.
    pub locale: Option<&'a str>,
    /// Where the provider sends the buyer after paying.
    pub success_url: &'a str,
    /// Where its back button leads.
    pub return_url: &'a str,
}

/// What a portal session needs.
#[derive(Clone, Debug)]
pub struct PortalRequest<'a> {
    /// The account.
    pub account: Id,
    /// The provider's customer ID, when the server stored one.
    pub customer: Option<&'a str>,
    /// Where the portal's back button leads.
    pub return_url: &'a str,
}

/// What happened to a subscription, in the server's own terms.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChangeKind {
    /// It started or came back: created, paid, uncanceled or resumed.
    Active,
    /// Something else changed, such as its plan or its period.
    Updated,
    /// A cancellation was scheduled; the plan holds until the period ends.
    Canceled,
    /// A payment failed and the provider retries it.
    PaymentFailed,
    /// It ended: revoked, unpaid after the retries, or paused.
    Ended,
}

/// A subscription as a provider's event reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SubscriptionChange {
    /// What happened.
    pub kind: ChangeKind,
    /// The account the provider's customer names.
    pub account: Id,
    /// The plan ID its product maps to.
    pub plan: String,
    /// The provider's customer ID.
    pub customer: String,
    /// The provider's subscription ID.
    pub subscription: String,
    /// Where it stands.
    pub state: SubscriptionState,
    /// The end of its current period, in Unix seconds.
    pub period_end: Option<u64>,
    /// When the provider last changed it, in Unix seconds.
    pub changed_at: u64,
}

impl SubscriptionChange {
    /// The row the store keeps for this change under `provider`.
    pub fn row(&self, provider: &str) -> AccountPlanRow {
        AccountPlanRow {
            provider: provider.to_owned(),
            customer: self.customer.clone(),
            subscription: self.subscription.clone(),
            plan: self.plan.clone(),
            state: self.state,
            period_end: self.period_end,
            changed_at: self.changed_at,
            ..AccountPlanRow::default()
        }
    }
}

/// A webhook delivery that passed its signature check.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebhookEvent {
    /// The provider's event ID, the same on every retry.
    pub id: String,
    /// The event's type, for the log.
    pub kind: String,
    /// The subscription it reports, or `None` for an event that changes no
    /// plan: another type, an unknown account or an unmapped product.
    pub change: Option<SubscriptionChange>,
}

/// Why a webhook delivery is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
pub enum WebhookError {
    /// The signature, its headers or its timestamp fail the check: 403.
    #[error(transparent)]
    Rejected(#[from] webhooks::Rejection),
    /// The body passed the check but doesn't parse: 400.
    #[error("the webhook body doesn't parse")]
    Malformed,
}

/// A payment provider: checkout, a customer portal and signed webhooks.
pub trait BillingProvider: Send + Sync {
    /// The provider's name, which the webhook route and the stored rows
    /// carry, such as `polar`.
    fn name(&self) -> &'static str;

    /// Whether the provider sells `plan`.
    fn sells(&self, plan: &str) -> bool;

    /// A checkout link for `request`.
    fn checkout_url<'a>(&'a self, request: CheckoutRequest<'a>) -> ProviderFuture<'a, String>;

    /// A link that signs the account into the customer portal.
    fn portal_url<'a>(&'a self, request: PortalRequest<'a>) -> ProviderFuture<'a, String>;

    /// Checks a webhook delivery's signature at `now`, in Unix seconds, and
    /// reads its event.
    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        now: u64,
    ) -> Result<WebhookEvent, WebhookError>;
}

/// The tiers a plan ID belongs to, so a checkout offers both intervals of
/// the chosen one.
pub fn sibling(plan: &str) -> Option<String> {
    if let Some(tier) = plan.strip_suffix("_year") {
        return Some(format!("{tier}_month"));
    }
    plan.strip_suffix("_month")
        .map(|tier| format!("{tier}_year"))
}

/// The plans a checkout for `plan` offers: `plan`, then its other interval
/// when the provider sells it.
pub fn offered(provider: &dyn BillingProvider, plan: &str) -> Vec<String> {
    let mut plans = vec![plan.to_owned()];
    if let Some(other) = sibling(plan).filter(|other| provider.sells(other)) {
        plans.push(other);
    }
    plans
}

/// The checkout language for an `Accept-Language` header: Canadian French
/// when the visitor prefers French, and the provider's English otherwise.
pub fn checkout_locale(accept_language: Option<&str>) -> Option<&'static str> {
    let first = accept_language?
        .split(',')
        .next()?
        .split(';')
        .next()?
        .trim()
        .to_ascii_lowercase();
    (first == "fr" || first.starts_with("fr-")).then_some("fr-CA")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checkout_offers_both_intervals_and_french_gets_canadian_french() {
        assert_eq!(sibling("plus_year").as_deref(), Some("plus_month"));
        assert_eq!(
            sibling("plus_100gb_month").as_deref(),
            Some("plus_100gb_year")
        );
        assert_eq!(sibling("free"), None);
        assert_eq!(checkout_locale(Some("fr-FR,fr;q=0.9")), Some("fr-CA"));
        assert_eq!(checkout_locale(Some("fr")), Some("fr-CA"));
        assert_eq!(checkout_locale(Some("en-CA,fr;q=0.8")), None);
        assert_eq!(checkout_locale(None), None);
    }
}
