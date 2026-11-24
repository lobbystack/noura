//! Commands: the kernel's own, registering more, and running them from a
//! key press, a menu or the palette.

use std::{sync::Arc, time::Instant};

use super::{App, Change, SearchScope};
use crate::{
    Area, Command, CommandSource, KeyPress, Menu, MenuName, Overlay, Platform, ShortcutEntry,
    ShortcutGroup, Tab,
    commands::{Covered, Run, area_keys, command_id, default_shortcuts},
    layout::FILES_SECTION,
    menus,
    plugins::COMMAND_PREFIX,
    settings::settings_section,
};

/// The commands whose keys work while no workspace is open, as on the
/// chooser: the ones that don't need a workspace.
const WITHOUT_WORKSPACE: [&str; 6] = [
    command_id::QUIT,
    command_id::CLOSE_WINDOW,
    command_id::ZOOM_IN,
    command_id::ZOOM_OUT,
    command_id::ZOOM_RESET,
    command_id::SETTINGS,
];

impl App {
    /// Register the kernel's commands, with the default keymap's keys.
    pub(super) fn register_kernel_commands(&mut self) {
        use command_id::*;
        let quit = match self.platform() {
            Platform::MacOs => "Quit Noura",
            Platform::Windows => "Exit",
            Platform::Linux => "Quit",
        };
        let kernel: [(&str, &str, Run); 22] = [
            (NEW_NOTE, "New Note", Arc::new(|app| app.new_note(None))),
            (
                NEW_FOLDER,
                "New Folder",
                Arc::new(|app| {
                    // The new folder opens for naming in the tree.
                    app.show_files_section();
                    app.new_folder(None);
                }),
            ),
            (COMMAND_PALETTE, "Search…", Arc::new(App::toggle_palette)),
            (
                QUICK_OPEN,
                "Search File Names…",
                Arc::new(|app| app.open_palette(SearchScope::Names)),
            ),
            (
                SEARCH,
                "Search File Contents…",
                Arc::new(|app| app.open_palette(SearchScope::Contents)),
            ),
            (RENAME_FILE, "Rename File", Arc::new(App::rename_open_file)),
            (SHARE_LINK, "Share Link…", Arc::new(App::share_open_note)),
            (SAVE, "Save", Arc::new(App::save_active_note)),
            (CLOSE_TAB, "Close Tab", Arc::new(App::close_active_tab)),
            (
                REOPEN_CLOSED_TAB,
                "Reopen Closed Tab",
                Arc::new(App::reopen_closed_tab),
            ),
            (NEW_WINDOW, "New Window", Arc::new(App::open_new_window)),
            (CLOSE_WINDOW, "Close Window", Arc::new(App::close_window)),
            (NEXT_TAB, "Next Tab", Arc::new(|app| app.cycle_tab(1))),
            (
                PREVIOUS_TAB,
                "Previous Tab",
                Arc::new(|app| app.cycle_tab(-1)),
            ),
            (
                TOGGLE_SIDEBAR,
                "Toggle Sidebar",
                Arc::new(App::toggle_sidebar),
            ),
            (ZOOM_IN, "Zoom In", Arc::new(|app| app.zoom_view(Zoom::In))),
            (
                ZOOM_OUT,
                "Zoom Out",
                Arc::new(|app| app.zoom_view(Zoom::Out)),
            ),
            (
                ZOOM_RESET,
                "Actual Size",
                Arc::new(|app| app.zoom_view(Zoom::ActualSize)),
            ),
            (SETTINGS, "Settings…", Arc::new(App::open_settings)),
            (
                ABOUT,
                "About Noura",
                Arc::new(|app| app.open_settings_at(settings_section::ABOUT)),
            ),
            (
                KEYBOARD_SHORTCUTS,
                "Keyboard Shortcuts",
                Arc::new(|app| app.open_settings_at(settings_section::SHORTCUTS)),
            ),
            (QUIT, quit, Arc::new(App::request_quit)),
        ];
        let platform = self.platform();
        for (id, label, run) in kernel {
            let mut command = Command::new(id, label, Area::Anywhere)
                .with_shortcuts(default_shortcuts(id, platform));
            if id == NEW_WINDOW {
                command = command.in_menu(MenuName::File);
            }
            if let Err(error) = self.registry.register(command, run) {
                tracing::error!(id, %error.code, "a kernel command didn't register");
            }
        }
    }

    /// Every command: the registered ones in the order they registered,
    /// then one Go to command per rail page, then the commands running
    /// plugins add, which have no shortcuts.
    pub fn commands(&self) -> Vec<Command> {
        let mut commands = self.registry.list(&self.rail.ordered(), self.platform());
        commands.extend(
            self.session
                .plugins()
                .commands()
                .into_iter()
                .map(|(id, title)| Command::new(&id, &title, Area::Anywhere)),
        );
        commands
    }

