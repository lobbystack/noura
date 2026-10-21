//! Seam 1: the tree operations reach the change stream as Noura's own
//! changes, never as changes from outside, while the native watcher
//! reports them as it reports any change to the folder.

mod support;

use std::{
    sync::{
        Arc,
        mpsc::{Receiver, channel},
    },
    time::{Duration, Instant},
};

use app_core::{App, Change, Origin};
use support::{FakeSystem, Fixture, document, note, rows, select};

const PLAN_ID: &str = "01jbcdefghjkmnpqrstvwxyz00";
const SPEC_ID: &str = "01jbcdefghjkmnpqrstvwxyz01";

/// How long a test waits for the change stream before it fails.
const PATIENCE: Duration = Duration::from_secs(15);

/// How long a test keeps listening once Noura's change arrives, to catch
/// a late watcher report that would mark it as from outside. The watch
/// thread polls every 100 ms, and platforms report within a second.
const SETTLE: Duration = Duration::from_secs(2);

/// An app with a workspace open and every change it announces.
struct Live {
    fixture: Fixture,
    app: App,
    changes: Receiver<Change>,
    seen: Vec<Change>,
}

impl Live {
    fn open(files: &[(&str, &[u8])]) -> Self {
        Self::open_with(files, FakeSystem::new())
    }

    fn open_with(files: &[(&str, &[u8])], system: Arc<FakeSystem>) -> Self {
        let fixture = Fixture::new(files);
        let app = fixture.app_with(system);
        let (sender, changes) = channel();
        app.subscribe(move |change| {
            let _ = sender.send(change);
        });
        let mut live = Self {
            fixture,
            app,
            changes,
            seen: Vec::new(),
        };
        // Late reports of the files the fixture wrote belong to no test.
        live.settle();
        live.seen.clear();
        live
    }

    /// Wait on the change stream, polling after each change, until Noura's
    /// change arrives and `done` holds for the view state. Then keep
    /// listening for `SETTLE` and check that nothing came from outside.
    fn expect_own_change(&mut self, what: &str, done: impl Fn(&mut App) -> bool) {
        let deadline = Instant::now() + PATIENCE;
        loop {
            self.app.poll();
            self.seen.extend(self.changes.try_iter());
            if self.seen.contains(&Change::Files(Origin::Noura)) && done(&mut self.app) {
                break;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(
                !left.is_zero(),
                "timed out waiting for {what} as Noura's own change: {:?}",
                self.seen
            );
            if let Ok(change) = self.changes.recv_timeout(left) {
                self.seen.push(change);
            }
        }
        self.settle();
        assert!(
            !self.seen.contains(&Change::Files(Origin::Outside)),
            "{what} read as a change from outside: {:?}",
            self.seen
        );
        assert!(done(&mut self.app), "{what} no longer shows after settling");
        assert_eq!(self.app.problems(), [], "after {what}");
        self.seen.clear();
    }

    /// Keep polling and recording changes for `SETTLE`.
    fn settle(&mut self) {
        let deadline = Instant::now() + SETTLE;
        loop {
            self.app.poll();
            self.seen.extend(self.changes.try_iter());
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return;
            }
            if let Ok(change) = self
                .changes
                .recv_timeout(left.min(Duration::from_millis(100)))
            {
                self.seen.push(change);
            }
        }
    }
}

/// Whether the tree holds the item at `relative_path`. It opens the
/// folders above the item.
fn shows(app: &mut App, relative_path: &str) -> bool {
    app.tree_mut().unwrap().select(relative_path)
}

#[test]
fn a_new_folder_is_noura_s_own_change() {
    let mut live = Live::open(&[("Plan.md", &note(PLAN_ID, "Plan\n"))]);

    live.app.new_folder(Some(""));

    live.expect_own_change("the new folder", |app| rows(app) == ["Untitled", "Plan"]);
}

#[test]
fn renaming_a_note_a_picture_and_a_folder_is_noura_s_own_change() {
    let mut live = Live::open(&[
        ("Plan.md", &note(PLAN_ID, "Plan\n")),
        ("cat.jpg", b"jpg"),
        ("Docs/Spec.md", &note(SPEC_ID, "Spec\n")),
    ]);

    live.app.start_rename("Plan.md");
    live.app.commit_rename("Roadmap");
    live.expect_own_change("the renamed note", |app| shows(app, "Roadmap.md"));
    live.app.start_rename("cat.jpg");
    live.app.commit_rename("kitten.jpg");
    live.expect_own_change("the renamed picture", |app| shows(app, "kitten.jpg"));
    live.app.start_rename("Docs");
    live.app.commit_rename("Notes");
    live.expect_own_change("the renamed folder", |app| shows(app, "Notes/Spec.md"));
}

