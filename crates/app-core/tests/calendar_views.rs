//! Seam 1: the Calendar page with the clock stopped in Toronto. The month
//! view opens on today in the device's time zone, and week and day views
//! place timed entries on the device's local day, side by side where they
//! overlap. Previous, Next and Today move the range for the current mode,
//! and switching modes keeps the focused date. Selecting an entry shows
//! its panel, Open shows its object in a tab with its plugin's properties
//! panel, and outside edits reach the open view without a reload.

mod support;

use std::sync::Arc;

use app_core::{
    App, CalendarMode, GridEntry, OpenAs, PluginEvent, Span, Target, ViewKind, grid_days, lay_out,
};
use jiff::civil::Date;
use support::{
    FakeSystem, Fixture,
    calendar::{self, find, grid, page, press, select, show, texts_under, titles, with_calendar},
    plugins::wait_until,
    tasks::{self, with_tasks},
    write,
};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";
const B: &str = "01jbcdefghjkmnpqrstvwxyz0b";
const C: &str = "01jbcdefghjkmnpqrstvwxyz0c";
const D: &str = "01jbcdefghjkmnpqrstvwxyz0d";
const E: &str = "01jbcdefghjkmnpqrstvwxyz0e";
const F: &str = "01jbcdefghjkmnpqrstvwxyz0f";
const G: &str = "01jbcdefghjkmnpqrstvwxyz0g";
const H: &str = "01jbcdefghjkmnpqrstvwxyz0h";

/// An object's file of type `object_type` with the stable ID suffix
/// `suffix` and `properties` as frontmatter lines.
fn object_file(object_type: &str, suffix: &str, properties: &str) -> Vec<u8> {
    format!(
        "---\nid: {object_type}_{suffix}\ntype: {object_type}\n{properties}\
         created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\nNotes.\n"
    )
    .into_bytes()
}

/// A workspace whose clock reads 22:00 on Saturday 24 October 2026 in
/// Toronto, already 02:00 on Sunday the 25th in UTC.
fn week() -> (Fixture, Arc<FakeSystem>) {
    let fixture = Fixture::new(&[
        (
            "tasks/Standup.md",
            &object_file(
                "task",
                A,
                "status: todo\npriority: high\nstart: 2026-10-21T13:00:00Z\n\
                 end: 2026-10-21T13:30:00Z\n",
            ),
        ),
        (
            "Review.md",
            &object_file(
                "note",
                B,
                "start: 2026-10-21T13:15:00Z\nend: 2026-10-21T14:00:00Z\n",
            ),
        ),
        (
            "Lunch.md",
            &object_file(
                "note",
                C,
                "start: 2026-10-21T16:00:00Z\nend: 2026-10-21T17:00:00Z\n",
            ),
        ),
        (
            // 22:30 on Wednesday in Toronto, Thursday in UTC.
            "Late call.md",
            &object_file(
                "note",
                D,
                "start: 2026-10-22T02:30:00Z\nend: 2026-10-22T03:00:00Z\n",
            ),
        ),
        (
            // 21:00 on Saturday in Toronto, Sunday in UTC.
            "Saturday night.md",
            &object_file("note", E, "due: 2026-10-25T01:00:00Z\n"),
        ),
        (
            // 22:00 on Saturday the 17th in Toronto, the week before.
            "Week before.md",
            &object_file("note", F, "due: 2026-10-18T02:00:00Z\n"),
        ),
        (
            "Retreat.md",
            &object_file("project", G, "date: 2026-10-20\n"),
        ),
        ("Market.md", &object_file("note", H, "date: 2026-11-03\n")),
    ]);
    let system = FakeSystem::new();
    system.set_clock("2026-10-25T02:00:00Z", "America/Toronto");
    (fixture, system)
}

/// The grid laid out in the app's time zone, as the UI kit draws it.
type Days = Vec<Day>;

/// A day of the grid: its date, its all-day titles, and its timed titles
/// with their lane and how many lanes they share.
type Day = (Date, Vec<String>, Vec<(String, usize, usize)>);

