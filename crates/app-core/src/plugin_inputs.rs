//! The values of a plugin's date and date-and-time inputs, between the
//! text fields Noura shows and the forms the plugin interface promises:
//! `YYYY-MM-DD` for a date, and RFC 3339 with an explicit offset for a date
//! and time.

use jiff::{
    Timestamp,
    civil::{Date, DateTime, Time},
    fmt::temporal::Pieces,
    tz::{Offset, TimeZone},
};

/// The value of a date field: `Some(None)` when it is empty, `Some(date)`
/// for a `YYYY-MM-DD` date, and `None` for anything else.
pub fn date_value(text: &str) -> Option<Option<String>> {
    let text = text.trim();
    if text.is_empty() {
        Some(None)
    } else if workspace_format::is_civil_date(text) {
        Some(Some(text.to_owned()))
    } else {
        None
    }
}

/// The date field and the time field that show `value`, an RFC 3339 date
/// and time, as it reads in its own offset: `YYYY-MM-DD`, and `HH:MM` or
/// `HH:MM:SS` when the seconds aren't zero.
pub fn date_time_fields(value: &str) -> Option<(String, String)> {
    let pieces = Pieces::parse(value).ok()?;
    let time = pieces.time()?;
    pieces.offset()?;
    let shown = if time.second() == 0 {
        format!("{:02}:{:02}", time.hour(), time.minute())
    } else {
        format!(
            "{:02}:{:02}:{:02}",
            time.hour(),
            time.minute(),
            time.second()
        )
    };
    Some((pieces.date().to_string(), shown))
}

/// The value of a date field and a time field together: `Some(None)` when
/// both are empty, `Some(value)` for a valid date and a time of `HH:MM` or
/// `HH:MM:SS`, and `None` for anything else. The value keeps the offset of
/// `held`, the value the input held before, or takes this device's time
/// zone's offset at that time when it held none.
pub fn date_time_value(date: &str, time: &str, held: Option<&str>) -> Option<Option<String>> {
    let (date, time) = (date.trim(), time.trim());
    if date.is_empty() && time.is_empty() {
        return Some(None);
    }
    if !workspace_format::is_civil_date(date) || !(time.len() == 5 || time.len() == 8) {
        return None;
    }
    let date: Date = date.parse().ok()?;
    let time: Time = time.parse().ok()?;
    let at = DateTime::from_parts(date, time);
    let offset = held
        .and_then(|held| Pieces::parse(held).ok()?.offset())
        .map(|offset| offset.to_numeric_offset())
        .map_or_else(|| local_offset(at), Some)?;
    let instant: Timestamp = offset.to_timestamp(at).ok()?;
    Some(Some(instant.display_with_offset(offset).to_string()))
}

/// This device's offset from UTC at the civil time `at`.
fn local_offset(at: DateTime) -> Option<Offset> {
    at.to_zoned(TimeZone::system())
        .ok()
        .map(|zoned| zoned.offset())
}
