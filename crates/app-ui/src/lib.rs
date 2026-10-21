//! The egui screens and the widget kit that draws plugin UI.
//!
//! This crate calls only `app-core`: it draws [`App`]'s view state and sends
//! it what people do.

mod chooser;
mod common;
mod ignored_files;
mod rail;
mod settings;
mod tab_bar;
pub mod theme;
mod tree;
mod workspace;

use std::{path::PathBuf, time::Duration};

use app_core::{App, Appearance, DragOut, Screen, TabId};
use egui::ThemePreference;

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
    ignored: ignored_files::IgnoredFilesView,
    settings: settings::SettingsView,
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
            new_workspace: None,
            tree: tree::TreeView::default(),
            ignored: ignored_files::IgnoredFilesView::default(),
            settings: settings::SettingsView::default(),
            tab_bar: tab_bar::TabBarView::default(),
            shown_tab: None,
            started: false,
        }
    }

    /// The app whose view state this window draws.
    pub fn app(&self) -> &App {
        &self.app
    }

    /// Whether a workspace is opening in the background.
    pub fn is_busy(&self) -> bool {
        self.app.is_busy()
    }

    /// The native drag of a tree row that left the window, for the desktop
    /// shell to start now.
    pub fn take_drag_out(&mut self) -> Option<DragOut> {
        self.app.take_drag_out()
    }

    /// The desktop shell couldn't start the native drag from
    /// [`Self::take_drag_out`].
    pub fn drag_out_failed(&mut self) {
        self.app.drag_out_failed();
    }

    /// The native drag from [`Self::take_drag_out`] ended, wherever it
    /// landed.
    pub fn drag_out_ended(&mut self) {
        self.app.drag_out_ended();
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
        apply_preferences(ui.ctx(), &self.app);
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
        if settings::show(ui, &mut self.app, &mut self.settings) {
            // Back where Settings opened from.
            ui.ctx()
                .memory_mut(|memory| memory.request_focus(rail::settings_button_id()));
        }
    }

    fn pick_and_open(&mut self) {
        if let Some(folder) = self.picker.pick_folder("Open a folder") {
            self.app.open_folder(folder);
        }
    }
}

/// Draw with this device's appearance and zoom. In Match system, egui
/// follows the system's appearance as it changes.
fn apply_preferences(ctx: &egui::Context, app: &App) {
    let preferences = app.preferences();
    let theme = match preferences.appearance {
        Appearance::Light => ThemePreference::Light,
        Appearance::Dark => ThemePreference::Dark,
        Appearance::MatchSystem => ThemePreference::System,
    };
    if ctx.options(|options| options.theme_preference) != theme {
        ctx.set_theme(theme);
    }
    if ctx.zoom_factor() != preferences.zoom_factor() {
        ctx.set_zoom_factor(preferences.zoom_factor());
    }
}
