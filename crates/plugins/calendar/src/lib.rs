//! The Calendar plugin. It reads the workspace and places every object
//! with a due date, a date, or a start and end on month, week and day
//! views, whichever plugin owns the object's type, and the Inbox gains a
//! Next 7 days section.
//!
//! Each entry belongs to a calendar and draws in its color. Your
//! calendars are files of the `calendar` type, and your events files of
//! the `event` type that name their calendar by ID. Every other type with
//! dates gets a built-in calendar of its own, such as Tasks. The sidebar
//! lists them all with a checkbox that hides a calendar's entries in every
//! view, and the settings file keeps what you hid. Selecting an entry
//! opens a popover beside it: an event of yours edits in place there, and
//! any other entry shows what it is and when, with Open. Calendar writes
//! only files of its own types.
//!
//! The rules that pick and order a range's entries are the kernel's,
//! moved here from `workspace-format` in [`rules`].
//!
//! Build it with `crates/plugin-sdk/build-plugins.sh`, then install the
//! folder holding `plugin.toml` and the built `calendar_plugin.wasm`.

mod calendars;
mod dates;
mod details;
mod entries;
mod names;
mod rules;
mod sidebar;
mod types;

use std::sync::Mutex;

use jiff::{ToSpan, civil::Date};
use plugin_sdk::{
    CalendarMode, Color, Event, Icon, InboxSection, Node, ObjectType, Page, Plugin, Target, Value,
    View,
    navigation::{self, Placement},
};

use calendars::{Key, Own, Settings, built_in, key_of};
use dates::{Now, add_days};
use details::Edit;
use entries::{Entries, Entry};
use rules::select_calendar_entries;
use types::{CALENDAR, EVENT};

/// The Calendar page.
const PAGE: &str = "calendar";

/// The Inbox section's button that opens the Calendar page.
const OPEN_CALENDAR: &str = "open-calendar";

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
pub(crate) struct State {
    mode: Mode,
    /// The day the grid shows the period of, or none for today.
    focus: Option<Date>,
    /// The key of the entry whose popover shows.
    selected: Option<String>,
    /// Why the last change in the popover failed.
    problem: Option<String>,
    /// Why the last Open or New event failed.
    notice: Option<String>,
    /// The calendar you selected in the sidebar, which new events go to.
    pub(crate) calendar: Option<String>,
    /// Whether the sidebar shows a name field for a new calendar.
    pub(crate) naming_calendar: bool,
    /// The calendar whose name field shows in its place.
    pub(crate) renaming: Option<String>,
    /// The calendar whose deletion waits for you to confirm it.
    deleting: Option<String>,
    /// Why the last change to a calendar failed.
    pub(crate) sidebar_notice: Option<String>,
    entries: Entries,
}

static STATE: Mutex<State> = Mutex::new(State {
    mode: Mode::Month,
    focus: None,
    selected: None,
    problem: None,
    notice: None,
    calendar: None,
    naming_calendar: false,
    renaming: None,
    deleting: None,
    sidebar_notice: None,
    entries: Entries::new(),
});

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(|error| error.into_inner())
}

struct Calendar;

impl Plugin for Calendar {
    fn object_types() -> Vec<ObjectType> {
        types::DECLARED
            .iter()
            .map(|(name, schema)| ObjectType {
                name: (*name).into(),
                schema: (*schema).into(),
            })
            .collect()
    }

    fn pages() -> Vec<Page> {
        vec![Page {
            id: PAGE.into(),
            title: "Calendar".into(),
        }]
    }

    fn render(_page: String) -> Vec<Node> {
        page()
    }

