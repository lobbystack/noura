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
use sync_store::{AccountPlanRow, Backend, BillingMail, EarlyEnd};

use super::lang::{Lang, negotiate};
use super::{
    Accounts, cookie_value, cross_origin, failure, field, html, message, pages, read_body,
    redirect, same_origin, signed_in,
};
use crate::billing::terms::{self, Context, EndReason, MailLang};
use crate::billing::{
    BillingProvider, CheckoutRequest, Choices, PaidOrder, PortalRequest, ProviderError,
    REFUSAL_DAYS,
};
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

/// A plan's name on the account pages and in mail.
pub fn plan_name(plan: &str, lang: Lang) -> &'static str {
    match (plan, lang) {
        ("standard_year", Lang::En) => "Standard, yearly",
        ("standard_year", Lang::Fr) => "Standard, annuel",
        ("standard_month", Lang::En) => "Standard, monthly",
        ("standard_month", Lang::Fr) => "Standard, mensuel",
        ("plus_year", Lang::En) => "Plus, yearly",
        ("plus_year", Lang::Fr) => "Plus, annuel",
        ("plus_month", Lang::En) => "Plus, monthly",
        ("plus_month", Lang::Fr) => "Plus, mensuel",
        ("plus_100gb_year", Lang::En) => "Plus 100 GB, yearly",
        ("plus_100gb_year", Lang::Fr) => "Plus 100 Go, annuel",
        ("plus_100gb_month", Lang::En) => "Plus 100 GB, monthly",
        ("plus_100gb_month", Lang::Fr) => "Plus 100 Go, mensuel",
        (_, Lang::En) => "Free",
        (_, Lang::Fr) => "Gratuit",
    }
}

/// `bytes` in decimal units, as the plans are sold, in `lang`'s units and
/// decimal mark.
pub fn size(bytes: u64, lang: Lang) -> String {
    let units: [(&str, f64); 3] = match lang {
        Lang::En => [("GB", 1e9), ("MB", 1e6), ("KB", 1e3)],
        Lang::Fr => [("Go", 1e9), ("Mo", 1e6), ("ko", 1e3)],
    };
    for (unit, scale) in units {
        let value = bytes as f64 / scale;
        if value >= 1.0 {
            let text = format!("{value:.1}");
            let text = text.strip_suffix(".0").unwrap_or(&text);
            let text = match lang {
                Lang::En => text.to_owned(),
                Lang::Fr => text.replace('.', ","),
            };
            return format!("{text} {unit}");
        }
    }
    match lang {
        Lang::En => format!("{bytes} B"),
        Lang::Fr => format!("{bytes} o"),
    }
}

/// The plan section's content for an account's report.
pub struct Section {
    pub plan_id: String,
    pub plan_name: &'static str,
    pub usage: String,
    pub notice: String,
    pub over_plan: String,
    pub offers: Vec<(&'static str, String)>,
    pub manage: bool,
    pub cancel: bool,
    pub withdraw: bool,
    pub refund: bool,
}

/// What the account page shows of `report` while `provider` sells plans,
/// in `lang`. `choices` says which of cancel, refund and withdraw apply,
/// and `ends` gives the day a plan set to end ends.
pub fn section(
    provider: &dyn BillingProvider,
    report: &AccountResponse,
    lang: Lang,
    choices: &Choices,
    ends: Option<u64>,
) -> Section {
    let used = report.used_bytes.saturating_add(report.history_bytes);
    let usage = match (report.storage_bytes, lang) {
        (Some(limit), Lang::En) => format!("{} of {} used", size(used, lang), size(limit, lang)),
        (Some(limit), Lang::Fr) => {
            format!("{} utilisés sur {}", size(used, lang), size(limit, lang))
        }
        (None, Lang::En) => format!("{} used", size(used, lang)),
        (None, Lang::Fr) => format!("{} utilisés", size(used, lang)),
    };
    let over = report
        .workspaces
        .0
        .iter()
        .filter(|w| w.state == OwnedState::OverPlan)
        .count();
    let over_plan = match (over, lang) {
        (0, _) => String::new(),
        (1, Lang::En) => {
            "1 workspace is read-only until you upgrade or choose which stay synced in Noura."
                .to_owned()
        }
        (1, Lang::Fr) => "1 espace de travail est en lecture seule jusqu\u{2019}à ce que vous changiez de forfait ou choisissiez dans Noura ceux qui restent synchronisés.".to_owned(),
        (n, Lang::En) => format!(
            "{n} workspaces are read-only until you upgrade or choose which stay synced in Noura."
        ),
        (n, Lang::Fr) => format!(
            "{n} espaces de travail sont en lecture seule jusqu\u{2019}à ce que vous changiez de forfait ou choisissiez dans Noura ceux qui restent synchronisés."
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
                    .map(|plan| {
                        let name = match lang {
                            Lang::En => (*name).to_owned(),
                            Lang::Fr => name.replace("GB", "Go"),
                        };
                        let label = match lang {
                            Lang::En => format!("Upgrade to {name}"),
                            Lang::Fr => format!("Passer à {name}"),
                        };
                        (plan, label)
                    })
            })
            .collect()
    };
    let notice = if report.status == SubscriptionStatus::PastDue {
        lang.pick(
            "Your last payment failed. Update your payment method in Manage billing.",
            "Votre dernier paiement a échoué. Mettez à jour votre moyen de paiement dans Gérer la facturation.",
        )
        .to_owned()
    } else if let Some(ends) = ends.filter(|_| paid) {
        match lang {
            Lang::En => format!("Your plan ends on {}.", terms::date(ends, lang)),
            Lang::Fr => format!("Votre forfait prend fin le {}.", terms::date(ends, lang)),
        }
    } else {
        String::new()
    };
    Section {
        plan_id: report.plan.as_str().to_owned(),
        plan_name: plan_name(report.plan.as_str(), lang),
        usage,
        notice,
        over_plan,
        offers,
        manage: report.status != SubscriptionStatus::None,
        cancel: paid && choices.cancel,
        withdraw: paid && choices.withdraw,
        refund: paid && choices.refund,
    }
}

