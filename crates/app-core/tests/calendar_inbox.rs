//! Seam 1: the Inbox's Next 7 days with the clock stopped at 11:00 on
//! Wednesday 21 October 2026 in Toronto. With Calendar on, the section
//! lists up to five entries from the start of today through seven days
//! ahead, in the device's time zone, each with when it starts, in the
//! order they start on the device's clock; it says
//! "Nothing scheduled this week." when nothing is, and it leaves the Inbox
//! when Calendar turns off.

mod support;

use app_core::{
    App, Field, INBOX_PAGE, InputValue, OpenAs, PluginEvent, Target, ViewKind, ViewNode,
};
use support::{
    FakeSystem, Fixture,
    calendar::{self, with_calendar},
    plugins::wait_until,
    tasks::{self, with_tasks},
};

/// A note's file with the stable ID suffix `suffix` and `properties` as
/// frontmatter lines.
fn note(suffix: &str, properties: &str) -> Vec<u8> {
    format!(
        "---\nid: note_01jbcdefghjkmnpqrstvwxyz0{suffix}\ntype: note\n{properties}\
         created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\nNotes.\n"
    )
    .into_bytes()
}

fn app_on(fixture: &Fixture) -> App {
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T15:00:00Z", "America/Toronto");
    fixture.app_with(system)
}

/// A line of a section: an item's label and detail, or a line of text.
type Line = (String, Option<String>);

/// What Calendar's section shows, under its title, once it has drawn.
fn section(app: &App) -> Option<(String, Vec<Line>)> {
    let section = app
        .inbox()
        .sections
        .into_iter()
        .find(|section| section.plugin_id == "calendar")?;
    let view = section.content.ok()?;
    let mut lines = Vec::new();
    collect(view.root(), &mut lines);
    Some((section.title, lines))
}

fn collect(node: &ViewNode, lines: &mut Vec<Line>) {
    match &node.kind {
        ViewKind::Text(text) => lines.push((text.clone(), None)),
        ViewKind::Item { label, detail, .. } => lines.push((label.clone(), detail.clone())),
        _ => {}
    }
    for child in &node.children {
        collect(child, lines);
    }
}

fn line(label: &str, detail: &str) -> Line {
    (label.to_owned(), Some(detail.to_owned()))
}

#[test]
fn next_7_days_lists_five_entries_from_the_start_of_today_with_readable_dates() {
    let fixture = Fixture::new(&[
        // 23:00 yesterday in Toronto, though already today in UTC.
        ("Last night.md", &note("a", "due: 2026-10-21T03:00:00Z\n")),
        ("Yesterday.md", &note("b", "date: 2026-10-20\n")),
        // 09:00 today, before the clock's 11:00.
        ("Standup.md", &note("c", "start: 2026-10-21T13:00:00Z\n")),
        ("Dentist.md", &note("d", "due: 2026-10-21T18:30:00Z\n")),
        ("Market.md", &note("e", "date: 2026-10-22\n")),
        // 21:00 tomorrow in Toronto, the day after in UTC.
        ("Late call.md", &note("f", "due: 2026-10-23T01:00:00Z\n")),
        ("Retreat.md", &note("g", "date: 2026-10-24\n")),
        ("Planting.md", &note("h", "date: 2026-10-27\n")),
        ("Too far.md", &note("j", "date: 2026-10-28\n")),
    ]);
    let mut app = app_on(&fixture);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    wait_until(&mut app, "the section draws", |app| section(app).is_some());

    assert_eq!(
        section(&app),
        Some((
            "Next 7 days".to_owned(),
            vec![
                line("Standup", "09:00"),
                line("Dentist", "14:30"),
                line("Market", "Tomorrow"),
                line("Late call", "Tomorrow 21:00"),
                line("Retreat", "Saturday"),
            ]
        ))
    );
}

#[test]
fn an_entry_this_evening_lists_before_tomorrows_all_day_entries() {
    // 21:00 tonight in Toronto is already tomorrow in UTC, after the
    // midnight in UTC that the rules give tomorrow's all-day entries.
    let fixture = Fixture::new(&[
        (
            "Evening call.md",
            &note(
                "a",
                "due: 2026-10-22T01:00:00Z
",
            ),
        ),
        (
            "Fair.md",
            &note(
                "b",
                "date: 2026-10-22
",
            ),
        ),
        (
            "Garden.md",
            &note(
                "c",
                "date: 2026-10-22
",
            ),
        ),
        (
            "Harvest.md",
            &note(
                "d",
                "date: 2026-10-22
",
            ),
        ),
        (
            "Market.md",
            &note(
                "e",
                "date: 2026-10-22
",
            ),
        ),
        (
            "Picnic.md",
            &note(
                "f",
                "date: 2026-10-22
",
            ),
        ),
    ]);
    let mut app = app_on(&fixture);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    wait_until(&mut app, "the section draws", |app| section(app).is_some());

    assert_eq!(
        section(&app).map(|(_, lines)| lines),
        Some(vec![
            line("Evening call", "21:00"),
            line("Fair", "Tomorrow"),
            line("Garden", "Tomorrow"),
            line("Harvest", "Tomorrow"),
            line("Market", "Tomorrow"),
        ])
    );
}

