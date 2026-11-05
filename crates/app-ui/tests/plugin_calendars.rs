//! Calendar grids through AccessKit: a test plugin's grid in month, week
//! and day modes, switched by the plugin's own buttons; its entries laid
//! out on the days they cover and labeled with their title and time; and
//! the keyboard path through them, with the events they send by node ID.
//! The fake system keeps Toronto's time zone and US English.

mod support;

use egui::{Key, Modifiers, accesskit::Role};
use egui_kittest::kittest::Queryable;
use support::{
    kit::{
        Ui, focused, is_selected, labels, last_event, open_page, opened, press, press_with, run,
    },
    plugins,
};

fn grid(harness: &Ui) -> egui_kittest::Node<'_> {
    harness.get_by_role_and_label(Role::Grid, "Schedule")
}

/// Scroll the entry `label` into view, as a screen reader asks, so a
/// pointer can reach it below the window's fold.
fn scroll_to(harness: &mut Ui, label: &str) {
    harness.get_by_label(label).scroll_to_me();
    run(harness);
}

fn press_button(harness: &mut Ui, label: &str) {
    harness.get_by_role_and_label(Role::Button, label).click();
    run(harness);
}

/// The labels of the entries in the grid's cell `cell`.
fn entries_in(harness: &Ui, cell: &str) -> Vec<String> {
    labels(
        grid(harness)
            .get_by_role_and_label(Role::Cell, cell)
            .get_all_by_role(Role::Button),
    )
}

#[test]
fn a_month_grid_shows_six_weeks_from_sunday() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Calendar");

    press_button(harness, "Month");

    assert!(
        harness
            .query_by_role_and_label(Role::Heading, "October 2026")
            .is_some()
    );
    assert_eq!(
        labels(grid(harness).get_all_by_role(Role::ColumnHeader)),
        [
            "Sunday",
            "Monday",
            "Tuesday",
            "Wednesday",
            "Thursday",
            "Friday",
            "Saturday"
        ]
    );
    // The header row, then six weeks.
    assert_eq!(grid(harness).get_all_by_role(Role::Row).count(), 7);
    let days = labels(grid(harness).get_all_by_role(Role::Cell));
    assert_eq!(days.len(), 42);
    assert_eq!(days[0], "Sunday, September 27, 2026");
    assert_eq!(days[41], "Saturday, November 7, 2026");
    // An all-day entry fills each day it covers.
    for day in ["Monday, October 19, 2026", "Tuesday, October 20, 2026"] {
        assert_eq!(entries_in(harness, day), ["Retreat, all day"]);
    }
    assert_eq!(
        entries_in(harness, "Wednesday, October 21, 2026"),
        [
            "Retreat, all day",
            "Standup, 9:00 AM–9:30 AM",
            "Review, 9:15 AM–10:00 AM",
            "Deploy, 11:00 PM–12:00 AM",
        ]
    );
    assert_eq!(
        entries_in(harness, "Thursday, October 22, 2026"),
        ["Deploy, 12:00 AM–1:00 AM"]
    );
    assert_eq!(
        entries_in(harness, "Saturday, November 7, 2026"),
        ["Seed market, 10:00 AM–12:00 PM"]
    );
    assert!(harness.unnamed_widgets().is_empty());

    // The plugin's Next moves the grid a month on.
    press_button(harness, "Next");
    assert!(
        harness
            .query_by_role_and_label(Role::Heading, "November 2026")
            .is_some()
    );
    let days = labels(grid(harness).get_all_by_role(Role::Cell));
    assert_eq!(days[0], "Sunday, November 1, 2026");
}

#[test]
fn a_week_grid_runs_sunday_to_saturday_with_an_all_day_row_and_hour_rows() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Calendar");

    let headers = labels(grid(harness).get_all_by_role(Role::ColumnHeader));
    assert_eq!(headers.len(), 7);
    assert_eq!(headers[0], "Sunday, October 18, 2026");
    assert_eq!(headers[6], "Saturday, October 24, 2026");
    let rows = labels(grid(harness).get_all_by_role(Role::RowHeader));
    assert_eq!(rows.len(), 25);
    assert_eq!(rows[..3], ["All day", "12 AM", "1 AM"]);
    assert_eq!(rows[13], "12 PM");
    assert_eq!(rows[24], "11 PM");

    assert_eq!(
        entries_in(harness, "Wednesday, October 21, 2026"),
        ["Retreat, all day"]
    );
    assert_eq!(
        entries_in(harness, "Saturday, October 24, 2026"),
        ["First frost, all day"]
    );
    // Overlapping entries share their hour, and an entry past midnight
    // shows on both days.
    assert_eq!(
        entries_in(harness, "Wednesday, October 21, 2026, 9 AM"),
        ["Standup, 9:00 AM–9:30 AM", "Review, 9:15 AM–10:00 AM"]
    );
    assert_eq!(
        entries_in(harness, "Wednesday, October 21, 2026, 11 PM"),
        ["Deploy, 11:00 PM–12:00 AM"]
    );
    assert_eq!(
        entries_in(harness, "Thursday, October 22, 2026, 12 AM"),
        ["Deploy, 12:00 AM–1:00 AM"]
    );
    // Standup and Review overlap, so they sit side by side.
    let standup = harness.get_by_label("Standup, 9:00 AM–9:30 AM").rect();
    let review = harness.get_by_label("Review, 9:15 AM–10:00 AM").rect();
    assert!(standup.right() <= review.left(), "{standup:?} {review:?}");
    assert!(harness.unnamed_widgets().is_empty());

    press_button(harness, "Next");
    let headers = labels(grid(harness).get_all_by_role(Role::ColumnHeader));
    assert_eq!(headers[0], "Sunday, October 25, 2026");
    press_button(harness, "Today");
    let headers = labels(grid(harness).get_all_by_role(Role::ColumnHeader));
    assert_eq!(headers[0], "Sunday, October 18, 2026");
}

