//! The popover beside the entry you select. An event of yours edits in
//! place, each change written to its file as you make it: its title, its
//! day and times or its days, All day, its calendar and its notes, with
//! Delete under them. Any other entry shows what it is, when, a field or
//! two such as a meeting's attendees or a task's status, and Open.
//!
//! Times step by 15 minutes and read on a 24-hour clock, as the Inbox's
//! do. An end on another day than the start names its weekday.

use jiff::{
    SignedDuration, Timestamp,
    civil::{Date, Time},
};
use plugin_sdk::{
    Color, EditableStyle, Error, Value, View,
    objects::{self, Changes, NewObject},
};
use serde_json::{Map, Value as Json, json};

use crate::{
    calendars::{self, Own},
    dates::{Now, add_days, day_name},
    entries::Entry,
    names,
    types::EVENT,
};

/// The popover's node ID, and the IDs of what it holds.
pub(crate) const POPOVER: &str = "details";
pub(crate) const TITLE: &str = "event-title";
pub(crate) const DAY: &str = "event-day";
pub(crate) const LAST: &str = "event-last";
pub(crate) const START: &str = "event-start";
pub(crate) const END: &str = "event-end";
pub(crate) const ALL_DAY: &str = "event-all-day";
pub(crate) const CALENDAR: &str = "event-calendar";
pub(crate) const NOTES: &str = "event-notes";
pub(crate) const DELETE: &str = "delete-event";
pub(crate) const OPEN: &str = "open";

/// The title a new event takes, and the calendar Noura makes for your
/// first event when you have none.
const NEW_EVENT: &str = "New event";
const FIRST_CALENDAR: &str = "Calendar";

/// How far apart the times to pick from are, and how long an event
/// without an end lasts, as the grid draws it.
const STEP: i64 = 15;
const POINT: i64 = 30;

/// When an event happens, as its file holds it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Span {
    /// Its first day and the day after its last.
    AllDay {
        first: Date,
        after: Date,
    },
    Timed {
        start: Timestamp,
        end: Timestamp,
    },
}

impl Span {
    /// The span of an event whose file holds `properties`, or none
    /// without a start of either form. An end that isn't after the start
    /// takes one day, or 30 minutes.
    pub(crate) fn of(properties: &Map<String, Json>) -> Option<Self> {
        let text = |key: &str| properties.get(key).and_then(Json::as_str);
        let start = text("start")?;
        if start.len() == 10 {
            let first: Date = start.parse().ok()?;
            let after = text("end")
                .and_then(|end| end.parse::<Date>().ok())
                .filter(|after| *after > first)
                .unwrap_or_else(|| add_days(first, 1));
            return Some(Self::AllDay { first, after });
        }
        let start: Timestamp = start.parse().ok()?;
        let end = text("end")
            .and_then(|end| end.parse::<Timestamp>().ok())
            .filter(|end| *end > start)
            .unwrap_or_else(|| start + SignedDuration::from_mins(POINT));
        Some(Self::Timed { start, end })
    }

    /// The `start` and `end` the file holds. A one-day event holds no
    /// end.
    fn written(self, now: &Now) -> (String, Option<String>) {
        match self {
            Self::AllDay { first, after } => (
                first.to_string(),
                (after > add_days(first, 1)).then(|| after.to_string()),
            ),
            Self::Timed { start, end } => (now.stamp(start), Some(now.stamp(end))),
        }
    }

    /// The span moved to start on `day`, as long as before, at the same
    /// time of day.
    fn on_day(self, now: &Now, day: Date) -> Self {
        match self {
            Self::AllDay { first, after } => {
                let length = (after - first).get_days();
                Self::AllDay {
                    first: day,
                    after: add_days(day, i64::from(length)),
                }
            }
            Self::Timed { start, end } => {
                let moved = at(now, day, now.local(start).time()).unwrap_or(start);
                Self::Timed {
                    start: moved,
                    end: moved + start.duration_until(end),
                }
            }
        }
    }

    /// An all-day span that ends on `last`, or on its first day when
    /// `last` is none or before it.
    fn ending(self, last: Option<Date>) -> Self {
        match self {
            Self::AllDay { first, .. } => Self::AllDay {
                first,
                after: add_days(last.filter(|last| *last >= first).unwrap_or(first), 1),
            },
            timed => timed,
        }
    }

