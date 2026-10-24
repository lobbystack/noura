//! Which dated entries a range holds, and their order. Dates use
//! `YYYY-MM-DD` (all day) or RFC 3339 with an explicit offset (timed).
//!
//! These are the kernel's calendar rules, moved from `workspace-format`'s
//! `calendar.rs` with its tests, so the plugin answers the shared
//! `queries-v1` cases as the kernel's index does.

/// Invalid calendar range bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CalendarError {
    /// A bound isn't `YYYY-MM-DD` or RFC 3339 with an explicit offset.
    InvalidDate,
    /// The range's end isn't after its start.
    InvalidRange,
}

/// A dated value that a range can hold and order.
pub(crate) trait CalendarItem {
    fn start(&self) -> &str;
    fn end(&self) -> Option<&str>;
    fn all_day(&self) -> bool;
    fn title(&self) -> &str;
    fn source_id(&self) -> &str;
    fn property(&self) -> &str;
}

/// Keep the entries that overlap `[start, end)` and order them by start
/// instant, then title, source ID, and property. Entries with a malformed
/// start are dropped; malformed or non-increasing ends count as points.
pub(crate) fn select_calendar_entries<T: CalendarItem>(
    mut entries: Vec<T>,
    start: &str,
    end: &str,
) -> Result<Vec<T>, CalendarError> {
    let range = CalendarRange::parse(start, end)?;
    entries.retain(|entry| calendar_entry_overlaps(entry, range));
    // Each start parses once, not in every comparison.
    let mut keyed: Vec<(Option<jiff::Timestamp>, T)> = entries
        .into_iter()
        .map(|entry| (calendar_instant(entry.start()).ok(), entry))
        .collect();
    keyed.sort_by(|(left_at, left), (right_at, right)| {
        left_at
            .cmp(right_at)
            .then_with(|| left.title().cmp(right.title()))
            .then_with(|| left.source_id().cmp(right.source_id()))
            .then_with(|| left.property().cmp(right.property()))
    });
    Ok(keyed.into_iter().map(|(_, entry)| entry).collect())
}

/// Check calendar range bounds without selecting anything.
#[cfg(test)]
fn validate_calendar_range(start: &str, end: &str) -> Result<(), CalendarError> {
    CalendarRange::parse(start, end).map(|_| ())
}

/// The instant a value starts at: a date's midnight in UTC, or the
/// instant itself.
pub(crate) fn calendar_instant(value: &str) -> Result<jiff::Timestamp, CalendarError> {
    let normalized = if value.len() == 10 {
        format!("{value}T00:00:00Z")
    } else {
        value.to_owned()
    };
    normalized
        .parse::<jiff::Timestamp>()
        .map_err(|_| CalendarError::InvalidDate)
}

#[derive(Debug, Clone, Copy)]
struct CalendarRange {
    start_instant: jiff::Timestamp,
    end_instant: jiff::Timestamp,
    start_date: jiff::civil::Date,
    end_date: jiff::civil::Date,
}

impl CalendarRange {
    fn parse(start: &str, end: &str) -> Result<Self, CalendarError> {
        let start_instant = calendar_instant(start)?;
        let end_instant = calendar_instant(end)?;
        if start_instant >= end_instant {
            return Err(CalendarError::InvalidRange);
        }
        let start_date = calendar_date(start)?;
        let mut end_date = calendar_date(end)?;
        if !calendar_boundary_is_midnight(end) {
            end_date = end_date
                .tomorrow()
                .map_err(|_| CalendarError::InvalidDate)?;
        }
        Ok(Self {
            start_instant,
            end_instant,
            start_date,
            end_date,
        })
    }
}

/// The calendar date a value starts on, as it is written.
pub(crate) fn calendar_date(value: &str) -> Result<jiff::civil::Date, CalendarError> {
    value
        .get(..10)
        .and_then(|date| date.parse::<jiff::civil::Date>().ok())
        .ok_or(CalendarError::InvalidDate)
}

fn calendar_boundary_is_midnight(value: &str) -> bool {
    value.len() == 10
        || value.get(11..).is_some_and(|time_and_offset| {
            let offset = time_and_offset
                .find(['Z', '+', '-'])
                .unwrap_or(time_and_offset.len());
            time_and_offset[..offset]
                .parse::<jiff::civil::Time>()
                .is_ok_and(|time| time == jiff::civil::Time::midnight())
        })
}

