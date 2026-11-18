//! Where a calendar grid puts the entries a plugin hands it, by the
//! legacy calendar's rules: the days each mode shows, all-day spans on
//! every day they cover, timed spans clipped to each day, lanes for
//! overlapping timed entries, and days that start at midnight on this
//! device's clock across a daylight-saving change. These are the legacy
//! `calendar.test.ts` cases, against the kit's public layout API.

use app_core::{CalendarMode, GridDay, GridEntry, Span, grid_days, lay_out};
use jiff::{Timestamp, civil::Date, tz::TimeZone};

/// Toronto's rules, which move the clocks forward on 2026-03-08 and back
/// on 2026-11-01.
fn toronto() -> TimeZone {
    TimeZone::posix("EST5EDT,M3.2.0,M11.1.0").unwrap()
}

fn date(text: &str) -> Date {
    text.parse().unwrap()
}

fn at(text: &str) -> Timestamp {
    text.parse().unwrap()
}

fn entry<'a>(title: &'a str, start: &str, end: Option<&str>) -> GridEntry<'a> {
    GridEntry {
        title,
        span: Span::parse(start, end).expect("a valid span"),
    }
}

fn titles(entries: &[GridEntry], indices: &[usize]) -> Vec<String> {
    indices
        .iter()
        .map(|&index| entries[index].title.to_owned())
        .collect()
}

#[test]
fn a_month_shows_six_weeks_from_the_sunday_on_or_before_its_first_day() {
    let days = grid_days(CalendarMode::Month, date("2026-09-16"));

    assert_eq!(days.len(), 42);
    assert_eq!(days[0], date("2026-08-30"));
    assert_eq!(days[41], date("2026-10-10"));
}

#[test]
fn a_week_runs_from_sunday_to_saturday() {
    let days = grid_days(CalendarMode::Week, date("2026-09-04"));

    assert_eq!(days.first(), Some(&date("2026-08-30")));
    assert_eq!(days.last(), Some(&date("2026-09-05")));
    assert_eq!(days.len(), 7);
}

#[test]
fn a_day_shows_only_the_entries_of_that_day() {
    let days = grid_days(CalendarMode::Day, date("2026-09-04"));
    assert_eq!(days, [date("2026-09-04")]);
    let entries = [
        entry("Yesterday", "2026-09-03", None),
        entry("Today", "2026-09-04", None),
    ];

    let laid_out = lay_out(&entries, &days, &toronto());

    assert_eq!(titles(&entries, &laid_out[0].all_day), ["Today"]);
}

#[test]
fn days_start_at_midnight_on_this_devices_clock_on_each_side_of_a_daylight_saving_change() {
    let days = lay_out(&[], &[date("2026-03-08"), date("2026-03-09")], &toronto());

    assert_eq!(days[0].start, at("2026-03-08T00:00:00-05:00"));
    assert_eq!(days[0].end, at("2026-03-09T00:00:00-04:00"));
    assert_eq!(days[1].start, at("2026-03-09T00:00:00-04:00"));
    // The day the clocks go forward lasts 23 hours.
    assert_eq!(days[0].start.duration_until(days[0].end).as_hours(), 23);
}

#[test]
fn a_timed_entry_sits_at_its_time_on_the_day_the_clocks_change() {
    let entries = [
        entry(
            "Late call",
            "2026-03-07T23:00:00-05:00",
            Some("2026-03-08T01:00:00-05:00"),
        ),
        entry(
            "Breakfast",
            "2026-03-08T09:00:00-04:00",
            Some("2026-03-08T09:30:00-04:00"),
        ),
        entry(
            "Night shift",
            "2026-03-08T23:30:00-04:00",
            Some("2026-03-09T00:30:00-04:00"),
        ),
    ];

    let days = lay_out(
        &entries,
        &grid_days(CalendarMode::Week, date("2026-03-08")),
        &toronto(),
    );

    let minutes = |day: &GridDay| -> Vec<(String, u32, u32)> {
        day.timed
            .iter()
            .map(|segment| {
                (
                    entries[segment.entry].title.to_owned(),
                    segment.start_minute,
                    segment.end_minute,
                )
            })
            .collect()
    };
    // Sunday the 8th: the clocks jump from 2:00 to 3:00, yet breakfast at
    // 9:00 still sits at 9:00.
    assert_eq!(days[0].date, date("2026-03-08"));
    assert_eq!(
        minutes(&days[0]),
        [
            ("Late call".to_owned(), 0, 60),
            ("Breakfast".to_owned(), 9 * 60, 9 * 60 + 30),
            ("Night shift".to_owned(), 23 * 60 + 30, 24 * 60),
        ]
    );
    assert_eq!(minutes(&days[1]), [("Night shift".to_owned(), 0, 30)]);
}

