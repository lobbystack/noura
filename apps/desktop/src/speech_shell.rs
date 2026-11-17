//! The desktop side of speech (ADR 0006): the plugins' global shortcuts,
//! registered with the system through `global-hotkey`, and Noura's menu
//! in the macOS menu bar, the Windows notification area or the Linux tray
//! through `tray-icon`, which holds the plugins' entries, Open Noura and
//! Quit Noura. While the menu shows, closing the last window hides it and
//! Noura keeps running there, so a shortcut and a recording keep working.
//!
//! Under Wayland, `global-hotkey` can't see keys. There you bind
//! `noura --toggle-dictation`, or `noura --shortcut <plugin>/<shortcut>`,
//! in the desktop's keyboard settings; the running copy hears it through
//! its single-instance socket and sends a press, then a release.

use std::collections::BTreeMap;
use std::sync::mpsc::{Receiver, Sender, channel};

use app_core::{App, GlobalShortcut, MenuBarItems};
use global_hotkey::{GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState, hotkey::HotKey};
use muda::{CheckMenuItem, Menu, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{TrayIcon, TrayIconBuilder};

/// The prefix of every tray menu item's ID, which the menu event handler
/// routes here.
pub const TRAY_PREFIX: &str = "tray:";
const OPEN: &str = "tray:open";
const QUIT: &str = "tray:quit";

/// What the shell heard from the system.
pub enum Heard {
    Hotkey(u32, bool),
    Tray(String),
}

/// What the windows should do after a pass.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Asked {
    pub open: bool,
    pub quit: bool,
}

pub struct SpeechShell {
    hotkeys: Option<GlobalHotKeyManager>,
    /// The registered keys, by the hotkey's ID: whose shortcut and the
    /// hotkey itself.
    registered: BTreeMap<u32, (String, String, HotKey)>,
    shortcuts: Vec<GlobalShortcut>,
    tray: Option<TrayIcon>,
    /// The plugin entries the tray shows, by item ID.
    entries: BTreeMap<String, (String, String)>,
    menus: Vec<MenuBarItems>,
    heard: Receiver<Heard>,
    sender: Sender<Heard>,
}

/// The item ID of `plugin`'s entry `id`.
fn entry_id(plugin: &str, id: &str) -> String {
    format!("{TRAY_PREFIX}{plugin}/{id}")
}

impl SpeechShell {
    /// Start listening for global shortcuts. `wake` draws the windows
    /// again, as a press arrives while they're hidden.
    pub fn new(wake: impl Fn() + Send + Sync + 'static) -> Self {
        let (sender, heard) = channel();
        let hotkeys = match GlobalHotKeyManager::new() {
            Ok(manager) => Some(manager),
            Err(error) => {
                tracing::warn!(%error, "global shortcuts aren't available");
                None
            }
        };
        let tell = sender.clone();
        GlobalHotKeyEvent::set_event_handler(Some(move |event: GlobalHotKeyEvent| {
            let pressed = event.state() == HotKeyState::Pressed;
            if tell.send(Heard::Hotkey(event.id(), pressed)).is_ok() {
                wake();
            }
        }));
        Self {
            hotkeys,
            registered: BTreeMap::new(),
            shortcuts: Vec::new(),
            tray: None,
            entries: BTreeMap::new(),
            menus: Vec::new(),
            heard,
            sender,
        }
    }

    /// Where the menu event handler sends a tray item's ID.
    pub fn tray_sender(&self) -> Sender<Heard> {
        self.sender.clone()
    }

    /// Whether Noura should stay in the menu bar with every window closed:
    /// a plugin added menu entries or answers a shortcut.
    pub fn keeps_running(&self) -> bool {
        self.tray.is_some()
    }

    /// Follow the app: register the shortcuts it lists, show its menu
    /// entries, and hand presses and choices to it. Returns what the
    /// windows should do.
    pub fn pass(&mut self, app: &mut App) -> Asked {
        self.register(app);
        self.update_tray(app);
        let mut asked = Asked::default();
        while let Ok(heard) = self.heard.try_recv() {
            match heard {
                Heard::Hotkey(id, pressed) => {
                    if let Some((plugin, shortcut, _)) = self.registered.get(&id) {
                        app.press_shortcut(plugin, shortcut, pressed);
                    }
                }
                Heard::Tray(id) if id == OPEN => asked.open = true,
                Heard::Tray(id) if id == QUIT => asked.quit = true,
                Heard::Tray(id) => {
                    if let Some((plugin, entry)) = self.entries.get(&id) {
                        app.choose_menu_item(plugin, entry);
                    }
                }
            }
        }
        asked
    }

