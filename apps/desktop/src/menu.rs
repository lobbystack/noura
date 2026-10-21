//! The native menu bar on macOS and Windows, built with muda from the
//! menu model in `app-core`. Choosing a command item, Quit and About
//! among them, runs the command through the same dispatch as its
//! shortcut in the window that had focus last; the Edit items act on the
//! focused text field, as the S6 trial (ticket 04) proved. macOS has one
//! menu bar for the app, and on Windows every window carries its own.

use std::collections::HashMap;

use app_core::{Key, MenuItem, Shortcut, SystemItem};
use egui::{Event, Modifiers, ViewportCommand, ViewportId};
use muda::accelerator::{Accelerator, CMD_OR_CTRL, Code, Modifiers as Held};

/// What a native menu item stands for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Choice {
    /// A registered command, by ID.
    Command(String),
    System(SystemItem),
}

const COMMAND_PREFIX: &str = "command:";
const SYSTEM_PREFIX: &str = "system:";

/// Every system item the desktop app carries out itself: the Edit items.
const OWN_ITEMS: [SystemItem; 6] = SystemItem::EDIT;

/// The muda menu item ID of a choice.
pub(crate) fn item_id(choice: &Choice) -> String {
    match choice {
        Choice::Command(id) => format!("{COMMAND_PREFIX}{id}"),
        Choice::System(item) => format!("{SYSTEM_PREFIX}{item:?}"),
    }
}

/// The choice behind a muda menu item ID, if the app handles it.
pub(crate) fn choice_for(id: &muda::MenuId) -> Option<Choice> {
    let id = id.as_ref();
    if let Some(command) = id.strip_prefix(COMMAND_PREFIX) {
        return Some(Choice::Command(command.to_owned()));
    }
    OWN_ITEMS
        .into_iter()
        .find(|item| id == item_id(&Choice::System(*item)))
        .map(Choice::System)
}

/// muda's key code for a key. + and = share a key, so a menu shows Zoom
/// In on =; the window still answers to +.
fn code(key: Key) -> Option<Code> {
    Some(match key {
        Key::A => Code::KeyA,
        Key::B => Code::KeyB,
        Key::C => Code::KeyC,
        Key::D => Code::KeyD,
        Key::E => Code::KeyE,
        Key::F => Code::KeyF,
        Key::G => Code::KeyG,
        Key::H => Code::KeyH,
        Key::I => Code::KeyI,
        Key::J => Code::KeyJ,
        Key::K => Code::KeyK,
        Key::L => Code::KeyL,
        Key::M => Code::KeyM,
        Key::N => Code::KeyN,
        Key::O => Code::KeyO,
        Key::P => Code::KeyP,
        Key::Q => Code::KeyQ,
        Key::R => Code::KeyR,
        Key::S => Code::KeyS,
        Key::T => Code::KeyT,
        Key::U => Code::KeyU,
        Key::V => Code::KeyV,
        Key::W => Code::KeyW,
        Key::X => Code::KeyX,
        Key::Y => Code::KeyY,
        Key::Z => Code::KeyZ,
        Key::Num0 => Code::Digit0,
        Key::Num1 => Code::Digit1,
        Key::Num2 => Code::Digit2,
        Key::Num3 => Code::Digit3,
        Key::Num4 => Code::Digit4,
        Key::Num5 => Code::Digit5,
        Key::Num6 => Code::Digit6,
        Key::Num7 => Code::Digit7,
        Key::Num8 => Code::Digit8,
        Key::Num9 => Code::Digit9,
        Key::F1 => Code::F1,
        Key::F2 => Code::F2,
        Key::F3 => Code::F3,
        Key::F4 => Code::F4,
        Key::F5 => Code::F5,
        Key::F6 => Code::F6,
        Key::F7 => Code::F7,
        Key::F8 => Code::F8,
        Key::F9 => Code::F9,
        Key::F10 => Code::F10,
        Key::F11 => Code::F11,
        Key::F12 => Code::F12,
        Key::Tab => Code::Tab,
        Key::Enter => Code::Enter,
        Key::Escape => Code::Escape,
        Key::Space => Code::Space,
        Key::Backspace => Code::Backspace,
        Key::Delete => Code::Delete,
        Key::Home => Code::Home,
        Key::End => Code::End,
        Key::ArrowUp => Code::ArrowUp,
        Key::ArrowDown => Code::ArrowDown,
        Key::ArrowLeft => Code::ArrowLeft,
        Key::ArrowRight => Code::ArrowRight,
        Key::Comma => Code::Comma,
        Key::Period => Code::Period,
        Key::Minus => Code::Minus,
        Key::Plus | Key::Equals => Code::Equal,
        Key::Slash => Code::Slash,
        Key::Backslash => Code::Backslash,
        Key::OpenBracket => Code::BracketLeft,
        Key::CloseBracket => Code::BracketRight,
        Key::Menu => Code::ContextMenu,
        Key::Letters => return None,
    })
}