    /// A timed span that starts at `time` on its day, as long as before.
    fn starting_at(self, now: &Now, time: Time) -> Self {
        match self {
            Self::Timed { start, end } => {
                let day = now.local(start).date();
                let moved = at(now, day, time).unwrap_or(start);
                Self::Timed {
                    start: moved,
                    end: moved + start.duration_until(end),
                }
            }
            all_day => all_day,
        }
    }

    /// A timed span that ends `minutes` after it starts.
    fn lasting(self, minutes: i64) -> Self {
        match self {
            Self::Timed { start, .. } if minutes > 0 => Self::Timed {
                start,
                end: start + SignedDuration::from_mins(minutes),
            },
            span => span,
        }
    }

    /// The span all day on the days it covers, or from 9:00 to 10:00 on
    /// its first day.
    fn all_day(self, now: &Now, all_day: bool) -> Self {
        match (self, all_day) {
            (Self::Timed { start, end }, true) => {
                let first = now.local(start).date();
                // An end at midnight ends the day before.
                let last = now
                    .local(end - SignedDuration::from_nanos(1))
                    .date()
                    .max(first);
                Self::AllDay {
                    first,
                    after: add_days(last, 1),
                }
            }
            (Self::AllDay { first, .. }, false) => {
                let nine = Time::constant(9, 0, 0, 0);
                let start = at(now, first, nine).unwrap_or(Timestamp::UNIX_EPOCH);
                Self::Timed {
                    start,
                    end: start + SignedDuration::from_hours(1),
                }
            }
            (span, _) => span,
        }
    }
}

/// The instant `time` on `day` in the device's time zone.
fn at(now: &Now, day: Date, time: Time) -> Option<Timestamp> {
    day.to_datetime(time)
        .to_zoned(now.zone.clone())
        .ok()
        .map(|at| at.timestamp())
}

/// `at` on the device's clock, as a picker reads it.
fn clock(now: &Now, at: Timestamp) -> String {
    now.local(at).strftime("%H:%M").to_string()
}

/// The times an event that starts at `start` can start at on its day, by
/// their `HH:MM`, with its own among them.
fn start_times(now: &Now, start: Timestamp) -> Vec<(String, String)> {
    let own = now.local(start).time();
    let mut times: Vec<Time> = (0..24 * 60 / STEP)
        .filter_map(|step| {
            let minutes = step * STEP;
            Time::new((minutes / 60) as i8, (minutes % 60) as i8, 0, 0).ok()
        })
        .collect();
    let own = own
        .with()
        .second(0)
        .subsec_nanosecond(0)
        .build()
        .unwrap_or(own);
    if !times.contains(&own) {
        times.push(own);
        times.sort();
    }
    times
        .into_iter()
        .map(|time| {
            let id = time.strftime("%H:%M").to_string();
            (id.clone(), id)
        })
        .collect()
}

/// The ends an event that starts at `start` can take: every 15 minutes
/// for a day, by the minutes after the start, with its own end among
/// them. An end on another day names its weekday.
fn end_times(now: &Now, start: Timestamp, end: Timestamp) -> Vec<(String, String)> {
    let first = now.local(start).date();
    let mut lengths: Vec<i64> = (1..=24 * 60 / STEP).map(|step| step * STEP).collect();
    let own = start.duration_until(end).as_mins();
    if own > 0 && !lengths.contains(&own) {
        lengths.push(own);
        lengths.sort_unstable();
    }
    lengths
        .into_iter()
        .map(|minutes| {
            let at = start + SignedDuration::from_mins(minutes);
            let local = now.local(at);
            let label = if local.date() == first {
                clock(now, at)
            } else {
                format!("{} {}", local.strftime("%a"), clock(now, at))
            };
            (format!("{minutes}"), label)
        })
        .collect()
}

