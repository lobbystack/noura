//! Seam 1: search's one field. Unfiltered, it lists files by name, then
//! files by contents with the matching text, then commands; with nothing
//! typed, the recent notes and a few commands.

use crate::support;
use std::time::{Duration, Instant};

use app_core::{
    App, Area, Command, EMPTY_COMMANDS, GROUP_LIMIT, Key, KeyPress, PaletteGroup, PaletteItem,
    PaletteStatus, Platform, SEARCH_PAGE, SearchState, command_id,
};
use support::{FakeSystem, Fixture, document};

fn labels(app: &App) -> Vec<String> {
    app.palette_matches()
        .into_iter()
        .map(|command| command.label)
        .collect()
}

fn opened(fixture: &Fixture) -> App {
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    assert!(app.press_key(&KeyPress::new(Key::K).with_cmd(), None, Instant::now()));
    assert!(app.palette().is_some(), "Mod-K opens search");
    app
}

/// Each row's group and what it shows: a file's path or a command's
/// label.
fn rows(app: &App) -> Vec<(PaletteGroup, String)> {
    app.palette_results()
        .into_iter()
        .map(|row| {
            let shown = match &row {
                PaletteItem::Command(command) => command.label.clone(),
                other => other.path().unwrap_or_default().to_owned(),
            };
            (row.group(), shown)
        })
        .collect()
}

/// Poll until the contents search has answered.
fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.palette().unwrap().state == SearchState::Searching {
        assert!(Instant::now() < deadline, "the search never answered");
        std::thread::sleep(Duration::from_millis(2));
        app.poll();
    }
}

#[test]
fn with_nothing_typed_search_lists_the_recent_notes_and_a_few_commands() {
    let fixture = Fixture::new(&[("Plan.md", b"p"), ("Ideas.md", b"i")]);
    let app = opened(&fixture);

    let recent: Vec<String> = app
        .inbox()
        .recent
        .into_iter()
        .map(|note| note.relative_path)
        .collect();
    assert_eq!(recent.len(), 2);
    let commands: Vec<String> = app
        .commands()
        .into_iter()
        .take(EMPTY_COMMANDS)
        .map(|command| command.label)
        .collect();
    let expected: Vec<(PaletteGroup, String)> = recent
        .into_iter()
        .map(|path| (PaletteGroup::Files, path))
        .chain(
            commands
                .into_iter()
                .map(|label| (PaletteGroup::Commands, label)),
        )
        .collect();
    assert_eq!(rows(&app), expected);
    assert_eq!(app.palette().unwrap().query(), "");
    assert_eq!(app.palette().unwrap().selected(), 0);
    assert_eq!(app.palette_status(), PaletteStatus::Results);
}

#[test]
fn typing_lists_files_by_name_then_by_contents_then_commands() {
    let fixture = Fixture::new(&[
        ("salut.md", b"hello\n"),
        ("Letters/Greeting.md", b"Salut tout le monde\n"),
        ("Other.md", b"nothing here\n"),
    ]);
    let mut app = opened(&fixture);

    // The owner's case: a file named salut.
    app.set_palette_query("salut");
    assert_eq!(rows(&app)[0], (PaletteGroup::Files, "salut.md".to_owned()));
    settle(&mut app);
    let found = rows(&app);
    let files: Vec<&str> = found
        .iter()
        .filter(|(group, _)| *group != PaletteGroup::Commands)
        .map(|(_, path)| path.as_str())
        .collect();
    // salut.md shows once, by name, and Greeting by its text.
    assert_eq!(files, ["salut.md", "Letters/Greeting.md"]);
    assert_eq!(found[1].0, PaletteGroup::Contents);
    let PaletteItem::Hit(hit) = &app.palette_results()[1] else {
        panic!("not a contents match");
    };
    assert!(hit.snippet.contains("tout le monde"), "{}", hit.snippet);

    // Commands come last, with their shortcuts.
    app.set_palette_query("sidebar");
    settle(&mut app);
    let last = app.palette_results().pop().unwrap();
    let PaletteItem::Command(command) = last else {
        panic!("not a command");
    };
    assert_eq!(command.label, "Toggle Sidebar");
    assert!(!command.shortcuts.is_empty());

    // Enter runs the command.
    assert!(app.layout().sidebar_shown);
    let at = app.palette_results().len() - 1;
    app.select_palette_row(at);
    assert!(app.run_palette_selection(Instant::now()));
    assert_eq!(app.palette(), None);
    assert!(!app.layout().sidebar_shown);
}