/// The muda accelerator for a shortcut: Mod is Command on macOS and Ctrl
/// elsewhere.
pub(crate) fn accelerator(shortcut: &Shortcut) -> Option<Accelerator> {
    let mut held = Held::empty();
    if shortcut.command {
        held |= CMD_OR_CTRL;
    }
    if shortcut.ctrl {
        held |= Held::CONTROL;
    }
    if shortcut.alt {
        held |= Held::ALT;
    }
    if shortcut.shift {
        held |= Held::SHIFT;
    }
    Some(Accelerator::new(held, code(shortcut.key)?))
}

/// The key presses and requests the Edit items stand for, waiting for
/// each window's next frame, and the menu choices waiting to run.
#[derive(Default)]
pub(crate) struct EditInput {
    pending: HashMap<ViewportId, Vec<Event>>,
}

impl EditInput {
    /// Carry out an Edit item for the focused window. Undo, Redo and
    /// Select All send the key press their shortcut makes; Cut, Copy and
    /// Paste ask eframe for the clipboard events the shortcuts make.
    pub(crate) fn run(&mut self, ctx: &egui::Context, item: SystemItem) {
        let viewport = focused_viewport(ctx);
        match item {
            SystemItem::Cut => ctx.send_viewport_cmd_to(viewport, ViewportCommand::RequestCut),
            SystemItem::Copy => ctx.send_viewport_cmd_to(viewport, ViewportCommand::RequestCopy),
            SystemItem::Paste => {
                ctx.send_viewport_cmd_to(viewport, ViewportCommand::RequestPaste);
            }
            SystemItem::Undo | SystemItem::Redo | SystemItem::SelectAll => {
                let Some(shortcut) = item.shortcut() else {
                    return;
                };
                let key = match shortcut.key {
                    Key::A => egui::Key::A,
                    _ => egui::Key::Z,
                };
                let mut modifiers = command_modifiers();
                if shortcut.shift {
                    modifiers |= Modifiers::SHIFT;
                }
                let events = self.pending.entry(viewport).or_default();
                for pressed in [true, false] {
                    events.push(Event::Key {
                        key,
                        physical_key: None,
                        pressed,
                        repeat: false,
                        modifiers,
                    });
                }
                ctx.request_repaint_of(viewport);
            }
            _ => {}
        }
    }

    /// Add the key presses waiting for this window to its frame. Call it
    /// from `eframe::App::raw_input_hook`.
    pub(crate) fn inject(&mut self, input: &mut egui::RawInput) {
        if let Some(events) = self.pending.remove(&input.viewport_id) {
            input.events.extend(events);
        }
    }
}

/// The window with keyboard focus, or the main window when none has it.
fn focused_viewport(ctx: &egui::Context) -> ViewportId {
    ctx.input(|input| {
        input
            .raw
            .viewports
            .iter()
            .find(|(_, info)| info.focused == Some(true))
            .map(|(id, _)| *id)
    })
    .unwrap_or(ViewportId::ROOT)
}

/// The modifiers egui-winit reports for the platform's command key.
fn command_modifiers() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers::MAC_CMD | Modifiers::COMMAND
    } else {
        Modifiers::CTRL | Modifiers::COMMAND
    }
}

