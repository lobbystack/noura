//! Seam 1: the file tree's keys for renaming, trashing and the row menu.

mod support;

use app_core::{OpenAs, TreeAction, TreeKey};
use support::{Fixture, current, document};

fn rename_target(app: &app_core::App) -> Option<String> {
    app.renaming().map(|rename| rename.relative_path.clone())
}

#[test]
fn enter_opens_a_file_and_renames_the_file_that_is_already_open() {
    let fixture = Fixture::new(&[("A.md", b"a"), ("B.md", b"b")]);
    let mut app = fixture.app();
    support::select(&mut app, "A.md");

    assert_eq!(app.press_tree_key(TreeKey::Enter), None);
    assert_eq!(document(&app).as_deref(), Some("A.md"));
    assert_eq!(rename_target(&app), None);

    app.press_tree_key(TreeKey::Enter);
    assert_eq!(rename_target(&app).as_deref(), Some("A.md"));
    app.cancel_rename();

    app.press_tree_key(TreeKey::Down);
    app.press_tree_key(TreeKey::Enter);
    assert_eq!(document(&app).as_deref(), Some("B.md"));
    assert_eq!(rename_target(&app), None);
}

#[test]
fn space_never_renames() {
    let fixture = Fixture::new(&[("A.md", b"a")]);
    let mut app = fixture.app();
    support::select(&mut app, "A.md");
    app.open_file("A.md", OpenAs::Preview);

    app.press_tree_key(TreeKey::Activate);

    assert_eq!(rename_target(&app), None);
}

#[test]
fn f2_renames_the_current_row() {
    let fixture = Fixture::new(&[("Docs/A.md", b"a")]);
    let mut app = fixture.app();
    support::select(&mut app, "Docs");

    app.press_tree_key(TreeKey::Rename);

    assert_eq!(rename_target(&app).as_deref(), Some("Docs"));
}

#[test]
fn delete_trashes_a_file_at_once() {
    let fixture = Fixture::new(&[("A.md", b"a"), ("B.md", b"b")]);
    let mut app = fixture.app();
    support::select(&mut app, "A.md");

    app.press_tree_key(TreeKey::Trash);

    assert!(!fixture.exists("A.md"));
    assert!(app.pending_trash().is_none());
    assert_eq!(current(&app).as_deref(), Some("B.md"));
}

#[test]
fn delete_on_a_folder_asks_first() {
    let fixture = Fixture::new(&[("Docs/A.md", b"a"), ("Keep/B.md", b"b")]);
    let mut app = fixture.app();
    support::select(&mut app, "Docs");

    app.press_tree_key(TreeKey::Trash);
    let (path, question) = app.pending_trash().unwrap();
    assert_eq!(path, "Docs");
    assert_eq!(question.title, "Move \u{201c}Docs\u{201d} to the trash?");
    assert!(fixture.exists("Docs/A.md"));

    app.cancel_trash();
    assert!(app.pending_trash().is_none());
    assert!(fixture.exists("Docs/A.md"));

    app.request_trash("Docs");
    app.confirm_trash();
    assert!(!fixture.exists("Docs"));
    assert!(app.pending_trash().is_none());
}

#[test]
fn the_menu_key_asks_for_the_current_rows_menu() {
    let fixture = Fixture::new(&[("Docs/A.md", b"a")]);
    let mut app = fixture.app();
    support::select(&mut app, "Docs/A.md");

    assert_eq!(
        app.press_tree_key(TreeKey::Menu),
        Some(TreeAction::Menu {
            relative_path: "Docs/A.md".to_owned()
        })
    );
}

#[test]
fn collapse_all_closes_every_folder() {
    let fixture = Fixture::new(&[("Docs/Deep/A.md", b"a"), ("Other/B.md", b"b")]);
    let mut app = fixture.app();
    support::select(&mut app, "Docs/Deep/A.md");
    support::select(&mut app, "Other/B.md");

    app.collapse_all();

    assert_eq!(support::rows(&app), ["Docs", "Other"]);
    assert_eq!(current(&app).as_deref(), Some("Other"));
}
