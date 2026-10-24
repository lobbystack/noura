//! Seam 1: a project's board against a real workspace folder. It shows a
//! column per task status with cards in board order. A drag or the
//! keyboard's move menu writes one task through the Tasks update command,
//! with the expected revision and an order key between its new
//! neighbors, and no other file changes. The order lives in the task
//! files, so it survives reopening the workspace without its index, a
//! stale revision refuses the move, and Add task names a task first.

mod support;

use app_core::{App, PluginEvent, PluginMove, PluginNaming, TabId, ViewKind};
use support::{
    Fixture, index_files, open_from_tree,
    projects::{self, board, project_file, send_panel, text_of, with_projects},
    tasks::{A, B, C, D, E, F, G, H, find, property, task_file, with_tasks},
};

const MOVE_CHANGED: &str = "Could not move the task. The task changed while you were viewing it.";

fn task(suffix: &str, status: &str, more: &str) -> Vec<u8> {
    task_file(
        suffix,
        &format!("status: {status}\npriority: medium\n{more}project: project_{H}\n"),
        "2026-10-01T10:00:00Z",
    )
}

/// Garden's tasks: four to do, two of them with one key, one without a
/// key, one in progress and one done, and another project's task.
fn garden() -> Fixture {
    Fixture::new(&[
        (
            "projects/Garden/Garden.md",
            &project_file(H, "status: active\n", "Grow vegetables.\n"),
        ),
        ("tasks/Weed.md", &task(A, "todo", "kanban_order: a1\n")),
        ("tasks/Water.md", &task(B, "todo", "kanban_order: a0\n")),
        ("tasks/Mulch.md", &task(C, "todo", "")),
        ("tasks/Prune.md", &task(D, "todo", "kanban_order: a0\n")),
        (
            "tasks/Rake.md",
            &task(E, "in-progress", "kanban_order: V\n"),
        ),
        ("tasks/Plant.md", &task(F, "done", "")),
        (
            "tasks/Paint.md",
            &task_file(
                G,
                "status: todo\npriority: medium\nproject: project_01jbcdefghjkmnpqrstvwxyz0c\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
    ])
}

/// The board's columns as (label, card labels).
fn columns(app: &App, tab: TabId) -> Vec<(String, Vec<String>)> {
    board(app, tab)
        .into_iter()
        .map(|(_, label, cards)| (label, cards))
        .collect()
}

fn column(label: &str, cards: &[&str]) -> (String, Vec<String>) {
    (
        label.to_owned(),
        cards.iter().map(|card| (*card).to_owned()).collect(),
    )
}

/// The app with Tasks and Projects on and Garden open, its board drawn.
struct Opened {
    app: App,
    tab: TabId,
    _plugins: (
        support::plugins::PluginFolder,
        support::plugins::PluginFolder,
    ),
}

fn opened(fixture: &Fixture) -> Option<Opened> {
    let mut app = fixture.app();
    let (Some(tasks), Some(projects)) = (with_tasks(&mut app), with_projects(&mut app)) else {
        return None;
    };
    let tab = open_from_tree(&mut app, "projects/Garden/Garden.md");
    projects::panel_with(&mut app, tab, "board");
    Some(Opened {
        app,
        tab,
        _plugins: (tasks, projects),
    })
}

fn moved(task: &str, column: &str, position: u32) -> PluginEvent {
    PluginEvent::Moved(PluginMove {
        node: "board".to_owned(),
        item: format!("card:task_{task}"),
        to: format!("column:{column}"),
        position,
    })
}

/// The files whose bytes differ between two snapshots.
fn changed(
    before: &[(String, Option<Vec<u8>>)],
    after: &[(String, Option<Vec<u8>>)],
) -> Vec<String> {
    let mut paths: Vec<String> = after
        .iter()
        .filter(|entry| !before.contains(entry))
        .map(|(path, _)| path.clone())
        .collect();
    paths.extend(
        before
            .iter()
            .filter(|(path, _)| !after.iter().any(|(other, _)| other == path))
            .map(|(path, _)| path.clone()),
    );
    paths.sort();
    paths
}

fn key(fixture: &Fixture, path: &str) -> String {
    property(&fixture.read(path), "kanban_order")
        .and_then(|key| key.as_str().map(str::to_owned))
        .unwrap_or_else(|| panic!("{path} has no order key"))
}

#[test]
fn the_board_shows_a_column_per_status_with_cards_by_order_key_then_stable_id() {
    let fixture = garden();
    let Some(Opened { app, tab, _plugins }) = opened(&fixture) else {
        return;
    };

    assert_eq!(
        board(&app, tab)
            .into_iter()
            .map(|(id, label, _)| (id, label))
            .collect::<Vec<_>>(),
        [
            ("column:todo".to_owned(), "To do".to_owned()),
            ("column:in-progress".to_owned(), "In progress".to_owned()),
            ("column:done".to_owned(), "Done".to_owned()),
            ("column:cancelled".to_owned(), "Cancelled".to_owned()),
        ]
    );
    // Water and Prune share a key, so their stable IDs order them, and
    // Mulch, which has none, sorts by its stable ID.
    assert_eq!(
        columns(&app, tab),
        [
            column("To do", &["Water", "Prune", "Weed", "Mulch"]),
            column("In progress", &["Rake"]),
            column("Done", &["Plant"]),
            column("Cancelled", &[]),
        ]
    );
}

#[test]
fn a_drag_within_a_column_writes_only_the_cards_order_and_to_another_column_its_status_too() {
    let fixture = garden();
    let Some(Opened {
        mut app,
        tab,
        _plugins,
    }) = opened(&fixture)
    else {
        return;
    };

    // Mulch to the top of To do.
    let before = fixture.snapshot();
    send_panel(&mut app, tab, moved(C, "todo", 0));
    let after = fixture.snapshot();
    assert_eq!(changed(&before, &after), ["tasks/Mulch.md"]);
    let mulch = fixture.read("tasks/Mulch.md");
    assert_eq!(property(&mulch, "status"), Some("todo".into()));
    assert!(key(&fixture, "tasks/Mulch.md").as_str() < "a0");
    let text = String::from_utf8(mulch).unwrap();
    assert!(text.ends_with("---\nNotes.\n"), "{text}");
    assert_eq!(
        columns(&app, tab)[0],
        column("To do", &["Mulch", "Water", "Prune", "Weed"])
    );

    // Nothing fits between Water and Prune, which share a key, so Mulch
    // dropped between them lands after both, and before Weed.
    send_panel(&mut app, tab, moved(C, "todo", 1));
    let mulch = key(&fixture, "tasks/Mulch.md");
    assert!("a0" < mulch.as_str() && mulch.as_str() < "a1", "{mulch}");
    assert_eq!(
        columns(&app, tab)[0],
        column("To do", &["Water", "Prune", "Mulch", "Weed"])
    );

    // Water into In progress, below Rake.
    let before = fixture.snapshot();
    send_panel(&mut app, tab, moved(B, "in-progress", 1));
    let after = fixture.snapshot();
    assert_eq!(changed(&before, &after), ["tasks/Water.md"]);
    assert_eq!(
        property(&fixture.read("tasks/Water.md"), "status"),
        Some("in-progress".into())
    );
    assert!(key(&fixture, "tasks/Water.md").as_str() > "V");
    assert_eq!(
        columns(&app, tab)[1],
        column("In progress", &["Rake", "Water"])
    );
    // The page doesn't say anything went wrong.
    assert_eq!(text_of(projects::panel(&app, tab).root(), "notice"), None);
}

#[test]
fn the_keyboard_move_menu_moves_a_card_up_down_and_to_another_column() {
    let fixture = garden();
    let Some(Opened {
        mut app,
        tab,
        _plugins,
    }) = opened(&fixture)
    else {
        return;
    };
    let view = projects::panel(&app, tab);
    let menu = find(view.root(), &format!("card-menu:task_{A}")).expect("Weed has a menu");
    let entries: Vec<(String, bool)> = menu
        .children
        .iter()
        .filter_map(|item| match &item.kind {
            ViewKind::MenuItem { label, enabled } => Some((label.clone(), *enabled)),
            _ => None,
        })
        .collect();
    assert_eq!(
        entries,
        [
            ("Move up".to_owned(), true),
            ("Move down".to_owned(), true),
            ("To do".to_owned(), false),
            ("In progress".to_owned(), true),
            ("Done".to_owned(), true),
            ("Cancelled".to_owned(), true),
        ]
    );

    let before = fixture.snapshot();
    send_panel(
        &mut app,
        tab,
        PluginEvent::Chosen(format!("move-up:task_{C}")),
    );
    assert_eq!(changed(&before, &fixture.snapshot()), ["tasks/Mulch.md"]);
    assert_eq!(
        columns(&app, tab)[0],
        column("To do", &["Water", "Prune", "Mulch", "Weed"])
    );

    send_panel(
        &mut app,
        tab,
        PluginEvent::Chosen(format!("move-down:task_{B}")),
    );
    assert_eq!(
        columns(&app, tab)[0],
        column("To do", &["Prune", "Water", "Mulch", "Weed"])
    );

    let before = fixture.snapshot();
    send_panel(
        &mut app,
        tab,
        PluginEvent::Chosen(format!("move-to:done:task_{C}")),
    );
    assert_eq!(changed(&before, &fixture.snapshot()), ["tasks/Mulch.md"]);
    assert_eq!(
        property(&fixture.read("tasks/Mulch.md"), "status"),
        Some("done".into())
    );
    assert_eq!(columns(&app, tab)[2], column("Done", &["Plant", "Mulch"]));
}

#[test]
fn board_order_survives_reopening_the_workspace_without_its_index() {
    let fixture = garden();
    let Some(Opened {
        mut app,
        tab,
        _plugins,
    }) = opened(&fixture)
    else {
        return;
    };
    send_panel(&mut app, tab, moved(C, "todo", 0));
    send_panel(&mut app, tab, moved(A, "todo", 1));
    send_panel(&mut app, tab, moved(E, "done", 0));
    let shown = columns(&app, tab);
    assert_eq!(
        shown,
        [
            column("To do", &["Mulch", "Weed", "Water", "Prune"]),
            column("In progress", &[]),
            column("Done", &["Rake", "Plant"]),
            column("Cancelled", &[]),
        ]
    );
    let index = index_files(&fixture, &app);
    drop(app);
    for file in &index {
        if file.exists() {
            std::fs::remove_file(file).unwrap();
        }
    }

    let mut app = fixture.app();
    support::plugins::wait_until(&mut app, "Tasks and Projects run", |app| {
        ["tasks", "projects"]
            .iter()
            .all(|id| app.plugins().iter().any(|row| row.id == *id && row.running))
    });
    let tab = open_from_tree(&mut app, "projects/Garden/Garden.md");
    projects::panel_with(&mut app, tab, "board");
    support::plugins::wait_until(&mut app, "the board shows every task", |app| {
        columns(app, tab) == shown
    });
    assert!(index[0].exists());
}

#[test]
fn a_move_against_a_stale_revision_changes_nothing_and_reloads_the_board() {
    let fixture = garden();
    let Some(Opened {
        mut app,
        tab,
        _plugins,
    }) = opened(&fixture)
    else {
        return;
    };
    // Another app changes Water after the board drew it.
    let outside = task_file(
        B,
        &format!("status: todo\npriority: high\nkanban_order: a0\nproject: project_{H}\n"),
        "2026-10-21T10:00:00Z",
    );
    std::fs::write(fixture.root.join("tasks/Water.md"), &outside).unwrap();
    let before = fixture.snapshot();

    send_panel(&mut app, tab, moved(B, "done", 0));

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(fixture.read("tasks/Water.md"), outside);
    support::plugins::wait_until(&mut app, "the board shows the latest Water", |app| {
        let view = projects::panel(app, tab);
        text_of(view.root(), "notice").as_deref() == Some(MOVE_CHANGED)
            && matches!(
                find(view.root(), &format!("card:task_{B}")).map(|card| &card.kind),
                Some(ViewKind::Card { detail: Some(detail), .. }) if detail == "High"
            )
    });
    assert_eq!(columns(&app, tab)[0].1[0], "Water");

    // A move from the latest version goes in.
    send_panel(&mut app, tab, moved(B, "done", 0));
    assert_eq!(
        property(&fixture.read("tasks/Water.md"), "status"),
        Some("done".into())
    );
    assert_eq!(text_of(projects::panel(&app, tab).root(), "notice"), None);
}

fn name_field_shows(app: &App, tab: TabId) -> bool {
    let view = projects::panel(app, tab);
    find(view.root(), "new-task-name").is_some_and(
        |node| matches!(&node.kind, ViewKind::NameField { label, .. } if label == "New task name"),
    )
}

#[test]
fn add_task_names_a_task_first_and_creates_it_in_the_project_through_tasks() {
    let fixture = garden();
    let Some(Opened {
        mut app,
        tab,
        _plugins,
    }) = opened(&fixture)
    else {
        return;
    };
    let before = fixture.snapshot();

    app.press_panel_button(tab, "add-task");
    support::settle(&mut app);
    assert!(name_field_shows(&app, tab));
    // Escape creates nothing.
    send_panel(
        &mut app,
        tab,
        PluginEvent::Cancelled("new-task-name".into()),
    );
    assert!(!name_field_shows(&app, tab));
    assert_eq!(fixture.snapshot(), before);

    app.press_panel_button(tab, "add-task");
    support::settle(&mut app);
    send_panel(
        &mut app,
        tab,
        PluginEvent::Named(PluginNaming {
            node: "new-task-name".into(),
            name: "Sow beans".into(),
        }),
    );

    assert_eq!(
        changed(&before, &fixture.snapshot()),
        ["tasks/Sow beans.md"]
    );
    let sown = fixture.read("tasks/Sow beans.md");
    assert_eq!(
        property(&sown, "project"),
        Some(format!("project_{H}").into())
    );
    assert_eq!(property(&sown, "status"), Some("todo".into()));
    assert_eq!(property(&sown, "priority"), Some("medium".into()));
    assert!(!name_field_shows(&app, tab));
    // It joins the end of To do.
    assert_eq!(
        columns(&app, tab)[0],
        column("To do", &["Water", "Prune", "Weed", "Mulch", "Sow beans"])
    );
}
