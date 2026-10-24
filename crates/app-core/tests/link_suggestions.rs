//! Seam 1: what typing `[[` in a note suggests. Workspace files that match
//! what you typed, Markdown files by name without `.md` and other files by
//! their whole name, with the folder shown and the path inserted when two
//! files share a name.

mod support;

use app_core::{App, LinkSuggestion};
use support::{Fixture, open_from_tree};

const FILES: &[(&str, &[u8])] = &[
    ("Inbox.md", b"# Inbox\n"),
    ("Plan.md", b"Plan at the top\n"),
    ("Projects/Plan.md", b"Plan in projects\n"),
    ("Archive/plan.md", b"Plan in the archive\n"),
    ("Projects/Assets/diagram.png", b"png"),
    ("Reading/paper.pdf", b"%PDF"),
    ("Reading/Old notes.md", b"Old\n"),
];

/// What typing `typed` after `[[` in the note at `source` suggests, as
/// (name, folder shown, text inserted).
fn suggest(app: &mut App, source: &str, typed: &str) -> Vec<(String, Option<String>, String)> {
    let tab = open_from_tree(app, source);
    app.note_links(tab)
        .expect("the note is open")
        .suggest(typed)
        .into_iter()
        .map(|LinkSuggestion { name, folder, link }| (name, folder, link))
        .collect()
}

fn entry(name: &str, folder: Option<&str>, link: &str) -> (String, Option<String>, String) {
    (name.to_owned(), folder.map(str::to_owned), link.to_owned())
}

#[test]
fn files_are_listed_by_name_and_shared_names_show_their_folder() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();

    assert_eq!(
        suggest(&mut app, "Reading/Old notes.md", ""),
        [
            entry("diagram.png", None, "diagram.png"),
            entry("Inbox", None, "Inbox"),
            entry("Old notes", None, "Old notes"),
            entry("paper.pdf", None, "paper.pdf"),
            entry("plan", Some("Archive"), "Archive/plan"),
            entry("Plan", Some(""), "Plan"),
            entry("Plan", Some("Projects"), "Projects/Plan"),
        ]
    );
}

#[test]
fn the_list_narrows_as_you_type() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();

    let names = |found: Vec<(String, Option<String>, String)>| -> Vec<String> {
        found.into_iter().map(|(_, _, link)| link).collect()
    };
    assert_eq!(
        names(suggest(&mut app, "Inbox.md", "p")),
        [
            "Archive/plan",
            "Plan",
            "Projects/Plan",
            "paper.pdf",
            "diagram.png",
        ]
    );
    assert_eq!(
        names(suggest(&mut app, "Inbox.md", "pla")),
        ["Archive/plan", "Plan", "Projects/Plan"]
    );
    assert_eq!(
        names(suggest(&mut app, "Inbox.md", "diag")),
        ["diagram.png"]
    );
    assert_eq!(
        names(suggest(&mut app, "Inbox.md", "zzz")),
        Vec::<String>::new()
    );
}

#[test]
fn every_suggestion_links_to_its_own_file() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();

    for source in ["Inbox.md", "Projects/Plan.md", "Reading/Old notes.md"] {
        let tab = open_from_tree(&mut app, source);
        let links = app.note_links(tab).unwrap();
        let suggested = links.suggest("");
        assert_eq!(suggested.len(), FILES.len(), "{source}");
        let mut led_to: Vec<String> = suggested
            .iter()
            .map(|suggestion| links.resolve(&suggestion.link).expect("it resolves"))
            .collect();
        led_to.sort();
        let mut files: Vec<String> = FILES.iter().map(|(path, _)| (*path).to_owned()).collect();
        files.sort();
        assert_eq!(led_to, files, "from {source}");
    }
}

#[test]
fn a_name_that_leads_to_another_file_links_by_its_whole_path() {
    let fixture = Fixture::new(&[
        ("Inbox.md", b"# Inbox\n"),
        ("Reading/Report.pdf", b"%PDF"),
        ("Reading/Report.pdf.md", b"Notes on the report\n"),
        ("Drafts/Plan.md", b"Plan\n"),
        ("Drafts/Plan.md.md", b"Odd name\n"),
    ]);
    let mut app = fixture.app();

    let tab = open_from_tree(&mut app, "Inbox.md");
    let links = app.note_links(tab).unwrap();
    let mut found: Vec<(String, String)> = links
        .suggest("")
        .into_iter()
        .map(|suggestion| {
            let led_to = links.resolve(&suggestion.link).expect("it resolves");
            (suggestion.link, led_to)
        })
        .collect();
    found.sort();
    assert_eq!(
        found,
        [
            ("Drafts/Plan.md.md", "Drafts/Plan.md.md"),
            ("Inbox", "Inbox.md"),
            ("Plan", "Drafts/Plan.md"),
            ("Reading/Report.pdf", "Reading/Report.pdf"),
            ("Reading/Report.pdf.md", "Reading/Report.pdf.md"),
        ]
        .map(|(link, file)| (link.to_owned(), file.to_owned()))
    );
}
