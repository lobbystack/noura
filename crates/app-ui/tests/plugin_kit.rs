//! The UI kit through AccessKit: a test plugin's lists, tables, forms,
//! menus and virtual list, their roles and labels, their keyboard paths,
//! and the events they send back to the plugin by node ID.

mod support;

use std::time::{Duration, Instant};

use app_core::{ColumnPress, PluginEvent};
use egui::{
    Event, Key, Modifiers, MouseWheelSource, MouseWheelUnit, TouchPhase,
    accesskit::{Role, SortDirection, Toggled},
    vec2,
};
use egui_kittest::{
    Harness,
    kittest::{By, NodeT, Queryable},
};
use support::{
    FakePicker, create_workspace, harness,
    plugins::{self, install_and_enable},
    settle,
};

type Ui = Harness<'static, app_ui::NouraUi>;

struct Opened {
    harness: Ui,
    /// The app data and the folder around the workspace, which last as long
    /// as the test binds them.
    _folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace with the kit plugin installed and on, opened at launch.
fn opened(kit: &[u8]) -> Opened {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Garden", &parent.path().join("Garden"));
    let folder = plugins::folder(parent.path(), &plugins::kit_manifest(), "plugin.wasm", kit);
    install_and_enable(app_data.path(), &folder, "kit");
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    Opened {
        harness,
        _folders: (app_data, parent),
    }
}

fn open_page(harness: &mut Ui, title: &str) {
    harness
        .get_by_role_and_label(Role::Navigation, "Pages")
        .get_by_role_and_label(Role::Link, title)
        .click();
    harness.run();
}

fn press(harness: &mut Ui, key: Key) {
    harness.key_press(key);
    harness.run();
}

fn press_with(harness: &mut Ui, modifiers: Modifiers, key: Key) {
    harness.key_press_modifiers(modifiers, key);
    harness.run();
}

/// What the plugin says the last event was.
fn last_event(harness: &Ui) -> String {
    let said: Vec<String> = harness
        .query_all(By::new().role(Role::Label))
        .filter_map(|label| label.accesskit_node().value())
        .filter_map(|value| value.strip_prefix("Last event: ").map(str::to_owned))
        .collect();
    assert_eq!(said.len(), 1, "{said:?}");
    said[0].clone()
}

fn labels<'a>(nodes: impl Iterator<Item = egui_kittest::Node<'a>>) -> Vec<String> {
    nodes
        .filter_map(|node| node.accesskit_node().label())
        .collect()
}

fn option<'a>(harness: &'a Ui, label: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::ListBoxOption, label)
}

fn is_selected(node: &egui_kittest::Node<'_>) -> bool {
    node.accesskit_node().is_selected() == Some(true)
}

/// The label of the widget with keyboard focus.
fn focused(harness: &Ui) -> Option<String> {
    harness
        .query_all(By::new())
        .find(|node| node.is_focused())
        .and_then(|node| node.accesskit_node().label())
}

#[test]
fn list_items_are_selected_by_click_and_arrow_keys_and_opened_with_enter() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Lists");

    let list = harness.get_by_role_and_label(Role::ListBox, "Beds");
    assert_eq!(
        labels(list.get_all_by_role(Role::ListBoxOption)),
        ["North bed", "South bed", "Herb spiral"]
    );
    assert_eq!(
        option(harness, "North bed").accesskit_node().description(),
        Some("Tomatoes".into())
    );
    assert!(harness.unnamed_widgets().is_empty());

    option(harness, "North bed").click();
    harness.run();
    assert_eq!(last_event(harness), "selected north in beds");
    assert!(option(harness, "North bed").is_focused());
    assert!(is_selected(&option(harness, "North bed")));

    press(harness, Key::ArrowDown);
    assert_eq!(last_event(harness), "selected south in beds");
    assert!(option(harness, "South bed").is_focused());
    assert!(is_selected(&option(harness, "South bed")));
    assert!(!is_selected(&option(harness, "North bed")));

    press(harness, Key::End);
    assert_eq!(last_event(harness), "selected herbs in beds");
    press(harness, Key::Home);
    assert_eq!(last_event(harness), "selected north in beds");
    press(harness, Key::Enter);
    assert_eq!(last_event(harness), "activated north in beds");

    // The list is one stop in the Tab order, on its selected item.
    press(harness, Key::Tab);
    assert_eq!(focused(harness).as_deref(), Some("More"));
    press_with(harness, Modifiers::SHIFT, Key::Tab);
    assert!(option(harness, "North bed").is_focused());

    let pos = option(harness, "Herb spiral").rect().center();
    double_click_at(harness, pos);
    assert_eq!(last_event(harness), "activated herbs in beds");
    assert!(is_selected(&option(harness, "Herb spiral")));
}

