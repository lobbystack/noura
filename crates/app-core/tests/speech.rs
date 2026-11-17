//! Speech through the app (ADR 0006): a plugin's declared shortcut is
//! listed with the keys you gave it, a press reaches the plugin as
//! something you did so it may paste, a menu bar entry it adds shows and
//! a choice reaches it, a notification it shows comes back when you
//! choose it, what a kernel event opens shows in the window you used
//! last, a plugin without the capabilities gets nothing, and Settings
//! lists the catalog's models graded for your language.

mod support;

use std::time::Instant;

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
    // Keys typed as macOS writes them save as the manifest writes them.
    app.set_shortcut_keys("probe", "paste", Some("\u{2303}\u{2325}D"))
        .unwrap();
    assert_eq!(app.global_shortcuts()[0].keys, "Ctrl+Alt+D");
    app.set_shortcut_keys("probe", "paste", None).unwrap();
    assert_eq!(app.global_shortcuts()[0].keys, "Alt+Space");

    // The capability line shows the keys as this system writes them.
    let keys = if cfg!(target_os = "macos") {
        "\u{2325}Space"
    } else {
        "Alt+Space"
    };
    let row = app
        .plugins()
        .into_iter()
        .find(|row| row.id == "probe")
        .unwrap();
    let line = row
        .approved
        .iter()
        .find(|line| line.name == "global-shortcuts")
        .unwrap();
    assert!(
        line.description
            .ends_with(&format!(": Dictation ({keys}).")),
        "{}",
        line.description
    );

    app.press_shortcut("probe", "paste", true, Instant::now());
    app.press_shortcut("probe", "paste", false, Instant::now());
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
    app.press_shortcut("probe", "paste", true, Instant::now());
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
    app.set_speech_language(Some("de")).unwrap();
    // The choices live in the app data folder, not the workspace.
    assert!(fixture.app_data.path().join("speech/state.json").exists());
    assert!(!fixture.root.join("speech").exists());
}

#[test]
fn a_kernel_event_notifies_and_opens_in_the_window_you_used_last() {
    let fixture = Fixture::new(&[]);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let Some(probe) = folder_for(
        "probe_plugin",
        "probe",
        &[
            "name = \"views\"",
            "name = \"global-shortcuts\"\n\
             shortcuts = [{ id = \"notify\", title = \"Notify\", keys = \"Alt+N\" }, \
             { id = \"open\", title = \"Open\", keys = \"Alt+O\" }]",
            "name = \"notifications\"",
        ],
    ) else {
        return;
    };
    plugins::install_and_enable(&mut app, probe.path(), "probe");
    let mut other = app.new_window().expect("a second window opens");
    support::settle(&mut other);

    // Outside something you did, nothing shows.
    assert_eq!(
        plugins::texts(&app, "plugin:probe:notify").unwrap(),
        ["error failed Noura shows notifications only when you act."]
    );
    assert!(system.notifier.shown().is_empty());

    app.press_shortcut("probe", "notify", true, Instant::now());
    wait_until(&mut app, "the notification shows", |_| {
        !system.notifier.shown().is_empty()
    });
    let shown = system.notifier.shown();
    assert_eq!(shown[0].key, "probe/hello");
    assert_eq!(shown[0].title, "Hello");
    assert_eq!(shown[0].actions, [("wave".to_owned(), "Wave".to_owned())]);
    system.notifier.choose("probe/hello", Some("wave"));
    wait_until(&mut app, "the choice reaches the plugin", |app| {
        events(app)
            .iter()
            .any(|line| line.contains("NotificationChosen"))
    });
    assert!(events(&app).contains(
        &"kernel KernelEvent::NotificationChosen(NotificationChoice { id: \"hello\", action: Some(\"wave\") })"
            .to_owned()
    ));

    // You used the second window last: the page opens there.
    other.window_used();
    let events_page = "plugin:probe:events";
    app.press_shortcut("probe", "open", true, Instant::now());
    wait_until(&mut other, "the page opens in the second window", |other| {
        other.current_page() == events_page
    });
    support::settle(&mut app);
    assert_ne!(app.current_page(), events_page);
    assert!(events(&app).contains(&"kernel open ok".to_owned()));
}
