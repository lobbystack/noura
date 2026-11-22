//! Seam 1: Calendar's calendars with the clock stopped at 11:00 on
//! Wednesday 21 October 2026 in Toronto. Your calendars are files with a
//! name and a color, and every other type with dates gets a built-in
//! calendar under Other. Each entry draws in its calendar's color, and
//! hiding a calendar hides its entries in every view and the Inbox, which
//! the settings file keeps. You create, rename, recolor and delete
//! calendars, create events in one, edit them in place and move them to
//! another, and outside edits, copies, broken files and a rebuilt index
//! leave the same calendars.

use crate::support;
use app_core::{
    App, Field, InputValue, PluginCheck, PluginColor, PluginEvent, PluginNaming, Target, ViewKind,
};
use support::{
    FakeSystem, Fixture,
    calendar::{
        self, find, grid, listed, page, press, send, send_sidebar, show, texts_under, with_calendar,
    },
    index_files,
    plugins::wait_until,
    write,
};

const HOME: &str = "calendar_01jbcdefghjkmnpqrstvwxyz0a";
const WORK: &str = "calendar_01jbcdefghjkmnpqrstvwxyz0b";
const GONE: &str = "calendar_01jbcdefghjkmnpqrstvwxyz0z";
const SETTINGS: &str = "Calendars/Calendar settings.md";

fn calendar_file(id: &str, color: &str) -> Vec<u8> {
    format!("---\nid: {id}\ntype: calendar\ncolor: {color}\n---\n").into_bytes()
}

fn event_file(suffix: &str, calendar: &str, start: &str, end: Option<&str>) -> Vec<u8> {
    let end = end.map(|end| format!("end: {end}\n")).unwrap_or_default();
    format!(
        "---\nid: event_01jbcdefghjkmnpqrstvwxyz{suffix}\ntype: event\ncalendar: {calendar}\n\
         start: {start}\n{end}---\n"
    )
    .into_bytes()
}

/// Home and Work, an event in each, one whose calendar is gone, a task
/// due tomorrow and a Transcription on Friday.
fn workspace() -> Fixture {
    Fixture::new(&[
        ("Calendars/Home.md", &calendar_file(HOME, "blue")),
        ("Calendars/Work.md", &calendar_file(WORK, "green")),
        (
            "Events/Standup.md",
            &event_file(
                "1a",
                WORK,
                "2026-10-21T13:00:00Z",
                Some("2026-10-21T13:30:00Z"),
            ),
        ),
        (
            "Events/Dinner.md",
            &event_file(
                "1b",
                HOME,
                "2026-10-21T22:00:00Z",
                Some("2026-10-21T23:00:00Z"),
            ),
        ),
        (
            "Events/Lost.md",
            &event_file("1c", GONE, "2026-10-22", None),
        ),
        (
            "Tasks/Water.md",
            b"---\nid: task_01jbcdefghjkmnpqrstvwxyz2a\ntype: task\ndue: 2026-10-22\n---\n",
        ),
        (
            "Meetings/Sync/Sync.md",
            b"---\nid: transcription_01jbcdefghjkmnpqrstvwxyz3a\ntype: transcription\n\
              start: 2026-10-23T14:00:00Z\nend: 2026-10-23T14:30:00Z\n---\n",
        ),
    ])
}

fn app_on(fixture: &Fixture) -> App {
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T15:00:00Z", "America/Toronto");
    fixture.app_with(system)
}

fn item(id: &str, name: &str, checked: Option<bool>, color: PluginColor) -> calendar::Listed {
    (id.to_owned(), name.to_owned(), checked, Some(color))
}

/// The grid's entries as titles with their colors, in order.
fn colored(app: &App) -> Vec<(String, Option<PluginColor>)> {
    grid(app)
        .2
        .into_iter()
        .map(|entry| (entry.title, entry.color))
        .collect()
}

fn titles(app: &App) -> Vec<String> {
    calendar::titles(app)
}