/// Double-click at `pos`. Both clicks land in one frame: the harness
/// steps a quarter second per event, slower than a double-click.
fn double_click_at(harness: &mut Ui, pos: egui::Pos2) {
    let button = |pressed| Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: Modifiers::NONE,
    };
    harness.input_mut().events.extend([
        Event::PointerMoved(pos),
        button(true),
        button(false),
        button(true),
        button(false),
    ]);
    harness.step();
    harness.run();
}

#[test]
fn an_items_menu_opens_from_the_keyboard_and_sends_the_chosen_item() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Lists");
    option(harness, "South bed").click();
    harness.run();

    press_with(harness, Modifiers::SHIFT, Key::F10);

    let menu = harness.get_by_label("Actions for South bed");
    assert_eq!(
        labels(menu.get_all_by_role(Role::Button)),
        ["Water", "Harvest", "Remove"]
    );
    assert!(
        menu.get_by_role_and_label(Role::Button, "Harvest")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        menu.get_by_role_and_label(Role::Button, "Water")
            .is_focused()
    );
    assert!(harness.unnamed_widgets().is_empty());
    // The arrow skips the disabled item.
    press(harness, Key::ArrowDown);
    assert_eq!(focused(harness).as_deref(), Some("Remove"));
    press(harness, Key::Enter);

    assert_eq!(last_event(harness), "chose south-remove");
    assert!(harness.query_by_label("Actions for South bed").is_none());
    assert!(option(harness, "South bed").is_focused());

    press(harness, Key::Menu);
    assert!(harness.query_by_label("Actions for South bed").is_some());
    press(harness, Key::Escape);
    assert!(harness.query_by_label("Actions for South bed").is_none());
    assert!(option(harness, "South bed").is_focused());

    option(harness, "North bed").click_secondary();
    harness.run();
    harness
        .get_by_label("Actions for North bed")
        .get_by_role_and_label(Role::Button, "Water")
        .click();
    harness.run();
    assert_eq!(last_event(harness), "chose north-water");
}

#[test]
fn a_menu_button_opens_its_menu_from_the_keyboard() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Lists");

    harness.get_by_role_and_label(Role::Button, "More").focus();
    harness.run();
    press(harness, Key::Enter);

    let menu = harness.get_by_label("More actions");
    assert_eq!(
        labels(menu.get_all_by_role(Role::Button)),
        ["Refresh", "Export"]
    );
    assert!(
        menu.get_by_role_and_label(Role::Button, "Export")
            .accesskit_node()
            .is_disabled()
    );
    assert!(
        menu.get_by_role_and_label(Role::Button, "Refresh")
            .is_focused()
    );
    press(harness, Key::Enter);

    assert_eq!(last_event(harness), "chose refresh");
    assert!(harness.query_by_label("More actions").is_none());
    assert_eq!(focused(harness).as_deref(), Some("More"));
}

/// The text of each cell in the table, row by row.
fn cells(harness: &Ui) -> Vec<String> {
    labels(
        harness
            .get_by_role_and_label(Role::Table, "Plants")
            .get_all_by_role(Role::Cell),
    )
}

fn header<'a>(harness: &'a Ui, title: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::ColumnHeader, title)
}