fn not_sold(lang: Lang) -> Response {
    message(
        StatusCode::NOT_FOUND,
        lang.pick(
            "This plan isn't for sale here",
            "Ce forfait n\u{2019}est pas offert ici",
        ),
        lang.pick(
            "Check the link, or open your account page.",
            "Vérifiez le lien, ou ouvrez la page de votre compte.",
        ),
    )
}

/// The page for a provider that failed, with the failure's kind for the
/// request log.
fn unavailable(lang: Lang, what: (&str, &str), error: ProviderError) -> Response {
    let mut response = message(
        StatusCode::SERVICE_UNAVAILABLE,
        lang.pick(what.0, what.1),
        lang.pick(
            "Try again in a few minutes.",
            "Réessayez dans quelques minutes.",
        ),
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
/// shows the plan's prices and renewal terms and asks for consent (§26,
/// Consent); an account that already pays goes to the portal instead.
pub async fn upgrade<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    RawQuery(query): RawQuery,
) -> Response {
    let accounts = &state.accounts;
    let lang = negotiate(&headers);
    let Some(provider) = accounts.billing() else {
        return not_sold(lang);
    };
    let query = query.unwrap_or_default();
    let Some(plan) = field(query.as_bytes(), "plan").filter(|plan| provider.sells(plan)) else {
        return not_sold(lang);
    };
    let account = match signed_in(&state.store, &headers).await {
        Ok(Some((account, _))) => account,
        Ok(None) if field(query.as_bytes(), "continue").is_none() => {
            // A link from another site carries no strict cookie, so this page
            // asks again from the server's own origin.
            return html(
                StatusCode::OK,
                &pages::Upgrade {
                    lang: lang.code(),
                    plan: &plan,
                    heading: &upgrade_heading(&plan, lang),
                    button: lang.pick("Continue", "Continuer"),
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
        Ok(Some(row)) if row.state.grants() => redirect("/account/billing"),
        Ok(_) => consent_page(accounts, provider, &plan, lang, "").await,
        Err(error) => failure(error),
    }
}

fn upgrade_heading(plan: &str, lang: Lang) -> String {
    match lang {
        Lang::En => format!("Upgrade to {}", plan_name(plan, lang)),
        Lang::Fr => format!("Passer au forfait {}", plan_name(plan, lang)),
    }
}

/// The consent page for `plan`, with `error` above the form when not
/// empty.
async fn consent_page(
    accounts: &Accounts,
    provider: &dyn BillingProvider,
    plan: &str,
    lang: Lang,
    error: &str,
) -> Response {
    let mut prices = Vec::new();
    for offered in crate::billing::offered(provider, plan) {
        let Ok(Some(price)) = provider.price(&offered).await else {
            continue;
        };
        let amount = terms::money(price.amount, &price.currency, lang);
        let line = match (price.interval.as_str(), lang) {
            ("year", Lang::En) => {
                format!("{}: {amount} a year, plus tax", plan_name(&offered, lang))
            }
            ("year", Lang::Fr) => {
                format!(
                    "{} : {amount} par année, plus taxes",
                    plan_name(&offered, lang)
                )
            }
            (_, Lang::En) => format!("{}: {amount} a month, plus tax", plan_name(&offered, lang)),
            (_, Lang::Fr) => {
                format!(
                    "{} : {amount} par mois, plus taxes",
                    plan_name(&offered, lang)
                )
            }
        };
        prices.push(line);
    }
    let email = accounts
        .merchant()
        .map(|m| m.email.as_str())
        .unwrap_or_default();
    let renewal = match lang {
        Lang::En => format!(
            "Your plan renews automatically at the price in effect until you cancel. You can cancel at any time from your account page or by emailing {email}."
        ),
        Lang::Fr => format!(
            "Votre forfait se renouvelle automatiquement au prix en vigueur jusqu\u{2019}à ce que vous le résiliiez. Vous pouvez le résilier en tout temps depuis la page de votre compte ou en écrivant à {email}."
        ),
    };
    let status = if error.is_empty() {
        StatusCode::OK
    } else {
        StatusCode::BAD_REQUEST
    };
    html(
        status,
        &pages::Consent {
            lang: lang.code(),
            plan,
            heading: &upgrade_heading(plan, lang),
            prices: &prices,
            renewal: &renewal,
            error,
            renewal_box: lang.pick(
                "I agree that my plan renews automatically until I cancel.",
                "J\u{2019}accepte que mon forfait se renouvelle automatiquement jusqu\u{2019}à ce que je le résilie.",
            ),
            immediate_box: lang.pick(
                "I ask Noura to start my sync subscription right away, during the 14-day withdrawal period. I understand that if I withdraw within 14 days, I get a full refund.",
                "Je demande à Noura de commencer mon abonnement de synchronisation immédiatement, pendant le délai de rétractation de 14 jours. Je comprends que si je me rétracte dans les 14 jours, je serai remboursé en entier.",
            ),
            button: lang.pick("Continue to checkout", "Continuer vers le paiement"),
            switch_to: lang.other().code(),
            switch_label: lang.switch_label(),
        },
    )
}

/// `POST /account/upgrade`: records the account's consent to the plan's
/// renewal and immediate start, then sends it to the provider's checkout
/// for the plan (§26, Consent).
pub async fn checkout<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    let accounts = &state.accounts;
    let lang = negotiate(&headers);
    let Some(provider) = accounts.billing() else {
        return not_sold(lang);
    };
    if !same_origin(accounts, &headers) {
        return cross_origin();
    }
    let account = match signed_in(&state.store, &headers).await {
        Ok(Some((account, _))) => account,
        Ok(None) => return redirect("/account/sign-in"),
        Err(error) => return failure(error),
    };
    let Some(form) = read_body(body).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let Some(plan) = field(&form, "plan").filter(|plan| provider.sells(plan)) else {
        return not_sold(lang);
    };
    let checked = |name| field(&form, name).as_deref() == Some("yes");
    if !(checked("renewal") && checked("immediate")) {
        let error = lang.pick(
            "Check both boxes to continue.",
            "Cochez les deux cases pour continuer.",
        );
        return consent_page(accounts, provider, &plan, lang, error).await;
    }
    match state.store.account_plan(account).await {
        Ok(Some(row)) if row.state.grants() => return redirect("/account/billing"),
        Ok(_) => {}
        Err(error) => return failure(error),
    }
    let consent = sync_store::ConsentRow {
        accepted_at: state.store.clock().now(),
        plan: plan.clone(),
        version: terms::CONSENT_VERSION.to_owned(),
        immediate: true,
        renewal: true,
    };
    if let Err(error) = state.store.record_consent(account, consent).await {
        return failure(error);
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
    )
    .or((lang == Lang::Fr).then_some("fr-CA"));
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
        Err(error) => unavailable(
            lang,
            (
                "Checkout isn't available right now",
                "Le paiement n\u{2019}est pas offert pour le moment",
            ),
            error,
        ),
    }
}

/// `GET /account/billing`: sends the signed-in account to the provider's
/// customer portal, where it changes its plan, its payment method and its
/// invoices.
pub async fn portal<B: Backend>(State(state): State<AppState<B>>, headers: HeaderMap) -> Response {
    let accounts = &state.accounts;
    let lang = negotiate(&headers);
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
        Err(error) => unavailable(
            lang,
            (
                "Billing isn't available right now",
                "La facturation n\u{2019}est pas offerte pour le moment",
            ),
            error,
        ),
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
    let mail = match event.order.as_ref().filter(|order| order.first) {
        Some(order) => match contract_mail(&state, order).await {
            Ok(mail) => mail.into_iter().collect(),
            Err(error) => return failure(error),
        },
        None => Vec::new(),
    };
    let change = event
        .change
        .map(|change| (change.account, change.row(provider.name())));
    let order = event.order.map(|order| (order.account, order.row()));
    match state
        .store
        .apply_billing_event(sync_store::BillingEvent {
            provider: provider.name().to_owned(),
            id: event.id,
            change,
            order,
            mail,
        })
        .await
    {
        Ok(applied) => {
            tracing::info!(event = %event.kind, applied, "billing event");
            if let Some(account) = account {
                state.rate.forget_plan(account);
            }
            state.accounts.send_billing_mail_soon(&state.store);
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

/// Whether a price change of `plan` is announced, or took effect at most
/// [`REFUSAL_DAYS`] days before `today`, so a subscriber may refuse it.
pub(crate) async fn refusing<B: Backend>(
    store: &sync_store::Store<B>,
    plan: &str,
    today: u64,
) -> sync_store::Result<bool> {
    Ok(store
        .price_changes()
        .await?
        .iter()
        .any(|change| change.plan == plan && today <= change.effective_day + REFUSAL_DAYS))
}

/// What a paying account may do now, with its subscription.
pub(crate) async fn account_choices<B: Backend>(
    store: &sync_store::Store<B>,
    account: Id,
) -> sync_store::Result<Option<(AccountPlanRow, Choices)>> {
    let Some(row) = store.account_plan(account).await? else {
        return Ok(None);
    };
    let now = store.clock().now();
    let order = store.latest_order(account).await?;
    let refusal = refusing(store, &row.plan, now / sync_store::plans::DAY).await?;
    let choices = crate::billing::choices(&row, order, now, refusal);
    Ok(Some((row, choices)))
}

/// A signed-in account with a subscription that grants a plan, on a server
/// that sells plans, as the cancel, refund and withdraw pages need it.
struct Paying<'a> {
    provider: &'a dyn BillingProvider,
    context: Context<'a>,
    account: Id,
    email: String,
    row: AccountPlanRow,
    choices: Choices,
    lang: Lang,
}

/// The paying account a request comes from, or the page to answer with.
async fn paying<'a, B: Backend>(
    state: &'a AppState<B>,
    headers: &HeaderMap,
) -> Result<Paying<'a>, Box<Response>> {
    let lang = negotiate(headers);
    let accounts = &state.accounts;
    let (Some(provider), Some(context)) = (accounts.billing(), mail_context(accounts)) else {
        return Err(Box::new(redirect("/account")));
    };
    let account = match signed_in(&state.store, headers).await {
        Ok(Some((account, _))) => account,
        Ok(None) => return Err(Box::new(redirect("/account/sign-in"))),
        Err(error) => return Err(Box::new(failure(error))),
    };
    let email = match state.store.account_email(account).await {
        Ok(Some(email)) => email,
        Ok(None) => return Err(Box::new(redirect("/account/sign-in"))),
        Err(error) => return Err(Box::new(failure(error))),
    };
    match account_choices(&state.store, account).await {
        Ok(Some((row, choices))) if row.state.grants() && row.provider == provider.name() => {
            Ok(Paying {
                provider,
                context,
                account,
                email,
                row,
                choices,
                lang,
            })
        }
        Ok(_) => Err(Box::new(redirect("/account"))),
        Err(error) => Err(Box::new(failure(error))),
    }
}

/// Which page asks for a change to the plan.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Change {
    Cancel,
    Refund,
    Withdraw,
}

impl Change {
    fn action(self) -> &'static str {
        match self {
            Self::Cancel => "/account/cancel",
            Self::Refund => "/account/refund",
            Self::Withdraw => "/account/withdraw",
        }
    }

    fn allowed(self, choices: &Choices) -> bool {
        match self {
            Self::Cancel => choices.cancel,
            Self::Refund => choices.refund,
            Self::Withdraw => choices.withdraw,
        }
    }
}