#[test]
fn a_word_inside_a_file_opens_it_and_nothing_says_no_results() {
    let fixture = Fixture::new(&[("Recipe.md", b"Add the saffron last\n")]);
    let mut app = opened(&fixture);

    app.set_palette_query("saffron");
    settle(&mut app);
    assert_eq!(
        rows(&app),
        [(PaletteGroup::Contents, "Recipe.md".to_owned())]
    );
    assert!(app.run_palette_selection(Instant::now()));
    assert_eq!(document(&app).as_deref(), Some("Recipe.md"));
    assert!(app.active_tab().unwrap().is_preview());

    app.toggle_palette();
    app.set_palette_query("zzqx");
    settle(&mut app);
    assert!(rows(&app).is_empty());
    assert_eq!(app.palette_status(), PaletteStatus::NoResults);
}

#[test]
fn each_file_group_stops_at_the_limit_unfiltered() {
    let paths: Vec<String> = (0..GROUP_LIMIT + 3)
        .map(|index| format!("Note {index}.md"))
        .collect();
    let memos: Vec<String> = (0..7).map(|index| format!("Memo {index}.md")).collect();
    let files: Vec<(&str, &[u8])> = paths
        .iter()
        .map(|path| (path.as_str(), b"x".as_slice()))
        .chain(
            memos
                .iter()
                .map(|path| (path.as_str(), b"a note".as_slice())),
        )
        .collect();
    let fixture = Fixture::new(&files);
    let mut app = opened(&fixture);

    app.set_palette_query("note");
    settle(&mut app);
    counted(&mut app);
    let found = app.palette_results();
    let by_name = found
        .iter()
        .filter(|row| matches!(row, PaletteItem::File(_)))
        .count();
    let by_contents = found
        .iter()
        .filter(|row| matches!(row, PaletteItem::Hit(_)))
        .count();
    assert_eq!(by_name, GROUP_LIMIT);
    // The index finds "note" in every title and in the memos' text; those
    // not listed by name show by contents.
    assert_eq!(by_contents, GROUP_LIMIT);
    // Each group ends with a row that shows every file it found.
    let more: Vec<&PaletteItem> = found
        .iter()
        .filter(|row| matches!(row, PaletteItem::More { .. }))
        .collect();
    assert_eq!(
        more,
        [
            &PaletteItem::More {
                group: PaletteGroup::Files,
                total: Some(GROUP_LIMIT + 3)
            },
            &PaletteItem::More {
                group: PaletteGroup::Contents,
                total: Some(GROUP_LIMIT + 3 + 7)
            }
        ]
    );
    assert_eq!(app.palette_total(), None, "unfiltered, the rows count");

    // Choosing one filters search to its group and keeps what you typed.
    let files_more = found
        .iter()
        .position(|row| {
            matches!(
                row,
                PaletteItem::More {
                    group: PaletteGroup::Files,
                    ..
                }
            )
        })
        .unwrap();
    app.select_palette_row(files_more);
    assert!(app.run_palette_selection(Instant::now()));
    let palette = app.palette().expect("search stays open");
    assert_eq!(palette.scope, app_core::SearchScope::Names);
    assert_eq!(palette.query(), "note");
    assert_eq!(app.palette_results().len(), GROUP_LIMIT + 3);
    assert_eq!(app.palette_total(), Some(GROUP_LIMIT + 3));
}

/// Poll until the contents search has counted its files.
fn counted(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.palette().unwrap().hit_count.is_none() {
        assert!(Instant::now() < deadline, "the search never counted");
        std::thread::sleep(Duration::from_millis(2));
        app.poll();
    }
}

