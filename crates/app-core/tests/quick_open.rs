//! Seam 1: quick open finds any file in the tree by name. A file matches
//! when its name, or its folder and name, holds the typed letters in
//! order. Name matches beat folder matches, exact names beat prefixes, and
//! consecutive letters and word starts beat scattered letters.

mod support;

use app_core::{App, Palette};
use support::{Fixture, document};

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
        assert!(app.open_palette_selection());
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
    assert!(app.open_palette_selection());

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
