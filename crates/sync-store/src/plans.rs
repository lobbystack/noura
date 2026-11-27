//! Plans (docs/workspace-format/sync-v2.md §20): the fixed table of what
//! each plan allows, keyed by the server's own plan IDs, and what an
//! account's stored subscription makes of it. A payment provider maps its
//! own product IDs to these plan IDs; nothing it reports changes a limit.

use sync_protocol::codec::{List, Text};
use sync_protocol::messages::{AccountResponse, OwnedState, OwnedWorkspace, SubscriptionStatus};
use sync_protocol::types::Id;

use crate::create::MAX_OWNED_WORKSPACES;
use crate::{Backend, OwnedWorkspaceRow, ReadTxn, Result, Session, Store, WriteTxn, refuse};

/// Seconds in a day; days count from the Unix epoch.
pub const DAY: u64 = 86_400;

/// How long a workspace stays over plan before the server closes it.
pub const OVER_PLAN_DAYS: u64 = 90;

/// Decimal sizes, as the plans are sold.
const MB: u64 = 1_000_000;
const GB: u64 = 1_000_000_000;

/// The plan of an account with no subscription, or whose subscription
/// ended.
pub const FREE: &str = "free";

/// The plan every account has on a server that sells no plans.
pub const OPERATOR: &str = "operator";

/// What a plan allows. `None` is no limit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanLimits {
    /// Storage the account's open workspaces share, history included.
    pub storage_bytes: Option<u64>,
    /// Open workspaces the account may own.
    pub owned_workspaces: u64,
    /// The largest file one blob may hold.
    pub max_file_bytes: Option<u64>,
    /// Member accounts in each workspace the account owns.
    pub members: Option<u64>,
    /// Bytes the account may push and upload each UTC day.
    pub upload_per_day: Option<u64>,
    /// Bytes the account may read each UTC day.
    pub download_per_day: Option<u64>,
}

impl PlanLimits {
    /// The limits of `operator`: only the operator's settings and the
    /// protocol's own limits apply.
    pub const OPERATOR: Self = Self {
        storage_bytes: None,
        owned_workspaces: MAX_OWNED_WORKSPACES,
        max_file_bytes: None,
        members: None,
        upload_per_day: None,
        download_per_day: None,
    };
}

const FREE_LIMITS: PlanLimits = PlanLimits {
    storage_bytes: Some(100 * MB),
    owned_workspaces: 1,
    max_file_bytes: Some(5 * MB),
    members: Some(4),
    upload_per_day: Some(GB),
    download_per_day: Some(10 * GB),
};

const STANDARD: PlanLimits = PlanLimits {
    storage_bytes: Some(GB),
    owned_workspaces: 1,
    max_file_bytes: Some(5 * MB),
    members: Some(20),
    upload_per_day: Some(10 * GB),
    download_per_day: Some(100 * GB),
};

const PLUS: PlanLimits = PlanLimits {
    storage_bytes: Some(10 * GB),
    owned_workspaces: 10,
    max_file_bytes: Some(200 * MB),
    members: Some(20),
    upload_per_day: Some(50 * GB),
    download_per_day: Some(500 * GB),
};

const PLUS_100GB: PlanLimits = PlanLimits {
    storage_bytes: Some(100 * GB),
    ..PLUS
};

/// The plans a server sells, by plan ID, `free` first.
pub const PLANS: &[(&str, PlanLimits)] = &[
    (FREE, FREE_LIMITS),
    ("standard_year", STANDARD),
    ("standard_month", STANDARD),
    ("plus_year", PLUS),
    ("plus_month", PLUS),
    ("plus_100gb_year", PLUS_100GB),
    ("plus_100gb_month", PLUS_100GB),
];

/// The limits of a sold plan, or `None` for an ID the table lacks.
pub fn limits(plan: &str) -> Option<PlanLimits> {
    PLANS
        .iter()
        .find(|(id, _)| *id == plan)
        .map(|(_, limits)| *limits)
}

/// Whether `plan` names a plan a provider can sell: any but `free`.
pub fn is_paid(plan: &str) -> bool {
    plan != FREE && limits(plan).is_some()
}

/// Where a stored subscription stands, as the store keeps it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubscriptionState {
    /// Paid and running.
    Active,
    /// A payment failed and the provider retries it: the plan holds.
    PastDue,
    /// Canceled, unpaid, revoked or never paid: the account is on `free`.
    #[default]
    Ended,
}