    /// Add a command that does `run`. Fails when another command has its
    /// ID, or uses one of its keys, or the default keymap keeps one of
    /// its keys for another command.
    pub fn register_command(
        &mut self,
        command: Command,
        run: impl Fn(&mut App) + Send + Sync + 'static,
    ) -> Result<(), crate::AppError> {
        self.registry.register(command, Arc::new(run))?;
        self.listeners.emit(Change::Commands);
        Ok(())
    }

    /// Run the command `id`, chosen at `at` from `source`. Returns whether
    /// it ran: nothing runs for an unknown ID, or for the second report of
    /// a key press that the menu and the window both report.
    pub fn run_command(&mut self, id: &str, source: CommandSource, at: Instant) -> bool {
        // A key or a menu item reaches what covers the window first. The
        // palette and buttons can't be used while something covers them.
        let covered = matches!(source, CommandSource::Key | CommandSource::Menu)
            .then(|| self.overlay())
            .flatten();
        if id.starts_with(COMMAND_PREFIX) {
            if covered.is_some() {
                return false;
            }
            // The plugin runs it on its own thread and draws its views
            // again; the window shows them once they arrive.
            let ran = self.session.plugins().run_command(id, self.key);
            if ran {
                self.session.wake_all();
            }
            return ran;
        }
        let Some(run) = self.registry.run_of(id, &self.rail.ordered()) else {
            return false;
        };
        if !self.registry.is_new_run(id, source, at) {
            return false;
        }
        match covered.map(|overlay| overlay.takes(id)) {
            None | Some(Covered::Runs) => {
                run(self);
                true
            }
            Some(Covered::Closes) => {
                self.close_overlay();
                true
            }
            Some(Covered::Waits) => false,
        }
    }

    /// What covers the window now, the one on top: a dialog that asks
    /// something, search or Settings. Keys and menu items that act on the
    /// window's tabs and files don't reach past it.
    pub fn overlay(&self) -> Option<Overlay> {
        let asking = self.unsaved_prompt().is_some()
            || self.share_screen().open
            || self.ai_tool_prompt().is_some()
            || self.plugin_install.is_some();
        if asking {
            return Some(Overlay::Dialog);
        }
        if self.palette.is_some() {
            return Some(Overlay::Palette);
        }
        if self.settings.is_some() {
            return Some(Overlay::Settings);
        }
        let reviewing = self
            .tabs
            .active()
            .and_then(|tab| self.note(tab.id()))
            .and_then(|note| note.conflict())
            .is_some_and(|conflict| conflict.is_reviewing());
        (self.pending_trash.is_some() || reviewing).then_some(Overlay::Dialog)
    }

    /// Close search or Settings, as Close Tab does while it covers the
    /// window.
    fn close_overlay(&mut self) {
        match self.overlay() {
            Some(Overlay::Palette) => self.close_palette(),
            Some(Overlay::Settings) => self.close_settings(),
            Some(Overlay::Dialog) | None => {}
        }
    }

    /// The command a key press runs while `focus` has the keyboard: a
    /// command for anywhere, or one for that area. The keys the focused
    /// area handles itself, as the shortcuts list shows them, stay with
    /// it. While a text field or a note's text has the keyboard
    /// ([`Area::Writing`], [`Area::Note`]), so do the keys it types and
    /// edits with. While no workspace is open, only
    /// Quit and Close Window take their keys.
    pub fn command_for_key(&self, press: &KeyPress, focus: Option<Area>) -> Option<String> {
        let id = self
            .registry
            .for_key(press, self.platform(), focus, &self.rail.ordered())?;
        (self.workspace.is_some() || WITHOUT_WORKSPACE.contains(&id.as_str())).then_some(id)
    }

    /// Run the command a key press at `at` stands for. Returns whether a
    /// command took the key, even when it was the second report of one
    /// press and didn't run again.
    pub fn press_key(&mut self, press: &KeyPress, focus: Option<Area>, at: Instant) -> bool {
        match self.command_for_key(press, focus) {
            Some(id) => {
                self.run_command(&id, CommandSource::Key, at);
                true
            }
            None => false,
        }
    }

    /// The menu bar on this platform, built from the registered commands.
    pub fn menu_bar(&self) -> Vec<Menu> {
        menus::menu_bar(self.platform(), &self.commands())
    }