#[test]
fn a_table_exposes_its_header_rows_and_cells_and_a_header_press_sorts() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Table");

    let table = harness.get_by_role_and_label(Role::Table, "Plants");
    assert_eq!(
        labels(table.get_all_by_role(Role::ColumnHeader)),
        ["Plant", "Bed", "Sown"]
    );
    // The header row, then a row per plant.
    assert_eq!(table.get_all_by_role(Role::Row).count(), 4);
    assert_eq!(
        cells(harness),
        [
            "Basil",
            "Herb spiral",
            "2026-04-02",
            "Bean",
            "South bed",
            "2026-05-01",
            "Tomato",
            "North bed",
            "2026-03-15",
        ]
    );
    assert_eq!(
        header(harness, "Plant").accesskit_node().sort_direction(),
        Some(SortDirection::Ascending)
    );
    assert!(harness.unnamed_widgets().is_empty());

    header(harness, "Sown").click();
    harness.run();

    assert_eq!(last_event(harness), "sorted plants by sown");
    assert_eq!(cells(harness)[0], "Tomato");
    assert_eq!(
        header(harness, "Sown").accesskit_node().sort_direction(),
        Some(SortDirection::Ascending)
    );
    assert_eq!(
        header(harness, "Plant").accesskit_node().sort_direction(),
        None
    );

    // A header takes the keyboard too.
    header(harness, "Sown").focus();
    harness.run();
    press(harness, Key::Enter);
    assert_eq!(cells(harness)[0], "Bean");
    assert_eq!(
        header(harness, "Sown").accesskit_node().sort_direction(),
        Some(SortDirection::Descending)
    );
}

fn row<'a>(harness: &'a Ui, first_cell: &'a str) -> egui_kittest::Node<'a> {
    harness.get_by_role_and_label(Role::Row, first_cell)
}

#[test]
fn table_rows_select_from_the_keyboard_and_open_their_menu_by_right_click_or_key() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Table");

    row(harness, "Basil").click();
    harness.run();
    assert_eq!(last_event(harness), "selected basil in plants");
    press(harness, Key::ArrowDown);
    assert_eq!(last_event(harness), "selected bean in plants");
    assert!(row(harness, "Bean").is_focused());
    assert!(is_selected(&row(harness, "Bean")));

    press_with(harness, Modifiers::SHIFT, Key::F10);
    assert!(
        harness
            .get_by_label("Actions for Bean")
            .get_by_role_and_label(Role::Button, "Open")
            .is_focused()
    );
    press(harness, Key::Enter);
    assert_eq!(last_event(harness), "chose bean-open");

    row(harness, "Tomato").click_secondary();
    harness.run();
    harness
        .get_by_label("Actions for Tomato")
        .get_by_role_and_label(Role::Button, "Remove")
        .click();
    harness.run();
    assert_eq!(last_event(harness), "chose tomato-remove");
}

fn form_field<'a>(harness: &'a Ui, role: Role, label: &'a str) -> egui_kittest::Node<'a> {
    harness
        .get_by_role_and_label(Role::Form, "New plant")
        .get_by_role_and_label(role, label)
}

/// Select everything in the focused text field and type `text` over it.
fn retype(harness: &mut Ui, text: &str) {
    press_with(harness, Modifiers::COMMAND, Key::A);
    harness
        .query_all(By::new())
        .find(|node| node.is_focused())
        .expect("a field has focus")
        .type_text(text);
    harness.run();
}

#[test]
fn form_inputs_are_labeled_and_reached_by_tab_in_order() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Form");

    form_field(harness, Role::TextInput, "Name");
    form_field(harness, Role::MultilineTextInput, "Notes");
    form_field(harness, Role::CheckBox, "Start indoors");
    let sun = form_field(harness, Role::RadioGroup, "Sun");
    assert_eq!(
        labels(sun.get_all_by_role(Role::RadioButton)),
        ["Full sun", "Partial shade", "Shade"]
    );
    form_field(harness, Role::TextInput, "Sown on");
    form_field(harness, Role::TextInput, "Transplant at, date");
    assert_eq!(
        form_field(harness, Role::TextInput, "Transplant at, time").value(),
        Some("09:30".into())
    );
    form_field(harness, Role::Button, "Add plant");
    assert!(harness.unnamed_widgets().is_empty());

    form_field(harness, Role::TextInput, "Name").focus();
    harness.run();
    let mut order = vec![focused(harness).unwrap()];
    for _ in 0..9 {
        press(harness, Key::Tab);
        order.push(focused(harness).unwrap());
    }
    assert_eq!(
        order,
        [
            "Name",
            "Notes",
            "Start indoors",
            "Full sun",
            "Partial shade",
            "Shade",
            "Sown on",
            "Transplant at, date",
            "Transplant at, time",
            "Add plant",
        ]
    );
}

