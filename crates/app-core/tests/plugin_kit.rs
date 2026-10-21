//! Seam 1: the UI kit's events and rows between Noura and the kit test
//! plugin. Input values of the wrong form never reach the plugin, a
//! virtual list's rows come from the plugin in batches as they're asked
//! for, and rows that break the rules show why in place of the list.
//! Below them, the date and date-and-time fields' values.

mod support;

use app_core::{
    App, Field, InputValue, PluginEvent, Submission, date_time_fields, date_time_value, date_value,
};
use support::{
    Fixture,
    plugins::{self, PluginFolder},
};

const FORM: &str = "plugin:kit:form";
const SEEDS: &str = "plugin:kit:seeds";
const BROKEN_ROWS: &str = "plugin:kit:broken-rows";

/// The kit plugin, installed and on in a new workspace, or `None` when it
/// isn't built.
fn with_kit() -> Option<(Fixture, App, PluginFolder)> {
    let kit = plugins::built("kit_plugin")?;
    let folder = PluginFolder::new(
        &plugins::manifest("kit", "plugin.wasm", &["name = \"views\""]),
        "plugin.wasm",
        &kit,
    );
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install(&mut app, folder.path());
    assert!(app.set_plugin_enabled("kit", true));
    Some((fixture, app, folder))
}

/// What the kit plugin says the last event was on `page`.
fn last_event(app: &App, page: &str) -> String {
    let texts = plugins::texts(app, page).expect("the page draws");
    texts
        .iter()
        .find_map(|text| text.strip_prefix("Last event: "))
        .expect("the page says what the last event was")
        .to_owned()
}

fn field(node: &str, value: InputValue) -> Field {
    Field {
        node: node.into(),
        value,
    }
}

fn changed(node: &str, value: InputValue) -> PluginEvent {
    PluginEvent::Changed(field(node, value))
}

#[test]
fn a_change_or_a_submit_with_a_date_of_the_wrong_form_never_reaches_the_plugin() {
    let Some((_fixture, mut app, _folder)) = with_kit() else {
        return;
    };

    for refused in [
        changed("sown", InputValue::Date(Some("2026-02-30".into()))),
        changed("sown", InputValue::Date(Some("21/10/2026".into()))),
        changed(
            "transplant",
            InputValue::DateTime(Some("2026-10-21 09:30".into())),
        ),
        changed(
            "transplant",
            InputValue::DateTime(Some("2026-10-21T09:30:00".into())),
        ),
        PluginEvent::Submitted(Submission {
            node: "new-plant".into(),
            fields: vec![
                field("name", InputValue::Text("Chard".into())),
                field("sown", InputValue::Date(Some("2026-13-01".into()))),
            ],
        }),
    ] {
        app.send_plugin_event(FORM, &refused);
        assert_eq!(last_event(&app, FORM), "none", "{refused:?}");
    }

    app.send_plugin_event(
        FORM,
        &changed("sown", InputValue::Date(Some("2026-10-21".into()))),
    );
    assert_eq!(last_event(&app, FORM), "changed sown date 2026-10-21");
    app.send_plugin_event(
        FORM,
        &changed(
            "transplant",
            InputValue::DateTime(Some("2026-10-21T10:15:00+02:00".into())),
        ),
    );
    assert_eq!(
        last_event(&app, FORM),
        "changed transplant date-time 2026-10-21T10:15:00+02:00"
    );
    app.send_plugin_event(FORM, &changed("sown", InputValue::Date(None)));
    assert_eq!(last_event(&app, FORM), "changed sown date none");
    app.send_plugin_event(
        FORM,
        &PluginEvent::Submitted(Submission {
            node: "new-plant".into(),
            fields: vec![
                field("name", InputValue::Text("Chard".into())),
                field("sown", InputValue::Date(Some("2026-10-21".into()))),
                field(
                    "transplant",
                    InputValue::DateTime(Some("2026-10-21T10:15:00Z".into())),
                ),
            ],
        }),
    );
    assert_eq!(
        last_event(&app, FORM),
        "submitted new-plant: name text \"Chard\", sown date 2026-10-21, \
         transplant date-time 2026-10-21T10:15:00Z"
    );
}

/// Each row in `range` of the Seeds list as `label (detail)`, which names
/// the fetch that returned it.
fn seeds(app: &mut App, range: std::ops::Range<u32>) -> Vec<String> {
    app.plugin_rows(SEEDS, "seeds", range)
        .expect("the rows show")
        .into_iter()
        .map(|row| {
            let row = row.expect("the plugin returned the row");
            format!(
                "{} ({})",
                row.item.label,
                row.item.detail.as_deref().unwrap_or("-")
            )
        })
        .collect()
}