impl SubscriptionState {
    /// The stored code.
    pub fn code(self) -> u8 {
        match self {
            Self::Active => 1,
            Self::PastDue => 2,
            Self::Ended => 3,
        }
    }

    /// The state a stored code names; an unknown code reads as ended, so a
    /// corrupt row never grants a plan.
    pub fn from_code(code: u8) -> Self {
        match code {
            1 => Self::Active,
            2 => Self::PastDue,
            _ => Self::Ended,
        }
    }

    /// Whether the subscription's plan applies.
    pub fn grants(self) -> bool {
        matches!(self, Self::Active | Self::PastDue)
    }
}

/// An account's stored subscription (§29, Plans and billing).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AccountPlanRow {
    /// The payment provider's name, such as `polar`.
    pub provider: String,
    /// The provider's customer ID.
    pub customer: String,
    /// The provider's subscription ID.
    pub subscription: String,
    /// The plan ID the subscription's product maps to.
    pub plan: String,
    /// Where it stands.
    pub state: SubscriptionState,
    /// The end of its current period, in Unix seconds.
    pub period_end: Option<u64>,
    /// The time of the provider's last change to it, in Unix seconds,
    /// which orders the provider's events.
    pub changed_at: u64,
    /// The start of its current period, in Unix seconds.
    pub period_start: Option<u64>,
    /// When it started, in Unix seconds, which opens the withdrawal period.
    pub started_at: Option<u64>,
    /// Its price for one period before tax, in the currency's smallest unit.
    pub amount: Option<u64>,
    /// Its currency, as a lower-case ISO 4217 code.
    pub currency: Option<String>,
    /// `month` or `year`.
    pub interval: Option<String>,
    /// Whether it ends when its period ends instead of renewing.
    pub canceling: bool,
    /// The billing address's country, as an ISO 3166-1 alpha-2 code.
    pub country: Option<String>,
    /// The billing address's state or province, such as `QC`.
    pub region: Option<String>,
    /// The period end the renewal reminders went out for. The provider
    /// knows nothing of the three reminder fields, so a webhook's row
    /// keeps the stored ones while it reports the same subscription.
    pub reminded_for: Option<u64>,
    /// How many renewal reminders went out for that period.
    pub reminders: u8,
    /// The day, from the Unix epoch, a monthly plan's last yearly reminder
    /// went out.
    pub reminded_day: Option<u64>,
}

/// The member states of the European Union, and the United Kingdom, whose
/// consumers the terms give the same right of withdrawal.
const WITHDRAWAL_COUNTRIES: &[&str] = &[
    "AT", "BE", "BG", "HR", "CY", "CZ", "DK", "EE", "FI", "FR", "DE", "GR", "HU", "IE", "IT", "LV",
    "LT", "LU", "MT", "NL", "PL", "PT", "RO", "SK", "SI", "ES", "SE", "GB",
];

impl AccountPlanRow {
    /// Whether the billing address is in Québec.
    pub fn in_quebec(&self) -> bool {
        self.country.as_deref() == Some("CA") && self.region.as_deref() == Some("QC")
    }

    /// Whether the billing address is in the European Union or the United
    /// Kingdom.
    pub fn in_eu_or_uk(&self) -> bool {
        self.country
            .as_deref()
            .is_some_and(|country| WITHDRAWAL_COUNTRIES.contains(&country))
    }

    /// Whether it bills every year.
    pub fn yearly(&self) -> bool {
        self.interval.as_deref() == Some("year")
    }

    /// The same subscription as `self`, with `stored`'s reminders when
    /// `stored` is the same subscription.
    pub fn keeping_reminders(mut self, stored: Option<&AccountPlanRow>) -> Self {
        if let Some(stored) = stored
            && stored.provider == self.provider
            && stored.subscription == self.subscription
        {
            self.reminded_for = stored.reminded_for;
            self.reminders = stored.reminders;
            self.reminded_day = stored.reminded_day;
        }
        self
    }
}

/// The plan an account has, given whether the server sells plans and its
/// stored subscription.
pub fn effective(billing: bool, row: Option<&AccountPlanRow>) -> (&'static str, PlanLimits) {
    if !billing {
        return (OPERATOR, PlanLimits::OPERATOR);
    }
    if let Some(row) = row
        && row.state.grants()
        && let Some((id, limits)) = PLANS.iter().find(|(id, _)| *id == row.plan)
    {
        return (id, *limits);
    }
    (FREE, FREE_LIMITS)
}

