//! Seam 1: a task selected on the Tasks page shows its fields in the
//! page's right pane: status, priority, a due date you set and clear, and
//! a project picker with No project and every project. Each change in the
//! pane, and each check in a Tasks list, writes against the revision it
//! showed: after an outside edit the write fails, the file keeps the
//! outside bytes, and the latest version shows.

use crate::support;
use app_core::{App, Choice, InputValue, PluginCheck, PluginEvent, ViewKind};
use support::{
    Fixture, settle,
    tasks::{
        self, A, B, C, H, find, pane_change, pane_input, pane_text, project_file, property, rows,
        select_in_pane, send, task_file, text, with_tasks,
    },
};

const CHANGED_ELSEWHERE: &str = "It changed elsewhere. Showing the latest version.";

fn workspace() -> Fixture {
    Fixture::new(&[
        (
            "tasks/Paint the fence.md",
            &task_file(
                A,
                "status: todo\npriority: medium\ndue: 2026-10-24\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        ("projects/Garden/Garden.md", &project_file(H)),
        ("projects/Kitchen/Kitchen.md", &project_file(C)),
    ])
}

/// The task the right pane shows, standing for it in the calls below.
#[derive(Clone, Copy)]
struct Pane;

/// Select the task `path` holds on the Tasks page, so its fields show.
fn open_in_pane(app: &mut App, path: &str) -> Pane {
    let id = match path {
        "tasks/Paint the fence.md" => format!("task_{A}"),
        other => panic!("no task at {other}"),
    };
    select_in_pane(app, &id);
    Pane
}

/// An input of the right pane: its value and its choices, once it shows.
fn input(app: &App, _pane: Pane, node: &str) -> Option<(InputValue, Vec<Choice>)> {
    pane_input(app, node)
}

fn panel_text(app: &App, _pane: Pane, node: &str) -> Option<String> {
    pane_text(app, node)
}

/// Wait until the pane's input `node` holds `value`.
fn wait_for(app: &mut App, pane: Pane, node: &str, value: InputValue) {
    support::plugins::wait_until(app, &format!("the pane's {node} shows {value:?}"), |app| {
        input(app, pane, node).is_some_and(|(shown, _)| shown == value)
    });
}

fn change(app: &mut App, _pane: Pane, node: &str, value: InputValue) {
    pane_change(app, node, value);
    settle(app);
}

fn choice(id: &str) -> InputValue {
    InputValue::Choice(Some(id.to_owned()))
}

fn ids(choices: &[Choice]) -> Vec<(String, String)> {
    choices
        .iter()
        .map(|choice| (choice.id.clone(), choice.label.clone()))
        .collect()
}

#[test]
fn a_selected_task_shows_status_priority_due_date_and_a_project_picker() {
    let fixture = workspace();
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };

    let tab = open_in_pane(&mut app, "tasks/Paint the fence.md");
    wait_for(&mut app, tab, "status", choice("todo"));

    let pairs = |pairs: &[(&str, &str)]| -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(id, label)| ((*id).to_owned(), (*label).to_owned()))
            .collect()
    };
    let (_, statuses) = input(&app, tab, "status").unwrap();
    assert_eq!(
        ids(&statuses),
        pairs(&[
            ("todo", "To do"),
            ("in-progress", "In progress"),
            ("done", "Done"),
            ("cancelled", "Cancelled"),
        ])
    );
    let (priority, priorities) = input(&app, tab, "priority").unwrap();
    assert_eq!(priority, choice("medium"));
    assert_eq!(
        ids(&priorities),
        pairs(&[
            ("low", "Low"),
            ("medium", "Medium"),
            ("high", "High"),
            ("urgent", "Urgent")
        ])
    );
    assert_eq!(
        input(&app, tab, "due").unwrap().0,
        InputValue::Date(Some("2026-10-24".into()))
    );
    let (project, projects) = input(&app, tab, "project").unwrap();
    assert_eq!(project, choice("none"));
    let garden = format!("project_{H}");
    let kitchen = format!("project_{C}");
    assert_eq!(
        ids(&projects),
        pairs(&[
            ("none", "No project"),
            (&garden, "Garden"),
            (&kitchen, "Kitchen")
        ])
    );
}

