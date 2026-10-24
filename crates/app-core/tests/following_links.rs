//! Seam 1: following a link in a note. A link to a workspace file opens it
//! the way the file tree does, under the preview-tab rules, or in a new tab
//! that keeps the current one. Web links go to the browser, other schemes
//! do nothing, and a link to a missing note says so and writes nothing.

mod support;

use app_core::{App, OpenAs};
use support::{Call, FakeSystem, Fixture, notice, open_from_tree};

const FILES: &[(&str, &[u8])] = &[
    ("Inbox.md", b"See [[Plan]].\n"),
    ("Projects/Plan.md", b"Plan in projects\n"),
    ("Projects/Assets/diagram.png", b"png"),
    ("Reading/paper.pdf", b"%PDF"),
    ("Data/table.csv", b"a,b\n"),
    ("Data/archive.zip", b"PK"),
];

/// Each tab's workspace path, with whether it's the preview, in order.
fn tabs(app: &App) -> Vec<(String, bool)> {
    app.tabs()
        .iter()
        .map(|tab| {
            let path = tab.file().map_or("", |file| file.relative_path.as_str());
            (path.to_owned(), tab.is_preview())
        })
        .collect()
}

fn active(app: &App) -> Option<String> {
    app.active_tab()
        .and_then(|tab| tab.file())
        .map(|file| file.relative_path.clone())
}

#[test]
fn a_link_opens_its_target_in_the_preview_tab_as_a_single_click_in_the_tree_does() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();

    // From a preview tab, the target replaces it.
    let tab = open_from_tree(&mut app, "Inbox.md");
    app.follow_note_link(tab, "Plan", OpenAs::Preview);
    assert_eq!(tabs(&app), [("Projects/Plan.md".to_owned(), true)]);
    assert_eq!(active(&app).as_deref(), Some("Projects/Plan.md"));

    // From a kept tab, the target opens in a preview tab beside it.
    app.close_tab(app.active_tab().unwrap().id());
    app.open_file("Inbox.md", OpenAs::Kept);
    let tab = app.active_tab().unwrap().id();
    app.follow_note_link(tab, "Plan", OpenAs::Preview);
    assert_eq!(
        tabs(&app),
        [
            ("Inbox.md".to_owned(), false),
            ("Projects/Plan.md".to_owned(), true),
        ]
    );
    assert_eq!(active(&app).as_deref(), Some("Projects/Plan.md"));
}

#[test]
fn a_link_opened_in_a_new_tab_keeps_the_current_tab() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();

    let tab = open_from_tree(&mut app, "Inbox.md");
    app.follow_note_link(tab, "Plan", OpenAs::Kept);

    assert_eq!(
        tabs(&app),
        [
            ("Inbox.md".to_owned(), true),
            ("Projects/Plan.md".to_owned(), false),
        ]
    );
    assert_eq!(active(&app).as_deref(), Some("Projects/Plan.md"));
}

/// What the window and the system show after opening a file: the tabs,
/// the active tab and what the system was asked to do, with the
/// workspace folder written as `<workspace>`.
fn outcome(
    app: &App,
    system: &FakeSystem,
    fixture: &Fixture,
) -> (Vec<(String, bool)>, Option<String>, Vec<String>) {
    let roots = [fixture.root.clone(), fixture.root.canonicalize().unwrap()];
    let calls = system
        .calls()
        .iter()
        .map(|call| {
            roots.iter().fold(format!("{call:?}"), |call, root| {
                call.replace(&root.display().to_string(), "<workspace>")
            })
        })
        .collect();
    (tabs(app), active(app), calls)
}

