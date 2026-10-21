//! Boards and reorderable lists through AccessKit: a test plugin's board
//! of labeled columns and cards, the keyboard path through them, card
//! menus, and dragging cards and list items, which sends one move event
//! and then shows only what the plugin draws.

mod support;

use egui::{Key, Modifiers, Pos2, accesskit::Role, vec2};
use egui_kittest::kittest::{NodeT, Queryable};
use support::{
    kit::{Ui, focused, is_selected, labels, last_event, open_page, opened, press, press_with},
    plugins,
};

fn board(harness: &Ui) -> egui_kittest::Node<'_> {
    harness.get_by_role_and_label(Role::Group, "Work")
}

/// The labels of the cards in the column `column`, top to bottom.
fn cards(harness: &Ui, column: &str) -> Vec<String> {
    labels(
        board(harness)
            .get_by_role_and_label(Role::List, column)
            .get_all_by_role(Role::ListItem),
    )
}

fn card<'a>(harness: &'a Ui, label: &'a str) -> egui_kittest::Node<'a> {
    board(harness).get_by_role_and_label(Role::ListItem, label)
}

/// The column that holds the card with keyboard focus.
fn focused_column(harness: &Ui) -> Option<String> {
    let card = focused(harness)?;
    board(harness)
        .get_all_by_role(Role::List)
        .find(|column| column.query_by_label(&card).is_some())
        .and_then(|column| column.accesskit_node().label())
}

#[test]
fn a_board_draws_labeled_columns_of_labeled_cards_that_the_keyboard_reaches() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Board");

    assert_eq!(
        labels(board(harness).get_all_by_role(Role::List)),
        ["To do", "Doing", "Done"]
    );
    assert_eq!(cards(harness, "To do"), ["Write report", "Fix bug"]);
    assert_eq!(cards(harness, "Doing"), ["Review notes"]);
    assert_eq!(cards(harness, "Done"), ["Plan sprint"]);
    assert_eq!(
        card(harness, "Write report").accesskit_node().description(),
        Some("Due Friday".into())
    );
    assert!(harness.unnamed_widgets().is_empty());

    // A screen reader moves the keyboard to the first card, and the arrow
    // keys go from there to every card in every column.
    card(harness, "Write report").focus();
    harness.run();
    let mut visited = vec![(focused_column(harness), focused(harness))];
    for key in [
        Key::ArrowDown,
        Key::ArrowRight,
        Key::ArrowRight,
        Key::ArrowLeft,
        Key::ArrowLeft,
        Key::ArrowUp,
    ] {
        press(harness, key);
        visited.push((focused_column(harness), focused(harness)));
    }
    let visited: Vec<String> = visited
        .into_iter()
        .map(|(column, card)| format!("{}: {}", column.unwrap(), card.unwrap()))
        .collect();
    assert_eq!(
        visited,
        [
            "To do: Write report",
            "To do: Fix bug",
            "Doing: Review notes",
            "Done: Plan sprint",
            "Doing: Review notes",
            "To do: Write report",
            "To do: Write report",
        ]
    );
    // Selection follows the keyboard, and Enter opens the card.
    assert_eq!(last_event(harness), "selected write-report in work");
    assert!(is_selected(&card(harness, "Write report")));
    press(harness, Key::Enter);
    assert_eq!(last_event(harness), "activated write-report in work");

    // The board is one stop in the Tab order, on its selected card.
    press(harness, Key::Tab);
    assert_eq!(focused(harness).as_deref(), Some("Water seedlings"));
    press_with(harness, Modifiers::SHIFT, Key::Tab);
    assert_eq!(focused(harness).as_deref(), Some("Write report"));
}

#[test]
fn a_cards_menu_opens_from_the_keyboard_and_moves_the_card_to_another_column() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Board");
    card(harness, "Write report").focus();
    harness.run();

    press_with(harness, Modifiers::SHIFT, Key::F10);

    let menu = harness.get_by_label("Move Write report");
    assert_eq!(
        labels(menu.get_all_by_role(Role::Button)),
        ["To do", "Doing", "Done"]
    );
    assert!(
        menu.get_by_role_and_label(Role::Button, "To do")
            .accesskit_node()
            .is_disabled()
    );
    assert_eq!(focused(harness).as_deref(), Some("Doing"));
    press(harness, Key::Enter);

    assert_eq!(last_event(harness), "chose write-report-to-doing");
    assert!(harness.query_by_label("Move Write report").is_none());
    assert_eq!(cards(harness, "To do"), ["Fix bug"]);
    assert_eq!(cards(harness, "Doing"), ["Review notes", "Write report"]);
    // The card keeps the keyboard in its new column.
    assert_eq!(focused_column(harness).as_deref(), Some("Doing"));
    assert_eq!(focused(harness).as_deref(), Some("Write report"));
}

