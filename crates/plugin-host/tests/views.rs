//! Seam 3: UI as data. A view is a flat node list linked by index, which
//! the host checks before anything draws it, and events go back to the
//! plugin by node ID: presses, selections, header presses, menu choices,
//! and input changes and form submits with typed values. A virtual list's
//! rows come from the plugin as Noura asks for them.

mod support;

use plugin_host::{
    CalendarMode, Check, ColumnPress, Event, Field, Item, Move, Naming, PluginError, RunningPlugin,
    Services, Submission, Target, Value, View, ViewKind, ViewNode, VirtualRow, check_rows,
};
use support::plugins;

const VIEWS: [&str; 1] = ["name = \"views\""];

fn leaf(id: &str, kind: ViewKind) -> ViewNode {
    ViewNode {
        id: id.into(),
        kind,
        children: Vec::new(),
    }
}

#[test]
fn a_flat_node_list_becomes_a_tree_of_panels_text_and_buttons() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

    let view = View::from_nodes(instance.render("probe").unwrap()).expect("the view is valid");

    assert_eq!(
        view.root(),
        &ViewNode {
            id: "root".into(),
            kind: ViewKind::Panel {
                label: Some("Probe".into())
            },
            children: vec![
                leaf("line", ViewKind::Text("Probe is running.".into())),
                leaf(
                    "press",
                    ViewKind::Button {
                        label: "Press".into()
                    }
                ),
            ],
        }
    );
}

#[test]
fn presses_reach_the_plugin_by_node_id() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

    instance
        .handle_event("probe", &Event::Pressed("press".into()))
        .expect("the plugin takes the event");

    let nodes = instance.render("events").unwrap();
    assert_eq!(plugins::texts(&nodes), ["probe pressed press"]);
}

#[test]
fn a_node_list_that_breaks_the_rules_is_refused_with_a_reason() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

    for (case, reason) in [
        ("out-of-range", "child 5"),
        ("cycle", "cycle"),
        ("unlabeled", "label"),
        ("duplicate-id", "same"),
        ("children-of-text", "holds no other nodes"),
        ("empty", "no nodes"),
    ] {
        let nodes = instance.render(&format!("nodes:{case}")).unwrap();
        let error = View::from_nodes(nodes).expect_err(case);
        assert!(error.to_string().contains(reason), "{case}: {error}");
    }
}

#[test]
fn malformed_lists_tables_forms_and_menus_are_refused_with_a_reason() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

    for (case, reason) in [
        (
            "row-cells",
            "The row basil has 3 cells, but its table has 2 columns.",
        ),
        ("unlabeled-input", "The input name has no label."),
        ("empty-menu", "The menu more-menu has no items."),
        ("item-outside-list", "The item north isn't in a list."),
        (
            "panel-in-list",
            "The list beds holds inner, which isn't an item.",
        ),
        ("two-menus", "The item north holds more than one menu."),
        (
            "menu-in-panel",
            "The menu loose isn't on an item, a row, a card or a button.",
        ),
        (
            "bad-date",
            "The input sown holds 2026-02-30, which isn't a date in the form YYYY-MM-DD.",
        ),
        (
            "bad-date-time",
            "The input at holds 2026-10-21 09:30, which isn't an RFC 3339 date and time \
             with an offset.",
        ),
        (
            "unknown-choice",
            "The input sun chooses moon, which isn't one of its options.",
        ),
        (
            "choices-on-text",
            "Only a choice input has options, and the input name isn't one.",
        ),
        ("form-in-form", "The form inner sits inside another form."),
        ("no-columns", "The table plants has no columns."),
        ("blank-row", "The row gap has no cell with text."),
        (
            "long-virtual-list",
            "The virtual list seeds holds 1000001 rows; a virtual list holds at most 1000000.",
        ),
    ] {
        let nodes = instance.render(&format!("nodes:{case}")).unwrap();
        let error = View::from_nodes(nodes).expect_err(case);
        assert_eq!(error.to_string(), reason, "{case}");
    }
}