/// Draw the popover of the entry `entry` beside it: the editable details
/// of an event of yours, whose file holds `body`, with `own` as the
/// calendars to choose from, or what any other entry is.
pub(crate) fn render(
    view: &mut View,
    now: &Now,
    entry: &Entry,
    event: Option<(&str, &[Own], Option<&str>)>,
    notice: Option<&str>,
) {
    let popover = view.popover(View::ROOT, POPOVER, &entry.title, &entry.key);
    match event {
        Some((body, own, calendar)) => {
            event_details(view, popover, now, entry, (body, own, calendar))
        }
        None => entry_details(view, popover, now, entry),
    }
    if let Some(notice) = notice {
        view.text(popover, "details-notice", notice);
    }
}

fn event_details(
    view: &mut View,
    popover: u32,
    now: &Now,
    entry: &Entry,
    (body, own, calendar): (&str, &[Own], Option<&str>),
) {
    view.editable(
        popover,
        TITLE,
        "Title",
        &entry.title,
        EditableStyle::Title,
        false,
    );
    let span = Span::of(&entry.properties);
    match span {
        Some(Span::AllDay { first, after }) => {
            view.date_picker(popover, DAY, "Starts", Some(&first.to_string()));
            let last = add_days(after, -1);
            view.date_picker(popover, LAST, "Ends", Some(&last.to_string()));
        }
        Some(Span::Timed { start, end }) => {
            let day = now.local(start).date().to_string();
            view.date_picker(popover, DAY, "Date", Some(&day));
            let starts = start_times(now, start);
            let starts: Vec<(&str, &str, Option<Color>)> = starts
                .iter()
                .map(|(id, label)| (id.as_str(), label.as_str(), None))
                .collect();
            view.picker(popover, START, "Starts", &starts, Some(&clock(now, start)));
            let ends = end_times(now, start, end);
            let ends: Vec<(&str, &str, Option<Color>)> = ends
                .iter()
                .map(|(id, label)| (id.as_str(), label.as_str(), None))
                .collect();
            let length = start.duration_until(end).as_mins().to_string();
            view.picker(popover, END, "Ends", &ends, Some(&length));
        }
        None => {}
    }
    let all_day = matches!(span, Some(Span::AllDay { .. }));
    view.input(popover, ALL_DAY, "All day", Value::Checkbox(all_day));
    if !own.is_empty() {
        let options: Vec<(&str, &str, Option<Color>)> = own
            .iter()
            .map(|calendar| {
                (
                    calendar.id.as_str(),
                    calendar.name.as_str(),
                    Some(calendar.color),
                )
            })
            .collect();
        let chosen = calendar
            .filter(|id| own.iter().any(|calendar| calendar.id == *id))
            .unwrap_or(own[0].id.as_str());
        view.picker(popover, CALENDAR, "Calendar", &options, Some(chosen));
    }
    view.editable(popover, NOTES, "Notes", body, EditableStyle::Body, false);
    view.button(popover, DELETE, "Delete");
}

/// What the entry is, as its popover names it: a meeting, a task's due
/// date, or its type's name.
fn kind(entry: &Entry) -> String {
    match (entry.source_type.as_str(), entry.property.as_str()) {
        ("transcription", _) => "Meeting".to_owned(),
        ("task", "due") => "Task due".to_owned(),
        (object_type, _) => {
            let mut letters = object_type.chars();
            letters.next().map_or_else(String::new, |first| {
                format!("{}{}", first.to_uppercase(), letters.as_str()).replace('-', " ")
            })
        }
    }
}

/// When the entry happens, in words: its day or days, and its times.
fn when(now: &Now, entry: &Entry) -> Option<String> {
    let mut properties = Map::new();
    properties.insert("start".into(), json!(entry.start));
    if let Some(end) = &entry.end {
        properties.insert("end".into(), json!(end));
    }
    let day = |day: Date| day_name(day, now.today);
    Some(match Span::of(&properties)? {
        Span::AllDay { first, after } if after == add_days(first, 1) => day(first),
        Span::AllDay { first, after } => {
            format!("{} to {}", day(first), day(add_days(after, -1)))
        }
        Span::Timed { start, .. } if entry.end.is_none() => {
            format!("{}, {}", day(now.local(start).date()), clock(now, start))
        }
        Span::Timed { start, end } => {
            let (first, last) = (now.local(start).date(), now.local(end).date());
            if first == last {
                format!(
                    "{}, {} to {}",
                    day(first),
                    clock(now, start),
                    clock(now, end)
                )
            } else {
                format!(
                    "{}, {} to {}, {}",
                    day(first),
                    clock(now, start),
                    day(last),
                    clock(now, end)
                )
            }
        }
    })
}

