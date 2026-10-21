//! The egui screens and the widget kit that draws plugin UI.
//!
//! This crate calls only `app-core`: it draws [`App`]'s view state and sends
//! it what people do.

mod chooser;
mod common;
mod ignored_files;
mod rail;
mod tab_bar;
pub mod theme;
mod tree;
mod workspace;

use std::{path::PathBuf, time::Duration};

use app_core::{App, Screen, TabId};

/// Asks the person for a folder. The desktop app shows the system dialog;
/// tests answer with a folder of their own.
pub trait FolderPicker: Send {
    /// The chosen folder, or `None` when the person cancels.
    fn pick_folder(&mut self, title: &str) -> Option<PathBuf>;
}

/// A window's contents.
pub struct NouraUi {
    app: App,
    picker: Box<dyn FolderPicker>,
    /// Windows this one opened, until the desktop shows them.
    new_windows: Vec<App>,
    /// The New workspace form, while it is open.
    new_workspace: Option<common::NewWorkspace>,
    tree: tree::TreeView,
    ignored: ignored_files::IgnoredFilesView,
    tab_bar: tab_bar::TabBarView,
    /// The tab the document area showed last frame.
    shown_tab: Option<TabId>,
    started: bool,
}

impl NouraUi {
    pub fn new(app: App, picker: Box<dyn FolderPicker>) -> Self {
        Self {
            app,
            picker,
            new_windows: Vec::new(),
            new_workspace: None,
            tree: tree::TreeView::default(),
            ignored: ignored_files::IgnoredFilesView::default(),
            tab_bar: tab_bar::TabBarView::default(),
            shown_tab: None,
            started: false,
        }
    }

    /// Whether a workspace is opening in the background.
    pub fn is_busy(&self) -> bool {
        self.app.is_busy()
    }

    /// The windows this one opened since the last call: with New window,
    /// or the workspace's other saved windows when it opened. The desktop
    /// shows each one.
    pub fn take_new_windows(&mut self) -> Vec<App> {
        let mut windows = std::mem::take(&mut self.new_windows);
        windows.extend(self.app.take_opened_windows());
        windows
    }

    /// Close this window, as its close button does. See
    /// [`App::close_window`].
    pub fn close_window(&mut self) {
        self.app.close_window();
    }

    /// Whether this window has closed, so the desktop stops showing it.
    pub fn is_closed(&self) -> bool {
        self.app.is_closed()
    }

    /// Draw one frame.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        if !self.started {
            self.started = true;
            theme::install(ui.ctx());
            let ctx = ui.ctx().clone();
            // Each window is its own viewport, which repaints on its own.
            let viewport = ctx.viewport_id();
            self.app
                .subscribe(move |_| ctx.request_repaint_of(viewport));
        }
        // Cmd-Shift-W on macOS and Ctrl-Shift-W elsewhere closes the window.
        if ui.input_mut(|input| {
            input.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::W,
            )
        }) {
            self.app.close_window();
        }
        if self.app.is_closed() {
            return;
        }
        self.app.poll();
        if self.app.is_closed() {
            return;
        }
        if self.app.is_busy() {
            // The change stream wakes the window when the work finishes;
            // this also keeps the loading spinner turning.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        match self.app.screen().clone() {
            Screen::Loading => common::loading(ui),
            Screen::Chooser { problem } => chooser::show(self, ui, problem.as_deref()),
            Screen::Workspace => workspace::show(self, ui),
        }
    }

    /// Open another window on the open workspace.
    fn new_window(&mut self) {
        if let Some(window) = self.app.new_window() {
            self.new_windows.push(window);
        }
    }

    fn pick_and_open(&mut self) {
        if let Some(folder) = self.picker.pick_folder("Open a folder") {
            self.app.open_folder(folder);
        }
    }
}