#[test]
fn filtered_search_counts_every_file_and_lists_them_a_page_at_a_time() {
    let paths: Vec<String> = (0..120)
        .map(|index| format!("Plum {index:03}.md"))
        .collect();
    let files: Vec<(&str, &[u8])> = paths
        .iter()
        .map(|path| (path.as_str(), b"plums".as_slice()))
        .collect();
    let fixture = Fixture::new(&files);
    let mut app = opened(&fixture);
    let shown = |app: &App| app.palette_results().len();

    // By name: every match ranks at once; the list grows a page at a time.
    app.open_palette(app_core::SearchScope::Names);
    app.set_palette_query("plum");
    assert_eq!(app.palette_total(), Some(120));
    assert_eq!(shown(&app), SEARCH_PAGE);
    app.show_more_results();
    assert_eq!(shown(&app), 2 * SEARCH_PAGE);
    // Down at the last row lists the rest.
    app.move_palette_selection(1_000);
    assert_eq!(shown(&app), 120);
    app.move_palette_selection(1_000);
    assert_eq!(app.palette().unwrap().selected(), 119);
    app.show_more_results();
    assert_eq!(shown(&app), 120);

    // By contents: the first page, then the count, then more pages.
    app.set_palette_scope(app_core::SearchScope::Contents);
    app.set_palette_query("plums");
    settle(&mut app);
    assert_eq!(shown(&app), SEARCH_PAGE);
    counted(&mut app);
    assert_eq!(app.palette_total(), Some(120));
    app.show_more_results();
    // While the page is on its way, asking again changes nothing.
    app.show_more_results();
    let deadline = Instant::now() + Duration::from_secs(20);
    while shown(&app) < 2 * SEARCH_PAGE {
        assert!(Instant::now() < deadline, "the next page never came");
        std::thread::sleep(Duration::from_millis(2));
        app.poll();
    }
    assert_eq!(shown(&app), 2 * SEARCH_PAGE);
    app.show_more_results();
    let deadline = Instant::now() + Duration::from_secs(20);
    while shown(&app) < 120 {
        assert!(Instant::now() < deadline, "the last page never came");
        std::thread::sleep(Duration::from_millis(2));
        app.poll();
    }
    let mut listed: Vec<String> = app
        .palette_results()
        .iter()
        .filter_map(|row| row.path().map(str::to_owned))
        .collect();
    listed.sort();
    listed.dedup();
    assert_eq!(listed.len(), 120, "each file once");

    // Typing again starts from the first page.
    app.set_palette_query("plum");
    settle(&mut app);
    assert_eq!(shown(&app), SEARCH_PAGE);
}

#[test]
fn commands_filter_by_the_letters_of_the_label_in_order_ignoring_case_and_accents() {
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
fn arrows_move_the_highlight_across_groups() {
    let fixture = Fixture::new(&[("Close notes.md", b"x")]);
    let mut app = opened(&fixture);
    app.set_palette_query("close");
    settle(&mut app);
    assert_eq!(
        rows(&app),
        [
            (PaletteGroup::Files, "Close notes.md".to_owned()),
            (PaletteGroup::Commands, "Close Tab".to_owned()),
            (PaletteGroup::Commands, "Reopen Closed Tab".to_owned()),
            (PaletteGroup::Commands, "Close Window".to_owned()),
        ]
    );

    app.move_palette_selection(-1);
    assert_eq!(app.palette().unwrap().selected(), 0);
    app.move_palette_selection(1);
    assert_eq!(app.palette().unwrap().selected(), 1);
    app.move_palette_selection(100);
    assert_eq!(app.palette().unwrap().selected(), 3);

    // Typing again highlights the first row.
    app.set_palette_query("close ");
    assert_eq!(app.palette().unwrap().selected(), 0);

    // Enter runs the highlighted row, not the first.
    app.move_palette_selection(3);
    app.run_palette_selection(Instant::now());
    assert_eq!(app.palette(), None);
    assert!(app.is_closed());
}

#[test]
fn contents_that_arrive_later_keep_the_highlighted_command() {
    let fixture = Fixture::new(&[("Pantry.md", b"Set the table\n")]);
    let mut app = opened(&fixture);
    app.set_palette_query("tab");
    // The contents search hasn't answered: only commands show.
    assert!(
        rows(&app)
            .iter()
            .all(|(group, _)| *group == PaletteGroup::Commands)
    );
    app.move_palette_selection(1);
    let highlighted = rows(&app)[1].clone();
    settle(&mut app);

    assert_eq!(rows(&app)[0].0, PaletteGroup::Contents);
    let selected = app.palette().unwrap().selected();
    assert_eq!(rows(&app)[selected], highlighted);
}

#[test]
fn mod_k_again_or_closing_leaves_search() {
    let fixture = Fixture::new(&[]);
    let mut app = opened(&fixture);

    // Search's own field doesn't keep Mod-K.
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
