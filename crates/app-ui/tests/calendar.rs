//! The Calendar plugin through AccessKit: the page's Previous, Today and
//! Next buttons and View choice, entries on the grid labeled with their
//! title and their time on this device's clock, and the entry panel's
//! title, type, properties and Open, which shows a task beside the Tasks
//! properties panel. The fake system keeps Toronto's time zone and US
//! English, and its clock reads 11:00 on Wednesday 21 October 2026.

mod support;

use egui::accesskit::Role;
use egui_kittest::kittest::Queryable;
use support::{
    FakePicker, FakeSystem, create_workspace, harness_with,
    kit::{Ui, labels, open_page, run},
    plugins, settle,
};

const CALENDAR: &str = include_str!("../../plugins/calendar/plugin.toml");
const TASKS: &str = include_str!("../../plugins/tasks/plugin.toml");

struct Opened {
    harness: Ui,
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace with a task at 9:00 and a three-day retreat, with the
/// Calendar and Tasks plugins installed and on and the Calendar page open.
/// `None` when the plugins aren't built.
fn opened() -> Option<Opened> {
    let calendar = plugins::built("calendar_plugin")?;
    let tasks = plugins::built("tasks_plugin")?;
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
        "tasks/Standup.md",
        "---\nid: task_01jbcdefghjkmnpqrstvwxyz0a\ntype: task\nstatus: todo\npriority: high\n\
         start: 2026-10-21T13:00:00Z\nend: 2026-10-21T13:30:00Z\n\
         created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\nNotes.\n",
    );
    write(
        "Retreat.md",
        "---\nid: project_01jbcdefghjkmnpqrstvwxyz0b\ntype: project\nstatus: active\n\
         start: 2026-10-19\nend: 2026-10-22\n\
         created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\nOverview.\n",
    );
    let plugin = |id: &str, manifest: &str, entry: &str, component: &[u8]| {
        let place = parent.path().join(format!("{id}-plugin"));
        let folder = plugins::folder(&place, manifest, entry, component);
        plugins::install_and_enable(app_data.path(), &folder, id);
    };
    plugin("tasks", TASKS, "tasks_plugin.wasm", &tasks);
    plugin("calendar", CALENDAR, "calendar_plugin.wasm", &calendar);
    let system = FakeSystem::new(app_core::Platform::current());
    system.set_now("2026-10-21T15:00:00Z");
    let mut harness = harness_with(app_data.path(), FakePicker::default(), system);
    settle(&mut harness);
    open_page(&mut harness, "Calendar");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Grid, "Calendar")
            .is_some()
    });
    Some(Opened {
        harness,
        _folders: (app_data, parent),
    })
}

/// Run frames, waiting in real time for the plugins' answers, until
/// `done` holds.
fn wait_until(harness: &mut Ui, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while !done(harness) {
        assert!(
            std::time::Instant::now() < deadline,
            "the screen never got there; the active tab is {:?}",
            harness
                .state()
                .app()
                .active_tab()
                .map(|tab| tab.title().to_owned()),
        );
        std::thread::sleep(std::time::Duration::from_millis(20));
        harness.step();
    }
    run(harness);
}

fn press_button(harness: &mut Ui, label: &str) {
    harness.get_by_role_and_label(Role::Button, label).click();
    run(harness);
}

/// The labels of the entries in the grid's cell `cell`.
fn entries_in(harness: &Ui, cell: &str) -> Vec<String> {
    labels(
        harness
            .get_by_role_and_label(Role::Grid, "Calendar")
            .get_by_role_and_label(Role::Cell, cell)
            .get_all_by_role(Role::Button),
    )
}

#[test]
fn the_page_labels_its_buttons_its_view_choice_and_its_entries() {
    let Some(Opened {
        mut harness,
        _folders,
    }) = opened()
    else {
        return;
    };

    for button in ["Previous month", "Today", "Next month"] {
        harness.get_by_role_and_label(Role::Button, button);
    }
    let view = harness.get_by_role_and_label(Role::RadioGroup, "View");
    assert_eq!(
        labels(view.get_all_by_role(Role::RadioButton)),
        ["Month", "Week", "Day"]
    );
    assert!(
        harness
            .query_by_role_and_label(Role::Heading, "October 2026")
            .is_some()
    );
    assert_eq!(
        entries_in(&harness, "Wednesday, October 21, 2026"),
        ["Retreat, all day", "Standup, 9:00 AM–9:30 AM"]
    );
    assert!(harness.unnamed_widgets().is_empty());

    harness
        .get_by_role_and_label(Role::RadioButton, "Week")
        .click();
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Button, "Next week")
            .is_some()
    });
    harness.get_by_role_and_label(Role::Button, "Previous week");
    let headers = labels(
        harness
            .get_by_role_and_label(Role::Grid, "Calendar")
            .get_all_by_role(Role::ColumnHeader),
    );
    assert_eq!(
        headers.first().map(String::as_str),
        Some("Sunday, October 18, 2026")
    );

    press_button(&mut harness, "Next week");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Grid, "Calendar")
            .is_some_and(|grid| {
                labels(grid.get_all_by_role(Role::ColumnHeader))
                    .first()
                    .is_some_and(|first| first == "Sunday, October 25, 2026")
            })
    });
}

#[test]
fn an_entry_shows_its_panel_and_open_shows_the_task_beside_the_tasks_panel() {
    let Some(Opened {
        mut harness,
        _folders,
    }) = opened()
    else {
        return;
    };
    let standup = "Standup, 9:00 AM–9:30 AM";
    harness.get_by_label(standup).scroll_to_me();
    run(&mut harness);

    harness.get_by_label(standup).click();
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Group, "Selected entry")
            .is_some()
    });

    let panel = harness.get_by_role_and_label(Role::Group, "Selected entry");
    panel.get_by_label("Standup");
    panel.get_by_label("Type: task");
    let table = panel.get_by_role_and_label(Role::Table, "Properties");
    let rows = labels(table.get_all_by_role(Role::Row));
    assert_eq!(rows, ["end", "priority", "start", "status"]);
    panel.get_by_role_and_label(Role::Button, "Close");

    press_button(&mut harness, "Open");
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Group, "Properties")
            .is_some_and(|panel| panel.query_by_role_and_label(Role::Group, "Task").is_some())
    });
    assert_eq!(
        harness
            .state()
            .app()
            .active_tab()
            .map(|tab| tab.title().to_owned())
            .as_deref(),
        Some("Standup")
    );
    harness
        .get_by_role_and_label(Role::Group, "Properties")
        .get_by_role_and_label(Role::RadioGroup, "Status");
}
