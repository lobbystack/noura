//! Seam 1: a note's links lead where today's app sends them. A target
//! resolves relative to the note, then from the workspace root, then by
//! file name anywhere, ignoring case and preferring the note's folder and
//! then the shortest path, and never outside the workspace.

mod support;

use app_core::App;
use support::{Fixture, open_from_tree, settle};

const FILES: &[(&str, &[u8])] = &[
    ("Inbox.md", b"# Inbox\n"),
    ("Projects/Plan.md", b"Plan in projects\n"),
    ("Projects/Assets/diagram.png", b"png"),
    ("Archive/Plan.md", b"Plan in the archive\n"),
    ("Archive/2024/Old notes.md", b"Old\n"),
    ("Reading/paper.pdf", b"%PDF"),
];

/// Where `target` leads from the note at `source`.
fn resolve(app: &mut App, source: &str, target: &str) -> Option<String> {
    let tab = open_from_tree(app, source);
    app.note_links(tab)
        .expect("the note is open")
        .resolve(target)
}

#[test]
fn a_target_resolves_by_file_name_anywhere_ignoring_case() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();

    for (target, expected) in [
        ("Old notes", "Archive/2024/Old notes.md"),
        ("old NOTES", "Archive/2024/Old notes.md"),
        ("diagram.png", "Projects/Assets/diagram.png"),
        ("Old notes#Heading", "Archive/2024/Old notes.md"),
    ] {
        assert_eq!(
            resolve(&mut app, "Inbox.md", target).as_deref(),
            Some(expected),
            "{target:?}"
        );
    }
}

#[test]
fn a_name_found_twice_prefers_the_notes_folder_then_the_shortest_path() {
    let mut files = FILES.to_vec();
    files.push(("Projects/Deep/Inner/Plan.md", b"deep"));
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();

    assert_eq!(
        resolve(&mut app, "Archive/2024/Old notes.md", "Plan").as_deref(),
        Some("Archive/Plan.md")
    );
    assert_eq!(
        resolve(&mut app, "Projects/Plan.md", "Plan").as_deref(),
        Some("Projects/Plan.md")
    );
    assert_eq!(
        resolve(&mut app, "Projects/Deep/Inner/Plan.md", "plan").as_deref(),
        Some("Projects/Deep/Inner/Plan.md")
    );
}

#[test]
fn relative_and_root_paths_resolve_before_names() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();

    for (target, expected) in [
        ("../Inbox.md", "Inbox.md"),
        ("Archive/Plan#Goals", "Archive/Plan.md"),
        ("Archive/2024/Old%20notes.md", "Archive/2024/Old notes.md"),
        ("/Inbox", "Inbox.md"),
        ("Assets/diagram.png", "Projects/Assets/diagram.png"),
        ("../Reading/paper.pdf", "Reading/paper.pdf"),
    ] {
        assert_eq!(
            resolve(&mut app, "Projects/Plan.md", target).as_deref(),
            Some(expected),
            "{target:?}"
        );
    }
}

#[test]
fn missing_files_web_addresses_and_escapes_lead_nowhere() {
    let fixture = Fixture::new(FILES);
    std::fs::write(fixture.outside().join("Secret.md"), "secret").unwrap();
    let mut app = fixture.app();

    for target in [
        "Nowhere",
        "https://example.com",
        "mailto:someone@example.com",
        "../../etc/passwd",
        "../Outside/Secret.md",
        "#Heading",
        "",
    ] {
        assert_eq!(resolve(&mut app, "Inbox.md", target), None, "{target:?}");
    }
}

#[cfg(unix)]
#[test]
fn a_file_behind_a_symlinked_folder_leads_nowhere() {
    let fixture = Fixture::new(FILES);
    std::fs::write(fixture.outside().join("Secret.md"), "secret").unwrap();
    std::os::unix::fs::symlink(fixture.outside(), fixture.root.join("Linked")).unwrap();
    let mut app = fixture.app();
    settle(&mut app);

    for target in ["Secret", "Linked/Secret.md", "Linked/Secret"] {
        assert_eq!(resolve(&mut app, "Inbox.md", target), None, "{target:?}");
    }
}

#[test]
fn links_follow_files_created_after_the_note_opened() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Inbox.md");
    let links = app.note_links(tab).unwrap();
    assert_eq!(links.resolve("Later"), None);

    std::fs::write(fixture.root.join("Later.md"), "later").unwrap();
    app.rescan();
    settle(&mut app);

    assert_eq!(links.resolve("Later").as_deref(), Some("Later.md"));
}
