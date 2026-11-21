//! The Tasks plugin through AccessKit: adding tasks from the field at the
//! top of the list, again and again, from New task and from the command
//! palette, without leaving the page; the sidebar's views and counts; the
//! list's sections with their counts; the task rows with their checkboxes
//! and overdue dates; and the selected task's details, edited in place.

use crate::support;
use std::path::{Path, PathBuf};

use egui::{Event, Key, Modifiers, accesskit::Role};
use egui_kittest::kittest::{NodeT, Queryable};
use support::{
    FakePicker, create_workspace, harness,
    kit::{Ui, focused, labels, open_page, press, press_with, run},
    plugins, settle,
};

const MANIFEST: &str = include_str!("../../../plugins/tasks/plugin.toml");

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
    create_workspace(app_data.path(), &folder);
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

/// The sections a task list shows, in order.
const SECTIONS: [&str; 5] = ["Overdue", "Today", "Upcoming", "No date", "Completed"];

/// The tasks the page lists, top to bottom, across its sections.
fn task_list(harness: &Ui) -> Vec<String> {
    SECTIONS
        .iter()
        .filter_map(|section| harness.query_by_role_and_label(Role::ListBox, section))
        .flat_map(|list| labels(list.get_all_by_role(Role::ListBoxOption)))
        .collect()
}

/// The tasks one section lists.
fn section(harness: &Ui, title: &str) -> Vec<String> {
    harness
        .query_by_role_and_label(Role::ListBox, title)
        .map(|list| labels(list.get_all_by_role(Role::ListBoxOption)))
        .unwrap_or_default()
}

/// The text a task's file holds.
fn file(folder: &Path, path: &str) -> String {
    std::fs::read_to_string(folder.join(path)).unwrap_or_default()
}

/// The add field at the top of the list.
fn add_field(harness: &Ui) -> egui_kittest::Node<'_> {
    harness.get_by_role_and_label(Role::TextInput, "Add task")
}

/// Whether the add field is empty and has the keyboard.
fn add_ready(harness: &Ui) -> bool {
    harness
        .query_by_role_and_label(Role::TextInput, "Add task")
        .is_some_and(|field| field.is_focused() && field.value().unwrap_or_default().is_empty())
}

/// The title of the active tab.
fn active_tab(harness: &Ui) -> Option<String> {
    harness
        .state()
        .app()
        .active_tab()
        .map(|tab| tab.title().to_owned())
}

/// The selected task's title field, once its details show.
fn title_value(harness: &Ui) -> Option<String> {
    harness
        .query_by_role_and_label(Role::TextInput, "Title")
        .and_then(|field| field.value())
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
fn the_add_field_creates_a_task_named_by_its_title_and_keeps_you_on_the_page() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    let today = harness.state().app().today().to_string();
    add_field(&harness).click();
    run(&mut harness);
    type_text(&mut harness, "Buy seeds");
    // Nothing reaches the folder until Return.
    assert!(!folder.join("tasks/Buy seeds.md").exists());
    press(&mut harness, Key::Enter);

    // The file is named after the title, and the task is due today, as
    // it was added in Today.
    wait_until(&mut harness, |_| folder.join("tasks/Buy seeds.md").exists());
    let text = file(&folder, "tasks/Buy seeds.md");
    assert!(text.contains("status: todo"), "{text}");
    assert!(text.contains(&format!("due: {today}")), "{text}");
    // The task shows in the list and its details beside it, and the page
    // stays: no tab opens for it.
    wait_until(&mut harness, |harness| {
        section(harness, "Today") == ["Buy seeds"]
            && title_value(harness).as_deref() == Some("Buy seeds")
    });
    assert_eq!(active_tab(&harness).as_deref(), Some("Tasks"));
    assert_eq!(harness.state().app().tabs().len(), 1);
    // The field empties and keeps the keyboard for the next one.
    assert!(add_ready(&harness));
    type_text(&mut harness, "Order seeds");
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |harness| {
        section(harness, "Today") == ["Buy seeds", "Order seeds"]
            && title_value(harness).as_deref() == Some("Order seeds")
    });
    assert!(folder.join("tasks/Order seeds.md").exists());
    assert!(add_ready(&harness));

    // Escape empties the field and creates nothing.
    type_text(&mut harness, "Rake the leaves");
    press(&mut harness, Key::Escape);
    run(&mut harness);
    assert_eq!(add_field(&harness).value().unwrap_or_default(), "");
    assert!(!folder.join("tasks/Rake the leaves.md").exists());
}

