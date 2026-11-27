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
pub mod terms;
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

/// A plan's recurring price, as the provider sells it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Price {
    /// The amount before tax, in the currency's smallest unit.
    pub amount: u64,
    /// The currency, as a lower-case ISO 4217 code.
    pub currency: String,
    /// `month` or `year`.
    pub interval: String,
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
    /// What the provider reports of its terms.
    pub terms: Terms,
}

/// A subscription's terms as a provider reports them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Terms {
    /// The start of its current period, in Unix seconds.
    pub period_start: Option<u64>,
    /// When it started, in Unix seconds.
    pub started_at: Option<u64>,
    /// Its price for one period before tax, in the currency's smallest
    /// unit.
    pub amount: Option<u64>,
    /// Its currency, as a lower-case ISO 4217 code.
    pub currency: Option<String>,
    /// `month` or `year`.
    pub interval: Option<String>,
    /// Whether it ends when its period ends.
    pub canceling: bool,
    /// The billing address's country, as an ISO 3166-1 alpha-2 code.
    pub country: Option<String>,
    /// The billing address's state or province, as its code alone, such
    /// as `QC`.
    pub region: Option<String>,
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
            period_start: self.terms.period_start,
            started_at: self.terms.started_at,
            amount: self.terms.amount,
            currency: self.terms.currency.clone(),
            interval: self.terms.interval.clone(),
            canceling: self.terms.canceling,
            country: self.terms.country.clone(),
            region: self.terms.region.clone(),
            ..AccountPlanRow::default()
        }
    }
}

/// A paid order of a subscription, as a provider's event reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaidOrder {
    /// The account the provider's customer names.
    pub account: Id,
    /// The plan ID its product maps to.
    pub plan: String,
    /// The provider's subscription ID.
    pub subscription: String,
    /// The provider's order ID.
    pub order_id: String,
    /// Whether it's the subscription's first order.
    pub first: bool,
    /// When it was paid, in Unix seconds.
    pub paid_at: u64,
    /// Its amount before tax, its tax and its total, in the currency's
    /// smallest unit.
    pub net: u64,
    pub tax: u64,
    pub total: u64,
    /// Its currency, as a lower-case ISO 4217 code.
    pub currency: String,
    /// The customer's name for the invoice, when given.
    pub name: Option<String>,
    /// The billing address, one line each, when given.
    pub address: Vec<String>,
    /// The end of the period it pays for, in Unix seconds.
    pub period_end: Option<u64>,
    /// The subscription's terms.
    pub terms: Terms,
}

impl PaidOrder {
    /// The row the store keeps for this order.
    pub fn row(&self) -> sync_store::OrderRow {
        sync_store::OrderRow {
            subscription: self.subscription.clone(),
            order_id: self.order_id.clone(),
            net: self.net,
            currency: self.currency.clone(),
            paid_at: self.paid_at,
        }
    }
}

/// A state or province's code alone, from an ISO 3166-2 code such as
/// `CA-QC` or the code itself, in upper case; Québec's name reads as `QC`.
pub fn region_code(state: &str) -> String {
    let state = state.trim();
    let folded = state.to_lowercase();
    if folded == "quebec" || folded == "québec" {
        return "QC".to_owned();
    }
    state
        .rsplit('-')
        .next()
        .unwrap_or(state)
        .to_ascii_uppercase()
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
    /// The paid order it reports, or `None`.
    pub order: Option<PaidOrder>,
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

    /// The recurring price of `plan`, or `None` when the provider sells it
    /// at no fixed price.
    fn price<'a>(&'a self, plan: &'a str) -> ProviderFuture<'a, Option<Price>>;

    /// A checkout link for `request`.
    fn checkout_url<'a>(&'a self, request: CheckoutRequest<'a>) -> ProviderFuture<'a, String>;

    /// A link that signs the account into the customer portal.
    fn portal_url<'a>(&'a self, request: PortalRequest<'a>) -> ProviderFuture<'a, String>;

    /// Schedules the subscription to end when its period ends. A
    /// subscription already set to end counts as done.
    fn cancel<'a>(&'a self, subscription: &'a str) -> ProviderFuture<'a, ()>;

    /// Ends the subscription now, so it never renews. A subscription that
    /// already ended counts as done.
    fn revoke<'a>(&'a self, subscription: &'a str) -> ProviderFuture<'a, ()>;

    /// Refunds `amount` of the order, before tax, in the currency's
    /// smallest unit; the provider refunds the tax in proportion.
    fn refund<'a>(&'a self, order: &'a str, amount: u64) -> ProviderFuture<'a, ()>;

    /// Checks a webhook delivery's signature at `now`, in Unix seconds, and
    /// reads its event.
    fn verify_webhook(
        &self,
        headers: &HeaderMap,
        body: &[u8],
        now: u64,
    ) -> Result<WebhookEvent, WebhookError>;
}