#[test]
fn malformed_boards_are_refused_with_a_reason() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

    for (case, reason) in [
        (
            "card-outside-column",
            "The card plan isn't in a board column.",
        ),
        (
            "column-outside-board",
            "The board column todo isn't in a board.",
        ),
        (
            "item-in-board",
            "The board work holds north, which isn't a board column.",
        ),
        (
            "panel-in-column",
            "The board column todo holds inner, which isn't a card.",
        ),
        ("unlabeled-column", "The board column todo has no label."),
        ("unlabeled-card", "The card plan has no label."),
        ("board-without-columns", "The board work has no columns."),
        (
            "card-with-two-menus",
            "The card plan holds more than one menu.",
        ),
    ] {
        let nodes = instance.render(&format!("nodes:{case}")).unwrap();
        let error = View::from_nodes(nodes).expect_err(case);
        assert_eq!(error.to_string(), reason, "{case}");
    }
}

/// The node with `id` in the tree under `node`.
fn find<'a>(node: &'a ViewNode, id: &str) -> Option<&'a ViewNode> {
    if node.id == id {
        return Some(node);
    }
    node.children.iter().find_map(|child| find(child, id))
}

fn kinds(nodes: &[ViewNode]) -> Vec<(&str, &ViewKind)> {
    nodes
        .iter()
        .map(|node| (node.id.as_str(), &node.kind))
        .collect()
}

fn menu_item(label: &str, enabled: bool) -> ViewKind {
    ViewKind::MenuItem {
        label: label.into(),
        enabled,
    }
}

#[test]
fn lists_with_item_menus_and_menu_buttons_become_a_tree() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&kit, &VIEWS, None);

    let view = View::from_nodes(instance.render("lists").unwrap()).expect("the view is valid");

    let beds = find(view.root(), "beds").unwrap();
    assert_eq!(
        beds.kind,
        ViewKind::List {
            label: "Beds".into(),
            reorder: false,
            selected: None,
        }
    );
    let item = |label: &str, detail: Option<&str>| ViewKind::Item {
        label: label.into(),
        detail: detail.map(Into::into),
        checked: None,
        warning: None,
    };
    assert_eq!(
        kinds(&beds.children),
        [
            ("north", &item("North bed", Some("Tomatoes"))),
            ("south", &item("South bed", Some("Beans"))),
            ("herbs", &item("Herb spiral", None)),
        ]
    );
    let menu = &beds.children[1].children[0];
    assert_eq!(
        menu.kind,
        ViewKind::Menu {
            label: "Actions for South bed".into()
        }
    );
    assert_eq!(
        kinds(&menu.children),
        [
            ("south-water", &menu_item("Water", true)),
            ("south-gap", &ViewKind::Separator),
            ("south-harvest", &menu_item("Harvest", false)),
            ("south-remove", &menu_item("Remove", true)),
        ]
    );
    let more = find(view.root(), "more").unwrap();
    assert_eq!(
        kinds(&more.children),
        [(
            "more-menu",
            &ViewKind::Menu {
                label: "More actions".into()
            }
        )]
    );
}

#[test]
fn a_table_holds_its_columns_and_one_cell_per_column_in_each_row() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&kit, &VIEWS, None);

    let view = View::from_nodes(instance.render("table").unwrap()).expect("the view is valid");

    let table = find(view.root(), "plants").unwrap();
    let ViewKind::Table { label, columns } = &table.kind else {
        panic!("{:?}", table.kind);
    };
    assert_eq!(label, "Plants");
    let titles: Vec<_> = columns.iter().map(|column| column.title.as_str()).collect();
    assert_eq!(titles, ["Plant", "Bed", "Sown"]);
    let rows: Vec<_> = table
        .children
        .iter()
        .map(|row| match &row.kind {
            ViewKind::Row { cells } => cells.join(" | "),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(
        rows,
        [
            "Basil | Herb spiral | 2026-04-02",
            "Bean | South bed | 2026-05-01",
            "Tomato | North bed | 2026-03-15",
        ]
    );
}

#[test]
fn malformed_calendar_grids_are_refused_with_a_reason() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

    for (case, reason) in [
        (
            "entry-outside-calendar",
            "The entry standup isn't in a calendar.",
        ),
        (
            "item-in-calendar",
            "The calendar schedule holds north, which isn't an entry.",
        ),
        ("unlabeled-calendar", "The calendar schedule has no label."),
        ("untitled-entry", "The entry standup has no title."),
        (
            "bad-calendar-date",
            "The calendar schedule shows 2026-10-32, which isn't a date in the form \
             YYYY-MM-DD.",
        ),
        (
            "bad-entry-start",
            "The entry standup starts at 2026-10-21 09:00, which is neither a date in the \
             form YYYY-MM-DD nor an RFC 3339 date and time with an offset.",
        ),
        (
            "mixed-entry-end",
            "The entry standup ends at 2026-10-22, which doesn't have the form of its start.",
        ),
    ] {
        let nodes = instance.render(&format!("nodes:{case}")).unwrap();
        let error = View::from_nodes(nodes).expect_err(case);
        assert_eq!(error.to_string(), reason, "{case}");
    }
}