/// The question before `change`, with a button for each way it can go.
fn confirm_page(paying: &Paying<'_>, change: Change) -> Response {
    let lang = paying.lang;
    let row = &paying.row;
    let choices = &paying.choices;
    let ends = row
        .period_end
        .map(|end| terms::date(end, lang))
        .unwrap_or_default();
    let currency = choices
        .order
        .as_ref()
        .map_or("usd", |order| order.currency.as_str());
    let money = |amount| terms::money(amount, currency, lang);
    let net = choices.order.as_ref().map_or(0, |order| order.net);
    let (heading, lines, buttons): (&str, Vec<String>, Vec<(&str, String)>) = match (change, lang) {
        (Change::Cancel, Lang::En) if choices.end_today => (
            "Cancel your plan",
            vec![format!(
                "End it today and get {} back, plus tax, for the part of the period you haven't used. Or keep it until {ends}.",
                money(choices.unused)
            )],
            vec![
                ("today", "End today".to_owned()),
                ("end", format!("End on {ends}")),
            ],
        ),
        (Change::Cancel, Lang::Fr) if choices.end_today => (
            "Résilier votre forfait",
            vec![format!(
                "Mettez-y fin aujourd\u{2019}hui et récupérez {}, plus taxes, pour la partie de la période que vous n\u{2019}avez pas utilisée. Ou gardez-le jusqu\u{2019}au {ends}.",
                money(choices.unused)
            )],
            vec![
                ("today", "Mettre fin aujourd\u{2019}hui".to_owned()),
                ("end", format!("Mettre fin le {ends}")),
            ],
        ),
        (Change::Cancel, Lang::En) => (
            "Cancel your plan",
            vec![format!(
                "Your plan stays active until {ends}. Your account then moves to the Free plan, and you won't be charged again."
            )],
            vec![("end", "Cancel plan".to_owned())],
        ),
        (Change::Cancel, Lang::Fr) => (
            "Résilier votre forfait",
            vec![format!(
                "Votre forfait reste actif jusqu\u{2019}au {ends}. Votre compte passe ensuite au forfait Gratuit, et vous ne serez plus facturé."
            )],
            vec![("end", "Résilier le forfait".to_owned())],
        ),
        (Change::Refund, Lang::En) => (
            "Refund last payment",
            vec![format!(
                "You get {} back, plus tax. The period it paid for ends today, and your account moves to the Free plan.",
                money(net)
            )],
            vec![("today", "Refund payment".to_owned())],
        ),
        (Change::Refund, Lang::Fr) => (
            "Rembourser le dernier paiement",
            vec![format!(
                "Vous récupérez {}, plus taxes. La période qu\u{2019}il couvrait prend fin aujourd\u{2019}hui, et votre compte passe au forfait Gratuit.",
                money(net)
            )],
            vec![("today", "Rembourser le paiement".to_owned())],
        ),
        (Change::Withdraw, Lang::En) => (
            "Withdraw from contract",
            vec![format!(
                "You get back everything you paid, {} plus tax, and your account moves to the Free plan today.",
                money(net)
            )],
            vec![("today", "Confirm withdrawal".to_owned())],
        ),
        (Change::Withdraw, Lang::Fr) => (
            "Se rétracter du contrat",
            vec![format!(
                "Vous récupérez tout ce que vous avez payé, {} plus taxes, et votre compte passe au forfait Gratuit aujourd\u{2019}hui.",
                money(net)
            )],
            vec![("today", "Confirmer la rétractation".to_owned())],
        ),
    };
    html(
        StatusCode::OK,
        &pages::Confirm {
            lang: lang.code(),
            heading,
            lines: &lines,
            action: change.action(),
            choices: &buttons,
            back: lang.pick("Back to your account", "Retour à votre compte"),
            switch_to: lang.other().code(),
            switch_label: lang.switch_label(),
        },
    )
}

