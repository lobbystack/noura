//! The Calendar plugin through AccessKit: the page's Previous, Today and
//! Next buttons and View choice, today's cell marked as the current date,
//! entries on the grid labeled with their title and their time on this
//! device's clock, the popover a click on an entry opens beside it, which
//! Escape and a click outside close, where an event's title, date, time
//! and calendar change in place and a meeting's Open opens it, and the
//! sidebar's calendars, whose checkboxes hide their entries. The fake
//! system keeps Toronto's time zone and US English, and its clock reads
//! 11:00 on Wednesday 21 October 2026.

use crate::support;
use egui::{
    Event, Key, Modifiers,
    accesskit::{AriaCurrent, Role},
};
use egui_kittest::kittest::{NodeT, Queryable};
use support::{
    FakePicker, FakeSystem, create_workspace, harness_with,
    kit::{Ui, labels, open_page, press, press_with, run},
    plugins, settle,
};

const CALENDAR: &str = include_str!("../../../plugins/calendar/plugin.toml");
const TASKS: &str = include_str!("../../../plugins/tasks/plugin.toml");

struct Opened {
    harness: Ui,
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

const HOME: &str = "calendar_01jbcdefghjkmnpqrstvwxyz0e";

/// A workspace with a task at 9:00, a three-day retreat, the calendars
/// Home and Work with a dentist's appointment in Work at 10:00 on
/// Thursday, and a meeting with Ada and Grace at 10:00 on Friday, with the
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
        "Calendars/Home.md",
        &format!("---\nid: {HOME}\ntype: calendar\ncolor: blue\n---\n"),
    );
    write(
        "Meetings/Sync.md",
        "---\nid: transcription_01jbcdefghjkmnpqrstvwxyz0f\ntype: transcription\n\
         start: 2026-10-23T14:00:00Z\nend: 2026-10-23T14:30:00Z\nattendees:\n- Ada\n- Grace\n---\n",
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
    // The page's title is the month it shows, said once; the tab and the
    // rail say Calendar.
    let headings = labels(harness.query_all(egui_kittest::kittest::By::new().role(Role::Heading)));
    assert_eq!(
        headings
            .iter()
            .filter(|heading| *heading == "October 2026")
            .count(),
        1,
        "{headings:?}"
    );
    assert!(
        !headings.iter().any(|heading| heading == "Calendar"),
        "{headings:?}"
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
fn the_period_title_gives_way_to_the_buttons_in_a_narrow_window() {
    let Some(Opened {
        mut harness,
        _folders,
    }) = opened()
    else {
        return;
    };
    let title = |harness: &Ui| {
        harness
            .get_by_role_and_label(Role::Heading, "October 2026")
            .get_by_role(Role::Label)
            .rect()
    };
    let new_event = |harness: &Ui| {
        harness
            .get_by_role_and_label(Role::Button, "New event")
            .rect()
    };
    assert!(title(&harness).right() < new_event(&harness).left());

    harness.set_size(egui::vec2(820.0, 700.0));
    run(&mut harness);
    assert!(
        title(&harness).right() < new_event(&harness).left(),
        "{:?} before {:?}",
        title(&harness),
        new_event(&harness)
    );
}

/// The popover named `label`, if it shows.
fn popover<'a>(harness: &'a Ui, label: &'a str) -> Option<egui_kittest::Node<'a>> {
    harness.query_by_role_and_label(Role::Dialog, label)
}

/// Click the entry labeled `label` on the grid, on its first day.
fn click_entry(harness: &mut Ui, label: &str) {
    harness
        .query_all_by_label(label)
        .next()
        .expect("the entry shows")
        .scroll_to_me();
    run(harness);
    harness
        .query_all_by_label(label)
        .next()
        .expect("the entry shows")
        .click();
    run(harness);
}

/// Click the empty header before New event, outside the popovers.
fn click_outside(harness: &mut Ui) {
    let button = harness
        .get_by_role_and_label(Role::Button, "New event")
        .rect();
    let at = button.left_center() - egui::vec2(80.0, 0.0);
    for pressed in [true, false] {
        harness.event(Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Modifiers::NONE,
        });
    }
    run(harness);
}

fn type_text(harness: &mut Ui, text: &str) {
    harness.event(Event::Text(text.to_owned()));
    run(harness);
}

#[test]
fn a_click_opens_an_entrys_popover_which_escape_and_a_click_outside_close() {
    let Some(Opened {
        mut harness,
        _folders,
    }) = opened()
    else {
        return;
    };
    let standup = "Standup, 9:00 AM–9:30 AM";
    assert!(popover(&harness, "Standup").is_none());

    click_entry(&mut harness, standup);
    wait_until(&mut harness, |harness| {
        popover(harness, "Standup").is_some()
    });
    // It says what the entry is in words, with no table of properties.
    let details = popover(&harness, "Standup").unwrap();
    details.get_by_label("Task");
    details.get_by_label("Today, 09:00 to 09:30");
    details.get_by_label("To do");
    details.get_by_role_and_label(Role::Button, "Open");
    assert!(details.query_by_role(Role::Table).is_none());
    // It floats beside the entry, which stays in sight: under it in this
    // narrow window.
    let entry = harness.get_by_label(standup).rect();
    let shown = popover(&harness, "Standup")
        .unwrap()
        .get_by_label("Task")
        .rect();
    assert!(shown.top() > entry.bottom(), "{shown:?} beside {entry:?}");
    assert!(harness.unnamed_widgets().is_empty());

    press(&mut harness, Key::Escape);
    wait_until(&mut harness, |harness| {
        popover(harness, "Standup").is_none()
    });

    click_entry(&mut harness, standup);
    wait_until(&mut harness, |harness| {
        popover(harness, "Standup").is_some()
    });
    click_outside(&mut harness);
    wait_until(&mut harness, |harness| {
        popover(harness, "Standup").is_none()
    });

    // A click on another entry closes this popover and opens that one.
    click_entry(&mut harness, standup);
    wait_until(&mut harness, |harness| {
        popover(harness, "Standup").is_some()
    });
    click_entry(&mut harness, "Retreat, all day");
    wait_until(&mut harness, |harness| {
        popover(harness, "Standup").is_none() && popover(harness, "Retreat").is_some()
    });
}

#[test]
fn an_events_popover_changes_its_title_date_time_and_calendar_in_place() {
    let Some(Opened {
        mut harness,
        _folders,
    }) = opened()
    else {
        return;
    };
    let file =
        |path: &str| std::fs::read_to_string(_folders.1.path().join("Garden").join(path)).ok();
    let shown = |harness: &Ui, picker: &str| {
        harness
            .get_by_role_and_label(Role::Button, picker)
            .accesskit_node()
            .value()
    };
    click_entry(&mut harness, "Dentist, 10:00 AM–11:00 AM");
    wait_until(&mut harness, |harness| {
        popover(harness, "Dentist").is_some()
    });
    assert_eq!(shown(&harness, "Calendar").as_deref(), Some("Work"));
    assert_eq!(shown(&harness, "Starts").as_deref(), Some("10:00"));
    assert_eq!(shown(&harness, "Ends").as_deref(), Some("11:00"));

    // Return sends the title, which renames the file.
    harness
        .get_by_role_and_label(Role::TextInput, "Title")
        .click();
    run(&mut harness);
    press_with(&mut harness, Modifiers::COMMAND, Key::A);
    type_text(&mut harness, "Dentist visit");
    press(&mut harness, Key::Enter);
    wait_until(&mut harness, |_| file("Events/Dentist visit.md").is_some());

    // The date picker moves it to Friday at the same time.
    harness.get_by_role_and_label(Role::Button, "Date").click();
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::Button, "Friday, October 23, 2026")
        .click();
    wait_until(&mut harness, |_| {
        file("Events/Dentist visit.md")
            .is_some_and(|text| text.contains("start: 2026-10-23T10:00:00-04:00\n"))
    });

    // The start time keeps its hour long.
    wait_until(&mut harness, |harness| {
        popover(harness, "Dentist visit").is_some()
    });
    harness
        .get_by_role_and_label(Role::Button, "Starts")
        .click();
    run(&mut harness);
    // The menu of times scrolls to the chosen one, so a later time needs
    // scrolling to.
    harness
        .get_by_role_and_label(Role::MenuItemRadio, "13:30")
        .scroll_to_me();
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::MenuItemRadio, "13:30")
        .click();
    wait_until(&mut harness, |_| {
        file("Events/Dentist visit.md").is_some_and(|text| {
            text.contains("start: 2026-10-23T13:30:00-04:00\nend: 2026-10-23T14:30:00-04:00\n")
        })
    });

    // The calendar's menu moves it to Home.
    harness
        .get_by_role_and_label(Role::Button, "Calendar")
        .click();
    run(&mut harness);
    harness
        .get_by_role_and_label(Role::MenuItemRadio, "Home")
        .click();
    wait_until(&mut harness, |_| {
        file("Events/Dentist visit.md")
            .is_some_and(|text| text.contains(&format!("calendar: {HOME}\n")))
    });
    wait_until(&mut harness, |harness| {
        harness
            .query_by_role_and_label(Role::Button, "Calendar")
            .and_then(|picker| picker.accesskit_node().value())
            .as_deref()
            == Some("Home")
    });

    // A click outside sends the title you typed before it closes the
    // popover.
    harness
        .get_by_role_and_label(Role::TextInput, "Title")
        .click();
    run(&mut harness);
    press_with(&mut harness, Modifiers::COMMAND, Key::A);
    type_text(&mut harness, "Checkup");
    click_outside(&mut harness);
    wait_until(&mut harness, |harness| {
        file("Events/Checkup.md").is_some() && popover(harness, "Checkup").is_none()
    });
}

#[test]
fn a_meetings_popover_says_who_comes_and_open_opens_it() {
    let Some(Opened {
        mut harness,
        _folders,
    }) = opened()
    else {
        return;
    };
    click_entry(&mut harness, "Sync, 10:00 AM–10:30 AM");
    wait_until(&mut harness, |harness| popover(harness, "Sync").is_some());
    let details = popover(&harness, "Sync").unwrap();
    details.get_by_label("Meeting");
    details.get_by_label("Friday, 10:00 to 10:30");
    details.get_by_label("With Ada and Grace");

    details.get_by_role_and_label(Role::Button, "Open").click();
    wait_until(&mut harness, |harness| {
        harness
            .state()
            .app()
            .active_tab()
            .is_some_and(|tab| tab.title() == "Sync")
    });
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
        ["Home", "Work"]
    );
    let other = harness.get_by_role_and_label(Role::ListBox, "Other");
    assert_eq!(
        labels(other.get_all_by_role(Role::ListBoxOption)),
        ["Meetings", "Projects", "Tasks"]
    );
    for calendar in ["Home", "Work", "Meetings", "Projects", "Tasks"] {
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
