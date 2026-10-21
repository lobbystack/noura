//! Seam 1: quick open finds any file in the tree by name. A file matches
//! when its name, or its folder and name, holds the typed letters in
//! order. Name matches beat folder matches, exact names beat prefixes, and
//! consecutive letters and word starts beat scattered letters.

mod support;

use std::time::Instant;

use app_core::{
    App, Area, CommandSource, Key, KeyPress, Palette, PaletteMode, Platform, command_id,
};
use support::{FakeSystem, Fixture, document};

const FILES: [&str; 16] = [
    "Groceries.md",
    "Background check.md",
    "Meeting notes.md",
    "Meeting archive.md",
    "Projects/Launch brief.md",
    "Archive/launch-notes-2023.md",
    "Daily/2026-09-28.md",
    "Reading/Books/The Pragmatic Programmer.pdf",
    "lab.md",
    "Launch.md",
    "Images/Map.png",
    "Docs/report.txt",
    "Café menu.md",
    "Grocery list.md",
    "Gigantic hill.md",
    "Lights install.md",
];

fn fixture() -> Fixture {
    let files: Vec<(&str, &[u8])> = FILES.iter().map(|path| (*path, b"x".as_slice())).collect();
    Fixture::new(&files)
}

/// The paths quick open lists for `query`, best first.
fn matches(app: &mut App, query: &str) -> Vec<String> {
    app.open_quick_open();
    app.set_palette_query(query);
    match app.palette() {
        Some(Palette::QuickOpen(quick_open)) => quick_open
            .matches
            .iter()
            .map(|found| found.relative_path.clone())
            .collect(),
        other => panic!("quick open isn't open: {other:?}"),
    }
}

fn best(app: &mut App, query: &str) -> String {
    matches(app, query)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("nothing matches {query}"))
}

#[test]
fn quick_open_finds_files_whose_name_holds_the_letters_in_order() {
    let fixture = fixture();
    let mut app = fixture.app();

    assert_eq!(best(&mut app, "groc"), "Groceries.md");
    assert_eq!(best(&mut app, "meeting n"), "Meeting notes.md");
    let meetings = matches(&mut app, "meeting n");
    assert!(meetings.contains(&"Meeting archive.md".to_owned()));
    assert_eq!(best(&mut app, "lb"), "Projects/Launch brief.md");
    assert_eq!(
        best(&mut app, "prag prog"),
        "Reading/Books/The Pragmatic Programmer.pdf"
    );
    assert_eq!(best(&mut app, "cafe"), "Café menu.md");
    assert!(matches(&mut app, "xyz").is_empty());
    assert!(matches(&mut app, "  ").is_empty());
}

#[test]
fn name_matches_beat_folder_matches_and_exact_names_beat_prefixes() {
    let fixture = fixture();
    let mut app = fixture.app();

    assert_eq!(
        matches(&mut app, "launch")[..3],
        [
            "Launch.md",
            "Projects/Launch brief.md",
            "Archive/launch-notes-2023.md"
        ]
    );
    // Only the folder holds "books".
    assert_eq!(
        matches(&mut app, "books"),
        ["Reading/Books/The Pragmatic Programmer.pdf"]
    );
    let projects = matches(&mut app, "proj launch");
    assert_eq!(projects[0], "Projects/Launch brief.md");
}

#[test]
fn consecutive_letters_and_word_starts_beat_scattered_letters() {
    let fixture = fixture();
    let mut app = fixture.app();

    let list = matches(&mut app, "list");
    assert_eq!(list[0], "Grocery list.md");
    assert!(list.contains(&"Lights install.md".to_owned()));
    let starts = matches(&mut app, "gl");
    assert_eq!(starts[0], "Grocery list.md");
    assert!(starts.contains(&"Gigantic hill.md".to_owned()));
}

#[test]
fn quick_open_lists_every_kind_of_file_and_shows_notes_without_md() {
    let fixture = fixture();
    let mut app = fixture.app();

    assert_eq!(best(&mut app, "map"), "Images/Map.png");
    assert_eq!(best(&mut app, "report"), "Docs/report.txt");
    app.set_palette_query("groceries");
    let Some(Palette::QuickOpen(quick_open)) = app.palette() else {
        panic!("quick open isn't open");
    };
    assert_eq!(quick_open.matches[0].name, "Groceries");
    app.set_palette_query("map");
    let Some(Palette::QuickOpen(quick_open)) = app.palette() else {
        panic!("quick open isn't open");
    };
    assert_eq!(quick_open.matches[0].name, "Map.png");
    assert_eq!(quick_open.matches[0].folder, "Images");
}

