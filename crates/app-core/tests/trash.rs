//! Seam 1: moving files and folders to the trash, with a fake system
//! trash.

mod support;

use app_core::NoticeKind;
use support::{Call, FakeSystem, Fixture, current, document, notice, rows};

/// The one folder in the workspace's trash folder, named for when the item
/// went there.
fn stamped_folder(fixture: &Fixture) -> std::path::PathBuf {
    let trash = fixture.root.join(".noura/trash");
    let folders: Vec<_> = std::fs::read_dir(&trash)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(folders.len(), 1, "{folders:?}");
    folders.into_iter().next().unwrap()
}

#[test]
fn move_to_trash_uses_the_system_trash_and_says_so() {
    let fixture = Fixture::new(&[("Docs/Plan.md", b"plan"), ("Readme.md", b"r")]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());

    app.trash("Docs/Plan.md");

    assert!(!fixture.exists("Docs/Plan.md"));
    assert_eq!(std::fs::read(system.in_bin("Plan.md")).unwrap(), b"plan");
    assert_eq!(
        system.calls(),
        [Call::Trash(fixture.root.join("Docs").join("Plan.md"))]
    );
    assert_eq!(
        notice(&app),
        Some((
            "Moved \u{201c}Plan\u{201d} to the trash".to_owned(),
            "You can restore it from your system's trash.".to_owned()
        ))
    );
    assert_eq!(app.notice().unwrap().kind, NoticeKind::Done);
    assert_eq!(
        std::fs::read_dir(fixture.root.join(".noura/trash"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn when_the_system_trash_refuses_the_item_goes_to_the_workspace_trash_folder() {
    let fixture = Fixture::new(&[("Docs/Deep/Plan.md", b"plan")]);
    let system = FakeSystem::refusing_trash();
    let mut app = fixture.app_with(system.clone());

    app.trash("Docs/Deep/Plan.md");

    assert!(!fixture.exists("Docs/Deep/Plan.md"));
    let stamped = stamped_folder(&fixture);
    assert_eq!(
        std::fs::read(stamped.join("Docs/Deep/Plan.md")).unwrap(),
        b"plan"
    );
    assert_eq!(system.calls().len(), 1);
    let stamp = stamped.file_name().unwrap().to_str().unwrap();
    assert_eq!(
        notice(&app),
        Some((
            "Moved \u{201c}Plan\u{201d} to .noura/trash".to_owned(),
            format!(
                "Noura couldn't use the system trash and moved it to \
                 .noura/trash/{stamp}/Docs/Deep/Plan.md in this workspace."
            )
        ))
    );
}

#[test]
fn without_a_system_trash_a_folder_goes_to_the_workspace_trash_folder() {
    let fixture = Fixture::new(&[("Docs/Plan.md", b"plan"), ("Docs/Deep/Inner.md", b"i")]);
    let system = FakeSystem::without_trash();
    let mut app = fixture.app_with(system.clone());

    app.trash("Docs");

    assert!(!fixture.exists("Docs"));
    let stamped = stamped_folder(&fixture);
    assert_eq!(
        std::fs::read(stamped.join("Docs/Plan.md")).unwrap(),
        b"plan"
    );
    assert_eq!(
        std::fs::read(stamped.join("Docs/Deep/Inner.md")).unwrap(),
        b"i"
    );
    assert!(system.calls().is_empty());
    assert_eq!(
        notice(&app).unwrap().0,
        "Moved \u{201c}Docs\u{201d} to .noura/trash"
    );
    assert!(rows(&app).is_empty());
}

#[test]
fn a_folder_asks_first_and_says_where_it_goes() {
    let fixture = Fixture::new(&[("Docs/Plan.md", b"plan"), ("Readme.md", b"r")]);

    let app = fixture.app_with(FakeSystem::new());
    let question = app.trash_question("Docs").unwrap();
    assert_eq!(question.title, "Move \u{201c}Docs\u{201d} to the trash?");
    assert_eq!(
        question.message,
        "The folder and everything in it go to your system's trash, where you can \
         restore them."
    );
    assert_eq!(app.trash_question("Readme.md"), None);
    drop(app);

    let app = fixture.app_with(FakeSystem::without_trash());
    assert_eq!(
        app.trash_question("Docs").unwrap().message,
        "The folder and everything in it move to .noura/trash in this workspace."
    );
    assert!(fixture.exists("Docs/Plan.md"));
}

#[test]
fn trashing_the_open_file_closes_it_and_the_next_row_becomes_current() {
    let fixture = Fixture::new(&[("A.md", b"a"), ("B.md", b"b"), ("C.md", b"c")]);
    let mut app = fixture.app();
    support::select(&mut app, "B.md");
    app.open_file("B.md");

    app.trash("B.md");

    assert_eq!(document(&app), None);
    assert_eq!(current(&app).as_deref(), Some("C.md"));
    assert_eq!(rows(&app), ["A", "C"]);

    support::select(&mut app, "C.md");
    app.trash("C.md");
    assert_eq!(current(&app).as_deref(), Some("A.md"));
}

#[test]
fn trashing_a_folder_closes_a_file_open_inside_it() {
    let fixture = Fixture::new(&[("Docs/Plan.md", b"plan"), ("Readme.md", b"r")]);
    let mut app = fixture.app();
    app.open_file("Docs/Plan.md");

    app.trash("Docs");

    assert_eq!(document(&app), None);
}

#[test]
fn trash_refuses_items_outside_the_workspace_and_says_when_one_left() {
    let fixture = Fixture::new(&[("Plan.md", b"plan")]);
    let outside = fixture.outside();
    std::fs::write(outside.join("Keep.md"), b"keep").unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(outside.join("Keep.md"), fixture.root.join("Link.md")).unwrap();
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let before = fixture.snapshot();

    for path in ["", "..", "../Outside/Keep.md", "/tmp", ".noura", "Link.md"] {
        app.trash(path);
        assert!(app.notice().is_some(), "{path}");
        assert_eq!(app.notice().unwrap().kind, NoticeKind::Problem, "{path}");
    }
    assert_eq!(fixture.snapshot(), before);
    assert_eq!(std::fs::read(outside.join("Keep.md")).unwrap(), b"keep");
    assert!(system.calls().is_empty());

    std::fs::remove_file(fixture.root.join("Plan.md")).unwrap();
    app.trash("Plan.md");
    assert_eq!(
        notice(&app),
        Some((
            "Couldn't move \u{201c}Plan\u{201d} to the trash".to_owned(),
            "Plan.md isn't in the workspace anymore.".to_owned()
        ))
    );
}

#[test]
fn the_next_action_replaces_the_trash_notice() {
    let fixture = Fixture::new(&[("A.md", b"a"), ("B.md", b"b")]);
    let mut app = fixture.app();
    app.trash("A.md");
    assert!(app.notice().is_some());

    app.start_rename("B.md");
    assert!(app.commit_rename("C"));

    assert_eq!(app.notice(), None);
}