#[test]
fn new_task_gives_the_add_field_the_keyboard_without_leaving_the_page() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    harness
        .get_by_role_and_label(Role::ListBox, "Views")
        .get_by_role_and_label(Role::ListBoxOption, "All")
        .click();
    wait_until(&mut harness, |harness| task_list(harness).len() == 2);
    let before = task_files(&folder);

    // New task, from the keyboard alone.
    harness
        .get_by_role_and_label(Role::Button, "New task")
        .focus();
    run(&mut harness);
    press(&mut harness, Key::Space);
    wait_until(&mut harness, add_ready);
    assert_eq!(task_files(&folder), before);
    type_text(&mut harness, "Plan the trip");
    press(&mut harness, Key::Enter);

    wait_until(&mut harness, |harness| {
        title_value(harness).as_deref() == Some("Plan the trip")
    });
    // In All, a task gets no due date, and lists under No date.
    let text = file(&folder, "tasks/Plan the trip.md");
    assert!(!text.contains("due:"), "{text}");
    assert_eq!(
        section(&harness, "No date"),
        ["Read the manual", "Plan the trip"]
    );
    assert_eq!(active_tab(&harness).as_deref(), Some("Tasks"));

    // Pressed again, it takes the keyboard back from the list.
    harness
        .get_by_role_and_label(Role::ListBox, "No date")
        .get_by_role_and_label(Role::ListBoxOption, "Read the manual")
        .click();
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::Button, "New task")
        .click();
    wait_until(&mut harness, add_ready);
}

#[test]
fn new_task_in_the_command_palette_opens_tasks_with_the_add_field_ready() {
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
        .get_by_role_and_label(Role::TextInput, "Search")
        .type_text("New task");
    run(&mut harness);
    press(&mut harness, Key::Enter);

    wait_until(&mut harness, add_ready);
    assert_eq!(active_tab(&harness).as_deref(), Some("Tasks"));
    type_text(&mut harness, "Order seeds");
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |_| {
        folder.join("tasks/Order seeds.md").exists()
    });
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

#[test]
fn the_sidebar_lists_the_views_projects_and_folders_with_open_counts() {
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
        ["Today", "Next 7 days", "All", "Completed"]
    );
    let count = |harness: &Ui, list: &str, item: &str| {
        harness
            .get_by_role_and_label(Role::ListBox, list)
            .get_by_role_and_label(Role::ListBoxOption, item)
            .accesskit_node()
            .description()
    };
    assert_eq!(count(&harness, "Views", "Today"), Some("1".into()));
    assert_eq!(count(&harness, "Views", "Next 7 days"), Some("1".into()));
    assert_eq!(count(&harness, "Views", "All"), Some("2".into()));
    assert_eq!(count(&harness, "Views", "Completed"), None);
    let projects = harness.get_by_role_and_label(Role::ListBox, "Projects");
    assert_eq!(
        labels(projects.get_all_by_role(Role::ListBoxOption)),
        ["Garden"]
    );
    assert_eq!(count(&harness, "Folders", "tasks"), Some("2".into()));
    // The file tree stays below the section.
    harness.get_by_role_and_label(Role::TreeItem, "tasks");
    assert_eq!(task_list(&harness), ["Water the beds, Jan 6, 2020"]);

    harness
        .get_by_role_and_label(Role::ListBox, "Views")
        .get_by_role_and_label(Role::ListBoxOption, "All")
        .click();
    wait_until(&mut harness, |harness| task_list(harness).len() == 2);
    assert_eq!(
        task_list(&harness),
        ["Water the beds, Jan 6, 2020", "Read the manual"]
    );

    // One arrow key moves to the next view.
    press(&mut harness, Key::ArrowDown);
    wait_until(&mut harness, |harness| task_list(harness).is_empty());
    assert_eq!(focused(&harness).as_deref(), Some("Completed"));
    harness.get_by_label("No tasks here");
}