/// On a case-insensitive volume the old and new names name one file.
#[test]
fn a_case_only_rename_is_noura_s_own_change() {
    let mut live = Live::open(&[
        ("Plan.md", &note(PLAN_ID, "Plan\n")),
        ("Docs/Spec.md", &note(SPEC_ID, "Spec\n")),
    ]);

    live.app.start_rename("Plan.md");
    live.app.commit_rename("plan");
    live.expect_own_change("the renamed note", |app| shows(app, "plan.md"));
    live.app.start_rename("Docs");
    live.app.commit_rename("docs");
    live.expect_own_change("the renamed folder", |app| shows(app, "docs/Spec.md"));
}

#[test]
fn moving_files_and_a_folder_into_another_folder_is_noura_s_own_change() {
    let mut live = Live::open(&[
        ("Plan.md", &note(PLAN_ID, "Plan\n")),
        ("scan.pdf", b"%PDF-1.7\n"),
        ("Photos/cat.jpg", b"jpg"),
        ("Docs/Spec.md", &note(SPEC_ID, "Spec\n")),
    ]);

    live.app.move_into("Plan.md", "Docs");
    live.expect_own_change("the moved note", |app| shows(app, "Docs/Plan.md"));
    live.app.move_into("scan.pdf", "Docs");
    live.expect_own_change("the moved PDF", |app| shows(app, "Docs/scan.pdf"));
    live.app.move_into("Photos", "Docs");
    live.expect_own_change("the moved folder", |app| shows(app, "Docs/Photos/cat.jpg"));
}

#[test]
fn trashing_a_file_and_a_folder_is_noura_s_own_change() {
    for system in [FakeSystem::new(), FakeSystem::without_trash()] {
        let mut live = Live::open_with(
            &[
                ("Plan.md", &note(PLAN_ID, "Plan\n")),
                ("Docs/Spec.md", &note(SPEC_ID, "Spec\n")),
                ("Docs/cat.jpg", b"jpg"),
            ],
            system,
        );

        live.app.trash("Plan.md");
        live.expect_own_change("the trashed note", |app| rows(app) == ["Docs"]);
        live.app.request_trash("Docs");
        live.app.confirm_trash();
        live.expect_own_change("the trashed folder", |app| rows(app).is_empty());
    }
}

#[test]
fn duplicating_a_note_and_a_picture_is_noura_s_own_change() {
    let mut live = Live::open(&[("Plan.md", &note(PLAN_ID, "Plan\n")), ("cat.jpg", b"jpg")]);

    live.app.duplicate("Plan.md");
    live.expect_own_change("the note's copy", |app| shows(app, "Plan 1.md"));
    live.app.duplicate("cat.jpg");
    live.expect_own_change("the picture's copy", |app| shows(app, "cat 1.jpg"));
}

#[test]
fn importing_files_and_folders_is_noura_s_own_change() {
    let mut live = Live::open(&[("Docs/Spec.md", &note(SPEC_ID, "Spec\n"))]);
    let outside = live.fixture.outside();
    std::fs::write(outside.join("scan.pdf"), b"%PDF-1.7\n").unwrap();
    std::fs::create_dir(outside.join("Album")).unwrap();
    std::fs::write(outside.join("Album/dog.jpg"), b"dog").unwrap();
    std::fs::write(outside.join("Album/Idea.md"), b"# Idea\n").unwrap();
    // A copy of a note the workspace holds gets a new ID.
    std::fs::write(outside.join("Spec.md"), note(SPEC_ID, "Spec\n")).unwrap();
    select(&mut live.app, "Docs/Spec.md");

    live.app
        .workspace()
        .unwrap()
        .import(
            &[
                outside.join("scan.pdf"),
                outside.join("Album"),
                outside.join("Spec.md"),
            ],
            "Docs",
        )
        .unwrap();

    live.expect_own_change("the imported files", |app| {
        [
            "Docs/scan.pdf",
            "Docs/Album/dog.jpg",
            "Docs/Album/Idea.md",
            "Docs/Spec 1.md",
        ]
        .iter()
        .all(|path| shows(app, path))
    });
}

/// The watch thread lists the workspace after each change. A listing that
/// arrives after a later change already shows must not undo it.
#[test]
fn a_listing_from_before_the_last_change_never_undoes_it() {
    let mut live = Live::open(&[]);

    live.app.new_note(Some(""));
    // The watch thread lists the new note before the app looks.
    std::thread::sleep(Duration::from_millis(500));
    live.app.start_rename("Untitled.md");
    live.app.commit_rename("Roadmap");
    live.app.poll();

    assert_eq!(rows(&live.app), ["Roadmap"]);
    assert_eq!(document(&live.app).as_deref(), Some("Roadmap.md"));
    live.expect_own_change("the renamed note", |app| {
        rows(app) == ["Roadmap"] && document(app).as_deref() == Some("Roadmap.md")
    });
}
