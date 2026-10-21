//! Settings through AccessKit: its sections, labels, keyboard paths and
//! what its controls do.

mod support;

use std::{
    io,
    sync::{Arc, Mutex},
    time::Instant,
};

use app_core::{ABOUT_LINKS, LoginItem, Platform, VERSION, command_id};
use app_ui::theme::Palette;
use egui::{
    Key, Modifiers, Theme,
    accesskit::{Role, Toggled},
};
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, FakeSystem, create_workspace, harness_for, harness_on, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

/// A login item that keeps its state in memory.
#[derive(Default)]
struct FakeLoginItem(Mutex<bool>);

impl LoginItem for FakeLoginItem {
    fn is_enabled(&self) -> io::Result<bool> {
        Ok(*self.0.lock().unwrap())
    }

    fn set_enabled(&self, enabled: bool) -> io::Result<()> {
        *self.0.lock().unwrap() = enabled;
        Ok(())
    }
}

struct Opened {
    harness: Ui,
    system: Arc<FakeSystem>,
    login: Arc<FakeLoginItem>,
    folder: std::path::PathBuf,
    /// The app data and the workspace's parent folder, which last as long
    /// as the test binds them.
    folders: (tempfile::TempDir, tempfile::TempDir),
}

/// A workspace named Garden, opened at launch, with a fake system and a
/// fake login item.
fn opened() -> Opened {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), "Garden", &folder);
    let system = FakeSystem::new(Platform::MacOs);
    let login = Arc::new(FakeLoginItem::default());
    let item = login.clone();
    let mut harness = harness_on(
        app_data.path(),
        FakePicker::default(),
        system.clone(),
        move |app| app.set_login_item(item),
    );
    settle(&mut harness);
    Opened {
        harness,
        system,
        login,
        folder,
        folders: (app_data, parent),
    }
}

fn open_settings(harness: &mut Ui) {
    harness
        .get_by_role_and_label(Role::Button, "Settings")
        .click();
    harness.run();
}

fn dialog(harness: &Ui) -> egui_kittest::Node<'_> {
    harness.get_by_role_and_label(Role::Dialog, "Settings")
}

fn section<'a>(harness: &'a Ui, title: &'a str) -> egui_kittest::Node<'a> {
    harness
        .get_by_role_and_label(Role::Navigation, "Settings sections")
        .get_by_role_and_label(Role::Button, title)
}

fn choose_section(harness: &mut Ui, title: &str) {
    section(harness, title).click();
    harness.run();
}

#[test]
fn the_rail_s_settings_button_opens_settings_with_its_sections_by_group() {
    let Opened {
        mut harness,
        folders: _folders,
        ..
    } = opened();
    assert!(
        harness
            .query_by_role_and_label(Role::Dialog, "Settings")
            .is_none()
    );

    open_settings(&mut harness);

    let nav = dialog(&harness).get_by_role_and_label(Role::Navigation, "Settings sections");
    let groups: Vec<(String, Vec<String>)> = nav
        .get_all_by_role(Role::Group)
        .map(|group| {
            let sections = group
                .get_all_by_role(Role::Button)
                .filter_map(|button| button.accesskit_node().label())
                .collect();
            (group.accesskit_node().label().unwrap_or_default(), sections)
        })
        .collect();
    assert_eq!(
        groups,
        [
            (
                "This computer".to_owned(),
                vec![
                    "General".to_owned(),
                    "Editor".to_owned(),
                    "Keyboard shortcuts".to_owned()
                ]
            ),
            ("Workspace".to_owned(), vec!["Name and location".to_owned()]),
            ("App".to_owned(), vec!["About Noura".to_owned()]),
        ]
    );
    // General shows first, and has the keyboard focus.
    let general = section(&harness, "General");
    assert_eq!(general.accesskit_node().toggled(), Some(Toggled::True));
    assert!(general.is_focused());
    assert_eq!(
        section(&harness, "Editor").accesskit_node().toggled(),
        Some(Toggled::False)
    );

    harness.key_press(Key::Escape);
    harness.run();
    assert!(
        harness
            .query_by_role_and_label(Role::Dialog, "Settings")
            .is_none()
    );
    assert!(
        harness
            .get_by_role_and_label(Role::Button, "Settings")
            .is_focused()
    );
}

/// Every interactive node in Settings, as its role and label.
fn controls(harness: &Ui) -> Vec<(Role, Option<String>)> {
    dialog(harness)
        .query_all(egui_kittest::kittest::By::new().predicate(|node| {
            matches!(
                node.role(),
                Role::Button | Role::CheckBox | Role::RadioButton | Role::TextInput | Role::Link
            )
        }))
        .map(|node| (node.accesskit_node().role(), node.accesskit_node().label()))
        .collect()
}

