//! A test plugin for the UI kit. Each page holds one kind of node, and a
//! text node at its top, `last`, says what the last event was, such as
//! `Last event: selected south in beds`:
//!
//! - `lists`, titled Lists: the list Beds, whose items each have a menu,
//!   a More menu button, a Redraw button and a Remove first bed button,
//!   which takes the list's first item away.
//! - `table`, titled Table: the table Plants, which sorts by the column
//!   whose header you press, and whose rows each have a menu.
//! - `form`, titled Form: the form New plant with one input of each kind.
//!   The inputs show the values the plugin holds, which each change
//!   updates. Submitting adds the plant to the table.
//! - `seeds`, titled Seeds: the virtual list Seeds of 10,000 rows. Each
//!   row's detail names the fetch that returned it, such as `Batch 1`. A
//!   press of `insert` puts three rows, New seed 1 to 3, above them.
//! - `broken-rows`, titled Broken rows: the virtual list Broken of 100
//!   rows, which returns one row more than Noura asks for. A text node
//!   says which rows Noura asked for, such as `Asked for rows: 0 to 63`.
//!   Below it, the virtual list Repeated of 100 rows, whose row IDs start
//!   over at the 81st row.
//! - `board`, titled Board: the board Work, with the columns To do, Doing
//!   and Done. Each card has a menu, Move <card>, with an item per column
//!   that moves the card to the end of that column. The plugin applies
//!   every move except one into Done, which it refuses. Below the board,
//!   the list Queue, whose items you can drag into a new order.
//! - `calendar`, titled Calendar: the buttons Month, Week, Day, Previous,
//!   Today and Next, which set the grid's mode and move its period, and
//!   the calendar grid Schedule, which starts on the week of 2026-10-21.
//!   Its entries: Retreat, all day from the 19th to the 21st; Standup at
//!   9:00 and Review at 9:15, which overlap; Deploy from 23:00 to 1:00 the
//!   next day; First frost, all day on the 24th; and Seed market on
//!   2026-11-07. Times are in Toronto's offsets.

use std::sync::Mutex;

use jiff::{ToSpan, civil::Date};
use plugin_sdk::{
    CalendarMode, Column, Event, Field, Item, Move, Node, Page, Plugin, SortOrder, Value, View,
    VirtualRow,
};

/// The rows the Seeds list holds.
const SEEDS: u32 = 10_000;

/// The rows the Broken list holds.
const BROKEN: u32 = 100;

/// The rows the Repeated list holds, and how many have their own ID.
const REPEATED: u32 = 100;
const REPEATED_IDS: u32 = 80;

/// The rows a press of `insert` puts above the seeds.
const INSERTED: u32 = 3;

/// The board's columns: ID and label.
const COLUMNS: [(&str, &str); 3] = [("todo", "To do"), ("doing", "Doing"), ("done", "Done")];

/// The day the calendar page starts on, and Today goes back to.
const TODAY: Date = jiff::civil::date(2026, 10, 21);

/// The calendar page's entries: ID, title, start and end.
const ENTRIES: [(&str, &str, &str, Option<&str>); 6] = [
    ("retreat", "Retreat", "2026-10-19", Some("2026-10-22")),
    (
        "standup",
        "Standup",
        "2026-10-21T09:00:00-04:00",
        Some("2026-10-21T09:30:00-04:00"),
    ),
    (
        "review",
        "Review",
        "2026-10-21T09:15:00-04:00",
        Some("2026-10-21T10:00:00-04:00"),
    ),
    (
        "deploy",
        "Deploy",
        "2026-10-21T23:00:00-04:00",
        Some("2026-10-22T01:00:00-04:00"),
    ),
    ("frost", "First frost", "2026-10-24", None),
    (
        "market",
        "Seed market",
        "2026-11-07T10:00:00-05:00",
        Some("2026-11-07T12:00:00-05:00"),
    ),
];

/// A card on the board.
#[derive(Clone)]
struct Card {
    id: &'static str,
    label: &'static str,
    detail: Option<&'static str>,
}

