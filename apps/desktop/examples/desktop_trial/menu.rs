//! The menu bar model, and its muda build on macOS and Windows.

use crate::command::{Command, Shortcut};
use muda::accelerator::{Accelerator, CMD_OR_CTRL, Code, Modifiers};

/// One menu in the menu bar.
pub struct Menu {
    pub title: &'static str,
    pub entries: Vec<Entry>,
}

/// An item in a menu.
pub enum Entry {
    /// An item that runs one of the trial's commands.
    Command(Command),
    Separator,
    /// An item the operating system implements.
    Native(Native),
}

/// Menu items the operating system implements. muda leaves out the ones a
/// platform lacks.
pub enum Native {
    Services,
    Hide,
    HideOthers,
    ShowAll,
    Minimize,
    CloseWindow,
}

/// The trial's menu bar, first menu first. On macOS the first menu is the
/// app menu.
pub fn menu_bar() -> Vec<Menu> {
    vec![
        Menu {
            title: "Noura Desktop Trial",
            entries: vec![
                Entry::Command(Command::About),
                Entry::Separator,
                Entry::Native(Native::Services),
                Entry::Separator,
                Entry::Native(Native::Hide),
                Entry::Native(Native::HideOthers),
                Entry::Native(Native::ShowAll),
                Entry::Separator,
                Entry::Command(Command::Quit),
            ],
        },
        Menu {
            title: "Edit",
            entries: vec![
                Entry::Command(Command::Undo),
                Entry::Command(Command::Redo),
                Entry::Separator,
                Entry::Command(Command::Cut),
                Entry::Command(Command::Copy),
                Entry::Command(Command::Paste),
                Entry::Command(Command::SelectAll),
            ],
        },
        Menu {
            title: "Trial",
            entries: vec![Entry::Command(Command::Ping)],
        },
        Menu {
            title: "Window",
            entries: vec![
                Entry::Command(Command::NewWindow),
                Entry::Separator,
                Entry::Native(Native::Minimize),
                Entry::Native(Native::CloseWindow),
            ],
        },
    ]
}

/// The muda menu item ID for a command.
pub fn command_id(command: Command) -> String {
    format!("{command:?}")
}

/// The command behind a muda menu item ID, if the item runs one.
pub fn command_for_id(id: &muda::MenuId) -> Option<Command> {
    Command::ALL
        .into_iter()
        .find(|command| *id == command_id(*command))
}

/// The muda accelerator for a shortcut: Command on macOS, Ctrl elsewhere.
pub fn accelerator(shortcut: Shortcut) -> Accelerator {
    let mut modifiers = CMD_OR_CTRL;
    if shortcut.shift {
        modifiers |= Modifiers::SHIFT;
    }
    let code = format!("Key{}", shortcut.key.name())
        .parse::<Code>()
        .expect("the trial's shortcuts use letter keys");
    Accelerator::new(modifiers, code)
}

/// Sends each menu choice that runs a command to `commands`, and wakes the
/// main window so its next frame runs it. muda calls this on the main
/// thread from the menu's own event handling, outside any egui frame.
pub fn forward_menu_choices(ctx: egui::Context, commands: std::sync::mpsc::Sender<Command>) {
    muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
        if let Some(command) = command_for_id(event.id())
            && commands.send(command).is_ok()
        {
            ctx.request_repaint_of(egui::ViewportId::ROOT);
        }
    }));
}

