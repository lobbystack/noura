//! The Tasks plugin through AccessKit: New task from the keyboard alone,
//! again and again, from the button and from the command palette, the
//! sidebar's views, the task rows with their checkboxes and overdue
//! warning, and the labels of a task's properties panel.

mod support;

use std::path::{Path, PathBuf};

use egui::{Event, Key, Modifiers, accesskit::Role};
use egui_kittest::kittest::{NodeT, Queryable};
use support::{
    FakePicker, create_workspace, harness,
    kit::{Ui, focused, labels, open_page, press, press_with, run},
    plugins, settle,
};

const MANIFEST: &str = include_str!("../../plugins/tasks/plugin.toml");

const PROJECT: &str = "---\nid: project_01jbcdefghjkmnpqrstvwxyz0h\ntype: project\nstatus: active\n\
                       created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\nOverview.\n";

fn task(suffix: &str, properties: &str) -> String {
    format!(
        "---\nid: task_01jbcdefghjkmnpqrstvwxyz0{suffix}\ntype: task\n{properties}\
         created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\nNotes.\n"
    )
}

struct Opened {
    harness: Ui,
    folder: PathBuf,
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace with two tasks, one long overdue, and a project, with the
/// Tasks plugin installed and on, opened at launch. `None` when the plugin
/// isn't built.
fn opened() -> Option<Opened> {
    let component = plugins::built("tasks_plugin")?;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    let write = |path: &str, text: &str| {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    write(
        "tasks/Water the beds.md",
        &task("a", "status: todo\npriority: medium\ndue: 2020-01-06\n"),
    );
    write(
        "tasks/Read the manual.md",
        &task("b", "status: todo\npriority: high\n"),
    );
    write("projects/Garden/Garden.md", PROJECT);
    let plugin = plugins::folder(parent.path(), MANIFEST, "tasks_plugin.wasm", &component);
    plugins::install_and_enable(app_data.path(), &plugin, "tasks");
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    open_page(&mut harness, "Tasks");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::ListBox, "Views")
            .is_some()
    });
    Some(Opened {
        harness,
        folder,
        _folders: (app_data, parent),
    })
}

/// Run frames, waiting in real time for the plugin's answers, until
/// `done` holds.
fn wait_until(harness: &mut Ui, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done(harness) {
        assert!(
            std::time::Instant::now() < deadline,
            "the screen never got there; the active tab is {:?} and the list shows {:?}",
            harness
                .state()
                .app()
                .active_tab()
                .map(|tab| tab.title().to_owned()),
            task_list(harness),
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        harness.step();
    }
    run(harness);
}

fn task_list(harness: &Ui) -> Vec<String> {
    harness
        .query_by_role_and_label(Role::ListBox, "Task list")
        .map(|list| labels(list.get_all_by_role(Role::ListBoxOption)))
        .unwrap_or_default()
}

fn type_text(harness: &mut Ui, text: &str) {
    harness.event(Event::Text(text.to_owned()));
    run(harness);
}

fn status(folder: &Path, path: &str) -> String {
    let text = std::fs::read_to_string(folder.join(path)).unwrap();
    text.lines()
        .find_map(|line| line.strip_prefix("status: "))
        .unwrap_or_default()
        .to_owned()
}

#[test]
fn a_task_is_created_from_the_keyboard_alone() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    harness
        .get_by_role_and_label(Role::Button, "New task")
        .focus();
    run(&mut harness);

    press(&mut harness, Key::Space);
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::TextInput, "New task name")
            .is_some_and(|field| field.is_focused())
    });
    assert!(!folder.join("tasks/Buy seeds.md").exists());
    type_text(&mut harness, "Buy seeds");
    press(&mut harness, Key::Enter);

    wait_until(&mut harness, |_| folder.join("tasks/Buy seeds.md").exists());
    assert_eq!(status(&folder, "tasks/Buy seeds.md"), "todo");
    // The new task opens in its own tab, and the Tasks list shows All.
    wait_until(&mut harness, |harness| {
        harness
            .state()
            .app()
            .active_tab()
            .is_some_and(|tab| tab.title() == "Buy seeds")
    });
    open_page(&mut harness, "Tasks");
    wait_until(&mut harness, |harness| {
        task_list(harness).contains(&"Buy seeds".to_owned())
    });
    assert!(
        harness
            .query_by_role_and_label(Role::TextInput, "New task name")
            .is_none()
    );
}

#[test]
fn escape_closes_the_name_field_and_creates_nothing() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    harness
        .get_by_role_and_label(Role::Button, "New task")
        .click();
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::TextInput, "New task name")
            .is_some_and(|field| field.is_focused())
    });
    type_text(&mut harness, "Rake the leaves");

    press(&mut harness, Key::Escape);

    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::TextInput, "New task name")
            .is_none()
    });
    assert!(!folder.join("tasks/Rake the leaves.md").exists());
}