fn laid_out(app: &App) -> Days {
    let (mode, date, shown) = grid(app);
    let entries: Vec<GridEntry> = shown
        .iter()
        .map(|entry| GridEntry {
            title: &entry.title,
            span: Span::parse(&entry.start, entry.end.as_deref()).expect("the grid takes it"),
        })
        .collect();
    let days = grid_days(mode, date.parse().unwrap());
    lay_out(&entries, &days, &app.time_zone())
        .into_iter()
        .map(|day| {
            let all_day = day
                .all_day
                .iter()
                .map(|&index| entries[index].title.to_owned())
                .collect();
            let timed = day
                .timed
                .iter()
                .map(|segment| {
                    (
                        entries[segment.entry].title.to_owned(),
                        segment.lane,
                        segment.lanes,
                    )
                })
                .collect();
            (day.date, all_day, timed)
        })
        .collect()
}

fn on<'a>(days: &'a Days, date: &str) -> &'a Day {
    let date: Date = date.parse().unwrap();
    days.iter()
        .find(|day| day.0 == date)
        .unwrap_or_else(|| panic!("the grid doesn't show {date}"))
}

fn timed(entries: &[(&str, usize, usize)]) -> Vec<(String, usize, usize)> {
    entries
        .iter()
        .map(|(title, lane, lanes)| ((*title).to_owned(), *lane, *lanes))
        .collect()
}

/// The label of the page's button `id`.
fn button(app: &App, id: &str) -> Option<String> {
    let view = page(app);
    match &find(view.root(), id)?.kind {
        ViewKind::Button { label } => Some(label.clone()),
        _ => None,
    }
}

#[test]
fn the_month_opens_on_today_in_the_devices_time_zone_with_six_weeks_of_entries() {
    let (fixture, system) = week();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    let (mode, date, _) = grid(&app);
    assert_eq!((mode, date.as_str()), (CalendarMode::Month, "2026-10-24"));
    assert_eq!(app.today().to_string(), "2026-10-24");
    let days = laid_out(&app);
    assert_eq!(days.len(), 42);
    assert_eq!(days[0].0.to_string(), "2026-09-27");
    // The days after the month still show their entries.
    assert_eq!(on(&days, "2026-11-03").1, ["Market"]);
    assert_eq!(on(&days, "2026-10-17").2, timed(&[("Week before", 0, 1)]));
}

#[test]
fn a_week_places_timed_entries_on_the_local_day_side_by_side_where_they_overlap() {
    let (fixture, system) = week();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    show(&mut app, "week");

    let (mode, date, _) = grid(&app);
    assert_eq!((mode, date.as_str()), (CalendarMode::Week, "2026-10-24"));
    // Sunday the 18th in Toronto starts after the entry at 22:00 the day
    // before, which UTC would put on the 18th.
    assert!(!titles(&app).contains(&"Week before".to_owned()));
    let days = laid_out(&app);
    assert_eq!(days[0].0.to_string(), "2026-10-18");
    assert_eq!(on(&days, "2026-10-18").2, timed(&[]));
    assert_eq!(on(&days, "2026-10-20").1, ["Retreat"]);
    assert_eq!(
        on(&days, "2026-10-21").2,
        timed(&[
            ("Standup", 0, 2),
            ("Review", 1, 2),
            ("Lunch", 0, 1),
            ("Late call", 0, 1),
        ])
    );
    assert_eq!(on(&days, "2026-10-22").2, timed(&[]));
    assert_eq!(
        on(&days, "2026-10-24").2,
        timed(&[("Saturday night", 0, 1)])
    );

    show(&mut app, "day");
    calendar::go_to(&mut app, "2026-10-21");
    let days = laid_out(&app);
    assert_eq!(days.len(), 1);
    assert_eq!(
        days[0].2,
        timed(&[
            ("Standup", 0, 2),
            ("Review", 1, 2),
            ("Lunch", 0, 1),
            ("Late call", 0, 1),
        ])
    );
    assert_eq!(titles(&app), ["Standup", "Review", "Lunch", "Late call"]);
}

