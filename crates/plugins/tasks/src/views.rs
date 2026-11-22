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
    /// Due from today through the next six days, or overdue.
    Week,
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
    (Choice::Week, "view:week", "Next 7 days"),
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

/// Whose tasks the assignee filter keeps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum AssigneeFilter {
    Any,
    /// Tasks that name no one.
    Without,
    /// The tasks assigned to this name, ignoring case.
    Is(String),
}

/// The filters that narrow the current view. They combine.
#[derive(Debug, Clone)]
pub(crate) struct Filters {
    pub(crate) status: Option<String>,
    pub(crate) project: ProjectFilter,
    pub(crate) assignee: AssigneeFilter,
    /// Tasks in this folder, empty for the top level.
    pub(crate) folder: Option<String>,
    /// Tasks due on or before this day.
    pub(crate) due_by: Option<Date>,
}

impl Filters {
    pub(crate) const NONE: Self = Self {
        status: None,
        project: ProjectFilter::Any,
        assignee: AssigneeFilter::Any,
        folder: None,
        due_by: None,
    };

    /// Whether any filter narrows the view.
    pub(crate) fn active(&self) -> bool {
        self.status.is_some()
            || !matches!(self.project, ProjectFilter::Any)
            || !matches!(self.assignee, AssigneeFilter::Any)
            || self.folder.is_some()
            || self.due_by.is_some()
    }

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
        let assignee = match &self.assignee {
            AssigneeFilter::Any => true,
            AssigneeFilter::Without => task.assignee.is_none(),
            AssigneeFilter::Is(name) => task
                .assignee
                .as_deref()
                .is_some_and(|assignee| assignee.to_lowercase() == name.to_lowercase()),
        };
        let folder = self
            .folder
            .as_ref()
            .is_none_or(|folder| task.folder() == folder);
        let due = self
            .due_by
            .is_none_or(|by| task.due.is_some_and(|due| due.day(&now.zone) <= by));
        status && project && assignee && folder && due
    }
}

/// Whether the view `choice` holds `task`.
pub(crate) fn holds(choice: &Choice, task: &Task, now: &Now) -> bool {
    match choice {
        Choice::Today => !task.done() && task.due.is_some_and(|due| due_by_today(due, now)),
        Choice::Week => !task.done() && task.due.is_some_and(|due| within_week(due, now)),
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

/// Whether `due` falls by the end of the sixth day after today, overdue
/// included.
fn within_week(due: Due, now: &Now) -> bool {
    now.today
        .checked_add(jiff::ToSpan::days(6))
        .is_ok_and(|last| due.day(&now.zone) <= last)
}

/// A group of a task list, in the order the list shows them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Section {
    Overdue,
    Today,
    Upcoming,
    NoDate,
    Completed,
}

impl Section {
    pub(crate) const ALL: [Self; 5] = [
        Self::Overdue,
        Self::Today,
        Self::Upcoming,
        Self::NoDate,
        Self::Completed,
    ];

    /// What node IDs of the section say after `tasks-`.
    pub(crate) fn key(self) -> &'static str {
        match self {
            Self::Overdue => "overdue",
            Self::Today => "today",
            Self::Upcoming => "upcoming",
            Self::NoDate => "no-date",
            Self::Completed => "completed",
        }
    }

    pub(crate) fn title(self) -> &'static str {
        match self {
            Self::Overdue => "Overdue",
            Self::Today => "Today",
            Self::Upcoming => "Upcoming",
            Self::NoDate => "No date",
            Self::Completed => "Completed",
        }
    }
}

/// The section a list shows `task` in: done and cancelled tasks last, the
/// rest by when they're due.
pub(crate) fn section(task: &Task, now: &Now) -> Section {
    match task.due {
        _ if task.closed() => Section::Completed,
        None => Section::NoDate,
        Some(_) if overdue(task, now) => Section::Overdue,
        Some(due) if due_by_today(due, now) => Section::Today,
        Some(_) => Section::Upcoming,
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
            .then_with(|| by_update(left, right))
            .then_with(|| left.path.cmp(&right.path))
    });
}

/// Older last updates first, compared as instants, since another app may
/// write one with its own offset or with fractions of a second. A value
/// that isn't RFC 3339 sorts after those that are, by its text.
fn by_update(left: &Task, right: &Task) -> Ordering {
    let instant = |task: &Task| task.updated.parse::<Timestamp>().ok();
    match (instant(left), instant(right)) {
        (Some(left), Some(right)) => left.cmp(&right),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => left.updated.cmp(&right.updated),
    }
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
