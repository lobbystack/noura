//! The editor's shortcuts, independent of any UI toolkit. `editor-egui`
//! turns key presses into [`Shortcut`]s and runs the [`Command`] each one
//! names, and the toolbar labels its buttons from the same table.
//!
//! The formatting keys follow Obsidian where Obsidian has a default (Mod-B,
//! Mod-I, Mod-K, Mod-Enter), the legacy app for underline and
//! strikethrough, and common editors for headings and lists.

use crate::commands::Format;
use crate::input::Event;

/// A key the editor's shortcuts use.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Key {
    B,
    C,
    F,
    I,
    K,
    Q,
    U,
    X,
    Y,
    Z,
    Enter,
    Tab,
    Num0,
    Num1,
    Num2,
    Num3,
    Num7,
    Num8,
    Num9,
}

/// A key with the modifiers held for it. `command` is ⌘ on macOS and Ctrl
/// elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub key: Key,
    pub command: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Shortcut {
    /// The key alone.
    pub const fn new(key: Key) -> Self {
        Self {
            key,
            command: false,
            shift: false,
            alt: false,
        }
    }

    /// The key with ⌘ on macOS and Ctrl elsewhere.
    pub const fn command(key: Key) -> Self {
        Self {
            command: true,
            ..Self::new(key)
        }
    }

    pub const fn with_shift(self) -> Self {
        Self {
            shift: true,
            ..self
        }
    }

    pub const fn with_alt(self) -> Self {
        Self { alt: true, ..self }
    }
}

/// Something a shortcut does in the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Command {
    Format(Format),
    /// Check or uncheck a task, or make the line one.
    ToggleTaskLine,
    /// Nest a list item under the one above.
    Indent,
    /// Lift a list item out one level.
    Outdent,
    Undo,
    Redo,
    /// Open find in the note.
    Find,
}

impl Command {
    /// What the shortcuts list calls it.
    pub fn label(self) -> &'static str {
        match self {
            Self::Format(format) => format.label(),
            Self::ToggleTaskLine => "Check or uncheck a task",
            Self::Indent => "Nest a list item",
            Self::Outdent => "Lift a list item out",
            Self::Undo => "Undo",
            Self::Redo => "Redo",
            Self::Find => "Find in the note",
        }
    }

    /// The edit the command makes, or `None` for find, which the editor's
    /// window opens.
    pub fn event(self) -> Option<Event> {
        Some(match self {
            Self::Format(format) => Event::Format(format),
            Self::ToggleTaskLine => Event::ToggleTaskLine,
            Self::Indent => Event::Indent { deindent: false },
            Self::Outdent => Event::Indent { deindent: true },
            Self::Undo => Event::Undo,
            Self::Redo => Event::Redo,
            Self::Find => return None,
        })
    }
}

/// Every command with its shortcuts, the one labels show first. `apple`
/// adds the keys macOS and iOS use, and leaves out Ctrl-Y, which redoes
/// only elsewhere.
pub fn keymap(apple: bool) -> Vec<(Command, Vec<Shortcut>)> {
    use Key::*;
    let command = Shortcut::command;
    let format = |format, shortcut: Shortcut| (Command::Format(format), vec![shortcut]);
    let mut redo = vec![command(Z).with_shift()];
    if !apple {
        redo.push(command(Y));
    }
    vec![
        format(Format::Bold, command(B)),
        format(Format::Italic, command(I)),
        format(Format::Underline, command(U)),
        format(Format::Strikethrough, command(X).with_shift()),
        format(Format::Link, command(K)),
        format(Format::Code, command(C).with_shift()),
        format(Format::Heading(0), command(Num0).with_alt()),
        format(Format::Heading(1), command(Num1).with_alt()),
        format(Format::Heading(2), command(Num2).with_alt()),
        format(Format::Heading(3), command(Num3).with_alt()),
        format(Format::BlockQuote, command(Q).with_alt()),
        format(Format::BulletList, command(Num8).with_shift()),
        format(Format::NumberedList, command(Num7).with_shift()),
        format(Format::CheckList, command(Num9).with_shift()),
        (Command::ToggleTaskLine, vec![command(Enter)]),
        (Command::Indent, vec![Shortcut::new(Tab)]),
        (Command::Outdent, vec![Shortcut::new(Tab).with_shift()]),
        (Command::Undo, vec![command(Z)]),
        (Command::Redo, redo),
        (Command::Find, vec![command(F)]),
    ]
}

/// The command a shortcut runs.
pub fn command_for(shortcut: Shortcut, apple: bool) -> Option<Command> {
    keymap(apple)
        .into_iter()
        .find(|(_, shortcuts)| shortcuts.contains(&shortcut))
        .map(|(command, _)| command)
}

/// The shortcuts of a command, the one labels show first.
pub fn shortcuts_of(command: Command, apple: bool) -> Vec<Shortcut> {
    keymap(apple)
        .into_iter()
        .find(|(listed, _)| *listed == command)
        .map(|(_, shortcuts)| shortcuts)
        .unwrap_or_default()
}
