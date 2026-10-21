//! The command registry and the default keymap. The palette, the key
//! handling, the native menus and the shortcuts page all read the
//! registry, so a command registered here shows up in each of them with
//! the same label and keys.

use std::{
    collections::HashMap,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    App, AppError, FILES_PAGE, INBOX_PAGE, Key, KeyPress, MenuName, Page, Platform, Shortcut,
    SystemItem,
};

/// The IDs of the kernel's commands. Features that haven't landed yet
/// register their commands under these IDs and get the default keys.
pub mod command_id {
    pub const NEW_NOTE: &str = "new-note";
    pub const NEW_FOLDER: &str = "new-folder";
    /// Open a file by name: quick open.
    pub const QUICK_OPEN: &str = "quick-open";
    /// Search file contents.
    pub const SEARCH: &str = "search";
    pub const COMMAND_PALETTE: &str = "command-palette";
    pub const RENAME_FILE: &str = "rename-file";
    /// Save the open note now.
    pub const SAVE: &str = "save";
    pub const CLOSE_TAB: &str = "close-tab";
    /// Open another window on the open workspace. It has no default
    /// shortcut.
    pub const NEW_WINDOW: &str = "new-window";
    pub const CLOSE_WINDOW: &str = "close-window";
    pub const NEXT_TAB: &str = "next-tab";
    pub const PREVIOUS_TAB: &str = "previous-tab";
    pub const TOGGLE_SIDEBAR: &str = "toggle-sidebar";
    pub const ZOOM_IN: &str = "zoom-in";
    pub const ZOOM_OUT: &str = "zoom-out";
    pub const ZOOM_RESET: &str = "zoom-reset";
    pub const SETTINGS: &str = "settings";
    /// Settings, on its About section.
    pub const ABOUT: &str = "about";
    /// Settings, on its Keyboard shortcuts section.
    pub const KEYBOARD_SHORTCUTS: &str = "keyboard-shortcuts";
    /// Quit on macOS and Linux, Exit on Windows.
    pub const QUIT: &str = "quit";
    /// The prefix of the commands that show a rail page, followed by the
    /// page's ID.
    pub const GO_TO_PAGE: &str = "go-to-page:";
}

/// Where a key works: anywhere in the window, or only while one part of
/// it has the keyboard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Area {
    Anywhere,
    FileTree,
    Tabs,
    /// The rail's pages.
    Pages,
    /// Text fields.
    Writing,
    /// A note's text in the editor, which formats with its own keys and
    /// types and edits like a text field.
    Note,
}

impl Area {
    /// Every area, in the order the shortcuts page lists them.
    pub const ALL: [Self; 6] = [
        Self::Anywhere,
        Self::FileTree,
        Self::Tabs,
        Self::Pages,
        Self::Writing,
        Self::Note,
    ];

    /// The area's heading on the shortcuts page.
    pub fn title(self) -> &'static str {
        match self {
            Self::Anywhere => "Anywhere",
            Self::FileTree => "File tree",
            Self::Tabs => "Tabs",
            Self::Pages => "Pages",
            Self::Writing => "Writing",
            Self::Note => "Notes",
        }
    }
}

/// Something you can run from its shortcut, the command palette or a
/// menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Command {
    pub id: String,
    pub label: String,
    pub area: Area,
    /// The keys that run it, the one menus show first.
    pub shortcuts: Vec<Shortcut>,
    /// The menu that lists it, for a command the kernel's menus don't
    /// already place.
    pub menu: Option<MenuName>,
}

impl Command {
    /// A command with the keys the default keymap gives `id`, if any.
    pub fn new(id: &str, label: &str, area: Area) -> Self {
        Self {
            id: id.to_owned(),
            label: label.to_owned(),
            area,
            shortcuts: default_shortcuts(id),
            menu: None,
        }
    }

    /// The same command with these keys instead of the default ones.
    pub fn with_shortcuts(self, shortcuts: impl IntoIterator<Item = Shortcut>) -> Self {
        Self {
            shortcuts: shortcuts.into_iter().collect(),
            ..self
        }
    }

    /// The same command, listed in `menu`.
    pub fn in_menu(self, menu: MenuName) -> Self {
        Self {
            menu: Some(menu),
            ..self
        }
    }
}