#[test]
fn a_calendar_grid_holds_its_mode_its_date_and_its_entries() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&kit, &VIEWS, None);

    let view = View::from_nodes(instance.render("calendar").unwrap()).expect("the view is valid");

    let calendar = find(view.root(), "schedule").unwrap();
    assert_eq!(
        calendar.kind,
        ViewKind::Calendar {
            label: "Schedule".into(),
            mode: CalendarMode::Week,
            date: "2026-10-21".into(),
        }
    );
    let entry = |title: &str, start: &str, end: Option<&str>| ViewKind::Entry {
        title: title.into(),
        start: start.into(),
        end: end.map(Into::into),
    };
    assert_eq!(
        kinds(&calendar.children)[..2],
        [
            (
                "retreat",
                &entry("Retreat", "2026-10-19", Some("2026-10-22"))
            ),
            (
                "standup",
                &entry(
                    "Standup",
                    "2026-10-21T09:00:00-04:00",
                    Some("2026-10-21T09:30:00-04:00")
                )
            ),
        ]
    );

    // The plugin's own buttons switch the mode and the period.
    instance
        .handle_event("calendar", &Event::Pressed("month".into()))
        .unwrap();
    instance
        .handle_event("calendar", &Event::Pressed("next".into()))
        .unwrap();
    let view = View::from_nodes(instance.render("calendar").unwrap()).unwrap();
    let ViewKind::Calendar { mode, date, .. } = &find(view.root(), "schedule").unwrap().kind else {
        panic!("not a calendar");
    };
    assert_eq!((*mode, date.as_str()), (CalendarMode::Month, "2026-11-21"));
}

/// The node IDs of each column of the board `work` on the kit's board
/// page, as `column: card card`.
fn board_columns(instance: &RunningPlugin) -> Vec<String> {
    let view = View::from_nodes(instance.render("board").unwrap()).unwrap();
    let board = find(view.root(), "work").unwrap();
    board
        .children
        .iter()
        .map(|column| {
            let cards: Vec<&str> = column
                .children
                .iter()
                .map(|card| card.id.as_str())
                .collect();
            format!("{}: {}", column.id, cards.join(" "))
        })
        .collect()
}

#[test]
fn a_board_holds_labeled_columns_of_labeled_cards_with_menus() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&kit, &VIEWS, None);

    let view = View::from_nodes(instance.render("board").unwrap()).expect("the view is valid");

    let board = find(view.root(), "work").unwrap();
    assert_eq!(
        board.kind,
        ViewKind::Board {
            label: "Work".into()
        }
    );
    let column = |label: &str| ViewKind::BoardColumn {
        label: label.into(),
    };
    assert_eq!(
        kinds(&board.children),
        [
            ("todo", &column("To do")),
            ("doing", &column("Doing")),
            ("done", &column("Done")),
        ]
    );
    let card = &board.children[0].children[0];
    assert_eq!(card.id, "write-report");
    assert_eq!(
        card.kind,
        ViewKind::Card {
            label: "Write report".into(),
            detail: Some("Due Friday".into())
        }
    );
    let menu = &card.children[0];
    assert_eq!(
        menu.kind,
        ViewKind::Menu {
            label: "Move Write report".into()
        }
    );
    assert_eq!(
        kinds(&menu.children),
        [
            ("write-report-to-todo", &menu_item("To do", false)),
            ("write-report-to-doing", &menu_item("Doing", true)),
            ("write-report-to-done", &menu_item("Done", true)),
        ]
    );
    let queue = find(view.root(), "queue").unwrap();
    assert_eq!(
        queue.kind,
        ViewKind::List {
            label: "Queue".into(),
            reorder: true,
            selected: None,
        }
    );
}

