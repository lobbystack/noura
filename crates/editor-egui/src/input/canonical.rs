use crate::MdEdit;
use crate::input::{Advance, Bound, Event, Increment, Region};
use comrak::nodes::AstNode;
use editor_core::keymap;
use egui::os::OperatingSystem;
use egui::{self, Key, Modifiers};

/// The advance a navigation key makes with these modifiers held.
fn advance_for(modifiers: Modifiers) -> Advance {
    {
        let should_jump_line = modifiers.mac_cmd;

        let is_apple = cfg!(target_vendor = "apple");
        let is_apple_alt = is_apple && modifiers.alt;
        let is_non_apple_ctrl = !is_apple && modifiers.ctrl;
        let should_jump_word = is_apple_alt || is_non_apple_ctrl;

        if should_jump_line {
            Advance::To(Bound::Line)
        } else if should_jump_word {
            Advance::Next(Bound::Word)
        } else {
            Advance::By(Increment::Char)
        }
    }
}

const PAGE_LINES: usize = 50;

/// `editor-core`'s name for a key the editor's shortcuts use.
fn shortcut_key(key: Key) -> Option<keymap::Key> {
    use keymap::Key as K;
    Some(match key {
        Key::B => K::B,
        Key::C => K::C,
        Key::F => K::F,
        Key::I => K::I,
        Key::K => K::K,
        Key::Q => K::Q,
        Key::U => K::U,
        Key::X => K::X,
        Key::Y => K::Y,
        Key::Z => K::Z,
        Key::Enter => K::Enter,
        Key::Tab => K::Tab,
        Key::Num0 => K::Num0,
        Key::Num1 => K::Num1,
        Key::Num2 => K::Num2,
        Key::Num3 => K::Num3,
        Key::Num7 => K::Num7,
        Key::Num8 => K::Num8,
        Key::Num9 => K::Num9,
        _ => return None,
    })
}

/// Whether shortcuts here read as macOS's, with ⌘ and without Ctrl-Y.
pub(crate) fn is_apple(ctx: &egui::Context) -> bool {
    matches!(ctx.os(), OperatingSystem::Mac | OperatingSystem::IOS)
}

/// The editor command a key press runs. The key the press types comes
/// first; the key at its place on the keyboard stands in when the typed
/// one isn't a shortcut key, as for Shift-8, which types `*`.
pub(crate) fn command_for_key(
    key: Key,
    physical_key: Option<Key>,
    modifiers: Modifiers,
    apple: bool,
) -> Option<keymap::Command> {
    // On macOS, Control isn't the command key and runs no editor shortcut.
    if apple && modifiers.ctrl {
        return None;
    }
    let shortcut = |key| keymap::Shortcut {
        key,
        command: modifiers.command,
        shift: modifiers.shift,
        alt: modifiers.alt,
    };
    [Some(key), physical_key]
        .into_iter()
        .flatten()
        .filter_map(shortcut_key)
        .find_map(|key| keymap::command_for(shortcut(key), apple))
}

