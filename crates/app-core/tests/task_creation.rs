//! Seam 1: New task names a task before its file exists. Nothing reaches
//! the workspace folder until you press Enter or leave the field with a
//! name, Escape or an empty name creates nothing, a failed create keeps
//! the field open, and a name still in the field when the window closes
//! is created first. A new task gets a stable ID in `tasks/`, joins the
//! project whose view it was named in, and opens selected, with the list
//! switching to All when the view would hide it.

mod support;

use app_core::{OpenAs, PluginEvent, PluginNaming, PluginPlace};
use support::{
    Fixture, settle,
    tasks::{
        self, A, PAGE, labels, name_new_task, naming, project_file, property, selected, send,
        sidebar, text, with_tasks,
    },
};

fn named(name: &str) -> PluginEvent {
    PluginEvent::Named(PluginNaming {
        node: "new-task-name".to_owned(),
        name: name.to_owned(),
    })
}

#[test]
fn new_task_writes_nothing_until_you_name_the_task() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let before = fixture.snapshot();

    app.press_plugin_button(PAGE, "new-task");
    settle(&mut app);

    assert!(naming(&app));
    assert_eq!(fixture.snapshot(), before);

    send(&mut app, named("Buy seeds"));

    assert!(!naming(&app));
    let bytes = fixture.read("tasks/Buy seeds.md");
    let id = support::object_id(&bytes);
    assert!(id.starts_with("task_"), "{id}");
    assert_eq!(property(&bytes, "status"), Some("todo".into()));
    assert_eq!(property(&bytes, "priority"), Some("medium".into()));
}

#[test]
fn escape_or_an_empty_name_creates_nothing() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let before = fixture.snapshot();

    app.press_plugin_button(PAGE, "new-task");
    settle(&mut app);
    send(&mut app, PluginEvent::Cancelled("new-task-name".to_owned()));

    assert!(!naming(&app));
    assert_eq!(fixture.snapshot(), before);

    // A name of only spaces names nothing, even if it reaches the plugin.
    app.press_plugin_button(PAGE, "new-task");
    settle(&mut app);
    send(&mut app, named("   "));

    assert!(!naming(&app));
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn a_failed_create_keeps_the_field_open_so_you_can_try_again() {
    // A file named `tasks` stands where the tasks folder goes.
    let fixture = Fixture::new(&[("tasks", b"in the way")]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let before = fixture.snapshot();

    name_new_task(&mut app, "Buy seeds");

    assert!(naming(&app));
    let problem = text(&app, "new-task-problem").expect("the page says why");
    assert!(
        problem.starts_with("Couldn't create \u{201c}Buy seeds\u{201d}."),
        "{problem}"
    );
    assert_eq!(fixture.snapshot(), before);

    std::fs::remove_file(fixture.root.join("tasks")).unwrap();
    send(&mut app, named("Buy seeds"));

    assert!(!naming(&app));
    assert_eq!(text(&app, "new-task-problem"), None);
    assert!(fixture.exists("tasks/Buy seeds.md"));
}

#[test]
fn a_name_still_in_the_field_when_the_window_closes_is_created_first() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    // A second window keeps the workspace open while this one closes.
    let mut other = app.new_window().expect("a second window opens");
    app.press_plugin_button(PAGE, "new-task");
    settle(&mut app);
    let place = PluginPlace::Page(PAGE.to_owned());
    app.set_plugin_name_draft(place.clone(), "new-task-name", "", "Call the plumber");

    app.close_window();
    support::plugins::wait_until(&mut app, "the window closes", |app| app.is_closed());

    let bytes = fixture.read("tasks/Call the plumber.md");
    assert_eq!(property(&bytes, "status"), Some("todo".into()));
    settle(&mut other);
    assert_eq!(labels(&other), ["Call the plumber"]);
}

#[test]
fn an_empty_or_cancelled_field_leaves_nothing_to_create_when_the_window_closes() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let mut other = app.new_window().expect("a second window opens");
    let before = fixture.snapshot();
    app.press_plugin_button(PAGE, "new-task");
    settle(&mut app);
    let place = PluginPlace::Page(PAGE.to_owned());
    app.set_plugin_name_draft(place.clone(), "new-task-name", "", "Water the roses");
    send(&mut app, PluginEvent::Cancelled("new-task-name".to_owned()));
    app.set_plugin_name_draft(place, "other-name", "", "   ");

    app.close_window();
    support::plugins::wait_until(&mut app, "the window closes", |app| app.is_closed());

    settle(&mut other);
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn a_new_task_opens_selected_and_the_list_switches_to_all_when_the_view_would_hide_it() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    app.open_page(PAGE, OpenAs::Preview);
    assert_eq!(text(&app, "view").as_deref(), Some("Today"));

    name_new_task(&mut app, "Plan the trip");

    // Today holds only dated tasks, so the list shows All.
    assert_eq!(text(&app, "view").as_deref(), Some("All"));
    assert_eq!(labels(&app), ["Plan the trip"]);
    let id = support::object_id(&fixture.read("tasks/Plan the trip.md"));
    assert_eq!(selected(&app).as_deref(), Some(id.as_str()));
    let views = &sidebar(&app)[0];
    assert_eq!(views.selected.as_deref(), Some("view:all"));
    // It opens in the note editor, in a tab of its own, and the Tasks
    // page keeps its tab.
    let tab = app.active_tab().expect("the task opens");
    assert_eq!(
        tab.file().map(|file| file.relative_path.as_str()),
        Some("tasks/Plan the trip.md")
    );
    assert!(app.tabs().iter().any(|tab| matches!(
        tab.content(),
        app_core::TabContent::Page { page_id, .. } if page_id == PAGE
    )));
}

#[test]
fn a_task_named_in_a_project_view_joins_the_project_and_the_view_stays() {
    let project = format!("project_{A}");
    let fixture = Fixture::new(&[("projects/Garden/Garden.md", &project_file(A))]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    tasks::choose(&mut app, "projects", &format!("project:{project}"));
    assert_eq!(text(&app, "view").as_deref(), Some("Garden"));

    name_new_task(&mut app, "Turn the compost");

    let bytes = fixture.read("tasks/Turn the compost.md");
    assert_eq!(property(&bytes, "project"), Some(project.clone().into()));
    assert_eq!(text(&app, "view").as_deref(), Some("Garden"));
    assert_eq!(labels(&app), ["Turn the compost"]);
}

#[test]
fn new_task_from_the_command_palette_opens_the_tasks_page_with_the_name_field() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    app.open_page(app_core::INBOX_PAGE, OpenAs::Kept);
    let before = fixture.snapshot();

    assert!(app.run_command(
        "plugin-command:tasks:new-task",
        app_core::CommandSource::Palette,
        std::time::Instant::now()
    ));
    settle(&mut app);

    let tab = app.active_tab().expect("a tab shows");
    assert!(
        matches!(tab.content(), app_core::TabContent::Page { page_id, .. } if page_id == PAGE),
        "{:?}",
        tab.content()
    );
    assert!(naming(&app));
    assert_eq!(fixture.snapshot(), before);
}
