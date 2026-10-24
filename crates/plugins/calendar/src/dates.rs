//! The device's day and time zone, the bounds of the days a view shows,
//! and how a date reads in the Inbox.

use jiff::{Timestamp, ToSpan, civil::Date, tz::TimeZone};
use plugin_sdk::clock;

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

    /// The first instant of `day` in the device's time zone, in RFC 3339
    /// with the offset the zone has then, such as
    /// `2026-10-21T00:00:00-04:00`: the bound a range of days starts or
    /// ends at.
    pub(crate) fn midnight(&self, day: Date) -> String {
        match day.to_zoned(self.zone.clone()) {
            Ok(start) => {
                let offset = start.offset().seconds();
                let sign = if offset < 0 { '-' } else { '+' };
                let minutes = offset.unsigned_abs() / 60;
                format!(
                    "{}T{}{sign}{:02}:{:02}",
                    start.date(),
                    start.time().strftime("%H:%M:%S"),
                    minutes / 60,
                    minutes % 60
                )
            }
            Err(_) => format!("{day}T00:00:00Z"),
        }
    }

    /// How `start`, a date or an instant, reads in a list: Today,
    /// Tomorrow, a weekday within the week or a short date, and the time
    /// on the device's clock for an instant, alone when it falls today.
    pub(crate) fn label(&self, start: &str) -> String {
        if start.len() == 10 {
            return start
                .parse::<Date>()
                .map_or_else(|_| start.to_owned(), |day| day_name(day, self.today));
        }
        let Ok(at) = start.parse::<Timestamp>() else {
            return start.to_owned();
        };
        let local = at.to_zoned(self.zone.clone());
        let time = local.strftime("%H:%M").to_string();
        if local.date() == self.today {
            time
        } else {
            format!("{} {time}", day_name(local.date(), self.today))
        }
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

/// `day` as a list reads it, from `today`.
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

/// `day` moved by `days`, or `day` itself at the ends of the calendar.
pub(crate) fn add_days(day: Date, days: i64) -> Date {
    day.checked_add(days.days()).unwrap_or(day)
}