#[test]
fn enter_opens_the_best_match_as_the_file_tree_does_and_quick_open_opens_again() {
    let fixture = fixture();
    let mut app = fixture.app();

    for (query, path) in [
        ("groc", "Groceries.md"),
        ("meeting n", "Meeting notes.md"),
        ("groc", "Groceries.md"),
    ] {
        app.open_quick_open();
        app.set_palette_query(query);
        assert!(app.run_palette_selection(Instant::now()));
        assert!(app.palette().is_none());
        assert_eq!(document(&app).as_deref(), Some(path));
        // A preview tab, which the next file replaces.
        assert!(app.active_tab().unwrap().is_preview());
        assert_eq!(app.tabs().len(), 1);
    }
}

#[test]
fn the_arrow_keys_pick_another_match() {
    let fixture = fixture();
    let mut app = fixture.app();

    let listed = matches(&mut app, "meeting");
    assert_eq!(listed.len(), 2);
    // Down past the last match stops there.
    app.move_palette_selection(5);
    app.move_palette_selection(-1);
    app.move_palette_selection(-1);
    app.move_palette_selection(1);
    assert!(app.run_palette_selection(Instant::now()));

    assert_eq!(document(&app).as_deref(), Some(listed[1].as_str()));
}

#[test]
fn quick_open_follows_files_added_outside_noura() {
    let fixture = fixture();
    let mut app = fixture.app();

    app.open_quick_open();
    app.set_palette_query("budget");
    support::write(&fixture.root, "Budget.md", b"b");
    app.rescan();

    let Some(Palette::QuickOpen(quick_open)) = app.palette() else {
        panic!("quick open isn't open");
    };
    assert_eq!(quick_open.matches[0].relative_path, "Budget.md");
}

fn mode(app: &App) -> Option<PaletteMode> {
    app.palette().map(Palette::mode)
}

#[test]
fn quick_open_and_search_are_commands_with_the_default_keys() {
    let fixture = fixture();
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let commands = app.commands();
    let label = |id: &str| {
        commands
            .iter()
            .find(|command| command.id == id)
            .map(|command| command.label.as_str())
    };
    assert_eq!(label(command_id::QUICK_OPEN), Some("Quick Open…"));
    assert_eq!(label(command_id::SEARCH), Some("Search Files…"));

    let mod_o = KeyPress::new(Key::O).with_cmd();
    let mod_shift_f = KeyPress::new(Key::F).with_cmd().with_shift();
    let mod_k = KeyPress::new(Key::K).with_cmd();
    let now = Instant::now();
    assert!(app.press_key(&mod_o, None, now));
    assert_eq!(mode(&app), Some(PaletteMode::QuickOpen));
    app.set_palette_query("groc");

    // One overlay: from its field, Mod-Shift-F switches it to search with
    // an empty field, and Mod-O back to quick open.
    assert!(app.press_key(&mod_shift_f, Some(Area::Writing), now));
    assert_eq!(mode(&app), Some(PaletteMode::Search));
    assert_eq!(app.palette().unwrap().query(), "");
    assert!(app.press_key(&mod_o, Some(Area::Writing), now));
    assert_eq!(mode(&app), Some(PaletteMode::QuickOpen));

    // Mod-K closes the palette whatever it shows, then opens commands.
    assert!(app.press_key(&mod_k, Some(Area::Writing), now));
    assert_eq!(mode(&app), None);
    assert!(app.press_key(&mod_k, None, now));
    assert_eq!(mode(&app), Some(PaletteMode::Commands));

    // The command palette runs Quick Open like any other command.
    app.set_palette_query("quick open");
    assert!(app.run_palette_selection(now));
    assert_eq!(mode(&app), Some(PaletteMode::QuickOpen));
    app.set_palette_query("groc");
    assert!(app.run_palette_selection(now));
    assert_eq!(document(&app).as_deref(), Some("Groceries.md"));

    // So does the File menu, a moment after the last key press.
    let later = now + std::time::Duration::from_secs(1);
    assert!(app.run_command(command_id::SEARCH, CommandSource::Menu, later));
    assert_eq!(mode(&app), Some(PaletteMode::Search));
}

#[test]
fn without_a_workspace_quick_open_and_search_stay_closed() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch_with(app_data.path(), FakeSystem::on(Platform::MacOs));
    support::settle(&mut app);
    assert!(app.workspace().is_none());

    let now = Instant::now();
    assert!(!app.press_key(&KeyPress::new(Key::O).with_cmd(), None, now));
    app.run_command(command_id::QUICK_OPEN, CommandSource::Menu, now);
    app.run_command(command_id::SEARCH, CommandSource::Menu, now);
    assert_eq!(mode(&app), None);
}
