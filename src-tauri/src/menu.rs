//! The macOS menu bar. Each app command is a menu item with a keyboard
//! shortcut; choosing it emits an `app-menu` event with the command id, which
//! the app handles like the same shortcut typed in the window. Windows and
//! Linux keep a menu-free window and the app handles those shortcuts itself.

use tauri::{AppHandle, Runtime};

/// Event the frontend listens to. The payload is one command id below.
#[cfg(target_os = "macos")]
pub const MENU_EVENT: &str = "app-menu";

/// Menu item ids carry this prefix so predefined items never reach the app.
#[cfg(target_os = "macos")]
const COMMAND_PREFIX: &str = "noura:";

/// Every command the menu can send, with its label and shortcut. The ids are
/// part of the contract with `packages/workspace/src/app-menu.ts`.
#[cfg(any(target_os = "macos", test))]
pub const COMMANDS: &[(&str, &str, &str)] = &[
    ("new-note", "New Note", "CmdOrCtrl+N"),
    ("new-folder", "New Folder", "CmdOrCtrl+Shift+N"),
    ("quick-open", "Quick Open…", "CmdOrCtrl+O"),
    ("search", "Search Files…", "CmdOrCtrl+Shift+F"),
    ("close-tab", "Close Tab", "CmdOrCtrl+W"),
    ("close-window", "Close Window", "CmdOrCtrl+Shift+W"),
    ("settings", "Settings…", "CmdOrCtrl+,"),
    ("toggle-sidebar", "Toggle Sidebar", "CmdOrCtrl+\\"),
    ("go-home", "Home", "CmdOrCtrl+1"),
    ("go-files", "Files", "CmdOrCtrl+2"),
    ("go-tasks", "Tasks", "CmdOrCtrl+3"),
    ("go-calendar", "Calendar", "CmdOrCtrl+4"),
    ("go-projects", "Projects", "CmdOrCtrl+5"),
    ("go-ai", "AI", "CmdOrCtrl+6"),
    ("next-tab", "Next Tab", "CmdOrCtrl+Shift+]"),
    ("previous-tab", "Previous Tab", "CmdOrCtrl+Shift+["),
];

/// Install the menu bar on macOS. A no-op elsewhere.
pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    {
        app.set_menu(build(app)?)?;
        app.on_menu_event(|app, event| {
            let Some(command) = event.id().as_ref().strip_prefix(COMMAND_PREFIX) else {
                return;
            };
            if command == "close-window" {
                use tauri::Manager;
                if let Some(window) = app.get_webview_window("main") {
                    // Closing asks the app first, so unsaved edits are flushed.
                    let _ = window.close();
                }
                return;
            }
            use tauri::Emitter;
            let _ = app.emit(MENU_EVENT, command);
        });
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
    Ok(())
}

#[cfg(target_os = "macos")]
fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<tauri::menu::Menu<R>> {
    use tauri::menu::{MenuBuilder, MenuItem, MenuItemBuilder, SubmenuBuilder};

    let item = |id: &str| -> tauri::Result<MenuItem<R>> {
        let (_, label, accelerator) = COMMANDS
            .iter()
            .find(|(candidate, _, _)| *candidate == id)
            .copied()
            .unwrap_or((id, id, ""));
        let builder = MenuItemBuilder::with_id(format!("{COMMAND_PREFIX}{id}"), label);
        if accelerator.is_empty() {
            builder.build(app)
        } else {
            builder.accelerator(accelerator).build(app)
        }
    };

    // macOS names the first menu after the app, so it stays "Noura".
    let app_menu = SubmenuBuilder::new(app, "Noura")
        .about(None)
        .separator()
        .item(&item("settings")?)
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .quit()
        .build()?;
    let file = SubmenuBuilder::new(app, "File")
        .item(&item("new-note")?)
        .item(&item("new-folder")?)
        .separator()
        .item(&item("quick-open")?)
        .item(&item("search")?)
        .separator()
        .item(&item("close-tab")?)
        .item(&item("close-window")?)
        .build()?;
    let edit = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let view = SubmenuBuilder::new(app, "View")
        .item(&item("toggle-sidebar")?)
        .separator()
        .item(&item("go-home")?)
        .item(&item("go-files")?)
        .item(&item("go-tasks")?)
        .item(&item("go-calendar")?)
        .item(&item("go-projects")?)
        .item(&item("go-ai")?)
        .separator()
        .fullscreen()
        .build()?;
    let window = SubmenuBuilder::new(app, "Window")
        .minimize()
        .maximize()
        .separator()
        .item(&item("next-tab")?)
        .item(&item("previous-tab")?)
        .build()?;
    MenuBuilder::new(app)
        .items(&[&app_menu, &file, &edit, &view, &window])
        .build()
}

#[cfg(test)]
mod tests {
    use super::COMMANDS;

    #[test]
    fn command_ids_and_shortcuts_are_unique() {
        let mut ids = COMMANDS.iter().map(|(id, _, _)| *id).collect::<Vec<_>>();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), COMMANDS.len());
        let mut shortcuts = COMMANDS
            .iter()
            .map(|(_, _, shortcut)| *shortcut)
            .collect::<Vec<_>>();
        shortcuts.sort_unstable();
        shortcuts.dedup();
        assert_eq!(shortcuts.len(), COMMANDS.len());
    }
}
