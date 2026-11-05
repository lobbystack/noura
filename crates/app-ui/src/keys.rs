//! Between egui's keys and `app-core`'s: key presses going in, and
//! shortcuts drawn as ⌘K on macOS and Ctrl+K elsewhere.

use app_core::{Key, KeyPress, Platform, Shortcut};
use egui::{ModifierNames, Modifiers};

/// `app-core`'s name for an egui key. A curly bracket is the shifted
/// bracket on the same key.
fn key_from_egui(key: egui::Key) -> Option<Key> {
    use egui::Key as E;
    Some(match key {
        E::A => Key::A,
        E::B => Key::B,
        E::C => Key::C,
        E::D => Key::D,
        E::E => Key::E,
        E::F => Key::F,
        E::G => Key::G,
        E::H => Key::H,
        E::I => Key::I,
        E::J => Key::J,
        E::K => Key::K,
        E::L => Key::L,
        E::M => Key::M,
        E::N => Key::N,
        E::O => Key::O,
        E::P => Key::P,
        E::Q => Key::Q,
        E::R => Key::R,
        E::S => Key::S,
        E::T => Key::T,
        E::U => Key::U,
        E::V => Key::V,
        E::W => Key::W,
        E::X => Key::X,
        E::Y => Key::Y,
        E::Z => Key::Z,
        E::Num0 => Key::Num0,
        E::Num1 => Key::Num1,
        E::Num2 => Key::Num2,
        E::Num3 => Key::Num3,
        E::Num4 => Key::Num4,
        E::Num5 => Key::Num5,
        E::Num6 => Key::Num6,
        E::Num7 => Key::Num7,
        E::Num8 => Key::Num8,
        E::Num9 => Key::Num9,
        E::F1 => Key::F1,
        E::F2 => Key::F2,
        E::F3 => Key::F3,
        E::F4 => Key::F4,
        E::F5 => Key::F5,
        E::F6 => Key::F6,
        E::F7 => Key::F7,
        E::F8 => Key::F8,
        E::F9 => Key::F9,
        E::F10 => Key::F10,
        E::F11 => Key::F11,
        E::F12 => Key::F12,
        E::Tab => Key::Tab,
        E::Enter => Key::Enter,
        E::Escape => Key::Escape,
        E::Space => Key::Space,
        E::Backspace => Key::Backspace,
        E::Delete => Key::Delete,
        E::Home => Key::Home,
        E::End => Key::End,
        E::ArrowUp => Key::ArrowUp,
        E::ArrowDown => Key::ArrowDown,
        E::ArrowLeft => Key::ArrowLeft,
        E::ArrowRight => Key::ArrowRight,
        E::Comma => Key::Comma,
        E::Period => Key::Period,
        E::Minus => Key::Minus,
        E::Plus => Key::Plus,
        E::Equals => Key::Equals,
        E::Slash => Key::Slash,
        E::Backslash => Key::Backslash,
        E::OpenBracket | E::OpenCurlyBracket => Key::OpenBracket,
        E::CloseBracket | E::CloseCurlyBracket => Key::CloseBracket,
        E::Menu => Key::Menu,
        _ => return None,
    })
}