#[test]
fn previous_next_and_today_move_the_range_and_switching_modes_keeps_the_focused_date() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T15:00:00Z", "America/Toronto");
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };
    let shown = |app: &App| {
        let (mode, date, _) = grid(app);
        (mode, date)
    };
    let month = |date: &str| (CalendarMode::Month, date.to_owned());
    let week = |date: &str| (CalendarMode::Week, date.to_owned());
    let day = |date: &str| (CalendarMode::Day, date.to_owned());

    assert_eq!(shown(&app), month("2026-10-21"));
    assert_eq!(button(&app, "previous").as_deref(), Some("Previous month"));
    assert_eq!(button(&app, "next").as_deref(), Some("Next month"));
    press(&mut app, "next");
    assert_eq!(shown(&app), month("2026-11-01"));
    press(&mut app, "next");
    assert_eq!(shown(&app), month("2026-12-01"));
    press(&mut app, "previous");
    press(&mut app, "previous");
    press(&mut app, "previous");
    assert_eq!(shown(&app), month("2026-09-01"));
    press(&mut app, "today");
    assert_eq!(shown(&app), month("2026-10-21"));

    show(&mut app, "week");
    assert_eq!(shown(&app), week("2026-10-21"));
    assert_eq!(button(&app, "next").as_deref(), Some("Next week"));
    press(&mut app, "next");
    assert_eq!(shown(&app), week("2026-10-28"));

    show(&mut app, "day");
    assert_eq!(shown(&app), day("2026-10-28"));
    assert_eq!(button(&app, "previous").as_deref(), Some("Previous day"));
    press(&mut app, "next");
    assert_eq!(shown(&app), day("2026-10-29"));
    press(&mut app, "previous");
    press(&mut app, "previous");
    assert_eq!(shown(&app), day("2026-10-27"));

    show(&mut app, "month");
    assert_eq!(shown(&app), month("2026-10-27"));
    press(&mut app, "today");
    assert_eq!(shown(&app), month("2026-10-21"));
}

