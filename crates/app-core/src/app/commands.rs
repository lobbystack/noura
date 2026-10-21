//! Commands: the kernel's own, registering more, and running them from a
//! key press, a menu or the palette.

use std::{sync::Arc, time::Instant};

use super::{App, Change};
use crate::{
    Area, Command, CommandSource, KeyPress, Menu, ShortcutEntry, ShortcutGroup, Tab,
    commands::{Run, area_keys, command_id},
    layout::FILES_SECTION,
    menus,
};

impl App {
    /// Register the kernel's commands, with the default keymap's keys.
    pub(super) fn register_kernel_commands(&mut self) {
        use command_id::*;
        let kernel: [(&str, &str, Run); 10] = [
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
            (
                COMMAND_PALETTE,
                "Command Palette",
                Arc::new(App::toggle_palette),
            ),
            (RENAME_FILE, "Rename File", Arc::new(App::rename_open_file)),
            (CLOSE_TAB, "Close Tab", Arc::new(App::close_active_tab)),
            (
                CLOSE_WINDOW,
                "Close Window",
                Arc::new(|app| {
                    app.close_window = true;
                    app.listeners.emit(Change::Window);
                }),
            ),
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
            (
                KEYBOARD_SHORTCUTS,
                "Keyboard Shortcuts",
                Arc::new(App::open_shortcuts),
            ),
        ];
        for (id, label, run) in kernel {
            let command = Command::new(id, label, Area::Anywhere);
            if let Err(error) = self.registry.register(command, run) {
                tracing::error!(id, %error.code, "a kernel command didn't register");
            }
        }
    }

    /// Every command: the registered ones in the order they registered,
    /// then one Go to command per rail page.
    pub fn commands(&self) -> Vec<Command> {
        self.registry.list(&self.rail.ordered())
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
        let Some(run) = self.registry.run_of(id, &self.rail.ordered()) else {
            return false;
        };
        if !self.registry.is_new_run(id, source, at) {
            return false;
        }
        run(self);
        true
    }

    /// The command a key press runs while `focus` has the keyboard: a
    /// command for anywhere, or one for that area. The keys the focused
    /// area handles itself, as the shortcuts list shows them, stay with
    /// it. While a text field has the keyboard ([`Area::Writing`]), so do
    /// the keys it types and edits with.
    pub fn command_for_key(&self, press: &KeyPress, focus: Option<Area>) -> Option<String> {
        self.registry
            .for_key(press, self.platform(), focus, &self.rail.ordered())
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
        let areas = area_keys();
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

    /// Whether the Keyboard shortcuts page is open.
    pub fn shortcuts_open(&self) -> bool {
        self.shortcuts_open
    }

    /// Open the Keyboard shortcuts page.
    pub fn open_shortcuts(&mut self) {
        if !self.shortcuts_open {
            self.shortcuts_open = true;
            self.listeners.emit(Change::Settings);
        }
    }

    pub fn close_shortcuts(&mut self) {
        if self.shortcuts_open {
            self.shortcuts_open = false;
            self.listeners.emit(Change::Settings);
        }
    }

    /// Whether Close Window ran since the last call. The window that ran
    /// it closes.
    pub fn take_close_window(&mut self) -> bool {
        std::mem::take(&mut self.close_window)
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
