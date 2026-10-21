//! Shared helpers for the UI tests.

#![allow(dead_code, reason = "each test file uses a different subset")]

pub mod kit;
pub mod plugins;

use std::{
    collections::VecDeque,
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use app_core::{App, Platform, System};
use app_ui::{FolderPicker, NouraUi};
use egui_kittest::Harness;

/// A folder picker that answers with folders queued by the test and records
/// the titles it was asked with.
#[derive(Clone, Default)]
pub struct FakePicker {
    answers: Arc<Mutex<VecDeque<Option<PathBuf>>>>,
    pub titles: Arc<Mutex<Vec<String>>>,
}

impl FakePicker {
    pub fn answer(&self, folder: Option<&Path>) {
        self.answers
            .lock()
            .unwrap()
            .push_back(folder.map(Path::to_owned));
    }
}

impl FolderPicker for FakePicker {
    fn pick_folder(&mut self, title: &str) -> Option<PathBuf> {
        self.titles.lock().unwrap().push(title.to_owned());
        self.answers.lock().unwrap().pop_front().flatten()
    }
}

pub fn harness(app_data: &Path, picker: FakePicker) -> Harness<'static, NouraUi> {
    harness_with(app_data, picker, FakeSystem::new(Platform::current()))
}

pub fn harness_with(
    app_data: &Path,
    picker: FakePicker,
    system: Arc<FakeSystem>,
) -> Harness<'static, NouraUi> {
    harness_on(app_data, picker, system, |_| {})
}

/// A harness whose app runs `register` first, such as to add test pages
/// to the rail.
pub fn harness_registering(
    app_data: &Path,
    picker: FakePicker,
    register: impl FnOnce(&mut App),
) -> Harness<'static, NouraUi> {
    harness_on(
        app_data,
        picker,
        FakeSystem::new(Platform::current()),
        register,
    )
}

/// A harness whose app uses `system`, after `register` runs. On macOS,
/// as for [`harness_for`], shortcut symbols such as ⌘ draw without the
/// system's fonts.
pub fn harness_on(
    app_data: &Path,
    picker: FakePicker,
    system: Arc<FakeSystem>,
    register: impl FnOnce(&mut App),
) -> Harness<'static, NouraUi> {
    let mac = system.platform == Platform::MacOs;
    let mut app = App::launch_with(app_data, system);
    register(&mut app);
    let ui = NouraUi::new(app, Box::new(picker));
    let mut builder = builder();
    if mac {
        builder = builder.allow_missing_glyphs();
    }
    builder.build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui)
}

/// A harness whose app believes it runs on `platform`, after `register`
/// adds test commands or pages. macOS draws shortcuts with symbols such
/// as ⌘, which egui's bundled fonts lack; the desktop app finds them in
/// the system's fonts, which kittest doesn't load.
pub fn harness_for(
    app_data: &Path,
    platform: Platform,
    register: impl FnOnce(&mut App),
) -> Harness<'static, NouraUi> {
    let mut app = App::launch_with(app_data, FakeSystem::new(platform));
    register(&mut app);
    let ui = NouraUi::new(app, Box::new(FakePicker::default()));
    builder()
        .allow_missing_glyphs()
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui)
}

/// A harness of the window's size. The watch thread announces each change
/// to the files, Noura's own included, a moment after it lands, and the
/// window draws once more to show it, so a run may take a few more frames
/// than kittest's default allows.
pub fn builder() -> egui_kittest::HarnessBuilder<NouraUi> {
    Harness::builder()
        .with_size([1000.0, 700.0])
        .with_max_steps(12)
}

/// A system that records what Noura asks of it. Its trash refuses, so
/// trashed items land in the workspace's trash folder, where tests can
/// find them. Its clock says 9 am until a test sets another hour.
pub struct FakeSystem {
    platform: Platform,
    pub calls: Mutex<Vec<String>>,
    hour: AtomicU8,
}

impl FakeSystem {
    pub fn new(platform: Platform) -> Arc<Self> {
        Arc::new(Self {
            platform,
            calls: Mutex::new(Vec::new()),
            hour: AtomicU8::new(9),
        })
    }

    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    /// Set the system's clock to `hour` o'clock.
    pub fn set_hour(&self, hour: u8) {
        self.hour.store(hour, Ordering::SeqCst);
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }
}

impl System for FakeSystem {
    fn platform(&self) -> Platform {
        self.platform
    }

    fn has_trash(&self) -> bool {
        true
    }

    fn trash(&self, path: &Path) -> io::Result<()> {
        self.record(format!("trash {}", path.display()));
        Err(io::Error::other("the fake trash refuses"))
    }

    fn reveal(&self, path: &Path) -> io::Result<()> {
        self.record(format!("reveal {}", path.display()));
        Ok(())
    }

    fn open_folder(&self, folder: &Path) -> io::Result<()> {
        self.record(format!("open {}", folder.display()));
        Ok(())
    }

    fn open_terminal(&self, folder: &Path) -> io::Result<()> {
        self.record(format!("terminal {}", folder.display()));
        Ok(())
    }

    fn copy_text(&self, text: &str) {
        self.record(format!("copy {text}"));
    }

    fn open_url(&self, url: &str) -> io::Result<()> {
        self.record(format!("browse {url}"));
        Ok(())
    }

    fn open_file(&self, path: &Path) -> io::Result<()> {
        self.record(format!("open file {}", path.display()));
        Ok(())
    }

    fn local_hour(&self) -> u8 {
        self.hour.load(Ordering::SeqCst)
    }
}

/// Run frames until the app finishes background work and settles.
pub fn settle(harness: &mut Harness<'static, NouraUi>) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        // The spinner keeps repainting while work runs, so step frames
        // one at a time until it stops.
        harness.step();
        if !harness.state().is_busy() {
            harness.run();
            return;
        }
        assert!(Instant::now() < deadline, "background work never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

pub fn create_workspace(app_data: &Path, name: &str, folder: &Path) -> String {
    std::fs::create_dir_all(folder).unwrap();
    app_core::WorkspaceService::new(app_data)
        .create(name, folder)
        .unwrap()
        .id()
        .to_owned()
}