#[test]
fn form_changes_and_submits_reach_the_plugin_with_typed_values() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Form");

    form_field(harness, Role::TextInput, "Name").focus();
    harness.run();
    retype(harness, "Chard");
    assert_eq!(last_event(harness), "changed name text \"Chard\"");

    form_field(harness, Role::TextInput, "Sown on").focus();
    harness.run();
    retype(harness, "2026-10-2");
    // Half a date isn't a date, so the plugin hears nothing yet.
    assert_eq!(last_event(harness), "changed name text \"Chard\"");
    retype(harness, "2026-10-21");
    assert_eq!(last_event(harness), "changed sown date 2026-10-21");

    form_field(harness, Role::TextInput, "Transplant at, time").focus();
    harness.run();
    retype(harness, "10:15");
    assert_eq!(
        last_event(harness),
        "changed transplant date-time 2026-10-21T10:15:00-04:00"
    );

    form_field(harness, Role::CheckBox, "Start indoors").focus();
    harness.run();
    press(harness, Key::Space);
    assert_eq!(last_event(harness), "changed indoors checkbox true");
    assert_eq!(
        form_field(harness, Role::CheckBox, "Start indoors")
            .accesskit_node()
            .toggled(),
        Some(Toggled::True)
    );

    form_field(harness, Role::RadioButton, "Partial shade").click();
    harness.run();
    assert_eq!(last_event(harness), "changed sun choice partial");

    form_field(harness, Role::Button, "Add plant").focus();
    harness.run();
    press(harness, Key::Enter);
    assert_eq!(
        last_event(harness),
        "submitted new-plant: name text \"Chard\", notes multiline \"\", indoors checkbox true, \
         sun choice partial, sown date 2026-10-21, transplant date-time 2026-10-21T10:15:00-04:00"
    );
}

#[test]
fn a_virtual_list_of_ten_thousand_rows_fetches_rows_as_they_scroll_into_view() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Seeds");

    let list = harness.get_by_role_and_label(Role::ListBox, "Seeds");
    let drawn = list.get_all_by_role(Role::ListBoxOption).count();
    assert!((10..100).contains(&drawn), "drew {drawn} rows of 10,000");
    let first = option(harness, "Seed 1");
    assert_eq!(first.accesskit_node().description(), Some("Batch 1".into()));
    assert_eq!(first.accesskit_node().position_in_set(), Some(1));
    assert_eq!(first.accesskit_node().size_of_set(), Some(10_000));

    first.click();
    harness.run();
    press(harness, Key::End);

    let last = option(harness, "Seed 10000");
    assert!(last.is_focused());
    assert!(is_selected(&last));
    assert_eq!(last.accesskit_node().position_in_set(), Some(10_000));
    // A later fetch returned it.
    assert_ne!(last.accesskit_node().description(), Some("Batch 1".into()));
    assert_eq!(last_event(harness), "selected seed-10000 in seeds");
    assert!(harness.query_by_label("Seed 1").is_none());
    assert!(
        harness
            .get_by_role_and_label(Role::ListBox, "Seeds")
            .get_all_by_role(Role::ListBoxOption)
            .count()
            < 100
    );
}

#[test]
fn a_virtual_list_whose_rows_break_the_rules_shows_why_and_noura_keeps_running() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Broken rows");

    let list = harness.get_by_role_and_label(Role::ListBox, "Broken");
    assert!(list.query_by_role(Role::ListBoxOption).is_none());
    list.get_by_role(Role::Alert)
        .get_by_label("The plugin returned 65 rows when Noura asked for 64.");

    open_page(harness, "Lists");
    option(harness, "North bed").click();
    harness.run();
    assert_eq!(last_event(harness), "selected north in beds");
}