struct Plant {
    id: String,
    name: String,
    bed: String,
    sown: String,
}

struct State {
    last: String,
    /// The Beds list's items: ID, label and detail.
    beds: Vec<(&'static str, &'static str, Option<&'static str>)>,
    /// The rows above the seeds.
    new_seeds: u32,
    sort: (String, SortOrder),
    plants: Vec<Plant>,
    name: String,
    notes: String,
    indoors: bool,
    sun: Option<String>,
    sown: Option<String>,
    transplant: Option<String>,
    batches: u32,
    /// The rows of the Broken list Noura asked for, as `first to last`.
    broken_asks: Vec<String>,
    /// The cards of each board column, in the order of [`COLUMNS`].
    columns: Vec<Vec<Card>>,
    /// The Queue list's items: ID and label.
    queue: Vec<(&'static str, &'static str)>,
    /// The calendar grid's mode and a day of its period.
    mode: CalendarMode,
    day: Date,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut state = STATE.lock().unwrap();
    let state = state.get_or_insert_with(|| State {
        last: "none".to_owned(),
        beds: vec![
            ("north", "North bed", Some("Tomatoes")),
            ("south", "South bed", Some("Beans")),
            ("herbs", "Herb spiral", None),
        ],
        new_seeds: 0,
        sort: ("name".to_owned(), SortOrder::Ascending),
        plants: [
            ("basil", "Basil", "Herb spiral", "2026-04-02"),
            ("tomato", "Tomato", "North bed", "2026-03-15"),
            ("bean", "Bean", "South bed", "2026-05-01"),
        ]
        .into_iter()
        .map(|(id, name, bed, sown)| Plant {
            id: id.to_owned(),
            name: name.to_owned(),
            bed: bed.to_owned(),
            sown: sown.to_owned(),
        })
        .collect(),
        name: String::new(),
        notes: String::new(),
        indoors: false,
        sun: None,
        sown: None,
        transplant: Some("2026-10-21T09:30:00-04:00".to_owned()),
        batches: 0,
        broken_asks: Vec::new(),
        columns: vec![
            vec![
                Card {
                    id: "write-report",
                    label: "Write report",
                    detail: Some("Due Friday"),
                },
                Card {
                    id: "fix-bug",
                    label: "Fix bug",
                    detail: None,
                },
            ],
            vec![Card {
                id: "review-notes",
                label: "Review notes",
                detail: None,
            }],
            vec![Card {
                id: "plan-sprint",
                label: "Plan sprint",
                detail: None,
            }],
        ],
        queue: vec![
            ("first", "Water seedlings"),
            ("second", "Turn compost"),
            ("third", "Sow beans"),
        ],
        mode: CalendarMode::Week,
        day: TODAY,
    });
    f(state)
}

struct Kit;

impl Plugin for Kit {
    fn pages() -> Vec<Page> {
        [
            ("lists", "Lists"),
            ("table", "Table"),
            ("form", "Form"),
            ("seeds", "Seeds"),
            ("broken-rows", "Broken rows"),
            ("board", "Board"),
            ("calendar", "Calendar"),
        ]
        .into_iter()
        .map(|(id, title)| Page {
            id: id.to_owned(),
            title: title.to_owned(),
        })
        .collect()
    }

    fn render(page: String) -> Vec<Node> {
        with_state(|state| {
            let title = match page.as_str() {
                "lists" => "Lists",
                "table" => "Table",
                "form" => "Form",
                "broken-rows" => "Broken rows",
                "board" => "Board",
                "calendar" => "Calendar",
                _ => "Seeds",
            };
            let mut view = View::panel("root", Some(title));
            view.text(View::ROOT, "last", &format!("Last event: {}", state.last));
            match page.as_str() {
                "lists" => lists(&mut view, state),
                "table" => table(&mut view, state),
                "form" => form(&mut view, state),
                "board" => board(&mut view, state),
                "calendar" => calendar(&mut view, state),
                "broken-rows" => {
                    let asked = if state.broken_asks.is_empty() {
                        "none".to_owned()
                    } else {
                        state.broken_asks.join(", ")
                    };
                    view.text(View::ROOT, "asked", &format!("Asked for rows: {asked}"));
                    view.virtual_list(View::ROOT, "broken", "Broken", BROKEN);
                    view.virtual_list(View::ROOT, "repeated", "Repeated", REPEATED);
                }
                _ => {
                    let rows = SEEDS + state.new_seeds;
                    view.virtual_list(View::ROOT, "seeds", "Seeds", rows);
                }
            }
            view.into_nodes()
        })
    }

