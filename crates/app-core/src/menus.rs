//! The native menu bar as data. The desktop app builds macOS's and
//! Windows' menus from it and sends each choice back as a command.

use crate::{Command, Key, Platform, Shortcut, commands::command_id};

/// The menus a command can name to be listed in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuName {
    File,
    View,
    Window,
    Help,
}

/// One menu in the menu bar.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Menu {
    pub title: String,
    pub items: Vec<MenuItem>,
}

/// An item in a menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuItem {
    /// An item that runs a registered command.
    Command {
        id: String,
        label: String,
        /// The shortcut the menu shows and answers to.
        shortcut: Option<Shortcut>,
    },
    Separator,
    /// An item the operating system or the desktop app carries out itself.
    System(SystemItem),
}

/// Menu items that aren't commands: the operating system's own, and the
/// Edit items that act on the focused text field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SystemItem {
    About,
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    FullScreen,
    Minimize,
    /// Zoom on macOS, Maximize on Windows.
    Zoom,
}

impl SystemItem {
    /// The Edit items, which act on the focused text field.
    pub const EDIT: [Self; 6] = [
        Self::Undo,
        Self::Redo,
        Self::Cut,
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
    ];

    /// The item's label in the menu.
    pub fn label(self) -> &'static str {
        match self {
            Self::About => "About Noura",
            Self::Services => "Services",
            Self::Hide => "Hide Noura",
            Self::HideOthers => "Hide Others",
            Self::ShowAll => "Show All",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Cut => "Cut",
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select All",
            Self::FullScreen => "Enter Full Screen",
            Self::Minimize => "Minimize",
            Self::Zoom => "Zoom",
        }
    }

    /// The shortcut an Edit item shows and answers to. The operating
    /// system binds its own items' keys.
    pub fn shortcut(self) -> Option<Shortcut> {
        let command = Shortcut::command;
        Some(match self {
            Self::Undo => command(Key::Z),
            Self::Redo => command(Key::Z).with_shift(),
            Self::Cut => command(Key::X),
            Self::Copy => command(Key::C),
            Self::Paste => command(Key::V),
            Self::SelectAll => command(Key::A),
            _ => return None,
        })
    }
}

/// A slot in the menu layout.
enum Slot {
    Command(&'static str),
    /// The rail's pages, in rail order.
    Pages,
    /// Registered commands that name this menu and have no slot of their
    /// own.
    Others(MenuName),
    System(SystemItem),
}

/// The menu bar on `platform`: today's menus on macOS, and on Windows the
/// same commands with Settings and Exit under File and About under Help.
/// Linux shows no menu bar, and gets the Windows layout. Items whose
/// command isn't registered, the separators around them and menus left
/// empty drop out.
pub(crate) fn menu_bar(platform: Platform, commands: &[Command]) -> Vec<Menu> {
    use Slot::{Command as Item, Others, Pages, System};
    use SystemItem::*;
    use command_id::*;

    let file = vec![
        vec![Item(NEW_NOTE), Item(NEW_FOLDER)],
        vec![Item(QUICK_OPEN), Item(SEARCH)],
        vec![Item(CLOSE_TAB), Item(CLOSE_WINDOW)],
        vec![Others(MenuName::File)],
    ];
    let edit = vec![
        vec![System(Undo), System(Redo)],
        vec![System(Cut), System(Copy), System(Paste), System(SelectAll)],
    ];
    let view = vec![
        vec![Item(TOGGLE_SIDEBAR)],
        vec![Item(ZOOM_IN), Item(ZOOM_OUT), Item(ZOOM_RESET)],
        vec![Pages],
        vec![Others(MenuName::View)],
    ];
    let window = vec![
        vec![System(Minimize), System(Zoom)],
        vec![Item(NEXT_TAB), Item(PREVIOUS_TAB)],
        vec![Others(MenuName::Window)],
    ];
    let help = vec![Others(MenuName::Help)];
    let layout: Vec<(&str, Vec<Vec<Slot>>)> = match platform {
        Platform::MacOs => {
            let mut view = view;
            view.push(vec![System(FullScreen)]);
            vec![
                (
                    "Noura",
                    vec![
                        vec![System(About)],
                        vec![Item(SETTINGS)],
                        vec![System(Services)],
                        vec![System(Hide), System(HideOthers), System(ShowAll)],
                        vec![Item(QUIT)],
                    ],
                ),
                ("File", file),
                ("Edit", edit),
                ("View", view),
                ("Window", window),
                ("Help", vec![help]),
            ]
        }
        Platform::Windows | Platform::Linux => {
            let mut file = file;
            file.push(vec![Item(SETTINGS)]);
            file.push(vec![Item(QUIT)]);
            vec![
                ("File", file),
                ("Edit", edit),
                ("View", view),
                ("Window", window),
                ("Help", vec![help, vec![System(About)]]),
            ]
        }
    };
    let placed: Vec<&str> = layout
        .iter()
        .flat_map(|(_, sections)| sections.iter().flatten())
        .filter_map(|slot| match slot {
            Slot::Command(id) => Some(*id),
            _ => None,
        })
        .collect();
    let item = |command: &Command| MenuItem::Command {
        id: command.id.clone(),
        label: command.label.clone(),
        shortcut: command.shortcuts.first().copied(),
    };
    layout
        .into_iter()
        .map(|(title, sections)| {
            let mut items = Vec::new();
            for section in sections {
                let filled: Vec<MenuItem> = section
                    .iter()
                    .flat_map(|slot| match slot {
                        Slot::Command(id) => commands
                            .iter()
                            .filter(|command| command.id == *id)
                            .map(item)
                            .collect(),
                        Slot::Pages => commands
                            .iter()
                            .filter(|command| command.id.starts_with(GO_TO_PAGE))
                            .map(item)
                            .collect(),
                        Slot::Others(menu) => commands
                            .iter()
                            .filter(|command| {
                                command.menu == Some(*menu)
                                    && !placed.contains(&command.id.as_str())
                            })
                            .map(item)
                            .collect(),
                        Slot::System(system) => vec![MenuItem::System(*system)],
                    })
                    .collect();
                if filled.is_empty() {
                    continue;
                }
                if !items.is_empty() {
                    items.push(MenuItem::Separator);
                }
                items.extend(filled);
            }
            Menu {
                title: title.to_owned(),
                items,
            }
        })
        .filter(|menu| !menu.items.is_empty())
        .collect()
}
