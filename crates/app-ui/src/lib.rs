//! The egui screens and the widget kit that draws plugin UI.
//!
//! This crate calls only `app-core`: it draws [`App`]'s view state and sends
//! it what people do.

mod chooser;
mod common;
mod finder;
mod ignored_files;
mod keys;
mod palette;
mod rail;
mod settings;
mod shortcuts;
mod tab_bar;
pub mod theme;
mod tree;
mod updates;
mod workspace;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use app_core::{App, Appearance, Area, CommandSource, DragOut, Menu, Palette, Screen, TabId};
use egui::{Event, ThemePreference};

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
    palette: palette::PaletteView,
    /// Keys typed in the same frame as the shortcut that opened the
    /// palette, for the palette's field on the next frame.
    type_ahead: Vec<Event>,
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
            palette: palette::PaletteView::default(),
            type_ahead: Vec::new(),
            shown_tab: None,
            started: false,
        }
    }

    /// Whether a workspace is opening in the background.
    pub fn is_busy(&self) -> bool {
        self.app.is_busy()
    }

    /// The app this window draws.
    pub fn app(&self) -> &App {
        &self.app
    }

    /// The app, to register commands and pages.
    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    /// The menu bar the native menus show, built from the command
    /// registry.
    pub fn menu_bar(&self) -> Vec<Menu> {
        self.app.menu_bar()
    }

    /// Run the command a native menu item stands for, chosen at `at`.
    pub fn run_menu_command(&mut self, id: &str, at: Instant) -> bool {
        self.app.run_command(id, CommandSource::Menu, at)
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
        if let Some(next) = self.app.updates().next_check() {
            // Wake the window when the next update check is due.
            ui.ctx()
                .request_repaint_after(next.saturating_duration_since(Instant::now()));
        }
        let screen = self.app.screen().clone();
        // Without a workspace, only Quit and Close Window take their keys.
        self.dispatch_keys(ui);
        match screen {
            Screen::Loading => common::loading(ui),
            Screen::Chooser { problem } => chooser::show(self, ui, problem.as_deref()),
            Screen::Workspace => workspace::show(self, ui),
        }
        updates::restart_offer(ui, &mut self.app);
        settings::show(ui, &mut self.app, &mut self.settings);
        palette::show(ui, &mut self.app, &mut self.palette);
        self.close_requested_windows(ui.ctx());
    }

    /// Close what Quit or Close Window asked to close: Quit closes the
    /// main window, which ends the app, and Close Window this one. Each
    /// frame does this; the desktop shell also calls it after menu
    /// choices, which can arrive while the window is hidden and draws no
    /// frame.
    pub fn close_requested_windows(&mut self, ctx: &egui::Context) {
        if self.app.take_quit() {
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        }
        if self.app.take_close_window() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    /// The part of the window that has the keyboard, as the command
    /// registry names areas.
    fn focus_area(&self, ctx: &egui::Context) -> Option<Area> {
        if ctx.text_edit_focused() {
            Some(Area::Writing)
        } else if self.tree.has_focus(ctx) {
            Some(Area::FileTree)
        } else if tab_bar::has_focus(ctx, &self.app) {
            Some(Area::Tabs)
        } else if rail::has_focus(ctx, &self.app) {
            Some(Area::Pages)
        } else {
            None
        }
    }

    /// Run the commands this frame's key presses stand for, before any
    /// view sees them. Keys the focused view or text field handles stay
    /// for it. Keys typed after a shortcut that opens the palette, or
    /// switches it to another mode, wait for the palette's field.
    fn dispatch_keys(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        let typed_ahead = std::mem::take(&mut self.type_ahead);
        let events = ui.input_mut(|input| {
            let mut events = typed_ahead;
            events.append(&mut input.events);
            events
        });
        let platform = self.app.platform();
        let focus = self.focus_area(&ctx);
        let now = Instant::now();
        let mut kept = Vec::with_capacity(events.len());
        let mut palette_opened = false;
        for event in events {
            if palette_opened {
                if matches!(event, Event::Key { .. } | Event::Text(_) | Event::Paste(_)) {
                    self.type_ahead.push(event);
                } else {
                    kept.push(event);
                }
                continue;
            }
            let press = match &event {
                Event::Key {
                    key,
                    physical_key,
                    pressed: true,
                    repeat: false,
                    modifiers,
                } => keys::press(*key, *physical_key, *modifiers, platform),
                _ => None,
            };
            let before = self.app.palette().map(Palette::mode);
            if press.is_some_and(|press| self.app.press_key(&press, focus, now)) {
                let after = self.app.palette().map(Palette::mode);
                palette_opened = after.is_some() && after != before;
            } else {
                kept.push(event);
            }
        }
        if !self.type_ahead.is_empty() {
            ctx.request_repaint();
        }
        ui.input_mut(|input| input.events = kept);
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
