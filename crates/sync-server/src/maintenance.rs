//! The server's background work (docs/workspace-format/sync-v2.md §19,
//! §20): retention floors, pruning, reclaiming deleted domains, the quota's
//! claim on history, and the blob collector. Each instance runs it; the
//! store's locks serve one at a time.
//!
//! On a server that sells plans, the pass also runs the plan jobs: it
//! warns about workspaces over plan on their first day, on day 60 and on
//! day 85, closes them on day 90, warns `free` owners whom nobody used for
//! 11 and 12 months, closes their workspaces at 13, forgets webhook
//! events after 100 days and consents after 3 years, and sends the queued
//! mail about plans. Two instances may send the same notice once
//! each; each records what it sent after the mail went out.

use std::sync::Arc;
use std::time::Duration;

use sync_protocol::types::Id;
use sync_store::plans::{DAY, OVER_PLAN_DAYS};
use sync_store::{Backend, Maintenance, Store};

use crate::accounts::Accounts;
use crate::blobs::{SegmentStore, segment_key};

/// The days a workspace over plan gets its notices on: the first day,
/// day 60 and day 85.
const NOTICE_DAYS: [u64; 3] = [0, 60, 85];

/// The months without use after which a `free` owner gets each warning,
/// and after which the server closes its workspaces.
const WARNING_MONTHS: [u64; 2] = [11, 12];
const CLOSING_MONTHS: u64 = 13;

/// The month `now`, in Unix seconds, falls in, counted from January of
/// year 0 (§20, Inactive accounts).
pub fn month_of(now: u64) -> u64 {
    let zoned = jiff::Timestamp::from_second(i64::try_from(now).unwrap_or(i64::MAX))
        .unwrap_or(jiff::Timestamp::MAX)
        .to_zoned(jiff::tz::TimeZone::UTC);
    let year = u64::try_from(zoned.year()).unwrap_or(0);
    let month = u64::try_from(zoned.month()).unwrap_or(1);
    year * 12 + month - 1
}

/// The first day of `month`, as mail writes it.
fn month_start(month: u64) -> String {
    let year = i16::try_from(month / 12).unwrap_or(i16::MAX);
    let number = i8::try_from(month % 12 + 1).unwrap_or(1);
    jiff::civil::Date::new(year, number, 1)
        .map(|date| date.strftime("%B %-d, %Y").to_string())
        .unwrap_or_default()
}

/// `day`, counted from the Unix epoch, as mail writes it.
fn day_text(day: u64) -> String {
    jiff::Timestamp::from_second(i64::try_from(day.saturating_mul(DAY)).unwrap_or(i64::MAX))
        .map(|time| {
            time.to_zoned(jiff::tz::TimeZone::UTC)
                .strftime("%B %-d, %Y")
                .to_string()
        })
        .unwrap_or_default()
}

/// What the plan jobs did in one pass.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PlanJobs {
    /// Notices about workspaces over plan, one per workspace.
    pub notices: u64,
    /// Workspaces the server closed after 90 days over plan.
    pub closed_over_plan: u64,
    /// Inactivity warnings sent.
    pub warnings: u64,
    /// Workspaces the server closed after 13 months without use.
    pub closed_inactive: u64,
    /// Queued messages about plans that went out.
    pub mail: u64,
}

/// Sends `text` to each address, and returns whether every send went out.
async fn mail_all(accounts: &Accounts, to: &[String], subject: &str, text: &str) -> bool {
    let mut sent = true;
    for address in to {
        if let Err(error) = accounts.send(address, subject, text.to_owned()).await {
            tracing::error!(cause = %error, "a plan notice failed to send");
            sent = false;
        }
    }
    sent
}