/// egui's key for `app-core`'s, for drawing it.
fn key_to_egui(key: Key) -> Option<egui::Key> {
    use egui::Key as E;
    Some(match key {
        Key::A => E::A,
        Key::B => E::B,
        Key::C => E::C,
        Key::D => E::D,
        Key::E => E::E,
        Key::F => E::F,
        Key::G => E::G,
        Key::H => E::H,
        Key::I => E::I,
        Key::J => E::J,
        Key::K => E::K,
        Key::L => E::L,
        Key::M => E::M,
        Key::N => E::N,
        Key::O => E::O,
        Key::P => E::P,
        Key::Q => E::Q,
        Key::R => E::R,
        Key::S => E::S,
        Key::T => E::T,
        Key::U => E::U,
        Key::V => E::V,
        Key::W => E::W,
        Key::X => E::X,
        Key::Y => E::Y,
        Key::Z => E::Z,
        Key::Num0 => E::Num0,
        Key::Num1 => E::Num1,
        Key::Num2 => E::Num2,
        Key::Num3 => E::Num3,
        Key::Num4 => E::Num4,
        Key::Num5 => E::Num5,
        Key::Num6 => E::Num6,
        Key::Num7 => E::Num7,
        Key::Num8 => E::Num8,
        Key::Num9 => E::Num9,
        Key::F1 => E::F1,
        Key::F2 => E::F2,
        Key::F3 => E::F3,
        Key::F4 => E::F4,
        Key::F5 => E::F5,
        Key::F6 => E::F6,
        Key::F7 => E::F7,
        Key::F8 => E::F8,
        Key::F9 => E::F9,
        Key::F10 => E::F10,
        Key::F11 => E::F11,
        Key::F12 => E::F12,
        Key::Tab => E::Tab,
        Key::Enter => E::Enter,
        Key::Escape => E::Escape,
        Key::Space => E::Space,
        Key::Backspace => E::Backspace,
        Key::Delete => E::Delete,
        Key::Home => E::Home,
        Key::End => E::End,
        Key::ArrowUp => E::ArrowUp,
        Key::ArrowDown => E::ArrowDown,
        Key::ArrowLeft => E::ArrowLeft,
        Key::ArrowRight => E::ArrowRight,
        Key::Comma => E::Comma,
        Key::Period => E::Period,
        Key::Minus => E::Minus,
        Key::Plus => E::Plus,
        Key::Equals => E::Equals,
        Key::Slash => E::Slash,
        Key::Backslash => E::Backslash,
        Key::OpenBracket => E::OpenBracket,
        Key::CloseBracket => E::CloseBracket,
        Key::Menu => E::Menu,
        Key::Letters => return None,
    })
}

/// The key press an egui key event reports on `platform`. egui-winit
/// reports Command on macOS as `mac_cmd` and `command`, and Ctrl
/// elsewhere as `ctrl` and `command`.
pub(crate) fn press(
    key: egui::Key,
    physical: Option<egui::Key>,
    modifiers: Modifiers,
    platform: Platform,
) -> Option<KeyPress> {
    let mut press = KeyPress::new(key_from_egui(key)?);
    if let Some(physical) = physical.and_then(key_from_egui) {
        press = press.at(physical);
    }
    let (cmd, ctrl) = match platform {
        Platform::MacOs => (modifiers.mac_cmd || modifiers.command, modifiers.ctrl),
        Platform::Windows | Platform::Linux => {
            (modifiers.mac_cmd, modifiers.ctrl || modifiers.command)
        }
    };
    if cmd {
        press = press.with_cmd();
    }
    if ctrl {
        press = press.with_ctrl();
    }
    if modifiers.alt {
        press = press.with_alt();
    }
    if modifiers.shift {
        press = press.with_shift();
    }
    Some(press)
}

/// How a key reads in a shortcut. macOS shows the arrow, Return,
/// Delete, Tab and Escape keys as their symbols; elsewhere arrows read
/// as Up, Down, Left and Right and punctuation as itself.
fn key_text(key: Key, is_mac: bool) -> &'static str {
    let mac_symbol = match key {
        Key::ArrowUp => "↑",
        Key::ArrowDown => "↓",
        Key::ArrowLeft => "←",
        Key::ArrowRight => "→",
        Key::Enter => "↩",
        Key::Backspace => "⌫",
        Key::Delete => "⌦",
        Key::Tab => "⇥",
        Key::Escape => "⎋",
        _ => "",
    };
    if is_mac && !mac_symbol.is_empty() {
        return mac_symbol;
    }
    match key_to_egui(key) {
        None => "A–Z",
        Some(egui::Key::Minus) => "-",
        Some(
            key @ (egui::Key::ArrowUp
            | egui::Key::ArrowDown
            | egui::Key::ArrowLeft
            | egui::Key::ArrowRight),
        ) => key.name(),
        Some(key) => key.symbol_or_name(),
    }
}

/// A shortcut as people read it: `⇧⌘N` on macOS, with egui's modifier
/// symbols, and `Ctrl+Shift+N` elsewhere, with its modifier names.
pub(crate) fn shortcut_text(shortcut: &Shortcut, platform: Platform) -> String {
    let is_mac = platform == Platform::MacOs;
    let mut modifiers = Modifiers::NONE;
    if shortcut.command {
        modifiers |= Modifiers::COMMAND;
    }
    if shortcut.ctrl {
        modifiers |= Modifiers::CTRL;
    }
    if shortcut.alt {
        modifiers |= Modifiers::ALT;
    }
    if shortcut.shift {
        modifiers |= Modifiers::SHIFT;
    }
    let names = if is_mac {
        ModifierNames::SYMBOLS
    } else {
        ModifierNames::NAMES
    };
    let mut text = names.format(&modifiers, is_mac);
    if !text.is_empty() {
        text += names.concat;
    }
    text + key_text(shortcut.key, is_mac)
}