#[test]
fn all_day_spans_fill_each_day_they_cover_and_timed_spans_clip_to_each_day() {
    let entries = [
        entry("Retreat", "2026-09-01", Some("2026-09-04")),
        entry(
            "Deploy",
            "2026-09-02T23:00:00-04:00",
            Some("2026-09-03T02:00:00-04:00"),
        ),
    ];

    let days = lay_out(
        &entries,
        &grid_days(CalendarMode::Week, date("2026-09-02")),
        &toronto(),
    );

    let counts: Vec<usize> = days.iter().map(|day| day.all_day.len()).collect();
    assert_eq!(counts, [0, 0, 1, 1, 1, 0, 0]);
    let deploy = |day: &GridDay| {
        let segment = &day.timed[0];
        (segment.start_minute, segment.end_minute)
    };
    assert_eq!(deploy(&days[3]), (23 * 60, 24 * 60));
    assert_eq!(deploy(&days[4]), (0, 2 * 60));
    assert_eq!(days[3].timed[0].start, at("2026-09-02T23:00:00-04:00"));
    assert_eq!(days[3].timed[0].end, at("2026-09-03T00:00:00-04:00"));
}

#[test]
fn overlapping_timed_entries_take_separate_lanes() {
    let entries = [
        entry(
            "First",
            "2026-09-02T09:00:00-04:00",
            Some("2026-09-02T11:00:00-04:00"),
        ),
        entry(
            "Second",
            "2026-09-02T10:00:00-04:00",
            Some("2026-09-02T12:00:00-04:00"),
        ),
        entry(
            "Later",
            "2026-09-02T13:00:00-04:00",
            Some("2026-09-02T14:00:00-04:00"),
        ),
    ];

    let days = lay_out(&entries, &[date("2026-09-02")], &toronto());

    let lanes: Vec<(usize, usize)> = days[0]
        .timed
        .iter()
        .map(|segment| (segment.lane, segment.lanes))
        .collect();
    assert_eq!(lanes, [(0, 2), (1, 2), (0, 1)]);
}

#[test]
fn entries_without_an_end_or_with_one_not_after_the_start_take_a_day_or_thirty_minutes() {
    let point = Span::parse("2026-09-02T09:00:00-04:00", None).unwrap();
    let backwards = Span::parse(
        "2026-09-02T09:00:00-04:00",
        Some("2026-09-02T08:00:00-04:00"),
    )
    .unwrap();
    let thirty_minutes = Span::Timed {
        start: at("2026-09-02T09:00:00-04:00"),
        end: at("2026-09-02T09:30:00-04:00"),
    };
    assert_eq!(point, thirty_minutes);
    assert_eq!(backwards, thirty_minutes);

    let one_day = Span::AllDay {
        start: date("2026-09-02"),
        end: date("2026-09-03"),
    };
    assert_eq!(Span::parse("2026-09-02", None), Some(one_day));
    assert_eq!(Span::parse("2026-09-02", Some("2026-09-01")), Some(one_day));
}

#[test]
fn a_start_or_an_end_of_the_wrong_form_isnt_a_span() {
    assert_eq!(Span::parse("2026-09-02 09:00", None), None);
    assert_eq!(Span::parse("2026-02-30", None), None);
    assert_eq!(
        Span::parse("2026-09-02", Some("2026-09-03T09:00:00-04:00")),
        None
    );
    assert_eq!(
        Span::parse("2026-09-02T09:00:00-04:00", Some("2026-09-03")),
        None
    );
}

#[test]
fn entries_on_a_day_keep_the_legacy_order_by_start_then_title() {
    let entries = [
        entry("Zebra", "2026-09-02", None),
        entry("Apple", "2026-09-02", None),
        entry("Earlier", "2026-09-01", Some("2026-09-03")),
    ];

    let days = lay_out(&entries, &[date("2026-09-02")], &toronto());

    assert_eq!(
        titles(&entries, &days[0].all_day),
        ["Earlier", "Apple", "Zebra"]
    );
}