#[test]
fn a_virtual_list_fetches_its_rows_in_batches_of_64_once_until_the_page_draws_again() {
    let Some((_fixture, mut app, _folder)) = with_kit() else {
        return;
    };

    let first = seeds(&mut app, 0..3);
    assert_eq!(
        first,
        ["Seed 1 (Batch 1)", "Seed 2 (Batch 1)", "Seed 3 (Batch 1)"]
    );
    // Rows 60 to 63 came with the first batch, and the next batch starts
    // at 64.
    assert_eq!(
        seeds(&mut app, 62..66),
        [
            "Seed 63 (Batch 1)",
            "Seed 64 (Batch 1)",
            "Seed 65 (Batch 2)",
            "Seed 66 (Batch 2)",
        ]
    );
    // Rows already fetched aren't asked for again.
    assert_eq!(seeds(&mut app, 0..3), first);
    // A range past the end stops at the last row.
    assert_eq!(
        seeds(&mut app, 9_998..10_050),
        ["Seed 9999 (Batch 3)", "Seed 10000 (Batch 3)"]
    );
    assert_eq!(seeds(&mut app, 10_000..10_010), Vec::<String>::new());

    // A redraw asks for the rows again.
    app.press_plugin_button(SEEDS, "anything");
    assert_eq!(seeds(&mut app, 0..1), ["Seed 1 (Batch 4)"]);

    assert_eq!(
        app.plugin_rows(SEEDS, "nothing", 0..1).err(),
        Some("The page has no virtual list nothing.".to_owned())
    );
}

#[test]
fn rows_that_break_the_rules_show_why_until_the_page_draws_again() {
    let Some((_fixture, mut app, _folder)) = with_kit() else {
        return;
    };
    let reason = "The plugin returned 65 rows when Noura asked for 64.".to_owned();
    let asked = |app: &App| {
        plugins::texts(app, BROKEN_ROWS)
            .expect("the page draws")
            .into_iter()
            .find_map(|text| text.strip_prefix("Asked for rows: ").map(str::to_owned))
            .expect("the page says which rows Noura asked for")
    };

    assert_eq!(
        app.plugin_rows(BROKEN_ROWS, "broken", 0..10).err(),
        Some(reason.clone())
    );
    // The reason stays without asking the plugin again, even for rows of
    // another batch.
    assert_eq!(
        app.plugin_rows(BROKEN_ROWS, "broken", 64..70).err(),
        Some(reason.clone())
    );
    // The plugin's other lists and pages keep working.
    assert_eq!(seeds(&mut app, 0..1), ["Seed 1 (Batch 1)"]);
    app.press_plugin_button(FORM, "anything");
    assert_eq!(last_event(&app, FORM), "pressed anything");
    assert_eq!(asked(&app), "0 to 63");

    // A redraw asks again.
    assert_eq!(
        app.plugin_rows(BROKEN_ROWS, "broken", 0..10).err(),
        Some(reason)
    );
    app.press_plugin_button(BROKEN_ROWS, "anything");
    assert_eq!(asked(&app), "0 to 63, 0 to 63");
}

#[test]
fn a_date_field_holds_a_whole_date_or_nothing() {
    for (typed, value) in [
        ("", Some(None)),
        ("   ", Some(None)),
        ("2026-10-21", Some(Some("2026-10-21"))),
        (" 2026-10-21 ", Some(Some("2026-10-21"))),
        ("2024-02-29", Some(Some("2024-02-29"))),
        ("2026-10-2", None),
        ("2026-02-30", None),
        ("2026-13-01", None),
        ("21/10/2026", None),
        ("2026-10-21T09:30:00Z", None),
    ] {
        assert_eq!(
            date_value(typed),
            value.map(|date| date.map(str::to_owned)),
            "{typed:?}"
        );
    }
}

#[test]
fn a_date_and_time_shows_as_a_date_field_and_a_time_field_in_its_own_offset() {
    for (value, fields) in [
        ("2026-10-21T09:30:00-04:00", Some(("2026-10-21", "09:30"))),
        (
            "2026-10-21T09:30:15+02:00",
            Some(("2026-10-21", "09:30:15")),
        ),
        ("2026-10-21T23:05:00Z", Some(("2026-10-21", "23:05"))),
        ("2026-10-21T09:30:00", None),
        ("2026-10-21", None),
        ("soon", None),
    ] {
        assert_eq!(
            date_time_fields(value),
            fields.map(|(date, time)| (date.to_owned(), time.to_owned())),
            "{value:?}"
        );
    }
}

#[test]
fn a_date_and_time_keeps_the_offset_it_held() {
    let held = Some("2026-05-01T09:30:00-04:00");
    for (date, time, value) in [
        (
            "2026-10-21",
            "10:15",
            Some(Some("2026-10-21T10:15:00-04:00")),
        ),
        (
            " 2026-10-21 ",
            " 10:15:30 ",
            Some(Some("2026-10-21T10:15:30-04:00")),
        ),
        ("", "", Some(None)),
        ("2026-10-21", "", None),
        ("", "10:15", None),
        ("2026-10-21", "9:30", None),
        ("2026-10-21", "24:00", None),
        ("2026-10-21", "10:15:3", None),
        ("2026-02-30", "10:15", None),
    ] {
        assert_eq!(
            date_time_value(date, time, held),
            value.map(|at| at.map(str::to_owned)),
            "{date:?} {time:?}"
        );
    }
    assert_eq!(
        date_time_value("2026-10-21", "10:15", Some("2026-05-01T09:30:00+05:30")),
        Some(Some("2026-10-21T10:15:00+05:30".to_owned()))
    );
}

#[test]
fn a_date_and_time_that_held_nothing_takes_this_devices_offset() {
    let at = date_time_value("2026-10-21", "10:15", None)
        .expect("a whole date and time")
        .expect("a value");

    assert!(workspace_format::is_instant(&at), "{at}");
    assert_eq!(
        date_time_fields(&at),
        Some(("2026-10-21".to_owned(), "10:15".to_owned()))
    );
}
