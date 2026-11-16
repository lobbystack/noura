//! Speech through the app (ADR 0006): a plugin's declared shortcut is
//! listed with the keys you gave it, a press reaches the plugin as
//! something you did so it may paste, a menu bar entry it adds shows and
//! a choice reaches it, a plugin without the capabilities gets nothing,
//! and Settings lists the catalog's models graded for your language.

mod support;

use support::{
    FakeSystem, Fixture,
    plugins::{self, folder_for, wait_until},
};

const SHORTCUTS: &str = "name = \"global-shortcuts\"\n\
     shortcuts = [{ id = \"paste\", title = \"Dictation\", keys = \"Alt+Space\" }]";

fn events(app: &app_core::App) -> Vec<String> {
    plugins::texts(app, "plugin:probe:events").unwrap_or_default()
}

#[test]
fn a_shortcut_press_reaches_the_plugin_which_pastes_and_its_menu_shows() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let Some(probe) = folder_for(
        "probe_plugin",
        "probe",
        &[
            "name = \"views\"",
            SHORTCUTS,
            "name = \"paste\"",
            "name = \"menu-bar-items\"",
        ],
    ) else {
        return;
    };
    plugins::install_and_enable(&mut app, probe.path(), "probe");

    let shortcuts = app.global_shortcuts();
    assert_eq!(shortcuts.len(), 1, "{shortcuts:?}");
    assert_eq!(
        (
            shortcuts[0].plugin.as_str(),
            shortcuts[0].id.as_str(),
            shortcuts[0].keys.as_str()
        ),
        ("probe", "paste", "Alt+Space")
    );
    app.set_shortcut_keys("probe", "paste", Some("Ctrl+Alt+D"))
        .unwrap();
    assert_eq!(app.global_shortcuts()[0].keys, "Ctrl+Alt+D");
    assert_eq!(
        app.set_shortcut_keys("probe", "paste", Some("ctrl+d"))
            .unwrap_err()
            .code,
        "invalid_keys"
    );
    app.set_shortcut_keys("probe", "paste", None).unwrap();
    assert_eq!(app.global_shortcuts()[0].keys, "Alt+Space");

    app.press_shortcut("probe", "paste", true);
    app.press_shortcut("probe", "paste", false);
    wait_until(&mut app, "the press reaches the plugin", |app| {
        events(app)
            .iter()
            .any(|line| line.contains("ShortcutReleased"))
    });
    let seen = events(&app);
    assert!(seen.contains(&"kernel paste pasted".to_owned()), "{seen:?}");
    assert_eq!(*system.pasted.lock().unwrap(), ["paste pasted"]);

    // A paste outside something you did is refused.
    let refused = plugins::texts(&app, "plugin:probe:speech-paste").unwrap();
    assert_eq!(refused, ["error failed Noura pastes only when you act."]);

    // The menu bar entries show, and choosing one reaches the plugin.
    wait_until(&mut app, "the menu shows", |app| {
        !app.tray_menu().is_empty()
    });
    let menu = app.tray_menu();
    assert_eq!(menu[0].plugin, "probe");
    assert_eq!(menu[0].entries.len(), 2);
    app.choose_menu_item("probe", "item-0");
    wait_until(&mut app, "the choice reaches the plugin", |app| {
        events(app)
            .iter()
            .any(|line| line.contains("MenuItemChosen"))
    });

    // Turning the plugin off takes its shortcut and its menu away.
    assert!(app.set_plugin_enabled("probe", false));
    support::settle(&mut app);
    assert!(app.global_shortcuts().is_empty());
    assert!(app.tray_menu().is_empty());
}

#[test]
fn a_plugin_without_the_capabilities_hears_no_shortcut() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let Some(probe) = folder_for("probe_plugin", "probe", &["name = \"views\""]) else {
        return;
    };
    plugins::install_and_enable(&mut app, probe.path(), "probe");

    assert!(app.global_shortcuts().is_empty());
    app.press_shortcut("probe", "paste", true);
    app.choose_menu_item("probe", "item-0");
    support::settle(&mut app);
    assert!(events(&app).iter().all(|line| !line.starts_with("kernel")));
    assert!(system.pasted.lock().unwrap().is_empty());
}

#[test]
fn settings_lists_the_catalog_graded_for_your_language_and_recommends_parakeet_ultra() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let onboarding = app.speech_onboarding().expect("onboarding shows");
    assert_eq!(onboarding.name, "Parakeet Ultra");
    assert_eq!(onboarding.license, "cc-by-4.0");
    assert!(!onboarding.downloaded);

    app.set_speech_language(Some("fr")).unwrap();
    assert_eq!(app.speech_language().as_deref(), Some("fr"));
    let models = app.speech_models();
    assert!(
        models
            .iter()
            .all(|row| row.license != "other" && !row.license.contains("-nc"))
    );
    let ultra = models
        .iter()
        .find(|row| row.id == "parakeet-ultra")
        .unwrap();
    assert_eq!(ultra.grade, Some("A"));
    assert!(models.iter().any(|row| row.recommended));
    // A model that doesn't speak French has no grade.
    assert!(models.iter().any(|row| row.grade.is_none()));

    app.finish_speech_onboarding().unwrap();
    assert!(app.speech_onboarding().is_none());
    assert!(!app.paste_by_typing());
    app.set_paste_by_typing(true).unwrap();
    assert!(app.paste_by_typing());
    // The choices live in the app data folder, not the workspace.
    assert!(fixture.app_data.path().join("speech/state.json").exists());
    assert!(!fixture.root.join("speech").exists());
}
