//! Where a calendar grid puts the entries a plugin hands it. The rules are
//! the legacy calendar's: a month shows six weeks from the Sunday on or
//! before its first day, and a week runs from Sunday to Saturday. An
//! all-day entry shows on each day it covers; a timed entry shows on each
//! day it touches, clipped to that day, and overlapping timed entries take
//! separate lanes. Days start at midnight in a time zone, normally this
//! device's, so a day the clocks change lasts 23 or 25 hours.
//!
//! One change from the legacy calendar: a timed entry sits at its time on
//! the day's clock, so 9:00 stays at 9:00 on the day the clocks change.

use jiff::{SignedDuration, Timestamp, ToSpan, civil::Date, tz::TimeZone};
use plugin_host::CalendarMode;

/// How long an entry without a usable end lasts when it has a time.
const POINT: SignedDuration = SignedDuration::from_mins(30);

/// The minutes in a day on its clock, where a day's segments end.
const DAY_MINUTES: u32 = 24 * 60;

/// When an entry happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Span {
    /// Every day from `start` up to but not including `end`.
    AllDay { start: Date, end: Date },
    /// From `start` up to `end`.
    Timed { start: Timestamp, end: Timestamp },
}

impl Span {
    /// The span from `start` to `end`: `YYYY-MM-DD` dates for an all-day
    /// entry, or RFC 3339 dates and times with an offset for a timed one.
    /// Without an end, or with one not after the start, an all-day entry
    /// takes its first day and a timed entry 30 minutes. `None` when the
    /// start has neither form or the end has another form than the start.
    pub fn parse(start: &str, end: Option<&str>) -> Option<Self> {
        if workspace_format::is_civil_date(start) {
            let start: Date = start.parse().ok()?;
            let end = match end {
                Some(end) if workspace_format::is_civil_date(end) => end.parse::<Date>().ok()?,
                Some(_) => return None,
                None => start,
            };
            let end = if end > start {
                end
            } else {
                start.tomorrow().ok()?
            };
            return Some(Self::AllDay { start, end });
        }
        if !workspace_format::is_instant(start) {
            return None;
        }
        let start: Timestamp = start.parse().ok()?;
        let end = match end {
            Some(end) if workspace_format::is_instant(end) => Some(end.parse::<Timestamp>().ok()?),
            Some(_) => return None,
            None => None,
        };
        let end = match end {
            Some(end) if end > start => end,
            _ => start.checked_add(POINT).ok()?,
        };
        Some(Self::Timed { start, end })
    }
}

/// An entry to lay out: its title, which orders entries that start
/// together, and when it happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GridEntry<'a> {
    pub title: &'a str,
    pub span: Span,
}

/// One day of a grid and the entries on it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GridDay {
    pub date: Date,
    /// Midnight at the day's start and at its end, in the time zone.
    pub start: Timestamp,
    pub end: Timestamp,
    /// The all-day entries that cover the day, as indices into the
    /// entries, by first day, then title.
    pub all_day: Vec<usize>,
    /// The timed entries that touch the day, clipped to it, by start, then
    /// title.
    pub timed: Vec<Segment>,
}

/// The part of a timed entry on one day.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Segment {
    /// The entry's index into the entries.
    pub entry: usize,
    /// The entry's start and end, clipped to the day.
    pub start: Timestamp,
    pub end: Timestamp,
    /// Where the segment starts and ends on the day's clock, in minutes
    /// after midnight. A segment that runs to the end of the day ends at
    /// 1440.
    pub start_minute: u32,
    pub end_minute: u32,
    /// The segment's lane among those it overlaps, from 0, and how many
    /// lanes they take.
    pub lane: usize,
    pub lanes: usize,
}

