//! Seam 1: duplicating a file next to itself.

mod support;

use local_core::ParsedMarkdown;
use support::{Fixture, current, document, note, notice, object_id};

const PLAN_ID: &str = "01jbcdefghjkmnpqrstvwxyz00";

fn managed(bytes: &[u8]) -> local_core::WorkspaceObject {
    match local_core::parse_markdown("x.md", bytes) {
        ParsedMarkdown::Managed(object) => object,
        other => panic!("not a managed file: {other:?}"),
    }
}

#[test]
fn duplicating_a_managed_note_gives_the_copy_a_new_id_and_the_same_properties_and_body() {
    let bytes = note(PLAN_ID, "First line\n\n- [ ] a task\n");
    let fixture = Fixture::new(&[("Docs/Plan.md", &bytes)]);
    let mut app = fixture.app();

    app.duplicate("Docs/Plan.md");

    let original = managed(&fixture.read("Docs/Plan.md"));
    let copy = managed(&fixture.read("Docs/Plan 1.md"));
    assert_eq!(fixture.read("Docs/Plan.md"), bytes);
    assert_ne!(copy.id, original.id);
    assert!(copy.id.starts_with("note_"));
    assert_eq!(copy.object_type, "note");
    assert_eq!(copy.properties, original.properties);
    // Today's format writes a managed body with one final line break and
    // reads it back without one; ticket 17 keeps the final newline as is.
    assert_eq!(copy.body.trim_end(), original.body.trim_end());
    assert_eq!(copy.body.trim_end(), "First line\n\n- [ ] a task");
    assert_eq!(current(&app).as_deref(), Some("Docs/Plan 1.md"));
    let opened = app.document().unwrap();
    assert_eq!(opened.relative_path, "Docs/Plan 1.md");
    assert_eq!(opened.name, "Plan 1");
}

#[test]
fn any_other_file_copies_byte_for_byte() {
    let raw = b"---\ntags: [a]\n---\n# Heading\r\nText without a final newline";
    let broken = b"---\nid: [unclosed\n---\nbody";
    let fixture = Fixture::new(&[
        ("data.csv", b"1,2\n"),
        ("raw.md", raw),
        ("broken.md", broken),
        ("archive.tar.gz", b"\x1f\x8b\x00"),
        ("Makefile", b"all:\n"),
    ]);
    let mut app = fixture.app();

    for path in [
        "data.csv",
        "raw.md",
        "broken.md",
        "archive.tar.gz",
        "Makefile",
    ] {
        app.duplicate(path);
        assert!(app.notice().is_none(), "{path}: {:?}", notice(&app));
    }

    assert_eq!(fixture.read("data 1.csv"), b"1,2\n");
    assert_eq!(fixture.read("raw 1.md"), raw);
    assert_eq!(fixture.read("broken 1.md"), broken);
    assert_eq!(fixture.read("archive.tar 1.gz"), b"\x1f\x8b\x00");
    assert_eq!(fixture.read("Makefile 1"), b"all:\n");
    assert_eq!(document(&app).as_deref(), Some("Makefile 1"));
}

#[test]
fn the_copy_takes_the_next_free_number() {
    let bytes = note(PLAN_ID, "Body\n");
    let fixture = Fixture::new(&[("Plan.md", &bytes), ("plan 1.md", b"taken")]);
    let mut app = fixture.app();

    app.duplicate("Plan.md");
    app.duplicate("Plan.md");

    assert_eq!(fixture.read("plan 1.md"), b"taken");
    let second = object_id(&fixture.read("Plan 2.md"));
    let third = object_id(&fixture.read("Plan 3.md"));
    assert_ne!(second, third);
    assert_ne!(second, format!("note_{PLAN_ID}"));
}

#[test]
fn folders_and_items_outside_the_workspace_are_not_duplicated() {
    let fixture = Fixture::new(&[("Docs/Plan.md", b"p")]);
    let outside = fixture.outside();
    std::fs::write(outside.join("Keep.md"), b"keep").unwrap();
    let mut app = fixture.app();
    let before = fixture.snapshot();

    app.duplicate("Docs");
    assert_eq!(
        notice(&app),
        Some((
            "Couldn't duplicate \u{201c}Docs\u{201d}".to_owned(),
            "Only files can be duplicated.".to_owned()
        ))
    );
    for path in [
        "",
        "../Outside/Keep.md",
        "/tmp/x.md",
        ".noura/workspace.yaml",
    ] {
        app.dismiss_notice();
        app.duplicate(path);
        assert!(app.notice().is_some(), "{path}");
    }

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 1);
}
