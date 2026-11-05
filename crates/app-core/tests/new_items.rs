//! Seam 1: New note and New folder from the file tree.

mod support;

use support::{Fixture, current, document, object_id, rows, select};

#[test]
fn new_note_creates_untitled_then_untitled_1_as_managed_notes_and_opens_each() {
    let fixture = Fixture::new(&[("Readme.md", b"x")]);
    let mut app = fixture.app();

    app.new_note(None);

    let first = fixture.read("Untitled.md");
    assert!(object_id(&first).starts_with("note_"));
    assert_eq!(document(&app).as_deref(), Some("Untitled.md"));
    assert_eq!(current(&app).as_deref(), Some("Untitled.md"));

    app.new_note(None);

    let second = fixture.read("Untitled 1.md");
    assert_ne!(object_id(&second), object_id(&first));
    assert_eq!(document(&app).as_deref(), Some("Untitled 1.md"));
    assert_eq!(current(&app).as_deref(), Some("Untitled 1.md"));
    // The tree sorts a space before a dot, as the legacy collator does.
    assert_eq!(rows(&app), ["Readme", "Untitled 1", "Untitled"]);
    assert!(app.notice().is_none());
}

#[test]
fn new_note_goes_in_the_selected_rows_folder() {
    let fixture = Fixture::new(&[("Docs/Guide.md", b"x"), ("Docs/Deep/Inner.md", b"x")]);
    let mut app = fixture.app();
    select(&mut app, "Docs/Guide.md");

    app.new_note(None);
    assert!(fixture.exists("Docs/Untitled.md"));
    assert_eq!(document(&app).as_deref(), Some("Docs/Untitled.md"));

    select(&mut app, "Docs/Deep");
    app.new_note(None);
    assert!(fixture.exists("Docs/Deep/Untitled.md"));
    assert_eq!(current(&app).as_deref(), Some("Docs/Deep/Untitled.md"));
}

#[test]
fn new_note_from_a_folders_menu_goes_in_that_folder_and_opens_it_in_the_tree() {
    let fixture = Fixture::new(&[("Docs/Guide.md", b"x"), ("Readme.md", b"x")]);
    let mut app = fixture.app();
    select(&mut app, "Readme.md");

    app.new_note(Some("Docs"));

    assert!(fixture.exists("Docs/Untitled.md"));
    assert_eq!(rows(&app), ["Docs", "Guide", "Untitled", "Readme"]);
    assert_eq!(current(&app).as_deref(), Some("Docs/Untitled.md"));
}

#[test]
fn new_note_skips_names_taken_in_any_case_and_by_files_not_downloaded() {
    let fixture = Fixture::new(&[("untitled.md", b"x"), (".Untitled 1.md.icloud", b"x")]);
    let mut app = fixture.app();

    app.new_note(None);

    assert!(fixture.exists("Untitled 2.md"));
    assert_eq!(fixture.read("untitled.md"), b"x");
}

#[test]
fn new_folder_creates_untitled_then_untitled_1_and_starts_renaming_it() {
    let fixture = Fixture::new(&[("Docs/Guide.md", b"x")]);
    let mut app = fixture.app();

    app.new_folder(None);

    assert!(fixture.root.join("Untitled").is_dir());
    assert_eq!(current(&app).as_deref(), Some("Untitled"));
    assert_eq!(
        app.renaming().map(|rename| rename.relative_path.as_str()),
        Some("Untitled")
    );
    app.cancel_rename();
    assert!(app.renaming().is_none());

    app.new_folder(Some("Docs"));

    assert!(fixture.root.join("Docs/Untitled").is_dir());
    assert_eq!(
        app.renaming().map(|rename| rename.relative_path.as_str()),
        Some("Docs/Untitled")
    );
    assert_eq!(rows(&app), ["Docs", "Untitled", "Guide", "Untitled"]);
    app.cancel_rename();
    app.new_folder(Some(""));
    assert!(fixture.root.join("Untitled 1").is_dir());
}

#[test]
fn new_items_refuse_a_folder_outside_the_workspace_and_change_nothing() {
    let fixture = Fixture::new(&[("Readme.md", b"x")]);
    let outside = fixture.outside();
    #[cfg(unix)]
    std::os::unix::fs::symlink(&outside, fixture.root.join("Linked")).unwrap();
    let mut app = fixture.app();
    let before = fixture.snapshot();

    for folder in ["..", "../Outside", "/tmp", ".noura", "Readme.md", "Missing"] {
        app.new_note(Some(folder));
        assert!(app.notice().is_some(), "{folder}");
        app.dismiss_notice();
        app.new_folder(Some(folder));
        assert!(app.notice().is_some(), "{folder}");
        app.dismiss_notice();
    }
    #[cfg(unix)]
    {
        app.new_note(Some("Linked"));
        let notice = app.notice().unwrap().clone();
        assert_eq!(notice.title, "Couldn't create the note");
        assert_eq!(
            notice.message,
            "Linked is a link or sits in a linked folder, so Noura won't change it."
        );
    }

    assert_eq!(fixture.snapshot(), before);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}