#[test]
fn a_list_shows_its_selected_item_and_items_carry_checkboxes_and_warnings() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

    let view =
        View::from_nodes(instance.render("nodes:checklist").unwrap()).expect("the view is valid");

    let chores = find(view.root(), "chores").unwrap();
    assert_eq!(
        chores.kind,
        ViewKind::List {
            label: "Chores".into(),
            reorder: false,
            selected: Some("mulch".into()),
        }
    );
    let item = |label: &str, checked: Option<bool>, warning: Option<&str>| ViewKind::Item {
        label: label.into(),
        detail: None,
        checked,
        warning: warning.map(Into::into),
    };
    assert_eq!(
        kinds(&chores.children),
        [
            ("weed", &item("Weed the beds", Some(false), Some("Overdue"))),
            ("mulch", &item("Mulch", Some(true), None)),
            // A warning of only spaces shows nothing.
            ("plain", &item("Look around", None, None)),
        ]
    );
    assert_eq!(
        find(view.root(), "new-chore").unwrap().kind,
        ViewKind::NameField {
            label: "New chore name".into(),
            value: String::new(),
        }
    );
}

#[test]
fn a_selection_outside_the_list_a_card_checkbox_and_an_unlabeled_name_field_are_refused() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

    for (case, reason) in [
        (
            "unknown-selection",
            "The list chores selects missing, which isn't one of its items.",
        ),
        (
            "card-with-checkbox",
            "The card plan holds a checkbox or a warning, which only list items show.",
        ),
        (
            "unlabeled-name-field",
            "The name field new-chore has no label.",
        ),
        (
            "name-field-with-children",
            "The name field new-chore holds no other nodes.",
        ),
    ] {
        let nodes = instance.render(&format!("nodes:{case}")).unwrap();
        let error = View::from_nodes(nodes).expect_err(case);
        assert_eq!(error.to_string(), reason, "{case}");
    }
}

#[test]
fn checks_names_and_cancels_reach_the_plugin_by_node_id() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&probe, &VIEWS, None);

    for event in [
        Event::Checked(Check {
            node: "chores".into(),
            item: "weed".into(),
            checked: true,
        }),
        Event::Named(Naming {
            node: "new-chore".into(),
            name: "Prune the roses".into(),
        }),
        Event::Cancelled("new-chore".into()),
    ] {
        instance.handle_event("checklist", &event).unwrap();
    }

    let nodes = instance.render("events").unwrap();
    assert_eq!(
        plugins::texts(&nodes),
        [
            "checklist Event::Checked(Check { node: \"chores\", item: \"weed\", checked: true })",
            "checklist Event::Named(Naming { node: \"new-chore\", name: \"Prune the roses\" })",
            "checklist Event::Cancelled(\"new-chore\")",
        ]
    );
}