/// Whether `incoming` should replace `stored`: a newer change to the same
/// subscription, or another subscription that grants a plan while the
/// stored one doesn't, or that changed later.
pub fn supersedes(stored: Option<&AccountPlanRow>, incoming: &AccountPlanRow) -> bool {
    let Some(stored) = stored else {
        return true;
    };
    if stored.provider == incoming.provider && stored.subscription == incoming.subscription {
        return incoming.changed_at >= stored.changed_at;
    }
    if incoming.state.grants() {
        return !stored.state.grants() || incoming.changed_at >= stored.changed_at;
    }
    !stored.state.grants() && incoming.changed_at >= stored.changed_at
}

/// Which of `rows`, the open workspaces an account owns in creation order,
/// stay active under `limit`: those the owner kept first, then the
/// oldest.
fn active_set(rows: &[OwnedWorkspaceRow], limit: u64) -> Vec<Id> {
    let mut order: Vec<&OwnedWorkspaceRow> = rows.iter().collect();
    order.sort_by_key(|row| (!row.kept, row.ordinal));
    order
        .into_iter()
        .take(usize::try_from(limit).unwrap_or(usize::MAX))
        .map(|row| row.workspace)
        .collect()
}

/// Puts the workspaces `account` owns beyond its plan over plan, from
/// `today`, and makes the others active (§20, Over plan). On a server that
/// sells no plans every workspace is active.
pub(crate) async fn reconcile(
    txn: &mut impl WriteTxn,
    account: Id,
    billing: bool,
    today: u64,
) -> Result<()> {
    let limit = crate::retention::plan_of(txn, account, billing)
        .await?
        .owned_workspaces;
    let rows = txn.owned_workspace_rows(account).await?;
    let active = if billing {
        active_set(&rows, limit)
    } else {
        rows.iter().map(|row| row.workspace).collect()
    };
    for row in &rows {
        match (active.contains(&row.workspace), row.over_plan_since) {
            (true, Some(_)) => txn.set_over_plan(row.workspace, None, 0).await?,
            (false, None) => txn.set_over_plan(row.workspace, Some(today), 0).await?,
            _ => {}
        }
    }
    Ok(())
}

/// The `GET /v2/account` report, with today's transfer left at 0 for the
/// server to fill in from its counters.
async fn report(txn: &mut impl ReadTxn, account: Id, billing: bool) -> Result<AccountResponse> {
    let row = txn.account_plan(account).await?;
    let (plan, limits) = effective(billing, row.as_ref());
    let status = match (&row, billing) {
        (Some(row), true) => match row.state {
            SubscriptionState::Active => SubscriptionStatus::Active,
            SubscriptionState::PastDue => SubscriptionStatus::PastDue,
            SubscriptionState::Ended => SubscriptionStatus::Ended,
        },
        _ => SubscriptionStatus::None,
    };
    let period_end = row
        .as_ref()
        .filter(|row| billing && row.state.grants())
        .and_then(|row| row.period_end);
    let (used_bytes, history_bytes) = txn.account_usage(account).await?;
    let workspaces = txn
        .owned_workspace_rows(account)
        .await?
        .into_iter()
        .map(|row| OwnedWorkspace {
            workspace: row.workspace,
            state: if row.over_plan_since.is_some() {
                OwnedState::OverPlan
            } else {
                OwnedState::Active
            },
            closes_at: row
                .over_plan_since
                .map(|day| (day + OVER_PLAN_DAYS).saturating_mul(DAY)),
        })
        .collect();
    let clamp = |value: u64| u32::try_from(value).unwrap_or(u32::MAX);
    Ok(AccountResponse {
        plan: Text(plan.to_owned()),
        status,
        period_end,
        billing,
        storage_bytes: limits.storage_bytes,
        owned_workspaces: clamp(limits.owned_workspaces),
        max_file_bytes: limits.max_file_bytes,
        members: limits.members.map(clamp),
        upload_bytes_per_day: limits.upload_per_day,
        download_bytes_per_day: limits.download_per_day,
        used_bytes,
        history_bytes,
        uploaded_today: 0,
        downloaded_today: 0,
        workspaces: List(workspaces),
    })
}

impl<B: Backend> Store<B> {
    /// `GET /v2/account`: the session account's plan, limits and usage,
    /// and the state of each workspace it owns (§20, §26). Today's
    /// transfer comes from the server's counters, so it reads 0 here.
    pub async fn account(&self, session: Session) -> Result<AccountResponse> {
        self.account_report(session.account).await
    }