/// Drag from `from` to `to` in small steps, as a pointer does, and drop
/// there unless `drop` is false. Returns once the pointer is at `to`.
fn drag(harness: &mut Ui, from: Pos2, to: Pos2, drop: bool) {
    harness.hover_at(from);
    harness.run();
    harness.drag_at(from);
    harness.run();
    for step in 1..=4 {
        harness.hover_at(from + (to - from) * (step as f32 / 4.0));
        harness.run();
    }
    if drop {
        harness.drop_at(to);
        harness.run();
    }
}

/// Where to drop to land just below the node `label`, or just above it.
fn below(harness: &Ui, label: &str) -> Pos2 {
    let rect = harness.get_by_label(label).rect();
    rect.center_bottom() - vec2(0.0, 4.0)
}

fn above(harness: &Ui, label: &str) -> Pos2 {
    let rect = harness.get_by_label(label).rect();
    rect.center_top() + vec2(0.0, 4.0)
}

#[test]
fn dragging_a_card_shows_where_it_lands_and_sends_one_move() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Board");

    // Into another column, below its card.
    let from = card(harness, "Write report").rect().center();
    let to = below(harness, "Review notes");
    drag(harness, from, to, false);
    assert!(
        harness
            .query_by_role_and_label(Role::Status, "Write report goes to Doing, position 2")
            .is_some()
    );
    harness.drop_at(to);
    harness.run();
    assert_eq!(
        last_event(harness),
        "moved write-report in work to doing at 1"
    );
    assert!(harness.query_by_role(Role::Status).is_none());
    assert_eq!(cards(harness, "To do"), ["Fix bug"]);
    assert_eq!(cards(harness, "Doing"), ["Review notes", "Write report"]);

    // Within its column, above the card that was first.
    let from = card(harness, "Write report").rect().center();
    let to = above(harness, "Review notes");
    drag(harness, from, to, true);
    assert_eq!(
        last_event(harness),
        "moved write-report in work to doing at 0"
    );
    assert_eq!(cards(harness, "Doing"), ["Write report", "Review notes"]);
}

#[test]
fn a_move_the_plugin_refuses_puts_the_card_back() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Board");

    let from = card(harness, "Fix bug").rect().center();
    let to = above(harness, "Plan sprint");
    drag(harness, from, to, true);

    // The plugin heard of the drop, refused it, and drew Fix bug where it
    // was.
    assert_eq!(last_event(harness), "moved fix-bug in work to done at 0");
    assert_eq!(cards(harness, "To do"), ["Write report", "Fix bug"]);
    assert_eq!(cards(harness, "Done"), ["Plan sprint"]);
}

#[test]
fn dropping_a_card_where_it_was_sends_nothing() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Board");

    let from = card(harness, "Write report").rect().center();
    drag(harness, from, from + vec2(0.0, 8.0), true);

    assert_eq!(last_event(harness), "none");
    assert_eq!(cards(harness, "To do"), ["Write report", "Fix bug"]);
}

#[test]
fn dragging_a_list_item_to_a_new_position_shows_where_it_lands_and_sends_one_move() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Board");
    let queue = |harness: &Ui| {
        labels(
            harness
                .get_by_role_and_label(Role::ListBox, "Queue")
                .get_all_by_role(Role::ListBoxOption),
        )
    };
    assert_eq!(
        queue(harness),
        ["Water seedlings", "Turn compost", "Sow beans"]
    );

    let from = harness.get_by_label("Water seedlings").rect().center();
    let to = below(harness, "Sow beans");
    drag(harness, from, to, false);
    assert!(
        harness
            .query_by_role_and_label(Role::Status, "Water seedlings goes to position 3")
            .is_some()
    );
    harness.drop_at(to);
    harness.run();

    assert_eq!(last_event(harness), "moved first in queue to queue at 2");
    assert_eq!(
        queue(harness),
        ["Turn compost", "Sow beans", "Water seedlings"]
    );
}
