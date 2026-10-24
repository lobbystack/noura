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

/// Every file under the workspace, with its bytes and modification time.
fn snapshot(root: &std::path::Path) -> Vec<(String, Vec<u8>, std::time::SystemTime)> {
    let mut files = Vec::new();
    let mut folders = vec![root.to_path_buf()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                folders.push(path);
            } else {
                let metadata = std::fs::metadata(&path).unwrap();
                files.push((
                    path.strip_prefix(root).unwrap().display().to_string(),
                    std::fs::read(&path).unwrap(),
                    metadata.modified().unwrap(),
                ));
            }
        }
    }
    files.sort();
    files
}

/// What `target`'s embed shows once its read ends.
fn embed(links: &app_core::NoteLinks, target: &str) -> Option<app_core::NoteEmbed> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        match links.embed(target, || {}) {
            Some(app_core::NoteEmbedState::Reading { .. }) => {
                assert!(std::time::Instant::now() < deadline, "{target} never read");
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            Some(app_core::NoteEmbedState::Read(embed)) => return Some(embed),
            None => return None,
        }
    }
}

#[test]
fn an_embedded_note_shows_its_title_and_the_first_800_characters_of_its_text() {
    let long = "word ".repeat(400);
    let managed = "---\nid: note_01jbcdefghjkmnpqrstvwxyz0a\ntype: note\n\
        created: 2026-10-20T10:00:00Z\nupdated: 2026-10-20T10:00:00Z\n---\nManaged body.\n";
    let mut files = FILES.to_vec();
    files.push(("Long.md", long.as_bytes()));
    files.push(("Notes/Managed.md", managed.as_bytes()));
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Inbox.md");
    let links = app.note_links(tab).unwrap();
    let before = snapshot(&fixture.root);

    let old = embed(&links, "Old notes").expect("the note embeds");
    assert_eq!(
        (old.title.as_str(), old.text.as_str()),
        ("Old notes", "Old\n")
    );
    let long_embed = embed(&links, "Long").expect("the note embeds");
    assert_eq!(long_embed.title, "Long");
    assert_eq!(long_embed.text, long.chars().take(800).collect::<String>());
    let managed_embed = embed(&links, "Managed").expect("the note embeds");
    assert_eq!(managed_embed.text, "Managed body.\n");
    for not_a_note in ["Nowhere", "diagram.png", "paper.pdf"] {
        assert_eq!(embed(&links, not_a_note), None, "{not_a_note:?}");
    }

    assert_eq!(snapshot(&fixture.root), before, "embedding writes nothing");
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

#[test]
fn an_embed_reads_its_note_on_another_thread_and_says_when_the_read_ends() {
    // Longer than an embed reads: the embed still shows the note's start.
    let big = "start ".repeat(200_000);
    let mut files = FILES.to_vec();
    files.push(("Big.md", big.as_bytes()));
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Inbox.md");
    let links = app.note_links(tab).unwrap();
    let before = links.embeds_seq();

    let (sender, receiver) = std::sync::mpsc::channel();
    let first = links.embed("Big", move || sender.send(()).unwrap());

    assert_eq!(
        first,
        Some(app_core::NoteEmbedState::Reading {
            title: "Big".into()
        })
    );
    receiver
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the read ends");
    assert_ne!(links.embeds_seq(), before);
    assert_eq!(
        links.embed("Big", || {}),
        Some(app_core::NoteEmbedState::Read(app_core::NoteEmbed {
            title: "Big".into(),
            text: big.chars().take(800).collect(),
        }))
    );
}

#[test]
fn an_embed_keeps_what_it_shows_across_listings_and_reads_again_once_its_note_changes() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Inbox.md");
    let links = app.note_links(tab).unwrap();
    let old = embed(&links, "Old notes").expect("the note embeds");
    assert_eq!(old.text, "Old\n");

    // A listing after another file changed, as after a save of the note
    // itself, shows the same text without reading the note again.
    let listing = links.seq();
    std::fs::write(fixture.root.join("Projects/Plan.md"), "Plan, revised\n").unwrap();
    app.rescan();
    settle(&mut app);
    assert_ne!(links.seq(), listing);
    let (sender, receiver) = std::sync::mpsc::channel();
    assert_eq!(
        links.embed("Old notes", move || sender.send(()).unwrap()),
        Some(app_core::NoteEmbedState::Read(old.clone()))
    );
    assert!(
        receiver
            .recv_timeout(std::time::Duration::from_millis(300))
            .is_err(),
        "the note was read again"
    );

    // Once the note changes, the embed shows its old text until the new
    // read ends.
    std::fs::write(fixture.root.join("Archive/2024/Old notes.md"), "New\n").unwrap();
    app.rescan();
    settle(&mut app);
    let (sender, receiver) = std::sync::mpsc::channel();
    assert_eq!(
        links.embed("Old notes", move || sender.send(()).unwrap()),
        Some(app_core::NoteEmbedState::Read(old))
    );
    receiver
        .recv_timeout(std::time::Duration::from_secs(20))
        .expect("the note is read again");
    assert_eq!(
        embed(&links, "Old notes").map(|embed| embed.text),
        Some("New\n".into())
    );
}

#[test]
fn an_image_keeps_its_version_across_listings_until_its_file_changes() {
    let fixture = Fixture::new(FILES);
    let mut app = fixture.app();
    let tab = open_from_tree(&mut app, "Projects/Plan.md");
    let links = app.note_links(tab).unwrap();
    let image = links.image("Assets/diagram.png").expect("the image reads");
    assert_eq!(
        (image.path.as_str(), image.bytes.as_slice()),
        ("Projects/Assets/diagram.png", &b"png"[..])
    );
    assert_eq!(
        links.image_version("Assets/diagram.png"),
        Some(image.version.clone())
    );

    std::fs::write(fixture.root.join("Inbox.md"), "# Inbox, revised\n").unwrap();
    app.rescan();
    settle(&mut app);
    assert_eq!(
        links.image_version("diagram.png"),
        Some(image.version.clone())
    );

    std::fs::write(
        fixture.root.join("Projects/Assets/diagram.png"),
        b"a larger png",
    )
    .unwrap();
    let changed = links
        .image_version("Assets/diagram.png")
        .expect("the image is still there");
    assert_ne!(changed, image.version);
    assert_eq!(
        links.image("Assets/diagram.png").map(|image| image.version),
        Some(changed)
    );
    for not_an_image in ["Inbox", "paper.pdf", "missing.png", "../../outside.png"] {
        assert_eq!(links.image_version(not_an_image), None, "{not_an_image:?}");
    }
}
