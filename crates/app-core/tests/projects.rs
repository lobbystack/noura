//! Seam 1: the Projects plugin against a real workspace folder. A new
//! project gets a folder of its own, starts planned and opens; the
//! Projects page lists every project with its status and task count.

mod support;

use support::{
    Fixture,
    projects::{self, listed, name_new_project, project_file, with_projects},
    tasks::{A, B, C, D, E, H, property, task_file, with_tasks},
};

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