#[test]
fn two_project_files_with_one_id_show_as_one_project_in_the_pane_the_filters_and_the_sidebar() {
    let garden = format!("project_{H}");
    let fixture = Fixture::new(&[
        (
            "tasks/Paint the fence.md",
            &task_file(
                A,
                "status: todo\npriority: medium\nproject: '[[Garden]]'\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        ("projects/Garden/Garden.md", &project_file(H)),
        // A copy of the project's folder, made outside Noura.
        ("projects/Garden copy/Garden.md", &project_file(H)),
    ]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };

    let lists = tasks::sidebar(&app);
    assert_eq!(
        lists[1].items,
        [(format!("project:{garden}"), "Garden".to_owned(), None)]
    );
    tasks::show_filters(&mut app);
    let page = tasks::page(&app);
    let Some(ViewKind::Input { choices, .. }) =
        find(page.root(), "filter-project").map(|node| &node.kind)
    else {
        panic!("the page has a project filter");
    };
    let filters: Vec<String> = choices.iter().map(|choice| choice.id.clone()).collect();
    assert_eq!(filters, ["any", "none", &format!("is:{garden}")]);

    let tab = open_in_pane(&mut app, "tasks/Paint the fence.md");
    wait_for(&mut app, tab, "project", choice(&garden));
    let (_, projects) = input(&app, tab, "project").unwrap();
    assert_eq!(
        ids(&projects),
        [
            ("none".to_owned(), "No project".to_owned()),
            (garden.clone(), "Garden".to_owned())
        ]
    );
}

#[test]
fn each_change_in_the_pane_writes_the_task_and_keeps_its_body() {
    let fixture = workspace();
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let tab = open_in_pane(&mut app, "tasks/Paint the fence.md");
    wait_for(&mut app, tab, "status", choice("todo"));
    let read = || fixture.read("tasks/Paint the fence.md");

    change(&mut app, tab, "status", choice("in-progress"));
    assert_eq!(property(&read(), "status"), Some("in-progress".into()));
    wait_for(&mut app, tab, "status", choice("in-progress"));

    change(&mut app, tab, "priority", choice("urgent"));
    assert_eq!(property(&read(), "priority"), Some("urgent".into()));
    wait_for(&mut app, tab, "priority", choice("urgent"));

    change(
        &mut app,
        tab,
        "due",
        InputValue::Date(Some("2026-11-01".into())),
    );
    assert_eq!(property(&read(), "due"), Some("2026-11-01".into()));
    wait_for(
        &mut app,
        tab,
        "due",
        InputValue::Date(Some("2026-11-01".into())),
    );
    change(&mut app, tab, "due", InputValue::Date(None));
    assert_eq!(property(&read(), "due"), None);
    wait_for(&mut app, tab, "due", InputValue::Date(None));

    let garden = format!("project_{H}");
    change(&mut app, tab, "project", choice(&garden));
    assert_eq!(property(&read(), "project"), Some("[[Garden]]".into()));
    wait_for(&mut app, tab, "project", choice(&garden));
    change(&mut app, tab, "project", choice("none"));
    assert_eq!(property(&read(), "project"), None);
    wait_for(&mut app, tab, "project", choice("none"));

    let text = String::from_utf8(read()).unwrap();
    assert!(text.ends_with("---\nNotes.\n"), "{text}");
}

/// A task holds its project's stable ID, so it stays with the project
/// through a rename and a move of the project's file.
#[test]
fn a_task_keeps_its_project_when_the_project_is_renamed_and_moved() {
    let garden = format!("project_{H}");
    let task = task_file(
        A,
        "status: todo\npriority: medium\nproject: '[[Garden]]'\n",
        "2026-10-01T10:00:00Z",
    );
    let fixture = Fixture::new(&[
        ("tasks/Paint the fence.md", &task),
        ("projects/Garden/Garden.md", &project_file(H)),
    ]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let tab = open_in_pane(&mut app, "tasks/Paint the fence.md");
    wait_for(&mut app, tab, "project", choice(&garden));

    app.start_rename("projects/Garden/Garden.md");
    assert!(app.commit_rename("Allotment"));
    support::plugins::wait_until(&mut app, "the pane shows the new name", |app| {
        input(app, tab, "project").is_some_and(|(value, choices)| {
            value == choice(&garden)
                && ids(&choices).contains(&(garden.clone(), "Allotment".to_owned()))
        })
    });

    std::fs::rename(
        fixture.root.join("projects/Garden/Allotment.md"),
        fixture.root.join("Allotment.md"),
    )
    .unwrap();
    app.rescan();
    settle(&mut app);

    // Noura rewrote the task's link at the rename, and `[[Allotment]]`
    // still leads to the project after the move.
    wait_for(&mut app, tab, "project", choice(&garden));
    assert_eq!(
        fixture.read("tasks/Paint the fence.md"),
        String::from_utf8(task)
            .unwrap()
            .replace("[[Garden]]", "[[Allotment]]")
            .into_bytes()
    );
}

#[test]
fn after_an_outside_edit_a_pane_change_fails_and_the_pane_shows_the_latest_version() {
    let fixture = workspace();
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let tab = open_in_pane(&mut app, "tasks/Paint the fence.md");
    wait_for(&mut app, tab, "status", choice("todo"));
    let outside = task_file(
        A,
        "status: todo\npriority: low\ndue: 2026-10-24\n",
        "2026-10-21T10:00:00Z",
    );
    std::fs::write(fixture.root.join("tasks/Paint the fence.md"), &outside).unwrap();

    change(&mut app, tab, "status", choice("done"));

    assert_eq!(fixture.read("tasks/Paint the fence.md"), outside);
    support::plugins::wait_until(&mut app, "the pane shows the latest version", |app| {
        input(app, tab, "priority").is_some_and(|(value, _)| value == choice("low"))
            && panel_text(app, tab, "notice").as_deref() == Some(CHANGED_ELSEWHERE)
    });
    assert_eq!(input(&app, tab, "status").unwrap().0, choice("todo"));

    // A change from the latest version goes in.
    change(&mut app, tab, "status", choice("done"));
    assert_eq!(
        property(&fixture.read("tasks/Paint the fence.md"), "status"),
        Some("done".into())
    );
    assert_eq!(
        property(&fixture.read("tasks/Paint the fence.md"), "priority"),
        Some("low".into())
    );
}

fn checked(id: &str, checked: bool) -> PluginEvent {
    PluginEvent::Checked(PluginCheck {
        node: "tasks".to_owned(),
        item: id.to_owned(),
        checked,
    })
}

#[test]
fn checking_a_tasks_box_marks_it_done_and_unchecking_it_sets_it_back_to_todo() {
    let fixture = workspace();
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    tasks::choose(&mut app, "views", "view:all");
    let id = format!("task_{A}");
    let read = || fixture.read("tasks/Paint the fence.md");

    send(&mut app, checked(&id, true));
    assert_eq!(property(&read(), "status"), Some("done".into()));
    assert_eq!(rows(&app)[0].checked, Some(true));

    send(&mut app, checked(&id, false));
    assert_eq!(property(&read(), "status"), Some("todo".into()));
    assert_eq!(rows(&app)[0].checked, Some(false));
    assert_eq!(text(&app, "notice"), None);
}

#[test]
fn a_check_against_a_stale_revision_leaves_the_file_and_shows_the_latest_version() {
    let fixture = Fixture::new(&[
        (
            "tasks/Paint the fence.md",
            &task_file(
                A,
                "status: todo\npriority: medium\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        (
            "tasks/Fix the gate.md",
            &task_file(
                B,
                "status: todo\npriority: medium\n",
                "2026-10-02T10:00:00Z",
            ),
        ),
    ]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    tasks::choose(&mut app, "views", "view:all");
    let outside = task_file(
        A,
        "status: todo\npriority: urgent\n",
        "2026-10-21T10:00:00Z",
    );
    std::fs::write(fixture.root.join("tasks/Paint the fence.md"), &outside).unwrap();

    send(&mut app, checked(&format!("task_{A}"), true));

    assert_eq!(fixture.read("tasks/Paint the fence.md"), outside);
    assert_eq!(text(&app, "notice").as_deref(), Some(CHANGED_ELSEWHERE));
    let shown = rows(&app);
    assert_eq!(shown[0].label, "Paint the fence");
    assert_eq!(shown[0].detail.as_deref(), Some("Urgent"));
    assert_eq!(shown[0].checked, Some(false));
}