/// The page that says what a change to the plan did.
fn notice(lang: Lang, heading: &str, text: &str) -> Response {
    html(
        StatusCode::OK,
        &pages::Notice {
            lang: lang.code(),
            heading,
            text,
            back: lang.pick("Back to your account", "Retour à votre compte"),
        },
    )
}

/// `GET /account/cancel`, `/account/refund` and `/account/withdraw`: the
/// question before the change, for an account it applies to.
async fn ask<B: Backend>(state: &AppState<B>, headers: &HeaderMap, change: Change) -> Response {
    match paying(state, headers).await {
        Ok(paying) if change.allowed(&paying.choices) => confirm_page(&paying, change),
        Ok(_) => redirect("/account"),
        Err(response) => *response,
    }
}

pub async fn cancel_page<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
) -> Response {
    ask(&state, &headers, Change::Cancel).await
}

pub async fn refund_page<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
) -> Response {
    ask(&state, &headers, Change::Refund).await
}

pub async fn withdraw_page<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
) -> Response {
    ask(&state, &headers, Change::Withdraw).await
}

/// `POST /account/cancel`, `/account/refund` and `/account/withdraw`: the
/// change itself (§26, Cancellation, Refunds and withdrawal).
async fn decide<B: Backend>(
    state: &AppState<B>,
    headers: &HeaderMap,
    body: Body,
    change: Change,
) -> Response {
    if !same_origin(&state.accounts, headers) {
        return cross_origin();
    }
    let paying = match paying(state, headers).await {
        Ok(paying) if change.allowed(&paying.choices) => paying,
        Ok(_) => return redirect("/account"),
        Err(response) => return *response,
    };
    let Some(form) = read_body(body).await else {
        return StatusCode::PAYLOAD_TOO_LARGE.into_response();
    };
    let today = field(&form, "when").as_deref() == Some("today");
    let lang = paying.lang;
    let langs = match mail_langs(&state.store, paying.account, paying.row.in_quebec()).await {
        Ok(langs) => langs,
        Err(error) => return failure(error),
    };
    if change == Change::Cancel && !(today && paying.choices.end_today) {
        return cancel_at_period_end(state, &paying, langs).await;
    }
    let (reason, amount) = match change {
        Change::Cancel => (EndReason::Canceled, paying.choices.unused),
        Change::Refund => (
            EndReason::Refunded,
            paying.choices.order.as_ref().map_or(0, |o| o.net),
        ),
        Change::Withdraw => (
            EndReason::Withdrawn,
            paying.choices.order.as_ref().map_or(0, |o| o.net),
        ),
    };
    let order = paying.choices.order.as_ref();
    let currency = order.map_or("usd", |order| order.currency.as_str());
    let now = state.store.clock().now();
    let mail = terms::ended_today(
        &paying.context,
        langs,
        &paying.email,
        &paying.row,
        terms::Ending {
            reason,
            refund: amount,
            currency,
            today: now,
        },
    );
    let end = EarlyEnd {
        subscription: paying.row.subscription.clone(),
        refund: order
            .filter(|_| amount > 0)
            .map(|order| (order.order_id.clone(), amount)),
        mail: vec![mail],
    };
    let owed = match state.store.end_paid_period(paying.account, end).await {
        Ok(Some(owed)) => owed,
        Ok(None) => return redirect("/account"),
        Err(error) => return failure(error),
    };
    state.rate.forget_plan(paying.account);
    if let Err(error) = crate::billing::settle(&state.store, paying.provider, &owed).await {
        tracing::warn!(cause = %error, "a refund owed couldn't be recorded; the next pass retries it");
    }
    state.accounts.send_billing_mail_soon(&state.store);
    let money = terms::money(amount, currency, lang);
    let (heading, text) = match (reason, lang) {
        (EndReason::Canceled, Lang::En) => (
            "Your plan is cancelled",
            format!(
                "Your account is on the Free plan. We're refunding {money}, plus tax, and we emailed you a confirmation."
            ),
        ),
        (EndReason::Canceled, Lang::Fr) => (
            "Votre forfait est résilié",
            format!(
                "Votre compte est au forfait Gratuit. Nous vous remboursons {money}, plus taxes, et nous vous avons envoyé une confirmation par courriel."
            ),
        ),
        (EndReason::Refunded, Lang::En) => (
            "Your payment is refunded",
            format!(
                "Your account is on the Free plan. We're refunding {money}, plus tax, and we emailed you a confirmation."
            ),
        ),
        (EndReason::Refunded, Lang::Fr) => (
            "Votre paiement est remboursé",
            format!(
                "Votre compte est au forfait Gratuit. Nous vous remboursons {money}, plus taxes, et nous vous avons envoyé une confirmation par courriel."
            ),
        ),
        (EndReason::Withdrawn, Lang::En) => (
            "You withdrew from the contract",
            format!(
                "Your account is on the Free plan. We're refunding {money}, plus tax, and we emailed you an acknowledgment."
            ),
        ),
        (EndReason::Withdrawn, Lang::Fr) => (
            "Vous vous êtes rétracté du contrat",
            format!(
                "Votre compte est au forfait Gratuit. Nous vous remboursons {money}, plus taxes, et nous vous avons envoyé un accusé de réception par courriel."
            ),
        ),
    };
    notice(lang, heading, &text)
}

