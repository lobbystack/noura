//! The Plan section's view of `GET /v2/account` (sync-v2.md §20): the
//! plan's name, storage against it, a failed payment, and the workspaces
//! you own that went over your plan, named from their profiles.

use std::collections::BTreeMap;

use sync_protocol::messages::{AccountResponse, OwnedState, SubscriptionStatus};
use sync_protocol::types::Id;

use super::{OverPlanEntry, PlanView};

/// A plan's name, as the account page names it.
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

/// `bytes` in decimal units, as plans are sold.
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

/// A Unix time as a date, such as `February 25, 2027`.
fn date(seconds: u64) -> String {
    jiff::Timestamp::from_second(i64::try_from(seconds).unwrap_or(i64::MAX))
        .map(|time| {
            time.to_zoned(jiff::tz::TimeZone::UTC)
                .strftime("%B %-d, %Y")
                .to_string()
        })
        .unwrap_or_default()
}

/// The Plan section for `report`, or `None` on a server that sells no
/// plans. `names` holds each workspace's name from its profile.
pub fn view(report: &AccountResponse, names: &BTreeMap<Id, String>) -> Option<PlanView> {
    if !report.billing {
        return None;
    }
    let used = report.used_bytes.saturating_add(report.history_bytes);
    let usage = match report.storage_bytes {
        Some(limit) => format!("{} of {} used", size(used), size(limit)),
        None => format!("{} used", size(used)),
    };
    let over_plan = report
        .workspaces
        .0
        .iter()
        .filter(|w| w.state == OwnedState::OverPlan)
        .map(|w| OverPlanEntry {
            workspace: w.workspace,
            name: names
                .get(&w.workspace)
                .cloned()
                .unwrap_or_else(|| "A workspace".to_owned()),
            closes: w.closes_at.map(date).unwrap_or_default(),
        })
        .collect();
    Some(PlanView {
        name: plan_name(report.plan.as_str()).to_owned(),
        usage,
        past_due: report.status == SubscriptionStatus::PastDue,
        paid: matches!(
            report.status,
            SubscriptionStatus::Active | SubscriptionStatus::PastDue
        ),
        over_plan,
    })
}

/// The workspaces that stay active when you keep `keep` synced: it, then
/// the oldest active ones, as many as the plan keeps (§20, Over plan).
pub fn keeping(report: &AccountResponse, keep: Id) -> Vec<Id> {
    let limit = usize::try_from(report.owned_workspaces).unwrap_or(usize::MAX);
    std::iter::once(keep)
        .chain(
            report
                .workspaces
                .0
                .iter()
                .filter(|w| w.state == OwnedState::Active && w.workspace != keep)
                .map(|w| w.workspace),
        )
        .take(limit)
        .collect()
}

#[cfg(test)]
mod tests {
    use sync_protocol::codec::{List, Text};
    use sync_protocol::messages::OwnedWorkspace;

    use super::*;

    fn report(billing: bool) -> AccountResponse {
        AccountResponse {
            plan: Text("free".into()),
            status: SubscriptionStatus::Ended,
            period_end: None,
            billing,
            storage_bytes: Some(100_000_000),
            owned_workspaces: 1,
            max_file_bytes: Some(5_000_000),
            members: Some(4),
            upload_bytes_per_day: Some(1_000_000_000),
            download_bytes_per_day: Some(10_000_000_000),
            used_bytes: 12_000_000,
            history_bytes: 345_678,
            uploaded_today: 0,
            downloaded_today: 0,
            workspaces: List(vec![
                OwnedWorkspace {
                    workspace: Id([1; 16]),
                    state: OwnedState::Active,
                    closes_at: None,
                },
                OwnedWorkspace {
                    workspace: Id([2; 16]),
                    state: OwnedState::OverPlan,
                    closes_at: Some(1_803_600_000),
                },
            ]),
        }
    }

    #[test]
    fn the_section_names_the_plan_and_the_read_only_workspaces() {
        assert_eq!(view(&report(false), &BTreeMap::new()), None);
        let names = BTreeMap::from([(Id([2; 16]), "Garden".to_owned())]);
        let plan = view(&report(true), &names).expect("a plan");
        assert_eq!(plan.name, "Free");
        assert_eq!(plan.usage, "12.3 MB of 100 MB used");
        assert!(!plan.paid && !plan.past_due);
        assert_eq!(plan.over_plan.len(), 1);
        assert_eq!(plan.over_plan[0].name, "Garden");
        assert_eq!(plan.over_plan[0].closes, "February 26, 2027");
    }

    #[test]
    fn keeping_a_workspace_lets_the_newest_active_one_go() {
        assert_eq!(keeping(&report(true), Id([2; 16])), vec![Id([2; 16])]);
        let mut plus = report(true);
        plus.owned_workspaces = 2;
        assert_eq!(keeping(&plus, Id([2; 16])), vec![Id([2; 16]), Id([1; 16])]);
    }
}
