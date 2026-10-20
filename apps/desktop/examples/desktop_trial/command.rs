//! The trial's commands, which the menu bar and the window both run.

use egui::Key;

/// Something the trial can do from a menu item or its shortcut.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    About,
    Quit,
    Undo,
    Redo,
    Cut,
    Copy,
    Paste,
    SelectAll,
    /// The trial's custom item: it counts how often it ran.
    Ping,
    NewWindow,
}

impl Command {
    pub const ALL: [Self; 10] = [
        Self::About,
        Self::Quit,
        Self::Undo,
        Self::Redo,
        Self::Cut,
        Self::Copy,
        Self::Paste,
        Self::SelectAll,
        Self::Ping,
        Self::NewWindow,
    ];

    /// The menu item's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::About => "About Noura Desktop Trial",
            Self::Quit => "Quit Noura Desktop Trial",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Cut => "Cut",
            Self::Copy => "Copy",
            Self::Paste => "Paste",
            Self::SelectAll => "Select All",
            Self::Ping => "Ping",
            Self::NewWindow => "New Window",
        }
    }

    /// The default shortcut. The modifier is Command on macOS and Ctrl
    /// elsewhere.
    pub fn shortcut(self) -> Option<Shortcut> {
        let key = match self {
            Self::About => return None,
            Self::Redo => return Some(Shortcut::with_shift(Key::Z)),
            Self::Ping => return Some(Shortcut::with_shift(Key::P)),
            Self::NewWindow => return Some(Shortcut::with_shift(Key::N)),
            Self::Quit => Key::Q,
            Self::Undo => Key::Z,
            Self::Cut => Key::X,
            Self::Copy => Key::C,
            Self::Paste => Key::V,
            Self::SelectAll => Key::A,
        };
        Some(Shortcut::new(key))
    }
}

/// A shortcut made of the platform's command modifier, optional Shift, and a
/// key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub shift: bool,
    pub key: Key,
}

impl Shortcut {
    const fn new(key: Key) -> Self {
        Self { shift: false, key }
    }

    const fn with_shift(key: Key) -> Self {
        Self { shift: true, key }
    }

    /// The same shortcut as egui matches it.
    pub fn to_egui(self) -> egui::KeyboardShortcut {
        let modifiers = if self.shift {
            egui::Modifiers::COMMAND | egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::COMMAND
        };
        egui::KeyboardShortcut::new(modifiers, self.key)
    }
}
