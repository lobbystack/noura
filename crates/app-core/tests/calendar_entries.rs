//! Seam 3 on the real host, against real files: the Calendar plugin finds
//! its entries in any object's `due`, `date` or `start` and `end`. An
//! all-day value is a calendar date and a timed one an instant with an
//! explicit offset. A malformed start drops the entry, and a malformed or
//! non-increasing end makes it a point. Entries order by start, then
//! title, stable ID and property.
//!
//! The calendar cases of the shared `queries-v1` fixture, which the
//! kernel's index answers today, give the same answers through the
//! plugin's day view in UTC. The fixture's one invalid range has no view:
//! the plugin's own tests check it against its range rules.

mod support;

use serde_json::Value;
use support::{
    FakeSystem, Fixture,
    calendar::{Shown, go_to, grid, show, with_calendar},
};

const QUERIES: &str = include_str!("../../../docs/workspace-format/fixtures/queries-v1.json");

/// An object's file of type `object_type` with the stable ID suffix
/// `suffix` and `properties` as frontmatter lines.
fn object_file(object_type: &str, suffix: &str, properties: &str) -> Vec<u8> {
    format!(
        "---\nid: {object_type}_{suffix}\ntype: {object_type}\n{properties}\
         created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\nNotes.\n"
    )
    .into_bytes()
}

fn shown(id: &str, title: &str, start: &str, end: Option<&str>) -> Shown {
    Shown {
        id: id.to_owned(),
        title: title.to_owned(),
        start: start.to_owned(),
        end: end.map(str::to_owned),
    }
}

#[test]
fn any_objects_dates_become_entries_and_bad_values_drop_the_entry_or_make_it_a_point() {
    const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";
    const B: &str = "01jbcdefghjkmnpqrstvwxyz0b";
    const C: &str = "01jbcdefghjkmnpqrstvwxyz0c";
    const D: &str = "01jbcdefghjkmnpqrstvwxyz0d";
    const E: &str = "01jbcdefghjkmnpqrstvwxyz0e";
    const F: &str = "01jbcdefghjkmnpqrstvwxyz0f";
    const G: &str = "01jbcdefghjkmnpqrstvwxyz0g";
    let workspace = Fixture::new(&[
        (
            "Retreat.md",
            &object_file("project", A, "start: 2026-10-19\nend: 2026-10-22\n"),
        ),
        (
            // A type no running plugin owns.
            "recipes/Bread.md",
            &object_file("recipe", B, "date: 2026-10-21\n"),
        ),
        (
            "tasks/Call.md",
            &object_file(
                "task",
                C,
                "due: 2026-10-21T09:00:00-04:00\nstart: 2026-10-21T08:00:00-04:00\n\
                 end: 2026-10-21T08:30:00-04:00\n",
            ),
        ),
        (
            "Backwards.md",
            &object_file("note", D, "start: 2026-10-21\nend: 2026-10-20\n"),
        ),
        (
            "Mixed.md",
            &object_file("note", E, "start: 2026-10-21\nend: 2026-10-22T10:00:00Z\n"),
        ),
        (
            "Soon.md",
            &object_file(
                "note",
                F,
                "start: 2026-10-21T13:00:00Z\nend: soon\ndate: next tuesday\n",
            ),
        ),
        (
            // A time without an offset is no instant.
            "Local.md",
            &object_file("note", G, "due: 2026-10-21T09:00:00\n"),
        ),
    ]);
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T15:00:00Z", "UTC");
    let mut app = workspace.app_with(system);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };

    show(&mut app, "day");

    assert_eq!(
        grid(&app).2,
        [
            shown(
                &format!("project_{A}:start"),
                "Retreat",
                "2026-10-19",
                Some("2026-10-22")
            ),
            shown(&format!("note_{D}:start"), "Backwards", "2026-10-21", None),
            shown(&format!("recipe_{B}:date"), "Bread", "2026-10-21", None),
            shown(&format!("note_{E}:start"), "Mixed", "2026-10-21", None),
            shown(
                &format!("task_{C}:start"),
                "Call",
                "2026-10-21T12:00:00Z",
                Some("2026-10-21T12:30:00Z")
            ),
            // The object's end comes before its due time.
            shown(
                &format!("task_{C}:due"),
                "Call",
                "2026-10-21T13:00:00Z",
                None
            ),
            shown(
                &format!("note_{F}:start"),
                "Soon",
                "2026-10-21T13:00:00Z",
                None
            ),
        ]
    );
}

#[test]
fn the_queries_v1_calendar_cases_give_the_same_answers_through_the_plugin() {
    let fixture: Value = serde_json::from_str(QUERIES).unwrap();
    let files: Vec<(&str, &[u8])> = fixture["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| {
            (
                file["path"].as_str().unwrap(),
                file["content"].as_str().unwrap().as_bytes(),
            )
        })
        .collect();
    let workspace = Fixture::new(&files);
    let system = FakeSystem::new();
    system.set_clock("2026-09-04T12:00:00Z", "UTC");
    let mut app = workspace.app_with(system);
    let Some(_folder) = with_calendar(&mut app) else {
        return;
    };
    show(&mut app, "day");

    let mut checked = 0;
    for case in fixture["calendarQueries"].as_array().unwrap() {
        if case.get("error").is_some() {
            continue;
        }
        let start = case["start"].as_str().unwrap();
        let end = case["end"].as_str().unwrap();
        let day: jiff::civil::Date = start[..10].parse().unwrap();
        // Each case asks for one whole day in UTC, which is what the day
        // view shows with the clock in UTC.
        assert!(start.len() == 10 || start.ends_with("T00:00:00Z"), "{case}");
        assert_eq!(&end[..10], day.tomorrow().unwrap().to_string(), "{case}");

        go_to(&mut app, &day.to_string());

        let answers: Vec<Value> = grid(&app)
            .2
            .iter()
            .map(|entry| {
                let (id, property) = entry.source();
                serde_json::json!([id, property])
            })
            .collect();
        assert_eq!(Value::Array(answers), case["expected"], "calendar {case}");
        checked += 1;
    }
    assert_eq!(checked, 2);
}