#[test]
fn every_section_and_control_carries_a_label() {
    let Opened {
        mut harness,
        folders: _folders,
        ..
    } = opened();
    open_settings(&mut harness);

    for title in [
        "General",
        "Editor",
        "Keyboard shortcuts",
        "Name and location",
        "About Noura",
    ] {
        choose_section(&mut harness, title);
        let controls = controls(&harness);
        assert!(controls.len() > 4, "{title} shows too little: {controls:?}");
        for (role, label) in controls {
            assert!(
                label.is_some_and(|label| !label.trim().is_empty()),
                "a {role:?} in {title} has no label"
            );
        }
    }
}

#[test]
fn settings_works_from_the_keyboard() {
    let Opened {
        mut harness,
        login,
        folders: _folders,
        ..
    } = opened();

    // The rail's button opens Settings with Enter.
    harness
        .get_by_role_and_label(Role::Button, "Settings")
        .focus();
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    assert!(section(&harness, "General").is_focused());

    // Tab moves down the sections, and Space shows one.
    harness.key_press(Key::Tab);
    harness.run();
    assert!(section(&harness, "Editor").is_focused());
    harness.key_press(Key::Space);
    harness.run();
    harness
        .get_by_role_and_label(Role::RadioButton, "Large")
        .focus();
    harness.run();
    harness.key_press(Key::Space);
    harness.run();
    assert_eq!(
        harness.state().app().preferences().text_size,
        app_core::TextSize::Large
    );

    section(&harness, "General").focus();
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();
    for (label, role) in [
        ("Dark", Role::RadioButton),
        ("Zoom in", Role::Button),
        (
            "Open Noura when you sign in to this computer",
            Role::CheckBox,
        ),
    ] {
        harness.get_by_role_and_label(role, label).focus();
        harness.run();
        harness.key_press(Key::Space);
        harness.run();
    }
    let preferences = *harness.state().app().preferences();
    assert_eq!(preferences.appearance, app_core::Appearance::Dark);
    assert_eq!(preferences.zoom, 110);
    assert!(*login.0.lock().unwrap());
}

/// The window's theme and the background it paints.
fn drawn(harness: &Ui) -> (Theme, egui::Color32) {
    (
        harness.ctx.theme(),
        harness.ctx.global_style().visuals.panel_fill,
    )
}

#[test]
fn appearance_applies_at_once_and_match_system_follows_the_system() {
    let Opened {
        mut harness,
        folders: _folders,
        ..
    } = opened();
    harness.input_mut().system_theme = Some(Theme::Light);
    harness.run();
    open_settings(&mut harness);

    harness
        .get_by_role_and_label(Role::RadioButton, "Dark")
        .click();
    harness.run();
    assert_eq!(drawn(&harness), (Theme::Dark, Palette::dark().background));

    harness.input_mut().system_theme = Some(Theme::Dark);
    harness
        .get_by_role_and_label(Role::RadioButton, "Light")
        .click();
    harness.run();
    assert_eq!(drawn(&harness), (Theme::Light, Palette::light().background));

    harness
        .get_by_role_and_label(Role::RadioButton, "Match system")
        .click();
    harness.run();
    assert_eq!(drawn(&harness), (Theme::Dark, Palette::dark().background));
    for system in [Theme::Light, Theme::Dark, Theme::Light] {
        harness.input_mut().system_theme = Some(system);
        harness.run();
        let palette = match system {
            Theme::Light => Palette::light(),
            Theme::Dark => Palette::dark(),
        };
        assert_eq!(drawn(&harness), (system, palette.background));
    }
}

#[test]
fn zoom_buttons_scale_the_window() {
    let Opened {
        mut harness,
        folders: _folders,
        ..
    } = opened();
    open_settings(&mut harness);

    harness
        .get_by_role_and_label(Role::Button, "Zoom in")
        .click();
    harness.run();
    assert_eq!(harness.ctx.zoom_factor(), 1.1);
    harness.get_by_label("110%");

    harness
        .get_by_role_and_label(Role::Button, "Zoom out")
        .click();
    harness.run();
    harness
        .get_by_role_and_label(Role::Button, "Zoom out")
        .click();
    harness.run();
    assert_eq!(harness.ctx.zoom_factor(), 0.9);

    harness
        .get_by_role_and_label(Role::Button, "Actual size")
        .click();
    harness.run();
    assert_eq!(harness.ctx.zoom_factor(), 1.0);
}

#[test]
fn about_links_open_in_the_browser_and_diagnostics_reach_the_clipboard() {
    let Opened {
        mut harness,
        system,
        folders: _folders,
        ..
    } = opened();
    open_settings(&mut harness);
    choose_section(&mut harness, "About Noura");
    harness.get_by_label(app_core::VERSION);
    harness.get_by_label("MIT, free and open source");

    for link in &ABOUT_LINKS {
        harness
            .get_by_role_and_label(Role::Link, link.title)
            .click();
        harness.run();
    }
    harness
        .get_by_role_and_label(Role::Button, "Copy diagnostics")
        .click();
    harness.run();

    let calls = system.calls();
    let browsed: Vec<String> = ABOUT_LINKS
        .iter()
        .map(|link| format!("browse {}", link.url))
        .collect();
    assert_eq!(calls[..browsed.len()], browsed[..]);
    assert_eq!(calls.len(), browsed.len() + 1);
    assert!(calls[browsed.len()].starts_with("copy Noura "), "{calls:?}");
    harness.get_by_role(Role::Status);
    harness.get_by_label("Diagnostics copied");
}

