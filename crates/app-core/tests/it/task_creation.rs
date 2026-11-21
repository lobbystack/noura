//! Seam 1: a task is named before its file exists. New task gives the add
//! field at the top of the list the keyboard and writes nothing; Return in
//! the field creates the task, its file named after the title you typed,
//! with a stable ID of its own. The new task joins the list it was added
//! in and shows selected beside it, and the page stays: nothing opens in
//! a tab. An empty name creates nothing, and a failed create says why
//! under the field.

use crate::support;
use app_core::OpenAs;
use support::{
    Fixture,
    tasks::{
        self, A, PAGE, add_focused, add_task, labels, project_file, property, selected, sidebar,
        text, with_tasks,
    },
};

/// Whether only the Tasks page shows, in one tab.
fn only_the_page_shows(app: &app_core::App) -> bool {
    app.tabs().len() == 1
        && app.active_tab().is_some_and(|tab| {
            matches!(
                tab.content(),
                app_core::TabContent::Page { page_id, .. } if page_id == PAGE
            )
        })
}

#[test]
fn new_task_readies_the_add_field_and_writes_nothing_until_you_name_the_task() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    app.open_page(PAGE, OpenAs::Preview);
    support::settle(&mut app);
    let before = fixture.snapshot();

    app.press_plugin_button(PAGE, "new-task");
    support::settle(&mut app);

    assert!(add_focused(&app));
    assert_eq!(fixture.snapshot(), before);

    // You type the name and press Return.
    add_task(&mut app, "Buy seeds");

    let bytes = fixture.read("Tasks/Buy seeds.md");
    let id = support::object_id(&bytes);
    assert!(id.starts_with("task_"), "{id}");
    assert_eq!(property(&bytes, "status"), Some("todo".into()));
    assert_eq!(property(&bytes, "priority"), Some("medium".into()));
    // The field asks for the keyboard once, not on every draw.
    assert!(!add_focused(&app));
}

/// The bug where New task named a file `new-task--7traxe` and the list
/// showed that: the title you type names the task and its file, and the
/// stable ID lives in the file, apart from its path.
#[test]
fn the_title_you_type_names_the_task_and_its_file() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    app.open_page(PAGE, OpenAs::Preview);
    tasks::choose(&mut app, "views", "view:all");

    add_task(&mut app, "Plan the trip");
    add_task(&mut app, "Plan the trip");

    let first = support::object_id(&fixture.read("Tasks/Plan the trip.md"));
    let second = support::object_id(&fixture.read("Tasks/Plan the trip 1.md"));
    assert_ne!(first, second);
    assert_eq!(labels(&app), ["Plan the trip", "Plan the trip 1"]);
    assert!(
        labels(&app)
            .iter()
            .all(|label| !label.starts_with("new-task")),
        "{:?}",
        labels(&app)
    );
}

#[test]
fn an_empty_name_creates_nothing() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    app.open_page(PAGE, OpenAs::Preview);
    support::settle(&mut app);
    let before = fixture.snapshot();

    // A name of only spaces names nothing, even if it reaches the plugin.
    add_task(&mut app, "   ");

    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn a_failed_create_says_why_under_the_field_so_you_can_try_again() {
    // A file named `Tasks` stands where the Tasks folder goes.
    let fixture = Fixture::new(&[("Tasks", b"in the way")]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    app.open_page(PAGE, OpenAs::Preview);
    support::settle(&mut app);
    let before = fixture.snapshot();

    add_task(&mut app, "Buy seeds");

    let problem = text(&app, "add-problem").expect("the page says why");
    assert!(
        problem.starts_with("Couldn't create \u{201c}Buy seeds\u{201d}."),
        "{problem}"
    );
    assert_eq!(fixture.snapshot(), before);

    std::fs::remove_file(fixture.root.join("Tasks")).unwrap();
    add_task(&mut app, "Buy seeds");

    assert_eq!(text(&app, "add-problem"), None);
    assert!(fixture.exists("Tasks/Buy seeds.md"));
}

/// The bug where New task jumped to the Files tab and opened the task as a
/// note: the task shows selected beside the list, and no tab opens.
#[test]
fn a_new_task_shows_selected_beside_the_list_and_opens_no_tab() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    app.open_page(PAGE, OpenAs::Preview);
    support::settle(&mut app);
    assert_eq!(text(&app, "view").as_deref(), Some("Today"));

    add_task(&mut app, "Water the roses");

    // Added in Today, the task is due today, so it stays in the view.
    let bytes = fixture.read("Tasks/Water the roses.md");
    let id = support::object_id(&bytes);
    assert_eq!(
        property(&bytes, "due"),
        Some(app.today().to_string().into())
    );
    assert_eq!(text(&app, "view").as_deref(), Some("Today"));
    assert_eq!(labels(&app), ["Water the roses"]);
    assert_eq!(selected(&app).as_deref(), Some(id.as_str()));
    assert_eq!(
        tasks::pane_input(&app, "title").map(|(value, _)| value),
        Some(app_core::InputValue::Text("Water the roses".to_owned()))
    );
    assert!(only_the_page_shows(&app));
}

#[test]
fn a_task_added_where_the_view_would_hide_it_switches_the_list_to_all() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    app.open_page(PAGE, OpenAs::Preview);
    tasks::choose(&mut app, "views", "view:completed");

    add_task(&mut app, "Plan the trip");

    // Completed holds only done tasks, so the list shows All.
    assert_eq!(text(&app, "view").as_deref(), Some("All"));
    assert_eq!(labels(&app), ["Plan the trip"]);
    let id = support::object_id(&fixture.read("Tasks/Plan the trip.md"));
    assert_eq!(selected(&app).as_deref(), Some(id.as_str()));
    let views = &sidebar(&app)[0];
    assert_eq!(views.selected.as_deref(), Some("view:all"));
    // Added outside Today, it has no due date.
    assert_eq!(
        property(&fixture.read("Tasks/Plan the trip.md"), "due"),
        None
    );
    assert!(only_the_page_shows(&app));
}

#[test]
fn a_task_added_in_a_project_view_joins_the_project_and_the_view_stays() {
    let project = format!("project_{A}");
    let fixture = Fixture::new(&[("projects/Garden/Garden.md", &project_file(A))]);
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    tasks::choose(&mut app, "projects", &format!("project:{project}"));
    assert_eq!(text(&app, "view").as_deref(), Some("Garden"));

    add_task(&mut app, "Turn the compost");

    let bytes = fixture.read("Tasks/Turn the compost.md");
    assert_eq!(property(&bytes, "project"), Some("[[Garden]]".into()));
    assert_eq!(text(&app, "view").as_deref(), Some("Garden"));
    assert_eq!(labels(&app), ["Turn the compost"]);
}

#[test]
fn new_task_from_the_command_palette_opens_the_tasks_page_with_the_add_field_ready() {
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
    support::settle(&mut app);

    let tab = app.active_tab().expect("a tab shows");
    assert!(
        matches!(tab.content(), app_core::TabContent::Page { page_id, .. } if page_id == PAGE),
        "{:?}",
        tab.content()
    );
    assert!(add_focused(&app));
    assert_eq!(fixture.snapshot(), before);
}