#[test]
fn a_move_reaches_the_plugin_by_node_id_and_the_board_shows_where_the_plugin_put_the_card() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&kit, &VIEWS, None);
    let moved = |node: &str, item: &str, to: &str, position: u32| {
        Event::Moved(Move {
            node: node.into(),
            item: item.into(),
            to: to.into(),
            position,
        })
    };
    assert_eq!(
        board_columns(&instance),
        [
            "todo: write-report fix-bug",
            "doing: review-notes",
            "done: plan-sprint"
        ]
    );

    instance
        .handle_event("board", &moved("work", "write-report", "doing", 1))
        .unwrap();
    assert_eq!(
        last_event(&instance, "board"),
        "Last event: moved write-report in work to doing at 1"
    );
    assert_eq!(
        board_columns(&instance),
        [
            "todo: fix-bug",
            "doing: review-notes write-report",
            "done: plan-sprint"
        ]
    );

    // The plugin refuses moves into Done, so the card stays where it was.
    instance
        .handle_event("board", &moved("work", "fix-bug", "done", 0))
        .unwrap();
    assert_eq!(
        last_event(&instance, "board"),
        "Last event: moved fix-bug in work to done at 0"
    );
    assert_eq!(
        board_columns(&instance),
        [
            "todo: fix-bug",
            "doing: review-notes write-report",
            "done: plan-sprint"
        ]
    );

    instance
        .handle_event("board", &moved("queue", "first", "queue", 2))
        .unwrap();
    let view = View::from_nodes(instance.render("board").unwrap()).unwrap();
    let queue: Vec<&str> = find(view.root(), "queue")
        .unwrap()
        .children
        .iter()
        .map(|item| item.id.as_str())
        .collect();
    assert_eq!(queue, ["second", "third", "first"]);
}

/// The text of the page's `last` node, which names the last event.
fn last_event(instance: &RunningPlugin, page: &str) -> String {
    let view = View::from_nodes(instance.render(page).unwrap()).unwrap();
    match &find(view.root(), "last").unwrap().kind {
        ViewKind::Text(text) => text.clone(),
        other => panic!("{other:?}"),
    }
}

fn input_value(instance: &RunningPlugin, id: &str) -> Value {
    let view = View::from_nodes(instance.render("form").unwrap()).unwrap();
    match &find(view.root(), id).unwrap().kind {
        ViewKind::Input { value, .. } => value.clone(),
        other => panic!("{other:?}"),
    }
}

fn field(node: &str, value: Value) -> Field {
    Field {
        node: node.into(),
        value,
    }
}

#[test]
fn selections_header_presses_and_menu_choices_reach_the_plugin_by_node_id() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&kit, &VIEWS, None);
    let target = |item: &str| Target {
        node: "beds".into(),
        item: item.into(),
    };

    for (event, said) in [
        (
            Event::Selected(target("south")),
            "Last event: selected south in beds",
        ),
        (
            Event::Activated(target("north")),
            "Last event: activated north in beds",
        ),
        (
            Event::Chosen("south-water".into()),
            "Last event: chose south-water",
        ),
        (
            Event::Sorted(ColumnPress {
                node: "plants".into(),
                column: "bed".into(),
            }),
            "Last event: sorted plants by bed",
        ),
    ] {
        instance.handle_event("lists", &event).unwrap();
        assert_eq!(last_event(&instance, "lists"), said);
    }
}

#[test]
fn changes_and_submits_reach_the_plugin_by_node_id_with_typed_values() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&kit, &VIEWS, None);

    instance
        .handle_event(
            "form",
            &Event::Changed(field("sown", Value::Date(Some("2026-10-21".into())))),
        )
        .unwrap();

    assert_eq!(
        last_event(&instance, "form"),
        "Last event: changed sown date 2026-10-21"
    );
    assert_eq!(
        input_value(&instance, "sown"),
        Value::Date(Some("2026-10-21".into()))
    );

    let submission = Submission {
        node: "new-plant".into(),
        fields: vec![
            field("name", Value::Text("Chard".into())),
            field("notes", Value::Multiline("Water well.\nMulch.".into())),
            field("indoors", Value::Checkbox(true)),
            field("sun", Value::Choice(Some("partial".into()))),
            field("sown", Value::Date(None)),
            field(
                "transplant",
                Value::DateTime(Some("2026-11-02T08:00:00+01:00".into())),
            ),
        ],
    };
    instance
        .handle_event("form", &Event::Submitted(submission))
        .unwrap();

    assert_eq!(
        last_event(&instance, "form"),
        "Last event: submitted new-plant: name text \"Chard\", \
         notes multiline \"Water well.\\nMulch.\", indoors checkbox true, \
         sun choice partial, sown date none, transplant date-time 2026-11-02T08:00:00+01:00"
    );
    assert_eq!(input_value(&instance, "indoors"), Value::Checkbox(true));
    let view = View::from_nodes(instance.render("table").unwrap()).unwrap();
    let chard = find(view.root(), "plant-4").expect("the plant joined the table");
    assert_eq!(
        chard.kind,
        ViewKind::Row {
            cells: vec!["Chard".into(), "Pots".into(), "-".into()]
        }
    );
}