/// The choice and shortcut of a model item that becomes a muda item the
/// app handles, if it is one.
pub(crate) fn own_item(item: &MenuItem) -> Option<(Choice, String, Option<Shortcut>)> {
    match item {
        MenuItem::Command {
            id,
            label,
            shortcut,
        } => Some((Choice::Command(id.clone()), label.clone(), *shortcut)),
        MenuItem::System(system) if OWN_ITEMS.contains(system) => Some((
            Choice::System(*system),
            system.label().to_owned(),
            system.shortcut(),
        )),
        _ => None,
    }
}

/// The menu bar as muda builds it on macOS and Windows.
#[cfg(any(target_os = "macos", target_os = "windows"))]
pub(crate) mod native {
    use app_core::{Menu, MenuItem, SystemItem};
    use muda::{IsMenuItem, MenuItemKind, PredefinedMenuItem, Submenu};

    use super::{accelerator, item_id, own_item};

    /// The installed menu bar and the model it shows.
    pub(crate) struct NativeMenu {
        menu: muda::Menu,
        shown: Vec<Menu>,
        /// On Windows, the first window and its menu bar's accelerator
        /// table.
        #[cfg(target_os = "windows")]
        main: isize,
        /// On Windows, the menu bar of every other window. Each window
        /// owns its menu bar, which Windows destroys with the window.
        #[cfg(target_os = "windows")]
        others: Vec<(isize, muda::Menu)>,
    }

    /// Build the menu bar and attach it to the app on macOS, or to the
    /// main window on Windows.
    pub(crate) fn install(
        cc: &eframe::CreationContext<'_>,
        menus: &[Menu],
    ) -> muda::Result<NativeMenu> {
        let menu = muda::Menu::new();
        let window_menu = fill(&menu, menus)?;
        #[cfg(target_os = "windows")]
        let main = attach(cc, &menu, window_menu.as_ref())?;
        #[cfg(not(target_os = "windows"))]
        attach(cc, &menu, window_menu.as_ref())?;
        let native = NativeMenu {
            menu,
            shown: menus.to_vec(),
            #[cfg(target_os = "windows")]
            main,
            #[cfg(target_os = "windows")]
            others: Vec::new(),
        };
        #[cfg(target_os = "windows")]
        native.publish_accelerators();
        Ok(native)
    }

    impl NativeMenu {
        /// Show `menus` instead, when they changed, as when a command
        /// registers or the rail's pages move.
        pub(crate) fn update(&mut self, menus: &[Menu]) -> muda::Result<()> {
            if self.shown == menus {
                return Ok(());
            }
            let window_menu = refill(&self.menu, menus)?;
            after_update(&self.menu, window_menu.as_ref());
            #[cfg(target_os = "windows")]
            {
                for (_, menu) in &self.others {
                    refill(menu, menus)?;
                }
                self.publish_accelerators();
            }
            self.shown = menus.to_vec();
            Ok(())
        }
    }

    /// Take every submenu out of `menu` and fill it with `menus` again.
    /// Returns the Window menu.
    fn refill(menu: &muda::Menu, menus: &[Menu]) -> muda::Result<Option<Submenu>> {
        for item in menu.items() {
            if let MenuItemKind::Submenu(submenu) = item {
                menu.remove(&submenu)?;
            }
        }
        fill(menu, menus)
    }

    /// Append one submenu per menu. Returns the Window menu.
    fn fill(menu: &muda::Menu, menus: &[Menu]) -> muda::Result<Option<Submenu>> {
        let mut window_menu = None;
        for spec in menus {
            let submenu = Submenu::new(&spec.title, true);
            for item in &spec.items {
                let built: Box<dyn IsMenuItem> = match item {
                    MenuItem::Separator => Box::new(PredefinedMenuItem::separator()),
                    MenuItem::System(system) if own_item(item).is_none() => {
                        Box::new(predefined(*system))
                    }
                    _ => {
                        let Some((choice, label, shortcut)) = own_item(item) else {
                            continue;
                        };
                        // Every Edit item is a custom item. muda's
                        // predefined ones send selectors winit's view
                        // doesn't implement, so they'd take the shortcut
                        // and do nothing.
                        Box::new(muda::MenuItem::with_id(
                            item_id(&choice),
                            label,
                            true,
                            shortcut.as_ref().and_then(accelerator),
                        ))
                    }
                };
                submenu.append(built.as_ref())?;
            }
            if spec.title == "Window" {
                window_menu = Some(submenu.clone());
            }
            menu.append(&submenu)?;
        }
        Ok(window_menu)
    }