#[test]
fn selecting_an_entry_shows_its_panel_and_open_shows_its_task_with_the_tasks_panel() {
    let (fixture, system) = week();
    let mut app = fixture.app_with(system);
    let Some(_tasks) = with_tasks(&mut app) else {
        return;
    };
    let Some(_calendar) = with_calendar(&mut app) else {
        return;
    };
    // The Calendar page shows in the preview tab, as a click in the rail
    // opens it.
    app.open_page(calendar::PAGE, OpenAs::Preview);
    support::settle(&mut app);
    let standup = format!("task_{A}:start");
    assert_eq!(texts_under(&app, "entry"), None);

    select(&mut app, &standup);

    assert_eq!(
        texts_under(&app, "entry"),
        Some(vec!["Standup".to_owned(), "Type: task".to_owned()])
    );
    let view = page(&app);
    let table = find(view.root(), "entry-properties").expect("the panel lists the properties");
    let rows: Vec<Vec<String>> = table
        .children
        .iter()
        .filter_map(|row| match &row.kind {
            ViewKind::Row { cells } => Some(cells.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        rows,
        [
            ["end", "2026-10-21T13:30:00Z"],
            ["priority", "high"],
            ["start", "2026-10-21T13:00:00Z"],
            ["status", "todo"],
        ]
    );
    assert_eq!(button(&app, "open").as_deref(), Some("Open"));

    press(&mut app, "open");

    let tab = app.active_tab().expect("a tab shows");
    assert_eq!(tab.title(), "Standup");
    let tab = tab.id();
    wait_until(&mut app, "the Tasks panel shows beside the task", |app| {
        app.plugin_panel(tab)
            .and_then(Result::ok)
            .is_some_and(|view| tasks::find(view.root(), "status").is_some())
    });
    // The Calendar page keeps its own tab.
    assert!(
        app.tabs()
            .iter()
            .any(|tab| tab.title() == "Calendar" && tab.id() != app.active_tab().unwrap().id())
    );
}

#[test]
fn the_panel_of_an_entry_whose_id_another_file_shares_says_open_might_show_that_file() {
    let fixture = Fixture::new(&[
        (
            "Plan.md",
            &object_file(
                "note",
                A,
                "date: 2026-10-21
",
            ),
        ),
        // A copy made in another app, with the same ID.
        (
            "Plan copy.md",
            &object_file(
                "note",
                A,
                "date: 2026-10-22
",
            ),
        ),
        (
            "Review.md",
            &object_file(
                "note",
                B,
                "date: 2026-10-21
",
            ),
        ),
    ]);
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T15:00:00Z", "America/Toronto");
    let mut app = fixture.app_with(system);
    let Some(_calendar) = with_calendar(&mut app) else {
        return;
    };
    let shared = "Another file has the same ID, so Open might show that file.";

    // Paths order the copies, so the copy's file comes first.
    select(&mut app, &format!("note_{A}:date"));
    assert_eq!(
        texts_under(&app, "entry"),
        Some(vec![
            "Plan copy".to_owned(),
            "Type: note".to_owned(),
            shared.to_owned()
        ])
    );
    select(&mut app, &format!("note_{A}:date:2"));
    assert_eq!(
        texts_under(&app, "entry"),
        Some(vec![
            "Plan".to_owned(),
            "Type: note".to_owned(),
            shared.to_owned()
        ])
    );
    select(&mut app, &format!("note_{B}:date"));
    assert_eq!(
        texts_under(&app, "entry"),
        Some(vec!["Review".to_owned(), "Type: note".to_owned()])
    );
}

#[test]
fn opening_an_entry_from_the_grid_shows_its_object_and_close_hides_the_panel() {
    let (fixture, system) = week();
    let mut app = fixture.app_with(system);
    let Some(_calendar) = with_calendar(&mut app) else {
        return;
    };
    let review = format!("note_{B}:start");

    calendar::send(
        &mut app,
        PluginEvent::Activated(Target {
            node: "calendar".to_owned(),
            item: review.clone(),
        }),
    );

    assert_eq!(app.active_tab().map(|tab| tab.title()), Some("Review"));
    assert_eq!(
        texts_under(&app, "entry").unwrap_or_default().first(),
        Some(&"Review".to_owned())
    );
    press(&mut app, "close-entry");
    assert_eq!(texts_under(&app, "entry"), None);
}

#[test]
fn outside_edits_that_add_move_or_remove_a_date_update_the_open_view() {
    let (fixture, system) = week();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };
    show(&mut app, "week");
    let start_of = |app: &App, title: &str| {
        grid(app)
            .2
            .into_iter()
            .find(|entry| entry.title == title)
            .map(|entry| entry.start)
    };

    write(
        &fixture.root,
        "Dentist.md",
        &object_file("note", "01jbcdefghjkmnpqrstvwxyz0j", "due: 2026-10-23\n"),
    );
    wait_until(&mut app, "the new date shows", |app| {
        start_of(app, "Dentist").as_deref() == Some("2026-10-23")
    });

    write(
        &fixture.root,
        "Dentist.md",
        &object_file("note", "01jbcdefghjkmnpqrstvwxyz0j", "due: 2026-10-22\n"),
    );
    wait_until(&mut app, "the moved date shows", |app| {
        start_of(app, "Dentist").as_deref() == Some("2026-10-22")
    });

    write(
        &fixture.root,
        "Dentist.md",
        &object_file("note", "01jbcdefghjkmnpqrstvwxyz0j", ""),
    );
    wait_until(&mut app, "the removed date leaves", |app| {
        start_of(app, "Dentist").is_none()
    });
    assert!(titles(&app).contains(&"Standup".to_owned()));
}