#[test]
fn a_redraw_with_the_same_node_ids_keeps_selection_focus_and_scroll() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    let redraw = |harness: &mut Ui, page: &str| {
        harness
            .state_mut()
            .app_mut()
            .send_plugin_event(page, &PluginEvent::Pressed("redraw".into()));
        harness.run();
        assert_eq!(last_event(harness), "pressed redraw");
    };

    open_page(harness, "Lists");
    option(harness, "South bed").click();
    harness.run();
    redraw(harness, "plugin:kit:lists");
    assert!(option(harness, "South bed").is_focused());
    assert!(is_selected(&option(harness, "South bed")));

    // A table finds its selected row by ID wherever the redraw put it.
    open_page(harness, "Table");
    row(harness, "Bean").click();
    harness.run();
    harness.state_mut().app_mut().send_plugin_event(
        "plugin:kit:table",
        &PluginEvent::Sorted(ColumnPress {
            node: "plants".into(),
            column: "sown".into(),
        }),
    );
    harness.run();
    assert_eq!(cells(harness)[6], "Bean");
    let bean = row(harness, "Bean");
    assert!(bean.is_focused());
    assert!(is_selected(&bean));
    assert!(!is_selected(&row(harness, "Basil")));

    open_page(harness, "Seeds");
    option(harness, "Seed 1").click();
    harness.run();
    for _ in 0..40 {
        harness.key_press(Key::ArrowDown);
    }
    harness.run();
    assert!(option(harness, "Seed 41").is_focused());
    let top = option(harness, "Seed 41").rect().top();
    redraw(harness, "plugin:kit:seeds");
    let seed = option(harness, "Seed 41");
    assert!(seed.is_focused());
    assert!(is_selected(&seed));
    assert_eq!(seed.rect().top(), top);
    assert!(harness.query_by_label("Seed 1").is_none());

    open_page(harness, "Form");
    form_field(harness, Role::TextInput, "Name").focus();
    harness.run();
    retype(harness, "Ch");
    redraw(harness, "plugin:kit:form");
    let name = form_field(harness, Role::TextInput, "Name");
    assert!(name.is_focused());
    assert_eq!(name.value(), Some("Ch".into()));
}

/// Report the time each frame takes while the Seeds list of 10,000 rows
/// scrolls, its rows coming from the plugin as they appear. It measures
/// egui's layout and tessellation and the plugin's calls on the CPU, not
/// the GPU. CI runs it in release on the macOS arm64 runner:
/// `cargo test -p app-ui --release --test plugin_kit -- --ignored --nocapture`.
#[test]
#[ignore = "a timing report; CI runs it in release on macOS arm64"]
fn virtual_list_scroll_frame_times() {
    let Some(kit) = plugins::built("kit_plugin") else {
        return;
    };
    let mut opened = opened(&kit);
    let harness = &mut opened.harness;
    open_page(harness, "Seeds");
    let row = option(harness, "Seed 1").rect().center();
    harness.hover_at(row);
    harness.run();

    let mut frames = Vec::new();
    for _ in 0..600 {
        harness.event(Event::MouseWheel {
            unit: MouseWheelUnit::Point,
            delta: vec2(0.0, -240.0),
            phase: TouchPhase::Move,
            source: MouseWheelSource::Unknown,
            modifiers: Modifiers::NONE,
        });
        let start = Instant::now();
        harness.step();
        frames.push(start.elapsed());
    }

    frames.sort();
    let at = |fraction: f64| frames[((frames.len() - 1) as f64 * fraction) as usize];
    let ms = |duration: Duration| duration.as_secs_f64() * 1000.0;
    println!(
        "plugin virtual list, 10,000 rows, {} scroll frames: p50 {:.3} ms, p95 {:.3} ms, \
         max {:.3} ms",
        frames.len(),
        ms(at(0.5)),
        ms(at(0.95)),
        ms(at(1.0)),
    );
    // The first row stays in the AccessKit tree as the list's tab stop, so
    // look for the second.
    assert!(
        harness.query_by_label("Seed 2").is_none(),
        "the list scrolled"
    );
}