#[test]
fn the_list_groups_tasks_by_when_they_are_due_with_a_count_for_each_section() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    let today = harness.state().app().today();
    let day = |days: i64| {
        today
            .checked_add(jiff::Span::new().days(days))
            .unwrap()
            .to_string()
    };
    let write = |path: &str, text: String| std::fs::write(folder.join(path), text).unwrap();
    write(
        "tasks/Sow the peas.md",
        task("c", &format!("status: todo\ndue: {}\n", day(0))),
    );
    write(
        "tasks/Mulch.md",
        task("d", &format!("status: todo\ndue: {}\n", day(3))),
    );
    write(
        "tasks/Order tools.md",
        task("e", &format!("status: todo\ndue: {}\n", day(30))),
    );
    write(
        "tasks/Fix the gate.md",
        task("f", "status: done\npriority: low\n"),
    );
    harness
        .get_by_role_and_label(Role::ListBox, "Views")
        .get_by_role_and_label(Role::ListBoxOption, "All")
        .click();
    wait_until(&mut harness, |harness| task_list(harness).len() == 6);

    assert_eq!(
        section(&harness, "Overdue"),
        ["Water the beds, Jan 6, 2020"]
    );
    assert_eq!(section(&harness, "Today"), ["Sow the peas"]);
    assert_eq!(section(&harness, "Upcoming"), ["Mulch", "Order tools"]);
    assert_eq!(section(&harness, "No date"), ["Read the manual"]);
    assert_eq!(section(&harness, "Completed"), ["Fix the gate"]);
    // Each section's heading says how many tasks it holds.
    for (title, count) in [
        ("Overdue", "1"),
        ("Today", "1"),
        ("Upcoming", "2"),
        ("No date", "1"),
        ("Completed", "1"),
    ] {
        let heading = harness.get_by_role_and_label(Role::Heading, title);
        heading.get_by_label(count);
    }
    // A row's detail says its project, its due date and a priority that
    // isn't medium.
    let detail = |harness: &Ui, list: &str, row: &str| {
        harness
            .get_by_role_and_label(Role::ListBox, list)
            .get_by_role_and_label(Role::ListBoxOption, row)
            .accesskit_node()
            .description()
    };
    assert_eq!(
        detail(&harness, "Today", "Sow the peas"),
        Some("Today".into())
    );
    assert_eq!(
        detail(&harness, "No date", "Read the manual"),
        Some("High".into())
    );

    // Next 7 days holds what's overdue and due by the sixth day from now.
    harness
        .get_by_role_and_label(Role::ListBox, "Views")
        .get_by_role_and_label(Role::ListBoxOption, "Next 7 days")
        .click();
    wait_until(&mut harness, |harness| task_list(harness).len() == 3);
    assert_eq!(
        task_list(&harness),
        ["Water the beds, Jan 6, 2020", "Sow the peas", "Mulch"]
    );
    let views = harness.get_by_role_and_label(Role::ListBox, "Views");
    assert_eq!(
        views
            .get_by_role_and_label(Role::ListBoxOption, "Next 7 days")
            .accesskit_node()
            .description(),
        Some("3".into())
    );
    // In Today, each section has its title while there are two.
    harness
        .get_by_role_and_label(Role::ListBox, "Views")
        .get_by_role_and_label(Role::ListBoxOption, "Today")
        .click();
    wait_until(&mut harness, |harness| task_list(harness).len() == 2);
    let headings = |harness: &Ui, text: &str| {
        harness
            .get_all_by_role_and_label(Role::Heading, text)
            .count()
    };
    assert_eq!(headings(&harness, "Overdue"), 1);
    assert_eq!(headings(&harness, "Today"), 2);
    // Alone, the Today section needs no title under the page's.
    harness
        .get_by_role_and_label(Role::CheckBox, "Water the beds")
        .click();
    wait_until(&mut harness, |harness| {
        task_list(harness) == ["Sow the peas"]
    });
    assert_eq!(headings(&harness, "Today"), 1);
}

#[test]
fn a_rows_checkbox_marks_its_task_done_and_an_overdue_date_shows_as_a_warning() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    let row = harness.get_by_role_and_label(Role::ListBoxOption, "Water the beds, Jan 6, 2020");
    assert_eq!(row.accesskit_node().description(), None);
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

