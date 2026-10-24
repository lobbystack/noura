//! Dates on a project's page: this month's dated work for the project and
//! its tasks, and how a date reads. Which values count and which fall in
//! the month follow the calendar query's rules in `workspace-format`'s
//! `calendar.rs`: a `due`, `date` or `start` property, with the object's
//! `end` when it has one, kept when it overlaps the month.

use std::cmp::Ordering;

use jiff::{Timestamp, ToSpan, civil::Date, tz::TimeZone};
use plugin_sdk::clock;
use serde_json::{Map, Value};

/// The properties that date an object, in the order the page names them.
const DATED: [&str; 3] = ["due", "date", "start"];

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

/// A dated value of an object: its `due`, `date` or `start`.
#[derive(Debug, Clone)]
pub(crate) struct Dated {
    /// The stable ID of the object it dates.
    pub(crate) source: String,
    pub(crate) title: String,
    /// `due`, `date` or `start`.
    pub(crate) property: &'static str,
    /// A `YYYY-MM-DD` day for all-day work, or an RFC 3339 instant.
    pub(crate) start: String,
    pub(crate) end: Option<String>,
}

/// The dated values in an object's properties.
pub(crate) fn of(source: &str, title: &str, properties: &Map<String, Value>) -> Vec<Dated> {
    let end = properties
        .get("end")
        .and_then(Value::as_str)
        .map(str::to_owned);
    DATED
        .iter()
        .filter_map(|property| {
            let start = properties.get(*property)?.as_str()?;
            Some(Dated {
                source: source.to_owned(),
                title: title.to_owned(),
                property,
                start: start.to_owned(),
                end: end.clone(),
            })
        })
        .collect()
}

/// The month that holds today, from its first instant up to the next
/// month's, in the device's time zone.
struct Month {
    first: Date,
    next: Date,
    start: Timestamp,
    end: Timestamp,
}

impl Month {
    fn of(now: &Now) -> Option<Self> {
        let first = now.today.first_of_month();
        let next = first.checked_add(1.month()).ok()?;
        let at = |day: Date| day.to_zoned(now.zone.clone()).ok().map(|day| day.timestamp());
        Some(Self {
            first,
            next,
            start: at(first)?,
            end: at(next)?,
        })
    }

    /// Whether `dated` overlaps the month. An all-day value covers its day,
    /// or its days up to an end day after it; a timed value covers the
    /// instants up to an end after it, or its own instant. A value of
    /// neither form falls in no month.
    fn holds(&self, dated: &Dated) -> bool {
        if dated.start.len() == 10 {
            let Ok(start) = dated.start.parse::<Date>() else {
                return false;
            };
            let end = dated
                .end
                .as_deref()
                .filter(|end| end.len() == 10)
                .and_then(|end| end.parse::<Date>().ok())
                .filter(|end| *end > start)
                .or_else(|| start.tomorrow().ok());
            end.is_some_and(|end| start < self.next && end > self.first)
        } else {
            let Ok(start) = dated.start.parse::<Timestamp>() else {
                return false;
            };
            let end = dated
                .end
                .as_deref()
                .filter(|end| end.len() != 10)
                .and_then(|end| end.parse::<Timestamp>().ok())
                .filter(|end| *end > start);
            end.map_or_else(
                || start >= self.start && start < self.end,
                |end| start < self.end && end > self.start,
            )
        }
    }
}

/// Where a dated value sorts: a day at its first instant in UTC, as the
/// calendar query sorts them.
fn instant(value: &str) -> Option<Timestamp> {
    if value.len() == 10 {
        let day: Date = value.parse().ok()?;
        day.to_zoned(TimeZone::UTC).ok().map(|day| day.timestamp())
    } else {
        value.parse().ok()
    }
}

/// The values of `dated` that overlap this month, by start, then title,
/// source and property.
pub(crate) fn this_month(mut dated: Vec<Dated>, now: &Now) -> Vec<Dated> {
    let Some(month) = Month::of(now) else {
        return Vec::new();
    };
    dated.retain(|value| month.holds(value));
    dated.sort_by(|left, right| {
        instant(&left.start)
            .cmp(&instant(&right.start))
            .then_with(|| left.title.cmp(&right.title))
            .then_with(|| left.source.cmp(&right.source))
            .then_with(|| left.property.cmp(right.property))
    });
    dated
}

/// How a dated value reads, as `Due Oct 24`, `Starts Oct 1, 09:00` or
/// `Oct 3`.
pub(crate) fn label(dated: &Dated, now: &Now) -> String {
    let when = when(&dated.start, now).unwrap_or_else(|| dated.start.clone());
    match dated.property {
        "due" => format!("Due {when}"),
        "start" => format!("Starts {when}"),
        _ => when,
    }
}

/// A day or an instant as a short date, with the year when it isn't this
/// year's, and an instant's 24-hour time in the device's time zone.
pub(crate) fn when(value: &str, now: &Now) -> Option<String> {
    let day_name = |day: Date| match day.year().cmp(&now.today.year()) {
        Ordering::Equal => day.strftime("%b %-d").to_string(),
        _ => day.strftime("%b %-d, %Y").to_string(),
    };
    if value.len() == 10 {
        return value.parse::<Date>().ok().map(day_name);
    }
    let at = value
        .parse::<Timestamp>()
        .ok()?
        .to_zoned(now.zone.clone());
    Some(format!("{}, {}", day_name(at.date()), at.strftime("%H:%M")))
}
