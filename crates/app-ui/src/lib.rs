//! The egui screens and the widget kit that draws plugin UI.
//!
//! This crate calls only `app-core`: it draws [`App`]'s view state and sends
//! it what people do.

mod chooser;
mod common;
mod ignored_files;
mod keys;
mod note;
mod palette;
mod rail;
mod shortcuts;
mod tab_bar;
pub mod theme;
mod tree;
mod workspace;

use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

use app_core::{App, Area, CommandSource, DragOut, Menu, Screen, TabId};
use egui::Event;

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
    tab_bar: tab_bar::TabBarView,
    palette: palette::PaletteView,
    /// The editors of the notes open in tabs.
    notes: note::NoteView,
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
            tab_bar: tab_bar::TabBarView::default(),
            palette: palette::PaletteView::default(),
            notes: note::NoteView::default(),
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
            // The desktop app registers a font system with the system's
            // fonts first; without one, the editor uses egui's own.
            editor_egui::register_font_system(
                ui.ctx(),
                editor_egui::fonts::default_font_database(),
            );
            let ctx = ui.ctx().clone();
            self.app.subscribe(move |_| ctx.request_repaint());
        }
        self.app.poll();
        let now = Instant::now();
        self.app.save_due_notes(now);
        if let Some(due) = self.app.next_note_save() {
            ui.ctx()
                .request_repaint_after(due.saturating_duration_since(now));
        }
        self.guard_window_close(ui.ctx());
        self.notes.retain(&self.app);
        if self.app.is_busy() {
            // The change stream wakes the window when the work finishes;
            // this also keeps the loading spinner turning.
            ui.ctx().request_repaint_after(Duration::from_millis(100));
        }
        let screen = self.app.screen().clone();
        // Without a workspace, only Quit and Close Window take their keys.
        self.dispatch_keys(ui);
        match screen {
            Screen::Loading => common::loading(ui),
            Screen::Chooser { problem } => chooser::show(self, ui, problem.as_deref()),
            Screen::Workspace => workspace::show(self, ui),
        }
        palette::show(ui, &mut self.app, &mut self.palette);
        note::unsaved_prompt(ui, &mut self.app);
        self.close_requested_windows(ui.ctx());
    }

    /// The window's close button closes the app once every note is saved:
    /// the window stays while saves run, and asks when one fails.
    fn guard_window_close(&mut self, ctx: &egui::Context) {
        let requested = ctx.input(|input| input.viewport().close_requested());
        if requested && (self.app.has_unsaved_notes() || self.app.unsaved_prompt().is_some()) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.app.request_quit();
        }
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
        let active = self.app.active_tab().map(app_core::Tab::id);
        if ctx.text_edit_focused() || self.notes.editor_focused(ctx, active) {
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
    /// for it. Keys typed after the shortcut that opens the palette wait
    /// for the palette's field.
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
            let was_open = self.app.palette().is_some();
            if press.is_some_and(|press| self.app.press_key(&press, focus, now)) {
                palette_opened = !was_open && self.app.palette().is_some();
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
