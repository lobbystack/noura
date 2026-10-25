//! Seam 1 against a real workspace folder: the kernel holds no task or
//! project rules. With Tasks and Projects on, a task or project file whose
//! properties don't fit the plugin's declared schema shows in the Inbox's
//! Needs attention, named against the plugin, until you fix it or turn
//! the plugin off. Noura leaves the file's bytes alone.

mod support;

use app_core::App;
use support::{
    Fixture, plugins, projects,
    tasks::{A, B, C, task_file, with_tasks},
    write,
};

/// Needs attention as (message, the paths its links open).
fn attention(app: &App) -> Vec<(String, Vec<String>)> {
    app.inbox()
        .needs_attention
        .into_iter()
        .map(|item| {
            let paths = item
                .links
                .into_iter()
                .map(|link| link.relative_path)
                .collect();
            (item.message, paths)
        })
        .collect()
}

fn problem(message: &str, path: &str) -> (String, Vec<String>) {
    (message.to_owned(), vec![path.to_owned()])
}

const LATER: &str = "Tasks/Later.md";
const FINE: &str = "Tasks/Fine.md";
const ODD: &str = "Projects/Odd/Odd.md";

#[test]
fn a_property_the_owning_plugin_refuses_shows_in_needs_attention_until_it_is_fixed() {
    let later = task_file(A, "status: later\n", "2026-10-01T10:00:00Z");
    let fine = task_file(B, "status: todo\npriority: high\n", "2026-10-01T10:00:00Z");
    let odd = projects::project_file(C, "status: 5\n", "Overview.\n");
    let fixture = Fixture::new(&[(LATER, &later), (FINE, &fine), (ODD, &odd)]);
    let mut app = fixture.app();
    let before = fixture.snapshot();

    // With every plugin off, the files are generic managed objects.
    assert_eq!(attention(&app), []);

    let Some(_tasks) = with_tasks(&mut app) else {
        return;
    };
    plugins::wait_until(&mut app, "the task problem shows", |app| {
        !attention(app).is_empty()
    });
    assert_eq!(
        attention(&app),
        [problem(
            "Tasks/Later.md has a status property that the Tasks plugin doesn't accept.",
            LATER
        )]
    );
    let Some(_projects) = projects::with_projects(&mut app) else {
        return;
    };
    plugins::wait_until(&mut app, "the project problem shows", |app| {
        attention(app).len() == 2
    });
    assert_eq!(
        attention(&app),
        [
            problem(
                "Projects/Odd/Odd.md has a status property that the Projects plugin doesn't \
                 accept.",
                ODD
            ),
            problem(
                "Tasks/Later.md has a status property that the Tasks plugin doesn't accept.",
                LATER
            ),
        ]
    );
    assert_eq!(fixture.snapshot(), before, "Noura changed a file");

    // Fixing the task in another app takes its problem away.
    write(
        &fixture.root,
        LATER,
        &task_file(A, "status: done\n", "2026-10-02T10:00:00Z"),
    );
    plugins::wait_until(&mut app, "the fixed task leaves", |app| {
        attention(app).len() == 1
    });
    assert_eq!(attention(&app)[0].1, [ODD]);

    // Turning Projects off leaves its file a generic managed object again.
    assert!(app.set_plugin_enabled("projects", false));
    plugins::wait_until(&mut app, "the project problem leaves", |app| {
        attention(app).is_empty()
    });
    assert_eq!(fixture.read(ODD), odd);
}
