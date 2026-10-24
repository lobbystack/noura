//! Seam 1: a task row's menu in the Tasks views. Open shows the task as a
//! click in the file tree does, in the preview tab, and Open in new tab
//! keeps the current tab and adds one. Rename edits the name in the row
//! and saves it against the task's latest revision, and the task's tabs
//! follow. Move to Trash saves what you typed in the task first, sends the
//! file to the system trash or to the workspace's trash folder, closes the
//! task's tabs and says where the file went.

mod support;

use std::time::{Duration, Instant};

use app_core::{App, OpenAs, PluginEvent, PluginNaming, ViewKind};
use support::{
    FakeSystem, Fixture, notice, open_from_tree,
    tasks::{self, A, B, PAGE, find, labels, send, task_file, with_tasks},
};

fn workspace() -> Fixture {
    Fixture::new(&[
        (
            "tasks/Water the beds.md",
            &task_file(
                A,
                "status: todo\npriority: medium\ndue: 2026-10-19\n",
                "2026-10-01T10:00:00Z",
            ),
        ),
        (
            "tasks/Read the manual.md",
            &task_file(
                B,
                "status: todo\npriority: medium\n",
                "2026-10-02T10:00:00Z",
            ),
        ),
    ])
}

/// The app on `system`, with Tasks on and its page showing All.
fn tasks_app(
    fixture: &Fixture,
    system: std::sync::Arc<FakeSystem>,
) -> Option<(App, support::plugins::PluginFolder)> {
    let mut app = fixture.app_with(system);
    let folder = with_tasks(&mut app)?;
    tasks::choose(&mut app, "views", "view:all");
    Some((app, folder))
}

/// Choose the menu item `item` of the task with the ID suffix `suffix`.
fn choose(app: &mut App, item: &str, suffix: &str) {
    let menu = format!("{item}:task_{suffix}");
    let view = tasks::page(app);
    let row = find(view.root(), &format!("task_{suffix}")).expect("the task has a row");
    let menu_node = row
        .children
        .iter()
        .find(|child| matches!(child.kind, ViewKind::Menu { .. }))
        .expect("the row has a menu");
    assert!(
        find(menu_node, &menu).is_some(),
        "the menu has no item {menu}"
    );
    send(app, PluginEvent::Chosen(menu));
}

/// Each tab's title, and whether it's the preview tab.
fn tabs(app: &App) -> Vec<(String, bool)> {
    app.tabs()
        .iter()
        .map(|tab| (tab.title().to_owned(), tab.is_preview()))
        .collect()
}

fn active(app: &App) -> Option<String> {
    app.active_tab().map(|tab| tab.title().to_owned())
}

fn tab_of(app: &App, title: &str) -> app_core::TabId {
    app.tabs()
        .iter()
        .find(|tab| tab.title() == title)
        .unwrap_or_else(|| panic!("no tab is titled {title}"))
        .id()
}

fn wait_for_tabs(app: &mut App, expected: &[(&str, bool)]) {
    let expected: Vec<(String, bool)> = expected
        .iter()
        .map(|(title, preview)| ((*title).to_owned(), *preview))
        .collect();
    support::plugins::wait_until(app, &format!("the tabs are {expected:?}"), |app| {
        tabs(app) == expected
    });
}

#[test]
fn open_shows_the_task_in_the_current_preview_tab() {
    let fixture = workspace();
    let Some((mut app, _folder)) = tasks_app(&fixture, FakeSystem::new()) else {
        return;
    };
    for tab in app.tabs().iter().map(|tab| tab.id()).collect::<Vec<_>>() {
        app.close_tab(tab);
    }
    app.open_page(PAGE, OpenAs::Preview);
    wait_for_tabs(&mut app, &[("Tasks", true)]);

    choose(&mut app, "open", A);

    wait_for_tabs(&mut app, &[("Water the beds", true)]);
    assert_eq!(active(&app).as_deref(), Some("Water the beds"));
}