    fn sidebar(_page: String) -> Vec<Node> {
        sidebar_nodes()
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
        match event {
            Event::Activated(Target { item, .. }) => open(&item),
            Event::Pressed(node)
                if node == OPEN_CALENDAR && navigation::open_page(PAGE).is_err() =>
            {
                state().notice = Some("Noura couldn't open the calendar.".to_owned());
            }
            _ => {}
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

/// The calendars, what you hid, and the entries of the calendars you
/// show, each with its calendar's color.
struct Shown {
    own: Vec<Own>,
    entries: Vec<(Entry, Color)>,
}

/// The entries of `all` whose calendars you show, each with its
/// calendar's color.
fn visible(all: Vec<Entry>, own: &[Own], settings: &Settings) -> Vec<(Entry, Color)> {
    all.into_iter()
        .filter_map(|entry| {
            let key = key_of(&entry.source_type, &entry.properties, own);
            if settings.hides(&key) {
                return None;
            }
            let color = match &key {
                Key::Own(id) => own
                    .iter()
                    .find(|calendar| calendar.id == *id)
                    .map_or(Color::Blue, |calendar| calendar.color),
                Key::Type(object_type) => built_in(object_type).1,
            };
            Some((entry, color))
        })
        .collect()
}

/// Every entry of the calendars you show, or why the workspace couldn't
/// be read.
fn shown(state: &mut State) -> Result<Shown, String> {
    let all = all_entries(state)?;
    let own = calendars::own(state.entries.of_type(CALENDAR));
    let settings = Settings::read();
    Ok(Shown {
        entries: visible(all, &own, &settings),
        own,
    })
}

/// Your calendars, read with the workspace's dated entries. None when
/// the workspace can't be read.
fn own_calendars(state: &mut State) -> Vec<Own> {
    if all_entries(state).is_err() {
        return Vec::new();
    }
    calendars::own(state.entries.of_type(CALENDAR))
}

/// The calendar new events go to: the one you selected, or your first.
fn default_calendar(state: &State, own: &[Own]) -> Option<String> {
    state
        .calendar
        .as_ref()
        .filter(|id| own.iter().any(|calendar| calendar.id == **id))
        .cloned()
        .or_else(|| own.first().map(|calendar| calendar.id.clone()))
}

/// The entries of `all` in the `count` days from `first`, in the order
/// the rules set.
fn within<T: rules::CalendarItem>(
    all: Vec<T>,
    now: &Now,
    first: Date,
    count: i64,
) -> Result<Vec<T>, String> {
    let start = now.midnight(first);
    let end = now.midnight(add_days(first, count));
    select_calendar_entries(all, &start, &end).map_err(|_| "The period has no days.".to_owned())
}

impl rules::CalendarItem for (Entry, Color) {
    fn start(&self) -> &str {
        self.0.start()
    }
    fn end(&self) -> Option<&str> {
        self.0.end()
    }
    fn all_day(&self) -> bool {
        self.0.all_day()
    }
    fn title(&self) -> &str {
        self.0.title()
    }
    fn source_id(&self) -> &str {
        self.0.source_id()
    }
    fn property(&self) -> &str {
        self.0.property()
    }
}

/// The Calendar page: Previous, Today and Next, the mode, the grid, and
/// the selected entry's popover.
fn page() -> Vec<Node> {
    let now = Now::read();
    let mut state = state();
    let focus = state.focus.unwrap_or(now.today);
    let mode = state.mode;
    let (first, count) = mode.days(focus);
    let all = shown(&mut state);
    let own = all.as_ref().map(|all| all.own.clone()).unwrap_or_default();
    let selected = match (&state.selected, &all) {
        (Some(key), Ok(all)) => {
            let found = all
                .entries
                .iter()
                .map(|(entry, _)| entry)
                .find(|entry| &entry.key == key)
                .cloned();
            if found.is_none() {
                // Its date or its object went away, or you hid its
                // calendar.
                state.selected = None;
            }
            found
        }
        _ => None,
    };
    let shown = all.and_then(|all| within(all.entries, &now, first, count));

    let mut view = View::panel("root", None);
    // The title names the period, the month in bold and the year after
    // it, or the day, with New event, the mode, then Previous, Today and
    // Next at its end.
    let period = match mode {
        Mode::Month | Mode::Week => focus.strftime("%B").to_string(),
        Mode::Day => focus.strftime("%B %-d,").to_string(),
    };
    let year = focus.strftime("%Y").to_string();
    let heading = view.period_heading(View::ROOT, "title", &period, Some(&year));
    // New event comes first, so it stands out, as New task does.
    view.button(heading, "new-event", "New event");
    let modes: Vec<(&str, &str)> = Mode::ALL.iter().map(|(_, id, name)| (*id, *name)).collect();
    view.choice_input(heading, "mode", "View", &modes, Some(mode.id()));
    let (previous, next) = mode.steps();
    view.button(heading, "previous", previous);
    view.button(heading, "today", "Today");
    view.button(heading, "next", next);
    if let Some(notice) = &state.notice {
        view.text(View::ROOT, "notice", notice);
    }
    if let Some(id) = &state.deleting {
        delete_panel(&mut view, &own, id);
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
            &format!("Showing the first 5,000 of {} entries.", entries.len()),
        );
    }
    let grid = view.calendar(
        View::ROOT,
        "calendar",
        "Calendar",
        mode.grid(),
        &focus.to_string(),
    );
    let mut drawn = false;
    for (entry, color) in entries.iter().take(MOST_SHOWN) {
        if let Some((start, end)) = entry.span() {
            let node = view.entry(grid, &entry.key, &entry.title, &start, end.as_deref());
            view.color(node, *color);
            drawn |= selected
                .as_ref()
                .is_some_and(|selected| selected.key == entry.key);
        }
    }
    // The popover points at its entry, so it shows only while the grid
    // draws the entry.
    if let Some(entry) = selected.as_ref().filter(|_| drawn) {
        if entry.source_type == EVENT {
            let calendar = match key_of(&entry.source_type, &entry.properties, &own) {
                Key::Own(id) => Some(id),
                Key::Type(_) => None,
            };
            let notes = details::notes(&entry.source_id);
            details::render(
                &mut view,
                &now,
                entry,
                Some((&notes, &own, calendar.as_deref())),
                state.problem.as_deref(),
            );
        } else {
            details::render(&mut view, &now, entry, None, state.problem.as_deref());
        }
    }
    view.into_nodes()
}

/// The question before a calendar that holds events moves to the trash.
fn delete_panel(view: &mut View, own: &[Own], id: &str) {
    let Some(calendar) = own.iter().find(|calendar| calendar.id == id) else {
        return;
    };
    let count = calendars::events_of(id).map_or(0, |events| events.len());
    let panel = view.add_panel(View::ROOT, "delete-calendar", Some("Delete calendar"));
    view.text(
        panel,
        "delete-title",
        &format!("Delete \u{201c}{}\u{201d}?", calendar.name),
    );
    let detail = match count {
        1 => "Its event moves to the trash too.".to_owned(),
        count => format!("Its {count} events move to the trash too."),
    };
    view.text(panel, "delete-detail", &detail);
    view.button(panel, "confirm-delete", "Delete");
    view.button(panel, "cancel-delete", "Cancel");
}

/// The sidebar: your calendars, then the built-in ones of the types whose
/// objects have dates.
fn sidebar_nodes() -> Vec<Node> {
    let mut state = state();
    let all = all_entries(&mut state).unwrap_or_default();
    let own = calendars::own(state.entries.of_type(CALENDAR));
    let settings = Settings::read();
    let mut types: Vec<String> = all
        .iter()
        .filter_map(
            |entry| match key_of(&entry.source_type, &entry.properties, &own) {
                Key::Type(object_type) => Some(object_type),
                Key::Own(_) => None,
            },
        )
        .collect();
    types.sort();
    types.dedup();
    sidebar::render(&state, &own, &types, &settings)
}

fn handle(event: Event) {
    match event {
        Event::Pressed(node) => match node.as_str() {
            "new-event" => new_event(),
            details::DELETE => {
                let source = selected_entry().map(|entry| entry.source_id);
                if let Some(id) = source {
                    let deleted = details::delete(&id);
                    let mut state = state();
                    match deleted {
                        Ok(()) => {
                            state.selected = None;
                            state.problem = None;
                        }
                        Err(problem) => state.problem = Some(problem),
                    }
                }
            }
            sidebar::NEW => {
                let mut state = state();
                state.naming_calendar = true;
                state.renaming = None;
                state.sidebar_notice = None;
            }
            "confirm-delete" => {
                let id = state().deleting.take();
                if let Some(id) = id {
                    delete_calendar(&id);
                }
            }
            "cancel-delete" => state().deleting = None,
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
            details::OPEN => {
                let selected = state().selected.clone();
                if let Some(key) = selected {
                    open(&key);
                }
            }
            _ => {}
        },
        Event::Cancelled(node) if node == details::POPOVER => {
            let mut state = state();
            state.selected = None;
            state.problem = None;
        }
        Event::Changed(field) if field.node.starts_with("event-") => {
            let Some(edit) = Edit::of(&field.node, field.value) else {
                return;
            };
            let Some(entry) = selected_entry().filter(|entry| entry.source_type == EVENT) else {
                return;
            };
            let written = details::apply(&Now::read(), &entry.source_id, &entry.properties, edit);
            state().problem = written.err();
        }
        Event::Checked(check) if check.node == sidebar::MINE || check.node == sidebar::OTHER => {
            if let Some(key) = Key::of_node(&check.item) {
                let notice = Settings::set_hidden(&key, !check.checked)
                    .err()
                    .map(|_| "Noura couldn't save which calendars show. Try again.".to_owned());
                state().sidebar_notice = notice;
            }
        }
        Event::Selected(Target { node, item }) if node == sidebar::MINE => {
            if let Some(Key::Own(id)) = Key::of_node(&item) {
                state().calendar = Some(id);
            }
        }
        Event::Selected(Target { node, .. }) | Event::Activated(Target { node, .. })
            if node == sidebar::OTHER || node == sidebar::MINE => {}
        Event::Named(naming) if naming.node == sidebar::NEW_NAME => {
            if !state().naming_calendar {
                return;
            }
            let own = own_calendars(&mut state());
            let created = calendars::create(&naming.name, &own);
            let mut state = state();
            state.naming_calendar = false;
            match created {
                Ok(id) => {
                    state.calendar = Some(id);
                    state.sidebar_notice = None;
                }
                Err(error) => state.sidebar_notice = Some(calendar_problem(&error)),
            }
        }
        Event::Cancelled(node) if node == sidebar::NEW_NAME => state().naming_calendar = false,
        Event::Named(naming) if naming.node == sidebar::RENAME => {
            let Some(id) = state().renaming.clone() else {
                return;
            };
            let renamed = calendars::rename(&id, &naming.name);
            let mut state = state();
            match renamed {
                Ok(()) => {
                    state.renaming = None;
                    state.sidebar_notice = None;
                }
                Err(error) => state.sidebar_notice = Some(calendar_problem(&error)),
            }
        }
        Event::Cancelled(node) if node == sidebar::RENAME => {
            let mut state = state();
            state.renaming = None;
            state.sidebar_notice = None;
        }
        Event::Chosen(node) => choose(&node),
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
            state.problem = None;
            state.notice = None;
        }
        Event::Activated(Target { item, .. }) => {
            state().selected = Some(item.clone());
            open(&item);
        }
        _ => {}
    }
}

/// The entry whose popover shows, as the workspace holds it now.
fn selected_entry() -> Option<Entry> {
    let mut state = state();
    let key = state.selected.clone()?;
    all_entries(&mut state)
        .ok()?
        .into_iter()
        .find(|entry| entry.key == key)
}

/// Make an event on the day the grid shows, in the calendar you selected
/// or your first, and open its popover.
fn new_event() {
    let now = Now::read();
    let (day, calendar, own) = {
        let mut state = state();
        let own = own_calendars(&mut state);
        let day = state.focus.unwrap_or(now.today);
        (day, default_calendar(&state, &own), own)
    };
    let created = details::create(&now, day, calendar, &own);
    let mut state = state();
    match created {
        Ok(id) => {
            state.selected = Some(format!("{id}:start"));
            state.problem = None;
            state.notice = None;
        }
        Err(problem) => state.notice = Some(problem),
    }
}

/// Answer a choice in a calendar's menu: rename it, recolor it, or delete
/// it, after asking when it holds events.
fn choose(node: &str) {
    if let Some(id) = node.strip_prefix(sidebar::RENAME_ITEM) {
        let mut state = state();
        state.renaming = Some(id.to_owned());
        state.naming_calendar = false;
        state.sidebar_notice = None;
    } else if let Some(rest) = node.strip_prefix(sidebar::COLOR_ITEM) {
        let Some((color, id)) = rest.split_once(':') else {
            return;
        };
        let Some(color) = calendars::color_named(color) else {
            return;
        };
        let notice = calendars::recolor(id, color)
            .err()
            .map(|error| calendar_problem(&error));
        state().sidebar_notice = notice;
    } else if let Some(id) = node.strip_prefix(sidebar::DELETE_ITEM) {
        match calendars::events_of(id) {
            Ok(events) if events.is_empty() => delete_calendar(id),
            Ok(_) => state().deleting = Some(id.to_owned()),
            Err(error) => state().sidebar_notice = Some(calendar_problem(&error)),
        }
    }
}

/// Move the calendar `id` and its events to the trash.
fn delete_calendar(id: &str) {
    let deleted = calendars::delete(id);
    let mut state = state();
    match deleted {
        Ok(()) => {
            if state.calendar.as_deref() == Some(id) {
                state.calendar = None;
            }
            state.sidebar_notice = None;
        }
        Err(error) => state.sidebar_notice = Some(calendar_problem(&error)),
    }
}

/// What a failed change to a calendar says, in one sentence.
fn calendar_problem(error: &plugin_sdk::Error) -> String {
    match error {
        plugin_sdk::Error::AlreadyExists(_) => "Another calendar has that name.".to_owned(),
        plugin_sdk::Error::RevisionConflict(_) => {
            "The calendar changed in another app. Try again.".to_owned()
        }
        _ => "Noura couldn't change the calendar. Try again.".to_owned(),
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
/// through seven days ahead, each with when it starts. They list in the
/// order they start on this device, so west of UTC an entry this evening
/// lists before tomorrow's all-day entries, which the rules order by
/// midnight in UTC.
fn next_seven_days() -> Vec<Node> {
    let now = Now::read();
    let entries = shown(&mut state())
        .and_then(|all| within(all.entries, &now, now.today, 7))
        .map(|mut entries| {
            // A stable sort keeps the rules' order between equal starts.
            entries.sort_by_key(|(entry, _)| now.instant(&entry.start));
            entries
        });
    let mut view = View::panel(SECTION, None);
    let heading = view.section_heading(
        View::ROOT,
        "upcoming-heading",
        "Next 7 days",
        Some(Icon::Calendar),
    );
    view.button(heading, OPEN_CALENDAR, "Calendar");
    let card = view.add_panel(View::ROOT, "upcoming-card", None);
    match entries {
        Ok(entries) if entries.is_empty() => {
            view.text(card, "nothing", "Nothing scheduled this week.");
        }
        Ok(entries) => {
            let list = view.list(card, "upcoming", "Next 7 days");
            for (entry, color) in entries.iter().take(UPCOMING) {
                let item = view.item(
                    list,
                    &entry.key,
                    &entry.title,
                    Some(&now.label(&entry.start)),
                );
                view.color(item, *color);
            }
        }
        Err(problem) => {
            view.text(View::ROOT, "problem", &problem);
        }
    }
    view.into_nodes()
}

plugin_sdk::export!(Calendar);
