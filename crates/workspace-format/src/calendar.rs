//! Calendar range filtering and ordering. Dates use `YYYY-MM-DD` (all day) or
//! RFC 3339 with an explicit offset (timed).

use thiserror::Error;

/// Invalid calendar query bounds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CalendarError {
    #[error("Calendar values must use YYYY-MM-DD or RFC 3339 with an explicit offset")]
    InvalidDate,
    #[error("The calendar range end must be after its start")]
    InvalidRange,
}

/// A dated value that a calendar query can place and order.
pub trait CalendarItem {
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
pub fn select_calendar_entries<T: CalendarItem>(
    mut entries: Vec<T>,
    start: &str,
    end: &str,
) -> Result<Vec<T>, CalendarError> {
    let range = CalendarRange::parse(start, end)?;
    entries.retain(|entry| calendar_entry_overlaps(entry, range));
    entries.sort_by(|left, right| {
        calendar_instant(left.start())
            .ok()
            .cmp(&calendar_instant(right.start()).ok())
            .then_with(|| left.title().cmp(right.title()))
            .then_with(|| left.source_id().cmp(right.source_id()))
            .then_with(|| left.property().cmp(right.property()))
    });
    Ok(entries)
}

/// Check calendar query bounds without selecting anything.
pub fn validate_calendar_range(start: &str, end: &str) -> Result<(), CalendarError> {
    CalendarRange::parse(start, end).map(|_| ())
}

fn calendar_instant(value: &str) -> Result<jiff::Timestamp, CalendarError> {
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

fn calendar_date(value: &str) -> Result<jiff::civil::Date, CalendarError> {
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
        let end = entry
            .end()
            .filter(|value| value.len() == 10)
            .and_then(|value| calendar_date(value).ok())
            .filter(|end| *end > start)
            .or_else(|| start.tomorrow().ok());
        end.is_some_and(|end| start < range.end_date && end > range.start_date)
    } else {
        let Ok(start) = calendar_instant(entry.start()) else {
            return false;
        };
        let end = entry
            .end()
            .filter(|value| value.len() != 10)
            .and_then(|value| calendar_instant(value).ok())
            .filter(|end| *end > start);
        end.map_or_else(
            || start >= range.start_instant && start < range.end_instant,
            |end| start < range.end_instant && end > range.start_instant,
        )
    }
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