#[test]
fn a_virtual_list_fetches_only_the_rows_asked_for() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let (instance, _folders) = plugins::running(&kit, &VIEWS, None);
    let view = View::from_nodes(instance.render("seeds").unwrap()).unwrap();
    assert_eq!(
        find(view.root(), "seeds").unwrap().kind,
        ViewKind::VirtualList {
            label: "Seeds".into(),
            rows: 10_000
        }
    );

    let first = check_rows(instance.rows("seeds", "seeds", 0, 40).unwrap(), 40).unwrap();
    let last = check_rows(instance.rows("seeds", "seeds", 9_990, 40).unwrap(), 40).unwrap();

    assert_eq!(first.len(), 40);
    assert_eq!(
        first[0],
        VirtualRow {
            id: "seed-1".into(),
            item: Item {
                label: "Seed 1".into(),
                detail: Some("Batch 1".into()),
                checked: None,
                warning: None,
            }
        }
    );
    let labels: Vec<_> = last.iter().map(|row| row.item.label.as_str()).collect();
    assert_eq!(labels.len(), 10);
    assert_eq!(labels.last(), Some(&"Seed 10000"));
    assert_eq!(last[0].item.detail.as_deref(), Some("Batch 2"));
}

#[test]
fn rows_that_break_the_rules_are_refused_with_a_reason() {
    let row = |id: &str, label: &str| VirtualRow {
        id: id.into(),
        item: Item {
            label: label.into(),
            detail: None,
            checked: None,
            warning: None,
        },
    };

    for (rows, asked, reason) in [
        (
            vec![row("a", "A"), row("b", "B")],
            1,
            "The plugin returned 2 rows when Noura asked for 1.",
        ),
        (vec![row("a", " ")], 1, "The row a has no label."),
        (vec![row("", "A")], 1, "A row has no ID."),
        (
            vec![row("a", "A"), row("a", "B")],
            2,
            "Two rows have the ID a. Each row needs its own ID.",
        ),
    ] {
        assert_eq!(check_rows(rows, asked).unwrap_err().to_string(), reason);
    }
}

#[test]
fn a_node_of_a_kind_the_host_doesnt_know_fails_the_render_and_the_plugin_keeps_running() {
    let component = wat::parse_str(UNKNOWN_KIND).unwrap();
    let (instance, _folders) = plugins::running(&component, &VIEWS, None);

    let error = instance.render("any").expect_err("the render fails");

    assert!(matches!(error, PluginError::Call(_)), "{error:?}");
    assert_eq!(instance.pages().expect("the plugin still answers"), []);
}

#[test]
fn a_component_that_exports_only_its_view_registers_nothing_else() {
    let component = wat::parse_str(UNKNOWN_KIND).unwrap();
    let (plugin, _folders) = plugins::running(
        &component,
        &[
            "name = \"views\"",
            "name = \"commands\"",
            "name = \"tools\"",
        ],
        None,
    );

    assert_eq!(plugin.pages(), Ok(Vec::new()));
    assert_eq!(
        plugin.commands(),
        Err(PluginError::NotExported {
            interface: "noura:plugin/commands"
        })
    );
    assert_eq!(
        plugin.tools(),
        Err(PluginError::NotExported {
            interface: "noura:plugin/tools"
        })
    );
    assert_eq!(plugin.health().recent_failures, 0);
}

