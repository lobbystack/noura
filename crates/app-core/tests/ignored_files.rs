//! Seam 1: the Ignored files list, saved in the workspace manifest.

mod support;

use support::{Fixture, rows};

const MANIFEST: &str = ".noura/workspace.yaml";

fn patterns(app: &app_core::App) -> Vec<String> {
    app.ignored_files().unwrap().patterns.clone()
}

fn problem(app: &app_core::App) -> Option<String> {
    app.ignored_files().unwrap().problem.clone()
}

/// The `ignore` list in the manifest on disk.
fn manifest_ignore(fixture: &Fixture) -> Vec<String> {
    let manifest: local_core::WorkspaceManifest =
        serde_yaml_ng::from_slice(&fixture.read(MANIFEST)).unwrap();
    manifest.ignore
}

#[test]
fn adding_a_pattern_saves_it_to_the_manifest_and_hides_matches_from_the_tree() {
    let fixture = Fixture::new(&[
        ("build/out.txt", b"o"),
        ("Docs/draft.tmp", b"d"),
        ("Docs/Plan.md", b"p"),
        ("Readme.md", b"r"),
    ]);
    let mut app = fixture.app();
    app.open_ignored_files();
    assert!(patterns(&app).is_empty());

    assert!(app.add_ignored("build/"));
    assert!(app.add_ignored("  *.tmp  "));

    assert_eq!(patterns(&app), ["build/", "*.tmp"]);
    assert_eq!(manifest_ignore(&fixture), ["build/", "*.tmp"]);
    support::select(&mut app, "Docs/Plan.md");
    assert_eq!(rows(&app), ["Docs", "Plan", "Readme"]);
    assert_eq!(fixture.read("build/out.txt"), b"o");
}

/// The titles search finds for `text`, sorted.
fn found(app: &app_core::App, text: &str) -> Vec<String> {
    let query = app_core::SearchQuery {
        text: text.to_owned(),
        ..app_core::SearchQuery::default()
    };
    let mut titles: Vec<String> = app
        .workspace()
        .unwrap()
        .search(&query)
        .unwrap()
        .into_iter()
        .map(|hit| hit.title)
        .collect();
    titles.sort();
    titles
}

#[test]
fn ignored_files_leave_search_and_come_back_with_their_pattern() {
    let fixture = Fixture::new(&[
        ("build/out.md", b"zebrafinch in the build folder\n"),
        ("Docs/draft.tmp.md", b"zebrafinch in a draft\n"),
        ("Docs/Plan.md", b"zebrafinch in the plan\n"),
    ]);
    let mut app = fixture.app();
    assert_eq!(found(&app, "zebrafinch"), ["Plan", "draft.tmp", "out"]);
    app.open_ignored_files();

    assert!(app.add_ignored("build/"));
    assert!(app.add_ignored("*.tmp.md"));
    assert_eq!(found(&app, "zebrafinch"), ["Plan"]);

    // A matching file that arrives later stays out of search too.
    support::write(&fixture.root, "build/late.md", b"zebrafinch arrived late\n");
    app.rescan();
    support::settle(&mut app);
    assert_eq!(found(&app, "zebrafinch"), ["Plan"]);

    assert!(app.remove_ignored("build/"));
    support::settle(&mut app);
    assert_eq!(found(&app, "zebrafinch"), ["Plan", "late", "out"]);
    assert_eq!(fixture.read("build/out.md"), b"zebrafinch in the build folder\n");
}

#[test]
fn removing_a_pattern_brings_its_items_back() {
    let fixture = Fixture::new(&[("build/out.txt", b"o"), ("Readme.md", b"r")]);
    let mut app = fixture.app();
    app.open_ignored_files();
    app.add_ignored("build/");
    assert_eq!(rows(&app), ["Readme"]);

    app.remove_ignored("build/");

    assert!(patterns(&app).is_empty());
    assert!(manifest_ignore(&fixture).is_empty());
    assert_eq!(rows(&app), ["build", "Readme"]);
}

#[test]
fn the_list_shows_the_patterns_already_in_the_manifest() {
    let fixture = Fixture::new(&[("Readme.md", b"r")]);
    let manifest = fixture.root.join(MANIFEST);
    let text = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("ignore: []", "ignore:\n- drafts/\n- '*.log'");
    std::fs::write(&manifest, text).unwrap();
    let mut app = fixture.app();

    app.open_ignored_files();

    assert_eq!(patterns(&app), ["drafts/", "*.log"]);
    assert_eq!(problem(&app), None);
}

#[test]
fn an_invalid_pattern_is_refused_and_the_manifest_keeps_its_bytes() {
    let fixture = Fixture::new(&[("Readme.md", b"r")]);
    let mut app = fixture.app();
    app.open_ignored_files();
    let before = fixture.read(MANIFEST);

    assert!(!app.add_ignored("notes/{a,b"));
    assert_eq!(
        problem(&app).as_deref(),
        Some("\u{201c}notes/{a,b\u{201d} isn't a valid pattern.")
    );
    assert!(!app.add_ignored("   "));
    assert_eq!(problem(&app).as_deref(), Some("Enter a pattern."));

    assert!(patterns(&app).is_empty());
    assert_eq!(fixture.read(MANIFEST), before);
}

#[test]
fn a_manifest_changed_on_disk_asks_you_to_try_again() {
    let fixture = Fixture::new(&[("build/out.txt", b"o"), ("drafts/a.md", b"a")]);
    let mut app = fixture.app();
    app.open_ignored_files();
    let manifest = fixture.root.join(MANIFEST);
    let edited = std::fs::read_to_string(&manifest)
        .unwrap()
        .replace("ignore: []", "ignore:\n- drafts/");
    std::fs::write(&manifest, &edited).unwrap();

    assert!(!app.add_ignored("build/"));

    assert_eq!(
        problem(&app).as_deref(),
        Some(
            "The settings file .noura/workspace.yaml changed on disk since Noura read it. \
             Try again."
        )
    );
    assert_eq!(std::fs::read_to_string(&manifest).unwrap(), edited);
    assert_eq!(patterns(&app), ["drafts/"]);

    assert!(app.add_ignored("build/"));
    assert_eq!(manifest_ignore(&fixture), ["drafts/", "build/"]);
    assert_eq!(problem(&app), None);
}

#[test]
fn built_in_exclusions_still_apply_whatever_the_list_says() {
    let fixture = Fixture::new(&[
        ("node_modules/pkg/index.js", b"x"),
        ("target/debug/out", b"x"),
        (".git/config", b"x"),
        (".hidden.md", b"x"),
        ("Readme.md", b"r"),
    ]);
    let mut app = fixture.app();
    app.open_ignored_files();

    assert!(app.add_ignored("!node_modules"));
    assert!(app.add_ignored("!.hidden.md"));

    assert_eq!(rows(&app), ["Readme"]);
}

#[test]
fn adding_a_pattern_already_listed_changes_nothing() {
    let fixture = Fixture::new(&[("Readme.md", b"r")]);
    let mut app = fixture.app();
    app.open_ignored_files();
    app.add_ignored("build/");
    let before = fixture.read(MANIFEST);

    assert!(app.add_ignored("build/"));

    assert_eq!(fixture.read(MANIFEST), before);
    assert_eq!(patterns(&app), ["build/"]);
}
