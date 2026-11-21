//! The event form: a new event, or the selected one. It keeps what you
//! type as a draft, so switching All day redraws the start and end as
//! dates or as dates and times without losing the rest. An all-day
//! event's form shows its last day, where its file holds the day after.

use jiff::civil::{Date, Time};
use plugin_sdk::{
    Error, Value, View,
    objects::{self, Changes, NewObject},
};
use serde_json::{Map, Value as Json, json};

use crate::{
    calendars::{self, Own},
    dates::{Now, add_days},
    names,
    types::EVENT,
};

/// The form's node IDs.
pub(crate) const FORM: &str = "event";
pub(crate) const TITLE: &str = "event-title";
pub(crate) const ALL_DAY: &str = "event-all-day";
pub(crate) const START: &str = "event-start";
pub(crate) const END: &str = "event-end";
pub(crate) const CALENDAR: &str = "event-calendar";

/// The name of the calendar Noura makes for your first event when you
/// have none.
const FIRST_CALENDAR: &str = "Calendar";

/// What the form holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Draft {
    /// The event's stable ID, or none for a new one.
    pub(crate) event: Option<String>,
    pub(crate) title: String,
    pub(crate) all_day: bool,
    /// A date for an all-day event, an instant with an offset otherwise.
    pub(crate) start: Option<String>,
    /// In the start's form: an all-day event's last day, or a timed
    /// event's end.
    pub(crate) end: Option<String>,
    /// The calendar's stable ID.
    pub(crate) calendar: Option<String>,
    /// Why the last save failed.
    pub(crate) problem: Option<String>,
}

impl Draft {
    /// A new event in `calendar` from 9:00 to 10:00 on `day`.
    pub(crate) fn new(now: &Now, day: Date, calendar: Option<String>) -> Self {
        Self {
            event: None,
            title: String::new(),
            all_day: false,
            start: Some(now.at(day, Time::constant(9, 0, 0, 0))),
            end: Some(now.at(day, Time::constant(10, 0, 0, 0))),
            calendar,
            problem: None,
        }
    }

    /// The form of the event `id` titled `title`, whose file holds
    /// `properties`, in `calendar`.
    pub(crate) fn of(
        id: &str,
        title: &str,
        properties: &Map<String, Json>,
        calendar: Option<String>,
    ) -> Self {
        let text = |key: &str| {
            properties
                .get(key)
                .and_then(Json::as_str)
                .map(str::to_owned)
        };
        let start = text("start");
        let all_day = start.as_ref().is_some_and(|start| start.len() == 10);
        let mut end = text("end");
        if all_day {
            // The file holds the day after the last.
            let first = start
                .as_deref()
                .and_then(|start| start.parse::<Date>().ok());
            end = end
                .and_then(|end| end.parse::<Date>().ok())
                .filter(|end| first.is_none_or(|first| *end > first))
                .map(|end| add_days(end, -1).to_string());
        }
        Self {
            event: Some(id.to_owned()),
            title: title.to_owned(),
            all_day,
            start,
            end,
            calendar,
            problem: None,
        }
    }

    /// Take the value an input of the form now holds.
    pub(crate) fn change(&mut self, now: &Now, node: &str, value: Value) {
        match (node, value) {
            (TITLE, Value::Text(title)) => self.title = title,
            (ALL_DAY, Value::Checkbox(all_day)) => self.set_all_day(now, all_day),
            (START, Value::Date(day) | Value::DateTime(day)) => self.start = day,
            (END, Value::Date(day) | Value::DateTime(day)) => self.end = day,
            (CALENDAR, Value::Choice(calendar)) => self.calendar = calendar,
            _ => {}
        }
    }

    /// Switch between an all-day event and a timed one: the days the
    /// start and end fall on, or 9:00 to 10:00 on the first day.
    fn set_all_day(&mut self, now: &Now, all_day: bool) {
        if all_day == self.all_day {
            return;
        }
        let first = self.start.as_deref().and_then(|start| now.day_of(start));
        if all_day {
            let last = self.end.as_deref().and_then(|end| now.day_of(end));
            self.start = first.map(|day| day.to_string());
            self.end = last
                .filter(|last| first.is_none_or(|first| *last > first))
                .map(|day| day.to_string());
        } else {
            self.start = first.map(|day| now.at(day, Time::constant(9, 0, 0, 0)));
            self.end = first.map(|day| now.at(day, Time::constant(10, 0, 0, 0)));
        }
        self.all_day = all_day;
    }

    /// The start and end the file holds: an all-day end is the day after
    /// the last, and an end that isn't after the start is left out.
    fn span(&self) -> Option<(String, Option<String>)> {
        let start = self.start.clone()?;
        let end = if self.all_day {
            let first: Date = start.parse().ok()?;
            self.end
                .as_deref()
                .and_then(|end| end.parse::<Date>().ok())
                .filter(|last| *last >= first)
                .map(|last| add_days(last, 1).to_string())
        } else {
            let first: jiff::Timestamp = start.parse().ok()?;
            self.end
                .clone()
                .filter(|end| end.parse::<jiff::Timestamp>().is_ok_and(|end| end > first))
        };
        Some((start, end))
    }
}