/// Several shortcuts for one thing, as in `⌃⇥ or ⇧⌘]`.
pub(crate) fn shortcuts_text(shortcuts: &[Shortcut], platform: Platform) -> String {
    shortcuts
        .iter()
        .map(|shortcut| shortcut_text(shortcut, platform))
        .collect::<Vec<_>>()
        .join(" or ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shortcuts_read_as_symbols_on_macos_and_as_names_elsewhere() {
        let palette = Shortcut::command(Key::K);
        assert_eq!(shortcut_text(&palette, Platform::MacOs), "⌘K");
        assert_eq!(shortcut_text(&palette, Platform::Windows), "Ctrl+K");
        assert_eq!(shortcut_text(&palette, Platform::Linux), "Ctrl+K");

        let new_folder = Shortcut::command(Key::N).with_shift();
        assert_eq!(shortcut_text(&new_folder, Platform::MacOs), "⇧⌘N");
        assert_eq!(
            shortcut_text(&new_folder, Platform::Windows),
            "Ctrl+Shift+N"
        );

        let next_tab = [
            Shortcut::command(Key::CloseBracket).with_shift(),
            Shortcut::ctrl(Key::Tab),
        ];
        assert_eq!(shortcuts_text(&next_tab, Platform::MacOs), "⇧⌘] or ⌃⇥");
        assert_eq!(
            shortcuts_text(&next_tab, Platform::Windows),
            "Ctrl+Shift+] or Ctrl+Tab"
        );
        assert_eq!(
            shortcut_text(&Shortcut::new(Key::ArrowUp).with_alt(), Platform::MacOs),
            "⌥↑"
        );
        assert_eq!(
            shortcut_text(&Shortcut::new(Key::ArrowUp).with_alt(), Platform::Windows),
            "Alt+Up"
        );
        assert_eq!(
            shortcut_text(&Shortcut::new(Key::Letters), Platform::Windows),
            "A–Z"
        );
        assert_eq!(
            shortcut_text(&Shortcut::new(Key::F2), Platform::MacOs),
            "F2"
        );
    }

    #[test]
    fn egui_s_modifiers_become_the_keys_held_on_each_platform() {
        let cmd_k = press(egui::Key::K, None, Modifiers::COMMAND, Platform::MacOs);
        assert_eq!(cmd_k, Some(KeyPress::new(Key::K).with_cmd()));
        let mac_cmd = Modifiers::MAC_CMD | Modifiers::COMMAND;
        let cmd_k = press(egui::Key::K, None, mac_cmd, Platform::MacOs);
        assert_eq!(cmd_k, Some(KeyPress::new(Key::K).with_cmd()));
        let ctrl_tab = press(egui::Key::Tab, None, Modifiers::CTRL, Platform::MacOs);
        assert_eq!(ctrl_tab, Some(KeyPress::new(Key::Tab).with_ctrl()));

        let ctrl = Modifiers::CTRL | Modifiers::COMMAND;
        let ctrl_k = press(egui::Key::K, None, ctrl, Platform::Windows);
        assert_eq!(ctrl_k, Some(KeyPress::new(Key::K).with_ctrl()));
        let ctrl_k = press(egui::Key::K, None, Modifiers::COMMAND, Platform::Linux);
        assert_eq!(ctrl_k, Some(KeyPress::new(Key::K).with_ctrl()));

        // Shift and ] print }, which egui reports as a curly bracket.
        let shifted = Modifiers::COMMAND | Modifiers::SHIFT;
        let next_tab = press(
            egui::Key::CloseCurlyBracket,
            Some(egui::Key::CloseBracket),
            shifted,
            Platform::MacOs,
        );
        assert_eq!(
            next_tab,
            Some(
                KeyPress::new(Key::CloseBracket)
                    .with_cmd()
                    .with_shift()
                    .at(Key::CloseBracket)
            )
        );
    }
}