    /// [`Store::account`] by account, for the account page.
    pub async fn account_report(&self, account: Id) -> Result<AccountResponse> {
        let billing = self.billing;
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { report(txn, account, billing).await })
            }))
            .await
    }

    /// The limits of `account`'s plan.
    pub async fn plan_limits(&self, account: Id) -> Result<PlanLimits> {
        let billing = self.billing;
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { crate::retention::plan_of(txn, account, billing).await })
            }))
            .await
    }

    /// `POST /v2/account/active-workspaces`: the owned workspaces that stay
    /// active, at most as many as the plan allows; the owner's other
    /// workspaces beyond the plan go over plan (§20, Over plan).
    pub async fn set_active_workspaces(&self, session: Session, chosen: Vec<Id>) -> Result<()> {
        let billing = self.billing;
        let today = self.clock().now() / DAY;
        let account = session.account;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(account).await?;
                    let limits = crate::retention::plan_of(txn, account, billing).await?;
                    if chosen.len() as u64 > limits.owned_workspaces {
                        return Err(refuse(
                            "bad_request",
                            "the plan keeps fewer workspaces active",
                        ));
                    }
                    let rows = txn.owned_workspace_rows(account).await?;
                    if chosen
                        .iter()
                        .any(|id| !rows.iter().any(|row| row.workspace == *id))
                    {
                        return Err(refuse("not_found", "no such workspace of this account"));
                    }
                    for row in &rows {
                        let kept = chosen.contains(&row.workspace);
                        if kept != row.kept {
                            txn.set_kept(row.workspace, kept).await?;
                        }
                    }
                    reconcile(txn, account, billing, today).await
                })
            }))
            .await
    }

    /// Brings `account`'s workspaces in line with its plan (§20, Over
    /// plan).
    pub async fn reconcile_plan(&self, account: Id) -> Result<()> {
        let billing = self.billing;
        let today = self.clock().now() / DAY;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(account).await?;
                    reconcile(txn, account, billing, today).await
                })
            }))
            .await
    }

    /// Applies a payment provider's webhook event once: it records the
    /// event, stores its subscription when that [`supersedes`] the stored
    /// one, then brings the account's workspaces in line with the plan,
    /// stores its order when that's the account's latest, and queues its
    /// mail. Returns false for an event it already applied. An account the
    /// store doesn't hold gets nothing stored.
    pub async fn apply_billing_event(&self, event: crate::billing::BillingEvent) -> Result<bool> {
        let billing = self.billing;
        let today = self.clock().now() / DAY;
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    let crate::billing::BillingEvent {
                        provider,
                        id,
                        change,
                        order,
                        mail,
                    } = event;
                    if !txn.insert_billing_event(&provider, &id, today).await? {
                        return Ok(false);
                    }
                    if let Some((account, row)) = change
                        && txn.account_email(account).await?.is_some()
                    {
                        txn.lock_account(account).await?;
                        let stored = txn.account_plan(account).await?;
                        if supersedes(stored.as_ref(), &row) {
                            let row = row.keeping_reminders(stored.as_ref());
                            txn.put_account_plan(account, &row).await?;
                            reconcile(txn, account, billing, today).await?;
                        }
                    }
                    if let Some((account, row)) = order
                        && txn.account_email(account).await?.is_some()
                    {
                        let newer = txn
                            .latest_order(account)
                            .await?
                            .is_none_or(|stored| stored.paid_at <= row.paid_at);
                        if newer {
                            txn.put_latest_order(account, &row).await?;
                        }
                    }
                    for message in &mail {
                        txn.queue_billing_mail(message).await?;
                    }
                    Ok(true)
                })
            }))
            .await
    }

    /// The account's stored subscription, for the billing pages.
    pub async fn account_plan(&self, account: Id) -> Result<Option<AccountPlanRow>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.account_plan(account).await })
            }))
            .await
    }

    /// Deletes the webhook events recorded more than 100 days ago.
    pub async fn forget_billing_events(&self) -> Result<u64> {
        let before = (self.clock().now() / DAY).saturating_sub(100);
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.delete_billing_events_before(before).await })
            }))
            .await
    }

    /// Up to 4,096 open workspaces over plan, earliest first, after each
    /// of their owners' workspaces came back in line with the plan, so a
    /// plan that changed or a server that stopped selling plans releases
    /// them first.
    pub async fn over_plan_workspaces(&self) -> Result<Vec<crate::OverPlanRow>> {
        let read = || {
            self.backend.read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.over_plan_workspaces(4_096).await })
            }))
        };
        let mut owners: Vec<Id> = read().await?.iter().map(|row| row.owner).collect();
        owners.sort();
        owners.dedup();
        for owner in owners {
            self.reconcile_plan(owner).await?;
        }
        read().await
    }

    /// Records that `notices` notices went out about `workspace`, still
    /// over plan since `since`. Returns false when the workspace left that
    /// state in the meantime.
    pub async fn record_over_plan_notice(
        &self,
        owner: Id,
        workspace: Id,
        since: u64,
        notices: u8,
    ) -> Result<bool> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move {
                    txn.lock_account(owner).await?;
                    let still = txn
                        .owned_workspace_rows(owner)
                        .await?
                        .into_iter()
                        .any(|row| {
                            row.workspace == workspace && row.over_plan_since == Some(since)
                        });
                    if still {
                        txn.set_over_plan(workspace, Some(since), notices).await?;
                    }
                    Ok(still)
                })
            }))
            .await
    }

    /// The accounts and emails of a workspace's members, its owner first.
    pub async fn member_emails(&self, workspace: Id, owner: Id) -> Result<Vec<(Id, String)>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let mut accounts = vec![owner];
                    accounts.extend(
                        txn.member_accounts(workspace)
                            .await?
                            .into_iter()
                            .filter(|account| *account != owner),
                    );
                    let mut emails = Vec::new();
                    for account in accounts {
                        if let Some(email) = txn.account_email(account).await? {
                            emails.push((account, email));
                        }
                    }
                    Ok(emails)
                })
            }))
            .await
    }

    /// Up to 4,096 accounts that own an open workspace and were last
    /// active in `month` or before (§20, Inactive accounts).
    pub async fn inactive_owners(&self, month: u64) -> Result<Vec<Id>> {
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move { txn.inactive_owners(month, 4_096).await })
            }))
            .await
    }

    /// Where an owner stands for the inactivity rule: the latest month that
    /// it or a member of a workspace it owns was active, the warnings it
    /// got, whether its plan is `free`, its email, and its open workspaces.
    pub async fn inactivity(&self, account: Id) -> Result<Inactivity> {
        let billing = self.billing;
        self.backend
            .read(Box::new(move |txn: &mut B::Read| {
                Box::pin(async move {
                    let (mut last_month, warnings) =
                        txn.account_activity(account).await?.unwrap_or((0, 0));
                    let row = txn.account_plan(account).await?;
                    let free = billing && effective(true, row.as_ref()).0 == FREE;
                    let workspaces: Vec<Id> = txn
                        .owned_workspace_rows(account)
                        .await?
                        .into_iter()
                        .map(|row| row.workspace)
                        .collect();
                    for workspace in &workspaces {
                        for member in txn.member_accounts(*workspace).await? {
                            if let Some((month, _)) = txn.account_activity(member).await? {
                                last_month = last_month.max(month);
                            }
                        }
                    }
                    Ok(Inactivity {
                        last_month,
                        warnings,
                        free,
                        email: txn.account_email(account).await?,
                        workspaces,
                    })
                })
            }))
            .await
    }

    /// Records how many inactivity warnings `account` got.
    pub async fn set_inactivity_warnings(&self, account: Id, warnings: u8) -> Result<()> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.set_inactivity_warnings(account, warnings).await })
            }))
            .await
    }

    /// Records that `account` sent an authenticated request in `month`,
    /// counted from January of year 0, and clears its inactivity warnings
    /// (§20, Inactive accounts). Returns whether the month moved.
    pub async fn note_activity(&self, account: Id, month: u64) -> Result<bool> {
        self.backend
            .write(Box::new(move |txn: &mut B::Write| {
                Box::pin(async move { txn.note_activity(account, month).await })
            }))
            .await
    }
}