/// The default keymap: the keys of each kernel command, the first one
/// first. Commands for features that haven't landed yet get theirs when
/// they register.
pub fn default_shortcuts(id: &str) -> Vec<Shortcut> {
    use command_id::*;
    let command = Shortcut::command;
    match id {
        NEW_NOTE => vec![command(Key::N)],
        NEW_FOLDER => vec![command(Key::N).with_shift()],
        QUICK_OPEN => vec![command(Key::O)],
        SEARCH => vec![command(Key::F).with_shift()],
        COMMAND_PALETTE => vec![command(Key::K)],
        RENAME_FILE => vec![Shortcut::new(Key::F2)],
        SAVE => vec![command(Key::S)],
        CLOSE_TAB => vec![command(Key::W)],
        CLOSE_WINDOW => vec![command(Key::W).with_shift()],
        NEXT_TAB => vec![
            command(Key::CloseBracket).with_shift(),
            Shortcut::ctrl(Key::Tab),
        ],
        PREVIOUS_TAB => vec![
            command(Key::OpenBracket).with_shift(),
            Shortcut::ctrl(Key::Tab).with_shift(),
        ],
        TOGGLE_SIDEBAR => vec![command(Key::Backslash)],
        ZOOM_IN => vec![command(Key::Plus), command(Key::Equals)],
        ZOOM_OUT => vec![command(Key::Minus)],
        ZOOM_RESET => vec![command(Key::Num0)],
        SETTINGS => vec![command(Key::Comma)],
        QUIT => vec![command(Key::Q)],
        _ => Vec::new(),
    }
}

/// The kernel commands the default keymap names, in the order lists show
/// them.
const KEYMAP_ORDER: [&str; 17] = [
    command_id::NEW_NOTE,
    command_id::NEW_FOLDER,
    command_id::QUICK_OPEN,
    command_id::SEARCH,
    command_id::COMMAND_PALETTE,
    command_id::RENAME_FILE,
    command_id::SAVE,
    command_id::CLOSE_TAB,
    command_id::CLOSE_WINDOW,
    command_id::NEXT_TAB,
    command_id::PREVIOUS_TAB,
    command_id::TOGGLE_SIDEBAR,
    command_id::ZOOM_IN,
    command_id::ZOOM_OUT,
    command_id::ZOOM_RESET,
    command_id::SETTINGS,
    command_id::QUIT,
];

/// The keys that show rail pages: Mod-1 the Inbox, Mod-2 Files, and Mod-3
/// to Mod-9 the other pages in rail order.
fn page_shortcuts(pages: &[&Page]) -> Vec<(String, Option<Shortcut>)> {
    let mut others = Key::DIGITS[3..].iter();
    pages
        .iter()
        .map(|page| {
            let key = match page.id.as_str() {
                INBOX_PAGE => Some(&Key::Num1),
                FILES_PAGE => Some(&Key::Num2),
                _ => others.next(),
            };
            (page.id.clone(), key.copied().map(Shortcut::command))
        })
        .collect()
}

/// Keys no command takes: the Edit menu's, which act on the focused text
/// field, and Find, which the open note or PDF handles.
fn kept_keys() -> impl Iterator<Item = Shortcut> {
    SystemItem::EDIT
        .into_iter()
        .filter_map(SystemItem::shortcut)
        .chain([Shortcut::command(Key::F)])
}

/// Whether `shortcut` belongs to the default keymap under another ID, to
/// a page, or to the keys no command takes.
fn reserved(shortcut: &Shortcut, id: &str) -> bool {
    let keymap = KEYMAP_ORDER
        .iter()
        .filter(|kernel| **kernel != id)
        .flat_map(|kernel| default_shortcuts(kernel));
    let pages = Key::DIGITS[1..].iter().map(|key| Shortcut::command(*key));
    keymap
        .chain(pages)
        .chain(kept_keys())
        .any(|taken| taken.conflicts_with(shortcut))
}

/// What a command does when it runs.
pub(crate) type Run = Arc<dyn Fn(&mut App) + Send + Sync>;

/// The registered commands, in the order they registered.
#[derive(Default)]
pub(crate) struct Registry {
    commands: Vec<(Command, Run)>,
    recent: RecentRuns,
}

impl Registry {
    /// The same commands for another window, which hasn't run any yet.
    pub(crate) fn copy(&self) -> Self {
        Self {
            commands: self.commands.clone(),
            recent: RecentRuns::default(),
        }
    }

    /// Add a command. Fails when its ID is taken, or one of its keys
    /// belongs to another command or to the default keymap.
    pub(crate) fn register(&mut self, command: Command, run: Run) -> Result<(), AppError> {
        let taken = command.id.starts_with(command_id::GO_TO_PAGE)
            || self
                .commands
                .iter()
                .any(|(other, _)| other.id == command.id);
        if taken {
            return Err(AppError::new(
                "command_exists",
                format!("The ID {} belongs to another command.", command.id),
            ));
        }
        for shortcut in &command.shortcuts {
            let in_use = self.commands.iter().any(|(other, _)| {
                other
                    .shortcuts
                    .iter()
                    .any(|theirs| theirs.conflicts_with(shortcut))
            });
            if in_use || reserved(shortcut, &command.id) {
                return Err(AppError::new(
                    "shortcut_taken",
                    format!(
                        "One of the shortcuts of {} belongs to another command.",
                        command.label
                    ),
                ));
            }
        }
        self.commands.push((command, run));
        Ok(())
    }