#[test]
fn name_and_location_renames_the_workspace_and_shows_its_folder() {
    let Opened {
        mut harness,
        system,
        folder,
        folders: _folders,
        ..
    } = opened();
    open_settings(&mut harness);
    choose_section(&mut harness, "Name and location");
    let root = harness.state().app().workspace().unwrap().root().to_owned();
    harness.get_by_label(&root);

    let field = harness.get_by_role_and_label(Role::TextInput, "Workspace name");
    field.focus();
    harness.run();
    harness.key_press_modifiers(egui::Modifiers::COMMAND, Key::A);
    harness
        .get_by_role_and_label(Role::TextInput, "Workspace name")
        .type_text("Orchard");
    harness.run();
    harness.key_press(Key::Enter);
    harness.run();

    let app = harness.state().app();
    assert_eq!(app.workspace().unwrap().name(), "Orchard");
    assert_eq!(app.recents()[0].name, "Orchard");
    assert!(folder.is_dir(), "the folder keeps its name");

    harness
        .get_by_role_and_label(Role::Button, "Show folder")
        .click();
    harness.run();
    assert_eq!(system.calls(), [format!("reveal {root}")]);
}

/// A workspace named Garden opened on `platform`. The harness draws
/// without the system's fonts, which hold macOS's shortcut symbols.
fn opened_on(platform: Platform) -> (Ui, (tempfile::TempDir, tempfile::TempDir)) {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    create_workspace(app_data.path(), "Garden", &parent.path().join("Garden"));
    let mut harness = harness_for(app_data.path(), platform, |_| {});
    settle(&mut harness);
    (harness, (app_data, parent))
}

#[test]
fn mod_plus_equals_minus_and_0_zoom_and_mod_comma_opens_settings() {
    for platform in [Platform::MacOs, Platform::Windows] {
        let (mut harness, _folders) = opened_on(platform);
        for (key, zoom) in [
            (Key::Plus, 1.1),
            (Key::Equals, 1.25),
            (Key::Minus, 1.1),
            (Key::Num0, 1.0),
        ] {
            harness.key_press_modifiers(Modifiers::COMMAND, key);
            harness.run();
            assert_eq!(harness.ctx.zoom_factor(), zoom, "{platform:?} {key:?}");
            assert_eq!(
                harness.state().app().preferences().zoom_factor(),
                zoom,
                "{platform:?} {key:?}"
            );
        }

        assert!(
            harness
                .query_by_role_and_label(Role::Dialog, "Settings")
                .is_none()
        );
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Comma);
        harness.run();
        assert!(section(&harness, "General").is_focused(), "{platform:?}");
        harness.key_press(Key::Escape);
        harness.run();
        assert!(
            harness
                .query_by_role_and_label(Role::Dialog, "Settings")
                .is_none(),
            "{platform:?}"
        );
    }
}

#[test]
fn about_from_the_menu_opens_settings_on_its_about_section() {
    let (mut harness, _folders) = opened_on(Platform::Windows);
    let menus = harness.state().menu_bar();
    let help = menus.iter().find(|menu| menu.title == "Help").unwrap();
    assert!(help.items.iter().any(|item| matches!(
        item,
        app_core::MenuItem::Command { id, label, .. }
            if id == command_id::ABOUT && label == "About Noura"
    )));

    assert!(
        harness
            .state_mut()
            .run_menu_command(command_id::ABOUT, Instant::now())
    );
    harness.run();
    assert_eq!(
        section(&harness, "About Noura").accesskit_node().toggled(),
        Some(Toggled::True)
    );
    harness.get_by_label(VERSION);
}

#[test]
fn settings_lists_every_shortcut_by_area() {
    let (mut harness, _folders) = opened_on(Platform::Windows);
    harness
        .state_mut()
        .run_menu_command(command_id::KEYBOARD_SHORTCUTS, Instant::now());
    harness.run();
    assert_eq!(
        section(&harness, "Keyboard shortcuts")
            .accesskit_node()
            .toggled(),
        Some(Toggled::True)
    );

    let anywhere = dialog(&harness).get_by_role_and_label(Role::List, "Anywhere");
    for (label, keys) in [
        ("Zoom In", "Ctrl++ or Ctrl+="),
        ("Zoom Out", "Ctrl+-"),
        ("Actual Size", "Ctrl+0"),
        ("Settings…", "Ctrl+,"),
    ] {
        let entry = anywhere.get_by_role_and_label(Role::ListItem, label);
        assert_eq!(
            entry.accesskit_node().description().as_deref(),
            Some(keys),
            "{label}"
        );
    }
    dialog(&harness).get_by_role_and_label(Role::List, "File tree");
    dialog(&harness).get_by_role_and_label(Role::List, "Writing");
}