/// The files in the workspace's `tasks` folder, by name.
fn task_files(folder: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(folder.join("tasks"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Whether the name field shows, empty and with the keyboard.
fn empty_and_focused(harness: &Ui) -> bool {
    harness
        .query_by_role_and_label(Role::TextInput, "New task name")
        .is_some_and(|field| field.is_focused() && field.value().unwrap_or_default().is_empty())
}

#[test]
fn each_new_task_opens_an_empty_field_that_takes_the_keyboard() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    let before = task_files(&folder);

    // A name you cancelled never comes back, and leaving the field again
    // sends nothing.
    for name in ["Rake the leaves", "Prune the roses"] {
        harness
            .get_by_role_and_label(Role::Button, "New task")
            .focus();
        run(&mut harness);
        press(&mut harness, Key::Space);
        wait_until(&mut harness, empty_and_focused);
        type_text(&mut harness, name);
        press(&mut harness, Key::Escape);
        wait_until(&mut harness, |harness| {
            harness
                .query_by_role_and_label(Role::TextInput, "New task name")
                .is_none()
        });
    }
    // After a task is created, the next field starts empty too.
    harness
        .get_by_role_and_label(Role::Button, "New task")
        .focus();
    run(&mut harness);
    press(&mut harness, Key::Space);
    wait_until(&mut harness, empty_and_focused);
    type_text(&mut harness, "Buy seeds");
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |_| folder.join("tasks/Buy seeds.md").exists());
    open_page(&mut harness, "Tasks");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Button, "New task")
            .is_some()
    });
    harness
        .get_by_role_and_label(Role::Button, "New task")
        .focus();
    run(&mut harness);
    press(&mut harness, Key::Space);
    wait_until(&mut harness, empty_and_focused);
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::TextInput, "New task name")
            .is_none()
    });

    let mut expected = before;
    expected.push("Buy seeds.md".to_owned());
    expected.sort();
    assert_eq!(task_files(&folder), expected);
}

#[test]
fn new_task_in_the_command_palette_starts_the_same_name_first_flow() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    harness
        .state_mut()
        .app_mut()
        .open_page(app_core::INBOX_PAGE, app_core::OpenAs::Kept);
    run(&mut harness);

    press_with(&mut harness, Modifiers::COMMAND, Key::K);
    harness
        .get_by_label("Search commands")
        .type_text("New task");
    run(&mut harness);
    press(&mut harness, Key::Enter);

    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::TextInput, "New task name")
            .is_some_and(|field| field.is_focused())
    });
    type_text(&mut harness, "Order seeds");
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |_| {
        folder.join("tasks/Order seeds.md").exists()
    });
}

#[test]
fn the_sidebar_lists_the_views_projects_and_folders_and_one_selection_switches_the_list() {
    let Some(Opened {
        mut harness,
        _folders,
        ..
    }) = opened()
    else {
        return;
    };
    let views = harness.get_by_role_and_label(Role::ListBox, "Views");
    assert_eq!(
        labels(views.get_all_by_role(Role::ListBoxOption)),
        ["Today", "Upcoming", "All", "Completed"]
    );
    let projects = harness.get_by_role_and_label(Role::ListBox, "Projects");
    assert_eq!(
        labels(projects.get_all_by_role(Role::ListBoxOption)),
        ["Garden"]
    );
    let folders = harness.get_by_role_and_label(Role::ListBox, "Folders");
    let tasks_folder = folders.get_by_role_and_label(Role::ListBoxOption, "tasks");
    assert_eq!(
        tasks_folder.accesskit_node().description(),
        Some("2".into())
    );
    // The file tree stays below the section.
    harness.get_by_role_and_label(Role::TreeItem, "tasks");
    assert_eq!(task_list(&harness), ["Water the beds, Overdue"]);

    harness
        .get_by_role_and_label(Role::ListBox, "Views")
        .get_by_role_and_label(Role::ListBoxOption, "All")
        .click();
    wait_until(&mut harness, |harness| task_list(harness).len() == 2);
    assert_eq!(
        task_list(&harness),
        ["Water the beds, Overdue", "Read the manual"]
    );

    // One arrow key moves to the next view.
    press(&mut harness, Key::ArrowDown);
    wait_until(&mut harness, |harness| task_list(harness).is_empty());
    assert_eq!(focused(&harness).as_deref(), Some("Completed"));
    harness.get_by_label("No tasks here");
}

#[test]
fn a_rows_checkbox_marks_its_task_done_and_overdue_tasks_say_so() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    let row = harness.get_by_role_and_label(Role::ListBoxOption, "Water the beds, Overdue");
    assert_eq!(
        row.accesskit_node().description(),
        Some("Jan 6, 2020".into())
    );
    assert!(harness.unnamed_widgets().is_empty());

    harness
        .get_by_role_and_label(Role::CheckBox, "Water the beds")
        .click();

    wait_until(&mut harness, |_| {
        status(&folder, "tasks/Water the beds.md") == "done"
    });
    // Done, it leaves Today.
    wait_until(&mut harness, |harness| task_list(harness).is_empty());
}

