//! The Calendar plugin through AccessKit: the page's Previous, Today and
//! Next buttons and View choice, today's cell marked as the current date,
//! entries on the grid labeled with their title and their time on this
//! device's clock, the entry panel's
//! title, type, properties and Open, which opens the task in the note
//! editor, and the sidebar's calendars, whose checkboxes hide their
//! entries. The fake system keeps Toronto's time zone and US
//! English, and its clock reads 11:00 on Wednesday 21 October 2026.

use crate::support;
use egui::accesskit::{AriaCurrent, Role};
use egui_kittest::kittest::{NodeT, Queryable};
use support::{
    FakePicker, FakeSystem, create_workspace, harness_with,
    kit::{Ui, labels, open_page, run},
    plugins, settle,
};

const CALENDAR: &str = include_str!("../../../plugins/calendar/plugin.toml");
const TASKS: &str = include_str!("../../../plugins/tasks/plugin.toml");

struct Opened {
    harness: Ui,
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace with a task at 9:00, a three-day retreat, and the calendar
/// Work with a dentist's appointment at 10:00 on Thursday, with the
/// Calendar and Tasks plugins installed and on and the Calendar page open.
/// `None` when the plugins aren't built.
fn opened() -> Option<Opened> {
    let calendar = plugins::built("calendar_plugin")?;
    let tasks = plugins::built("tasks_plugin")?;
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
    write(
        "Calendars/Work.md",
        "---\nid: calendar_01jbcdefghjkmnpqrstvwxyz0c\ntype: calendar\ncolor: green\n---\n",
    );
    write(
        "Events/Dentist.md",
        "---\nid: event_01jbcdefghjkmnpqrstvwxyz0d\ntype: event\n\
         calendar: calendar_01jbcdefghjkmnpqrstvwxyz0c\nstart: 2026-10-22T14:00:00Z\n\
         end: 2026-10-22T15:00:00Z\n---\n",
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
            .query_all_by_role(Role::Button),
    )
}

/// The labels of the grid's nodes with `role` that are the current date.
fn today_in(harness: &Ui, role: Role) -> Vec<String> {
    labels(
        harness
            .get_by_role_and_label(Role::Grid, "Calendar")
            .get_all_by_role(role)
            .filter(|node| node.accesskit_node().aria_current() == Some(AriaCurrent::Date)),
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
    assert_eq!(
        today_in(&harness, Role::Cell),
        ["Wednesday, October 21, 2026"]
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
    assert_eq!(
        today_in(&harness, Role::ColumnHeader),
        ["Wednesday, October 21, 2026"]
    );
    assert!(today_in(&harness, Role::Cell).is_empty());

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
fn an_entry_shows_its_panel_and_open_opens_the_task_in_the_note_editor() {
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
            .query_by_role_and_label(Role::Document, "Note text")
            .is_some()
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
}

fn checked(harness: &Ui, label: &str) -> bool {
    harness
        .get_by_role_and_label(Role::CheckBox, label)
        .accesskit_node()
        .toggled()
        == Some(egui::accesskit::Toggled::True)
}

#[test]
fn the_sidebar_lists_calendars_whose_checkboxes_hide_their_entries() {
    let Some(Opened {
        mut harness,
        _folders,
    }) = opened()
    else {
        return;
    };
    let thursday = "Thursday, October 22, 2026";
    let dentist = "Dentist, 10:00 AM–11:00 AM";

    let calendars = harness.get_by_role_and_label(Role::ListBox, "Calendars");
    assert_eq!(
        labels(calendars.get_all_by_role(Role::ListBoxOption)),
        ["Work"]
    );
    let other = harness.get_by_role_and_label(Role::ListBox, "Other");
    assert_eq!(
        labels(other.get_all_by_role(Role::ListBoxOption)),
        ["Projects", "Tasks"]
    );
    for calendar in ["Work", "Projects", "Tasks"] {
        assert!(checked(&harness, calendar), "{calendar}");
    }
    harness.get_by_role_and_label(Role::Button, "New calendar");
    assert_eq!(entries_in(&harness, thursday), [dentist]);
    assert!(harness.unnamed_widgets().is_empty());

    harness
        .get_by_role_and_label(Role::CheckBox, "Work")
        .click();
    wait_until(&mut harness, |harness| {
        !checked(harness, "Work") && entries_in(harness, thursday).is_empty()
    });
    let settings = _folders
        .1
        .path()
        .join("Garden/Calendars/Calendar settings.md");
    let written = std::fs::read_to_string(settings).expect("the settings file holds it");
    assert!(
        written.contains("hidden_calendars:\n- calendar_01jbcdefghjkmnpqrstvwxyz0c\n"),
        "{written}"
    );
    // Hiding the built-in Tasks takes the task off the grid too.
    harness
        .get_by_role_and_label(Role::CheckBox, "Tasks")
        .click();
    wait_until(&mut harness, |harness| {
        !checked(harness, "Tasks")
            && entries_in(harness, "Wednesday, October 21, 2026") == ["Retreat, all day"]
    });

    harness
        .get_by_role_and_label(Role::CheckBox, "Work")
        .click();
    wait_until(&mut harness, |harness| {
        checked(harness, "Work") && entries_in(harness, thursday) == [dentist]
    });
}
