//! The egui screens and the widget kit that draws plugin UI.
//!
//! This crate calls only `app-core`: it draws [`App`]'s view state and sends
//! it what people do.

mod ai;
mod chooser;
mod common;
mod dates;
mod finder;
mod ignored_files;
mod image_decoders;
mod image_view;
mod inbox;
mod keys;
mod markdown;
mod note;
mod note_images;
mod note_links;
mod palette;
mod pdf_view;
mod plugin_view;
mod plugins;
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
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use app_core::{App, Appearance, Area, CommandSource, DragOut, Menu, Palette, Screen, TabId};
use egui::{Event, Id, ThemePreference, ViewportId};

/// Finds misspelled words for the notes, such as the system's spell
/// checker.
pub use editor_egui::SpellChecker;

/// How often a window checks on a running note save while it draws no
/// frame.
const SAVE_CHECK: Duration = Duration::from_millis(100);

/// How many windows this process has made, which numbers the next.
static WINDOWS: AtomicU64 = AtomicU64::new(0);

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
    /// Quit ran in this window.
    quitting: bool,
    /// The New workspace form, while it is open.
    new_workspace: Option<common::NewWorkspace>,
    tree: tree::TreeView,
    ignored: ignored_files::IgnoredFilesView,
    settings: settings::SettingsView,
    plugin_install: plugins::InstallView,
    shipped_plugins: plugins::ShippedView,
    /// The prompt that asks before a provider receives the workspace's
    /// content.
    consent: ai::ConsentView,
    tab_bar: tab_bar::TabBarView,
    palette: palette::PaletteView,
    /// The editors of the notes open in tabs.
    notes: note::NoteView,
    /// The PDF tabs' fields and pictures.
    pdfs: pdf_view::PdfViews,
    /// The image tabs' bytes in egui.
    images: image_view::ImageViews,
    /// Keys typed in the same frame as the shortcut that opened the
    /// palette, for the palette's field on the next frame.
    type_ahead: Vec<Event>,
    /// The tab the document area showed last frame.
    shown_tab: Option<TabId>,
    started: bool,
    /// This window's own salt for IDs in egui's memory. Every window draws
    /// with one egui context, so they share that memory.
    window: Id,
}

impl NouraUi {
    pub fn new(app: App, picker: Box<dyn FolderPicker>) -> Self {
        Self {
            app,
            picker,
            quitting: false,
            new_workspace: None,
            tree: tree::TreeView::default(),
            ignored: ignored_files::IgnoredFilesView::default(),
            settings: settings::SettingsView::default(),
            plugin_install: plugins::InstallView::default(),
            shipped_plugins: plugins::ShippedView::default(),
            consent: ai::ConsentView::default(),
            tab_bar: tab_bar::TabBarView::default(),
            palette: palette::PaletteView::default(),
            notes: note::NoteView::default(),
            pdfs: pdf_view::PdfViews::default(),
            images: image_view::ImageViews::default(),
            type_ahead: Vec::new(),
            shown_tab: None,
            started: false,
            window: Id::unique(("noura-window", WINDOWS.fetch_add(1, Ordering::Relaxed))),
        }
    }

    /// Check spelling in notes with `checker`, the system's spell checker,
    /// while the Check spelling setting is on. Without one, Settings
    /// offers no spell check.
    pub fn set_spell_checker(&mut self, checker: Option<Arc<dyn SpellChecker>>) {
        self.notes.spell_checker = checker;
    }

    /// Whether work runs in the background: a workspace opening, or
    /// plugin calls whose answers the window hasn't shown yet.
    pub fn is_busy(&self) -> bool {
        self.app.is_busy() || self.app.is_plugin_work_pending()
    }

    /// The app this window draws.
    pub fn app(&self) -> &App {
        &self.app
    }

    /// The app, to register commands and pages.
    pub fn app_mut(&mut self) -> &mut App {
        &mut self.app
    }

    /// The windows this one opened since the last call: with New Window,
    /// or the workspace's other saved windows when it opened. The desktop
    /// shows each one.
    pub fn take_new_windows(&mut self) -> Vec<App> {
        self.app.take_opened_windows()
    }

    /// Close this window once its notes are saved, as its close button
    /// does. What you typed in a note's name or property field goes in
    /// first. See [`App::close_window`].
    pub fn close_window(&mut self) {
        self.notes.commit_drafts(&mut self.app);
        self.app.close_window();
    }

    /// Whether this window must save notes, or wait for an answer about
    /// one, before it closes. The desktop keeps the window open
    /// meanwhile.
    pub fn must_save_before_closing(&self) -> bool {
        self.app.has_unsaved_notes()
            || self.notes.has_drafts()
            || self.app.unsaved_prompt().is_some()
    }

    /// Start the note saves that are due, whether or not this window draws
    /// frames. eframe draws none for a minimized, hidden or covered window,
    /// so the desktop calls this in every pass for each window with notes
    /// to save, and the root viewport wakes again when the next save is
    /// due or to check on a running one.
    pub fn keep_saving(&mut self, ctx: &egui::Context) {
        if self.app.is_closed() || !self.app.has_unsaved_notes() {
            return;
        }
        self.app.poll();
        let now = Instant::now();
        self.app.save_due_notes(now);
        if let Some(due) = self.app.next_note_save() {
            ctx.request_repaint_after_for(due.saturating_duration_since(now), ViewportId::ROOT);
        } else if self.app.is_saving_notes() {
            ctx.request_repaint_after_for(SAVE_CHECK, ViewportId::ROOT);
        }
    }