    /// Every command: the registered ones, then one per rail page.
    pub(crate) fn list(&self, pages: &[&Page]) -> Vec<Command> {
        let mut commands: Vec<Command> = self
            .commands
            .iter()
            .map(|(command, _)| command.clone())
            .collect();
        for ((page_id, shortcut), page) in page_shortcuts(pages).into_iter().zip(pages) {
            commands.push(Command {
                id: format!("{}{page_id}", command_id::GO_TO_PAGE),
                label: format!("Go to {}", page.title),
                area: Area::Anywhere,
                shortcuts: shortcut.into_iter().collect(),
                menu: None,
            });
        }
        commands
    }

    /// What the command `id` does, when a command has that ID.
    pub(crate) fn run_of(&self, id: &str, pages: &[&Page]) -> Option<Run> {
        if let Some(page_id) = id.strip_prefix(command_id::GO_TO_PAGE) {
            let page_id = pages.iter().find(|page| page.id == page_id)?.id.clone();
            return Some(Arc::new(move |app: &mut App| {
                app.open_page(&page_id, crate::OpenAs::Preview)
            }));
        }
        self.commands
            .iter()
            .find(|(command, _)| command.id == id)
            .map(|(_, run)| run.clone())
    }

    /// The command a key press runs on `platform` while `focus` has the
    /// keyboard: a command for anywhere, or one for that area. Keys the
    /// focused area handles itself stay with it, and so do the keys a
    /// text field types and edits with.
    pub(crate) fn for_key(
        &self,
        press: &KeyPress,
        platform: Platform,
        focus: Option<Area>,
        pages: &[&Page],
    ) -> Option<String> {
        if matches!(focus, Some(Area::Writing | Area::Note)) && press.edits_text(platform) {
            return None;
        }
        let handled_by_focus = area_keys(platform).iter().any(|(area, _, shortcuts)| {
            Some(*area) == focus
                && shortcuts
                    .iter()
                    .any(|shortcut| shortcut.matches(press, platform))
        });
        if handled_by_focus {
            return None;
        }
        let commands: Vec<Command> = self
            .list(pages)
            .into_iter()
            .filter(|command| command.area == Area::Anywhere || Some(command.area) == focus)
            .collect();
        let find = |matches: fn(&Shortcut, &KeyPress, Platform) -> bool| {
            commands.iter().find(|command| {
                command
                    .shortcuts
                    .iter()
                    .any(|shortcut| matches(shortcut, press, platform))
            })
        };
        find(Shortcut::matches)
            .or_else(|| find(Shortcut::matches_by_position))
            .map(|command| command.id.clone())
    }

    /// Whether a run from `source` at `at` is a new one, and not the same
    /// key press reported a second time.
    pub(crate) fn is_new_run(&mut self, id: &str, source: CommandSource, at: Instant) -> bool {
        self.recent.is_new(id, source, at)
    }
}

/// Where a command run came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommandSource {
    /// A key press in the window.
    Key,
    /// A native menu, chosen or through its shortcut.
    Menu,
    /// The command palette.
    Palette,
    /// A button, such as the rail's Search button.
    Button,
}

/// How long after a key press the menu's report of the same press can
/// arrive, or the other way around.
const SAME_PRESS: Duration = Duration::from_millis(300);

/// On macOS the menu bar and the window can both report one shortcut.
/// The second report of a pair runs nothing when it comes from the other
/// source within [`SAME_PRESS`], so pressing a shortcut twice quickly
/// still runs it twice.
#[derive(Default)]
struct RecentRuns(HashMap<String, (Instant, CommandSource)>);

impl RecentRuns {
    fn is_new(&mut self, id: &str, source: CommandSource, at: Instant) -> bool {
        if !matches!(source, CommandSource::Key | CommandSource::Menu) {
            return true;
        }
        if let Some((then, previous)) = self.0.get(id)
            && *previous != source
            && at.saturating_duration_since(*then) < SAME_PRESS
        {
            return false;
        }
        self.0.insert(id.to_owned(), (at, source));
        true
    }
}

/// One line of the shortcuts list: what the keys do. Several keys are
/// alternatives, or the two directions of one move.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutEntry {
    pub label: String,
    pub shortcuts: Vec<Shortcut>,
}

