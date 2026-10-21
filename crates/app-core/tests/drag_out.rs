//! Seam 1: rows dragged out of the window, which become native drags on
//! macOS and Windows.

mod support;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Instant,
};

use app_core::{App, AppError, DropSpot, NoticeKind, PendingSaves, Platform};
use support::{FakeSystem, Fixture, notice};

fn row_of(app: &App, relative_path: &str) -> usize {
    app.tree()
        .unwrap()
        .find_row(relative_path)
        .unwrap_or_else(|| panic!("{relative_path} isn't visible"))
}

/// Start dragging the row at `relative_path` and carry it past the
/// window's edge, as the tree widget does.
fn drag_out(app: &mut App, relative_path: &str) {
    let row = row_of(app, relative_path);
    app.start_tree_drag(row);
    app.drag_tree_over(DropSpot::Away, Instant::now());
    app.drag_row_out();
}

/// The system carries `paths` over `spot` and drops them there.
fn drop_on(app: &mut App, paths: &[PathBuf], spot: DropSpot) {
    app.drag_files_over(paths, spot, Instant::now());
    app.drop_files(paths);
}

/// The folder the tree lights up while the system carries `paths` over
/// the row at `relative_path`.
fn target_over(app: &mut App, paths: &[PathBuf], relative_path: &str) -> Option<String> {
    let spot = DropSpot::Row(row_of(app, relative_path));
    app.drag_files_over(paths, spot, Instant::now());
    app.tree().unwrap().drop_target().map(str::to_owned)
}

/// A save still waiting to reach disk, as the editor keeps one while you
/// type. Finishing it writes `bytes` to the file at `relative_path`.
struct FakePendingSave {
    root: std::path::PathBuf,
    relative_path: String,
    bytes: Vec<u8>,
    refuse: bool,
    finished: Mutex<Vec<String>>,
}

impl FakePendingSave {
    fn new(fixture: &Fixture, relative_path: &str, bytes: &[u8]) -> Arc<Self> {
        Arc::new(Self {
            root: fixture.root.clone(),
            relative_path: relative_path.to_owned(),
            bytes: bytes.to_vec(),
            refuse: false,
            finished: Mutex::new(Vec::new()),
        })
    }

    fn refusing(fixture: &Fixture) -> Arc<Self> {
        Arc::new(Self {
            refuse: true,
            ..Arc::into_inner(Self::new(fixture, "Plan.md", b"")).unwrap()
        })
    }

    fn finished(&self) -> Vec<String> {
        self.finished.lock().unwrap().clone()
    }
}

impl PendingSaves for FakePendingSave {
    fn save_now(&self, relative_path: &str) -> Result<(), AppError> {
        self.finished.lock().unwrap().push(relative_path.to_owned());
        if self.refuse {
            return Err(AppError::new("save_failed", "The disk is full."));
        }
        let inside = self.relative_path == relative_path
            || self.relative_path.starts_with(&format!("{relative_path}/"));
        if inside {
            std::fs::write(self.root.join(&self.relative_path), &self.bytes).unwrap();
        }
        Ok(())
    }
}

#[test]
fn a_row_dragged_past_the_window_edge_becomes_a_native_drag_once_its_pending_save_lands() {
    for platform in [Platform::MacOs, Platform::Windows] {
        let fixture = Fixture::new(&[("Plan.md", b"old"), ("Readme.md", b"r")]);
        let mut app = fixture.app_with(FakeSystem::on(platform));
        let saves = FakePendingSave::new(&fixture, "Plan.md", b"typed");
        app.set_pending_saves(saves.clone());

        drag_out(&mut app, "Plan.md");

        let drag = app.take_drag_out().expect("a native drag starts");
        assert_eq!(drag.path, fixture.root.join("Plan.md"));
        assert_eq!(drag.name, "Plan");
        assert_eq!(saves.finished(), ["Plan.md"]);
        assert_eq!(std::fs::read(&drag.path).unwrap(), b"typed");
        assert_eq!(app.tree().unwrap().dragging(), None);
        assert!(app.take_drag_out().is_none(), "the drag starts once");
        assert_eq!(notice(&app), None);
    }
}

