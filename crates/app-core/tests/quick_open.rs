//! Seam 1: search finds any file in the tree by name. A file matches
//! when its name, or its path, holds the typed letters in order. Name
//! matches beat folder matches, exact names beat prefixes, and
//! consecutive letters and word starts beat scattered letters.

mod support;

use std::time::Instant;

use app_core::{
    App, Area, CommandSource, Key, KeyPress, PaletteItem, Platform, SearchScope, command_id,
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

/// The paths search filtered to names lists for `query`, best first.
fn matches(app: &mut App, query: &str) -> Vec<String> {
    app.open_palette(SearchScope::Names);
    app.set_palette_query(query);
    app.palette_results()
        .iter()
        .map(|row| row.path().expect("a file").to_owned())
        .collect()
}

/// The names and folders search lists by name.
fn names(app: &App) -> Vec<(String, String)> {
    app.palette()
        .expect("search is open")
        .names
        .iter()
        .map(|found| (found.name.clone(), found.folder.clone()))
        .collect()
}

fn best(app: &mut App, query: &str) -> String {
    matches(app, query)
        .first()
        .cloned()
        .unwrap_or_else(|| panic!("nothing matches {query}"))
}

#[test]
fn search_finds_files_whose_name_holds_the_letters_in_order() {
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
    // The path counts with its extension, as the tree's file names have it.
    assert_eq!(best(&mut app, "groceries.md"), "Groceries.md");
    assert_eq!(best(&mut app, "brief.md"), "Projects/Launch brief.md");
    assert_eq!(best(&mut app, "report.txt"), "Docs/report.txt");
    assert!(matches(&mut app, "xyz").is_empty());
    // Blank, it matches nothing and lists the recent notes instead.
    let recent: Vec<String> = app
        .inbox()
        .recent
        .into_iter()
        .map(|note| note.relative_path)
        .collect();
    assert_eq!(matches(&mut app, "  "), recent);
    assert!(names(&app).is_empty());
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
fn search_lists_every_kind_of_file_and_shows_notes_without_md() {
    let fixture = fixture();
    let mut app = fixture.app();

    assert_eq!(best(&mut app, "map"), "Images/Map.png");
    assert_eq!(best(&mut app, "report"), "Docs/report.txt");
    app.set_palette_query("groceries");
    assert_eq!(names(&app)[0].0, "Groceries");
    app.set_palette_query("map");
    assert_eq!(names(&app)[0], ("Map.png".to_owned(), "Images".to_owned()));
}

#[test]
fn enter_opens_the_best_match_as_the_file_tree_does_and_search_opens_again() {
    let fixture = fixture();
    let mut app = fixture.app();

    for (query, path) in [
        ("groc", "Groceries.md"),
        ("meeting n", "Meeting notes.md"),
        ("groc", "Groceries.md"),
    ] {
        app.open_palette(SearchScope::Names);
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
fn search_by_name_follows_files_added_outside_noura() {
    let fixture = fixture();
    let mut app = fixture.app();

    app.open_palette(SearchScope::Names);
    app.set_palette_query("budget");
    support::write(&fixture.root, "Budget.md", b"b");
    app.rescan();

    assert_eq!(names(&app)[0].0, "Budget");
}

fn scope(app: &App) -> Option<SearchScope> {
    app.palette().map(|palette| palette.scope)
}

#[test]
fn mod_k_mod_o_and_mod_shift_f_open_one_search_with_a_filter() {
    let fixture = fixture();
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let commands = app.commands();
    let label = |id: &str| {
        commands
            .iter()
            .find(|command| command.id == id)
            .map(|command| command.label.as_str())
    };
    assert_eq!(label(command_id::COMMAND_PALETTE), Some("Search…"));
    assert_eq!(label(command_id::QUICK_OPEN), Some("Search File Names…"));
    assert_eq!(label(command_id::SEARCH), Some("Search File Contents…"));

    let mod_o = KeyPress::new(Key::O).with_cmd();
    let mod_shift_f = KeyPress::new(Key::F).with_cmd().with_shift();
    let mod_k = KeyPress::new(Key::K).with_cmd();
    let now = Instant::now();
    assert!(app.press_key(&mod_o, None, now));
    assert_eq!(scope(&app), Some(SearchScope::Names));
    app.set_palette_query("groc");

    // From the field, Mod-Shift-F changes the filter and keeps the text,
    // Mod-O changes it back, and removing it lists everything.
    assert!(app.press_key(&mod_shift_f, Some(Area::Writing), now));
    assert_eq!(scope(&app), Some(SearchScope::Contents));
    assert_eq!(app.palette().unwrap().query(), "groc");
    assert!(app.press_key(&mod_o, Some(Area::Writing), now));
    assert_eq!(scope(&app), Some(SearchScope::Names));
    app.set_palette_scope(SearchScope::All);
    assert_eq!(scope(&app), Some(SearchScope::All));
    assert!(matches!(app.palette_results()[0], PaletteItem::File(_)));

    // Mod-K closes search whatever it shows, then opens it unfiltered.
    assert!(app.press_key(&mod_k, Some(Area::Writing), now));
    assert_eq!(scope(&app), None);
    assert!(app.press_key(&mod_k, None, now));
    assert_eq!(scope(&app), Some(SearchScope::All));

    // Search runs its own filtered variants like any other command.
    app.set_palette_query("search file names");
    assert!(app.run_palette_selection(now));
    assert_eq!(scope(&app), Some(SearchScope::Names));
    app.set_palette_query("groc");
    assert!(app.run_palette_selection(now));
    assert_eq!(document(&app).as_deref(), Some("Groceries.md"));

    // So does the File menu, a moment after the last key press.
    let later = now + std::time::Duration::from_secs(1);
    assert!(app.run_command(command_id::SEARCH, CommandSource::Menu, later));
    assert_eq!(scope(&app), Some(SearchScope::Contents));
}

#[test]
fn without_a_workspace_search_stays_closed() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch_with(app_data.path(), FakeSystem::on(Platform::MacOs));
    support::settle(&mut app);
    assert!(app.workspace().is_none());

    let now = Instant::now();
    assert!(!app.press_key(&KeyPress::new(Key::O).with_cmd(), None, now));
    app.run_command(command_id::QUICK_OPEN, CommandSource::Menu, now);
    app.run_command(command_id::SEARCH, CommandSource::Menu, now);
    app.run_command(command_id::COMMAND_PALETTE, CommandSource::Menu, now);
    assert_eq!(scope(&app), None);
}