/// The status names of tasks and projects.
const STATUSES: [(&str, &str); 6] = [
    ("todo", "To do"),
    ("in-progress", "In progress"),
    ("done", "Done"),
    ("cancelled", "Cancelled"),
    ("active", "Active"),
    ("archived", "Archived"),
];

fn entry_details(view: &mut View, popover: u32, now: &Now, entry: &Entry) {
    view.section_heading(popover, "entry-title", &entry.title, None);
    view.text(popover, "entry-kind", &kind(entry));
    if let Some(when) = when(now, entry) {
        view.text(popover, "entry-when", &when);
    }
    let attendees: Vec<&str> = entry
        .properties
        .get("attendees")
        .and_then(Json::as_array)
        .map(|names| names.iter().filter_map(Json::as_str).collect())
        .unwrap_or_default();
    if !attendees.is_empty() {
        view.text(
            popover,
            "entry-attendees",
            &format!("With {}", join(&attendees)),
        );
    }
    let status = entry
        .properties
        .get("status")
        .and_then(Json::as_str)
        .and_then(|status| STATUSES.iter().find(|(id, _)| *id == status))
        .map(|(_, name)| *name);
    if let Some(status) = status {
        view.text(popover, "entry-status", status);
    }
    if entry.shared_id {
        // Open goes by stable ID, which names either file.
        view.text(
            popover,
            "entry-shared-id",
            "Another file has the same ID, so Open might show that file.",
        );
    }
    view.button(popover, OPEN, "Open");
}

/// `names` as a sentence lists them: `Ada`, `Ada and Grace`, or `Ada,
/// Grace and Alan`.
fn join(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// A change you made in an event's popover.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Edit {
    Title(String),
    Day(Date),
    Last(Option<Date>),
    Start(Time),
    /// The minutes the event lasts.
    Length(i64),
    AllDay(bool),
    Calendar(String),
    Notes(String),
}

impl Edit {
    /// The change the popover's control `node` sends with `value`.
    pub(crate) fn of(node: &str, value: Value) -> Option<Self> {
        match (node, value) {
            (TITLE, Value::Text(title)) => Some(Self::Title(title)),
            (DAY, Value::Date(Some(day))) => day.parse().ok().map(Self::Day),
            (LAST, Value::Date(day)) => Some(Self::Last(day.and_then(|day| day.parse().ok()))),
            (START, Value::Choice(Some(time))) => time.parse().ok().map(Self::Start),
            (END, Value::Choice(Some(minutes))) => minutes.parse().ok().map(Self::Length),
            (ALL_DAY, Value::Checkbox(all_day)) => Some(Self::AllDay(all_day)),
            (CALENDAR, Value::Choice(Some(calendar))) => Some(Self::Calendar(calendar)),
            (NOTES, Value::Multiline(notes) | Value::Text(notes)) => Some(Self::Notes(notes)),
            _ => None,
        }
    }
}

/// Write `edit` to the file of the event `id`, which holds `properties`.
pub(crate) fn apply(
    now: &Now,
    id: &str,
    properties: &Map<String, Json>,
    edit: Edit,
) -> Result<(), String> {
    let latest = objects::get(id).map_err(|error| problem(&error))?;
    let span = Span::of(properties);
    let moved = match &edit {
        Edit::Day(day) => span.map(|span| span.on_day(now, *day)),
        Edit::Last(last) => span.map(|span| span.ending(*last)),
        Edit::Start(time) => span.map(|span| span.starting_at(now, *time)),
        Edit::Length(minutes) => span.map(|span| span.lasting(*minutes)),
        Edit::AllDay(all_day) => span.map(|span| span.all_day(now, *all_day)),
        Edit::Title(_) | Edit::Calendar(_) | Edit::Notes(_) => None,
    };
    let changes = match edit {
        Edit::Title(title) => {
            let title = names::file_name(&title);
            if title.is_empty() || title == latest.title {
                return Ok(());
            }
            return objects::move_to(id, &latest.revision, &names::renamed(&latest.path, &title))
                .map(|_| ())
                .map_err(|error| problem(&error));
        }
        Edit::Calendar(calendar) => Changes {
            body: None,
            properties: json!({ "calendar": calendar }).to_string(),
            remove: Vec::new(),
        },
        Edit::Notes(notes) => Changes {
            body: Some(notes),
            properties: "{}".to_owned(),
            remove: Vec::new(),
        },
        _ => {
            let Some(span) = moved else {
                return Ok(());
            };
            let (start, end) = span.written(now);
            let mut set = json!({ "start": start });
            if let Some(end) = &end {
                set["end"] = json!(end);
            }
            Changes {
                body: None,
                properties: set.to_string(),
                remove: if end.is_none() {
                    vec!["end".to_owned()]
                } else {
                    Vec::new()
                },
            }
        }
    };
    objects::update(id, &latest.revision, &changes)
        .map(|_| ())
        .map_err(|error| problem(&error))
}