fn text(app: &App, path: &str, fixture: &Fixture) -> Option<String> {
    fixture
        .exists(path)
        .then(|| String::from_utf8(fixture.read(path)).unwrap())
        .filter(|_| app.workspace().is_some())
}

fn check(list: &str, item: &str, checked: bool) -> PluginEvent {
    PluginEvent::Checked(PluginCheck {
        node: list.to_owned(),
        item: item.to_owned(),
        checked,
    })
}

fn named(node: &str, name: &str) -> PluginEvent {
    PluginEvent::Named(PluginNaming {
        node: node.to_owned(),
        name: name.to_owned(),
    })
}

fn field(node: &str, value: InputValue) -> Field {
    Field {
        node: node.to_owned(),
        value,
    }
}

/// Change the popover's control `node` to `value`, as a pick or leaving
/// a text does.
fn change(app: &mut App, node: &str, value: InputValue) {
    send(app, PluginEvent::Changed(field(node, value)));
}

fn choice(value: &str) -> InputValue {
    InputValue::Choice(Some(value.to_owned()))
}

fn day(value: &str) -> InputValue {
    InputValue::Date(Some(value.to_owned()))
}

/// The option the picker `id` of the page shows chosen.
fn chosen(app: &App, id: &str) -> Option<String> {
    let view = page(app);
    match &find(view.root(), id)?.kind {
        ViewKind::Picker { chosen, .. } => chosen.clone(),
        _ => None,
    }
}

#[test]
fn the_sidebar_lists_your_calendars_then_the_built_in_ones_and_entries_take_their_colors() {
    let fixture = workspace();
    let mut app = app_on(&fixture);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    assert_eq!(
        listed(&app, "calendars"),
        [
            item(
                &format!("calendar:{HOME}"),
                "Home",
                Some(true),
                PluginColor::Blue
            ),
            item(
                &format!("calendar:{WORK}"),
                "Work",
                Some(true),
                PluginColor::Green
            ),
        ]
    );
    // Events belong to your calendars, so Other lists only the types
    // whose objects have dates.
    assert_eq!(
        listed(&app, "other-calendars"),
        [
            item(
                "type:transcription",
                "Meetings",
                Some(true),
                PluginColor::Purple
            ),
            item("type:task", "Tasks", Some(true), PluginColor::Orange),
        ]
    );
    let shown = colored(&app);
    let color_of = |title: &str| {
        shown
            .iter()
            .find(|(shown, _)| shown == title)
            .unwrap_or_else(|| panic!("{title} shows"))
            .1
    };
    assert_eq!(color_of("Standup"), Some(PluginColor::Green));
    assert_eq!(color_of("Dinner"), Some(PluginColor::Blue));
    // Its calendar is gone, so it shows in the first by name.
    assert_eq!(color_of("Lost"), Some(PluginColor::Blue));
    assert_eq!(color_of("Water"), Some(PluginColor::Orange));
    assert_eq!(color_of("Sync"), Some(PluginColor::Purple));
}

#[test]
fn hiding_a_calendar_hides_its_entries_in_every_view_and_the_settings_file_keeps_it() {
    let fixture = workspace();
    let mut app = app_on(&fixture);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    send_sidebar(
        &mut app,
        check("calendars", &format!("calendar:{WORK}"), false),
    );
    send_sidebar(&mut app, check("other-calendars", "type:task", false));

    let settings = text(&app, SETTINGS, &fixture).expect("the settings file is written");
    assert!(
        settings.contains(&format!("hidden_calendars:\n- {WORK}\n")),
        "{settings}"
    );
    assert!(settings.contains("hidden_types:\n- task\n"), "{settings}");
    assert_eq!(listed(&app, "calendars")[1].2, Some(false));
    assert_eq!(listed(&app, "other-calendars")[1].2, Some(false));
    for mode in ["month", "week", "day"] {
        show(&mut app, mode);
        let shown = titles(&app);
        assert!(!shown.contains(&"Standup".to_owned()), "{mode}: {shown:?}");
        assert!(!shown.contains(&"Water".to_owned()), "{mode}: {shown:?}");
        assert!(shown.contains(&"Dinner".to_owned()), "{mode}: {shown:?}");
    }
    let inbox = inbox_titles(&app);
    assert!(inbox.contains(&"Dinner".to_owned()), "{inbox:?}");
    assert!(!inbox.contains(&"Standup".to_owned()), "{inbox:?}");
    assert!(!inbox.contains(&"Water".to_owned()), "{inbox:?}");

    send_sidebar(
        &mut app,
        check("calendars", &format!("calendar:{WORK}"), true),
    );
    assert!(titles(&app).contains(&"Standup".to_owned()));
    let settings = text(&app, SETTINGS, &fixture).unwrap();
    assert!(settings.contains("hidden_calendars: []\n"), "{settings}");
}