/// Schedules the subscription to end with its period, and confirms it.
async fn cancel_at_period_end<B: Backend>(
    state: &AppState<B>,
    paying: &Paying<'_>,
    langs: MailLang,
) -> Response {
    let lang = paying.lang;
    if let Err(error) = paying.provider.cancel(&paying.row.subscription).await {
        return unavailable(
            lang,
            (
                "Cancelling isn't available right now",
                "La résiliation n\u{2019}est pas offerte pour le moment",
            ),
            error,
        );
    }
    let mail = terms::canceled_at_period_end(&paying.context, langs, &paying.email, &paying.row);
    match state
        .store
        .mark_canceling(paying.account, paying.row.subscription.clone(), vec![mail])
        .await
    {
        Ok(_) => {}
        Err(error) => return failure(error),
    }
    state.accounts.send_billing_mail_soon(&state.store);
    let ends = paying
        .row
        .period_end
        .map(|end| terms::date(end, lang))
        .unwrap_or_default();
    match lang {
        Lang::En => notice(
            lang,
            "Your plan is cancelled",
            &format!("It stays active until {ends}, and we emailed you a confirmation."),
        ),
        Lang::Fr => notice(
            lang,
            "Votre forfait est résilié",
            &format!(
                "Il reste actif jusqu\u{2019}au {ends}, et nous vous avons envoyé une confirmation par courriel."
            ),
        ),
    }
}