#[test]
fn a_task_opens_beside_the_tasks_panel_with_its_labeled_properties() {
    let Some(Opened {
        mut harness,
        _folders,
        ..
    }) = opened()
    else {
        return;
    };
    harness
        .get_by_role_and_label(Role::TreeItem, "tasks")
        .click();
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "Read the manual")
        .click();
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Group, "Properties")
            .is_some_and(|panel| panel.query_by_role_and_label(Role::Group, "Task").is_some())
    });

    harness.get_by_role_and_label(Role::Document, "Note text");
    let panel = harness.get_by_role_and_label(Role::Group, "Properties");
    let status = panel.get_by_role_and_label(Role::RadioGroup, "Status");
    assert_eq!(
        labels(status.get_all_by_role(Role::RadioButton)),
        ["To do", "In progress", "Done", "Cancelled"]
    );
    let priority = panel.get_by_role_and_label(Role::RadioGroup, "Priority");
    assert_eq!(
        labels(priority.get_all_by_role(Role::RadioButton)),
        ["Low", "Medium", "High", "Urgent"]
    );
    panel.get_by_role_and_label(Role::TextInput, "Due");
    let project = panel.get_by_role_and_label(Role::RadioGroup, "Project");
    assert_eq!(
        labels(project.get_all_by_role(Role::RadioButton)),
        ["No project", "Garden"]
    );
}

/// The Inbox's Due today list.
fn due_today(harness: &Ui) -> Vec<String> {
    harness
        .query_by_role_and_label(Role::ListBox, "Due today")
        .map(|list| labels(list.get_all_by_role(Role::ListBoxOption)))
        .unwrap_or_default()
}

#[test]
fn due_today_in_the_inbox_shows_each_tasks_name_and_due_label_and_opens_it_on_click() {
    let Some(Opened {
        mut harness,
        _folders,
        ..
    }) = opened()
    else {
        return;
    };
    open_page(&mut harness, "Inbox");
    wait_until(&mut harness, |harness| !due_today(harness).is_empty());

    harness.get_by_role_and_label(Role::Heading, "Due today");
    harness.get_by_label("1 task");
    assert_eq!(due_today(&harness), ["Water the beds, Overdue"]);
    let row = harness
        .get_by_role_and_label(Role::ListBox, "Due today")
        .get_by_role_and_label(Role::ListBoxOption, "Water the beds, Overdue");
    assert_eq!(
        row.accesskit_node().description(),
        Some("Jan 6, 2020".into())
    );
    assert!(harness.unnamed_widgets().is_empty());

    row.click();

    wait_until(&mut harness, |harness| {
        harness
            .state()
            .app()
            .active_tab()
            .is_some_and(|tab| tab.title() == "Water the beds")
    });
}

#[test]
fn due_today_says_when_nothing_is_due_and_all_tasks_opens_the_tasks_page() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    open_page(&mut harness, "Inbox");
    wait_until(&mut harness, |harness| !due_today(harness).is_empty());

    harness
        .get_by_role_and_label(Role::ListBox, "Due today")
        .get_by_role_and_label(Role::CheckBox, "Water the beds")
        .click();

    wait_until(&mut harness, |harness| {
        harness.query_by_label("Nothing due today.").is_some()
    });
    assert_eq!(status(&folder, "tasks/Water the beds.md"), "done");
    assert!(due_today(&harness).is_empty());

    harness
        .get_by_role_and_label(Role::Button, "All tasks")
        .click();

    wait_until(&mut harness, |harness| {
        harness
            .state()
            .app()
            .active_tab()
            .is_some_and(|tab| tab.title() == "Tasks")
    });
}

fn add_button(harness: &Ui) -> egui_kittest::Node<'_> {
    harness
        .get_by_role_and_label(Role::Form, "Add a task for today")
        .get_by_role_and_label(Role::Button, "Add")
}

#[test]
fn add_stays_disabled_until_you_type_a_name_and_the_field_clears_after_adding() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    open_page(&mut harness, "Inbox");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Form, "Add a task for today")
            .is_some()
    });
    assert!(add_button(&harness).accesskit_node().is_disabled());

    harness
        .get_by_role_and_label(Role::Form, "Add a task for today")
        .get_by_role_and_label(Role::TextInput, "Task")
        .focus();
    run(&mut harness);
    type_text(&mut harness, "   ");
    assert!(add_button(&harness).accesskit_node().is_disabled());
    type_text(&mut harness, "Buy seeds");
    assert!(!add_button(&harness).accesskit_node().is_disabled());

    add_button(&harness).click();

    wait_until(&mut harness, |harness| {
        due_today(harness).contains(&"Buy seeds".to_owned())
    });
    let today = jiff::Timestamp::now()
        .to_zoned(harness.state().app().time_zone())
        .date()
        .to_string();
    let text = std::fs::read_to_string(folder.join("tasks/Buy seeds.md")).unwrap();
    assert!(text.contains(&format!("due: {today}")), "{text}");
    let field = harness
        .get_by_role_and_label(Role::Form, "Add a task for today")
        .get_by_role_and_label(Role::TextInput, "Task");
    assert_eq!(field.value().unwrap_or_default(), "");
    assert!(add_button(&harness).accesskit_node().is_disabled());
}