/// Create an event from 9:00 to 10:00 on `day` in `calendar`, or in your
/// first calendar, which Noura makes when you have none, and return its
/// stable ID.
pub(crate) fn create(
    now: &Now,
    day: Date,
    calendar: Option<String>,
    own: &[Own],
) -> Result<String, String> {
    let calendar = match calendar.or_else(|| own.first().map(|calendar| calendar.id.clone())) {
        Some(id) => id,
        None => calendars::create(FIRST_CALENDAR, own).map_err(|error| problem(&error))?,
    };
    let start = now.at(day, Time::constant(9, 0, 0, 0));
    let end = now.at(day, Time::constant(10, 0, 0, 0));
    let properties = json!({ "calendar": calendar, "start": start, "end": end });
    // Noura numbers the name after one that's taken.
    objects::create(&NewObject {
        object_type: EVENT.to_owned(),
        title: NEW_EVENT.to_owned(),
        path: None,
        body: String::new(),
        properties: properties.to_string(),
    })
    .map(|object| object.id)
    .map_err(|error| problem(&error))
}

/// The notes of the event `id`, as its file's body holds them.
pub(crate) fn notes(id: &str) -> String {
    objects::get(id)
        .map(|object| object.body)
        .unwrap_or_default()
}

/// Move the event `id` to the trash.
pub(crate) fn delete(id: &str) -> Result<(), String> {
    let latest = objects::get(id).map_err(|error| problem(&error))?;
    objects::delete(id, &latest.revision).map_err(|error| problem(&error))
}

