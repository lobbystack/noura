//! The real engine through the speech service, as the Transcription
//! plugin records: `NOURA_PARAKEET_TEST=/path/to/models cargo test -p
//! speech --test parakeet -- --nocapture`. The folder must hold Parakeet
//! Ultra and Silero VAD; the test links them into a scratch folder, so it
//! never writes there. With no model picked, the service must choose the
//! downloaded one, cut `say`'s speech with Silero, and transcribe it while
//! it records, at full level and as quiet as a Bluetooth headset.

use std::sync::Arc;
use std::time::{Duration, Instant};

use speech::audio::{FakeAudio, FakeBackend, Source};
use speech::catalog::{ONBOARDING_MODEL, VAD_MODEL, catalog};
use speech::engine::TranscribeCpp;
use speech::paste::Paster;
use speech::recording::State;
use speech::{Speech, SpeechConfig};

struct NoPaste;

impl Paster for NoPaste {
    fn paste(&self, _text: &str, _typing: bool) -> speech::Result<()> {
        Ok(())
    }

    fn copy(&self, _text: &str) -> speech::Result<()> {
        Ok(())
    }
}

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
    hound::WavReader::open(&path)
        .unwrap()
        .into_samples::<i16>()
        .map(|sample| f32::from(sample.unwrap()) / f32::from(i16::MAX))
        .collect()
}

#[test]
fn parakeet_ultra_transcribes_a_recording_with_no_model_picked() {
    record(1.0);
}

/// A Bluetooth headset's microphone can send speech this quiet.
#[test]
fn parakeet_ultra_transcribes_a_quiet_microphone() {
    record(0.003);
}

/// Record `say`'s speech at `gain` and check what Parakeet Ultra heard.
fn record(gain: f32) {
    let Some(models) = std::env::var_os("NOURA_PARAKEET_TEST") else {
        eprintln!("Set NOURA_PARAKEET_TEST to a folder with Parakeet Ultra and Silero VAD.");
        return;
    };
    let models = std::path::PathBuf::from(models);
    let scratch = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(scratch.path().join("models")).unwrap();
    for model in [
        catalog().model(ONBOARDING_MODEL).unwrap(),
        catalog().helper(VAD_MODEL).unwrap(),
    ] {
        std::os::unix::fs::symlink(
            models.join(&model.file.filename),
            scratch.path().join("models").join(&model.file.filename),
        )
        .unwrap();
    }
    let audio = FakeAudio::new();
    let speech = Speech::open(SpeechConfig {
        folder: scratch.path().to_owned(),
        engine: Arc::new(TranscribeCpp),
        audio: Arc::new(FakeBackend(audio.clone())),
        paster: Arc::new(NoPaste),
        catalog: catalog(),
        download: speech::download::DownloadOptions::default(),
        microphone_watch: None,
    });
    let id = speech
        .start("transcription", vec![Source::Microphone], None, None, None)
        .expect("a recording starts with the downloaded model");
    let words = spoken(
        scratch.path(),
        "Good morning everyone. Let's review the budget for the next quarter. \
         [[slnc 1500]] Then we will talk about hiring two engineers.",
    );
    let words: Vec<f32> = words.iter().map(|sample| sample * gain).collect();
    let started = Instant::now();
    let mut first_text = None;
    for piece in words.chunks(480) {
        audio.feed(Source::Microphone, piece);
        // As fast as the microphone would.
        std::thread::sleep(Duration::from_millis(30));
        if first_text.is_none()
            && !speech
                .read("transcription", &id)
                .unwrap()
                .segments
                .is_empty()
        {
            first_text = Some(started.elapsed());
        }
    }
    // A second of silence, then Stop.
    for _ in 0..33 {
        audio.feed(Source::Microphone, &[0.0; 480]);
        std::thread::sleep(Duration::from_millis(30));
    }
    eprintln!("text arrived while recording after {first_text:?}");
    assert!(
        first_text.is_some(),
        "no text arrived while recording: {:?}",
        speech.read("transcription", &id).unwrap()
    );
    speech.stop("transcription", &id).unwrap();
    let deadline = Instant::now() + Duration::from_secs(120);
    let snapshot = loop {
        let snapshot = speech.read("transcription", &id).unwrap();
        if snapshot.state.ended() {
            break snapshot;
        }
        assert!(Instant::now() < deadline, "the recording never finished");
        std::thread::sleep(Duration::from_millis(50));
    };
    eprintln!("{snapshot:#?}");
    assert_eq!(snapshot.state, State::Finished, "{:?}", snapshot.failure);
    let text: String = snapshot
        .segments
        .iter()
        .map(|segment| segment.text.to_lowercase())
        .collect::<Vec<_>>()
        .join(" ");
    assert!(text.contains("budget"), "{text}");
    assert!(text.contains("engineers"), "{text}");
}