    fn predefined(item: SystemItem) -> PredefinedMenuItem {
        match item {
            SystemItem::Services => PredefinedMenuItem::services(None),
            SystemItem::Hide => PredefinedMenuItem::hide(None),
            SystemItem::HideOthers => PredefinedMenuItem::hide_others(None),
            SystemItem::ShowAll => PredefinedMenuItem::show_all(None),
            SystemItem::FullScreen => PredefinedMenuItem::fullscreen(None),
            SystemItem::Minimize => PredefinedMenuItem::minimize(None),
            SystemItem::Zoom => PredefinedMenuItem::maximize(None),
            // The app carries out the rest itself.
            _ => PredefinedMenuItem::separator(),
        }
    }

    #[cfg(target_os = "macos")]
    fn attach(
        _cc: &eframe::CreationContext<'_>,
        menu: &muda::Menu,
        window_menu: Option<&Submenu>,
    ) -> muda::Result<()> {
        menu.init_for_nsapp();
        after_update(menu, window_menu);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn after_update(_menu: &muda::Menu, window_menu: Option<&Submenu>) {
        if let Some(window_menu) = window_menu {
            window_menu.set_as_windows_menu_for_nsapp();
        }
    }

    /// Attach the menu bar to the main window. Returns the window.
    #[cfg(target_os = "windows")]
    fn attach(
        cc: &eframe::CreationContext<'_>,
        menu: &muda::Menu,
        _window_menu: Option<&Submenu>,
    ) -> muda::Result<isize> {
        use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

        if let Ok(handle) = cc.window_handle()
            && let RawWindowHandle::Win32(handle) = handle.as_raw()
        {
            let hwnd = handle.hwnd.get();
            // SAFETY: the handle is eframe's live main window.
            unsafe { menu.init_for_hwnd(hwnd)? };
            return Ok(hwnd);
        }
        Ok(0)
    }

    #[cfg(target_os = "windows")]
    fn after_update(_menu: &muda::Menu, _window_menu: Option<&Submenu>) {}

    #[cfg(target_os = "windows")]
    impl NativeMenu {
        /// Give every window of the app that has no menu bar its own, and
        /// let go of the menu bars of windows that closed. Returns how
        /// many windows other than the first have one.
        pub(crate) fn cover_windows(&mut self) -> muda::Result<usize> {
            use windows_sys::Win32::UI::WindowsAndMessaging::IsWindow;

            let before = self.others.len();
            // Windows destroyed the menu bar of a window that closed, with
            // the window.
            // SAFETY: IsWindow takes any handle and only reads it.
            self.others
                .retain(|(hwnd, _)| unsafe { IsWindow(*hwnd as _) } != 0);
            let mut changed = self.others.len() != before;
            for hwnd in windows_without_menu() {
                // A new window may reuse the handle of one that closed.
                self.others.retain(|(other, _)| *other != hwnd);
                let menu = muda::Menu::new();
                fill(&menu, &self.shown)?;
                // SAFETY: the handle is one of this thread's live windows.
                unsafe { menu.init_for_hwnd(hwnd)? };
                self.others.push((hwnd, menu));
                changed = true;
            }
            if changed {
                self.publish_accelerators();
            }
            Ok(self.others.len())
        }

        /// Hand the message hook each window's accelerator table.
        fn publish_accelerators(&self) {
            let tables = std::iter::once((self.main, self.menu.haccel()))
                .chain(
                    self.others
                        .iter()
                        .map(|(hwnd, menu)| (*hwnd, menu.haccel())),
                )
                .collect();
            *ACCELERATORS
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = tables;
        }
    }

    /// This thread's eframe windows that have no menu bar yet: windows
    /// that opened since the last look. winit gives each of its windows
    /// the class "Window Class".
    #[cfg(target_os = "windows")]
    fn windows_without_menu() -> Vec<isize> {
        use windows_sys::{
            Win32::{
                Foundation::{HWND, LPARAM},
                System::Threading::GetCurrentThreadId,
                UI::WindowsAndMessaging::{EnumThreadWindows, GetClassNameW, GetMenu},
            },
            core::BOOL,
        };

        unsafe extern "system" fn add(hwnd: HWND, found: LPARAM) -> BOOL {
            const WINIT_CLASS: &str = "Window Class";
            let mut class = [0u16; 64];
            // SAFETY: the buffer holds `class.len()` characters, and
            // `found` is the vector `windows_without_menu` passed in.
            unsafe {
                let length = GetClassNameW(hwnd, class.as_mut_ptr(), class.len() as i32);
                let class = String::from_utf16_lossy(&class[..length.max(0) as usize]);
                if class == WINIT_CLASS && GetMenu(hwnd).is_null() {
                    (*(found as *mut Vec<isize>)).push(hwnd as isize);
                }
            }
            1
        }

        let mut found: Vec<isize> = Vec::new();
        // SAFETY: the callback only runs during the call, while `found`
        // lives.
        unsafe {
            EnumThreadWindows(
                GetCurrentThreadId(),
                Some(add),
                &mut found as *mut Vec<isize> as LPARAM,
            );
        }
        found
    }

    /// Each window and its menu bar's accelerator table, for the message
    /// hook.
    #[cfg(target_os = "windows")]
    static ACCELERATORS: std::sync::Mutex<Vec<(isize, isize)>> = std::sync::Mutex::new(Vec::new());

    /// winit's message hook: turns menu shortcuts into menu choices before
    /// the window sees the key, with the accelerator table of the window
    /// that got it. Returns true when it handled the message.
    #[cfg(target_os = "windows")]
    pub(crate) fn translate_accelerator(message: *const std::ffi::c_void) -> bool {
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            GA_ROOT, GetAncestor, MSG, TranslateAcceleratorW,
        };

        let message = message.cast::<MSG>();
        // SAFETY: winit passes a valid MSG for the duration of the hook,
        // and GetAncestor only reads the handle.
        let window = unsafe { GetAncestor((*message).hwnd, GA_ROOT) };
        let Some(table) = ACCELERATORS
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .iter()
            .find(|(hwnd, table)| *hwnd == window as isize && *table != 0)
            .map(|(_, table)| *table)
        else {
            return false;
        };
        // SAFETY: as above; the table belongs to that window's live menu.
        unsafe { TranslateAcceleratorW(window, table as _, message) != 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Accelerator {
        text.parse().unwrap()
    }

    #[test]
    fn shortcuts_become_accelerators_on_the_platform_command_key() {
        let cases = [
            (Shortcut::command(Key::N), "CmdOrCtrl+N"),
            (Shortcut::command(Key::N).with_shift(), "CmdOrCtrl+Shift+N"),
            (
                Shortcut::command(Key::CloseBracket).with_shift(),
                "CmdOrCtrl+Shift+BracketRight",
            ),
            (Shortcut::command(Key::Backslash), "CmdOrCtrl+Backslash"),
            (Shortcut::command(Key::Num2), "CmdOrCtrl+Digit2"),
            (Shortcut::command(Key::Comma), "CmdOrCtrl+Comma"),
            (Shortcut::command(Key::Plus), "CmdOrCtrl+Equal"),
            (Shortcut::ctrl(Key::Tab), "Control+Tab"),
            (Shortcut::new(Key::F2), "F2"),
        ];
        for (shortcut, expected) in cases {
            assert_eq!(accelerator(&shortcut), Some(parse(expected)), "{expected}");
        }
        assert_eq!(accelerator(&Shortcut::new(Key::Letters)), None);
    }

    #[test]
    fn every_item_the_app_handles_comes_back_from_its_menu_id() {
        let mut choices = vec![Choice::Command("new-note".to_owned())];
        choices.extend(OWN_ITEMS.map(Choice::System));
        for choice in choices {
            let id = muda::MenuId::new(item_id(&choice));
            assert_eq!(choice_for(&id), Some(choice));
        }
        let services = muda::MenuId::new(item_id(&Choice::System(SystemItem::Services)));
        assert_eq!(choice_for(&services), None, "the system handles Services");
        assert_eq!(choice_for(&muda::MenuId::new("unknown")), None);
    }

    #[test]
    fn the_edit_items_answer_to_their_usual_shortcuts() {
        let shortcut = Shortcut::command;
        let items: Vec<_> = OWN_ITEMS
            .into_iter()
            .filter_map(|item| own_item(&MenuItem::System(item)))
            .map(|(_, label, shortcut)| (label, shortcut))
            .collect();
        assert_eq!(
            items,
            [
                ("Undo".to_owned(), Some(shortcut(Key::Z))),
                ("Redo".to_owned(), Some(shortcut(Key::Z).with_shift())),
                ("Cut".to_owned(), Some(shortcut(Key::X))),
                ("Copy".to_owned(), Some(shortcut(Key::C))),
                ("Paste".to_owned(), Some(shortcut(Key::V))),
                ("Select All".to_owned(), Some(shortcut(Key::A))),
            ]
        );
    }

    #[test]
    fn a_registered_command_in_the_menu_model_becomes_an_item_the_app_runs() {
        let item = MenuItem::Command {
            id: "test.ping".to_owned(),
            label: "Ping".to_owned(),
            shortcut: Some(Shortcut::command(Key::P).with_shift()),
        };
        let (choice, label, shortcut) = own_item(&item).unwrap();
        assert_eq!(choice, Choice::Command("test.ping".to_owned()));
        assert_eq!(label, "Ping");
        assert_eq!(
            shortcut.as_ref().and_then(accelerator),
            Some(parse("CmdOrCtrl+Shift+P"))
        );
        assert_eq!(own_item(&MenuItem::System(SystemItem::Services)), None);
    }

    /// Runs an Edit item and feeds the main window the input it asks
    /// for, the way `eframe::App::raw_input_hook` does.
    fn choose(harness: &mut egui_kittest::Harness<'static, String>, item: SystemItem) {
        let ctx = harness.ctx.clone();
        let mut edit = EditInput::default();
        edit.run(&ctx, item);
        let mut input = egui::RawInput::default();
        edit.inject(&mut input);
        harness.input_mut().events.extend(input.events);
        harness.run();
    }

    #[test]
    fn edit_menu_select_all_undo_and_redo_act_on_the_focused_field() {
        use egui_kittest::kittest::Queryable;

        let mut harness = egui_kittest::Harness::builder()
            .with_step_dt(0.5)
            .build_ui_state(
                |ui, text: &mut String| {
                    let label = ui.label("Name");
                    ui.text_edit_singleline(text).labelled_by(label.id);
                },
                String::new(),
            );
        harness.run();
        harness.get_by_label("Name").focus();
        harness.run();
        harness.get_by_label("Name").type_text("hello world");
        harness.run_steps(3);

        choose(&mut harness, SystemItem::SelectAll);
        harness.get_by_label("Name").type_text("x");
        harness.run_steps(3);
        assert_eq!(harness.state(), "x");

        choose(&mut harness, SystemItem::Undo);
        assert_eq!(harness.state(), "hello world");
        choose(&mut harness, SystemItem::Redo);
        assert_eq!(harness.state(), "x");
    }

    #[test]
    fn cut_copy_and_paste_ask_eframe_for_the_matching_window_command() {
        for (item, request) in [
            (SystemItem::Cut, ViewportCommand::RequestCut),
            (SystemItem::Copy, ViewportCommand::RequestCopy),
            (SystemItem::Paste, ViewportCommand::RequestPaste),
        ] {
            let mut harness = egui_kittest::Harness::new_ui(|ui| {
                ui.label("Window");
            });
            harness.run();
            let ctx = harness.ctx.clone();
            EditInput::default().run(&ctx, item);
            harness.step();
            let commands = &harness.output().viewport_output[&ViewportId::ROOT].commands;
            assert_eq!(commands, &[request], "{item:?}");
        }
    }
}