/// As in the legacy app, a task opened from the file tree shows its text
/// at the editor's whole width, with no panel beside it, and the whole
/// formatting toolbar stays in the window.
#[test]
fn a_task_opens_from_the_tree_at_full_width_with_no_panel_beside_it() {
    let Some(Opened {
        mut harness,
        _folders,
        ..
    }) = opened()
    else {
        return;
    };
    harness.set_size(egui::vec2(1100.0, 750.0));
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "tasks")
        .click();
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::TreeItem, "Read the manual")
        .click();
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Document, "Note text")
            .is_some()
    });
    // The plugin answers on its own thread; give it time to.
    for _ in 0..20 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        harness.step();
    }
    run(&mut harness);
    assert!(
        harness
            .query_by_role_and_label(Role::Group, "Properties")
            .is_none()
    );
    assert!(
        harness
            .query_by_role_and_label(Role::RadioGroup, "Status")
            .is_none()
    );

    let toolbar = harness.get_by_role_and_label(Role::Toolbar, "Formatting");
    let buttons: Vec<_> = toolbar.get_all_by_role(Role::Button).collect();
    assert!(buttons.len() > 10, "{}", buttons.len());
    for button in &buttons {
        let rect = button.rect();
        let label = button.accesskit_node().label();
        assert!(
            rect.left() >= 0.0 && rect.right() <= 1100.0 && rect.bottom() <= 750.0,
            "{label:?} at {rect:?} leaves the window"
        );
    }
    assert!(buttons.iter().any(|button| {
        button
            .accesskit_node()
            .label()
            .is_some_and(|label| label.starts_with("Find"))
    }));
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
        .get_by_role_and_label(Role::TextInput, "Add a task for today\u{2026}")
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
        .get_by_role_and_label(Role::TextInput, "Add a task for today\u{2026}");
    assert_eq!(field.value().unwrap_or_default(), "");
    assert!(add_button(&harness).accesskit_node().is_disabled());

    // Enter in the field adds the task too.
    field.focus();
    run(&mut harness);
    type_text(&mut harness, "Order seeds");
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |harness| {
        due_today(harness).contains(&"Order seeds".to_owned())
    });
    assert!(folder.join("tasks/Order seeds.md").exists());
}

/// The items of the open menu labeled `label`.
fn menu_items(harness: &Ui, label: &str) -> Vec<String> {
    labels(harness.get_by_label(label).get_all_by_role(Role::Button))
}

#[test]
fn a_task_rows_menu_opens_by_right_click_or_from_the_keyboard() {
    let Some(Opened {
        mut harness,
        _folders,
        ..
    }) = opened()
    else {
        return;
    };
    wait_until(&mut harness, |harness| {
        task_list(harness) == ["Water the beds, Jan 6, 2020"]
    });
    harness
        .get_by_role_and_label(Role::ListBox, "Overdue")
        .get_by_role_and_label(Role::ListBoxOption, "Water the beds, Jan 6, 2020")
        .click_secondary();
    run(&mut harness);

    assert_eq!(
        menu_items(&harness, "Actions for Water the beds"),
        [
            "Open",
            "Open in new tab",
            "Rename\u{2026}",
            "Mark as done",
            "Move to Trash"
        ]
    );
    assert!(harness.unnamed_widgets().is_empty());
    press(&mut harness, Key::Escape);
    assert!(
        harness
            .query_by_label("Actions for Water the beds")
            .is_none()
    );

    // The keyboard opens the same menu on the row it's on, with its first
    // item ready.
    assert_eq!(
        focused(&harness).as_deref(),
        Some("Water the beds, Jan 6, 2020")
    );
    press_with(&mut harness, Modifiers::SHIFT, Key::F10);
    assert_eq!(
        menu_items(&harness, "Actions for Water the beds"),
        [
            "Open",
            "Open in new tab",
            "Rename\u{2026}",
            "Mark as done",
            "Move to Trash"
        ]
    );
    assert_eq!(focused(&harness).as_deref(), Some("Open"));
    press(&mut harness, Key::Escape);
    press(&mut harness, Key::Menu);
    assert!(
        harness
            .query_by_label("Actions for Water the beds")
            .is_some()
    );
    press(&mut harness, Key::Escape);

    // A done task offers Mark as not done.
    harness
        .get_by_role_and_label(Role::ListBox, "Views")
        .get_by_role_and_label(Role::ListBoxOption, "All")
        .click();
    wait_until(&mut harness, |harness| task_list(harness).len() == 2);
    harness
        .get_by_role_and_label(Role::CheckBox, "Read the manual")
        .click();
    wait_until(&mut harness, |harness| {
        task_list(harness) == ["Water the beds, Jan 6, 2020", "Read the manual"]
            && harness
                .get_by_role_and_label(Role::CheckBox, "Read the manual")
                .accesskit_node()
                .toggled()
                == Some(egui::accesskit::Toggled::True)
    });
    harness
        .get_by_role_and_label(Role::ListBox, "Completed")
        .get_by_role_and_label(Role::ListBoxOption, "Read the manual")
        .click_secondary();
    run(&mut harness);
    assert_eq!(
        menu_items(&harness, "Actions for Read the manual"),
        [
            "Open",
            "Open in new tab",
            "Rename\u{2026}",
            "Mark as not done",
            "Move to Trash"
        ]
    );
}

