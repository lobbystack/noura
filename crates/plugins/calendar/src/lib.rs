//! The Calendar plugin. It reads the workspace and places every object
//! with a due date, a date, or a start and end on month, week and day
//! views, whichever plugin owns the object's type. Selecting an entry
//! shows its properties beside an Open button, and the Inbox gains a Next
//! 7 days section. Calendar writes nothing: its manifest declares no
//! capability that could.
//!
//! The rules that pick and order a range's entries are the kernel's,
//! moved here from `workspace-format` in [`rules`].
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `calendar_plugin.wasm`.

mod dates;
mod entries;
mod rules;

use std::sync::Mutex;

use jiff::{ToSpan, civil::Date};
use plugin_sdk::{
    CalendarMode, Column, Event, InboxSection, Node, Page, Plugin, Target, Value, View,
    navigation::{self, Placement},
};
use serde_json::Value as Json;

use dates::{Now, add_days};
use entries::{Entries, Entry};
use rules::select_calendar_entries;

/// The Calendar page.
const PAGE: &str = "calendar";

/// The Inbox section.
const SECTION: &str = "next-7-days";

/// How many entries the Inbox section lists.
const UPCOMING: usize = 5;

/// The most entries one grid draws. A view holds at most 10,000 nodes.
const MOST_SHOWN: usize = 5_000;

/// The period a grid shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Month,
    Week,
    Day,
}

impl Mode {
    const ALL: [(Self, &'static str, &'static str); 3] = [
        (Self::Month, "month", "Month"),
        (Self::Week, "week", "Week"),
        (Self::Day, "day", "Day"),
    ];

    fn id(self) -> &'static str {
        Self::ALL
            .iter()
            .find(|(mode, _, _)| *mode == self)
            .map_or("month", |(_, id, _)| id)
    }

    fn named(id: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .find(|(_, known, _)| *known == id)
            .map(|(mode, _, _)| *mode)
    }

    fn grid(self) -> CalendarMode {
        match self {
            Self::Month => CalendarMode::Month,
            Self::Week => CalendarMode::Week,
            Self::Day => CalendarMode::Day,
        }
    }

    /// The first day the grid shows around `focus`, and how many days: six
    /// weeks from the Sunday on or before the first of the month, Sunday
    /// to Saturday, or the day alone.
    fn days(self, focus: Date) -> (Date, i64) {
        let sunday_before =
            |day: Date| add_days(day, -i64::from(day.weekday().to_sunday_zero_offset()));
        match self {
            Self::Month => (sunday_before(focus.first_of_month()), 42),
            Self::Week => (sunday_before(focus), 7),
            Self::Day => (focus, 1),
        }
    }

    /// The day Previous or Next moves `focus` to: the first of the month
    /// before or after, or a week or a day away.
    fn step(self, focus: Date, forward: bool) -> Date {
        let sign = if forward { 1 } else { -1 };
        match self {
            Self::Month => focus
                .first_of_month()
                .checked_add((sign).months())
                .unwrap_or(focus),
            Self::Week => add_days(focus, 7 * sign),
            Self::Day => add_days(focus, sign),
        }
    }

    /// The labels of Previous and Next.
    fn steps(self) -> (&'static str, &'static str) {
        match self {
            Self::Month => ("Previous month", "Next month"),
            Self::Week => ("Previous week", "Next week"),
            Self::Day => ("Previous day", "Next day"),
        }
    }
}

/// What the page shows, between calls.
struct State {
    mode: Mode,
    /// The day the grid shows the period of, or none for today.
    focus: Option<Date>,
    /// The key of the entry whose panel shows.
    selected: Option<String>,
    /// Why the last Open failed.
    notice: Option<String>,
    entries: Entries,
}

static STATE: Mutex<State> = Mutex::new(State {
    mode: Mode::Month,
    focus: None,
    selected: None,
    notice: None,
    entries: Entries::new(),
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|error| error.into_inner())
}

struct Calendar;

impl Plugin for Calendar {
    fn pages() -> Vec<Page> {
        vec![Page {
            id: PAGE.into(),
            title: "Calendar".into(),
        }]
    }

    fn render(_page: String) -> Vec<Node> {
        page()
    }

    fn handle_event(_page: String, event: Event) {
        handle(event);
    }

    fn inbox_sections() -> Vec<InboxSection> {
        vec![InboxSection {
            id: SECTION.into(),
            title: "Next 7 days".into(),
        }]
    }

    fn render_inbox_section(_section: String) -> Vec<Node> {
        next_seven_days()
    }

    fn handle_inbox_event(_section: String, event: Event) {
        if let Event::Activated(Target { item, .. }) = event {
            open(&item);
        }
    }
}

/// Every dated entry in the workspace, or why it couldn't be read.
fn all_entries(state: &mut State) -> Result<Vec<Entry>, String> {
    state
        .entries
        .all()
        .map_err(|_| "Noura couldn't read the workspace.".to_owned())
}

/// The entries of `all` in the `count` days from `first`, in the order
/// the rules set.
fn within(all: Vec<Entry>, now: &Now, first: Date, count: i64) -> Result<Vec<Entry>, String> {
    let start = now.midnight(first);
    let end = now.midnight(add_days(first, count));
    select_calendar_entries(all, &start, &end).map_err(|_| "The period has no days.".to_owned())
}