/// An owner as the inactivity rule sees it (§20, Inactive accounts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inactivity {
    /// The latest month it or a member of its workspaces was active.
    pub last_month: u64,
    /// The warnings it got since.
    pub warnings: u8,
    /// Whether its plan is `free` on a server that sells plans.
    pub free: bool,
    /// Its email.
    pub email: Option<String>,
    /// The open workspaces it owns.
    pub workspaces: Vec<Id>,
}

/// `over_plan`: the workspace is read-only until it returns within its
/// owner's plan (§20, Over plan).
pub(crate) fn over_plan() -> crate::Error {
    crate::refuse(
        "over_plan",
        "the workspace is over its owner's plan and takes only what keeps it safe",
    )
}

/// `file_too_large`, with the largest file the owner's plan allows.
pub(crate) fn file_too_large(max_file_bytes: u64) -> crate::Error {
    use sync_protocol::codec::Encode;
    crate::Error::Refused(
        crate::Refusal::new(
            "file_too_large",
            "the file exceeds the largest file the owner's plan allows",
        )
        .detail(sync_protocol::messages::FileTooLargeDetail { max_file_bytes }.to_bytes()),
    )
}

/// `member_limit`.
pub(crate) fn member_limit() -> crate::Error {
    crate::refuse(
        "member_limit",
        "the workspace holds as many members as its owner's plan allows",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(subscription: &str, state: SubscriptionState, changed_at: u64) -> AccountPlanRow {
        AccountPlanRow {
            provider: "polar".into(),
            customer: "c".into(),
            subscription: subscription.into(),
            plan: "plus_year".into(),
            state,
            changed_at,
            ..AccountPlanRow::default()
        }
    }

    #[test]
    fn the_table_matches_section_20() {
        let free = limits(FREE).expect("free");
        assert_eq!(free.storage_bytes, Some(100_000_000));
        assert_eq!(free.owned_workspaces, 1);
        assert_eq!(free.max_file_bytes, Some(5_000_000));
        assert_eq!(free.members, Some(4));
        assert_eq!(
            (free.upload_per_day, free.download_per_day),
            (Some(1_000_000_000), Some(10_000_000_000))
        );
        for id in ["standard_year", "standard_month"] {
            let plan = limits(id).expect("standard");
            assert_eq!(plan.storage_bytes, Some(1_000_000_000));
            assert_eq!((plan.owned_workspaces, plan.members), (1, Some(20)));
            assert_eq!(plan.max_file_bytes, Some(5_000_000));
            assert_eq!(plan.download_per_day, Some(100_000_000_000));
        }
        for (id, storage) in [
            ("plus_year", 10_000_000_000),
            ("plus_month", 10_000_000_000),
            ("plus_100gb_year", 100_000_000_000),
            ("plus_100gb_month", 100_000_000_000),
        ] {
            let plan = limits(id).expect("plus");
            assert_eq!(plan.storage_bytes, Some(storage));
            assert_eq!((plan.owned_workspaces, plan.members), (10, Some(20)));
            assert_eq!(plan.max_file_bytes, Some(200_000_000));
            assert_eq!(
                (plan.upload_per_day, plan.download_per_day),
                (Some(50_000_000_000), Some(500_000_000_000))
            );
        }
        assert!(limits("enterprise").is_none());
        assert!(!is_paid(FREE) && is_paid("plus_month") && !is_paid(OPERATOR));
    }

    #[test]
    fn a_plan_applies_while_active_or_past_due() {
        assert_eq!(effective(false, None).0, OPERATOR);
        assert_eq!(effective(true, None).0, FREE);
        let active = row("s", SubscriptionState::Active, 1);
        assert_eq!(effective(true, Some(&active)).0, "plus_year");
        assert_eq!(effective(false, Some(&active)).0, OPERATOR);
        let due = row("s", SubscriptionState::PastDue, 1);
        assert_eq!(effective(true, Some(&due)).0, "plus_year");
        let ended = row("s", SubscriptionState::Ended, 1);
        assert_eq!(effective(true, Some(&ended)).0, FREE);
        let unknown = AccountPlanRow {
            plan: "gold".into(),
            ..active
        };
        assert_eq!(effective(true, Some(&unknown)).0, FREE);
        assert_eq!(SubscriptionState::from_code(9), SubscriptionState::Ended);
    }

    #[test]
    fn an_older_event_never_replaces_a_newer_one() {
        let stored = row("a", SubscriptionState::Active, 10);
        assert!(!supersedes(
            Some(&stored),
            &row("a", SubscriptionState::Ended, 9)
        ));
        assert!(supersedes(
            Some(&stored),
            &row("a", SubscriptionState::Ended, 11)
        ));
        // An old subscription's late end leaves the running one alone.
        assert!(!supersedes(
            Some(&stored),
            &row("b", SubscriptionState::Ended, 12)
        ));
        // A new subscription replaces an ended one.
        let ended = row("a", SubscriptionState::Ended, 10);
        assert!(supersedes(
            Some(&ended),
            &row("b", SubscriptionState::Active, 5)
        ));
        assert!(supersedes(None, &row("a", SubscriptionState::Ended, 0)));
    }
}
