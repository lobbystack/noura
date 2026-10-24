//! Which tasks each view and filter keeps, the order lists show them in,
//! when a task is overdue and how its due date reads. The rules come from
//! the legacy app's task list and date helpers.

use std::cmp::Ordering;

use jiff::{Timestamp, civil::Date, tz::TimeZone};
use plugin_sdk::clock;

use crate::task::{Due, Task};

/// The day it is in this device's time zone, and that zone.
pub(crate) struct Now {
    pub(crate) zone: TimeZone,
    pub(crate) today: Date,
}

impl Now {
    /// The device's clock, which the host gives: plugins have no other.
    pub(crate) fn read() -> Self {
        let reading = clock::now();
        let instant = reading.instant.parse().unwrap_or(Timestamp::UNIX_EPOCH);
        let zone = zone(&reading.time_zone);
        let today = instant.to_zoned(zone.clone()).date();
        Self { zone, today }
    }

    /// The first instant of tomorrow. A task due before it is due by the
    /// end of today.
    fn tomorrow(&self) -> Timestamp {
        self.today
            .tomorrow()
            .ok()
            .and_then(|tomorrow| tomorrow.to_zoned(self.zone.clone()).ok())
            .map_or(Timestamp::MAX, |start| start.timestamp())
    }
}

/// The time zone the host names: an IANA name, or an offset such as
/// `-04:00` for a zone without one. Anything else reads as UTC.
fn zone(name: &str) -> TimeZone {
    if let Some(sign) = name.chars().next().filter(|sign| matches!(sign, '+' | '-')) {
        let (hours, minutes) = name[1..].split_once(':').unwrap_or((&name[1..], "0"));
        let seconds =
            hours.parse::<i32>().unwrap_or(0) * 3600 + minutes.parse::<i32>().unwrap_or(0) * 60;
        let seconds = if sign == '-' { -seconds } else { seconds };
        return jiff::tz::Offset::from_seconds(seconds).map_or(TimeZone::UTC, TimeZone::fixed);
    }
    TimeZone::get(name).unwrap_or(TimeZone::UTC)
}

/// A list of tasks the sidebar offers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Choice {
    Today,
    Upcoming,
    All,
    Completed,
    /// The tasks of the project with this stable ID.
    Project(String),
    /// The tasks in this folder: empty for the top level.
    Folder(String),
}

/// The views the sidebar lists first, by node ID and name.
pub(crate) const VIEWS: [(Choice, &str, &str); 4] = [
    (Choice::Today, "view:today", "Today"),
    (Choice::Upcoming, "view:upcoming", "Upcoming"),
    (Choice::All, "view:all", "All"),
    (Choice::Completed, "view:completed", "Completed"),
];

/// Which project the project filter keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ProjectFilter {
    Any,
    /// Tasks that name no project.
    Without,
    /// The tasks of the project with this stable ID.
    Is(String),
}

/// The filters that narrow the current view. They combine.
#[derive(Debug, Clone)]
pub(crate) struct Filters {
    pub(crate) status: Option<String>,
    pub(crate) project: ProjectFilter,
    /// Tasks in this folder, empty for the top level.
    pub(crate) folder: Option<String>,
    /// Tasks due on or before this day.
    pub(crate) due_by: Option<Date>,
}

impl Filters {
    pub(crate) const NONE: Self = Self {
        status: None,
        project: ProjectFilter::Any,
        folder: None,
        due_by: None,
    };

    /// Whether `task` passes every filter.
    pub(crate) fn keep(&self, task: &Task, now: &Now) -> bool {
        let status = self
            .status
            .as_ref()
            .is_none_or(|status| task.status == *status);
        let project = match &self.project {
            ProjectFilter::Any => true,
            ProjectFilter::Without => task.project.is_none(),
            ProjectFilter::Is(id) => task.project.as_deref() == Some(id.as_str()),
        };
        let folder = self
            .folder
            .as_ref()
            .is_none_or(|folder| task.folder() == folder);
        let due = self
            .due_by
            .is_none_or(|by| task.due.is_some_and(|due| due.day(&now.zone) <= by));
        status && project && folder && due
    }
}

/// Whether the view `choice` holds `task`.
pub(crate) fn holds(choice: &Choice, task: &Task, now: &Now) -> bool {
    match choice {
        Choice::Today => !task.done() && task.due.is_some_and(|due| due_by_today(due, now)),
        Choice::Upcoming => !task.done() && task.due.is_some_and(|due| !due_by_today(due, now)),
        Choice::All => true,
        Choice::Completed => task.done(),
        Choice::Project(id) => task.project.as_deref() == Some(id.as_str()),
        Choice::Folder(folder) => task.folder() == folder,
    }
}

/// Whether `due` falls by the end of today, overdue included.
fn due_by_today(due: Due, now: &Now) -> bool {
    match due {
        Due::Day(day) => day <= now.today,
        Due::At(at) => at < now.tomorrow(),
    }
}

/// A task not marked done whose due day falls before today.
pub(crate) fn overdue(task: &Task, now: &Now) -> bool {
    !task.done() && task.due.is_some_and(|due| due.day(&now.zone) < now.today)
}

/// Put `tasks` in list order: tasks not done first, then by due date with
/// dated tasks before undated ones, then by priority from urgent to low,
/// then by last update, oldest first. The path settles the rest.
pub(crate) fn order(tasks: &mut [Task], now: &Now) {
    tasks.sort_by(|left, right| {
        left.done()
            .cmp(&right.done())
            .then_with(|| by_due(left, right, &now.zone))
            .then_with(|| weight(&left.priority).cmp(&weight(&right.priority)))
            .then_with(|| left.updated.cmp(&right.updated))
            .then_with(|| left.path.cmp(&right.path))
    });
}

fn by_due(left: &Task, right: &Task, zone: &TimeZone) -> Ordering {
    match (left.due, right.due) {
        (Some(left), Some(right)) => left.instant(zone).cmp(&right.instant(zone)),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

/// Urgent first, low last. A value outside the four sorts as medium.
fn weight(priority: &str) -> u8 {
    match priority {
        "urgent" => 0,
        "high" => 1,
        "low" => 3,
        _ => 2,
    }
}

/// How a due date reads: Today, Tomorrow or Yesterday, a weekday within
/// the week ahead, or a short date, with the year when it isn't this
/// year's. A due time adds its clock time, alone when it falls today.
pub(crate) fn due_label(due: Due, now: &Now) -> String {
    let day = due.day(&now.zone);
    let named = day_name(day, now.today);
    match due {
        Due::Day(_) => named,
        Due::At(at) => {
            let time = at.to_zoned(now.zone.clone()).strftime("%H:%M").to_string();
            if day == now.today {
                time
            } else {
                format!("{named} {time}")
            }
        }
    }
}

fn day_name(day: Date, today: Date) -> String {
    // Subtracting dates gives a span of whole days.
    let offset = (day - today).get_days();
    match offset {
        0 => "Today".to_owned(),
        1 => "Tomorrow".to_owned(),
        -1 => "Yesterday".to_owned(),
        2..7 => day.strftime("%A").to_string(),
        _ if day.year() == today.year() => day.strftime("%b %-d").to_string(),
        _ => day.strftime("%b %-d, %Y").to_string(),
    }
}
