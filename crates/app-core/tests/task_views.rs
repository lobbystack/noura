//! Seam 1: the Tasks page's views with the clock stopped. Today holds the
//! tasks not marked done that are due by the end of today, overdue
//! included, Upcoming those due later, All every task and Completed those
//! marked done. A project view holds the project's tasks and a folder view
//! the tasks in that folder. Lists order tasks not done first, then by
//! due date with dated tasks first, then by priority, then by last
//! update. The sidebar lists the views, then the projects, then the
//! folders that hold tasks; filters narrow the view and combine; and
//! overdue tasks say so in the device's time zone.

mod support;

use app_core::{InputValue, PluginEvent};
use support::{
    FakeSystem, Fixture,
    tasks::{
        self, A, B, C, D, E, F, G, H, SidebarList, labels, project_file, rows, send, sidebar,
        task_file, text, with_tasks,
    },
};

const J: &str = "01jbcdefghjkmnpqrstvwxyz0j";
const K: &str = "01jbcdefghjkmnpqrstvwxyz0k";

/// A workspace whose clock reads 11:00 on 21 October 2026 in Toronto, with
/// tasks due before, on and after that day, one marked done, two without a
/// due date, one in a project and the Garden project itself.
fn garden() -> (Fixture, std::sync::Arc<FakeSystem>) {
    let project = format!("project_{H}");
    let in_garden = format!("project: {project}\n");
    let fixture = Fixture::new(&[
        (
            "tasks/Water the beds.md",
            &task_file(
                A,
                "status: todo\npriority: medium\ndue: 2026-10-19\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        (
            "tasks/Pay rent.md",
            &task_file(
                B,
                "status: todo\npriority: urgent\ndue: 2026-10-21\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        (
            // 23:30 in Toronto is already the 22nd in UTC.
            "tasks/Call the vet.md",
            &task_file(
                C,
                "status: in-progress\npriority: low\ndue: 2026-10-21T23:30:00-04:00\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        (
            "tasks/Order seeds.md",
            &task_file(
                D,
                "status: todo\npriority: medium\ndue: 2026-10-22\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        (
            "tasks/Mulch.md",
            &task_file(
                E,
                "status: done\npriority: medium\ndue: 2026-10-20\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        (
            "tasks/Read the manual.md",
            &task_file(
                F,
                "status: todo\npriority: medium\n",
                "2026-10-02T10:00:00Z",
            ),
        ),
        (
            "Shed/Sharpen tools.md",
            &task_file(
                G,
                &format!("status: todo\npriority: high\n{in_garden}"),
                "2026-10-03T10:00:00Z",
            ),
        ),
        (
            "Sweep.md",
            &task_file(
                J,
                "status: todo\npriority: medium\n",
                "2026-10-04T10:00:00Z",
            ),
        ),
        ("projects/Garden/Garden.md", &project_file(H)),
    ]);
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T15:00:00Z", "America/Toronto");
    (fixture, system)
}

fn view(app: &mut app_core::App, item: &str) -> Vec<String> {
    tasks::choose(app, "views", item);
    labels(app)
}

#[test]
fn today_upcoming_all_and_completed_hold_the_tasks_their_rules_say_in_list_order() {
    let (fixture, system) = garden();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };

    assert_eq!(text(&app, "view").as_deref(), Some("Today"));
    assert_eq!(labels(&app), ["Water the beds", "Pay rent", "Call the vet"]);
    assert_eq!(view(&mut app, "view:upcoming"), ["Order seeds"]);
    assert_eq!(
        view(&mut app, "view:all"),
        [
            "Water the beds",
            "Pay rent",
            "Call the vet",
            "Order seeds",
            "Sharpen tools",
            "Read the manual",
            "Sweep",
            "Mulch",
        ]
    );
    assert_eq!(view(&mut app, "view:completed"), ["Mulch"]);
    assert_eq!(
        view(&mut app, "view:today"),
        ["Water the beds", "Pay rent", "Call the vet"]
    );
}

#[test]
fn a_project_view_holds_the_projects_tasks_and_a_folder_view_the_tasks_in_its_folder() {
    let (fixture, system) = garden();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };

    tasks::choose(&mut app, "projects", &format!("project:project_{H}"));
    assert_eq!(text(&app, "view").as_deref(), Some("Garden"));
    assert_eq!(labels(&app), ["Sharpen tools"]);

    tasks::choose(&mut app, "folders", "folder:");
    assert_eq!(text(&app, "view").as_deref(), Some("Top level"));
    assert_eq!(labels(&app), ["Sweep"]);

    tasks::choose(&mut app, "folders", "folder:Shed");
    assert_eq!(labels(&app), ["Sharpen tools"]);

    tasks::choose(&mut app, "folders", "folder:tasks");
    assert_eq!(
        labels(&app),
        [
            "Water the beds",
            "Pay rent",
            "Call the vet",
            "Order seeds",
            "Read the manual",
            "Mulch",
        ]
    );
}

#[test]
fn lists_order_by_done_then_due_date_then_priority_then_last_update() {
    let task =
        |suffix: &str, properties: &str, updated: &str| task_file(suffix, properties, updated);
    let fixture = Fixture::new(&[
        (
            "tasks/Low.md",
            &task(
                A,
                "priority: low\ndue: 2026-10-25\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        (
            "tasks/Urgent.md",
            &task(
                B,
                "priority: urgent\ndue: 2026-10-25\n",
                "2026-10-05T10:00:00Z",
            ),
        ),
        (
            "tasks/Newer.md",
            &task(
                C,
                "priority: medium\ndue: 2026-10-25\n",
                "2026-10-04T10:00:00Z",
            ),
        ),
        (
            "tasks/Older.md",
            &task(
                D,
                "priority: medium\ndue: 2026-10-25\n",
                "2026-10-02T10:00:00Z",
            ),
        ),
        (
            "tasks/Earlier.md",
            &task(
                E,
                "priority: low\ndue: 2026-10-24\n",
                "2026-10-09T10:00:00Z",
            ),
        ),
        (
            "tasks/Undated.md",
            &task(F, "priority: urgent\n", "2026-10-01T10:00:00Z"),
        ),
        (
            "tasks/Done.md",
            &task(
                G,
                "status: done\npriority: urgent\ndue: 2026-10-01\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        // Another app may write the last update with its own offset or
        // with fractions of a second: these sort by the instant they name.
        (
            "tasks/Offset.md",
            &task(
                H,
                "priority: medium\ndue: 2026-10-25\n",
                "2026-10-03T08:00:00-04:00",
            ),
        ),
        (
            "tasks/Zulu.md",
            &task(
                J,
                "priority: medium\ndue: 2026-10-25\n",
                "2026-10-03T10:00:00Z",
            ),
        ),
        (
            "tasks/Fraction.md",
            &task(
                K,
                "priority: medium\ndue: 2026-10-25\n",
                "2026-10-03T10:00:00.5Z",
            ),
        ),
    ]);
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T15:00:00Z", "America/Toronto");
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };

    assert_eq!(
        view(&mut app, "view:all"),
        [
            "Earlier", "Urgent", "Older", "Zulu", "Fraction", "Offset", "Newer", "Low", "Undated",
            "Done"
        ]
    );
}

#[test]
fn the_sidebar_lists_the_views_then_a_view_per_project_then_a_view_per_folder_with_counts() {
    let (fixture, system) = garden();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let item = |id: &str, label: &str, detail: Option<&str>| {
        (id.to_owned(), label.to_owned(), detail.map(str::to_owned))
    };

    assert_eq!(
        sidebar(&app),
        [
            SidebarList {
                id: "views".into(),
                items: vec![
                    item("view:today", "Today", None),
                    item("view:upcoming", "Upcoming", None),
                    item("view:all", "All", None),
                    item("view:completed", "Completed", None),
                ],
                selected: Some("view:today".into()),
            },
            SidebarList {
                id: "projects".into(),
                items: vec![item(&format!("project:project_{H}"), "Garden", None)],
                selected: None,
            },
            SidebarList {
                id: "folders".into(),
                items: vec![
                    item("folder:", "Top level", Some("1")),
                    item("folder:Shed", "Shed", Some("1")),
                    item("folder:tasks", "tasks", Some("6")),
                ],
                selected: None,
            },
        ]
    );

    // Picking a project moves the selection there.
    tasks::choose(&mut app, "projects", &format!("project:project_{H}"));
    let lists = sidebar(&app);
    assert_eq!(lists[0].selected, None);
    assert_eq!(lists[1].selected, Some(format!("project:project_{H}")));
}

fn changed(node: &str, value: InputValue) -> PluginEvent {
    PluginEvent::Changed(app_core::Field {
        node: node.to_owned(),
        value,
    })
}

fn choice(id: &str) -> InputValue {
    InputValue::Choice(Some(id.to_owned()))
}

#[test]
fn filters_for_status_project_folder_and_due_date_narrow_the_view_and_combine() {
    let (fixture, system) = garden();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    tasks::choose(&mut app, "views", "view:all");

    send(&mut app, changed("filter-status", choice("done")));
    assert_eq!(labels(&app), ["Mulch"]);
    send(&mut app, changed("filter-status", choice("any")));

    send(
        &mut app,
        changed("filter-project", choice(&format!("is:project_{H}"))),
    );
    assert_eq!(labels(&app), ["Sharpen tools"]);
    send(&mut app, changed("filter-project", choice("none")));
    assert!(!labels(&app).contains(&"Sharpen tools".to_owned()));
    send(&mut app, changed("filter-project", choice("any")));

    send(&mut app, changed("filter-folder", choice("in:")));
    assert_eq!(labels(&app), ["Sweep"]);
    send(&mut app, changed("filter-folder", choice("any")));

    // Due on or before the 21st: overdue, today and tonight, done or not.
    send(
        &mut app,
        changed("filter-due", InputValue::Date(Some("2026-10-21".into()))),
    );
    assert_eq!(
        labels(&app),
        ["Water the beds", "Pay rent", "Call the vet", "Mulch"]
    );

    // Filters combine, and with the view.
    send(&mut app, changed("filter-status", choice("todo")));
    assert_eq!(labels(&app), ["Water the beds", "Pay rent"]);
    send(&mut app, changed("filter-folder", choice("in:tasks")));
    assert_eq!(labels(&app), ["Water the beds", "Pay rent"]);
    tasks::choose(&mut app, "views", "view:upcoming");
    assert_eq!(labels(&app), Vec::<String>::new());
    assert_eq!(text(&app, "empty").as_deref(), Some("No tasks here"));

    send(&mut app, changed("filter-due", InputValue::Date(None)));
    send(&mut app, changed("filter-status", choice("any")));
    send(&mut app, changed("filter-folder", choice("any")));
    assert_eq!(labels(&app), ["Order seeds"]);
}

#[test]
fn a_task_due_before_today_in_the_devices_time_zone_shows_as_overdue() {
    // 19:00 on the 20th in Los Angeles is already the 21st in UTC.
    let fixture = Fixture::new(&[
        (
            "tasks/Yesterday.md",
            &task_file(A, "due: 2026-10-19\n", "2026-10-01T10:00:00Z"),
        ),
        (
            "tasks/Today.md",
            &task_file(B, "due: 2026-10-20\n", "2026-10-01T10:00:00Z"),
        ),
        (
            "tasks/Tonight.md",
            &task_file(
                C,
                "due: 2026-10-20T22:00:00-07:00\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        (
            "tasks/Last week.md",
            &task_file(D, "status: done\ndue: 2026-10-13\n", "2026-10-01T10:00:00Z"),
        ),
        (
            "tasks/This morning.md",
            &task_file(E, "due: 2026-10-20T08:00:00Z\n", "2026-10-01T10:00:00Z"),
        ),
    ]);
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T02:00:00Z", "America/Los_Angeles");
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    tasks::choose(&mut app, "views", "view:all");

    let warnings: Vec<(String, Option<String>)> = rows(&app)
        .into_iter()
        .map(|row| (row.label, row.warning))
        .collect();

    let overdue = Some("Overdue".to_owned());
    assert_eq!(
        warnings,
        [
            ("Yesterday".to_owned(), overdue),
            ("Today".to_owned(), None),
            // 08:00 UTC on the 20th is 01:00 on the 20th in Los Angeles.
            ("This morning".to_owned(), None),
            ("Tonight".to_owned(), None),
            ("Last week".to_owned(), None),
        ]
    );
}

#[test]
fn each_row_shows_its_due_date_project_and_priority_and_a_checkbox() {
    let (fixture, system) = garden();
    let mut app = fixture.app_with(system);
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    tasks::choose(&mut app, "views", "view:all");

    let shown: Vec<(String, Option<String>, Option<bool>)> = rows(&app)
        .into_iter()
        .map(|row| (row.label, row.detail, row.checked))
        .collect();

    let row = |label: &str, detail: Option<&str>, checked: bool| {
        (label.to_owned(), detail.map(str::to_owned), Some(checked))
    };
    assert_eq!(
        shown,
        [
            row("Water the beds", Some("Oct 19"), false),
            row("Pay rent", Some("Today · Urgent"), false),
            row("Call the vet", Some("23:30 · Low"), false),
            row("Order seeds", Some("Tomorrow"), false),
            row("Sharpen tools", Some("Garden · High"), false),
            row("Read the manual", None, false),
            row("Sweep", None, false),
            row("Mulch", Some("Yesterday"), true),
        ]
    );
}
