//! Seam 1: the Projects plugin against a real workspace folder. A new
//! project gets a folder of its own, starts planned and opens; the
//! Projects page lists every project with its status and task count; and
//! a project opens with its status and overview beside its tasks, notes,
//! files and this month's dated work.

mod support;

use app_core::{InputValue, PluginEvent, Target, ViewKind};
use support::{
    FakeSystem, Fixture, note, open_from_tree,
    projects::{self, items, listed, name_new_project, project_file, with_projects},
    tasks::{A, B, C, D, E, H, find, property, task_file, with_tasks},
};

/// Garden, with its tasks, notes and files, beside another project and
/// its task.
fn garden() -> Fixture {
    let garden = format!("project_{H}");
    let kitchen = format!("project_{C}");
    let task = |suffix: &str, status: &str, more: &str, project: &str| {
        task_file(
            suffix,
            &format!("status: {status}\npriority: medium\n{more}project: {project}\n"),
            "2026-10-01T10:00:00Z",
        )
    };
    Fixture::new(&[
        (
            "projects/Garden/Garden.md",
            &project_file(
                H,
                "status: active\nstart: 2026-10-05\nend: 2026-10-08\n",
                "Grow vegetables.\n",
            ),
        ),
        ("projects/Garden/Soil.md", &note(A, "Loam.\n")),
        ("projects/Garden/research/Compost.md", &note(B, "Turn it.\n")),
        ("projects/Garden/plan.pdf", b"%PDF-1.4\n"),
        ("projects/Garden/Sketch.md", b"A plain sketch.\n"),
        ("projects/Garden/Prune.md", &task(C, "done", "", &garden)),
        (
            "tasks/Weed.md",
            &task(A, "in-progress", "due: 2026-10-24\n", &garden),
        ),
        (
            "tasks/Water.md",
            &task(B, "todo", "due: 2026-11-02\n", &garden),
        ),
        (
            "projects/Kitchen/Kitchen.md",
            &project_file(C, "status: planned\n", ""),
        ),
        (
            "tasks/Paint.md",
            &task(D, "todo", "due: 2026-10-22\n", &kitchen),
        ),
    ])
}

fn item(id: &str, label: &str, detail: Option<&str>) -> (String, String, Option<String>) {
    (id.to_owned(), label.to_owned(), detail.map(str::to_owned))
}

#[test]
fn a_project_opens_with_its_status_overview_tasks_notes_files_and_dated_work() {
    let fixture = garden();
    let system = FakeSystem::new();
    system.set_clock("2026-10-21T15:00:00Z", "America/Toronto");
    let mut app = fixture.app_with(system);
    let (Some(_tasks), Some(_projects)) = (with_tasks(&mut app), with_projects(&mut app)) else {
        return;
    };

    let tab = open_from_tree(&mut app, "projects/Garden/Garden.md");
    let view = projects::panel_with(&mut app, tab, "status");

    // The overview is the project's body, in the note editor.
    assert_eq!(app.note(tab).unwrap().text(), "Grow vegetables.\n");
    let Some(ViewKind::Input { value, choices, .. }) =
        find(view.root(), "status").map(|node| &node.kind)
    else {
        panic!("the panel has a status");
    };
    assert_eq!(value, &InputValue::Choice(Some("active".into())));
    let statuses: Vec<&str> = choices.iter().map(|choice| choice.label.as_str()).collect();
    assert_eq!(
        statuses,
        ["Planned", "Active", "On hold", "Completed", "Cancelled"]
    );
    assert_eq!(
        items(view.root(), "tasks"),
        [
            item(&format!("task:task_{C}"), "Prune", Some("Done")),
            item(&format!("task:task_{B}"), "Water", Some("To do")),
            item(&format!("task:task_{A}"), "Weed", Some("In progress")),
        ]
    );
    assert_eq!(
        items(view.root(), "notes"),
        [
            item(&format!("note:note_{B}"), "Compost", Some("research")),
            item(&format!("note:note_{A}"), "Soil", None),
        ]
    );
    assert_eq!(
        items(view.root(), "files"),
        [
            item("file:projects/Garden/plan.pdf", "plan.pdf", None),
            item("file:projects/Garden/Sketch.md", "Sketch.md", None),
        ]
    );
    // This month's dated work: the project's own and its tasks', and
    // nothing of other projects or other months.
    assert_eq!(
        items(view.root(), "dated"),
        [
            item(
                &format!("dated:project_{H}:start"),
                "Garden",
                Some("Starts Oct 5")
            ),
            item(&format!("dated:task_{A}:due"), "Weed", Some("Due Oct 24")),
        ]
    );
}

#[test]
fn opening_a_task_or_a_note_from_the_project_opens_it_in_the_editor() {
    let fixture = garden();
    let mut app = fixture.app();
    let (Some(_tasks), Some(_projects)) = (with_tasks(&mut app), with_projects(&mut app)) else {
        return;
    };
    let tab = open_from_tree(&mut app, "projects/Garden/Garden.md");
    projects::panel_with(&mut app, tab, "tasks");

    projects::send_panel(
        &mut app,
        tab,
        PluginEvent::Activated(Target {
            node: "tasks".into(),
            item: format!("task:task_{A}"),
        }),
    );
    assert_eq!(app.active_tab().unwrap().title(), "Weed");

    let tab = open_from_tree(&mut app, "projects/Garden/Garden.md");
    projects::panel_with(&mut app, tab, "notes");
    projects::send_panel(
        &mut app,
        tab,
        PluginEvent::Activated(Target {
            node: "notes".into(),
            item: format!("note:note_{B}"),
        }),
    );
    assert_eq!(app.active_tab().unwrap().title(), "Compost");
}