    /// Save every note this window shows and wait for the saves, as the
    /// app ends without asking, as when the system quits Noura at logout.
    /// What you typed in a note's name or property field goes in first.
    /// Returns whether every note is on disk.
    pub fn save_before_exit(&mut self) -> bool {
        if self.app.is_closed() {
            return true;
        }
        self.notes.commit_drafts(&mut self.app);
        self.app.save_notes_before_exit()
    }

    /// Whether something waits for notes to save: this window closing, or
    /// a quit or a workspace switch that waits for every window. See
    /// [`Self::keep_up`].
    pub fn is_waiting(&self) -> bool {
        self.app.is_leaving()
    }

    /// Whether this window asks what to do about a note that couldn't
    /// save.
    pub fn is_asking(&self) -> bool {
        self.app.unsaved_prompt().is_some()
    }

    /// Apply background work and carry out Quit without drawing a frame.
    /// While a window waits for notes to save, the desktop calls this for
    /// it even when it draws nothing, as when it is minimized, so it
    /// closes once they are saved. During a quit or a workspace switch
    /// every window waits, and the one that asked goes ahead once all of
    /// them have saved.
    pub fn keep_up(&mut self, ctx: &egui::Context) {
        if self.app.is_closed() {
            return;
        }
        self.app.poll();
        self.close_requested_windows(ctx);
    }

    /// Whether this window has closed, so the desktop stops showing it.
    pub fn is_closed(&self) -> bool {
        self.app.is_closed()
    }

    /// Whether Quit ran in this window. The desktop then lets every
    /// window go at once, and they all come back at the next launch.
    pub fn is_quitting(&self) -> bool {
        self.quitting
    }

    /// The menu bar the native menus show, built from the command
    /// registry.
    pub fn menu_bar(&self) -> Vec<Menu> {
        self.app.menu_bar()
    }

    /// Run the command a native menu item stands for, chosen at `at`.
    /// What you typed in a note's name or property field goes in first.
    pub fn run_menu_command(&mut self, id: &str, at: Instant) -> bool {
        self.notes.commit_drafts(&mut self.app);
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
            // Each window is its own viewport, which repaints on its own.
            let viewport = ctx.viewport_id();
            self.app
                .subscribe(move |_| ctx.request_repaint_of(viewport));
        }
        if self.app.is_closed() {
            return;
        }
        self.app.poll();
        if self.app.is_closed() {
            return;
        }
        apply_preferences(ui.ctx(), &self.app);
        let now = Instant::now();
        self.app.save_due_notes(now);
        if let Some(due) = self.app.next_note_save() {
            ui.ctx()
                .request_repaint_after(due.saturating_duration_since(now));
        }
        self.notes.retain(&self.app);
        self.pdfs.retain(&self.app);
        self.images.retain(ui.ctx(), &self.app);
        if self.app.is_busy() || self.app.is_plugin_work_pending() {
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
        if self.app.is_closed() {
            // Close Window ran: the desktop stops showing this window.
            return;
        }
        // A click on a tab's close button or in the tree can close a note
        // before its fields draw again and report losing the keyboard.
        self.notes.commit_drafts_left(ui.ctx(), &mut self.app);
        match screen {
            Screen::Loading => common::loading(ui),
            Screen::Chooser { problem } => chooser::show(self, ui, problem.as_deref()),
            Screen::Workspace => workspace::show(self, ui),
        }
        updates::restart_offer(ui, &mut self.app);
        settings::show(
            ui,
            &mut self.app,
            self.window,
            &mut self.settings,
            &mut *self.picker,
            self.notes.spell_checker.is_some(),
        );
        plugins::install_dialog(ui, &mut self.app, &mut self.plugin_install);
        plugins::shipped_dialog(ui, &mut self.app, &mut self.shipped_plugins);
        ai::consent_prompt(ui, &mut self.app, &mut self.consent);
        palette::show(ui, &mut self.app, &mut self.palette);
        note::unsaved_prompt(ui, &mut self.app);
        self.notes.commit_hidden_drafts(&mut self.app);
        self.close_requested_windows(ui.ctx());
    }

    /// Carry out Quit: close the main window, which ends the app. Each
    /// frame does this; the desktop shell also calls it after menu
    /// choices, which can arrive while the window is hidden and draws no
    /// frame. Close Window needs nothing here: the window has closed, see
    /// [`Self::is_closed`], and the desktop stops showing it.
    pub fn close_requested_windows(&mut self, ctx: &egui::Context) {
        if self.app.take_quit() {
            self.quitting = true;
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Close);
        }
    }

    /// The part of the window that has the keyboard, as the command
    /// registry names areas.
    fn focus_area(&self, ctx: &egui::Context) -> Option<Area> {
        let active = self.app.active_tab().map(app_core::Tab::id);
        if self.notes.editor_focused(ctx, active) {
            Some(Area::Note)
        } else if ctx.text_edit_focused() {
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
            if let Some(press) = &press
                && self.app.command_for_key(press, focus).is_some()
            {
                // A command can close the note or move the keyboard before
                // its fields report losing it.
                self.notes.commit_drafts(&mut self.app);
            }
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