#[test]
fn an_entry_opens_from_the_section_in_a_tab() {
    let fixture = Fixture::new(&[("Market.md", &note("e", "date: 2026-10-22\n"))]);
    let mut app = app_on(&fixture);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };
    wait_until(&mut app, "the section draws", |app| section(app).is_some());

    app.send_inbox_event(
        "calendar",
        "next-7-days",
        &PluginEvent::Activated(Target {
            node: "upcoming".to_owned(),
            item: "note_01jbcdefghjkmnpqrstvwxyz0e:date".to_owned(),
        }),
    );
    support::settle(&mut app);

    assert_eq!(app.active_tab().map(|tab| tab.title()), Some("Market"));
}

/// One task, from New task to the Inbox: its due date goes in from its
/// panel, it shows on the calendar and under Next 7 days, and the Inbox
/// opens it with its text.
#[test]
fn a_task_due_today_shows_on_the_calendar_and_in_the_inbox_and_opens() {
    let fixture = Fixture::new(&[]);
    let mut app = app_on(&fixture);
    let Some(_tasks) = with_tasks(&mut app) else {
        return;
    };
    let Some(_calendar) = with_calendar(&mut app) else {
        return;
    };
    app.open_page(tasks::PAGE, OpenAs::Preview);
    support::settle(&mut app);
    tasks::name_new_task(&mut app, "Dentist");
    wait_until(&mut app, "the task opens", |app| {
        app.active_tab().is_some_and(|tab| tab.title() == "Dentist")
    });
    let tab = app.active_tab().unwrap().id();
    wait_until(&mut app, "the task's panel draws", |app| {
        app.plugin_panel(tab)
            .and_then(Result::ok)
            .is_some_and(|view| tasks::find(view.root(), "due").is_some())
    });

    app.send_panel_event(
        tab,
        &PluginEvent::Changed(Field {
            node: "due".to_owned(),
            value: InputValue::Date(Some("2026-10-21".to_owned())),
        }),
    );
    support::settle(&mut app);
    let bytes = fixture.read("tasks/Dentist.md");
    assert_eq!(tasks::property(&bytes, "due"), Some("2026-10-21".into()));
    let id = local_object_id(&bytes);

    app.open_page(calendar::PAGE, OpenAs::Preview);
    wait_until(&mut app, "the calendar shows the task", |app| {
        matches!(app.plugin_view(calendar::PAGE), Some(Ok(_)))
            && calendar::titles(app).iter().any(|title| title == "Dentist")
    });

    app.open_page(INBOX_PAGE, OpenAs::Preview);
    wait_until(&mut app, "Next 7 days lists the task", |app| {
        section(app).is_some_and(|(_, lines)| lines.iter().any(|(label, _)| label == "Dentist"))
    });
    app.send_inbox_event(
        "calendar",
        "next-7-days",
        &PluginEvent::Activated(Target {
            node: "upcoming".to_owned(),
            item: format!("{id}:due"),
        }),
    );
    support::settle(&mut app);

    let tab = app.active_tab().expect("the task opens");
    assert_eq!(tab.title(), "Dentist");
    assert_eq!(
        tab.file().map(|file| file.relative_path.as_str()),
        Some("tasks/Dentist.md")
    );
    let tab = tab.id();
    assert_eq!(app.note(tab).unwrap().text(), "");
    wait_until(&mut app, "the task opens beside its panel", |app| {
        app.plugin_panel(tab)
            .and_then(Result::ok)
            .is_some_and(|view| tasks::find(view.root(), "due").is_some())
    });
}

fn local_object_id(bytes: &[u8]) -> String {
    match local_core::parse_markdown("x.md", bytes) {
        local_core::ParsedMarkdown::Managed(object) => object.id,
        other => panic!("not a managed file: {other:?}"),
    }
}

#[test]
fn an_empty_week_says_so_and_the_section_leaves_when_calendar_turns_off() {
    let fixture = Fixture::new(&[
        ("Yesterday.md", &note("b", "date: 2026-10-20\n")),
        ("Too far.md", &note("j", "date: 2026-10-28\n")),
    ]);
    let mut app = app_on(&fixture);
    assert!(section(&app).is_none());
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };
    wait_until(&mut app, "the section draws", |app| section(app).is_some());

    assert_eq!(
        section(&app),
        Some((
            "Next 7 days".to_owned(),
            vec![("Nothing scheduled this week.".to_owned(), None)]
        ))
    );

    assert!(app.set_plugin_enabled("calendar", false));
    support::settle(&mut app);
    assert!(section(&app).is_none());
}