/// The notice about a workspace over plan, for its owner and for its
/// other members.
fn over_plan_notice(origin: &str, closes: &str, first: bool) -> (String, String, String) {
    let subject = if first {
        "A workspace you sync is read-only".to_owned()
    } else {
        format!("A workspace you sync closes on {closes}")
    };
    let owner = format!(
        "Your Noura plan keeps fewer synced workspaces than you own, so the server keeps one of them read-only. Its files stay on every device.\n\nTo sync it again, choose which workspaces stay synced in Noura under Settings > Account, or upgrade at {origin}/account. If nothing changes, the server deletes its synced copy on {closes}.\n"
    );
    let member = format!(
        "A Noura workspace you're a member of is over its owner's plan, so the server keeps it read-only. Your files stay on your device.\n\nIf its owner doesn't change their plan, the server deletes its synced copy on {closes}, and your folder goes on working without sync.\n"
    );
    (subject, owner, member)
}

/// Warns, then closes, workspaces over plan (§20, Over plan).
async fn over_plan_job<B: Backend>(
    store: &Store<B>,
    accounts: &Accounts,
    today: u64,
    report: &mut PlanJobs,
) -> Result<(), sync_store::Error> {
    for row in store.over_plan_workspaces().await? {
        let days = today.saturating_sub(row.since);
        if days >= OVER_PLAN_DAYS {
            store.close_workspace_for_plan(row.workspace).await?;
            report.closed_over_plan += 1;
            continue;
        }
        let due = NOTICE_DAYS
            .iter()
            .filter(|notice_day| days >= **notice_day)
            .count();
        if usize::from(row.notices) >= due {
            continue;
        }
        let closes = day_text(row.since + OVER_PLAN_DAYS);
        let (subject, owner_text, member_text) =
            over_plan_notice(accounts.origin().as_str(), &closes, row.notices == 0);
        let emails = store.member_emails(row.workspace, row.owner).await?;
        let (owner, members) = emails.split_at(emails.len().min(1));
        let sent = mail_all(accounts, owner, &subject, &owner_text).await
            & mail_all(accounts, members, &subject, &member_text).await;
        if sent {
            let notices = u8::try_from(due).unwrap_or(u8::MAX);
            store
                .record_over_plan_notice(row.owner, row.workspace, row.since, notices)
                .await?;
            report.notices += 1;
        }
    }
    Ok(())
}

/// Warns, then closes the workspaces of, `free` owners whom nobody used
/// for 11, 12 and 13 months (§20, Inactive accounts). A warning goes out
/// only in its month, so an owner the job missed for a month keeps its
/// workspaces rather than losing them with too little notice.
async fn inactivity_job<B: Backend>(
    store: &Store<B>,
    accounts: &Accounts,
    month: u64,
    report: &mut PlanJobs,
) -> Result<(), sync_store::Error> {
    let Some(since) = month.checked_sub(WARNING_MONTHS[0]) else {
        return Ok(());
    };
    for account in store.inactive_owners(since).await? {
        let owner = store.inactivity(account).await?;
        if !owner.free {
            continue;
        }
        let idle = month.saturating_sub(owner.last_month);
        if idle >= CLOSING_MONTHS && owner.warnings >= 2 {
            for workspace in owner.workspaces {
                store.close_workspace_for_plan(workspace).await?;
                report.closed_inactive += 1;
            }
            continue;
        }
        let Some(step) = WARNING_MONTHS.iter().position(|m| *m == idle) else {
            continue;
        };
        if usize::from(owner.warnings) != step {
            continue;
        }
        let Some(email) = owner.email else {
            continue;
        };
        let closes = month_start(owner.last_month + CLOSING_MONTHS);
        let text = format!(
            "Nobody has synced the Noura workspaces you own for {idle} months. On {closes}, the server deletes their synced copy, as it does for Free accounts after a year without use. Your files on your devices stay.\n\nTo keep the copy, open Noura on any of your devices before then, or upgrade at {}/account.\n",
            accounts.origin().as_str()
        );
        let subject = format!("Your synced workspaces close on {closes}");
        if mail_all(accounts, &[email], &subject, &text).await {
            let warnings = u8::try_from(step + 1).unwrap_or(u8::MAX);
            store.set_inactivity_warnings(account, warnings).await?;
            report.warnings += 1;
        }
    }
    Ok(())
}

