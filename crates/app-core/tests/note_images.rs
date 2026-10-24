//! Seam 1: workspace images in the formats today's app shows draw inline
//! from a note. They resolve like links, and a path that leaves the
//! workspace, symlinks included, reads nothing.

mod support;

use app_core::{App, NoteLinks};
use support::{Fixture, open_from_tree, settle};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really";

fn links(app: &mut App, source: &str) -> NoteLinks {
    let tab = open_from_tree(app, source);
    app.note_links(tab).expect("the note is open")
}

/// The workspace path and bytes `target` reads as an image, from `source`.
fn image(app: &mut App, source: &str, target: &str) -> Option<(String, Vec<u8>)> {
    links(app, source)
        .image(target)
        .map(|image| (image.path, image.bytes))
}

#[test]
fn an_image_reads_relative_to_the_note_or_by_name_anywhere() {
    let fixture = Fixture::new(&[
        ("Notes/Plan.md", b"![](assets/pic.png)\n"),
        ("Notes/assets/pic.png", PNG),
        ("Elsewhere/photo.jpg", b"jpeg"),
    ]);
    let mut app = fixture.app();

    for (target, path, bytes) in [
        ("assets/pic.png", "Notes/assets/pic.png", PNG),
        ("pic.png", "Notes/assets/pic.png", PNG),
        ("photo.jpg", "Elsewhere/photo.jpg", b"jpeg".as_slice()),
        ("PHOTO.jpg", "Elsewhere/photo.jpg", b"jpeg".as_slice()),
        (
            "../Elsewhere/photo.jpg",
            "Elsewhere/photo.jpg",
            b"jpeg".as_slice(),
        ),
    ] {
        assert_eq!(
            image(&mut app, "Notes/Plan.md", target),
            Some((path.to_string(), bytes.to_vec())),
            "{target:?}"
        );
    }
}

#[test]
fn images_read_in_the_formats_today_s_app_shows() {
    let names = [
        "a.png", "b.jpg", "c.jpeg", "d.gif", "e.webp", "f.svg", "g.bmp", "h.avif",
    ];
    let mut files: Vec<(&str, &[u8])> = names.iter().map(|name| (*name, b"x".as_slice())).collect();
    files.push(("Plan.md", b"plan"));
    files.push(("notes.txt", b"text"));
    let fixture = Fixture::new(&files);
    let mut app = fixture.app();

    for name in names {
        assert!(image(&mut app, "Plan.md", name).is_some(), "{name}");
    }
    for name in ["notes.txt", "Plan.md", "Plan", "missing.png"] {
        assert_eq!(image(&mut app, "Plan.md", name), None, "{name}");
    }
}

#[test]
fn an_image_outside_the_workspace_reads_nothing() {
    let fixture = Fixture::new(&[("Notes/Plan.md", b"plan")]);
    std::fs::write(fixture.outside().join("secret.png"), PNG).unwrap();
    let mut app = fixture.app();

    for target in [
        "../../Outside/secret.png",
        "../Outside/secret.png",
        "/Outside/secret.png",
        "secret.png",
    ] {
        assert_eq!(image(&mut app, "Notes/Plan.md", target), None, "{target:?}");
    }
}

#[cfg(unix)]
#[test]
fn an_image_behind_a_symlink_reads_nothing() {
    let fixture = Fixture::new(&[("Notes/Plan.md", b"plan")]);
    std::fs::write(fixture.outside().join("secret.png"), PNG).unwrap();
    std::os::unix::fs::symlink(fixture.outside(), fixture.root.join("Linked")).unwrap();
    std::os::unix::fs::symlink(
        fixture.outside().join("secret.png"),
        fixture.root.join("Notes/link.png"),
    )
    .unwrap();
    let mut app = fixture.app();
    settle(&mut app);

    for target in [
        "../Linked/secret.png",
        "secret.png",
        "link.png",
        "Notes/link.png",
    ] {
        assert_eq!(image(&mut app, "Notes/Plan.md", target), None, "{target:?}");
    }
}

#[test]
fn an_image_over_20_mb_reads_nothing() {
    let fixture = Fixture::new(&[("Plan.md", b"plan"), ("small.png", PNG)]);
    let big = std::fs::File::create(fixture.root.join("big.png")).unwrap();
    big.set_len(20 * 1024 * 1024 + 1).unwrap();
    drop(big);
    let mut app = fixture.app();
    settle(&mut app);

    assert_eq!(image(&mut app, "Plan.md", "big.png"), None);
    assert!(image(&mut app, "Plan.md", "small.png").is_some());
}