    fn register(&mut self, app: &mut App) {
        let wanted = app.global_shortcuts();
        if wanted == self.shortcuts {
            return;
        }
        let Some(manager) = &self.hotkeys else {
            self.shortcuts = wanted;
            return;
        };
        for (_, (_, _, hotkey)) in std::mem::take(&mut self.registered) {
            let _ = manager.unregister(hotkey);
        }
        for shortcut in &wanted {
            let parsed = shortcut.keys.parse::<HotKey>();
            let registered = match parsed {
                Ok(hotkey) => match manager.register(hotkey) {
                    Ok(()) => {
                        self.registered.insert(
                            hotkey.id(),
                            (shortcut.plugin.clone(), shortcut.id.clone(), hotkey),
                        );
                        true
                    }
                    Err(error) => {
                        tracing::warn!(%error, keys = %shortcut.keys, "a shortcut's keys are taken");
                        false
                    }
                },
                Err(error) => {
                    tracing::warn!(%error, keys = %shortcut.keys, "a shortcut's keys don't parse");
                    false
                }
            };
            app.shortcut_registered(&shortcut.plugin, &shortcut.id, registered);
        }
        self.shortcuts = wanted;
    }

    fn update_tray(&mut self, app: &App) {
        let menus = app.tray_menu();
        let wanted = !menus.is_empty() || !self.shortcuts.is_empty();
        if !wanted {
            self.tray = None;
            self.menus.clear();
            self.entries.clear();
            return;
        }
        if self.tray.is_some() && menus == self.menus {
            return;
        }
        let (menu, entries) = build_menu(&menus);
        self.entries = entries;
        self.menus = menus;
        match &self.tray {
            Some(tray) => tray.set_menu(Some(Box::new(menu))),
            None => {
                let built = TrayIconBuilder::new()
                    .with_menu(Box::new(menu))
                    .with_tooltip("Noura")
                    .with_icon(icon())
                    .build();
                match built {
                    Ok(tray) => self.tray = Some(tray),
                    Err(error) => tracing::warn!(%error, "the menu bar item didn't show"),
                }
            }
        }
    }
}

/// The tray menu: each plugin's entries, then Open Noura and Quit Noura.
fn build_menu(menus: &[MenuBarItems]) -> (Menu, BTreeMap<String, (String, String)>) {
    let menu = Menu::new();
    let mut ids = BTreeMap::new();
    for items in menus {
        let mut submenus: BTreeMap<String, Submenu> = BTreeMap::new();
        for entry in &items.entries {
            let id = entry_id(&items.plugin, &entry.id);
            let added: Result<(), muda::Error> = match entry.kind {
                plugin_host::MenuBarEntryKind::Separator => append(
                    &menu,
                    &submenus,
                    entry.parent.as_deref(),
                    &PredefinedMenuItem::separator(),
                ),
                plugin_host::MenuBarEntryKind::Submenu => {
                    let submenu = Submenu::with_id(MenuId::new(&id), &entry.label, entry.enabled);
                    let added = append(&menu, &submenus, entry.parent.as_deref(), &submenu);
                    submenus.insert(entry.id.clone(), submenu);
                    added
                }
                plugin_host::MenuBarEntryKind::Item => {
                    ids.insert(id.clone(), (items.plugin.clone(), entry.id.clone()));
                    match entry.checked {
                        Some(checked) => append(
                            &menu,
                            &submenus,
                            entry.parent.as_deref(),
                            &CheckMenuItem::with_id(
                                MenuId::new(&id),
                                &entry.label,
                                entry.enabled,
                                checked,
                                None,
                            ),
                        ),
                        None => append(
                            &menu,
                            &submenus,
                            entry.parent.as_deref(),
                            &MenuItem::with_id(MenuId::new(&id), &entry.label, entry.enabled, None),
                        ),
                    }
                }
            };
            if let Err(error) = added {
                tracing::warn!(%error, "a menu bar entry didn't show");
            }
        }
        let _ = menu.append(&PredefinedMenuItem::separator());
    }
    let _ = menu.append(&MenuItem::with_id(
        MenuId::new(OPEN),
        "Open Noura",
        true,
        None,
    ));
    let _ = menu.append(&MenuItem::with_id(
        MenuId::new(QUIT),
        "Quit Noura",
        true,
        None,
    ));
    (menu, ids)
}

fn append(
    menu: &Menu,
    submenus: &BTreeMap<String, Submenu>,
    parent: Option<&str>,
    item: &dyn muda::IsMenuItem,
) -> Result<(), muda::Error> {
    match parent.and_then(|parent| submenus.get(parent)) {
        Some(submenu) => submenu.append(item),
        None => menu.append(item),
    }
}

/// The tray icon: Noura's 32-pixel app icon.
fn icon() -> tray_icon::Icon {
    let decoded = (|| {
        let decoder = png::Decoder::new(std::io::Cursor::new(include_bytes!("../icons/32x32.png")));
        let mut reader = decoder.read_info().ok()?;
        let mut buffer = vec![0; reader.output_buffer_size()?];
        let frame = reader.next_frame(&mut buffer).ok()?;
        let rgba = match frame.color_type {
            png::ColorType::Rgba => buffer[..frame.buffer_size()].to_vec(),
            png::ColorType::Rgb => buffer[..frame.buffer_size()]
                .as_chunks::<3>()
                .0
                .iter()
                .flat_map(|[red, green, blue]| [*red, *green, *blue, 255])
                .collect(),
            _ => return None,
        };
        tray_icon::Icon::from_rgba(rgba, frame.width, frame.height).ok()
    })();
    decoded.unwrap_or_else(|| {
        tray_icon::Icon::from_rgba(vec![0; 32 * 32 * 4], 32, 32).expect("a blank icon")
    })
}
