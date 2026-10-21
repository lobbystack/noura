//! Seam 1: renaming files and folders in place in the tree.

mod support;

use app_core::OpenAs;
use support::{Fixture, current, document, note, notice, object_id, rows};

const PLAN_ID: &str = "01jbcdefghjkmnpqrstvwxyz00";

#[test]
fn renaming_a_managed_note_keeps_md_its_id_and_every_byte() {
    let bytes = note(PLAN_ID, "Body text\n");
    let fixture = Fixture::new(&[("Docs/Plan.md", &bytes)]);
    let mut app = fixture.app();
    app.start_rename("Docs/Plan.md");
    assert_eq!(app.renaming().unwrap().name, "Plan");

    assert!(app.commit_rename("Roadmap"));

    assert_eq!(fixture.read("Docs/Roadmap.md"), bytes);
    assert_eq!(
        object_id(&fixture.read("Docs/Roadmap.md")),
        format!("note_{PLAN_ID}")
    );
    assert!(!fixture.exists("Docs/Plan.md"));
    assert!(app.renaming().is_none());
    assert_eq!(current(&app).as_deref(), Some("Docs/Roadmap.md"));
    assert_eq!(rows(&app), ["Docs", "Roadmap"]);
}

#[test]
fn renaming_the_open_file_keeps_it_open_under_its_new_name() {
    let fixture = Fixture::new(&[("Plan.md", b"x")]);
    let mut app = fixture.app();
    app.open_file("Plan.md", OpenAs::Preview);
    app.start_rename("Plan.md");

    assert!(app.commit_rename("Roadmap"));

    let opened = app.document().unwrap();
    assert_eq!(opened.relative_path, "Roadmap.md");
    assert_eq!(opened.name, "Roadmap");
}

#[test]
fn other_files_and_folders_take_the_name_as_typed() {
    let fixture = Fixture::new(&[("data.csv", b"1,2"), ("Docs/Guide.MD", b"g")]);
    let mut app = fixture.app();

    app.start_rename("data.csv");
    assert_eq!(app.renaming().unwrap().name, "data.csv");
    assert!(app.commit_rename("numbers.txt"));
    assert_eq!(fixture.read("numbers.txt"), b"1,2");

    app.start_rename("Docs/Guide.MD");
    assert!(app.commit_rename("Manual"));
    assert_eq!(fixture.read("Docs/Manual.MD"), b"g");
}

#[test]
fn renaming_a_folder_keeps_its_contents_ids_and_open_state() {
    let bytes = note(PLAN_ID, "Body\n");
    let fixture = Fixture::new(&[("Docs/Plan.md", &bytes), ("Docs/Deep/Inner.md", b"i")]);
    let mut app = fixture.app();
    support::select(&mut app, "Docs/Deep/Inner.md");
    app.open_file("Docs/Deep/Inner.md", OpenAs::Preview);

    app.start_rename("Docs");
    assert!(app.commit_rename("Guides"));

    assert_eq!(fixture.read("Guides/Plan.md"), bytes);
    assert_eq!(fixture.read("Guides/Deep/Inner.md"), b"i");
    assert!(!fixture.exists("Docs"));
    assert_eq!(rows(&app), ["Guides", "Deep", "Inner", "Plan"]);
    assert_eq!(current(&app).as_deref(), Some("Guides"));
    assert_eq!(document(&app).as_deref(), Some("Guides/Deep/Inner.md"));
}

