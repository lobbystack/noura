//! Seam 1: checking for, downloading and installing app updates, with a
//! fake update source in place of the signed feed.

use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, Sender, channel},
    },
    time::{Duration, Instant},
};

use app_core::{
    App, AppError, Change, FIRST_UPDATE_CHECK, PendingSaves, PendingUpdate, PendingWrite, Screen,
    System, UpdateError, UpdateErrorKind, UpdateSource, UpdateStatus,
};

const HOURS: u64 = 60 * 60;

/// Work that runs inside a fake install.
type WhileInstalling = Box<dyn FnOnce() + Send>;

/// What the fake source does when asked, and a log of what it was asked.
#[derive(Clone, Default)]
struct FakeSource {
    log: Arc<Mutex<Vec<String>>>,
    release: Arc<Mutex<Option<String>>>,
    /// While set, the next download waits for a message.
    download_gate: Arc<Mutex<Option<Receiver<()>>>>,
    check_error: Arc<Mutex<Option<UpdateErrorKind>>>,
    download_error: Arc<Mutex<Option<UpdateErrorKind>>>,
    /// A file whose bytes the install logs, to show what reached disk
    /// before it ran.
    watched: Arc<Mutex<Option<PathBuf>>>,
    install_fails: Arc<Mutex<bool>>,
    /// Runs inside the next install, as you might keep working while it
    /// runs.
    during_install: Arc<Mutex<Option<WhileInstalling>>>,
    /// A file whose bytes the restart logs, to show what reached disk
    /// before Noura quit.
    restart_sees: Arc<Mutex<Option<PathBuf>>>,
}

impl FakeSource {
    /// Hold the next download until the returned sender sends.
    fn hold_downloads(&self) -> Sender<()> {
        let (sender, receiver) = channel();
        *self.download_gate.lock().unwrap() = Some(receiver);
        sender
    }

    fn calls(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn checks(&self) -> usize {
        self.calls().iter().filter(|call| *call == "check").count()
    }
}

impl UpdateSource for FakeSource {
    fn check(&self) -> Result<Option<Box<dyn PendingUpdate>>, UpdateError> {
        self.log.lock().unwrap().push("check".into());
        if let Some(kind) = *self.check_error.lock().unwrap() {
            return Err(UpdateError::new(kind, "fake check failure"));
        }
        Ok(self.release.lock().unwrap().clone().map(|version| {
            Box::new(FakeUpdate {
                version,
                log: self.log.clone(),
                gate: self.download_gate.lock().unwrap().take(),
                error: *self.download_error.lock().unwrap(),
                watched: self.watched.clone(),
                install_fails: self.install_fails.clone(),
                during_install: self.during_install.clone(),
            }) as Box<dyn PendingUpdate>
        }))
    }

    fn restart(&self) -> Result<(), UpdateError> {
        let entry = match &*self.restart_sees.lock().unwrap() {
            Some(path) => format!("restart saw {}", std::fs::read_to_string(path).unwrap()),
            None => "restart".into(),
        };
        self.log.lock().unwrap().push(entry);
        Ok(())
    }
}

struct FakeUpdate {
    version: String,
    log: Arc<Mutex<Vec<String>>>,
    gate: Option<Receiver<()>>,
    error: Option<UpdateErrorKind>,
    watched: Arc<Mutex<Option<PathBuf>>>,
    install_fails: Arc<Mutex<bool>>,
    during_install: Arc<Mutex<Option<WhileInstalling>>>,
}

impl PendingUpdate for FakeUpdate {
    fn version(&self) -> &str {
        &self.version
    }

    fn notes(&self) -> Option<&str> {
        None
    }

    fn download(&mut self) -> Result<(), UpdateError> {
        if let Some(gate) = &self.gate {
            gate.recv().unwrap();
        }
        self.log.lock().unwrap().push("download".into());
        match self.error {
            Some(kind) => Err(UpdateError::new(kind, "fake download failure")),
            None => Ok(()),
        }
    }