#[test]
fn a_new_project_gets_a_folder_of_its_own_starts_planned_and_opens() {
    let fixture = Fixture::new(&[(
        "projects/Garden/Garden.md",
        &project_file(H, "status: active\n", "Overview.\n"),
    )]);
    let mut app = fixture.app();
    let (Some(_tasks), Some(_projects)) = (with_tasks(&mut app), with_projects(&mut app)) else {
        return;
    };

    app.press_plugin_button(projects::PAGE, "new-project");
    support::settle(&mut app);
    // Nothing is written until you name it.
    assert!(!fixture.exists("projects/Kitchen"));
    name_new_project(&mut app, "Kitchen");

    let bytes = fixture.read("projects/Kitchen/Kitchen.md");
    match local_core::parse_markdown("Kitchen.md", &bytes) {
        local_core::ParsedMarkdown::Managed(object) => assert_eq!(object.object_type, "project"),
        other => panic!("not a project: {other:?}"),
    }
    assert_eq!(property(&bytes, "status"), Some("planned".into()));
    let tab = app.active_tab().expect("the new project opens");
    assert_eq!(tab.title(), "Kitchen");
    assert_eq!(
        tab.file().map(|file| file.relative_path.as_str()),
        Some("projects/Kitchen/Kitchen.md")
    );

    // A second project with a taken name gets the next free folder.
    name_new_project(&mut app, "Garden");
    assert_eq!(
        property(&fixture.read("projects/Garden 1/Garden 1.md"), "status"),
        Some("planned".into())
    );
    assert_eq!(
        fixture.read("projects/Garden/Garden.md"),
        project_file(H, "status: active\n", "Overview.\n")
    );
}

#[test]
fn the_projects_page_lists_every_project_with_its_status_and_task_count() {
    let garden = format!("project_{H}");
    let kitchen = format!("project_{C}");
    let in_garden = format!("status: todo\npriority: medium\nproject: {garden}\n");
    let fixture = Fixture::new(&[
        (
            "projects/Kitchen/Kitchen.md",
            // Another app wrote it without a status.
            &project_file(C, "", "Tiles.\n"),
        ),
        (
            "projects/Garden/Garden.md",
            &project_file(H, "status: on-hold\n", "Beds.\n"),
        ),
        (
            "tasks/Weed.md",
            &task_file(A, &in_garden, "2026-10-01T10:00:00Z"),
        ),
        (
            "tasks/Water.md",
            &task_file(B, &in_garden, "2026-10-01T10:00:00Z"),
        ),
        (
            "tasks/Paint.md",
            &task_file(
                D,
                &format!("status: done\npriority: low\nproject: {kitchen}\n"),
                "2026-10-01T10:00:00Z",
            ),
        ),
        (
            "tasks/Read.md",
            &task_file(E, "status: todo\npriority: low\n", "2026-10-01T10:00:00Z"),
        ),
    ]);
    let mut app = fixture.app();
    let (Some(_tasks), Some(_projects)) = (with_tasks(&mut app), with_projects(&mut app)) else {
        return;
    };

    assert_eq!(
        listed(&app),
        [
            ("Garden".to_owned(), Some("On hold · 2 tasks".to_owned())),
            ("Kitchen".to_owned(), Some("Planned · 1 task".to_owned())),
        ]
    );

    // A project made outside Noura joins the list.
    support::write(
        &fixture.root,
        "projects/Attic/Attic.md",
        &project_file(D, "status: active\n", ""),
    );
    support::plugins::wait_until(&mut app, "the page lists the new project", |app| {
        listed(app).len() == 3
    });
    assert_eq!(
        listed(&app)[0],
        ("Attic".to_owned(), Some("Active · 0 tasks".to_owned()))
    );
}

#[test]
fn the_project_page_shows_tasks_and_files_that_change_outside_noura() {
    let fixture = garden();
    let mut app = fixture.app();
    let (Some(_tasks), Some(_projects)) = (with_tasks(&mut app), with_projects(&mut app)) else {
        return;
    };
    let tab = open_from_tree(&mut app, "projects/Garden/Garden.md");
    projects::panel_with(&mut app, tab, "tasks");

    support::write(
        &fixture.root,
        "tasks/Mulch.md",
        &task_file(
            E,
            &format!("status: todo\npriority: medium\nproject: project_{H}\n"),
            "2026-10-01T10:00:00Z",
        ),
    );
    support::write(&fixture.root, "projects/Garden/seeds.csv", b"beans\n");

    support::plugins::wait_until(&mut app, "the page shows the new task and file", |app| {
        let view = projects::panel(app, tab);
        items(view.root(), "tasks")
            .iter()
            .any(|(_, label, _)| label == "Mulch")
            && items(view.root(), "files")
                .iter()
                .any(|(_, label, _)| label == "seeds.csv")
    });
}
