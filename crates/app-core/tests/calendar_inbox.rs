//! Seam 1: the Inbox's Next 7 days with the clock stopped at 11:00 on
//! Wednesday 21 October 2026 in Toronto. With Calendar on, the section
//! lists up to five entries from the start of today through seven days
//! ahead, in the device's time zone, each with when it starts; it says
//! "Nothing scheduled this week." when nothing is, and it leaves the Inbox
//! when Calendar turns off.

mod support;

use app_core::{App, PluginEvent, Target, ViewKind, ViewNode};
use support::{FakeSystem, Fixture, calendar::with_calendar, plugins::wait_until};

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