#[test]
fn from_a_kept_tab_open_uses_the_preview_tab_and_open_in_new_tab_adds_a_kept_one() {
    let fixture = workspace();
    let Some((mut app, _folder)) = tasks_app(&fixture, FakeSystem::new()) else {
        return;
    };
    for tab in app.tabs().iter().map(|tab| tab.id()).collect::<Vec<_>>() {
        app.close_tab(tab);
    }
    app.open_page(PAGE, OpenAs::Kept);
    wait_for_tabs(&mut app, &[("Tasks", false)]);

    choose(&mut app, "open", A);
    wait_for_tabs(&mut app, &[("Tasks", false), ("Water the beds", true)]);
    assert_eq!(active(&app).as_deref(), Some("Water the beds"));

    // The next Open replaces the preview.
    app.activate_tab(tab_of(&app, "Tasks"));
    choose(&mut app, "open", B);
    wait_for_tabs(&mut app, &[("Tasks", false), ("Read the manual", true)]);

    // Open in new tab keeps the current tab and adds one after the others.
    app.activate_tab(tab_of(&app, "Tasks"));
    choose(&mut app, "open-new", A);
    wait_for_tabs(
        &mut app,
        &[
            ("Tasks", false),
            ("Read the manual", true),
            ("Water the beds", false),
        ],
    );
    assert_eq!(active(&app).as_deref(), Some("Water the beds"));
}

/// The rename field in the row of the task with the ID suffix `suffix`:
/// its label and the name it starts from.
fn rename_field(app: &App, suffix: &str) -> Option<(String, String)> {
    let view = tasks::page(app);
    let row = find(view.root(), &format!("task_{suffix}"))?;
    row.children.iter().find_map(|child| match &child.kind {
        ViewKind::NameField { label, value } if child.id == "rename" => {
            Some((label.clone(), value.clone()))
        }
        _ => None,
    })
}

fn name(app: &mut App, name: &str) {
    send(
        app,
        PluginEvent::Named(PluginNaming {
            node: "rename".to_owned(),
            name: name.to_owned(),
        }),
    );
}

#[test]
fn rename_edits_the_name_in_the_row_and_saves_it_against_the_latest_revision() {
    let fixture = workspace();
    let Some((mut app, _folder)) = tasks_app(&fixture, FakeSystem::new()) else {
        return;
    };
    let tab = open_from_tree(&mut app, "tasks/Water the beds.md");
    app.open_page(PAGE, OpenAs::Kept);
    support::settle(&mut app);

    choose(&mut app, "rename", A);
    assert_eq!(
        rename_field(&app, A),
        Some(("Task name".to_owned(), "Water the beds".to_owned()))
    );
    // Another app writes the task after the list drew it.
    let outside = task_file(
        A,
        "status: todo\npriority: high\ndue: 2026-10-19\n",
        "2026-10-21T10:00:00Z",
    );
    std::fs::write(fixture.root.join("tasks/Water the beds.md"), &outside).unwrap();

    name(&mut app, "Water the roses");

    assert!(!fixture.exists("tasks/Water the beds.md"));
    assert_eq!(fixture.read("tasks/Water the roses.md"), outside);
    assert_eq!(rename_field(&app, A), None);
    assert_eq!(labels(&app), ["Water the roses", "Read the manual"]);
    support::plugins::wait_until(&mut app, "the task's tab shows its new name", |app| {
        app.tabs()
            .iter()
            .find(|shown| shown.id() == tab)
            .is_some_and(|shown| shown.title() == "Water the roses")
    });
}

#[test]
fn escape_or_the_same_name_keeps_the_old_name() {
    let fixture = workspace();
    let Some((mut app, _folder)) = tasks_app(&fixture, FakeSystem::new()) else {
        return;
    };
    let before = fixture.snapshot();

    choose(&mut app, "rename", B);
    send(&mut app, PluginEvent::Cancelled("rename".to_owned()));
    assert_eq!(rename_field(&app, B), None);

    choose(&mut app, "rename", B);
    name(&mut app, "Read the manual");
    assert_eq!(rename_field(&app, B), None);

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(labels(&app), ["Water the beds", "Read the manual"]);
}

