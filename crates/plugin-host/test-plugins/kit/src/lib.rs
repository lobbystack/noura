//! A test plugin for the UI kit. Each page holds one kind of node, and a
//! text node at its top, `last`, says what the last event was, such as
//! `Last event: selected south in beds`:
//!
//! - `lists`, titled Lists: the list Beds, whose items each have a menu,
//!   a More menu button and a Redraw button.
//! - `table`, titled Table: the table Plants, which sorts by the column
//!   whose header you press, and whose rows each have a menu.
//! - `form`, titled Form: the form New plant with one input of each kind.
//!   The inputs show the values the plugin holds, which each change
//!   updates. Submitting adds the plant to the table.
//! - `seeds`, titled Seeds: the virtual list Seeds of 10,000 rows. Each
//!   row's detail names the fetch that returned it, such as `Batch 1`.

use std::sync::Mutex;

use plugin_sdk::{
    Column, Event, Field, Item, Node, Page, Plugin, SortOrder, Value, View, VirtualRow,
};

/// The rows the Seeds list holds.
const SEEDS: u32 = 10_000;

struct Plant {
    id: String,
    name: String,
    bed: String,
    sown: String,
}

struct State {
    last: String,
    sort: (String, SortOrder),
    plants: Vec<Plant>,
    name: String,
    notes: String,
    indoors: bool,
    sun: Option<String>,
    sown: Option<String>,
    transplant: Option<String>,
    batches: u32,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

fn with_state<T>(f: impl FnOnce(&mut State) -> T) -> T {
    let mut state = STATE.lock().unwrap();
    let state = state.get_or_insert_with(|| State {
        last: "none".to_owned(),
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
                _ => "Seeds",
            };
            let mut view = View::panel("root", Some(title));
            view.text(View::ROOT, "last", &format!("Last event: {}", state.last));
            match page.as_str() {
                "lists" => lists(&mut view),
                "table" => table(&mut view, state),
                "form" => form(&mut view, state),
                _ => {
                    view.virtual_list(View::ROOT, "seeds", "Seeds", SEEDS);
                }
            }
            view.into_nodes()
        })
    }

    fn rows(_page: String, node: String, start: u32, count: u32) -> Vec<VirtualRow> {
        if node != "seeds" {
            return Vec::new();
        }
        with_state(|state| {
            state.batches += 1;
            let batch = state.batches;
            (start..start.saturating_add(count).min(SEEDS))
                .map(|index| VirtualRow {
                    id: format!("seed-{}", index + 1),
                    item: Item {
                        label: format!("Seed {}", index + 1),
                        detail: Some(format!("Batch {batch}")),
                    },
                })
                .collect()
        })
    }

    fn handle_event(_page: String, event: Event) {
        with_state(|state| {
            state.last = describe(&event);
            match event {
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

fn lists(view: &mut View) {
    let beds = view.list(View::ROOT, "beds", "Beds");
    for (id, label, detail) in [
        ("north", "North bed", Some("Tomatoes")),
        ("south", "South bed", Some("Beans")),
        ("herbs", "Herb spiral", None),
    ] {
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