fn calendar_entry_overlaps<T: CalendarItem>(entry: &T, range: CalendarRange) -> bool {
    if entry.all_day() {
        let Ok(start) = calendar_date(entry.start()) else {
            return false;
        };
        let end = all_day_end(entry.end(), start).or_else(|| start.tomorrow().ok());
        end.is_some_and(|end| start < range.end_date && end > range.start_date)
    } else {
        let Ok(start) = calendar_instant(entry.start()) else {
            return false;
        };
        let end = timed_end(entry.end(), start);
        end.map_or_else(
            || start >= range.start_instant && start < range.end_instant,
            |end| start < range.end_instant && end > range.start_instant,
        )
    }
}

/// An all-day entry's end, when it is a date after its first day `start`.
pub(crate) fn all_day_end(
    end: Option<&str>,
    start: jiff::civil::Date,
) -> Option<jiff::civil::Date> {
    end.filter(|value| value.len() == 10)
        .and_then(|value| calendar_date(value).ok())
        .filter(|end| *end > start)
}

/// A timed entry's end, when it is an instant after its start `start`.
pub(crate) fn timed_end(end: Option<&str>, start: jiff::Timestamp) -> Option<jiff::Timestamp> {
    end.filter(|value| value.len() != 10)
        .and_then(|value| calendar_instant(value).ok())
        .filter(|end| *end > start)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Clone, PartialEq)]
    struct Entry {
        start: String,
        end: Option<String>,
        title: String,
        source_id: String,
        property: String,
    }

    impl CalendarItem for Entry {
        fn start(&self) -> &str {
            &self.start
        }
        fn end(&self) -> Option<&str> {
            self.end.as_deref()
        }
        fn all_day(&self) -> bool {
            self.start.len() == 10
        }
        fn title(&self) -> &str {
            &self.title
        }
        fn source_id(&self) -> &str {
            &self.source_id
        }
        fn property(&self) -> &str {
            &self.property
        }
    }

    fn calendar_entry(start: &str, end: Option<&str>) -> Entry {
        Entry {
            start: start.into(),
            end: end.map(str::to_owned),
            title: "Example".into(),
            source_id: "task_example".into(),
            property: "due".into(),
        }
    }

    #[test]
    fn calendar_point_entries_use_half_open_range_boundaries() {
        let range = CalendarRange::parse("2026-09-04T12:00:00Z", "2026-09-04T13:00:00Z").unwrap();

        assert!(calendar_entry_overlaps(
            &calendar_entry("2026-09-04T12:00:00Z", None),
            range
        ));
        assert!(!calendar_entry_overlaps(
            &calendar_entry("2026-09-04T11:59:59Z", None),
            range
        ));
        assert!(!calendar_entry_overlaps(
            &calendar_entry("2026-09-04T13:00:00Z", None),
            range
        ));
    }

    #[test]
    fn calendar_timed_intervals_overlap_by_instant() {
        let range = CalendarRange::parse("2026-09-04T12:00:00Z", "2026-09-04T13:00:00Z").unwrap();

        assert!(calendar_entry_overlaps(
            &calendar_entry(
                "2026-09-04T13:30:00+02:00",
                Some("2026-09-04T14:30:00+02:00")
            ),
            range
        ));
        assert!(!calendar_entry_overlaps(
            &calendar_entry("2026-09-04T11:00:00Z", Some("2026-09-04T12:00:00Z")),
            range
        ));
        assert!(!calendar_entry_overlaps(
            &calendar_entry("2026-09-04T13:00:00Z", Some("2026-09-04T14:00:00Z")),
            range
        ));
    }

    #[test]
    fn calendar_all_day_intervals_overlap_as_civil_dates() {
        let range =
            CalendarRange::parse("2026-09-06T00:00:00-04:00", "2026-09-07T00:00:00-04:00").unwrap();

        assert!(calendar_entry_overlaps(
            &calendar_entry("2026-09-04", Some("2026-09-07")),
            range
        ));
        assert!(!calendar_entry_overlaps(
            &calendar_entry("2026-09-03", Some("2026-09-06")),
            range
        ));
        assert!(!calendar_entry_overlaps(
            &calendar_entry("2026-09-07", None),
            range
        ));
    }

    #[test]
    fn calendar_invalid_mixed_and_non_increasing_ends_fall_back_to_points() {
        let timed_range =
            CalendarRange::parse("2026-09-04T12:00:00Z", "2026-09-04T13:00:00Z").unwrap();
        let day_range = CalendarRange::parse("2026-09-04", "2026-09-05").unwrap();

        for end in [
            Some("not-a-date"),
            Some("2026-09-04"),
            Some("2026-09-04T11:00:00Z"),
        ] {
            assert!(calendar_entry_overlaps(
                &calendar_entry("2026-09-04T12:30:00Z", end),
                timed_range
            ));
        }
        for end in [
            Some("not-a-date"),
            Some("2026-09-04T18:00:00Z"),
            Some("2026-09-04"),
        ] {
            assert!(calendar_entry_overlaps(
                &calendar_entry("2026-09-04", end),
                day_range
            ));
        }
    }

    #[test]
    fn calendar_malformed_starts_are_excluded() {
        let range = CalendarRange::parse("2026-09-04", "2026-09-05").unwrap();
        assert!(!calendar_entry_overlaps(
            &calendar_entry("not-a-date", None),
            range
        ));
    }

    #[test]
    fn calendar_ranges_reject_bad_bounds() {
        assert_eq!(validate_calendar_range("2026-09-04", "2026-09-05"), Ok(()));
        assert_eq!(
            validate_calendar_range("2026-09-04T12:00:00", "2026-09-05"),
            Err(CalendarError::InvalidDate)
        );
        assert_eq!(
            select_calendar_entries(Vec::<Entry>::new(), "soon", "2026-09-05").unwrap_err(),
            CalendarError::InvalidDate
        );
        assert_eq!(
            select_calendar_entries(Vec::<Entry>::new(), "2026-09-05", "2026-09-04").unwrap_err(),
            CalendarError::InvalidRange
        );
        assert_eq!(
            select_calendar_entries(Vec::<Entry>::new(), "2026-09-05", "2026-09-05").unwrap_err(),
            CalendarError::InvalidRange
        );
    }

    /// The shared fixture's calendar cases that name an error, which no
    /// view of the plugin can ask for, fail here as the kernel fails them.
    #[test]
    fn the_queries_v1_error_cases_fail_as_the_kernel_fails_them() {
        let fixture: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../docs/workspace-format/fixtures/queries-v1.json"
        ))
        .unwrap();
        let mut checked = 0;
        for case in fixture["calendarQueries"].as_array().unwrap() {
            let Some(code) = case.get("error").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let error = validate_calendar_range(
                case["start"].as_str().unwrap(),
                case["end"].as_str().unwrap(),
            )
            .unwrap_err();
            let expected = match code {
                "invalid_calendar_range" => CalendarError::InvalidRange,
                "invalid_calendar_date" => CalendarError::InvalidDate,
                other => panic!("an error the rules don't know: {other}"),
            };
            assert_eq!(error, expected, "{case}");
            checked += 1;
        }
        assert_eq!(checked, 1);
    }

    #[test]
    fn calendar_selection_filters_and_orders_deterministically() {
        let entry = |start: &str, title: &str, source_id: &str, property: &str| Entry {
            start: start.into(),
            end: None,
            title: title.into(),
            source_id: source_id.into(),
            property: property.into(),
        };
        let selected = select_calendar_entries(
            vec![
                entry("2026-09-04T12:00:00Z", "Beta", "task_c", "due"),
                entry("2026-09-04T12:00:00Z", "Alpha", "task_b", "start"),
                entry("2026-09-04T12:00:00Z", "Alpha", "task_b", "due"),
                entry("2026-09-04T12:00:00Z", "Alpha", "task_a", "due"),
                entry("2026-09-04T13:30:00+02:00", "Earlier", "task_d", "due"),
                entry("2026-09-04", "All day", "task_e", "date"),
                entry("2026-09-05T00:00:00Z", "Outside", "task_f", "due"),
                entry("not-a-date", "Malformed", "task_g", "due"),
            ],
            "2026-09-04T00:00:00Z",
            "2026-09-05T00:00:00Z",
        )
        .unwrap();
        let order = selected
            .iter()
            .map(|entry| {
                (
                    entry.title.as_str(),
                    entry.source_id.as_str(),
                    entry.property.as_str(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            order,
            [
                ("All day", "task_e", "date"),
                ("Earlier", "task_d", "due"),
                ("Alpha", "task_a", "due"),
                ("Alpha", "task_b", "due"),
                ("Alpha", "task_b", "start"),
                ("Beta", "task_c", "due"),
            ]
        );
    }
}