    fn install(&mut self) -> Result<(), UpdateError> {
        let entry = match &*self.watched.lock().unwrap() {
            Some(path) => format!("install saw {}", std::fs::read_to_string(path).unwrap()),
            None => "install".into(),
        };
        self.log.lock().unwrap().push(entry);
        if let Some(work) = self.during_install.lock().unwrap().take() {
            work();
        }
        if *self.install_fails.lock().unwrap() {
            return Err(UpdateError::new(
                UpdateErrorKind::Install,
                "fake install failure",
            ));
        }
        Ok(())
    }
}

/// Poll at `now` until no update work runs in the background.
fn settle(app: &mut App, now: Instant) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll_at(now);
        if !matches!(
            app.updates().status(),
            UpdateStatus::Checking
                | UpdateStatus::Downloading { .. }
                | UpdateStatus::Installing { .. }
        ) {
            return;
        }
        assert!(Instant::now() < deadline, "update work never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Start a durable write of `bytes` to `path` that waits for the returned
/// sender, as a save on another thread would.
fn held_write(write: PendingWrite, path: &Path, bytes: &'static [u8]) -> Sender<()> {
    let (release, gate) = channel::<()>();
    let path = path.to_owned();
    std::thread::spawn(move || {
        gate.recv().unwrap();
        let result = atomic_write_file::AtomicWriteFile::open(&path).and_then(|mut file| {
            file.write_all(bytes)?;
            file.commit()
        });
        write.finish(result.map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => AppError::new(
                "folder_missing",
                "Its folder was moved or deleted outside Noura.",
            ),
            _ => AppError::new("write_failed", "The disk refused the write."),
        }));
    });
    release
}

/// Changes still waiting to be saved, as the editor keeps typing until a
/// pause. Saving them writes `bytes` to `path` durably, or fails with
/// `refusal`.
struct FakeSaves {
    path: PathBuf,
    bytes: &'static [u8],
    refusal: Option<AppError>,
    log: Arc<Mutex<Vec<String>>>,
}

impl PendingSaves for FakeSaves {
    fn save_now(&self, relative_path: &str) -> Result<(), AppError> {
        self.log
            .lock()
            .unwrap()
            .push(format!("save {relative_path:?}"));
        if let Some(refusal) = &self.refusal {
            return Err(refusal.clone());
        }
        let mut file = atomic_write_file::AtomicWriteFile::open(&self.path).unwrap();
        file.write_all(self.bytes).unwrap();
        file.commit().unwrap();
        Ok(())
    }
}

/// An app with version 0.3.0 downloaded and ready, and a workspace folder
/// holding `Notes/Plan.md`.
fn ready_to_restart() -> (
    App,
    FakeSource,
    Instant,
    tempfile::TempDir,
    tempfile::TempDir,
) {
    let folder = tempfile::tempdir().unwrap();
    std::fs::create_dir(folder.path().join("Notes")).unwrap();
    std::fs::write(folder.path().join("Notes/Plan.md"), "old").unwrap();
    let source = FakeSource::default();
    *source.release.lock().unwrap() = Some("0.3.0".into());
    *source.watched.lock().unwrap() = Some(folder.path().join("Notes/Plan.md"));
    let (mut app, launched, app_data) = launch_with(&source);
    let first = launched + FIRST_UPDATE_CHECK;
    settle(&mut app, first);
    assert!(matches!(app.updates().status(), UpdateStatus::Ready { .. }));
    (app, source, first, app_data, folder)
}

/// Open the folder at `folder` as the workspace and wait until it shows.
fn open_workspace(app: &mut App, folder: &Path, now: Instant) {
    app.open_folder(folder.to_owned());
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.screen() != &Screen::Workspace {
        assert!(Instant::now() < deadline, "the workspace never opened");
        std::thread::sleep(Duration::from_millis(5));
        app.poll_at(now);
    }
}

/// System services whose first trash check waits for a message. A
/// workspace that is opening asks right after it writes its manifest and
/// before it writes the recent list, so the check holds its setup there.
struct HeldSystem {
    gate: Mutex<Option<Receiver<()>>>,
}