#[test]
fn a_component_that_exports_the_view_interface_without_its_functions_or_types_fails_to_start() {
    for (case, component) in [
        ("missing functions", MISSING_FUNCTIONS),
        ("wrong types", WRONG_TYPES),
    ] {
        let folder = tempfile::tempdir().unwrap();
        let library = tempfile::tempdir().unwrap();
        plugins::write_folder(
            folder.path(),
            &plugins::manifest("probe", "plugin.wasm", &VIEWS),
            "plugin.wasm",
            &wat::parse_str(component).unwrap(),
        );
        let host = plugins::host(library.path());
        host.install(&host.inspect(folder.path()).unwrap()).unwrap();
        let loaded = host.load("probe").unwrap();
        let plugin = host.start(
            &loaded,
            &loaded.manifest().capabilities,
            Services::default(),
        );

        let Err(error) = plugin.pages() else {
            panic!("{case}: the plugin started");
        };
        assert!(
            matches!(error, PluginError::Instantiate(_)),
            "{case}: {error:?}"
        );
    }
}

/// A component exporting `noura:plugin/view@0.1.0` with `pages` alone.
const MISSING_FUNCTIONS: &str = r#"
(component
  (core module $m (func (export "pages")))
  (core instance $i (instantiate $m))
  (func $pages (canon lift (core func $i "pages")))
  (instance $view (export "pages" (func $pages)))
  (export "noura:plugin/view@0.1.0" (instance $view)))
"#;

/// A component exporting `noura:plugin/view@0.1.0` whose functions take
/// and return nothing.
const WRONG_TYPES: &str = r#"
(component
  (core module $m (func (export "f")))
  (core instance $i (instantiate $m))
  (func $f (canon lift (core func $i "f")))
  (instance $view
    (export "pages" (func $f))
    (export "render" (func $f))
    (export "handle-event" (func $f)))
  (export "noura:plugin/view@0.1.0" (instance $view)))
"#;