/// What a failed write says, in one sentence.
pub(crate) fn problem(error: &Error) -> String {
    match error {
        Error::RevisionConflict(_) => "The event changed in another app. Try again.".to_owned(),
        Error::AlreadyExists(_) => "Another file in its folder has that name.".to_owned(),
        Error::NotFound(_) => "The event isn't in the workspace anymore.".to_owned(),
        Error::IdentityConflict(_) => {
            "Another file has the event's ID. Remove the copy, then try again.".to_owned()
        }
        _ => "Noura couldn't save the event. Try again.".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use jiff::tz::TimeZone;

    use super::*;

    fn toronto() -> Now {
        Now {
            zone: TimeZone::get("America/Toronto").unwrap(),
            today: "2026-11-21".parse().unwrap(),
        }
    }

    fn span(start: &str, end: Option<&str>) -> Span {
        let mut properties = Map::new();
        properties.insert("start".into(), json!(start));
        if let Some(end) = end {
            properties.insert("end".into(), json!(end));
        }
        Span::of(&properties).unwrap()
    }

    fn written(span: Span) -> (String, Option<String>) {
        span.written(&toronto())
    }

    #[test]
    fn a_timed_event_moves_to_another_day_at_the_same_time_and_length() {
        let now = toronto();
        let moved = span(
            "2026-11-21T09:00:00-05:00",
            Some("2026-11-21T10:30:00-05:00"),
        )
        .on_day(&now, "2026-11-24".parse().unwrap());
        assert_eq!(
            written(moved),
            (
                "2026-11-24T09:00:00-05:00".into(),
                Some("2026-11-24T10:30:00-05:00".into())
            )
        );
    }

    #[test]
    fn a_new_start_time_keeps_the_length_and_a_length_sets_the_end() {
        let now = toronto();
        let start = span(
            "2026-11-21T09:00:00-05:00",
            Some("2026-11-21T10:00:00-05:00"),
        )
        .starting_at(&now, Time::constant(14, 15, 0, 0));
        assert_eq!(
            written(start),
            (
                "2026-11-21T14:15:00-05:00".into(),
                Some("2026-11-21T15:15:00-05:00".into())
            )
        );
        let longer = start.lasting(90);
        assert_eq!(
            written(longer).1.as_deref(),
            Some("2026-11-21T15:45:00-05:00")
        );
        assert_eq!(start.lasting(0), start);
    }

    #[test]
    fn all_day_covers_the_days_and_back_to_nine_to_ten() {
        let now = toronto();
        let overnight = span(
            "2026-11-21T22:00:00-05:00",
            Some("2026-11-23T00:00:00-05:00"),
        );
        let days = overnight.all_day(&now, true);
        // An end at midnight ends the day before.
        assert_eq!(
            written(days),
            ("2026-11-21".into(), Some("2026-11-23".into()))
        );
        let timed = days.all_day(&now, false);
        assert_eq!(
            written(timed),
            (
                "2026-11-21T09:00:00-05:00".into(),
                Some("2026-11-21T10:00:00-05:00".into())
            )
        );
    }

    #[test]
    fn an_all_day_event_takes_its_last_day_and_holds_no_end_for_one_day() {
        let retreat = span("2026-11-21", Some("2026-11-24"));
        assert_eq!(
            retreat,
            Span::AllDay {
                first: "2026-11-21".parse().unwrap(),
                after: "2026-11-24".parse().unwrap()
            }
        );
        let one = retreat.ending(None);
        assert_eq!(written(one), ("2026-11-21".into(), None));
        let before = retreat.ending(Some("2026-11-20".parse().unwrap()));
        assert_eq!(before, one);
        let moved = retreat.on_day(&toronto(), "2026-12-01".parse().unwrap());
        assert_eq!(
            written(moved),
            ("2026-12-01".into(), Some("2026-12-04".into()))
        );
    }

    #[test]
    fn an_end_not_after_the_start_takes_thirty_minutes() {
        let point = span(
            "2026-11-21T09:00:00-05:00",
            Some("2026-11-21T08:00:00-05:00"),
        );
        assert_eq!(
            written(point).1.as_deref(),
            Some("2026-11-21T09:30:00-05:00")
        );
    }

    #[test]
    fn times_step_by_fifteen_minutes_and_keep_the_events_own() {
        let now = toronto();
        let start: Timestamp = "2026-11-21T14:07:00-05:00".parse().unwrap();
        let starts = start_times(&now, start);
        assert_eq!(starts.len(), 97);
        assert!(starts.iter().any(|(id, _)| id == "14:07"));
        let ends = end_times(&now, start, start + SignedDuration::from_mins(50));
        assert_eq!(ends[0], ("15".into(), "14:22".into()));
        assert!(ends.iter().any(|(id, _)| id == "50"));
        // An end past midnight names its day.
        assert!(
            ends.iter()
                .any(|(id, label)| id == "600" && label == "Sun 00:07")
        );
    }

    #[test]
    fn changes_come_from_the_popovers_controls() {
        assert_eq!(
            Edit::of(START, Value::Choice(Some("08:45".into()))),
            Some(Edit::Start(Time::constant(8, 45, 0, 0)))
        );
        assert_eq!(
            Edit::of(END, Value::Choice(Some("90".into()))),
            Some(Edit::Length(90))
        );
        assert_eq!(Edit::of(LAST, Value::Date(None)), Some(Edit::Last(None)));
        assert_eq!(Edit::of(DAY, Value::Date(None)), None);
    }

    #[test]
    fn attendees_read_as_a_sentence() {
        assert_eq!(join(&["Ada"]), "Ada");
        assert_eq!(join(&["Ada", "Grace"]), "Ada and Grace");
        assert_eq!(join(&["Ada", "Grace", "Alan"]), "Ada, Grace and Alan");
    }
}
