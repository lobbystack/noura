//! Seam 1: moving rows by dragging them inside the tree.

mod support;

use std::time::{Duration, Instant};

use app_core::{App, DropSpot, SPRING_OPEN_DELAY, TreeAction};
use support::{Fixture, current, document, note, notice, object_id, rows};

const PLAN_ID: &str = "01jbcdefghjkmnpqrstvwxyz00";

fn row_of(app: &App, relative_path: &str) -> usize {
    app.tree()
        .unwrap()
        .find_row(relative_path)
        .unwrap_or_else(|| panic!("{relative_path} isn't visible"))
}

fn expanded(app: &App, relative_path: &str) -> Option<bool> {
    let tree = app.tree().unwrap();
    tree.rows()[row_of(app, relative_path)].expanded
}

/// Drag `from` over `spots` in turn, then drop it, as the tree widget does.
fn drag(app: &mut App, from: &str, over: &[&str], at: Instant) {
    let row = row_of(app, from);
    let tree = app.tree_mut().unwrap();
    tree.start_drag(row);
    for path in over {
        let row = tree.find_row(path).unwrap();
        tree.drag_over(DropSpot::Row(row), at);
    }
    if let Some(TreeAction::Move {
        relative_path,
        folder,
    }) = tree.finish_drag()
    {
        app.move_into(&relative_path, &folder);
    }
}

#[test]
fn dropping_a_row_on_a_folder_moves_it_there_and_keeps_its_id_and_bytes() {
    let bytes = note(PLAN_ID, "Body\n");
    let fixture = Fixture::new(&[("Plan.md", &bytes), ("Docs/Guide.md", b"g")]);
    let mut app = fixture.app();
    app.open_file("Plan.md");

    drag(&mut app, "Plan.md", &["Docs"], Instant::now());

    assert_eq!(fixture.read("Docs/Plan.md"), bytes);
    assert_eq!(
        object_id(&fixture.read("Docs/Plan.md")),
        format!("note_{PLAN_ID}")
    );
    assert!(!fixture.exists("Plan.md"));
    assert_eq!(rows(&app), ["Docs", "Guide", "Plan"]);
    assert_eq!(current(&app).as_deref(), Some("Docs/Plan.md"));
    assert_eq!(document(&app).as_deref(), Some("Docs/Plan.md"));
}

#[test]
fn dropping_a_row_on_a_file_moves_it_into_that_files_folder() {
    let fixture = Fixture::new(&[("Plan.md", b"p"), ("Docs/Guide.md", b"g")]);
    let mut app = fixture.app();
    support::select(&mut app, "Docs/Guide.md");

    drag(&mut app, "Plan.md", &["Docs/Guide.md"], Instant::now());

    assert_eq!(fixture.read("Docs/Plan.md"), b"p");
}

#[test]
fn dropping_on_the_blank_part_of_the_tree_moves_to_the_top() {
    let fixture = Fixture::new(&[("Docs/Deep/Plan.md", b"p")]);
    let mut app = fixture.app();
    support::select(&mut app, "Docs/Deep/Plan.md");
    let row = row_of(&app, "Docs/Deep/Plan.md");
    let tree = app.tree_mut().unwrap();

    tree.start_drag(row);
    tree.drag_over(DropSpot::Blank, Instant::now());
    assert_eq!(tree.drop_target(), Some(""));
    let Some(TreeAction::Move {
        relative_path,
        folder,
    }) = tree.finish_drag()
    else {
        panic!("the drop did nothing");
    };
    app.move_into(&relative_path, &folder);

    assert_eq!(fixture.read("Plan.md"), b"p");
}

#[test]
fn a_moved_folder_keeps_its_contents_and_ids() {
    let bytes = note(PLAN_ID, "Body\n");
    let fixture = Fixture::new(&[("Docs/Plan.md", &bytes), ("Archive/old.txt", b"o")]);
    let mut app = fixture.app();

    drag(&mut app, "Docs", &["Archive"], Instant::now());

    assert_eq!(fixture.read("Archive/Docs/Plan.md"), bytes);
    assert_eq!(rows(&app), ["Archive", "Docs", "old.txt"]);
}