#[test]
fn rename_shows_the_name_in_a_field_in_the_row_and_enter_saves_it() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    wait_until(&mut harness, |harness| {
        task_list(harness) == ["Water the beds, Jan 6, 2020"]
    });
    harness
        .get_by_role_and_label(Role::ListBox, "Overdue")
        .get_by_role_and_label(Role::ListBoxOption, "Water the beds, Jan 6, 2020")
        .click_secondary();
    run(&mut harness);
    harness
        .get_by_label("Actions for Water the beds")
        .get_by_role_and_label(Role::Button, "Rename\u{2026}")
        .click();

    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::TextInput, "Task name")
            .is_some_and(|field| field.is_focused())
    });
    let field = harness
        .get_by_role_and_label(Role::ListBox, "Overdue")
        .get_by_role_and_label(Role::TextInput, "Task name");
    assert_eq!(field.value().as_deref(), Some("Water the beds"));
    press_with(&mut harness, Modifiers::COMMAND, Key::A);
    type_text(&mut harness, "Water the roses");
    press(&mut harness, Key::Enter);

    wait_until(&mut harness, |_| {
        folder.join("tasks/Water the roses.md").exists()
    });
    assert!(!folder.join("tasks/Water the beds.md").exists());
    wait_until(&mut harness, |harness| {
        task_list(harness) == ["Water the roses, Jan 6, 2020"]
    });
    assert!(
        harness
            .query_by_role_and_label(Role::TextInput, "Task name")
            .is_none()
    );
}

