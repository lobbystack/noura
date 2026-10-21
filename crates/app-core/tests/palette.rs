//! Seam 1: the command palette's state and filtering.

mod support;

use std::time::Instant;

use app_core::{App, Area, Command, Key, KeyPress, Platform, command_id};
use support::{FakeSystem, Fixture};

fn labels(app: &App) -> Vec<String> {
    app.palette_matches()
        .into_iter()
        .map(|command| command.label)
        .collect()
}

fn opened(fixture: &Fixture) -> App {
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    assert!(app.press_key(&KeyPress::new(Key::K).with_cmd(), None, Instant::now()));
    assert!(app.palette().is_some(), "Mod-K opens the palette");
    app
}

#[test]
fn the_palette_lists_every_command_until_you_type() {
    let fixture = Fixture::new(&[]);
    let app = opened(&fixture);

    let listed: Vec<String> = app.palette_matches().into_iter().map(|c| c.id).collect();
    let every: Vec<String> = app.commands().into_iter().map(|c| c.id).collect();
    assert_eq!(listed, every);
    assert_eq!(app.palette().unwrap().query, "");
    assert_eq!(app.palette().unwrap().selected, 0);
}

#[test]
fn typing_filters_by_the_letters_of_the_label_in_order_ignoring_case_and_accents() {
    let fixture = Fixture::new(&[]);
    let mut app = opened(&fixture);
    let command = Command::new("test.resume", "Ouvrir le résumé", Area::Anywhere);
    app.register_command(command, |_| {}).unwrap();

    app.set_palette_query("next tab");
    assert_eq!(labels(&app).first().map(String::as_str), Some("Next Tab"));

    app.set_palette_query("tgsb");
    assert_eq!(labels(&app), ["Toggle Sidebar"]);

    app.set_palette_query("RESUME");
    assert_eq!(labels(&app), ["Ouvrir le résumé"]);

    app.set_palette_query("zzzz");
    assert!(labels(&app).is_empty());
}

#[test]
fn enter_runs_the_highlighted_command_and_closes_the_palette() {
    let fixture = Fixture::new(&[]);
    let mut app = opened(&fixture);
    assert!(app.layout().sidebar_shown);

    app.set_palette_query("sidebar");
    assert!(app.run_palette_selection(Instant::now()));

    assert_eq!(app.palette(), None);
    assert!(!app.layout().sidebar_shown);
}

#[test]
fn arrows_move_the_highlight_within_the_list() {
    let fixture = Fixture::new(&[]);
    let mut app = opened(&fixture);
    app.set_palette_query("tab");
    let matches = labels(&app);
    assert!(matches.len() >= 2, "{matches:?}");

    app.move_palette_selection(-1);
    assert_eq!(app.palette().unwrap().selected, 0);
    app.move_palette_selection(1);
    assert_eq!(app.palette().unwrap().selected, 1);
    app.move_palette_selection(100);
    assert_eq!(app.palette().unwrap().selected, matches.len() - 1);

    // Typing again highlights the best match.
    app.set_palette_query("tabs");
    assert_eq!(app.palette().unwrap().selected, 0);

    // Enter runs the highlighted row, not the first.
    app.set_palette_query("close");
    assert_eq!(labels(&app), ["Close Tab", "Close Window"]);
    app.move_palette_selection(1);
    app.run_palette_selection(Instant::now());
    assert_eq!(app.palette(), None);
    assert!(app.is_closed());
}

#[test]
fn mod_k_again_or_closing_leaves_the_palette() {
    let fixture = Fixture::new(&[]);
    let mut app = opened(&fixture);

    // The palette's own field doesn't keep Mod-K.
    assert!(app.press_key(
        &KeyPress::new(Key::K).with_cmd(),
        Some(Area::Writing),
        Instant::now()
    ));
    assert_eq!(app.palette(), None);

    app.run_command(
        command_id::COMMAND_PALETTE,
        app_core::CommandSource::Button,
        Instant::now(),
    );
    assert!(app.palette().is_some());
    app.close_palette();
    assert_eq!(app.palette(), None);
}