/// The Calendar page: Previous, Today and Next, the mode, the selected
/// entry's panel, and the grid.
fn page() -> Vec<Node> {
    let now = Now::read();
    let mut state = state();
    let focus = state.focus.unwrap_or(now.today);
    let mode = state.mode;
    let (first, count) = mode.days(focus);
    let all = all_entries(&mut state);
    let selected = match (&state.selected, &all) {
        (Some(key), Ok(all)) => {
            let found = all.iter().find(|entry| &entry.key == key).cloned();
            if found.is_none() {
                // Its date or its object went away.
                state.selected = None;
            }
            found
        }
        _ => None,
    };
    let shown = all.and_then(|all| within(all, &now, first, count));

    let mut view = View::panel("root", None);
    let (previous, next) = mode.steps();
    view.button(View::ROOT, "previous", previous);
    view.button(View::ROOT, "today", "Today");
    view.button(View::ROOT, "next", next);
    let modes: Vec<(&str, &str)> = Mode::ALL.iter().map(|(_, id, name)| (*id, *name)).collect();
    view.choice_input(View::ROOT, "mode", "View", &modes, Some(mode.id()));
    if let Some(notice) = &state.notice {
        view.text(View::ROOT, "notice", notice);
    }
    if let Some(entry) = &selected {
        entry_panel(&mut view, entry);
    }
    let entries = match shown {
        Ok(entries) => entries,
        Err(problem) => {
            view.text(View::ROOT, "problem", &problem);
            Vec::new()
        }
    };
    if entries.len() > MOST_SHOWN {
        view.text(
            View::ROOT,
            "too-many",
            &format!(
                "This period has {} entries. The calendar shows the first 5,000.",
                entries.len()
            ),
        );
    }
    let grid = view.calendar(
        View::ROOT,
        "calendar",
        "Calendar",
        mode.grid(),
        &focus.to_string(),
    );
    for entry in entries.iter().take(MOST_SHOWN) {
        if let Some((start, end)) = entry.span() {
            view.entry(grid, &entry.key, &entry.title, &start, end.as_deref());
        }
    }
    view.into_nodes()
}

/// The selected entry's title, type and properties, and Open.
fn entry_panel(view: &mut View, entry: &Entry) {
    let panel = view.add_panel(View::ROOT, "entry", Some("Selected entry"));
    view.text(panel, "entry-title", &entry.title);
    view.text(panel, "entry-type", &format!("Type: {}", entry.source_type));
    let columns = vec![
        Column {
            id: "property".into(),
            title: "Property".into(),
            sorted: None,
        },
        Column {
            id: "value".into(),
            title: "Value".into(),
            sorted: None,
        },
    ];
    let table = view.table(panel, "entry-properties", "Properties", columns);
    for (name, value) in &entry.properties {
        let value = shown_value(value);
        view.row(table, &format!("property:{name}"), &[name, &value]);
    }
    view.button(panel, "open", "Open");
    view.button(panel, "close-entry", "Close");
}

/// A property's value as the entry panel shows it.
fn shown_value(value: &Json) -> String {
    match value {
        Json::String(text) => text.clone(),
        Json::Null => String::new(),
        Json::Array(items) => items.iter().map(shown_value).collect::<Vec<_>>().join(", "),
        other => other.to_string(),
    }
}

fn handle(event: Event) {
    match event {
        Event::Pressed(node) => match node.as_str() {
            "previous" | "next" => {
                let today = Now::read().today;
                let mut state = state();
                let focus = state.focus.unwrap_or(today);
                state.focus = Some(state.mode.step(focus, node == "next"));
                state.selected = None;
            }
            "today" => {
                let mut state = state();
                state.focus = None;
                state.selected = None;
            }
            "open" => {
                let selected = state().selected.clone();
                if let Some(key) = selected {
                    open(&key);
                }
            }
            "close-entry" => state().selected = None,
            _ => {}
        },
        Event::Changed(field) if field.node == "mode" => {
            if let Value::Choice(Some(id)) = field.value
                && let Some(mode) = Mode::named(&id)
            {
                let mut state = state();
                if state.mode != mode {
                    state.mode = mode;
                    state.selected = None;
                }
            }
        }
        Event::Selected(Target { item, .. }) => {
            let mut state = state();
            state.selected = Some(item);
            state.notice = None;
        }
        Event::Activated(Target { item, .. }) => {
            state().selected = Some(item.clone());
            open(&item);
        }
        _ => {}
    }
}

/// Open the object of the entry `key` in a tab of its own, so the page
/// stays where it is.
fn open(key: &str) {
    let source = all_entries(&mut state())
        .ok()
        .and_then(|entries| entries.into_iter().find(|entry| entry.key == key))
        .map(|entry| entry.source_id);
    let Some(id) = source else {
        return;
    };
    let notice = navigation::open_object(&id, Placement::NewTab)
        .err()
        .map(|_| "Noura couldn't open it.".to_owned());
    state().notice = notice;
}

/// The Inbox's Next 7 days: up to five entries from the start of today
/// through seven days ahead, each with when it starts.
fn next_seven_days() -> Vec<Node> {
    let now = Now::read();
    let entries = all_entries(&mut state()).and_then(|all| within(all, &now, now.today, 7));
    let mut view = View::panel(SECTION, None);
    match entries {
        Ok(entries) if entries.is_empty() => {
            view.text(View::ROOT, "nothing", "Nothing scheduled this week.");
        }
        Ok(entries) => {
            let list = view.list(View::ROOT, "upcoming", "Next 7 days");
            for entry in entries.iter().take(UPCOMING) {
                view.item(
                    list,
                    &entry.key,
                    &entry.title,
                    Some(&now.label(&entry.start)),
                );
            }
        }
        Err(problem) => {
            view.text(View::ROOT, "problem", &problem);
        }
    }
    view.into_nodes()
}

plugin_sdk::export!(Calendar);