#[test]
fn a_case_only_rename_changes_the_name_on_disk() {
    let fixture = Fixture::new(&[("plan.md", b"x"), ("docs/a.txt", b"a")]);
    let mut app = fixture.app();

    app.start_rename("plan.md");
    assert!(app.commit_rename("Plan"));
    app.start_rename("docs");
    assert!(app.commit_rename("Docs"));

    let mut names: Vec<String> = std::fs::read_dir(&fixture.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .filter(|name| name != ".noura")
        .collect();
    names.sort();
    assert_eq!(names, ["Docs", "Plan.md"]);
    assert_eq!(fixture.read("Plan.md"), b"x");
    assert_eq!(rows(&app), ["Docs", "Plan"]);
}

#[test]
fn rename_refuses_bad_names_with_a_plain_message_and_changes_nothing() {
    let fixture = Fixture::new(&[
        ("Plan.md", b"x"),
        ("taken.md", b"t"),
        ("Docs/Other.md", b"o"),
    ]);
    let mut app = fixture.app();
    let before = fixture.snapshot();
    let characters = "Names can't contain \\ / : * ? \" < > or |.";
    let cases = [
        ("", "Enter a name."),
        ("a/b", characters),
        ("a\\b", characters),
        ("a:b", characters),
        ("a*b", characters),
        ("a?b", characters),
        ("a\"b", characters),
        ("a<b", characters),
        ("a>b", characters),
        ("a|b", characters),
        ("..", "Names can't start with a dot."),
        (".hidden", "Names can't start with a dot."),
        (" Plan", "Names can't start or end with a space."),
        ("Plan ", "Names can't start or end with a space."),
        (
            "Taken",
            "Something named \u{201c}Taken\u{201d} is already in this folder.",
        ),
    ];

    for (name, message) in cases {
        app.start_rename("Plan.md");
        assert!(!app.commit_rename(name), "{name:?}");
        let rename = app.renaming().unwrap();
        assert_eq!(rename.relative_path, "Plan.md");
        assert_eq!(rename.problem.as_deref(), Some(message), "{name:?}");
        app.cancel_rename();
    }

    app.start_rename("Docs");
    assert!(!app.commit_rename("Plan.md"));
    assert_eq!(
        app.renaming().unwrap().problem.as_deref(),
        Some("Something named \u{201c}Plan.md\u{201d} is already in this folder.")
    );
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn keeping_the_same_name_or_cancelling_changes_nothing() {
    let fixture = Fixture::new(&[("Plan.md", b"x")]);
    let mut app = fixture.app();
    let before = fixture.snapshot();

    app.start_rename("Plan.md");
    assert!(app.commit_rename("Plan"));
    assert!(app.renaming().is_none());
    app.start_rename("Plan.md");
    app.cancel_rename();

    assert!(app.renaming().is_none());
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn renaming_an_item_that_left_the_workspace_says_so_and_drops_its_row() {
    let fixture = Fixture::new(&[("Plan.md", b"x"), ("Readme.md", b"r")]);
    let mut app = fixture.app();
    app.open_file("Plan.md", OpenAs::Preview);
    app.start_rename("Plan.md");
    std::fs::remove_file(fixture.root.join("Plan.md")).unwrap();

    assert!(app.commit_rename("Roadmap"));

    assert!(app.renaming().is_none());
    assert_eq!(
        notice(&app),
        Some((
            "Couldn't rename \u{201c}Plan\u{201d}".to_owned(),
            "Plan.md isn't in the workspace anymore.".to_owned()
        ))
    );
    assert_eq!(document(&app), None);
    assert_eq!(rows(&app), ["Readme"]);
    assert!(!fixture.exists("Roadmap.md"));
}

#[test]
fn a_rename_that_fails_after_the_item_moved_follows_it() {
    let fixture = Fixture::new(&[("Docs/Plan.md", b"x"), ("Readme.md", b"r")]);
    let mut app = fixture.app();
    app.open_file("Docs/Plan.md", OpenAs::Preview);
    app.start_rename("Docs/Plan.md");
    // The file already has the new name, as when the move happened and
    // only syncing its folder failed.
    std::fs::rename(
        fixture.root.join("Docs/Plan.md"),
        fixture.root.join("Docs/Roadmap.md"),
    )
    .unwrap();

    assert!(app.commit_rename("Roadmap"));

    assert!(app.renaming().is_none());
    assert_eq!(
        notice(&app).unwrap().0,
        "Couldn't rename \u{201c}Plan\u{201d}"
    );
    assert_eq!(document(&app).as_deref(), Some("Docs/Roadmap.md"));
    assert_eq!(current(&app).as_deref(), Some("Docs/Roadmap.md"));
    assert_eq!(rows(&app), ["Docs", "Roadmap", "Readme"]);
}

#[test]
fn leaving_the_field_saves_the_name_or_says_why_it_couldnt() {
    let fixture = Fixture::new(&[("Plan.md", b"x")]);
    let mut app = fixture.app();

    app.start_rename("Plan.md");
    app.finish_rename("Roadmap");
    assert!(app.renaming().is_none());
    assert_eq!(fixture.read("Roadmap.md"), b"x");

    app.start_rename("Roadmap.md");
    app.finish_rename("a/b");

    assert!(app.renaming().is_none());
    assert_eq!(
        support::notice(&app),
        Some((
            "Couldn't rename \u{201c}Roadmap\u{201d}".to_owned(),
            "Names can't contain \\ / : * ? \" < > or |.".to_owned()
        ))
    );
    assert_eq!(fixture.read("Roadmap.md"), b"x");
}

#[test]
fn a_name_taken_after_noura_checked_it_names_the_name_not_the_file() {
    let fixture = Fixture::new(&[("Draft.md", b"draft"), ("Docs/a.txt", b"a")]);
    let mut app = fixture.app();
    app.start_rename("Draft.md");

    let renamed = support::race(
        &fixture,
        &mut app,
        |app| app.commit_rename("Plan"),
        || std::fs::write(fixture.root.join("Plan.md"), b"other app").unwrap(),
    );

    assert!(!renamed);
    assert_eq!(
        app.renaming().unwrap().problem.as_deref(),
        Some("Something named \u{201c}Plan\u{201d} is already in this folder.")
    );
    assert_eq!(fixture.read("Draft.md"), b"draft");
    assert_eq!(fixture.read("Plan.md"), b"other app");

    app.cancel_rename();
    app.start_rename("Docs");
    let renamed = support::race(
        &fixture,
        &mut app,
        |app| app.commit_rename("Guides"),
        || std::fs::create_dir(fixture.root.join("Guides")).unwrap(),
    );
    assert!(!renamed);
    assert_eq!(
        app.renaming().unwrap().problem.as_deref(),
        Some("Something named \u{201c}Guides\u{201d} is already in this folder.")
    );
    assert_eq!(fixture.read("Docs/a.txt"), b"a");
}
