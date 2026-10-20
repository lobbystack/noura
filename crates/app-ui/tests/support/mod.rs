//! Shared helpers for the UI tests.

#![allow(dead_code, reason = "each test file uses a different subset")]

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use app_core::App;
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
    harness_with(app_data, picker, |_| {})
}

/// A harness whose app runs `register` first, such as to add test pages
/// to the rail.
pub fn harness_with(
    app_data: &Path,
    picker: FakePicker,
    register: impl FnOnce(&mut App),
) -> Harness<'static, NouraUi> {
    let mut app = App::launch(app_data);
    register(&mut app);
    let ui = NouraUi::new(app, Box::new(picker));
    Harness::builder()
        .with_size([1000.0, 700.0])
        .build_ui_state(|ui, state: &mut NouraUi| state.ui(ui), ui)
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
