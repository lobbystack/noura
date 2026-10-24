//! Weekday, month and time names in the computer's locale, through
//! ICU4X's date formatting and the data compiled into the app, for
//! calendar grids. A locale ICU4X can't read falls back to US English.

use std::{cell::RefCell, rc::Rc};

use icu_calendar::{Date as IcuDate, Iso};
use icu_datetime::{DateTimeFormatter, NoCalendarFormatter, fieldsets, options::TimePrecision};
use icu_locale_core::{Locale, locale};
use icu_time::Time as IcuTime;
use jiff::civil::{Date, Time};

/// The names a calendar grid shows, in one locale.
pub(crate) struct DateNames {
    weekday: DateTimeFormatter<fieldsets::E>,
    weekday_long: DateTimeFormatter<fieldsets::E>,
    day_number: DateTimeFormatter<fieldsets::D>,
    full_date: DateTimeFormatter<fieldsets::YMDE>,
    month: DateTimeFormatter<fieldsets::YM>,
    time: NoCalendarFormatter<fieldsets::T>,
    hour: NoCalendarFormatter<fieldsets::T>,
}

thread_local! {
    /// The names last asked for, with their locale tag. The UI draws on
    /// one thread, and the locale rarely changes.
    static NAMES: RefCell<Option<(Option<String>, Rc<DateNames>)>> = const { RefCell::new(None) };
}

impl DateNames {
    /// The names in the locale `tag`, a BCP 47 tag such as `fr-CA`.
    pub(crate) fn of(tag: Option<&str>) -> Rc<Self> {
        NAMES.with_borrow_mut(|names| {
            if let Some((held, names)) = names
                && held.as_deref() == tag
            {
                return names.clone();
            }
            let made = Rc::new(
                tag.and_then(|tag| tag.parse::<Locale>().ok())
                    .and_then(|locale| Self::new(&locale))
                    .or_else(|| Self::new(&locale!("en-US")))
                    .expect("ICU4X's compiled data holds US English"),
            );
            *names = Some((tag.map(str::to_owned), made.clone()));
            made
        })
    }

    fn new(locale: &Locale) -> Option<Self> {
        let prefs = || locale.clone().into();
        Some(Self {
            weekday: DateTimeFormatter::try_new(prefs(), fieldsets::E::short()).ok()?,
            weekday_long: DateTimeFormatter::try_new(prefs(), fieldsets::E::long()).ok()?,
            day_number: DateTimeFormatter::try_new(prefs(), fieldsets::D::short()).ok()?,
            full_date: DateTimeFormatter::try_new(prefs(), fieldsets::YMDE::long()).ok()?,
            month: DateTimeFormatter::try_new(prefs(), fieldsets::YM::long()).ok()?,
            time: NoCalendarFormatter::try_new(prefs(), fieldsets::T::hm()).ok()?,
            hour: NoCalendarFormatter::try_new(
                prefs(),
                fieldsets::T::short().with_time_precision(TimePrecision::Hour),
            )
            .ok()?,
        })
    }

    /// The weekday's short name, such as `Sun`.
    pub(crate) fn weekday(&self, date: Date) -> String {
        plain(self.weekday.format(&iso(date)).to_string())
    }

    /// The weekday's whole name, such as `Sunday`.
    pub(crate) fn weekday_long(&self, date: Date) -> String {
        plain(self.weekday_long.format(&iso(date)).to_string())
    }

    /// The day of the month, such as `27`.
    pub(crate) fn day_number(&self, date: Date) -> String {
        plain(self.day_number.format(&iso(date)).to_string())
    }

    /// The whole date with its weekday, such as `Sunday, September 27,
    /// 2026`.
    pub(crate) fn full_date(&self, date: Date) -> String {
        plain(self.full_date.format(&iso(date)).to_string())
    }

    /// The month and year, such as `September 2026`.
    pub(crate) fn month(&self, date: Date) -> String {
        plain(self.month.format(&iso(date)).to_string())
    }

    /// The time to the minute, such as `9:00 AM`.
    pub(crate) fn time(&self, time: Time) -> String {
        plain(self.time.format(&icu_time(time)).to_string())
    }

    /// The hour alone, such as `9 AM`, for a row of hours.
    pub(crate) fn hour(&self, hour: i8) -> String {
        let time = Time::new(hour, 0, 0, 0).unwrap_or(Time::midnight());
        plain(self.hour.format(&icu_time(time)).to_string())
    }
}

fn iso(date: Date) -> IcuDate<Iso> {
    IcuDate::try_new_iso(
        i32::from(date.year()),
        date.month().unsigned_abs(),
        date.day().unsigned_abs(),
    )
    .unwrap_or_else(|_| IcuDate::try_new_iso(1970, 1, 1).expect("a valid date"))
}

fn icu_time(time: Time) -> IcuTime {
    IcuTime::try_new(
        time.hour().unsigned_abs(),
        time.minute().unsigned_abs(),
        time.second().unsigned_abs(),
        0,
    )
    .unwrap_or_else(|_| IcuTime::start_of_day())
}

/// `text` with the narrow and thin spaces some locales put in times, such
/// as before `AM`, made ordinary spaces, which every font draws.
fn plain(text: String) -> String {
    if text.contains(['\u{202f}', '\u{2009}']) {
        text.replace(['\u{202f}', '\u{2009}'], " ")
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use jiff::civil::{date, time};

    use super::DateNames;

    #[test]
    fn names_follow_the_locale_and_fall_back_to_us_english() {
        let english = DateNames::of(Some("en-US"));
        assert_eq!(
            english.full_date(date(2026, 9, 27)),
            "Sunday, September 27, 2026"
        );
        assert_eq!(english.time(time(9, 30, 0, 0)), "9:30 AM");
        assert_eq!(english.hour(0), "12 AM");

        let french = DateNames::of(Some("fr-CA"));
        assert_eq!(french.time(time(13, 5, 0, 0)), "13 h 05");
        assert_eq!(french.month(date(2026, 10, 21)), "octobre 2026");

        let unknown = DateNames::of(Some("not a locale"));
        assert_eq!(unknown.weekday(date(2026, 9, 27)), "Sun");
    }
}