/// The shortcuts that work in one area.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortcutGroup {
    pub area: Area,
    pub entries: Vec<ShortcutEntry>,
}

/// The keys the note editor handles in a note's text: the formatting
/// toolbar's commands, tasks, lists, undo, redo and find. They follow
/// `editor-core`'s keymap, and `app-ui`'s tests check that they match.
fn note_keys(platform: Platform) -> Vec<(Area, &'static str, Vec<Shortcut>)> {
    use Key::*;
    let command = Shortcut::command;
    let mut redo = vec![command(Z).with_shift()];
    if platform != Platform::MacOs {
        redo.push(command(Y));
    }
    [
        ("Bold", vec![command(B)]),
        ("Italic", vec![command(I)]),
        ("Underline", vec![command(U)]),
        ("Strikethrough", vec![command(X).with_shift()]),
        ("Link", vec![command(K)]),
        ("Inline code", vec![command(C).with_shift()]),
        ("Text", vec![command(Num0).with_alt()]),
        ("Heading 1", vec![command(Num1).with_alt()]),
        ("Heading 2", vec![command(Num2).with_alt()]),
        ("Heading 3", vec![command(Num3).with_alt()]),
        ("Quote", vec![command(Q).with_alt()]),
        ("Bulleted list", vec![command(Num8).with_shift()]),
        ("Numbered list", vec![command(Num7).with_shift()]),
        ("Checklist", vec![command(Num9).with_shift()]),
        ("Check or uncheck a task", vec![command(Enter)]),
        ("Nest a list item", vec![Shortcut::new(Tab)]),
        (
            "Lift a list item out",
            vec![Shortcut::new(Tab).with_shift()],
        ),
        ("Undo", vec![command(Z)]),
        ("Redo", redo),
        ("Find in the note", vec![command(F)]),
        // F2 moves from the text to the note's name field.
        ("Rename the note", vec![Shortcut::new(F2)]),
    ]
    .into_iter()
    .map(|(label, shortcuts)| (Area::Note, label, shortcuts))
    .collect()
}

/// Keys the kernel's views handle themselves on `platform` while they
/// have the keyboard, for the shortcuts list.
pub(crate) fn area_keys(platform: Platform) -> Vec<(Area, &'static str, Vec<Shortcut>)> {
    use Key::*;
    let key = Shortcut::new;
    let mut keys = vec![
        (Area::Anywhere, "Close a dialog", vec![key(Escape)]),
        (
            Area::FileTree,
            "Move up or down",
            vec![key(ArrowUp), key(ArrowDown)],
        ),
        (
            Area::FileTree,
            "Go to the first or last item",
            vec![key(Home), key(End)],
        ),
        (
            Area::FileTree,
            "Open or close a folder",
            vec![key(ArrowRight), key(ArrowLeft), key(Enter), key(Space)],
        ),
        (Area::FileTree, "Open a file", vec![key(Enter), key(Space)]),
        (Area::FileTree, "Rename", vec![key(F2)]),
        (
            Area::FileTree,
            "Move to the trash",
            vec![key(Delete), Shortcut::command(Backspace)],
        ),
        (
            Area::FileTree,
            "Show an item's menu",
            vec![key(F10).with_shift(), key(Menu)],
        ),
        (
            Area::FileTree,
            "Jump to a name by typing it",
            vec![key(Letters)],
        ),
        (
            Area::Tabs,
            "Move between tabs",
            vec![key(ArrowLeft), key(ArrowRight)],
        ),
        (
            Area::Tabs,
            "Go to the first or last tab",
            vec![key(Home), key(End)],
        ),
        (
            Area::Tabs,
            "Show the focused tab",
            vec![key(Enter), key(Space)],
        ),
        (
            Area::Tabs,
            "Close the focused tab",
            vec![key(Delete), key(Backspace)],
        ),
        (Area::Pages, "Open a page", vec![key(Enter), key(Space)]),
        (
            Area::Pages,
            "Move a page up or down",
            vec![key(ArrowUp).with_alt(), key(ArrowDown).with_alt()],
        ),
    ];
    keys.extend(note_keys(platform));
    // A text field answers to the Edit menu's keys, and to Ctrl-Y for
    // Redo outside macOS.
    for item in SystemItem::EDIT {
        let mut shortcuts: Vec<Shortcut> = item.shortcut().into_iter().collect();
        if item == SystemItem::Redo && platform != Platform::MacOs {
            shortcuts.push(Shortcut::command(Y));
        }
        keys.push((Area::Writing, item.label(), shortcuts));
    }
    keys
}
