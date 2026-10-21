//! Seam 1: search as the palette runs it, off the UI thread. Results from
//! an older query never show under a newer one, clearing the query clears
//! the results, and a broken index says so.

mod support;

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};

use app_core::{App, Change, OpenAs, Palette, SearchState};
use support::{Fixture, document};

fn fixture() -> Fixture {
    Fixture::new(&[
        ("Recipe.md", b"Add the saffron last\n"),
        ("Spices/Pepper.md", b"Black pepper and white pepper\n"),
        ("Pantry.md", b"Salt, pepper, saffron\n"),
    ])
}

/// What the search shows now.
fn state(app: &App) -> SearchState {
    match app.palette() {
        Some(Palette::Search(search)) => search.state.clone(),
        other => panic!("search isn't open: {other:?}"),
    }
}

/// The titles search shows, if it shows hits.
fn shown(app: &App) -> Option<Vec<String>> {
    match state(app) {
        SearchState::Done(hits) => Some(hits.into_iter().map(|hit| hit.title).collect()),
        _ => None,
    }
}

/// Counts the answers that arrived, taken or not.
fn count_answers(app: &App) -> Arc<AtomicUsize> {
    let answers = Arc::new(AtomicUsize::new(0));
    let counter = answers.clone();
    app.subscribe(move |change| {
        if change == Change::WorkFinished {
            counter.fetch_add(1, Ordering::SeqCst);
        }
    });
    answers
}

fn wait_for(answers: &AtomicUsize, count: usize) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while answers.load(Ordering::SeqCst) < count {
        assert!(Instant::now() < deadline, "the search never answered");
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Poll until the search has answered, checking after every poll that
/// nothing `forbidden` shows.
fn settle(app: &mut App, forbidden: &str) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        if let Some(titles) = shown(app) {
            assert!(!titles.iter().any(|title| title == forbidden), "{titles:?}");
            return titles;
        }
        assert!(
            matches!(state(app), SearchState::Searching),
            "{:?}",
            state(app)
        );
        assert!(Instant::now() < deadline, "the search never answered");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn search_runs_in_the_background_and_shows_its_hits() {
    let fixture = fixture();
    let mut app = fixture.app();

    app.open_search();
    assert_eq!(state(&app), SearchState::Idle);
    app.set_palette_query("saffron");
    assert_eq!(state(&app), SearchState::Searching);
    let mut titles = settle(&mut app, "");
    titles.sort();

    assert_eq!(titles, ["Pantry", "Recipe"]);
}

#[test]
fn results_from_an_older_query_never_show_under_a_newer_one() {
    let fixture = fixture();
    let mut app = fixture.app();
    let answers = count_answers(&app);

    app.open_search();
    app.set_palette_query("last");
    // The answer for "last" arrives but waits to be taken.
    wait_for(&answers, 1);
    app.set_palette_query("black");

    assert_eq!(settle(&mut app, "Recipe"), ["Pepper"]);
}

#[test]
fn typing_faster_than_search_answers_shows_only_the_last_query() {
    let fixture = fixture();
    let mut app = fixture.app();

    app.open_search();
    for typed in ["s", "sa", "saf", "saff", "saffron l"] {
        app.set_palette_query(typed);
        app.poll();
    }

    assert_eq!(settle(&mut app, "Pantry"), ["Recipe"]);
}

#[test]
fn clearing_the_query_clears_the_results() {
    let fixture = fixture();
    let mut app = fixture.app();
    let answers = count_answers(&app);

    app.open_search();
    app.set_palette_query("pepper");
    settle(&mut app, "");
    app.set_palette_query("saffron");
    wait_for(&answers, 2);
    app.set_palette_query("  ");
    assert_eq!(state(&app), SearchState::Idle);
    app.poll();

    assert_eq!(state(&app), SearchState::Idle);
}

#[test]
fn a_changed_filter_searches_again() {
    let fixture = fixture();
    let mut app = fixture.app();

    app.open_search();
    app.set_palette_query("pepper");
    assert_eq!(settle(&mut app, "").len(), 2);
    app.set_search_folder("Spices");
    assert_eq!(settle(&mut app, "Pantry"), ["Pepper"]);
    app.set_search_folder("");
    app.set_search_tag("missing");
    assert_eq!(settle(&mut app, "Pantry"), Vec::<String>::new());
}

#[test]
fn a_folder_outside_the_workspace_shows_why() {
    let fixture = fixture();
    let mut app = fixture.app();

    app.open_search();
    app.set_search_folder("../Elsewhere");
    app.set_palette_query("pepper");
    let deadline = Instant::now() + Duration::from_secs(20);
    let error = loop {
        app.poll();
        if let SearchState::Failed(error) = state(&app) {
            break error;
        }
        assert!(Instant::now() < deadline, "the search never answered");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(error.code, "unsafe_path");
}

#[test]
fn a_broken_index_says_search_isnt_available() {
    let fixture = fixture();
    let mut app = fixture.app();
    let index = fixture
        .app_data
        .path()
        .join("workspaces")
        .join(app.workspace().unwrap().id())
        .join("index.sqlite");
    rusqlite::Connection::open(index)
        .unwrap()
        .execute_batch("DROP TABLE object_fts;")
        .unwrap();

    app.open_search();
    app.set_palette_query("pepper");
    let deadline = Instant::now() + Duration::from_secs(20);
    let error = loop {
        app.poll();
        if let SearchState::Failed(error) = state(&app) {
            break error;
        }
        assert!(Instant::now() < deadline, "the search never answered");
        std::thread::sleep(Duration::from_millis(2));
    };
    assert_eq!(error.message, "Search isn't available right now.");
}

#[test]
fn enter_opens_the_highlighted_hit_and_closes_search() {
    let fixture = fixture();
    let mut app = fixture.app();

    app.open_search();
    app.set_palette_query("black");
    settle(&mut app, "");
    assert!(app.open_palette_selection());

    assert!(app.palette().is_none());
    assert_eq!(document(&app).as_deref(), Some("Spices/Pepper.md"));
    // As a click in the file tree does, it opens a preview tab.
    assert!(app.active_tab().unwrap().is_preview());
    app.open_file("Recipe.md", OpenAs::Preview);
    assert_eq!(app.tabs().len(), 1);
}