#[test]
fn the_selected_task_shows_beside_the_list_and_each_field_writes_its_file() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    harness.set_size(egui::vec2(1200.0, 800.0));
    harness
        .get_by_role_and_label(Role::ListBox, "Views")
        .get_by_role_and_label(Role::ListBoxOption, "All")
        .click();
    wait_until(&mut harness, |harness| {
        task_list(harness).contains(&"Read the manual".to_owned())
    });
    harness.get_by_label("No task selected");

    harness
        .get_by_role_and_label(Role::ListBox, "No date")
        .get_by_role_and_label(Role::ListBoxOption, "Read the manual")
        .click();
    wait_until(&mut harness, |harness| {
        title_value(harness).as_deref() == Some("Read the manual")
    });
    assert!(harness.query_by_label("No task selected").is_none());
    assert!(harness.unnamed_widgets().is_empty());
    let shown = |harness: &Ui, picker: &str| {
        harness
            .get_by_role_and_label(Role::Button, picker)
            .accesskit_node()
            .value()
    };
    assert_eq!(shown(&harness, "Status").as_deref(), Some("To do"));
    assert_eq!(shown(&harness, "Priority").as_deref(), Some("High"));
    assert_eq!(shown(&harness, "Project").as_deref(), Some("No project"));
    assert_eq!(shown(&harness, "Due date").as_deref(), Some(""));
    let path = "tasks/Read the manual.md";
    let id = "task_01jbcdefghjkmnpqrstvwxyz0b";

    // Pick a menu's option.
    let pick = |harness: &mut Ui, picker: &str, option: &str| {
        harness.get_by_role_and_label(Role::Button, picker).click();
        run(harness);
        let options = labels(harness.get_all_by_role(Role::MenuItemRadio));
        assert!(options.contains(&option.to_owned()), "{options:?}");
        harness
            .get_by_role_and_label(Role::MenuItemRadio, option)
            .click();
        run(harness);
    };
    pick(&mut harness, "Status", "In progress");
    wait_until(&mut harness, |_| {
        file(&folder, path).contains("status: in-progress")
    });
    pick(&mut harness, "Priority", "Urgent");
    wait_until(&mut harness, |_| {
        file(&folder, path).contains("priority: urgent")
    });
    pick(&mut harness, "Project", "Garden");
    wait_until(&mut harness, |_| {
        file(&folder, path).contains("project: '[[Garden]]'")
    });
    wait_until(&mut harness, |harness| {
        shown(harness, "Project").as_deref() == Some("Garden")
    });

    // The date picker's shortcuts set the due date, and Clear takes it away.
    let tomorrow = harness.state().app().today().tomorrow().unwrap();
    harness
        .get_by_role_and_label(Role::Button, "Due date")
        .click();
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::Button, "Tomorrow")
        .click();
    wait_until(&mut harness, |_| {
        file(&folder, path).contains(&format!("due: {tomorrow}"))
    });
    wait_until(&mut harness, |harness| {
        shown(harness, "Due date").as_deref() == Some("Tomorrow")
    });
    harness
        .get_by_role_and_label(Role::Button, "Due date")
        .click();
    run(&mut harness);
    harness.get_by_role_and_label(Role::Button, "Clear").click();
    wait_until(&mut harness, |_| !file(&folder, path).contains("due:"));

    // The description is the body's text, and leaving it saves it.
    harness
        .get_by_role_and_label(Role::MultilineTextInput, "Description")
        .click();
    run(&mut harness);
    press_with(&mut harness, Modifiers::COMMAND, Key::A);
    type_text(&mut harness, "Chapter two first.");
    press(&mut harness, Key::Tab);
    wait_until(&mut harness, |_| {
        file(&folder, path).ends_with("---\nChapter two first.\n")
    });

    // Subtasks go in with Return, as a checklist after the description.
    harness
        .get_by_role_and_label(Role::TextInput, "Add subtask")
        .click();
    run(&mut harness);
    type_text(&mut harness, "Skim the index");
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |_| {
        file(&folder, path).contains("- [ ] Skim the index\n")
    });
    type_text(&mut harness, "Take notes");
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |_| {
        file(&folder, path)
            .ends_with("Chapter two first.\n\n- [ ] Skim the index\n- [ ] Take notes\n")
    });
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::ListBox, "Subtasks")
            .is_some_and(|list| {
                labels(list.get_all_by_role(Role::ListBoxOption))
                    == ["Skim the index", "Take notes"]
            })
    });
    harness
        .get_by_role_and_label(Role::ListBox, "Subtasks")
        .get_by_role_and_label(Role::CheckBox, "Skim the index")
        .click();
    wait_until(&mut harness, |_| {
        file(&folder, path).contains("- [x] Skim the index\n- [ ] Take notes\n")
    });

    // A new title renames the file, and the task keeps its stable ID.
    harness
        .get_by_role_and_label(Role::TextInput, "Title")
        .click();
    run(&mut harness);
    press_with(&mut harness, Modifiers::COMMAND, Key::A);
    type_text(&mut harness, "Read the guide");
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |_| {
        folder.join("tasks/Read the guide.md").exists()
    });
    assert!(!folder.join(path).exists());
    let renamed = file(&folder, "tasks/Read the guide.md");
    assert!(renamed.contains(&format!("id: {id}")), "{renamed}");
    assert!(renamed.contains("- [x] Skim the index"), "{renamed}");
    wait_until(&mut harness, |harness| {
        section(harness, "No date") == ["Read the guide"]
            && title_value(harness).as_deref() == Some("Read the guide")
    });
}

#[test]
fn return_on_a_task_edits_its_title_and_escape_puts_it_back() {
    let Some(Opened {
        mut harness,
        folder,
        _folders,
    }) = opened()
    else {
        return;
    };
    let row = harness.get_by_role_and_label(Role::ListBoxOption, "Water the beds, Jan 6, 2020");
    row.click();
    run(&mut harness);
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::TextInput, "Title")
            .is_some_and(|field| field.is_focused())
    });
    press_with(&mut harness, Modifiers::COMMAND, Key::A);
    type_text(&mut harness, "Something else");
    press(&mut harness, Key::Escape);
    run(&mut harness);
    assert_eq!(title_value(&harness).as_deref(), Some("Water the beds"));
    assert!(folder.join("tasks/Water the beds.md").exists());
}