/// The days a grid shows in `mode` around `date`: 42 from the Sunday on or
/// before the first of its month, 7 from the Sunday on or before it, or
/// the day alone.
pub fn grid_days(mode: CalendarMode, date: Date) -> Vec<Date> {
    let sunday_before = |date: Date| {
        let back = i64::from(date.weekday().to_sunday_zero_offset());
        date.checked_sub(back.days()).unwrap_or(date)
    };
    let (first, count) = match mode {
        CalendarMode::Month => (sunday_before(date.first_of_month()), 42),
        CalendarMode::Week => (sunday_before(date), 7),
        CalendarMode::Day => (date, 1),
    };
    first.series(1.day()).take(count).collect()
}

/// Lay out `entries` on `days`, whose midnights fall in `time_zone`.
pub fn lay_out(entries: &[GridEntry], days: &[Date], time_zone: &TimeZone) -> Vec<GridDay> {
    days.iter()
        .map(|&date| lay_out_day(entries, date, time_zone))
        .collect()
}

/// The first instant of `date` in `time_zone`: midnight, or the end of a
/// gap the clocks skip over at midnight.
fn midnight(date: Date, time_zone: &TimeZone) -> Timestamp {
    date.to_zoned(time_zone.clone())
        .map_or(Timestamp::MIN, |zoned| zoned.timestamp())
}

fn lay_out_day(entries: &[GridEntry], date: Date, time_zone: &TimeZone) -> GridDay {
    let start = midnight(date, time_zone);
    let end = date
        .tomorrow()
        .map_or(Timestamp::MAX, |next| midnight(next, time_zone));
    let order = |a: &usize, b: &usize| entries[*a].title.cmp(entries[*b].title).then(a.cmp(b));

    let mut all_day: Vec<(Date, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| match entry.span {
            Span::AllDay {
                start: first,
                end: after,
            } if first <= date && date < after => Some((first, index)),
            _ => None,
        })
        .collect();
    all_day.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| order(&a.1, &b.1)));

    let mut timed: Vec<(Timestamp, usize, Timestamp, Timestamp)> = entries
        .iter()
        .enumerate()
        .filter_map(|(index, entry)| match entry.span {
            Span::Timed {
                start: from,
                end: to,
            } if from < end && to > start => Some((from, index, from.max(start), to.min(end))),
            _ => None,
        })
        .collect();
    timed.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| order(&a.1, &b.1)));
    let minute = |at: Timestamp, boundary: u32| {
        if at == start {
            0
        } else if at == end {
            boundary
        } else {
            let time = at.to_zoned(time_zone.clone()).time();
            u32::from(time.hour().unsigned_abs()) * 60 + u32::from(time.minute().unsigned_abs())
        }
    };
    let mut segments: Vec<Segment> = timed
        .into_iter()
        .map(|(_, entry, from, to)| {
            let start_minute = minute(from, 0);
            Segment {
                entry,
                start: from,
                end: to,
                start_minute,
                end_minute: minute(to, DAY_MINUTES).max(start_minute),
                lane: 0,
                lanes: 1,
            }
        })
        .collect();
    assign_lanes(&mut segments);

    GridDay {
        date,
        start,
        end,
        all_day: all_day.into_iter().map(|(_, index)| index).collect(),
        timed: segments,
    }
}

/// Put each run of overlapping segments in lanes: each takes the first
/// lane free at its start, and the run shares its count of lanes.
/// `segments` are in order of start.
fn assign_lanes(segments: &mut [Segment]) {
    let mut group_start = 0;
    while group_start < segments.len() {
        let mut group_end = group_start + 1;
        let mut latest_end = segments[group_start].end;
        while group_end < segments.len() && segments[group_end].start < latest_end {
            latest_end = latest_end.max(segments[group_end].end);
            group_end += 1;
        }
        let mut lane_ends: Vec<Timestamp> = Vec::new();
        for segment in &mut segments[group_start..group_end] {
            let free = lane_ends.iter().position(|end| *end <= segment.start);
            segment.lane = free.unwrap_or(lane_ends.len());
            if segment.lane == lane_ends.len() {
                lane_ends.push(segment.end);
            } else {
                lane_ends[segment.lane] = segment.end;
            }
        }
        for segment in &mut segments[group_start..group_end] {
            segment.lanes = lane_ends.len();
        }
        group_start = group_end;
    }
}
