//! The Dictation plugin through the app, with the fake engine and fake
//! microphone (ADR 0006): a held shortcut records, the engine's text gets
//! the custom words from `Settings/Dictation.md` and goes into the focused
//! app, a tap keeps recording until the next press, an empty result
//! pastes nothing, and Copy Last Dictation copies the last dictation,
//! even after Noura starts again.

mod support;

use support::{
    FakeSystem, Fixture,
    plugins::{self, PluginFolder, wait_until},
};

const MANIFEST: &str = include_str!("../../plugins/dictation/plugin.toml");

const SETTINGS: &[u8] = b"---\nid: dictation-settings_01jbcdefghjkmnpqrstvwxyz0a\n\
    type: dictation-settings\nwords:\n- Noura\nactivation: hold-or-toggle\npaste: clipboard\n\
    created: 2026-11-16T10:00:00Z\nupdated: 2026-11-16T10:00:00Z\n---\n";

/// The Dictation plugin as its crate ships it, or `None` when it isn't
/// built.
fn dictation() -> Option<PluginFolder> {
    plugins::built("dictation_plugin")
        .map(|component| PluginFolder::new(MANIFEST, "dictation_plugin.wasm", &component))
}

/// Pretend Parakeet Ultra is downloaded, so a recording can start.
fn downloaded(fixture: &Fixture) {
    let catalog = speech::catalog::catalog();
    let model = catalog.model(speech::catalog::ONBOARDING_MODEL).unwrap();
    let path = fixture
        .app_data
        .path()
        .join("speech/models")
        .join(&model.file.filename);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"GGUF").unwrap();
}

/// The labels of Dictation's menu entries.
fn menu(app: &app_core::App) -> Vec<String> {
    app.tray_menu()
        .into_iter()
        .filter(|items| items.plugin == "dictation")
        .flat_map(|items| items.entries)
        .map(|entry| entry.label)
        .collect()
}

fn showing(app: &app_core::App, label: &str) -> bool {
    menu(app).iter().any(|shown| shown == label)
}

/// A second of speech, then a pause, at 16 kHz.
fn speak(system: &FakeSystem) {
    let mut samples: Vec<f32> = (0..16_000).map(|n| (n as f32 * 0.05).sin() * 0.3).collect();
    samples.extend(std::iter::repeat_n(0.0, 11_200));
    for piece in samples.chunks(480) {
        system.audio.feed(speech::audio::Source::Microphone, piece);
    }
}

/// The moment the first press counts from.
static START: std::sync::LazyLock<std::time::Instant> =
    std::sync::LazyLock::new(std::time::Instant::now);

/// The shortcut's keys go down, or up, `ms` after [`START`].
fn press(app: &mut app_core::App, pressed: bool, ms: u64) {
    let at = *START + std::time::Duration::from_millis(ms);
    app.press_shortcut("dictation", "dictation", pressed, at);
}

fn pasted(system: &FakeSystem) -> Vec<String> {
    system.pasted.lock().unwrap().clone()
}

#[test]
fn a_held_shortcut_pastes_what_you_said_with_your_custom_words() {
    let fixture = Fixture::new(&[("Settings/Dictation.md", SETTINGS)]);
    downloaded(&fixture);
    let system = FakeSystem::new();
    system.engine.texts.lock().push("ask noora about it".into());
    let mut app = fixture.app_with(system.clone());
    let Some(folder) = dictation() else {
        return;
    };
    plugins::install_and_enable(&mut app, folder.path(), "dictation");
    wait_until(&mut app, "the menu shows", |app| {
        showing(app, "Copy Last Dictation")
    });
    assert_eq!(
        app.global_shortcuts()
            .iter()
            .map(|shortcut| (shortcut.plugin.as_str(), shortcut.id.as_str()))
            .collect::<Vec<_>>(),
        [("dictation", "dictation")]
    );

    press(&mut app, true, 0);
    wait_until(&mut app, "the recording starts", |app| {
        showing(app, "Cancel Dictation")
    });
    speak(&system);
    press(&mut app, false, 2000);
    wait_until(&mut app, "the text is pasted", |_| {
        !pasted(&system).is_empty()
    });
    assert_eq!(pasted(&system), ["paste ask Noura about it"]);
    wait_until(&mut app, "the menu is back", |app| {
        showing(app, "Copy Last Dictation") && !showing(app, "Cancel Dictation")
    });

    // Copy Last Dictation copies it, after Noura starts again too.
    drop(app);
    let mut app = fixture.app_with(system.clone());
    wait_until(&mut app, "the menu shows again", |app| {
        showing(app, "Copy Last Dictation")
    });
    app.choose_menu_item("dictation", "copy-last");
    wait_until(&mut app, "the dictation is copied", |_| {
        pasted(&system).len() == 2
    });
    assert_eq!(pasted(&system)[1], "copy ask Noura about it");
}

#[test]
fn a_tap_records_until_the_next_press_and_an_empty_result_pastes_nothing() {
    let fixture = Fixture::new(&[]);
    downloaded(&fixture);
    let system = FakeSystem::new();
    system.engine.texts.lock().push(" . ".into());
    let mut app = fixture.app_with(system.clone());
    let Some(folder) = dictation() else {
        return;
    };
    plugins::install_and_enable(&mut app, folder.path(), "dictation");

    press(&mut app, true, 0);
    wait_until(&mut app, "the recording starts", |app| {
        showing(app, "Cancel Dictation")
    });
    // A tap, under the 300 ms hold threshold.
    press(&mut app, false, 100);
    speak(&system);
    support::settle(&mut app);
    assert!(showing(&app, "Cancel Dictation"), "still recording");

    press(&mut app, true, 5000);
    press(&mut app, false, 6000);
    wait_until(&mut app, "the recording ends", |app| {
        showing(app, "Copy Last Dictation") && !showing(app, "Cancel Dictation")
    });
    assert!(pasted(&system).is_empty(), "{:?}", pasted(&system));
    assert!(
        !fixture.exists("Settings/Dictation.md"),
        "only a change on the settings page writes the file"
    );
}

#[test]
fn the_menu_follows_the_models_with_no_other_event() {
    let fixture = Fixture::new(&[]);
    downloaded(&fixture);
    let system = FakeSystem::new();
    let mut app = fixture.app_with(system.clone());
    let Some(folder) = dictation() else {
        return;
    };
    plugins::install_and_enable(&mut app, folder.path(), "dictation");
    wait_until(&mut app, "the model shows", |app| {
        showing(app, "Parakeet Ultra")
    });

    // Deleting the model redraws Dictation, so its menu drops it.
    app.delete_speech_model(speech::catalog::ONBOARDING_MODEL)
        .unwrap();
    wait_until(&mut app, "the model goes from the menu", |app| {
        !showing(app, "Parakeet Ultra")
    });
}