    fn rows(_page: String, node: String, start: u32, count: u32) -> Vec<VirtualRow> {
        if node == "broken" {
            return with_state(|state| broken_rows(state, start, count));
        }
        if node == "repeated" {
            return (start..start.saturating_add(count).min(REPEATED))
                .map(|index| VirtualRow {
                    id: format!("repeated-{}", index % REPEATED_IDS),
                    item: Item {
                        label: format!("Repeated {index}"),
                        detail: None,
                    },
                })
                .collect();
        }
        if node != "seeds" {
            return Vec::new();
        }
        with_state(|state| {
            state.batches += 1;
            let batch = state.batches;
            let new = state.new_seeds;
            (start..start.saturating_add(count).min(SEEDS + new))
                .map(|index| {
                    let (id, label) = if index < new {
                        (
                            format!("new-seed-{}", index + 1),
                            format!("New seed {}", index + 1),
                        )
                    } else {
                        let seed = index - new + 1;
                        (format!("seed-{seed}"), format!("Seed {seed}"))
                    };
                    VirtualRow {
                        id,
                        item: Item {
                            label,
                            detail: Some(format!("Batch {batch}")),
                        },
                    }
                })
                .collect()
        })
    }

    fn handle_event(_page: String, event: Event) {
        with_state(|state| {
            state.last = describe(&event);
            match event {
                Event::Pressed(node) if node == "trim" && !state.beds.is_empty() => {
                    state.beds.remove(0);
                }
                Event::Pressed(node) if node == "insert" => state.new_seeds = INSERTED,
                Event::Pressed(node) => press_calendar(state, &node),
                Event::Sorted(press) => {
                    let order =
                        if state.sort.0 == press.column && state.sort.1 == SortOrder::Ascending {
                            SortOrder::Descending
                        } else {
                            SortOrder::Ascending
                        };
                    state.sort = (press.column, order);
                }
                Event::Changed(field) => keep(state, field),
                Event::Moved(placed) => place(state, &placed),
                Event::Chosen(node) => {
                    let moved = COLUMNS.iter().find_map(|(column, _)| {
                        let card = node.strip_suffix(&format!("-to-{column}"))?;
                        Some(Move {
                            node: "work".to_owned(),
                            item: card.to_owned(),
                            to: (*column).to_owned(),
                            position: u32::MAX,
                        })
                    });
                    if let Some(moved) = moved {
                        place(state, &moved);
                    }
                }
                Event::Submitted(submission) => {
                    for field in submission.fields {
                        keep(state, field);
                    }
                    let id = format!("plant-{}", state.plants.len() + 1);
                    state.plants.push(Plant {
                        id,
                        name: state.name.clone(),
                        bed: "Pots".to_owned(),
                        sown: state.sown.clone().unwrap_or_else(|| "-".to_owned()),
                    });
                }
                _ => {}
            }
        });
    }
}

/// Apply a move on the board or in the queue, except one into Done. A
/// position past the end puts the card or item last.
fn place(state: &mut State, placed: &Move) {
    let at = |len: usize| (placed.position as usize).min(len);
    if placed.node == "queue" && placed.to == "queue" {
        if let Some(from) = state.queue.iter().position(|(id, _)| *id == placed.item) {
            let item = state.queue.remove(from);
            let index = at(state.queue.len());
            state.queue.insert(index, item);
        }
        return;
    }
    if placed.node != "work" || placed.to == "done" {
        return;
    }
    let Some(to) = COLUMNS.iter().position(|(id, _)| *id == placed.to) else {
        return;
    };
    let from = state
        .columns
        .iter()
        .enumerate()
        .find_map(|(column, cards)| {
            let index = cards.iter().position(|card| card.id == placed.item)?;
            Some((column, index))
        });
    if let Some((column, index)) = from {
        let card = state.columns[column].remove(index);
        let index = at(state.columns[to].len());
        state.columns[to].insert(index, card);
    }
}

fn board(view: &mut View, state: &State) {
    let board = view.board(View::ROOT, "work", "Work");
    for ((column_id, column_label), cards) in COLUMNS.iter().zip(&state.columns) {
        let column = view.board_column(board, column_id, column_label);
        for card in cards {
            let node = view.card(column, card.id, card.label, card.detail);
            let menu = view.menu(
                node,
                &format!("{}-menu", card.id),
                &format!("Move {}", card.label),
            );
            for (to, label) in COLUMNS {
                view.menu_item(
                    menu,
                    &format!("{}-to-{to}", card.id),
                    label,
                    to != *column_id,
                );
            }
        }
    }
    let queue = view.reorderable_list(View::ROOT, "queue", "Queue");
    for (id, label) in &state.queue {
        view.item(queue, id, label, None);
    }
}

fn calendar(view: &mut View, state: &State) {
    for (id, label) in [
        ("month", "Month"),
        ("week", "Week"),
        ("day", "Day"),
        ("previous", "Previous"),
        ("today", "Today"),
        ("next", "Next"),
    ] {
        view.button(View::ROOT, id, label);
    }
    let day = state.day.to_string();
    let calendar = view.calendar(View::ROOT, "schedule", "Schedule", state.mode, &day);
    for (id, title, start, end) in ENTRIES {
        view.entry(calendar, id, title, start, end);
    }
}

/// Set the calendar's mode, or move its period by a month, a week or a
/// day, as the mode shows.
fn press_calendar(state: &mut State, node: &str) {
    let step = match state.mode {
        CalendarMode::Month => 1.month(),
        CalendarMode::Week => 1.week(),
        CalendarMode::Day => 1.day(),
    };
    match node {
        "month" => state.mode = CalendarMode::Month,
        "week" => state.mode = CalendarMode::Week,
        "day" => state.mode = CalendarMode::Day,
        "today" => state.day = TODAY,
        "previous" => state.day = state.day.checked_sub(step).unwrap_or(state.day),
        "next" => state.day = state.day.checked_add(step).unwrap_or(state.day),
        _ => {}
    }
}

/// One row more than Noura asked for.
fn broken_rows(state: &mut State, start: u32, count: u32) -> Vec<VirtualRow> {
    let end = start.saturating_add(count).min(BROKEN);
    state
        .broken_asks
        .push(format!("{start} to {}", end.saturating_sub(1)));
    (start..=end)
        .map(|index| VirtualRow {
            id: format!("broken-{index}"),
            item: Item {
                label: format!("Broken {index}"),
                detail: None,
            },
        })
        .collect()
}

fn lists(view: &mut View, state: &State) {
    let beds = view.list(View::ROOT, "beds", "Beds");
    for &(id, label, detail) in &state.beds {
        let item = view.item(beds, id, label, detail);
        let menu = view.menu(item, &format!("{id}-menu"), &format!("Actions for {label}"));
        view.menu_item(menu, &format!("{id}-water"), "Water", true);
        view.separator(menu, &format!("{id}-gap"));
        view.menu_item(menu, &format!("{id}-harvest"), "Harvest", false);
        view.menu_item(menu, &format!("{id}-remove"), "Remove", true);
    }
    let more = view.button(View::ROOT, "more", "More");
    let menu = view.menu(more, "more-menu", "More actions");
    view.menu_item(menu, "refresh", "Refresh", true);
    view.menu_item(menu, "export", "Export", false);
    view.button(View::ROOT, "redraw", "Redraw");
    view.button(View::ROOT, "trim", "Remove first bed");
}

fn table(view: &mut View, state: &State) {
    let (sorted_by, order) = &state.sort;
    let columns = [("name", "Plant"), ("bed", "Bed"), ("sown", "Sown")]
        .into_iter()
        .map(|(id, title)| Column {
            id: id.to_owned(),
            title: title.to_owned(),
            sorted: (id == sorted_by).then_some(*order),
        })
        .collect();
    let table = view.table(View::ROOT, "plants", "Plants", columns);
    let mut plants: Vec<&Plant> = state.plants.iter().collect();
    plants.sort_by(|a, b| {
        let key = |plant: &Plant| match sorted_by.as_str() {
            "bed" => plant.bed.clone(),
            "sown" => plant.sown.clone(),
            _ => plant.name.clone(),
        };
        key(a).cmp(&key(b))
    });
    if *order == SortOrder::Descending {
        plants.reverse();
    }
    for plant in plants {
        let row = view.row(table, &plant.id, &[&plant.name, &plant.bed, &plant.sown]);
        let menu = view.menu(
            row,
            &format!("{}-menu", plant.id),
            &format!("Actions for {}", plant.name),
        );
        view.menu_item(menu, &format!("{}-open", plant.id), "Open", true);
        view.menu_item(menu, &format!("{}-remove", plant.id), "Remove", true);
    }
}

fn form(view: &mut View, state: &State) {
    let form = view.form(View::ROOT, "new-plant", "New plant", "Add plant");
    view.input(form, "name", "Name", Value::Text(state.name.clone()));
    view.input(
        form,
        "notes",
        "Notes",
        Value::Multiline(state.notes.clone()),
    );
    view.input(
        form,
        "indoors",
        "Start indoors",
        Value::Checkbox(state.indoors),
    );
    view.choice_input(
        form,
        "sun",
        "Sun",
        &[
            ("full", "Full sun"),
            ("partial", "Partial shade"),
            ("shade", "Shade"),
        ],
        state.sun.as_deref(),
    );
    view.input(form, "sown", "Sown on", Value::Date(state.sown.clone()));
    view.input(
        form,
        "transplant",
        "Transplant at",
        Value::DateTime(state.transplant.clone()),
    );
}

/// Keep a changed value, so the next render shows it.
fn keep(state: &mut State, field: Field) {
    match (field.node.as_str(), field.value) {
        ("name", Value::Text(text)) => state.name = text,
        ("notes", Value::Multiline(text)) => state.notes = text,
        ("indoors", Value::Checkbox(checked)) => state.indoors = checked,
        ("sun", Value::Choice(choice)) => state.sun = choice,
        ("sown", Value::Date(date)) => state.sown = date,
        ("transplant", Value::DateTime(at)) => state.transplant = at,
        _ => {}
    }
}

fn describe(event: &Event) -> String {
    match event {
        Event::Pressed(node) => format!("pressed {node}"),
        Event::Selected(target) => format!("selected {} in {}", target.item, target.node),
        Event::Activated(target) => format!("activated {} in {}", target.item, target.node),
        Event::Sorted(press) => format!("sorted {} by {}", press.node, press.column),
        Event::Changed(field) => format!("changed {}", field_text(field)),
        Event::Submitted(submission) => format!(
            "submitted {}: {}",
            submission.node,
            submission
                .fields
                .iter()
                .map(field_text)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Event::Chosen(node) => format!("chose {node}"),
        Event::Moved(placed) => format!(
            "moved {} in {} to {} at {}",
            placed.item, placed.node, placed.to, placed.position
        ),
    }
}

fn field_text(field: &Field) -> String {
    let value = match &field.value {
        Value::Text(text) => format!("text {text:?}"),
        Value::Multiline(text) => format!("multiline {text:?}"),
        Value::Checkbox(checked) => format!("checkbox {checked}"),
        Value::Choice(choice) => format!("choice {}", choice.as_deref().unwrap_or("none")),
        Value::Date(date) => format!("date {}", date.as_deref().unwrap_or("none")),
        Value::DateTime(at) => format!("date-time {}", at.as_deref().unwrap_or("none")),
    };
    format!("{} {value}", field.node)
}

plugin_sdk::export!(Kit);