#[test]
fn a_link_to_any_workspace_file_opens_it_the_way_the_tree_does() {
    for (target, path) in [
        ("Plan", "Projects/Plan.md"),
        ("diagram.png", "Projects/Assets/diagram.png"),
        ("Reading/paper.pdf", "Reading/paper.pdf"),
        ("table.csv", "Data/table.csv"),
        ("archive.zip", "Data/archive.zip"),
    ] {
        for how in [OpenAs::Preview, OpenAs::Kept] {
            // A click on the file's row, from the note's preview tab.
            let tree = Fixture::new(FILES);
            let tree_system = FakeSystem::new();
            let mut by_tree = tree.app_with(tree_system.clone());
            open_from_tree(&mut by_tree, "Inbox.md");
            support::select(&mut by_tree, path);
            let row = by_tree.tree().unwrap().focused().unwrap();
            by_tree.activate_tree_row(row, how);

            // The link, from the same tab.
            let linked = Fixture::new(FILES);
            let link_system = FakeSystem::new();
            let mut by_link = linked.app_with(link_system.clone());
            let tab = open_from_tree(&mut by_link, "Inbox.md");
            by_link.follow_note_link(tab, target, how);

            assert_eq!(
                outcome(&by_link, &link_system, &linked),
                outcome(&by_tree, &tree_system, &tree),
                "{target:?} as {how:?}"
            );
            assert_eq!(active(&by_link).as_deref(), Some(path), "{target:?}");
        }
    }
}

#[test]
fn web_links_open_in_the_browser_and_other_schemes_do_nothing() {
    let fixture = Fixture::new(FILES);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let tab = open_from_tree(&mut app, "Inbox.md");

    for target in [
        "https://example.com/a?b=c#d",
        "HTTP://example.org",
        "www.example.com/page",
        "mailto:someone@example.com",
        "obsidian://open?vault=x",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "ftp://example.com/file",
    ] {
        for how in [OpenAs::Preview, OpenAs::Kept] {
            app.follow_note_link(tab, target, how);
        }
    }

    assert_eq!(
        system.calls(),
        [
            "https://example.com/a?b=c#d",
            "https://example.com/a?b=c#d",
            "HTTP://example.org",
            "HTTP://example.org",
            "https://www.example.com/page",
            "https://www.example.com/page",
        ]
        .map(|url| Call::OpenUrl(url.to_owned()))
    );
    assert_eq!(tabs(&app), [("Inbox.md".to_owned(), true)]);
    assert_eq!(notice(&app), None);
}

#[test]
fn a_link_leads_somewhere_when_following_it_opens_a_page_or_a_file() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Inbox.md");
    let links = app.note_links(tab).unwrap();

    // What the browser opens leads somewhere, in a wikilink too.
    for target in [
        "https://example.com",
        "HTTP://example.org",
        "www.example.com/page",
        "Plan",
        "diagram.png",
    ] {
        assert!(links.leads_somewhere(target), "{target:?}");
    }
    for target in ["Plan B", "mailto:someone@example.com", "obsidian://open"] {
        assert!(!links.leads_somewhere(target), "{target:?}");
    }
}

#[test]
fn a_link_to_a_missing_note_says_so_and_writes_nothing() {
    let fixture = Fixture::new(FILES);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let tab = open_from_tree(&mut app, "Inbox.md");
    support::settle(&mut app);
    let before = fixture.snapshot();

    for (target, name) in [
        ("Plan B", "Plan B"),
        ("Ideas/Later%20maybe.md#Top", "Ideas/Later maybe.md"),
        ("Projects", "Projects"),
        ("../Outside/Secret.md", "../Outside/Secret.md"),
    ] {
        for how in [OpenAs::Preview, OpenAs::Kept] {
            app.dismiss_notice();
            app.follow_note_link(tab, target, how);
            assert_eq!(
                notice(&app),
                Some((
                    "Couldn't open the link".to_owned(),
                    format!("\u{201c}{name}\u{201d} isn't in this workspace yet."),
                )),
                "{target:?}"
            );
        }
    }
    support::settle(&mut app);

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(tabs(&app), [("Inbox.md".to_owned(), true)]);
    assert_eq!(system.calls(), []);
}

#[test]
fn a_heading_or_block_fragment_opens_the_file_without_more() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Inbox.md");

    // A fragment alone leads to the note itself, which is already open.
    for target in ["#Top", "#^block-1"] {
        app.follow_note_link(tab, target, OpenAs::Preview);
        assert_eq!(tabs(&app), [("Inbox.md".to_owned(), true)], "{target:?}");
        assert_eq!(notice(&app), None, "{target:?}");
    }

    app.follow_note_link(tab, "Plan#^block-1", OpenAs::Preview);
    assert_eq!(tabs(&app), [("Projects/Plan.md".to_owned(), true)]);
}
