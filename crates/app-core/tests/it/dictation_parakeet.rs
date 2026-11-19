//! The Dictation plugin with the real engine, while no window draws, as
//! when you dictate into another app: `NOURA_PARAKEET_TEST=/path/to/models
//! cargo test -p app-core --features parakeet-tests --test
//! dictation_parakeet -- --nocapture`. The folder must hold Parakeet Ultra
//! and Silero VAD; the test links them into a scratch folder, so it never
//! writes there. It runs on macOS only, where `say` makes the speech.
#![cfg(all(target_os = "macos", feature = "parakeet-tests"))]

use crate::support;
use std::sync::Arc;
use std::time::{Duration, Instant};

use speech::catalog::{ONBOARDING_MODEL, VAD_MODEL, catalog};
use support::{
    FakeSystem, Fixture,
    plugins::{self, PluginFolder, wait_until},
};

const MANIFEST: &str = include_str!("../../../plugins/dictation/plugin.toml");

/// `text` spoken by macOS's `say`, as 16 kHz mono samples.
fn spoken(folder: &std::path::Path, text: &str) -> Vec<f32> {
    let path = folder.join("spoken.wav");
    let status = std::process::Command::new("say")
        .arg("-o")
        .arg(&path)
        .args(["--data-format=LEI16@16000", text])
        .status()
        .expect("say runs");
    assert!(status.success());
    let bytes = std::fs::read(&path).unwrap();
    // The samples follow the `data` chunk's header.
    let data = bytes
        .windows(4)
        .position(|window| window == b"data")
        .expect("a WAV data chunk")
        + 8;
    bytes[data..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| f32::from(i16::from_le_bytes(*pair)) / f32::from(i16::MAX))
        .collect()
}

/// Whether Dictation's entry `label` shows, and can be chosen with
/// `enabled`.
fn showing(app: &app_core::App, label: &str) -> bool {
    entry(app, label).is_some()
}

fn enabled(app: &app_core::App, label: &str) -> bool {
    entry(app, label) == Some(true)
}

fn entry(app: &app_core::App, label: &str) -> Option<bool> {
    app.tray_menu()
        .into_iter()
        .filter(|items| items.plugin == "dictation")
        .flat_map(|items| items.entries)
        .find(|entry| entry.label == label)
        .map(|entry| entry.enabled)
}

fn press(app: &mut app_core::App, pressed: bool, at: Instant) {
    app.press_shortcut("dictation", "dictation", pressed, at);
}

#[test]
fn dictation_pastes_parakeet_s_text_while_no_window_draws() {
    let Some(models) = std::env::var_os("NOURA_PARAKEET_TEST") else {
        eprintln!("Set NOURA_PARAKEET_TEST to a folder with Parakeet Ultra and Silero VAD.");
        return;
    };
    let Some(component) = plugins::built("dictation_plugin") else {
        return;
    };
    let folder = PluginFolder::new(MANIFEST, "dictation_plugin.wasm", &component);
    let models = std::path::PathBuf::from(models);
    let fixture = Fixture::new(&[]);
    let linked = fixture.app_data.path().join("speech/models");
    std::fs::create_dir_all(&linked).unwrap();
    for model in [
        catalog().model(ONBOARDING_MODEL).unwrap(),
        catalog().helper(VAD_MODEL).unwrap(),
    ] {
        std::os::unix::fs::symlink(
            models.join(&model.file.filename),
            linked.join(&model.file.filename),
        )
        .unwrap();
    }
    let system = FakeSystem::new();
    *system.real_engine.lock().unwrap() = Some(Arc::new(speech::engine::TranscribeCpp));
    let mut app = fixture.app_with(system.clone());
    plugins::install_and_enable(&mut app, folder.path(), "dictation");
    wait_until(&mut app, "the menu shows", |app| {
        showing(app, "Copy Last Dictation")
    });

    // Tap ⌥Space, speak, tap it again; from here on no window polls.
    let start = Instant::now();
    press(&mut app, true, start);
    press(&mut app, false, start + Duration::from_millis(100));
    let deadline = Instant::now() + Duration::from_secs(60);
    while !showing(&app, "Cancel Dictation") {
        assert!(Instant::now() < deadline, "the recording never started");
        app.deliver_plugin_events();
        std::thread::sleep(Duration::from_millis(20));
    }
    let words = spoken(
        fixture.app_data.path(),
        "Let's review the budget, then hire two engineers.",
    );
    for piece in words
        .chunks(480)
        .chain(std::iter::repeat_n(&[0.0; 480][..], 20))
    {
        system.audio.feed(speech::audio::Source::Microphone, piece);
        // As fast as the microphone would.
        std::thread::sleep(Duration::from_millis(30));
        app.deliver_plugin_events();
    }
    let at = Instant::now();
    press(&mut app, true, at);
    press(&mut app, false, at + Duration::from_millis(100));
    let stopped = Instant::now();
    let deadline = stopped + Duration::from_secs(120);
    while system.pasted.lock().unwrap().is_empty() {
        assert!(Instant::now() < deadline, "nothing was pasted");
        app.deliver_plugin_events();
        std::thread::sleep(Duration::from_millis(20));
    }
    eprintln!(
        "pasted {:?} after {:?}",
        system.pasted.lock().unwrap(),
        stopped.elapsed()
    );
    let pasted = system.pasted.lock().unwrap()[0].to_lowercase();
    assert!(pasted.starts_with("paste "), "{pasted}");
    assert!(pasted.contains("budget"), "{pasted}");
    assert!(pasted.contains("engineers"), "{pasted}");
    let deadline = Instant::now() + Duration::from_secs(10);
    while showing(&app, "Cancel Dictation") || !enabled(&app, "Copy Last Dictation") {
        assert!(Instant::now() < deadline, "the menu never came back");
        app.deliver_plugin_events();
        std::thread::sleep(Duration::from_millis(20));
    }
}