/// Draw the form in the panel at `parent`, with `own` as the calendars to
/// choose from.
pub(crate) fn render(view: &mut View, parent: u32, draft: &Draft, own: &[Own]) {
    let submit = if draft.event.is_some() { "Save" } else { "Add" };
    let form = view.form(parent, FORM, "Event", submit);
    view.input(form, TITLE, "Title", Value::Text(draft.title.clone()));
    view.require(form, TITLE);
    view.input(form, ALL_DAY, "All day", Value::Checkbox(draft.all_day));
    let (start, end) = if draft.all_day {
        (
            Value::Date(draft.start.clone()),
            Value::Date(draft.end.clone()),
        )
    } else {
        (
            Value::DateTime(draft.start.clone()),
            Value::DateTime(draft.end.clone()),
        )
    };
    view.input(form, START, "Starts", start);
    view.input(form, END, "Ends", end);
    if !own.is_empty() {
        let choices: Vec<(&str, &str)> = own
            .iter()
            .map(|calendar| (calendar.id.as_str(), calendar.name.as_str()))
            .collect();
        let chosen = draft
            .calendar
            .as_deref()
            .filter(|id| own.iter().any(|calendar| calendar.id == *id))
            .unwrap_or(own[0].id.as_str());
        view.choice_input(form, CALENDAR, "Calendar", &choices, Some(chosen));
    }
    if let Some(problem) = &draft.problem {
        view.text(parent, "event-problem", problem);
    }
}

/// Write the draft to its event's file, or create the event, and return
/// its stable ID. With no calendar of yours, Noura makes one first.
pub(crate) fn save(draft: &Draft, own: &[Own]) -> Result<String, String> {
    let title = names::file_name(&draft.title);
    if title.is_empty() {
        return Err("Give the event a title.".to_owned());
    }
    let Some((start, end)) = draft.span() else {
        return Err("Give the event a start.".to_owned());
    };
    let calendar = match draft
        .calendar
        .as_deref()
        .filter(|id| own.iter().any(|calendar| calendar.id == *id))
        .or_else(|| own.first().map(|calendar| calendar.id.as_str()))
    {
        Some(id) => id.to_owned(),
        None => calendars::create(FIRST_CALENDAR, own).map_err(|error| problem(&error))?,
    };
    let mut properties = json!({ "calendar": calendar, "start": start });
    if let Some(end) = &end {
        properties["end"] = json!(end);
    }
    let Some(id) = &draft.event else {
        return objects::create(&NewObject {
            object_type: EVENT.to_owned(),
            title,
            path: None,
            body: String::new(),
            properties: properties.to_string(),
        })
        .map(|object| object.id)
        .map_err(|error| problem(&error));
    };
    let latest = objects::get(id).map_err(|error| problem(&error))?;
    let updated = objects::update(
        id,
        &latest.revision,
        &Changes {
            body: None,
            properties: properties.to_string(),
            remove: if end.is_none() {
                vec!["end".to_owned()]
            } else {
                Vec::new()
            },
        },
    )
    .map_err(|error| problem(&error))?;
    if title != updated.title {
        objects::move_to(
            id,
            &updated.revision,
            &names::renamed(&updated.path, &title),
        )
        .map_err(|error| problem(&error))?;
    }
    Ok(id.clone())
}

/// Move the event `id` to the trash.
pub(crate) fn delete(id: &str) -> Result<(), String> {
    let latest = objects::get(id).map_err(|error| problem(&error))?;
    objects::delete(id, &latest.revision).map_err(|error| problem(&error))
}

/// What a failed write says, in one sentence.
pub(crate) fn problem(error: &Error) -> String {
    match error {
        Error::RevisionConflict(_) => {
            "The event changed in another app. Select it again to see the change.".to_owned()
        }
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

    #[test]
    fn a_new_event_runs_from_nine_to_ten_on_its_day() {
        let draft = Draft::new(&toronto(), "2026-11-21".parse().unwrap(), None);
        assert_eq!(draft.start.as_deref(), Some("2026-11-21T09:00:00-05:00"));
        assert_eq!(draft.end.as_deref(), Some("2026-11-21T10:00:00-05:00"));
        assert!(!draft.all_day);
    }

    #[test]
    fn all_day_switches_between_days_and_times() {
        let now = toronto();
        let mut draft = Draft::new(&now, "2026-11-21".parse().unwrap(), None);
        draft.change(&now, ALL_DAY, Value::Checkbox(true));
        assert_eq!(
            (draft.start.as_deref(), draft.end.as_deref()),
            (Some("2026-11-21"), None)
        );
        draft.change(&now, END, Value::Date(Some("2026-11-23".into())));
        // The form shows the last day; the file holds the day after.
        assert_eq!(
            draft.span(),
            Some(("2026-11-21".into(), Some("2026-11-24".into())))
        );
        draft.change(&now, ALL_DAY, Value::Checkbox(false));
        assert_eq!(draft.start.as_deref(), Some("2026-11-21T09:00:00-05:00"));
    }

    #[test]
    fn an_all_day_file_shows_its_last_day() {
        let mut properties = Map::new();
        properties.insert("start".into(), json!("2026-11-21"));
        properties.insert("end".into(), json!("2026-11-23"));
        let draft = Draft::of("event_a", "Retreat", &properties, None);
        assert!(draft.all_day);
        assert_eq!(draft.end.as_deref(), Some("2026-11-22"));
        assert_eq!(
            draft.span(),
            Some(("2026-11-21".into(), Some("2026-11-23".into())))
        );
    }

    #[test]
    fn an_end_before_the_start_is_left_out() {
        let now = toronto();
        let mut draft = Draft::new(&now, "2026-11-21".parse().unwrap(), None);
        draft.change(
            &now,
            END,
            Value::DateTime(Some("2026-11-21T08:00:00-05:00".into())),
        );
        assert_eq!(
            draft.span(),
            Some(("2026-11-21T09:00:00-05:00".into(), None))
        );
    }
}