/// How long the 14-day refund and the withdrawal period last.
pub const REFUND_DAYS: u64 = 14;

/// How long after a price change takes effect a subscriber may still
/// refuse it by cancelling, with the unused part refunded.
pub const REFUSAL_DAYS: u64 = 30;

/// What a paying account may do with its plan (§26, Cancellation, Refunds
/// and withdrawal).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    /// It may cancel: its subscription grants a plan and isn't set to end.
    pub cancel: bool,
    /// Its cancellation may take effect today, with the unused part of
    /// the period refunded: in Québec, or while refusing a price change.
    pub end_today: bool,
    /// The refund of the unused part of the period, before tax.
    pub unused: u64,
    /// It may withdraw: in the EU or the UK, within 14 days of the start.
    pub withdraw: bool,
    /// It may get its latest payment back: within 14 days of it, and when
    /// it may not withdraw.
    pub refund: bool,
    /// The latest order, which a refund refunds.
    pub order: Option<sync_store::OrderRow>,
}

/// The unused part of `order`'s `net` amount at `now`, in proportion to
/// the time left of the period from `start` to `end`.
pub fn unused_part(net: u64, start: u64, end: u64, now: u64) -> u64 {
    if end <= start || now >= end {
        return 0;
    }
    let left = u128::from(end - now.max(start));
    let length = u128::from(end - start);
    u64::try_from(u128::from(net) * left / length).unwrap_or(net)
}

/// What an account whose subscription is `row`, whose latest order is
/// `order`, may do at `now`. `refusing` says whether a price change of its
/// plan is announced or took effect at most 30 days ago.
pub fn choices(
    row: &AccountPlanRow,
    order: Option<sync_store::OrderRow>,
    now: u64,
    refusing: bool,
) -> Choices {
    if !row.state.grants() {
        return Choices::default();
    }
    let window = REFUND_DAYS * sync_store::plans::DAY;
    let order = order.filter(|order| order.subscription == row.subscription);
    let started = row.started_at.or(order.as_ref().map(|order| order.paid_at));
    let withdraw = row.in_eu_or_uk()
        && order.is_some()
        && started.is_some_and(|started| now < started.saturating_add(window));
    let refund = !withdraw
        && order
            .as_ref()
            .is_some_and(|order| now < order.paid_at.saturating_add(window));
    let cancel = !row.canceling;
    let end_today = cancel && (row.in_quebec() || refusing);
    let unused = match (&order, row.period_end) {
        (Some(order), Some(end)) if end_today => unused_part(
            order.net,
            row.period_start.unwrap_or(order.paid_at),
            end,
            now,
        ),
        _ => 0,
    };
    Choices {
        cancel,
        end_today,
        unused,
        withdraw,
        refund,
        order,
    }
}