/// The menu bar as muda builds it on macOS and Windows.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub mod native {
    use muda::{AboutMetadata, MenuItem, PredefinedMenuItem, Submenu};

    use super::{Entry, Menu, Native, accelerator, command_id};
    use crate::command::Command;

    /// The installed menu bar. Keep it alive while the app runs.
    pub struct NativeMenu {
        _menu: muda::Menu,
    }

    /// Builds the menu bar and attaches it to the app on macOS, or to the
    /// main window on Windows.
    pub fn install(cc: &eframe::CreationContext<'_>, bar: &[Menu]) -> muda::Result<NativeMenu> {
        let menu = muda::Menu::new();
        let mut window_menu = None;
        for spec in bar {
            let submenu = Submenu::new(spec.title, true);
            for entry in &spec.entries {
                match entry {
                    Entry::Separator => submenu.append(&PredefinedMenuItem::separator())?,
                    Entry::Command(Command::About) => submenu.append(
                        &PredefinedMenuItem::about(Some(Command::About.label()), Some(about())),
                    )?,
                    // The Edit items are custom items too. muda's predefined
                    // ones send cut:, copy:, paste: and selectAll: to the
                    // first responder, winit's view implements none of them,
                    // and muda turns off auto-enabling, so the menu would take
                    // the shortcut and nothing would happen.
                    Entry::Command(command) => submenu.append(&MenuItem::with_id(
                        command_id(*command),
                        command.label(),
                        true,
                        command.shortcut().map(accelerator),
                    ))?,
                    Entry::Native(native) => submenu.append(&native_item(native))?,
                }
            }
            if spec.title == "Window" {
                window_menu = Some(submenu.clone());
            }
            menu.append(&submenu)?;
        }
        attach(cc, &menu, window_menu.as_ref())?;
        Ok(NativeMenu { _menu: menu })
    }

    fn about() -> AboutMetadata {
        AboutMetadata {
            name: Some("Noura Desktop Trial".to_owned()),
            version: Some(env!("CARGO_PKG_VERSION").to_owned()),
            comments: Some(
                "Trial S6: native menus, drag and drop, a second window and IME.".to_owned(),
            ),
            ..Default::default()
        }
    }

    fn native_item(native: &Native) -> PredefinedMenuItem {
        match native {
            Native::Services => PredefinedMenuItem::services(None),
            Native::Hide => PredefinedMenuItem::hide(None),
            Native::HideOthers => PredefinedMenuItem::hide_others(None),
            Native::ShowAll => PredefinedMenuItem::show_all(None),
            Native::Minimize => PredefinedMenuItem::minimize(None),
            Native::CloseWindow => PredefinedMenuItem::close_window(None),
        }
    }

    #[cfg(target_os = "macos")]
    fn attach(
        _cc: &eframe::CreationContext<'_>,
        menu: &muda::Menu,
        window_menu: Option<&Submenu>,
    ) -> muda::Result<()> {
        menu.init_for_nsapp();
        if let Some(window_menu) = window_menu {
            window_menu.set_as_windows_menu_for_nsapp();
        }
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn attach(
        cc: &eframe::CreationContext<'_>,
        menu: &muda::Menu,
        _window_menu: Option<&Submenu>,
    ) -> muda::Result<()> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        if let Ok(handle) = cc.window_handle()
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            // SAFETY: the handle is eframe's live main window.
            unsafe { menu.init_for_hwnd(handle.hwnd.get())? };
            ACCELERATORS.store(menu.haccel(), std::sync::atomic::Ordering::Relaxed);
        }
        Ok(())
    }

    /// The menu bar's accelerator table, for the message hook.
    #[cfg(target_os = "windows")]
    static ACCELERATORS: std::sync::atomic::AtomicIsize = std::sync::atomic::AtomicIsize::new(0);

    /// winit's message hook: turns menu shortcuts into menu choices before
    /// the window sees the key. Returns true when it handled the message.
    #[cfg(target_os = "windows")]
    pub fn translate_accelerator(message: *const std::ffi::c_void) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MSG, TranslateAcceleratorW};

        let accelerators = ACCELERATORS.load(std::sync::atomic::Ordering::Relaxed);
        if accelerators == 0 {
            return false;
        }
        let message = message.cast::<MSG>();
        // SAFETY: winit passes a valid MSG for the duration of the hook.
        unsafe { TranslateAcceleratorW((*message).hwnd, accelerators as _, message) != 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::Key;

    fn menu<'a>(bar: &'a [Menu], title: &str) -> &'a Menu {
        bar.iter()
            .find(|menu| menu.title == title)
            .unwrap_or_else(|| panic!("no {title} menu"))
    }

    fn commands(menu: &Menu) -> Vec<Command> {
        menu.entries
            .iter()
            .filter_map(|entry| match entry {
                Entry::Command(command) => Some(*command),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn the_app_menu_holds_about_and_quit() {
        let bar = menu_bar();
        let app_menu = &bar[0];
        assert_eq!(commands(app_menu), [Command::About, Command::Quit]);
        assert_eq!(
            Command::Quit.shortcut(),
            Some(Shortcut {
                shift: false,
                key: Key::Q
            })
        );
    }

    #[test]
    fn the_edit_menu_holds_the_standard_items_with_their_usual_shortcuts() {
        let bar = menu_bar();
        let edit = menu(&bar, "Edit");
        let items: Vec<_> = commands(edit)
            .into_iter()
            .map(|command| (command.label(), command.shortcut()))
            .collect();
        let shortcut = |shift, key| Some(Shortcut { shift, key });
        assert_eq!(
            items,
            [
                ("Undo", shortcut(false, Key::Z)),
                ("Redo", shortcut(true, Key::Z)),
                ("Cut", shortcut(false, Key::X)),
                ("Copy", shortcut(false, Key::C)),
                ("Paste", shortcut(false, Key::V)),
                ("Select All", shortcut(false, Key::A)),
            ]
        );
    }

    #[test]
    fn the_trial_menu_holds_a_custom_item_with_a_shortcut() {
        let bar = menu_bar();
        let trial = menu(&bar, "Trial");
        assert_eq!(commands(trial), [Command::Ping]);
        assert_eq!(
            Command::Ping.shortcut(),
            Some(Shortcut {
                shift: true,
                key: Key::P
            })
        );
    }

    #[test]
    fn the_window_menu_opens_a_second_window() {
        let bar = menu_bar();
        let window = menu(&bar, "Window");
        assert_eq!(commands(window), [Command::NewWindow]);
        assert_eq!(Command::NewWindow.label(), "New Window");
    }

    #[test]
    fn no_two_menu_items_share_a_shortcut() {
        let shortcuts: Vec<Shortcut> = menu_bar()
            .iter()
            .flat_map(commands)
            .filter_map(Command::shortcut)
            .collect();
        for (index, shortcut) in shortcuts.iter().enumerate() {
            assert!(
                !shortcuts[index + 1..].contains(shortcut),
                "{shortcut:?} is taken twice"
            );
        }
    }

    #[test]
    fn shortcuts_become_muda_accelerators_on_the_platform_command_key() {
        let parse = |text: &str| text.parse::<muda::accelerator::Accelerator>().unwrap();
        assert_eq!(
            accelerator(Command::Ping.shortcut().unwrap()),
            parse("CmdOrCtrl+Shift+P")
        );
        assert_eq!(
            accelerator(Command::SelectAll.shortcut().unwrap()),
            parse("CmdOrCtrl+A")
        );
    }

    #[test]
    fn every_menu_command_comes_back_from_its_menu_id() {
        for command in menu_bar().iter().flat_map(commands) {
            let id = muda::MenuId::new(command_id(command));
            assert_eq!(command_for_id(&id), Some(command));
        }
        assert_eq!(command_for_id(&muda::MenuId::new("unknown")), None);
    }
}
