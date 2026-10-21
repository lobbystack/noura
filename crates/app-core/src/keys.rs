//! Keys and shortcuts, described without a UI toolkit's types so every
//! client and the native menus read the same keymap.

use crate::Platform;

/// A key, named by the character or label printed on it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Key {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    Num0,
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Tab,
    Enter,
    Escape,
    Space,
    Backspace,
    Delete,
    Home,
    End,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Comma,
    Period,
    Minus,
    Plus,
    Equals,
    Slash,
    Backslash,
    /// `[`, and `{` on the same key.
    OpenBracket,
    /// `]`, and `}` on the same key.
    CloseBracket,
    /// The context menu key.
    Menu,
    /// Any letter, as when you type a name to jump to it. Lists of
    /// shortcuts show it as A–Z; no key press matches it.
    Letters,
}

impl Key {
    /// The number keys 0 to 9, in order.
    pub const DIGITS: [Self; 10] = [
        Self::Num0,
        Self::Num1,
        Self::Num2,
        Self::Num3,
        Self::Num4,
        Self::Num5,
        Self::Num6,
        Self::Num7,
        Self::Num8,
        Self::Num9,
    ];

    /// Keys a shortcut names by their printed character whether or not
    /// Shift is down, since some layouts need Shift to type them: + and =
    /// share a key on many layouts and sit on different keys on others.
    fn ignores_shift(self) -> bool {
        matches!(self, Self::Plus | Self::Minus | Self::Equals)
    }

    /// Keys that also match by their position on a US keyboard, for
    /// layouts that print another character there.
    fn matches_by_position(self) -> bool {
        matches!(
            self,
            Self::OpenBracket | Self::CloseBracket | Self::Backslash
        )
    }

    /// Keys a text field uses to move the caret or delete text.
    fn moves_in_text(self) -> bool {
        matches!(
            self,
            Self::ArrowUp
                | Self::ArrowDown
                | Self::ArrowLeft
                | Self::ArrowRight
                | Self::Home
                | Self::End
                | Self::Backspace
                | Self::Delete
        )
    }
}

/// A shortcut: a key and the modifiers held with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Shortcut {
    pub key: Key,
    /// The platform's command key, written Mod: Command on macOS, Ctrl on
    /// Windows and Linux.
    pub command: bool,
    /// The Control key on every platform, as in Ctrl-Tab.
    pub ctrl: bool,
    /// Option on macOS, Alt on Windows and Linux.
    pub alt: bool,
    pub shift: bool,
}

impl Shortcut {
    /// The key alone.
    pub const fn new(key: Key) -> Self {
        Self {
            key,
            command: false,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    /// Mod and the key.
    pub const fn command(key: Key) -> Self {
        Self {
            command: true,
            ..Self::new(key)
        }
    }

    /// Control and the key, on every platform.
    pub const fn ctrl(key: Key) -> Self {
        Self {
            ctrl: true,
            ..Self::new(key)
        }
    }

    /// The same shortcut with Shift.
    pub const fn with_shift(self) -> Self {
        Self {
            shift: true,
            ..self
        }
    }

    /// The same shortcut with Option or Alt.
    pub const fn with_alt(self) -> Self {
        Self { alt: true, ..self }
    }

    /// Whether the ⌘ key and the Control key are down for this shortcut
    /// on `platform`. Mod is the Control key outside macOS.
    fn held(self, platform: Platform) -> (bool, bool) {
        match platform {
            Platform::MacOs => (self.command, self.ctrl),
            Platform::Windows | Platform::Linux => (false, self.command || self.ctrl),
        }
    }

    /// Whether pressing `press` on `platform` presses this shortcut, by
    /// the character the key prints. A shortcut on +, - or = ignores
    /// Shift.
    pub fn matches(&self, press: &KeyPress, platform: Platform) -> bool {
        press.key == self.key && self.modifiers_match(press, platform)
    }

    /// Whether `press` presses this shortcut by the key's place on a US
    /// keyboard, for the bracket and backslash keys. Shortcuts that match
    /// by printed character come first.
    pub fn matches_by_position(&self, press: &KeyPress, platform: Platform) -> bool {
        self.key.matches_by_position()
            && press.physical == Some(self.key)
            && self.modifiers_match(press, platform)
    }

    fn modifiers_match(&self, press: &KeyPress, platform: Platform) -> bool {
        let (cmd, ctrl) = self.held(platform);
        press.cmd == cmd
            && press.ctrl == ctrl
            && press.alt == self.alt
            && (self.key.ignores_shift() || press.shift == self.shift)
    }

    /// Whether one key press on some platform would press both shortcuts.
    pub fn conflicts_with(&self, other: &Self) -> bool {
        self.key == other.key
            && self.alt == other.alt
            && (self.key.ignores_shift() || self.shift == other.shift)
            && [Platform::MacOs, Platform::Windows]
                .into_iter()
                .any(|platform| self.held(platform) == other.held(platform))
    }
}

/// A key press as the keyboard reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyPress {
    /// The key, by the character it types.
    pub key: Key,
    /// The key at the same position on a US keyboard, when known.
    pub physical: Option<Key>,
    /// The ⌘ Command key on macOS.
    pub cmd: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyPress {
    /// The key with no modifiers.
    pub const fn new(key: Key) -> Self {
        Self {
            key,
            physical: None,
            cmd: false,
            ctrl: false,
            alt: false,
            shift: false,
        }
    }

    pub const fn with_cmd(self) -> Self {
        Self { cmd: true, ..self }
    }

    pub const fn with_ctrl(self) -> Self {
        Self { ctrl: true, ..self }
    }

    pub const fn with_alt(self) -> Self {
        Self { alt: true, ..self }
    }

    pub const fn with_shift(self) -> Self {
        Self {
            shift: true,
            ..self
        }
    }

    /// The same press, reporting the key at its position on a US keyboard.
    pub const fn at(self, physical: Key) -> Self {
        Self {
            physical: Some(physical),
            ..self
        }
    }

    /// Whether a focused text field uses this press to type or edit text:
    /// any key without Mod, Control or Alt, Mod with A, Z, Y, X, C or V,
    /// and Mod or Alt with the keys that move the caret or delete.
    pub fn edits_text(&self, platform: Platform) -> bool {
        let (command, other) = match platform {
            Platform::MacOs => (self.cmd, self.ctrl),
            Platform::Windows | Platform::Linux => (self.ctrl, self.cmd),
        };
        match (command, other, self.alt) {
            (false, false, false) => true,
            (false, false, true) => self.key.moves_in_text(),
            (true, false, false) => {
                self.key.moves_in_text()
                    || matches!(
                        self.key,
                        Key::A | Key::Z | Key::Y | Key::X | Key::C | Key::V
                    )
            }
            _ => false,
        }
    }
}
