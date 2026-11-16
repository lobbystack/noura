//! The real engine with a small Whisper model, which the nightly workflow
//! runs: `NOURA_WHISPER_TEST=1 cargo test -p speech --test whisper`. It
//! downloads Whisper Tiny from Hugging Face through the catalog, checks
//! its hash, loads it with transcribe.cpp and transcribes speech. On a Mac
//! the speech comes from `say`; elsewhere it transcribes silence and a
//! tone, which must answer without failing.

use std::sync::Arc;

use speech::catalog::catalog;
use speech::download::{DownloadOptions, Progress, download};
use speech::engine::{Cancel, Engine, Options, SAMPLE_RATE, TranscribeCpp};

fn wanted() -> bool {
    std::env::var_os("NOURA_WHISPER_TEST").is_some()
}

/// Speech from macOS's `say`, as 16 kHz mono samples, if it's there.
fn spoken(folder: &std::path::Path) -> Option<Vec<f32>> {
    let path = folder.join("spoken.wav");
    let status = std::process::Command::new("say")
        .args(["-o"])
        .arg(&path)
        .args(["--data-format=LEI16@16000", "Hello world, this is a test."])
        .status()
        .ok()?;
    if !status.success() {
        return None;
    }
    let reader = hound::WavReader::open(&path).ok()?;
    Some(
        reader
            .into_samples::<i16>()
            .filter_map(|sample| sample.ok())
            .map(|sample| f32::from(sample) / f32::from(i16::MAX))
            .collect(),
    )
}

#[test]
fn whisper_tiny_downloads_loads_and_transcribes() {
    if !wanted() {
        eprintln!("Set NOURA_WHISPER_TEST=1 to run the Whisper test.");
        return;
    }
    let model = catalog()
        .model("whisper-tiny")
        .expect("Whisper Tiny is in the catalog");
    let folder = std::env::var_os("NOURA_MODEL_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::env::temp_dir().join("noura-whisper-test"));
    let path = folder.join(&model.file.filename);
    if !path.exists() {
        download(
            &model.file,
            &path,
            &DownloadOptions::default(),
            &Arc::new(Progress::default()),
        )
        .expect("Whisper Tiny downloads");
    }
    let loaded = TranscribeCpp
        .load(&path)
        .expect("transcribe.cpp loads Whisper Tiny");
    assert!(!loaded.streams());
    let mut session = loaded.session().unwrap();
    let options = Options {
        language: Some("en".into()),
    };
    let scratch = tempfile::tempdir().unwrap();
    match spoken(scratch.path()) {
        Some(speech) => {
            let pieces = session
                .transcribe(&speech, &options, &Cancel::new())
                .unwrap();
            let text: String = pieces.iter().map(|piece| piece.text.as_str()).collect();
            assert!(text.to_lowercase().contains("hello"), "{text}");
        }
        None => {
            let mut audio = vec![0.0; SAMPLE_RATE as usize];
            audio.extend((0..SAMPLE_RATE).map(|n| (n as f32 * 0.05).sin() * 0.2));
            session
                .transcribe(&audio, &options, &Cancel::new())
                .unwrap();
        }
    }
    // A cancelled run stops.
    let cancel = Cancel::new();
    cancel.cancel();
    let error = session
        .transcribe(&vec![0.0; SAMPLE_RATE as usize * 5], &options, &cancel)
        .unwrap_err();
    assert_eq!(error.code, "cancelled");
}