    /// Every shortcut by area: the commands' keys, then the keys each
    /// part of the window handles while it has the keyboard.
    pub fn shortcut_groups(&self) -> Vec<ShortcutGroup> {
        let commands = self.commands();
        let areas = area_keys(self.platform());
        Area::ALL
            .into_iter()
            .map(|area| {
                let from_commands = commands
                    .iter()
                    .filter(|command| command.area == area && !command.shortcuts.is_empty())
                    .map(|command| ShortcutEntry {
                        label: command.label.clone(),
                        shortcuts: command.shortcuts.clone(),
                    });
                let local = areas.iter().filter(|(local, _, _)| *local == area).map(
                    |(_, label, shortcuts)| ShortcutEntry {
                        label: (*label).to_owned(),
                        shortcuts: shortcuts.clone(),
                    },
                );
                ShortcutGroup {
                    area,
                    entries: from_commands.chain(local).collect(),
                }
            })
            .filter(|group| !group.entries.is_empty())
            .collect()
    }

    /// Quit once every window's notes are saved, as Quit does. Each
    /// window saves its own, and asks when one fails; Keep editing in any
    /// of them stays.
    pub fn request_quit(&mut self) {
        self.depart(super::notes::Departure::Quit);
    }

    /// Whether Quit ran since the last call, in this window, and every
    /// window's notes are on disk. The app closes every window, and they
    /// all come back at the next launch.
    pub fn take_quit(&mut self) -> bool {
        std::mem::take(&mut self.quit)
    }

    /// Show the tab after the active one, or before it when `offset` is
    /// negative, going round from the last tab to the first.
    pub fn cycle_tab(&mut self, offset: isize) {
        let tabs = self.tabs.list();
        if tabs.is_empty() {
            return;
        }
        let active = self.tabs.active().map(Tab::id);
        let index = tabs.iter().position(|tab| Some(tab.id()) == active);
        let next = match index {
            Some(index) => (index as isize + offset).rem_euclid(tabs.len() as isize) as usize,
            None => 0,
        };
        let id = tabs[next].id();
        self.activate_tab(id);
    }

    /// Show the tab at place `number`, counting from 1, or the last tab
    /// with `None`, as Mod-1 to Mod-9 do. Nothing happens when fewer tabs
    /// are open.
    pub fn show_tab_number(&mut self, number: Option<usize>) {
        let tabs = self.tabs.list();
        let tab = match number {
            Some(number) => number.checked_sub(1).and_then(|index| tabs.get(index)),
            None => tabs.last(),
        };
        if let Some(id) = tab.map(Tab::id) {
            self.activate_tab(id);
        }
    }

    /// New Window: open another window on the open workspace, which
    /// [`App::take_opened_windows`] hands the client.
    fn open_new_window(&mut self) {
        if let Some(window) = self.new_window() {
            self.opened_windows.push(window);
            self.listeners.emit(Change::Window);
        }
    }

    fn close_active_tab(&mut self) {
        if let Some(id) = self.tabs.active().map(Tab::id) {
            self.close_tab(id);
        }
    }

    /// Start renaming the open file in the file tree, showing the tree
    /// first.
    fn rename_open_file(&mut self) {
        let Some(path) = self.document().map(|file| file.relative_path.clone()) else {
            return;
        };
        self.show_files_section();
        self.start_rename(&path);
    }

    /// Show the sidebar and unfold its Files section.
    fn show_files_section(&mut self) {
        if !self.layout.sidebar_shown {
            self.toggle_sidebar();
        }
        if self.is_section_collapsed(FILES_SECTION) {
            self.toggle_section(FILES_SECTION);
        }
    }
}

/// What Zoom In, Zoom Out and Actual Size do.
#[derive(Debug, Clone, Copy)]
enum Zoom {
    In,
    Out,
    ActualSize,
}

impl App {
    /// Zoom In, Zoom Out and Actual Size zoom the PDF or the image while
    /// its tab is active, as Preview does, and the interface anywhere
    /// else.
    fn zoom_view(&mut self, zoom: Zoom) {
        let tab = self.active_tab().map(Tab::id);
        let pdf = tab.filter(|tab| self.pdf(*tab).is_some());
        let image = tab.filter(|tab| self.image(*tab).is_some());
        match (pdf, image, zoom) {
            (Some(tab), _, Zoom::In) => self.pdf_zoom_in(tab),
            (Some(tab), _, Zoom::Out) => self.pdf_zoom_out(tab),
            (Some(tab), _, Zoom::ActualSize) => self.pdf_actual_size(tab),
            (None, Some(tab), Zoom::In) => self.image_zoom_in(tab),
            (None, Some(tab), Zoom::Out) => self.image_zoom_out(tab),
            (None, Some(tab), Zoom::ActualSize) => self.image_actual_size(tab),
            (None, None, Zoom::In) => self.zoom_in(),
            (None, None, Zoom::Out) => self.zoom_out(),
            (None, None, Zoom::ActualSize) => self.actual_size(),
        }
    }
}