/// A component exporting `noura:plugin/view@0.1.0` whose `render` returns
/// one node with kind number 48, which `node-kind` doesn't have.
const UNKNOWN_KIND: &str = r#"
(component
  (component $view
    (core module $m
      (memory (export "memory") 1)
      (data (i32.const 8) "\10\00\00\00\01\00\00\00")
      (data (i32.const 16) "\64\00\00\00\04\00\00\00\30")
      (data (i32.const 100) "root")
      (func (export "pages") (result i32) i32.const 200)
      (func (export "render") (param i32 i32) (result i32) i32.const 8)
      (func (export "sidebar") (param i32 i32) (result i32) i32.const 200)
      (func (export "rows") (param i32 i32 i32 i32 i32 i32) (result i32) i32.const 200)
      (func (export "handle-event") (param i32 i32 i32 i32 i32 i32 i32 i32 i32 i32))
      (func (export "realloc") (param i32 i32 i32 i32) (result i32) i32.const 1024))
    (core instance $i (instantiate $m))
    (type $item' (record
      (field "label" string) (field "detail" (option string)) (field "checked" (option bool))
      (field "warning" (option string))))
    (export $item "item" (type $item'))
    (type $item-list' (record
      (field "label" string) (field "reorder" bool) (field "selected" (option string))))
    (export $item-list "item-list" (type $item-list'))
    (type $name-field' (record (field "label" string) (field "value" string)))
    (export $name-field "name-field" (type $name-field'))
    (type $virtual-list' (record (field "label" string) (field "rows" u32)))
    (export $virtual-list "virtual-list" (type $virtual-list'))
    (type $virtual-row' (record (field "id" string) (field "item" $item)))
    (export $virtual-row "virtual-row" (type $virtual-row'))
    (type $sort-order' (enum "ascending" "descending"))
    (export $sort-order "sort-order" (type $sort-order'))
    (type $column' (record
      (field "id" string) (field "title" string) (field "sorted" (option $sort-order))))
    (export $column "column" (type $column'))
    (type $table' (record (field "label" string) (field "columns" (list $column))))
    (export $table "table" (type $table'))
    (type $form' (record (field "label" string) (field "submit" string)))
    (export $form "form" (type $form'))
    (type $value' (variant
      (case "text" string) (case "multiline" string) (case "checkbox" bool)
      (case "choice" (option string)) (case "date" (option string))
      (case "date-time" (option string))))
    (export $value "value" (type $value'))
    (type $choice' (record (field "id" string) (field "label" string)))
    (export $choice "choice" (type $choice'))
    (type $input' (record
      (field "label" string) (field "value" $value) (field "choices" (list $choice))))
    (export $input "input" (type $input'))
    (type $menu-item' (record (field "label" string) (field "enabled" bool)))
    (export $menu-item "menu-item" (type $menu-item'))
    (type $calendar-mode' (enum "month" "week" "day"))
    (export $calendar-mode "calendar-mode" (type $calendar-mode'))
    (type $calendar' (record
      (field "label" string) (field "mode" $calendar-mode) (field "date" string)))
    (export $calendar "calendar" (type $calendar'))
    (type $calendar-entry' (record
      (field "title" string) (field "start" string) (field "end" (option string))))
    (export $calendar-entry "calendar-entry" (type $calendar-entry'))
    (type $node-kind' (variant
      (case "panel" (option string)) (case "text" string)
      (case "markdown" string) (case "button" string)
      (case "list" $item-list) (case "item" $item) (case "virtual-list" $virtual-list)
      (case "table" $table) (case "row" (list string)) (case "form" $form)
      (case "input" $input) (case "name-field" $name-field) (case "menu" string) (case "menu-item" $menu-item)
      (case "separator") (case "board" string) (case "board-column" string)
      (case "card" $item) (case "calendar" $calendar) (case "entry" $calendar-entry)))
    (export $node-kind "node-kind" (type $node-kind'))
    (type $node' (record
      (field "id" string) (field "kind" $node-kind) (field "children" (list u32))))
    (export $node "node" (type $node'))
    (type $page' (record (field "id" string) (field "title" string)))
    (export $page "page" (type $page'))
    (type $target' (record (field "node" string) (field "item" string)))
    (export $target "target" (type $target'))
    (type $move' (record
      (field "node" string) (field "item" string) (field "to" string) (field "position" u32)))
    (export $move "move" (type $move'))
    (type $column-press' (record (field "node" string) (field "column" string)))
    (export $column-press "column-press" (type $column-press'))
    (type $field' (record (field "node" string) (field "value" $value)))
    (export $field "field" (type $field'))
    (type $submission' (record (field "node" string) (field "fields" (list $field))))
    (export $submission "submission" (type $submission'))
    (type $check' (record (field "node" string) (field "item" string) (field "checked" bool)))
    (export $check "check" (type $check'))
    (type $naming' (record (field "node" string) (field "name" string)))
    (export $naming "naming" (type $naming'))
    (type $event' (variant
      (case "pressed" string) (case "selected" $target) (case "activated" $target)
      (case "sorted" $column-press) (case "changed" $field)
      (case "submitted" $submission) (case "chosen" string) (case "moved" $move)
      (case "checked" $check) (case "named" $naming) (case "cancelled" string)))
    (export $event "event" (type $event'))
    (func $pages (result (list $page))
      (canon lift (core func $i "pages") (memory (core memory $i "memory"))
        (realloc (core func $i "realloc"))))
    (export "pages" (func $pages))
    (func $render (param "page" string) (result (list $node))
      (canon lift (core func $i "render") (memory (core memory $i "memory"))
        (realloc (core func $i "realloc"))))
    (export "render" (func $render))
    (func $sidebar (param "page" string) (result (list $node))
      (canon lift (core func $i "sidebar") (memory (core memory $i "memory"))
        (realloc (core func $i "realloc"))))
    (export "sidebar" (func $sidebar))
    (func $rows (param "page" string) (param "node" string) (param "start" u32)
      (param "count" u32) (result (list $virtual-row))
      (canon lift (core func $i "rows") (memory (core memory $i "memory"))
        (realloc (core func $i "realloc"))))
    (export "rows" (func $rows))
    (func $handle-event (param "page" string) (param "event" $event)
      (canon lift (core func $i "handle-event") (memory (core memory $i "memory"))
        (realloc (core func $i "realloc"))))
    (export "handle-event" (func $handle-event)))
  (instance $view-instance (instantiate $view))
  (export "noura:plugin/view@0.1.0" (instance $view-instance)))
"#;