impl<'ast> MdEdit {
    /// The edit a shortcut makes, or `None` when it makes none here. Tab
    /// and Shift-Tab outside a list, or in a read-only note, move the
    /// keyboard to the next or previous control instead. Find belongs to
    /// the [`crate::Editor`].
    fn shortcut_event(&mut self, command: keymap::Command) -> Option<Event> {
        let tab = matches!(command, keymap::Command::Indent | keymap::Command::Outdent);
        if tab && (self.renderer.readonly || !self.renderer.tab_indents()) {
            let direction = if command == keymap::Command::Indent {
                egui::FocusDirection::Next
            } else {
                egui::FocusDirection::Previous
            };
            self.renderer
                .ctx
                .memory_mut(|memory| memory.move_focus(direction));
            return None;
        }
        if self.renderer.readonly {
            return None;
        }
        command.event()
    }

    pub fn translate_egui_keyboard_event(
        &mut self,
        event: egui::Event,
        _root: &'ast AstNode<'ast>,
    ) -> Option<Event> {
        if let egui::Event::Key {
            key,
            physical_key,
            pressed: true,
            modifiers,
            ..
        } = event
            && let Some(command) =
                command_for_key(key, physical_key, modifiers, is_apple(&self.renderer.ctx))
        {
            return self.shortcut_event(command);
        }
        match event {
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if matches!(
                key,
                Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown
            ) =>
            {
                let lines = if matches!(key, Key::PageUp | Key::PageDown) {
                    PAGE_LINES
                } else {
                    1
                };
                Some(Event::Select {
                    region: Region::ToAdvance {
                        advance: if modifiers.mac_cmd {
                            Advance::To(Bound::Doc)
                        } else {
                            Advance::By(Increment::Lines(lines))
                        },
                        backwards: matches!(key, Key::ArrowUp | Key::PageUp),
                        extend_selection: modifiers.shift,
                    },
                })
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if matches!(key, Key::ArrowRight | Key::ArrowLeft | Key::Home | Key::End) => {
                Some(Event::Select {
                    region: Region::ToAdvance {
                        advance: if matches!(key, Key::Home | Key::End) {
                            if modifiers.command {
                                Advance::To(Bound::Doc)
                            } else {
                                Advance::To(Bound::Line)
                            }
                        } else {
                            advance_for(modifiers)
                        },
                        backwards: matches!(key, Key::ArrowLeft | Key::Home),
                        extend_selection: modifiers.shift,
                    },
                })
            }
            egui::Event::Paste(text) => {
                if self.renderer.readonly {
                    return None;
                }
                Some(Event::Paste { text })
            }
            egui::Event::Text(text) => {
                if self.renderer.readonly {
                    return None;
                }
                Some(Event::Replace {
                    region: Region::Selection,
                    text: text.clone(),
                    advance_cursor: true,
                })
            }
            // An input method's committed text types like a key. Its
            // composition (preedit) stays out of the note until it commits.
            egui::Event::Ime(egui::ImeEvent::Commit(text)) => {
                if self.renderer.readonly || text.is_empty() {
                    return None;
                }
                Some(Event::Replace {
                    region: Region::Selection,
                    text,
                    advance_cursor: true,
                })
            }
            egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } if matches!(key, Key::Backspace | Key::Delete) => {
                if self.renderer.readonly {
                    return None;
                }
                Some(Event::Delete {
                    region: Region::SelectionOrAdvance {
                        advance: advance_for(modifiers),
                        backwards: key == Key::Backspace,
                    },
                })
            }
            egui::Event::Key {
                key: Key::Enter,
                pressed: true,
                modifiers,
                ..
            } if !cfg!(target_os = "ios") => {
                if self.renderer.readonly {
                    return None;
                }
                Some(Event::Newline {
                    shift: modifiers.shift,
                })
            }
            egui::Event::Key {
                key: Key::A,
                pressed: true,
                modifiers,
                ..
            } if modifiers.command && !cfg!(target_os = "ios") => Some(Event::Select {
                region: Region::Bound {
                    bound: Bound::Doc,
                    backwards: true,
                },
            }),
            egui::Event::Cut => Some(Event::Cut),
            egui::Event::Key {
                key: Key::X,
                pressed: true,
                modifiers,
                ..
            } if modifiers.command && !modifiers.shift && !cfg!(target_os = "ios") => {
                if self.renderer.readonly {
                    return None;
                }
                Some(Event::Cut)
            }
            egui::Event::Copy => Some(Event::Copy),
            egui::Event::Key {
                key: Key::C,
                pressed: true,
                modifiers,
                ..
            } if modifiers.command && !modifiers.shift && !cfg!(target_os = "ios") => {
                Some(Event::Copy)
            }
            egui::Event::Key {
                key: Key::D,
                pressed: true,
                modifiers,
                ..
            } if modifiers.command && modifiers.shift => {
                if self.renderer.readonly {
                    return None;
                }
                Some(Event::ToggleFold { node: None })
            }

            egui::Event::Key {
                key: Key::F2,
                pressed: true,
                ..
            } => Some(Event::ToggleDebug),
            egui::Event::Key {
                key: Key::Equals,
                pressed: true,
                modifiers,
                ..
            } if modifiers.command => Some(Event::IncrementBaseFontSize),
            egui::Event::Key {
                key: Key::Minus,
                pressed: true,
                modifiers,
                ..
            } if modifiers.command => Some(Event::DecrementBaseFontSize),
            _ => None,
        }
    }
}
