//! Seam 1: the menu bar the desktop app builds its native menus from.

mod support;

use std::time::Instant;

use app_core::{
    App, Area, Command, CommandSource, INBOX_PAGE, Key, Menu, MenuItem, MenuName, Platform,
    Shortcut, SystemItem, command_id,
};
use support::{FakeSystem, Fixture};

/// Each menu's title and its items' names: a command's label, a system
/// item's variant, or a dash for a separator.
fn outline(menus: &[Menu]) -> Vec<(String, Vec<String>)> {
    menus
        .iter()
        .map(|menu| {
            let items = menu
                .items
                .iter()
                .map(|item| match item {
                    MenuItem::Command { label, .. } => label.clone(),
                    MenuItem::Separator => "-".to_owned(),
                    MenuItem::System(system) => format!("{system:?}"),
                })
                .collect();
            (menu.title.clone(), items)
        })
        .collect()
}

fn items(menus: &[Menu], title: &str) -> Vec<String> {
    outline(menus)
        .into_iter()
        .find(|(menu, _)| menu == title)
        .unwrap_or_else(|| panic!("no {title} menu"))
        .1
}

/// The app with the commands and pages later tickets add: Quick Open and
/// Search (14), zoom and Settings (15) and the Inbox (16).
fn with_later_commands(fixture: &Fixture, platform: Platform) -> App {
    let mut app = fixture.app_with(FakeSystem::on(platform));
    for (id, label) in [
        (command_id::QUICK_OPEN, "Quick Open…"),
        (command_id::SEARCH, "Search Files…"),
        (command_id::ZOOM_IN, "Zoom In"),
        (command_id::ZOOM_OUT, "Zoom Out"),
        (command_id::ZOOM_RESET, "Actual Size"),
        (command_id::SETTINGS, "Settings…"),
    ] {
        app.register_command(Command::new(id, label, Area::Anywhere), |_| {})
            .unwrap();
    }
    app.register_page(INBOX_PAGE, "Inbox");
    app.register_page("tasks", "Tasks");
    app
}

fn strings(items: &[&str]) -> Vec<String> {
    items.iter().map(|item| (*item).to_owned()).collect()
}

#[test]
fn the_macos_menu_bar_keeps_today_s_menus() {
    let fixture = Fixture::new(&[]);
    let app = with_later_commands(&fixture, Platform::MacOs);
    let menus = app.menu_bar();

    let titles: Vec<_> = menus.iter().map(|menu| menu.title.as_str()).collect();
    assert_eq!(titles, ["Noura", "File", "Edit", "View", "Window"]);
    assert_eq!(
        items(&menus, "Noura"),
        strings(&[
            "About",
            "-",
            "Settings…",
            "-",
            "Services",
            "-",
            "Hide",
            "HideOthers",
            "ShowAll",
            "-",
            "Quit Noura"
        ])
    );
    assert_eq!(
        items(&menus, "File"),
        strings(&[
            "New Note",
            "New Folder",
            "-",
            "Quick Open…",
            "Search Files…",
            "-",
            "Close Tab",
            "Close Window"
        ])
    );
    assert_eq!(
        items(&menus, "Edit"),
        strings(&["Undo", "Redo", "-", "Cut", "Copy", "Paste", "SelectAll"])
    );
    assert_eq!(
        items(&menus, "View"),
        strings(&[
            "Toggle Sidebar",
            "-",
            "Zoom In",
            "Zoom Out",
            "Actual Size",
            "-",
            "Go to Inbox",
            "Go to Files",
            "Go to Tasks",
            "-",
            "FullScreen"
        ])
    );
    assert_eq!(
        items(&menus, "Window"),
        strings(&["Minimize", "Zoom", "-", "Next Tab", "Previous Tab"])
    );
}