#[test]
fn a_folder_cant_move_into_itself_or_a_folder_inside_it() {
    let fixture = Fixture::new(&[("Docs/Deep/Plan.md", b"p"), ("Other.md", b"o")]);
    let mut app = fixture.app();
    support::select(&mut app, "Docs/Deep/Plan.md");
    let before = fixture.snapshot();
    let docs = row_of(&app, "Docs");
    let tree = app.tree_mut().unwrap();

    tree.start_drag(docs);
    for spot in ["Docs", "Docs/Deep", "Docs/Deep/Plan.md"] {
        let row = tree.find_row(spot).unwrap();
        tree.drag_over(DropSpot::Row(row), Instant::now());
        assert_eq!(tree.drop_target(), None, "{spot}");
    }
    assert_eq!(tree.finish_drag(), None);

    app.move_into("Docs", "Docs/Deep");
    assert_eq!(
        notice(&app),
        Some((
            "Couldn't move \u{201c}Docs\u{201d}".to_owned(),
            "A folder can't move into itself or a folder inside it.".to_owned()
        ))
    );
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn a_row_dropped_on_its_own_folder_stays_put() {
    let fixture = Fixture::new(&[("Docs/Plan.md", b"p")]);
    let mut app = fixture.app();
    support::select(&mut app, "Docs/Plan.md");
    let row = row_of(&app, "Docs/Plan.md");
    let tree = app.tree_mut().unwrap();

    tree.start_drag(row);
    tree.drag_over(
        DropSpot::Row(tree.find_row("Docs").unwrap()),
        Instant::now(),
    );

    assert_eq!(tree.drop_target(), None);
    assert_eq!(tree.finish_drag(), None);
}

#[test]
fn moving_onto_a_taken_name_says_so_and_changes_nothing() {
    let fixture = Fixture::new(&[("Plan.md", b"top"), ("Docs/plan.md", b"inner")]);
    let mut app = fixture.app();
    let before = fixture.snapshot();

    drag(&mut app, "Plan.md", &["Docs"], Instant::now());

    assert_eq!(
        notice(&app),
        Some((
            "Couldn't move \u{201c}Plan\u{201d}".to_owned(),
            "Something named \u{201c}Plan\u{201d} is already in \u{201c}Docs\u{201d}.".to_owned()
        ))
    );
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn resting_on_a_closed_folder_for_a_second_opens_it() {
    let fixture = Fixture::new(&[("Plan.md", b"p"), ("Docs/Guide.md", b"g")]);
    let mut app = fixture.app();
    let start = Instant::now();
    let plan = row_of(&app, "Plan.md");
    let docs = row_of(&app, "Docs");
    let tree = app.tree_mut().unwrap();

    tree.start_drag(plan);
    tree.drag_over(DropSpot::Row(docs), start);
    assert_eq!(tree.spring_open_at(), Some(start + SPRING_OPEN_DELAY));
    tree.drag_over(
        DropSpot::Row(docs),
        start + SPRING_OPEN_DELAY - Duration::from_millis(1),
    );
    assert_eq!(expanded(&app, "Docs"), Some(false));

    let tree = app.tree_mut().unwrap();
    tree.drag_over(DropSpot::Row(docs), start + SPRING_OPEN_DELAY);
    assert_eq!(expanded(&app, "Docs"), Some(true));
    assert_eq!(rows(&app), ["Docs", "Guide", "Plan"]);
}

#[test]
fn folders_opened_while_dragging_close_again_unless_they_received_the_drop() {
    let fixture = Fixture::new(&[
        ("Plan.md", b"p"),
        ("Docs/Deep/Guide.md", b"g"),
        ("Other/x.md", b"x"),
    ]);
    let mut app = fixture.app();
    let start = Instant::now();
    let later = start + SPRING_OPEN_DELAY;

    // Rest on Docs, then on Deep inside it, then drop on Other.
    let plan = row_of(&app, "Plan.md");
    let tree = app.tree_mut().unwrap();
    tree.start_drag(plan);
    let docs = tree.find_row("Docs").unwrap();
    tree.drag_over(DropSpot::Row(docs), start);
    tree.drag_over(DropSpot::Row(docs), later);
    let deep = tree.find_row("Docs/Deep").unwrap();
    tree.drag_over(DropSpot::Row(deep), later);
    tree.drag_over(DropSpot::Row(deep), later + SPRING_OPEN_DELAY);
    let other = tree.find_row("Other").unwrap();
    tree.drag_over(DropSpot::Row(other), later + SPRING_OPEN_DELAY);
    let action = tree.finish_drag();
    assert_eq!(
        action,
        Some(TreeAction::Move {
            relative_path: "Plan.md".to_owned(),
            folder: "Other".to_owned()
        })
    );
    assert_eq!(expanded(&app, "Docs"), Some(false));
    app.move_into("Plan.md", "Other");
    assert_eq!(expanded(&app, "Docs"), Some(false));

    // This time the drop lands in Deep, so Docs and Deep stay open.
    let plan = row_of(&app, "Other/Plan.md");
    let tree = app.tree_mut().unwrap();
    tree.start_drag(plan);
    let docs = tree.find_row("Docs").unwrap();
    tree.drag_over(DropSpot::Row(docs), start);
    tree.drag_over(DropSpot::Row(docs), later);
    let deep = tree.find_row("Docs/Deep").unwrap();
    tree.drag_over(DropSpot::Row(deep), later);
    tree.drag_over(DropSpot::Row(deep), later + SPRING_OPEN_DELAY);
    let Some(TreeAction::Move {
        relative_path,
        folder,
    }) = tree.finish_drag()
    else {
        panic!("the drop did nothing");
    };
    app.move_into(&relative_path, &folder);

    assert_eq!(fixture.read("Docs/Deep/Plan.md"), b"p");
    assert_eq!(expanded(&app, "Docs"), Some(true));
    assert_eq!(expanded(&app, "Docs/Deep"), Some(true));
}

#[test]
fn cancelling_a_drag_moves_nothing_and_closes_the_folders_it_opened() {
    let fixture = Fixture::new(&[("Plan.md", b"p"), ("Docs/Guide.md", b"g")]);
    let mut app = fixture.app();
    let before = fixture.snapshot();
    let start = Instant::now();
    let plan = row_of(&app, "Plan.md");
    let docs = row_of(&app, "Docs");
    let tree = app.tree_mut().unwrap();

    tree.start_drag(plan);
    tree.drag_over(DropSpot::Row(docs), start);
    tree.drag_over(DropSpot::Row(docs), start + SPRING_OPEN_DELAY);
    assert_eq!(tree.dragging(), Some("Plan.md"));
    tree.cancel_drag();

    assert_eq!(tree.dragging(), None);
    assert_eq!(tree.finish_drag(), None);
    assert_eq!(expanded(&app, "Docs"), Some(false));
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn leaving_the_tree_while_dragging_drops_nowhere() {
    let fixture = Fixture::new(&[("Plan.md", b"p"), ("Docs/Guide.md", b"g")]);
    let mut app = fixture.app();
    let plan = row_of(&app, "Plan.md");
    let docs = row_of(&app, "Docs");
    let tree = app.tree_mut().unwrap();

    tree.start_drag(plan);
    tree.drag_over(DropSpot::Row(docs), Instant::now());
    tree.drag_over(DropSpot::Away, Instant::now());

    assert_eq!(tree.drop_target(), None);
    assert_eq!(tree.finish_drag(), None);
}
