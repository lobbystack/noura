//! The egui screens and the widget kit that draws plugin UI.
//!
//! This crate calls only `app-core`: it draws [`App`]'s view state and sends
//! it what people do.

mod chooser;
mod common;
pub mod theme;
mod tree;
mod updates;
mod workspace;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use app_core::{App, Screen};

pub use updates::update_settings;

/// Asks the person for a folder. The desktop app shows the system dialog;
/// tests answer with a folder of their own.
pub trait FolderPicker {
    /// The chosen folder, or `None` when the person cancels.
    fn pick_folder(&mut self, title: &str) -> Option<PathBuf>;
}

/// The window's contents.
pub struct NouraUi {
    app: App,
    picker: Box<dyn FolderPicker>,
    /// The New workspace form, while it is open.
    new_workspace: Option<common::NewWorkspace>,
    tree: tree::TreeView,
    started: bool,
}

impl NouraUi {
    pub fn new(app: App, picker: Box<dyn FolderPicker>) -> Self {
        Self {
            app,
            picker,
            new_workspace: None,
            tree: tree::TreeView::default(),
            started: false,
        }
    }

    /// Whether a workspace is opening in the background.
    pub fn is_busy(&self) -> bool {
        self.app.is_busy()
    }

    /// Draw one frame.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if !self.started {
            self.started = true;
            theme::install(ui.ctx());
            let ctx = ui.ctx().clone();
            self.app.subscribe(move |_| ctx.request_repaint());
        }
        self.app.poll();
        if self.app.is_busy() {
            // The change stream wakes the window when the work finishes;
            // this also keeps the loading spinner turning.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        if let Some(next) = self.app.updates().next_check() {
            // Wake the window when the next update check is due.
            ui.ctx()
                .request_repaint_after(next.saturating_duration_since(Instant::now()));
        }
        match self.app.screen().clone() {
            Screen::Loading => common::loading(ui),
            Screen::Chooser { problem } => chooser::show(self, ui, problem.as_deref()),
            Screen::Workspace => workspace::show(self, ui),
        }
        updates::restart_offer(ui, &mut self.app);
    }

    fn pick_and_open(&mut self) {
        if let Some(folder) = self.picker.pick_folder("Open a folder") {
            self.app.open_folder(folder);
        }
    }
}