#[test]
fn a_day_grid_shows_one_day_with_an_all_day_row_and_hour_rows() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Calendar");

    press_button(harness, "Day");

    assert!(
        harness
            .query_by_role_and_label(Role::Heading, "Wednesday, October 21, 2026")
            .is_some()
    );
    assert_eq!(
        labels(grid(harness).get_all_by_role(Role::ColumnHeader)),
        ["Wednesday, October 21, 2026"]
    );
    assert_eq!(grid(harness).get_all_by_role(Role::RowHeader).count(), 25);
    assert_eq!(
        labels(grid(harness).get_all_by_role(Role::Button)),
        [
            "Retreat, all day",
            "Standup, 9:00 AM–9:30 AM",
            "Review, 9:15 AM–10:00 AM",
            "Deploy, 11:00 PM–12:00 AM",
        ]
    );

    press_button(harness, "Next");
    assert_eq!(
        labels(grid(harness).get_all_by_role(Role::Button)),
        ["Deploy, 12:00 AM–1:00 AM"]
    );
}

#[test]
fn entries_take_the_keyboard_and_send_select_and_open_events_by_node_id() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Calendar");
    let entry = |harness: &Ui, label: &str| -> bool {
        is_selected(&grid(harness).get_by_role_and_label(Role::Button, label))
    };

    scroll_to(harness, "Standup, 9:00 AM–9:30 AM");
    harness.get_by_label("Standup, 9:00 AM–9:30 AM").click();
    run(harness);
    assert_eq!(last_event(harness), "selected standup in schedule");
    assert!(entry(harness, "Standup, 9:00 AM–9:30 AM"));
    press(harness, Key::Enter);
    assert_eq!(last_event(harness), "activated standup in schedule");

    // The arrow keys go through every entry, day by day.
    press(harness, Key::Home);
    let mut order = vec![focused(harness).unwrap()];
    for _ in 0..9 {
        press(harness, Key::ArrowRight);
        order.push(focused(harness).unwrap());
    }
    assert_eq!(
        order,
        [
            "Retreat, all day",
            "Retreat, all day",
            "Retreat, all day",
            "Standup, 9:00 AM–9:30 AM",
            "Review, 9:15 AM–10:00 AM",
            "Deploy, 11:00 PM–12:00 AM",
            "Deploy, 12:00 AM–1:00 AM",
            "First frost, all day",
            // The last entry keeps the keyboard.
            "First frost, all day",
            "First frost, all day",
        ]
    );
    assert_eq!(last_event(harness), "selected frost in schedule");
    press(harness, Key::ArrowLeft);
    assert_eq!(
        focused(harness).as_deref(),
        Some("Deploy, 12:00 AM–1:00 AM")
    );
    assert_eq!(last_event(harness), "selected deploy in schedule");

    // The grid is one stop in the Tab order, on the entry with the
    // keyboard.
    press_with(harness, Modifiers::SHIFT, Key::Tab);
    assert_ne!(
        focused(harness).as_deref(),
        Some("Deploy, 12:00 AM–1:00 AM")
    );
    press(harness, Key::Tab);
    assert_eq!(
        focused(harness).as_deref(),
        Some("Deploy, 12:00 AM–1:00 AM")
    );

    scroll_to(harness, "Review, 9:15 AM–10:00 AM");
    let review = harness
        .get_by_label("Review, 9:15 AM–10:00 AM")
        .rect()
        .center();
    harness.input_mut().events.extend(double_click(review));
    harness.step();
    run(harness);
    assert_eq!(last_event(harness), "activated review in schedule");
    assert!(entry(harness, "Review, 9:15 AM–10:00 AM"));
}

/// A double-click at `pos`, both clicks in one frame: the harness steps a
/// quarter second per event, slower than a double-click.
fn double_click(pos: egui::Pos2) -> Vec<egui::Event> {
    let button = |pressed| egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    vec![
        egui::Event::PointerMoved(pos),
        button(true),
        button(false),
        button(true),
        button(false),
    ]
}