#[test]
fn the_windows_menu_bar_puts_settings_and_exit_under_file_and_about_under_help() {
    let fixture = Fixture::new(&[]);
    let app = with_later_commands(&fixture, Platform::Windows);
    let menus = app.menu_bar();

    let titles: Vec<_> = menus.iter().map(|menu| menu.title.as_str()).collect();
    assert_eq!(titles, ["File", "Edit", "View", "Window", "Help"]);
    assert_eq!(
        items(&menus, "File"),
        strings(&[
            "New Note",
            "New Folder",
            "-",
            "Quick Open…",
            "Search Files…",
            "-",
            "Close Tab",
            "Close Window",
            "-",
            "Settings…",
            "-",
            "Exit"
        ])
    );
    assert_eq!(items(&menus, "Help"), strings(&["About"]));
    assert!(!items(&menus, "View").contains(&"FullScreen".to_owned()));
}

#[test]
fn items_whose_command_hasn_t_registered_and_their_separators_drop_out() {
    let fixture = Fixture::new(&[]);
    let app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let menus = app.menu_bar();

    assert_eq!(
        items(&menus, "File"),
        strings(&["New Note", "New Folder", "-", "Close Tab", "Close Window"])
    );
    assert_eq!(
        items(&menus, "View"),
        strings(&["Toggle Sidebar", "-", "Go to Files", "-", "FullScreen"])
    );
}

#[test]
fn menu_items_carry_the_command_s_id_and_first_shortcut() {
    let fixture = Fixture::new(&[]);
    let app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let menus = app.menu_bar();

    let next_tab = menus
        .iter()
        .flat_map(|menu| &menu.items)
        .find_map(|item| match item {
            MenuItem::Command { id, shortcut, .. } if id == command_id::NEXT_TAB => Some(*shortcut),
            _ => None,
        })
        .expect("Next Tab is in a menu");
    assert_eq!(
        next_tab,
        Some(Shortcut::command(Key::CloseBracket).with_shift())
    );
    // Quit is a command too, so it runs from the window where no menu
    // bar takes its key.
    let quit = MenuItem::Command {
        id: command_id::QUIT.to_owned(),
        label: "Quit Noura".to_owned(),
        shortcut: Some(Shortcut::command(Key::Q)),
    };
    assert!(
        menus
            .iter()
            .flat_map(|menu| &menu.items)
            .any(|item| *item == quit)
    );
    let edit = menus.iter().find(|menu| menu.title == "Edit").unwrap();
    assert!(
        edit.items
            .contains(&MenuItem::System(SystemItem::SelectAll))
    );
}

#[test]
fn a_command_added_to_the_registry_appears_in_the_menu_it_names() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::MacOs));
    let command = Command::new("test.ping", "Ping", Area::Anywhere)
        .with_shortcuts([Shortcut::command(Key::P).with_shift()])
        .in_menu(MenuName::File);
    app.register_command(command, |_| {}).unwrap();

    let menus = app.menu_bar();
    assert_eq!(
        items(&menus, "File"),
        strings(&[
            "New Note",
            "New Folder",
            "-",
            "Close Tab",
            "Close Window",
            "-",
            "Ping"
        ])
    );
    let ping = menus
        .iter()
        .flat_map(|menu| &menu.items)
        .find(|item| matches!(item, MenuItem::Command { id, .. } if id == "test.ping"));
    assert_eq!(
        ping,
        Some(&MenuItem::Command {
            id: "test.ping".to_owned(),
            label: "Ping".to_owned(),
            shortcut: Some(Shortcut::command(Key::P).with_shift()),
        })
    );

    // A command naming Help gets a Help menu on macOS too.
    let help = Command::new("test.guide", "Guide", Area::Anywhere).in_menu(MenuName::Help);
    app.register_command(help, |_| {}).unwrap();
    assert_eq!(items(&app.menu_bar(), "Help"), strings(&["Guide"]));
}

#[test]
fn choosing_a_menu_item_runs_its_command() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app_with(FakeSystem::on(Platform::Windows));
    assert!(app.layout().sidebar_shown);

    assert!(app.run_command(
        command_id::TOGGLE_SIDEBAR,
        CommandSource::Menu,
        Instant::now()
    ));

    assert!(!app.layout().sidebar_shown);
}