impl System for HeldSystem {
    fn has_trash(&self) -> bool {
        if let Some(gate) = self.gate.lock().unwrap().take() {
            gate.recv().unwrap();
        }
        false
    }

    fn trash(&self, _path: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    fn reveal(&self, _path: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    fn open_folder(&self, _folder: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    fn open_terminal(&self, _folder: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    fn copy_text(&self, _text: &str) {}

    fn open_url(&self, _url: &str) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }

    fn open_file(&self, _path: &Path) -> std::io::Result<()> {
        Err(std::io::ErrorKind::Unsupported.into())
    }
}

fn launch_with(source: &FakeSource) -> (App, Instant, tempfile::TempDir) {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());
    let launched = Instant::now();
    app.enable_updates("0.2.0", Arc::new(source.clone()), launched);
    (app, launched, app_data)
}

#[test]
fn noura_checks_five_seconds_after_launch_and_then_every_six_hours() {
    let source = FakeSource::default();
    let (mut app, launched, _app_data) = launch_with(&source);

    settle(&mut app, launched + Duration::from_millis(4_900));
    assert_eq!(source.checks(), 0, "no check before 5 seconds");

    let first = launched + Duration::from_secs(5);
    settle(&mut app, first);
    assert_eq!(source.checks(), 1);
    assert_eq!(app.updates().status(), UpdateStatus::UpToDate);

    settle(&mut app, first + Duration::from_secs(6 * HOURS - 1));
    assert_eq!(source.checks(), 1, "no second check before 6 hours");

    settle(&mut app, first + Duration::from_secs(6 * HOURS));
    assert_eq!(source.checks(), 2);
    settle(&mut app, first + Duration::from_secs(12 * HOURS));
    assert_eq!(source.checks(), 3);
}

#[test]
fn a_newer_release_downloads_in_the_background_and_installs_nothing_until_restart() {
    let source = FakeSource::default();
    *source.release.lock().unwrap() = Some("0.3.0".into());
    let gate = source.hold_downloads();
    let (mut app, launched, _app_data) = launch_with(&source);
    let first = launched + FIRST_UPDATE_CHECK;

    app.poll_at(first);
    let deadline = Instant::now() + Duration::from_secs(20);
    while !matches!(app.updates().status(), UpdateStatus::Downloading { .. }) {
        assert!(Instant::now() < deadline, "the download never started");
        std::thread::sleep(Duration::from_millis(5));
        app.poll_at(first);
    }
    assert_eq!(
        app.updates().status(),
        UpdateStatus::Downloading {
            version: "0.3.0".into()
        }
    );
    assert!(!app.updates().offers_restart());

    gate.send(()).unwrap();
    settle(&mut app, first);
    assert_eq!(
        app.updates().status(),
        UpdateStatus::Ready {
            version: "0.3.0".into(),
            notes: None,
            problem: None,
        }
    );
    assert!(app.updates().offers_restart());

    // Hours pass. No later check runs while the update waits, and nothing
    // installs.
    settle(&mut app, first + Duration::from_secs(13 * HOURS));
    assert_eq!(source.calls(), ["check", "download"]);
    assert!(matches!(app.updates().status(), UpdateStatus::Ready { .. }));
}

#[test]
fn an_unreachable_feed_fails_and_check_for_updates_tries_again() {
    let source = FakeSource::default();
    *source.check_error.lock().unwrap() = Some(UpdateErrorKind::Unreachable);
    let (mut app, launched, _app_data) = launch_with(&source);
    let first = launched + FIRST_UPDATE_CHECK;

    settle(&mut app, first);
    assert_eq!(
        app.updates().status(),
        UpdateStatus::Failed {
            message: "Couldn't reach the update server. Check your connection.".into()
        }
    );

    *source.check_error.lock().unwrap() = None;
    app.check_for_updates();
    assert_eq!(app.updates().status(), UpdateStatus::Checking);
    settle(&mut app, first);
    assert_eq!(app.updates().status(), UpdateStatus::UpToDate);
    assert_eq!(source.checks(), 2);
}

#[test]
fn a_download_with_a_bad_signature_fails_and_offers_no_restart() {
    let source = FakeSource::default();
    *source.release.lock().unwrap() = Some("0.3.0".into());
    *source.download_error.lock().unwrap() = Some(UpdateErrorKind::BadSignature);
    let (mut app, launched, _app_data) = launch_with(&source);
    let first = launched + FIRST_UPDATE_CHECK;

    settle(&mut app, first);
    let UpdateStatus::Failed { message } = app.updates().status() else {
        panic!("expected a failure, got {:?}", app.updates().status());
    };
    assert!(message.contains("signature"), "{message}");
    assert!(!app.updates().offers_restart());

    app.restart_to_update();
    settle(&mut app, first);
    assert_eq!(source.calls(), ["check", "download"]);
}

#[test]
fn check_for_updates_does_nothing_while_a_check_runs() {
    let source = FakeSource::default();
    *source.release.lock().unwrap() = Some("0.3.0".into());
    let gate = source.hold_downloads();
    let (mut app, launched, _app_data) = launch_with(&source);

    app.check_for_updates();
    app.check_for_updates();
    gate.send(()).unwrap();
    settle(&mut app, launched);
    app.check_for_updates();
    settle(&mut app, launched);

    assert_eq!(source.calls(), ["check", "download"]);
}

#[test]
fn restart_waits_for_a_held_write_to_reach_disk_before_installing() {
    let (mut app, source, now, _app_data, folder) = ready_to_restart();
    let plan = folder.path().join("Notes/Plan.md");
    let write = app.pending_writes().begin("Notes/Plan.md");
    let release = held_write(write, &plan, b"new");

    app.restart_to_update();
    assert_eq!(
        app.updates().status(),
        UpdateStatus::Installing {
            version: "0.3.0".into()
        }
    );
    std::thread::sleep(Duration::from_millis(200));
    app.poll_at(now);
    assert!(matches!(
        app.updates().status(),
        UpdateStatus::Installing { .. }
    ));
    assert_eq!(
        source.calls(),
        ["check", "download"],
        "nothing installs yet"
    );

    release.send(()).unwrap();
    settle(&mut app, now);
    assert_eq!(
        source.calls(),
        ["check", "download", "install saw new", "restart"]
    );
    assert_eq!(
        app.updates().status(),
        UpdateStatus::Restarting {
            version: "0.3.0".into()
        }
    );
}

#[test]
fn a_failed_write_keeps_this_version_running_and_says_why() {
    let (mut app, source, now, _app_data, folder) = ready_to_restart();
    let plan = folder.path().join("Notes/Plan.md");
    let write = app.pending_writes().begin("Notes/Plan.md");
    let release = held_write(write, &plan, b"new");

    app.restart_to_update();
    // Another app removes the note's folder while the save is held.
    std::fs::remove_dir_all(folder.path().join("Notes")).unwrap();
    release.send(()).unwrap();
    settle(&mut app, now);

    assert_eq!(
        app.updates().status(),
        UpdateStatus::Ready {
            version: "0.3.0".into(),
            notes: None,
            problem: Some(
                "Noura didn't restart, because it couldn't save \u{201c}Notes/Plan.md\u{201d}. \
                 Its folder was moved or deleted outside Noura. Save it again, or move it to \
                 the trash, then restart. If you quit Noura instead, you lose that change."
                    .into()
            ),
        }
    );
    assert!(app.updates().offers_restart());
    assert_eq!(source.calls(), ["check", "download"]);

    // Restart keeps refusing until the file saves.
    app.restart_to_update();
    settle(&mut app, now);
    assert_eq!(source.calls(), ["check", "download"]);

    std::fs::create_dir(folder.path().join("Notes")).unwrap();
    let write = app.pending_writes().begin("Notes/Plan.md");
    held_write(write, &plan, b"saved again").send(()).unwrap();
    app.restart_to_update();
    settle(&mut app, now);
    assert_eq!(
        source.calls(),
        ["check", "download", "install saw saved again", "restart"]
    );
}

#[test]
fn restart_saves_every_change_still_waiting_before_installing() {
    let (mut app, source, now, _app_data, folder) = ready_to_restart();
    app.set_pending_saves(Arc::new(FakeSaves {
        path: folder.path().join("Notes/Plan.md"),
        bytes: b"typed",
        refusal: None,
        log: source.log.clone(),
    }));

    app.restart_to_update();
    settle(&mut app, now);

    // The workspace root stands for every file in it. Noura saves again
    // after the install, for changes made while it ran.
    assert_eq!(
        source.calls(),
        [
            "check",
            "download",
            "save \"\"",
            "install saw typed",
            "save \"\"",
            "restart"
        ]
    );
}

#[test]
fn a_failed_save_keeps_this_version_running_and_says_why() {
    let (mut app, source, now, _app_data, folder) = ready_to_restart();
    app.set_pending_saves(Arc::new(FakeSaves {
        path: folder.path().join("Notes/Plan.md"),
        bytes: b"typed",
        refusal: Some(AppError::new(
            "write_failed",
            "The disk is full. Free some space, then try again.",
        )),
        log: source.log.clone(),
    }));

    app.restart_to_update();
    settle(&mut app, now);

    assert_eq!(
        app.updates().status(),
        UpdateStatus::Ready {
            version: "0.3.0".into(),
            notes: None,
            problem: Some(
                "Noura didn't restart, because it couldn't save your changes. The disk is \
                 full. Free some space, then try again."
                    .into()
            ),
        }
    );
    assert_eq!(source.calls(), ["check", "download", "save \"\""]);
    assert_eq!(
        std::fs::read_to_string(folder.path().join("Notes/Plan.md")).unwrap(),
        "old"
    );
}

#[test]
fn a_failed_install_keeps_this_version_running_and_restart_can_try_again() {
    let (mut app, source, now, _app_data, _folder) = ready_to_restart();
    *source.watched.lock().unwrap() = None;
    *source.install_fails.lock().unwrap() = true;

    app.restart_to_update();
    settle(&mut app, now);
    let UpdateStatus::Ready {
        problem: Some(problem),
        ..
    } = app.updates().status()
    else {
        panic!("expected a problem, got {:?}", app.updates().status());
    };
    assert!(
        problem.contains("couldn't install version 0.3.0"),
        "{problem}"
    );
    assert_eq!(source.calls(), ["check", "download", "install"]);

    *source.install_fails.lock().unwrap() = false;
    app.restart_to_update();
    settle(&mut app, now);
    assert_eq!(
        source.calls(),
        ["check", "download", "install", "install", "restart"]
    );
}

#[test]
fn restart_waits_for_a_write_that_starts_while_the_update_installs() {
    let (mut app, source, now, _app_data, folder) = ready_to_restart();
    let plan = folder.path().join("Notes/Plan.md");
    *source.restart_sees.lock().unwrap() = Some(plan.clone());
    let (started, held) = channel::<Sender<()>>();
    let writes = app.pending_writes().clone();
    *source.during_install.lock().unwrap() = Some(Box::new(move || {
        let write = writes.begin("Notes/Plan.md");
        started
            .send(held_write(write, &plan, b"typed while installing"))
            .unwrap();
    }));

    app.restart_to_update();
    let release = held.recv_timeout(Duration::from_secs(20)).unwrap();
    std::thread::sleep(Duration::from_millis(200));
    app.poll_at(now);
    assert!(matches!(
        app.updates().status(),
        UpdateStatus::Installing { .. }
    ));
    assert_eq!(
        source.calls(),
        ["check", "download", "install saw old"],
        "Noura doesn't quit while the write runs"
    );

    release.send(()).unwrap();
    settle(&mut app, now);
    assert_eq!(
        source.calls(),
        [
            "check",
            "download",
            "install saw old",
            "restart saw typed while installing"
        ]
    );
    assert_eq!(
        app.updates().status(),
        UpdateStatus::Restarting {
            version: "0.3.0".into()
        }
    );
}

#[test]
fn a_write_that_fails_while_the_update_installs_keeps_this_version_running_and_says_why() {
    let (mut app, source, now, _app_data, _folder) = ready_to_restart();
    let writes = app.pending_writes().clone();
    *source.during_install.lock().unwrap() = Some(Box::new(move || {
        writes
            .begin("Notes/Plan.md")
            .finish(Err(AppError::new("write_failed", "The disk is full.")));
    }));

    app.restart_to_update();
    settle(&mut app, now);

    assert_eq!(
        app.updates().status(),
        UpdateStatus::Ready {
            version: "0.3.0".into(),
            notes: None,
            problem: Some(
                "Noura didn't restart, because it couldn't save \u{201c}Notes/Plan.md\u{201d}. \
                 The disk is full. Save it again, or move it to the trash, then restart. If you \
                 quit Noura instead, you lose that change. Version 0.3.0 runs the next time \
                 you open Noura."
                    .into()
            ),
        }
    );
    assert!(app.updates().offers_restart());
    assert_eq!(source.calls(), ["check", "download", "install saw old"]);

    // Once the file saves, Restart only restarts: the update is installed.
    app.pending_writes().begin("Notes/Plan.md").finish(Ok(()));
    app.restart_to_update();
    settle(&mut app, now);
    assert_eq!(
        source.calls(),
        ["check", "download", "install saw old", "restart"]
    );
}

#[test]
fn restart_waits_for_a_workspace_that_is_still_being_set_up() {
    let source = FakeSource::default();
    *source.release.lock().unwrap() = Some("0.3.0".into());
    let app_data = tempfile::tempdir().unwrap();
    let (hold, gate) = channel();
    let mut app = App::launch_with(
        app_data.path(),
        Arc::new(HeldSystem {
            gate: Mutex::new(Some(gate)),
        }),
    );
    let launched = Instant::now();
    app.enable_updates("0.2.0", Arc::new(source.clone()), launched);
    let now = launched + FIRST_UPDATE_CHECK;
    settle(&mut app, now);
    let folder = tempfile::tempdir().unwrap();

    app.create_workspace("Plans", folder.path().to_owned());
    app.restart_to_update();
    std::thread::sleep(Duration::from_millis(200));
    app.poll_at(now);
    assert!(matches!(
        app.updates().status(),
        UpdateStatus::Installing { .. }
    ));
    assert_eq!(
        source.calls(),
        ["check", "download"],
        "nothing installs while the workspace is set up"
    );

    hold.send(()).unwrap();
    settle(&mut app, now);
    assert_eq!(source.calls(), ["check", "download", "install", "restart"]);
    assert_eq!(app.recents()[0].name, "Plans");
}

#[test]
fn moving_a_file_whose_write_failed_to_the_trash_lets_restart_go_ahead() {
    let (mut app, source, now, _app_data, folder) = ready_to_restart();
    *source.watched.lock().unwrap() = None;
    open_workspace(&mut app, folder.path(), now);
    drop(app.pending_writes().begin("Notes/Plan.md"));

    app.restart_to_update();
    settle(&mut app, now);
    assert_eq!(source.calls(), ["check", "download"]);

    app.trash("Notes/Plan.md");
    app.restart_to_update();
    settle(&mut app, now);
    assert_eq!(source.calls(), ["check", "download", "install", "restart"]);
}

#[test]
fn a_failed_write_follows_its_file_when_you_rename_its_folder() {
    let (mut app, source, now, _app_data, folder) = ready_to_restart();
    *source.watched.lock().unwrap() = None;
    open_workspace(&mut app, folder.path(), now);
    app.pending_writes()
        .begin("Notes/Plan.md")
        .finish(Err(AppError::new("write_failed", "The disk is full.")));

    app.start_rename("Notes");
    assert!(app.commit_rename("Archive"));
    app.restart_to_update();
    settle(&mut app, now);
    let UpdateStatus::Ready {
        problem: Some(problem),
        ..
    } = app.updates().status()
    else {
        panic!("expected a problem, got {:?}", app.updates().status());
    };
    assert!(
        problem.contains("\u{201c}Archive/Plan.md\u{201d}"),
        "{problem}"
    );

    app.pending_writes().begin("Archive/Plan.md").finish(Ok(()));
    app.restart_to_update();
    settle(&mut app, now);
    assert_eq!(source.calls(), ["check", "download", "install", "restart"]);
}

/// Every window shows the one update service, and Restart in one window
/// waits for a write another window started.
#[test]
fn every_window_shows_the_update_and_restart_waits_for_writes_from_any_window() {
    let (mut app, source, now, _app_data, folder) = ready_to_restart();
    open_workspace(&mut app, folder.path(), now);
    let mut second = app.new_window().expect("a workspace is open");
    assert_eq!(second.updates().status(), app.updates().status());
    assert!(second.updates().offers_restart());
    assert_eq!(second.updates().current_version(), "0.2.0");
    let plan = folder.path().join("Notes/Plan.md");
    let write = second.pending_writes().begin("Notes/Plan.md");
    let release = held_write(write, &plan, b"new");

    app.restart_to_update();
    std::thread::sleep(Duration::from_millis(200));
    second.poll_at(now);
    assert!(matches!(
        second.updates().status(),
        UpdateStatus::Installing { .. }
    ));
    assert_eq!(
        source.calls(),
        ["check", "download"],
        "nothing installs yet"
    );

    release.send(()).unwrap();
    settle(&mut second, now);
    assert_eq!(
        source.calls(),
        ["check", "download", "install saw new", "restart"]
    );
    assert!(matches!(
        app.updates().status(),
        UpdateStatus::Restarting { .. }
    ));
}

#[test]
fn the_windows_check_for_updates_once_between_them() {
    let source = FakeSource::default();
    let (mut app, launched, _app_data) = launch_with(&source);
    let folder = tempfile::tempdir().unwrap();
    open_workspace(&mut app, folder.path(), launched);
    let mut second = app.new_window().expect("a workspace is open");
    let first = launched + FIRST_UPDATE_CHECK;

    second.poll_at(first);
    app.poll_at(first);
    settle(&mut app, first);
    settle(&mut second, first);

    assert_eq!(source.checks(), 1);
    assert_eq!(second.updates().status(), UpdateStatus::UpToDate);
}

#[test]
fn later_in_one_window_hides_the_offer_in_every_window() {
    let (mut app, _source, now, _app_data, folder) = ready_to_restart();
    open_workspace(&mut app, folder.path(), now);
    let second = app.new_window().expect("a workspace is open");
    let (sender, changes) = channel();
    second.subscribe(move |change| {
        let _ = sender.send(change);
    });

    app.hide_update_offer();

    assert!(!second.updates().offers_restart());
    assert!(
        changes.try_iter().any(|change| change == Change::Update),
        "the other window wakes to hide the offer"
    );
}

#[test]
fn noura_waits_for_running_writes_before_it_quits() {
    let app_data = tempfile::tempdir().unwrap();
    let app = App::launch(app_data.path());
    let writes = app.pending_writes().clone();
    let write = writes.begin("Notes/Plan.md");
    let work = writes.begin_work("workspace");

    assert!(!writes.wait_for_running(Duration::from_millis(50)));
    write.finish(Ok(()));
    assert!(
        !writes.wait_for_running(Duration::from_millis(50)),
        "the workspace setup still runs"
    );
    let finished = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(100));
        drop(work);
    });
    assert!(writes.wait_for_running(Duration::from_secs(20)));
    finished.join().unwrap();
}

#[test]
fn a_build_without_updates_says_why_and_never_checks() {
    let app_data = tempfile::tempdir().unwrap();
    let mut app = App::launch(app_data.path());
    let reason = "Installed from the .deb package. Install new versions from noura.app.";

    app.disable_updates("0.2.0", reason);
    app.check_for_updates();
    app.poll_at(Instant::now() + Duration::from_secs(7 * HOURS));

    assert_eq!(
        app.updates().status(),
        UpdateStatus::Unavailable {
            reason: reason.into()
        }
    );
    assert_eq!(app.updates().current_version(), "0.2.0");
    assert_eq!(app.updates().next_check(), None);
}