#[test]
fn a_folder_dragged_out_saves_the_files_inside_it_first() {
    let fixture = Fixture::new(&[("Docs/Plan.md", b"old")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let saves = FakePendingSave::new(&fixture, "Docs/Plan.md", b"typed");
    app.set_pending_saves(saves.clone());

    drag_out(&mut app, "Docs");

    let drag = app.take_drag_out().unwrap();
    assert_eq!(drag.path, fixture.root.join("Docs"));
    assert_eq!(drag.name, "Docs");
    assert_eq!(saves.finished(), ["Docs"]);
    assert_eq!(fixture.read("Docs/Plan.md"), b"typed");
}

#[test]
fn a_pending_save_that_fails_keeps_the_row_in_the_window_and_says_why() {
    let fixture = Fixture::new(&[("Plan.md", b"old")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    app.set_pending_saves(FakePendingSave::refusing(&fixture));

    drag_out(&mut app, "Plan.md");

    assert!(app.take_drag_out().is_none());
    assert_eq!(
        notice(&app),
        Some((
            "Couldn't save \u{201c}Plan\u{201d} before dragging it out".to_owned(),
            "The disk is full.".to_owned()
        ))
    );
}

#[test]
fn a_native_drag_that_fails_to_start_says_so() {
    let fixture = Fixture::new(&[("Plan.md", b"p")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::Windows));
    drag_out(&mut app, "Plan.md");
    app.take_drag_out().unwrap();

    app.drag_out_failed();

    assert_eq!(
        notice(&app),
        Some((
            "Couldn't drag \u{201c}Plan\u{201d} out".to_owned(),
            "Noura couldn't start the drag. Try again.".to_owned()
        ))
    );
}

#[test]
fn on_linux_a_row_past_the_window_edge_stays_an_in_tree_drag() {
    let fixture = Fixture::new(&[("Plan.md", b"p"), ("Docs/Guide.md", b"g")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::Linux));

    drag_out(&mut app, "Plan.md");

    assert!(app.take_drag_out().is_none());
    assert_eq!(notice(&app), None);
    let tree = app.tree_mut().unwrap();
    assert_eq!(tree.dragging(), Some("Plan.md"));
    let docs = tree.find_row("Docs").unwrap();
    tree.drag_over(DropSpot::Row(docs), Instant::now());
    let action = tree.finish_drag().unwrap();
    app.apply(action);
    assert_eq!(fixture.read("Docs/Plan.md"), b"p");
}

#[test]
fn a_row_dragged_out_and_dropped_back_on_the_tree_moves_and_makes_no_copy() {
    let fixture = Fixture::new(&[("Plan.md", b"p"), ("Docs/Guide.md", b"g")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    drag_out(&mut app, "Plan.md");
    let drag = app.take_drag_out().unwrap();

    // The system drags the file back over the tree and drops it on Docs.
    let docs = DropSpot::Row(row_of(&app, "Docs"));
    drop_on(&mut app, &[drag.path], docs);

    assert_eq!(fixture.read("Docs/Plan.md"), b"p");
    assert!(!fixture.exists("Plan.md"));
    assert!(!fixture.exists("Docs/Plan 1.md"));
    assert_eq!(support::current(&app).as_deref(), Some("Docs/Plan.md"));
}

#[test]
fn a_dragged_out_row_coming_back_lights_up_only_the_folders_it_could_move_into() {
    let fixture = Fixture::new(&[
        ("Plan.md", b"p"),
        ("Docs/Guide.md", b"g"),
        ("Archive/Old.md", b"o"),
    ]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));

    drag_out(&mut app, "Docs");
    let docs = [app.take_drag_out().unwrap().path];
    assert_eq!(target_over(&mut app, &docs, "Docs"), None);
    assert_eq!(target_over(&mut app, &docs, "Plan.md"), None);
    assert_eq!(
        target_over(&mut app, &docs, "Archive").as_deref(),
        Some("Archive")
    );
    app.files_left();

    drag_out(&mut app, "Plan.md");
    let plan = [app.take_drag_out().unwrap().path];
    assert_eq!(
        target_over(&mut app, &plan, "Archive").as_deref(),
        Some("Archive")
    );
    assert_eq!(target_over(&mut app, &plan, "Plan.md"), None);
    app.drag_files_over(&plan, DropSpot::Blank, Instant::now());
    assert_eq!(app.tree().unwrap().drop_target(), None);

    // Other files light up the folder the row couldn't move into.
    let other = [fixture.root.join("Archive/Old.md")];
    assert_eq!(
        target_over(&mut app, &other, "Plan.md").as_deref(),
        Some("")
    );
}

#[test]
fn after_the_native_drag_ends_the_same_file_dropped_from_the_file_manager_copies() {
    let fixture = Fixture::new(&[("Plan.md", b"p"), ("Docs/Guide.md", b"g")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    drag_out(&mut app, "Plan.md");
    let drag = app.take_drag_out().unwrap();
    // The file landed on the desktop, or the drag was cancelled.
    app.drag_out_ended();

    // Later the workspace's own Plan.md comes back from a Finder window.
    drop_on(&mut app, std::slice::from_ref(&drag.path), DropSpot::Blank);
    assert_eq!(fixture.read("Plan.md"), b"p");
    assert_eq!(fixture.read("Plan 1.md"), b"p");
    assert_eq!(
        notice(&app),
        Some((
            "Copied \u{201c}Plan 1\u{201d} to the top of the workspace".to_owned(),
            String::new()
        ))
    );

    let docs = DropSpot::Row(row_of(&app, "Docs"));
    drop_on(&mut app, &[drag.path], docs);
    assert_eq!(fixture.read("Docs/Plan.md"), b"p");
    assert_eq!(fixture.read("Plan.md"), b"p");
}

#[test]
fn after_the_native_drag_ends_a_folder_dropped_on_itself_is_refused() {
    let fixture = Fixture::new(&[("Docs/Guide.md", b"g"), ("Readme.md", b"r")]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::Windows));
    drag_out(&mut app, "Docs");
    let docs = [app.take_drag_out().unwrap().path];
    app.drag_out_ended();
    let before = fixture.snapshot();

    assert_eq!(
        target_over(&mut app, &docs, "Docs").as_deref(),
        Some("Docs")
    );
    app.drop_files(&docs);

    assert_eq!(
        notice(&app),
        Some((
            "Couldn't copy the dropped files".to_owned(),
            "Noura can't copy a folder into itself.".to_owned()
        ))
    );
    assert_eq!(app.notice().unwrap().kind, NoticeKind::Problem);
    assert_eq!(fixture.snapshot(), before);
}