pub async fn cancel<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    decide(&state, &headers, body, Change::Cancel).await
}

pub async fn refund<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    decide(&state, &headers, body, Change::Refund).await
}

pub async fn withdraw<B: Backend>(
    State(state): State<AppState<B>>,
    headers: HeaderMap,
    body: Body,
) -> Response {
    decide(&state, &headers, body, Change::Withdraw).await
}

/// What the mail about plans needs from the server: who sells them, and
/// where the account pages are.
pub(crate) fn mail_context(accounts: &Accounts) -> Option<Context<'_>> {
    Some(Context {
        merchant: accounts.merchant()?,
        origin: accounts.origin().as_str(),
    })
}

/// The languages of the mail about `account`'s plan.
pub(crate) async fn mail_langs<B: Backend>(
    store: &sync_store::Store<B>,
    account: Id,
    quebec: bool,
) -> sync_store::Result<MailLang> {
    let stored = store.account_lang(account).await?;
    Ok(MailLang::for_account(stored.as_deref(), quebec))
}

/// The contract for a subscription's first order, for an account the
/// store holds (§26, Contract email).
async fn contract_mail<B: Backend>(
    state: &AppState<B>,
    order: &PaidOrder,
) -> sync_store::Result<Option<BillingMail>> {
    let Some(context) = mail_context(&state.accounts) else {
        return Ok(None);
    };
    let Some(email) = state.store.account_email(order.account).await? else {
        return Ok(None);
    };
    let quebec =
        order.terms.country.as_deref() == Some("CA") && order.terms.region.as_deref() == Some("QC");
    let langs = mail_langs(&state.store, order.account, quebec).await?;
    let consent = state.store.latest_consent(order.account).await?;
    Ok(Some(terms::contract(
        &context,
        langs,
        &email,
        order,
        consent.as_ref(),
    )))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_read_in_decimal_units() {
        assert_eq!(size(0, Lang::En), "0 B");
        assert_eq!(size(999, Lang::En), "999 B");
        assert_eq!(size(12_345_678, Lang::En), "12.3 MB");
        assert_eq!(size(100_000_000, Lang::En), "100 MB");
        assert_eq!(size(1_000_000_000, Lang::En), "1 GB");
        assert_eq!(size(10_500_000_000, Lang::En), "10.5 GB");
        assert_eq!(size(12_345_678, Lang::Fr), "12,3 Mo");
    }
}