#[test]
fn a_rename_that_fails_keeps_the_field_open_and_says_why() {
    let fixture = workspace();
    let Some((mut app, _folder)) = tasks_app(&fixture, FakeSystem::new()) else {
        return;
    };
    let before = fixture.snapshot();
    choose(&mut app, "rename", B);

    for (taken, problem) in [
        (
            "Water the beds",
            "Something named \u{201c}Water the beds\u{201d} is already in this folder.",
        ),
        ("Read/manual", "Names can't contain \\ / : * ? \" < > or |."),
        (".manual", "Names can't start with a dot."),
    ] {
        name(&mut app, taken);
        assert_eq!(
            rename_field(&app, B),
            Some(("Task name".to_owned(), "Read the manual".to_owned())),
            "{taken}"
        );
        assert_eq!(
            tasks::text(&app, "rename-problem").as_deref(),
            Some(problem),
            "{taken}"
        );
    }
    assert_eq!(fixture.snapshot(), before);

    // A name that works then goes in.
    name(&mut app, "Read the guide");
    assert!(fixture.exists("tasks/Read the guide.md"));
    assert_eq!(rename_field(&app, B), None);
    assert_eq!(tasks::text(&app, "rename-problem"), None);
}

/// Open the task at `path` and type in it, so it has edits that aren't on
/// disk yet. Returns its tab.
fn type_unsaved(app: &mut App, path: &str, added: &str) -> app_core::TabId {
    let tab = open_from_tree(app, path);
    support::settle(app);
    let text = format!("{}{added}", app.note(tab).unwrap().text());
    // Far enough ahead that no save starts on its own before the trash.
    app.edit_note(tab, 1, &text, Instant::now() + Duration::from_secs(600));
    assert!(app.note(tab).unwrap().is_unsaved());
    tab
}

#[test]
fn move_to_trash_saves_the_tasks_edits_first_then_uses_the_system_trash_and_closes_its_tabs() {
    let fixture = workspace();
    let system = FakeSystem::new();
    let Some((mut app, _folder)) = tasks_app(&fixture, system.clone()) else {
        return;
    };
    let tab = type_unsaved(
        &mut app,
        "tasks/Water the beds.md",
        "Typed before the trash.\n",
    );
    app.open_page(PAGE, OpenAs::Kept);
    support::settle(&mut app);

    choose(&mut app, "trash", A);

    support::plugins::wait_until(&mut app, "the task goes to the trash", |_| {
        !fixture.exists("tasks/Water the beds.md")
    });
    let trashed = std::fs::read_to_string(system.in_bin("Water the beds.md")).unwrap();
    assert!(trashed.contains("Typed before the trash."), "{trashed}");
    assert!(app.tabs().iter().all(|shown| shown.id() != tab));
    assert_eq!(
        notice(&app),
        Some((
            "Moved \u{201c}Water the beds\u{201d} to the trash".to_owned(),
            "You can restore it from your system's trash.".to_owned()
        ))
    );
    support::plugins::wait_until(&mut app, "the list drops the task", |app| {
        labels(app) == ["Read the manual"]
    });
}

#[test]
fn when_the_system_trash_refuses_move_to_trash_uses_the_workspace_trash_folder() {
    let fixture = workspace();
    let system = FakeSystem::refusing_trash();
    let Some((mut app, _folder)) = tasks_app(&fixture, system.clone()) else {
        return;
    };
    let tab = type_unsaved(
        &mut app,
        "tasks/Water the beds.md",
        "Typed before the trash.\n",
    );
    app.open_page(PAGE, OpenAs::Kept);
    support::settle(&mut app);

    choose(&mut app, "trash", A);

    support::plugins::wait_until(&mut app, "the task goes to the trash", |_| {
        !fixture.exists("tasks/Water the beds.md")
    });
    let stamps: Vec<_> = std::fs::read_dir(fixture.root.join(".noura/trash"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    let [stamp] = stamps.as_slice() else {
        panic!("{stamps:?}");
    };
    let trashed = std::fs::read_to_string(stamp.join("tasks/Water the beds.md")).unwrap();
    assert!(trashed.contains("Typed before the trash."), "{trashed}");
    assert_eq!(system.calls().len(), 1);
    assert!(app.tabs().iter().all(|shown| shown.id() != tab));
    let stamp = stamp.file_name().unwrap().to_str().unwrap();
    assert_eq!(
        notice(&app),
        Some((
            "Moved \u{201c}Water the beds\u{201d} to .noura/trash".to_owned(),
            format!(
                "Noura couldn't use the system trash and moved it to \
                 .noura/trash/{stamp}/tasks/Water the beds.md in this workspace."
            )
        ))
    );
}