/// One pass of the plan jobs, on a server that sells plans.
pub async fn plan_jobs<B: Backend>(
    store: &Store<B>,
    accounts: &Accounts,
) -> Result<PlanJobs, sync_store::Error> {
    let mut report = PlanJobs::default();
    if !store.billing() {
        return Ok(report);
    }
    let now = store.clock().now();
    over_plan_job(store, accounts, now / DAY, &mut report).await?;
    inactivity_job(store, accounts, month_of(now), &mut report).await?;
    store.forget_billing_events().await?;
    store.forget_consents().await?;
    report.mail = accounts.send_billing_mail(store).await?;
    Ok(report)
}

/// Whether the account `account` was already noted active this month on
/// this instance.
pub(crate) fn noted(
    seen: &std::sync::Mutex<std::collections::HashMap<Id, u64>>,
    account: Id,
    month: u64,
) -> bool {
    let mut seen = seen
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if seen.get(&account) == Some(&month) {
        return true;
    }
    if seen.len() >= 65_536 {
        seen.clear();
    }
    seen.insert(account, month);
    false
}

/// How often a pass runs.
pub const PERIOD: Duration = Duration::from_secs(60);

/// Takes each blob that's due through the collector's three steps: mark the
/// row, delete the store objects, delete the row. A crash between steps
/// leaves a marked row that the next pass finishes, and so does a failure
/// on one blob, which costs the others nothing. Returns how many blobs it
/// deleted.
pub async fn collect_blobs<B: Backend>(
    store: &Store<B>,
    blobs: &dyn SegmentStore,
) -> Result<u64, sync_store::Error> {
    let mut collected = 0;
    for target in store.collectable_blobs().await? {
        let segments = match store.mark_blob(target).await {
            Ok(Some(segments)) => segments,
            Ok(None) => continue,
            Err(error) => {
                tracing::warn!("can't mark a collected blob: {error}");
                continue;
            }
        };
        let mut gone = true;
        for (index, hash) in segments.iter().enumerate() {
            let key = segment_key(&target.workspace, &target.blob, index as u64, hash);
            if let Err(error) = blobs.delete(&key).await {
                tracing::warn!("can't delete a collected segment: {error}");
                gone = false;
            }
        }
        if gone {
            match store.finish_blob(target).await {
                Ok(()) => collected += 1,
                Err(error) => tracing::warn!("can't finish a collected blob: {error}"),
            }
        }
    }
    Ok(collected)
}

/// One pass: the store's work, then the collector, then the store again so
/// a domain whose last blob just went finishes reclaiming.
pub async fn run_once<B: Backend>(
    store: &Store<B>,
    blobs: &dyn SegmentStore,
) -> Result<Maintenance, sync_store::Error> {
    let report = store.maintain().await?;
    if let Some(failure) = &report.failure {
        tracing::error!(failed = report.failed, cause = %failure, "maintenance skipped some work");
    }
    if collect_blobs(store, blobs).await? > 0 {
        store.maintain().await?;
    }
    if let Err(error) = crate::restore::collect_quarantine(blobs, store.clock().now()).await {
        tracing::warn!("can't collect the blob quarantine: {error}");
    }
    Ok(report)
}

/// Runs a pass, and the plan jobs, every [`PERIOD`].
pub fn spawn<B: Backend>(
    store: Arc<Store<B>>,
    blobs: Arc<dyn SegmentStore>,
    accounts: Arc<Accounts>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut every = tokio::time::interval(PERIOD);
        loop {
            every.tick().await;
            if let Err(error) = run_once(&store, blobs.as_ref()).await {
                tracing::error!(cause = %error, "a maintenance pass failed");
            }
            if let Err(error) = plan_jobs(&store, &accounts).await {
                tracing::error!(cause = %error, "the plan jobs failed");
            }
        }
    })
}