/// The titles of the Inbox's Next 7 days.
fn inbox_titles(app: &App) -> Vec<String> {
    let Some(section) = app
        .inbox()
        .sections
        .into_iter()
        .find(|section| section.plugin_id == "calendar")
    else {
        return Vec::new();
    };
    let view = section.content.expect("the section draws");
    let list = find(view.root(), "upcoming").expect("the section lists entries");
    list.children
        .iter()
        .filter_map(|item| match &item.kind {
            ViewKind::Item { label, .. } => Some(label.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn you_create_rename_recolor_and_delete_a_calendar() {
    let fixture = workspace();
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T15:00:00Z", "America/Toronto");
    let mut app = fixture.app_with(system.clone());
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    send_sidebar(&mut app, PluginEvent::Pressed("new-calendar".to_owned()));
    assert_eq!(
        listed(&app, "calendars").last().map(|item| item.0.as_str()),
        Some("new-calendar-item")
    );
    send_sidebar(&mut app, named("new-calendar-name", "Garden club"));
    let club = text(&app, "Calendars/Garden club.md", &fixture).expect("the calendar is a file");
    // Blue and green are taken, so it takes purple.
    assert!(club.contains("type: calendar\ncolor: purple\n"), "{club}");
    let id = support::object_id(club.as_bytes());
    assert_eq!(
        listed(&app, "calendars")[0],
        item(
            &format!("calendar:{id}"),
            "Garden club",
            Some(true),
            PluginColor::Purple
        )
    );

    send_sidebar(&mut app, PluginEvent::Chosen(format!("rename:{id}")));
    send_sidebar(&mut app, named("rename-calendar", "Allotment"));
    assert!(!fixture.exists("Calendars/Garden club.md"));
    let allotment = text(&app, "Calendars/Allotment.md", &fixture).unwrap();
    assert_eq!(support::object_id(allotment.as_bytes()), id);

    send_sidebar(&mut app, PluginEvent::Chosen(format!("color:red:{id}")));
    let allotment = text(&app, "Calendars/Allotment.md", &fixture).unwrap();
    assert!(allotment.contains("color: red\n"), "{allotment}");
    assert_eq!(listed(&app, "calendars")[0].3, Some(PluginColor::Red));

    // An empty calendar goes to the trash at once.
    send_sidebar(&mut app, PluginEvent::Chosen(format!("delete:{id}")));
    assert!(!fixture.exists("Calendars/Allotment.md"));
    assert!(system.in_bin("Allotment.md").exists());

    // Work holds Standup, so Calendar asks first.
    send_sidebar(&mut app, PluginEvent::Chosen(format!("delete:{WORK}")));
    assert_eq!(
        texts_under(&app, "delete-calendar"),
        Some(vec![
            "Delete \u{201c}Work\u{201d}?".to_owned(),
            "Its event moves to the trash too.".to_owned(),
        ])
    );
    assert!(fixture.exists("Calendars/Work.md"));
    press(&mut app, "confirm-delete");
    assert!(!fixture.exists("Calendars/Work.md"));
    assert!(!fixture.exists("Events/Standup.md"));
    assert!(system.in_bin("Standup.md").exists());
    assert_eq!(texts_under(&app, "delete-calendar"), None);
    assert_eq!(
        listed(&app, "calendars"),
        [item(
            &format!("calendar:{HOME}"),
            "Home",
            Some(true),
            PluginColor::Blue
        )]
    );
    // An event whose calendar was deleted outside Noura stays, in the
    // first calendar.
    assert!(fixture.exists("Events/Lost.md"));
}

#[test]
fn a_new_event_goes_to_the_selected_calendar_and_its_popover_edits_it_in_place() {
    let fixture = workspace();
    let mut app = app_on(&fixture);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    send_sidebar(
        &mut app,
        PluginEvent::Selected(Target {
            node: "calendars".to_owned(),
            item: format!("calendar:{WORK}"),
        }),
    );
    press(&mut app, "new-event");
    // New event makes the event at once, from 9:00 to 10:00 today, and
    // opens its popover beside it.
    let made = text(&app, "Events/New event.md", &fixture).expect("the event is a file");
    assert!(
        made.contains(&format!(
            "type: event\ncalendar: {WORK}\nstart: 2026-10-21T09:00:00-04:00\n\
             end: 2026-10-21T10:00:00-04:00\n"
        )),
        "{made}"
    );
    let view = page(&app);
    let popover = find(view.root(), "details").expect("the popover shows");
    assert!(
        matches!(&popover.kind, ViewKind::Popover { label, anchor }
            if label == "New event" && anchor.ends_with(":start")),
        "{:?}",
        popover.kind
    );
    assert_eq!(chosen(&app, "event-calendar").as_deref(), Some(WORK));
    assert_eq!(chosen(&app, "event-start").as_deref(), Some("09:00"));
    assert_eq!(chosen(&app, "event-end").as_deref(), Some("60"));

    // Each change writes the file at once.
    change(
        &mut app,
        "event-title",
        InputValue::Text("Dentist".to_owned()),
    );
    assert!(!fixture.exists("Events/New event.md"));
    change(&mut app, "event-day", day("2026-10-22"));
    change(&mut app, "event-start", choice("14:15"));
    change(&mut app, "event-end", choice("90"));
    change(
        &mut app,
        "event-notes",
        InputValue::Multiline("Bring the card.".to_owned()),
    );
    let dentist = text(&app, "Events/Dentist.md", &fixture).expect("the file is renamed");
    assert!(
        dentist.contains(&format!(
            "calendar: {WORK}\nstart: 2026-10-22T14:15:00-04:00\n\
             end: 2026-10-22T15:45:00-04:00\n"
        )),
        "{dentist}"
    );
    assert!(
        dentist.ends_with("---\nBring the card.\n") || dentist.ends_with("---\nBring the card."),
        "{dentist}"
    );
    wait_until(&mut app, "the event shows as Dentist", |app| {
        colored(app).contains(&("Dentist".to_owned(), Some(PluginColor::Green)))
    });

    // The calendar moves it to Home, in Home's color.
    change(&mut app, "event-calendar", choice(HOME));
    let dentist = text(&app, "Events/Dentist.md", &fixture).unwrap();
    assert!(
        dentist.contains(&format!("calendar: {HOME}\n")),
        "{dentist}"
    );
    wait_until(&mut app, "the event takes Home's color", |app| {
        colored(app).contains(&("Dentist".to_owned(), Some(PluginColor::Blue)))
    });

    // All day keeps its day; the popover shows the last day, and the file
    // holds the day after.
    change(&mut app, "event-all-day", InputValue::Checkbox(true));
    let dentist = text(&app, "Events/Dentist.md", &fixture).unwrap();
    assert!(dentist.contains("start: 2026-10-22\n"), "{dentist}");
    assert!(!dentist.contains("end:"), "{dentist}");
    change(&mut app, "event-last", day("2026-10-24"));
    let dentist = text(&app, "Events/Dentist.md", &fixture).unwrap();
    assert!(
        dentist.contains("start: 2026-10-22\nend: 2026-10-25\n"),
        "{dentist}"
    );

    // Escape or a click outside closes the popover, and a second new
    // event takes the next free name.
    send(&mut app, PluginEvent::Cancelled("details".to_owned()));
    assert!(find(page(&app).root(), "details").is_none());
    press(&mut app, "new-event");
    press(&mut app, "new-event");
    assert!(fixture.exists("Events/New event.md"));
    assert!(fixture.exists("Events/New event 1.md"));

    // Delete moves the event to the trash and closes its popover.
    press(&mut app, "delete-event");
    assert!(!fixture.exists("Events/New event 1.md"));
    assert!(find(page(&app).root(), "details").is_none());
}

#[test]
fn outside_edits_to_a_calendar_reach_the_sidebar_and_its_events_keep_it_by_id() {
    let fixture = workspace();
    let mut app = app_on(&fixture);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    std::fs::remove_file(fixture.root.join("Calendars/Work.md")).unwrap();
    write(
        &fixture.root,
        "Calendars/Office.md",
        &calendar_file(WORK, "red"),
    );

    wait_until(&mut app, "the renamed calendar shows", |app| {
        listed(app, "calendars").get(1)
            == Some(&item(
                &format!("calendar:{WORK}"),
                "Office",
                Some(true),
                PluginColor::Red,
            ))
    });
    wait_until(&mut app, "its event takes its new color", |app| {
        colored(app).contains(&("Standup".to_owned(), Some(PluginColor::Red)))
    });
}

#[test]
fn copied_and_broken_calendar_files_leave_one_calendar_each_and_the_view_draws() {
    let fixture = workspace();
    write(
        &fixture.root,
        "Calendars/Work copy.md",
        &calendar_file(WORK, "yellow"),
    );
    write(
        &fixture.root,
        "Calendars/Broken.md",
        b"---\nid: calendar_01jbcdefghjkmnpqrstvwxyz0c\ntype: calendar\ncolor: [red\n---\n",
    );
    write(
        &fixture.root,
        "Calendars/Teal.md",
        &calendar_file("calendar_01jbcdefghjkmnpqrstvwxyz0d", "teal"),
    );
    let mut app = app_on(&fixture);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    // The copy shares Work's ID, so the first file by path stands for
    // both; a color outside the palette draws blue; malformed YAML isn't
    // a calendar.
    assert_eq!(
        listed(&app, "calendars"),
        [
            item(
                &format!("calendar:{HOME}"),
                "Home",
                Some(true),
                PluginColor::Blue
            ),
            item(
                "calendar:calendar_01jbcdefghjkmnpqrstvwxyz0d",
                "Teal",
                Some(true),
                PluginColor::Blue
            ),
            item(
                &format!("calendar:{WORK}"),
                "Work copy",
                Some(true),
                PluginColor::Yellow
            ),
        ]
    );
    assert!(colored(&app).contains(&("Standup".to_owned(), Some(PluginColor::Yellow))));
}

#[test]
fn a_rebuilt_index_shows_the_same_calendars_from_the_files() {
    let fixture = workspace();
    let mut app = app_on(&fixture);
    let folder = with_calendar(&mut app);
    if folder.is_none() {
        return;
    }
    send_sidebar(
        &mut app,
        check("calendars", &format!("calendar:{WORK}"), false),
    );
    let before = (listed(&app, "calendars"), colored(&app));
    let index = index_files(&fixture, &app);
    drop(app);
    for file in index {
        let _ = std::fs::remove_file(file);
    }

    // The plugin stays installed and on in the app data.
    let mut app = app_on(&fixture);
    wait_until(&mut app, "the calendars show again", |app| {
        app.plugin_sidebar(calendar::PAGE)
            .and_then(Result::ok)
            .is_some_and(|_| !listed(app, "calendars").is_empty())
    });
    assert_eq!((listed(&app, "calendars"), colored(&app)), before);
}
