//! Speech through the app (ADR 0006): a plugin's declared shortcut is
//! listed with the keys you gave it, a press reaches the plugin as
//! something you did so it may paste, a menu bar entry it adds shows and
//! a choice reaches it, a notification it shows comes back when you
//! choose it, what a kernel event opens shows in the window you used
//! last, a plugin without the capabilities gets nothing, and Settings
//! lists the catalog's models in sections, graded for English and your
//! language, searched, filtered and selected.

mod support;

use std::time::Instant;

use app_core::{SpeechModelFilter, SpeechModelSection, SpeechModelsPage};
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

fn section(
    page: &SpeechModelsPage,
    section: SpeechModelSection,
) -> Option<&Vec<app_core::SpeechModelRow>> {
    page.sections
        .iter()
        .find(|(shown, _)| *shown == section)
        .map(|(_, rows)| rows)
}

#[test]
fn settings_sorts_the_catalog_into_sections_graded_for_english_and_your_language() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let all = SpeechModelFilter::default();

    let page = app.speech_models(&all);
    assert_eq!(page.languages, ["en"]);
    assert!(
        section(&page, SpeechModelSection::OnThisComputer)
            .unwrap()
            .is_empty()
    );
    let yours = section(&page, SpeechModelSection::ForYourLanguages).unwrap();
    assert!(yours[0].recommended, "the recommendation comes first");
    assert!(yours.iter().all(|row| row.grades[0].is_some()));
    assert!(
        yours
            .iter()
            .all(|row| row.license != "other" && !row.license.contains("-nc"))
    );
    assert!(section(&page, SpeechModelSection::Others).is_none());

    app.set_speech_language(Some("fr")).unwrap();
    assert_eq!(app.speech_language().as_deref(), Some("fr"));
    let page = app.speech_models(&all);
    assert_eq!(page.languages, ["en", "fr"]);
    let yours = section(&page, SpeechModelSection::ForYourLanguages).unwrap();
    let ultra = yours.iter().find(|row| row.id == "parakeet-ultra").unwrap();
    assert_eq!(ultra.grades, [Some("A"), Some("A")]);
    assert!(ultra.speed.is_some());
    // A model that doesn't hear French isn't for your languages.
    assert!(yours.iter().all(|row| row.grades[1].is_some()));
    assert!(yours.iter().all(|row| row.id != "parakeet-tdt-0.6b-v2"));

    // The choices live in the app data folder, not the workspace.
    assert!(fixture.app_data.path().join("speech/state.json").exists());
    assert!(!fixture.root.join("speech").exists());
}

#[test]
fn search_and_the_filters_narrow_the_models_and_hide_empty_sections() {
    let fixture = Fixture::new(&[]);
    let app = fixture.app();

    let whisper = app.speech_models(&SpeechModelFilter {
        search: "WHISPER large".to_owned(),
        ..SpeechModelFilter::default()
    });
    assert!(section(&whisper, SpeechModelSection::OnThisComputer).is_none());
    let rows: Vec<&app_core::SpeechModelRow> =
        whisper.sections.iter().flat_map(|(_, rows)| rows).collect();
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|row| row.name.contains("Whisper Large")));
    // A search reaches past the curated models.
    assert!(rows.iter().any(|row| row.id == "whisper-large-v2"));

    let french = app.speech_models(&SpeechModelFilter {
        search: "french".to_owned(),
        ..SpeechModelFilter::default()
    });
    assert!(
        french
            .sections
            .iter()
            .flat_map(|(_, rows)| rows)
            .any(|row| row.id == "parakeet-ultra")
    );

    let live = app.speech_models(&SpeechModelFilter {
        live_preview: true,
        ..SpeechModelFilter::default()
    });
    let rows: Vec<_> = live.sections.iter().flat_map(|(_, rows)| rows).collect();
    assert!(!rows.is_empty() && rows.iter().all(|row| row.streaming));

    let both = app.speech_models(&SpeechModelFilter {
        live_preview: true,
        translates: true,
        ..SpeechModelFilter::default()
    });
    assert!(both.sections.is_empty());
}

#[test]
fn a_downloaded_model_is_on_this_computer_and_selecting_it_reaches_the_plugins() {
    let fixture = Fixture::new(&[]);
    let models = fixture.app_data.path().join("speech/models");
    std::fs::create_dir_all(&models).unwrap();
    let catalog = speech::catalog::catalog();
    for id in ["parakeet-ultra", "whisper-small"] {
        let file = &catalog.model(id).unwrap().file.filename;
        std::fs::write(models.join(file), b"GGUF").unwrap();
    }
    let mut app = fixture.app();
    let all = SpeechModelFilter::default();

    let page = app.speech_models(&all);
    let here = section(&page, SpeechModelSection::OnThisComputer).unwrap();
    assert_eq!(here.len(), 2);
    assert_eq!(here[0].id, "parakeet-ultra");
    assert!(here[0].selected && !here[1].selected);
    assert!(
        section(&page, SpeechModelSection::ForYourLanguages)
            .unwrap()
            .iter()
            .all(|row| !row.downloaded)
    );

    app.select_speech_model("whisper-small").unwrap();
    let page = app.speech_models(&all);
    let here = section(&page, SpeechModelSection::OnThisComputer).unwrap();
    assert_eq!(here[0].id, "whisper-small");
    assert!(here[0].selected);
    assert_eq!(
        app.select_speech_model("whisper-medium").unwrap_err().code,
        "model_missing"
    );

    app.delete_speech_model("whisper-small").unwrap();
    let page = app.speech_models(&all);
    let here = section(&page, SpeechModelSection::OnThisComputer).unwrap();
    assert_eq!(here.len(), 1);
    assert!(here[0].selected, "the recommendation takes over");
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