/// Pays what the server owes the provider for `owed`: the refund, then the
/// revocation, and records what's left, which the maintenance pass retries
/// (§26, Refunds and withdrawal). A refund the provider refuses as invalid
/// can't succeed on a retry, so the server logs it for the operator and
/// drops it.
pub async fn settle<B: sync_store::Backend>(
    store: &sync_store::Store<B>,
    provider: &dyn BillingProvider,
    owed: &sync_store::RefundOwed,
) -> Result<bool, sync_store::Error> {
    if owed.provider != provider.name() {
        return Ok(false);
    }
    let (mut amount, mut revoke) = (owed.amount, owed.revoke);
    if amount > 0 {
        match provider.refund(&owed.order_id, amount).await {
            Ok(()) => amount = 0,
            Err(ProviderError::Status(422)) => {
                tracing::error!("the payment provider refused a refund; refund the order by hand");
                amount = 0;
            }
            Err(error) => {
                tracing::warn!(cause = %error, "a refund failed; the next pass retries it")
            }
        }
    }
    if revoke {
        match provider.revoke(&owed.subscription).await {
            Ok(()) => revoke = false,
            Err(error) => {
                tracing::warn!(cause = %error, "a revocation failed; the next pass retries it");
            }
        }
    }
    if (amount, revoke) != (owed.amount, owed.revoke) {
        store.settle_refund(owed.id, amount, revoke).await?;
    }
    Ok(amount == 0 && !revoke)
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

    fn paying(country: &str, region: &str) -> AccountPlanRow {
        AccountPlanRow {
            subscription: "sub".into(),
            plan: "plus_year".into(),
            state: SubscriptionState::Active,
            period_start: Some(1_000),
            started_at: Some(1_000),
            period_end: Some(1_000 + 365 * DAY),
            country: Some(country.into()),
            region: Some(region.into()),
            ..AccountPlanRow::default()
        }
    }

    fn order(paid_at: u64) -> sync_store::OrderRow {
        sync_store::OrderRow {
            subscription: "sub".into(),
            order_id: "ord".into(),
            net: 9_600,
            currency: "usd".into(),
            paid_at,
        }
    }

    const DAY: u64 = sync_store::plans::DAY;

    #[test]
    fn the_unused_part_follows_the_time_left() {
        assert_eq!(unused_part(9_600, 0, 100, 0), 9_600);
        assert_eq!(unused_part(9_600, 0, 100, 25), 7_200);
        assert_eq!(unused_part(9_600, 0, 100, 100), 0);
        assert_eq!(unused_part(9_600, 100, 100, 50), 0);
        assert_eq!(unused_part(u64::MAX, 0, 3, 1), u64::MAX / 3 * 2);
    }

    #[test]
    fn each_place_gets_its_choices() {
        let half = 1_000 + 365 * DAY / 2;
        // Québec, after 14 days: end today with half refunded, or at the
        // period's end; no 14-day refund.
        let quebec = choices(&paying("CA", "QC"), Some(order(1_000)), half, false);
        assert!(quebec.cancel && quebec.end_today && !quebec.withdraw && !quebec.refund);
        assert_eq!(quebec.unused, 4_800);
        // Ontario in its first week: cancel at the period's end, or refund.
        let ontario = choices(&paying("CA", "ON"), Some(order(1_000)), 1_000 + DAY, false);
        assert!(ontario.cancel && !ontario.end_today && ontario.refund && !ontario.withdraw);
        assert_eq!(ontario.unused, 0);
        // Ontario refusing a price change may end today.
        assert!(choices(&paying("CA", "ON"), Some(order(1_000)), half, true).end_today);
        // France in its first week withdraws instead of the refund, and not
        // after 14 days.
        let france = choices(&paying("FR", ""), Some(order(1_000)), 1_000 + DAY, false);
        assert!(france.withdraw && !france.refund);
        let late = choices(
            &paying("FR", ""),
            Some(order(1_000)),
            1_000 + 15 * DAY,
            false,
        );
        assert!(!late.withdraw && !late.refund && late.cancel);
        // A renewal opens a new 14 days for the refund, not the withdrawal.
        let renewed = choices(
            &paying("FR", ""),
            Some(order(1_000 + 365 * DAY)),
            1_000 + 366 * DAY,
            false,
        );
        assert!(renewed.refund && !renewed.withdraw);
        // Another subscription's order refunds nothing, and a canceled or
        // ended plan offers no cancel.
        let other = sync_store::OrderRow {
            subscription: "old".into(),
            ..order(1_000)
        };
        assert!(!choices(&paying("CA", "ON"), Some(other), 1_000, false).refund);
        let canceling = AccountPlanRow {
            canceling: true,
            ..paying("CA", "QC")
        };
        assert!(!choices(&canceling, None, half, false).cancel);
        let ended = AccountPlanRow {
            state: SubscriptionState::Ended,
            ..paying("CA", "QC")
        };
        assert_eq!(
            choices(&ended, Some(order(1_000)), 1_000, false),
            Choices::default()
        );
    }

    #[test]
    fn regions_read_as_their_code() {
        for state in ["CA-QC", "QC", "qc", "Québec", "Quebec"] {
            assert_eq!(region_code(state), "QC", "{state}");
        }
        assert_eq!(region_code("US-CA"), "CA");
    }
}
