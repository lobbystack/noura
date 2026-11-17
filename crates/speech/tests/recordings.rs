//! Recordings through the speech service with the fake engine and fake
//! devices: two channels of recorded WAV audio become segments with their
//! channel and times, a streaming model shows live text, a device that
//! fails ends the recording with what it heard, cancelling drops
//! everything, and this device's choices persist.

use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::Mutex;
use speech::audio::{FakeAudio, FakeBackend, Source};
use speech::catalog::{ONBOARDING_MODEL, catalog};
use speech::engine::{FakeEngine, SAMPLE_RATE};
use speech::paste::Paster;
use speech::recording::{Change, State};
use speech::state::PluginSpeech;
use speech::{Event, Speech, SpeechConfig};

#[derive(Default)]
struct FakePaste(Mutex<Vec<String>>);

impl Paster for FakePaste {
    fn paste(&self, text: &str, typing: bool) -> speech::Result<()> {
        self.0.lock().push(format!("paste {typing} {text}"));
        Ok(())
    }

    fn copy(&self, text: &str) -> speech::Result<()> {
        self.0.lock().push(format!("copy {text}"));
        Ok(())
    }
}

struct Kit {
    folder: tempfile::TempDir,
    audio: Arc<FakeAudio>,
    engine: Arc<FakeEngine>,
    paste: Arc<FakePaste>,
    speech: Arc<Speech>,
    events: Arc<Mutex<Vec<Event>>>,
}

fn kit_with(engine: FakeEngine) -> Kit {
    let folder = tempfile::tempdir().unwrap();
    open_in(folder, engine)
}

fn open_in(folder: tempfile::TempDir, engine: FakeEngine) -> Kit {
    let audio = FakeAudio::new();
    let engine = Arc::new(engine);
    let paste = Arc::new(FakePaste::default());
    let speech = Speech::open(SpeechConfig {
        folder: folder.path().to_owned(),
        engine: engine.clone(),
        audio: Arc::new(FakeBackend(audio.clone())),
        paster: paste.clone(),
        catalog: catalog(),
        download: speech::download::DownloadOptions::default(),
        microphone_watch: None,
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let heard = events.clone();
    speech.subscribe(move |event| heard.lock().push(event));
    Kit {
        folder,
        audio,
        engine,
        paste,
        speech,
        events,
    }
}

/// Pretend the model `id` is downloaded.
fn downloaded(folder: &Path, id: &str) {
    let model = catalog().model(id).unwrap();
    let path = folder.join("models").join(&model.file.filename);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"GGUF").unwrap();
}

fn samples(ms: u64) -> usize {
    (ms * u64::from(SAMPLE_RATE) / 1000) as usize
}

/// A WAV file of someone who speaks for each `(talk, pause)` in ms, as a
/// recording of a call would hold, written to `path` and read back.
fn recorded(path: &Path, turns: &[(u64, u64)]) -> Vec<f32> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = hound::WavWriter::create(path, spec).unwrap();
    for (talk, pause) in turns {
        for n in 0..samples(*talk) {
            writer
                .write_sample(((n as f32 * 0.05).sin() * 9000.0) as i16)
                .unwrap();
        }
        for _ in 0..samples(*pause) {
            writer.write_sample(0i16).unwrap();
        }
    }
    writer.finalize().unwrap();
    hound::WavReader::open(path)
        .unwrap()
        .into_samples::<i16>()
        .map(|sample| f32::from(sample.unwrap()) / f32::from(i16::MAX))
        .collect()
}

fn feed(audio: &FakeAudio, source: Source, samples: &[f32]) {
    for piece in samples.chunks(480) {
        audio.feed(source, piece);
    }
}

fn wait_for(speech: &Speech, plugin: &str, id: &str, state: State) -> speech::recording::Snapshot {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = speech.read(plugin, id).unwrap();
        if snapshot.state == state {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "the recording stayed {:?}",
            snapshot.state
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn two_channels_of_a_call_become_segments_with_their_channel_and_times() {
    let kit = kit_with(FakeEngine::default());
    downloaded(kit.folder.path(), ONBOARDING_MODEL);
    let me = recorded(
        &kit.folder.path().join("me.wav"),
        &[(1500, 1000), (1000, 1500)],
    );
    let others = recorded(
        &kit.folder.path().join("others.wav"),
        &[(0, 1000), (2000, 2000)],
    );

    let id = kit
        .speech
        .start(
            "transcription",
            vec![Source::Microphone, Source::SystemAudio],
            None,
            None,
            Some("en"),
        )
        .unwrap();
    feed(&kit.audio, Source::Microphone, &me);
    feed(&kit.audio, Source::SystemAudio, &others);
    kit.speech.stop("transcription", &id).unwrap();
    let done = wait_for(&kit.speech, "transcription", &id, State::Finished);

    let me: Vec<_> = done
        .segments
        .iter()
        .filter(|s| s.channel == Source::Microphone)
        .collect();
    let others: Vec<_> = done
        .segments
        .iter()
        .filter(|s| s.channel == Source::SystemAudio)
        .collect();
    assert_eq!(me.len(), 2, "{:?}", done.segments);
    assert_eq!(others.len(), 1, "{:?}", done.segments);
    // The second thing you said starts after your pause.
    assert!(
        me[1].start_ms >= 2200 && me[1].start_ms <= 2600,
        "{:?}",
        me[1]
    );
    assert!(
        others[0].start_ms >= 700 && others[0].start_ms <= 1100,
        "{:?}",
        others[0]
    );
    assert!(
        done.segments
            .windows(2)
            .all(|pair| pair[0].start_ms <= pair[1].start_ms)
    );
    assert_eq!(done.elapsed_ms, 5000);
    assert!(done.pending.is_empty());

    // The audio comes back as one stereo WAV file.
    let wav = kit.speech.wav("transcription", &id).unwrap();
    let reader = hound::WavReader::new(std::io::Cursor::new(wav)).unwrap();
    assert_eq!(reader.spec().channels, 2);
    assert_eq!(reader.len() as usize, samples(5000) * 2);

    // Another plugin can't see it.
    assert_eq!(
        kit.speech.read("dictation", &id).unwrap_err().code,
        "not_found"
    );
    let events = kit.events.lock().clone();
    assert!(events.contains(&Event::Recording {
        plugin: "transcription".into(),
        id: id.clone(),
        change: Change::State,
    }));
    kit.speech.forget("transcription", &id).unwrap();
    assert_eq!(
        kit.speech.read("transcription", &id).unwrap_err().code,
        "not_found"
    );
    assert_eq!(kit.audio.open_count(), 0);
}

#[test]
fn a_streaming_model_shows_live_text_then_the_final_text() {
    let kit = kit_with(FakeEngine {
        streaming: true,
        texts: Arc::new(Mutex::new(vec!["Buy milk".into()])),
        ..FakeEngine::default()
    });
    downloaded(kit.folder.path(), ONBOARDING_MODEL);
    let id = kit
        .speech
        .start(
            "dictation",
            vec![Source::Microphone],
            Some(ONBOARDING_MODEL),
            None,
            None,
        )
        .unwrap();
    feed(&kit.audio, Source::Microphone, &vec![0.1; samples(1000)]);
    let deadline = Instant::now() + Duration::from_secs(5);
    while kit
        .speech
        .read("dictation", &id)
        .unwrap()
        .pending
        .is_empty()
    {
        assert!(Instant::now() < deadline, "no live text");
        std::thread::sleep(Duration::from_millis(10));
    }
    kit.speech.stop("dictation", &id).unwrap();
    let done = wait_for(&kit.speech, "dictation", &id, State::Finished);
    assert_eq!(done.segments.len(), 1);
    assert_eq!(done.segments[0].text, "Buy milk");
    assert!(done.pending.is_empty());
}

#[test]
fn a_device_that_fails_ends_the_recording_and_keeps_its_text() {
    let kit = kit_with(FakeEngine::default());
    downloaded(kit.folder.path(), ONBOARDING_MODEL);
    let id = kit
        .speech
        .start("dictation", vec![Source::Microphone], None, None, None)
        .unwrap();
    feed(
        &kit.audio,
        Source::Microphone,
        &recorded(&kit.folder.path().join("a.wav"), &[(1000, 1000)]),
    );
    // Wait for the first chunk's text.
    let deadline = Instant::now() + Duration::from_secs(5);
    while kit
        .speech
        .read("dictation", &id)
        .unwrap()
        .segments
        .is_empty()
    {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(10));
    }
    kit.audio.fail(Source::Microphone);
    let failed = kit.speech.read("dictation", &id).unwrap();
    assert_eq!(failed.state, State::Failed);
    assert_eq!(failed.failure.as_deref(), Some("The microphone went away."));
    assert_eq!(failed.segments.len(), 1);
}

#[test]
fn a_recording_without_a_model_or_a_device_doesnt_start() {
    let kit = kit_with(FakeEngine::default());
    let error = kit
        .speech
        .start("dictation", vec![Source::Microphone], None, None, None)
        .unwrap_err();
    assert_eq!(error.code, "model_missing");

    downloaded(kit.folder.path(), ONBOARDING_MODEL);
    kit.audio.broken.lock().push(Source::SystemAudio);
    let error = kit
        .speech
        .start(
            "dictation",
            vec![Source::Microphone, Source::SystemAudio],
            None,
            None,
            None,
        )
        .unwrap_err();
    assert_eq!(error.code, "device_unavailable");
    // The microphone it opened first closed again.
    assert_eq!(kit.audio.open_count(), 0);
}

#[test]
fn cancelling_drops_the_audio_and_the_text() {
    let kit = kit_with(FakeEngine::default());
    downloaded(kit.folder.path(), ONBOARDING_MODEL);
    let id = kit
        .speech
        .start("dictation", vec![Source::Microphone], None, None, None)
        .unwrap();
    feed(
        &kit.audio,
        Source::Microphone,
        &recorded(&kit.folder.path().join("a.wav"), &[(1000, 1000)]),
    );
    kit.speech.cancel("dictation", &id).unwrap();
    let cancelled = kit.speech.read("dictation", &id).unwrap();
    assert_eq!(cancelled.state, State::Cancelled);
    assert!(cancelled.segments.is_empty());
    assert!(
        !kit.folder
            .path()
            .join(format!("audio/{id}-me.wav"))
            .exists()
    );
    assert_eq!(kit.audio.open_count(), 0);
}

#[test]
fn choices_shortcuts_and_the_last_dictation_survive_a_relaunch() {
    let kit = kit_with(FakeEngine::default());
    downloaded(kit.folder.path(), ONBOARDING_MODEL);
    kit.speech.set_language(Some("fr".into())).unwrap();
    kit.speech
        .set_preferences(
            "dictation",
            PluginSpeech {
                model: Some(ONBOARDING_MODEL.into()),
                microphone: Some("fake".into()),
                system_audio: false,
                language: None,
                last_dictation: None,
            },
        )
        .unwrap();
    kit.speech
        .remember_dictation("dictation", "Buy milk")
        .unwrap();
    kit.speech
        .set_shortcut_keys("dictation", "dictation", Some("Ctrl+Alt+D".into()))
        .unwrap();
    // A model that isn't here, or a microphone that isn't, is refused.
    let refused = kit.speech.set_preferences(
        "dictation",
        PluginSpeech {
            model: Some("whisper-small".into()),
            ..PluginSpeech::default()
        },
    );
    assert_eq!(refused.unwrap_err().code, "model_missing");

    let Kit { folder, speech, .. } = kit;
    drop(speech);
    let again = open_in(folder, FakeEngine::default());
    assert_eq!(again.speech.state().language.as_deref(), Some("fr"));
    assert_eq!(
        again.speech.last_dictation("dictation").as_deref(),
        Some("Buy milk")
    );
    assert_eq!(
        again.speech.preferences("dictation").model.as_deref(),
        Some(ONBOARDING_MODEL)
    );
    assert_eq!(
        again
            .speech
            .shortcut_keys("dictation", "dictation", "Alt+Space"),
        "Ctrl+Alt+D"
    );
    assert_eq!(again.speech.shortcut_keys("dictation", "other", "F5"), "F5");
    // Nothing of it lives anywhere but the speech folder.
    assert!(again.folder.path().join("state.json").exists());
}

#[test]
fn kept_audio_stays_until_its_date_and_paste_goes_to_the_paster() {
    let kit = kit_with(FakeEngine::default());
    downloaded(kit.folder.path(), ONBOARDING_MODEL);
    let id = kit
        .speech
        .start("transcription", vec![Source::Microphone], None, None, None)
        .unwrap();
    feed(
        &kit.audio,
        Source::Microphone,
        &recorded(&kit.folder.path().join("a.wav"), &[(800, 800)]),
    );
    assert_eq!(
        kit.speech
            .keep_audio("transcription", &id, 7)
            .unwrap_err()
            .code,
        "not_finished"
    );
    kit.speech.stop("transcription", &id).unwrap();
    wait_for(&kit.speech, "transcription", &id, State::Finished);
    kit.speech.keep_audio("transcription", &id, 7).unwrap();
    kit.speech.forget("transcription", &id).unwrap();
    let kept = kit
        .folder
        .path()
        .join(format!("kept/transcription-{id}.wav"));
    assert!(kept.exists());
    assert_eq!(kit.speech.state().kept.len(), 1);

    kit.speech.paste("Buy milk", false).unwrap();
    kit.speech.paste("Eggs", true).unwrap();
    kit.speech.copy("Bread").unwrap();
    assert_eq!(
        *kit.paste.0.lock(),
        ["paste false Buy milk", "paste true Eggs", "copy Bread"]
    );
    let _ = &kit.engine;
}

#[test]
fn another_app_turning_the_microphone_on_is_an_event() {
    let kit = kit_with(FakeEngine::default());
    let mut was = None;
    *kit.audio.in_use.lock() = Some(false);
    kit.speech.check_microphone(&mut was);
    *kit.audio.in_use.lock() = Some(true);
    kit.speech.check_microphone(&mut was);
    kit.speech.check_microphone(&mut was);
    *kit.audio.in_use.lock() = Some(false);
    kit.speech.check_microphone(&mut was);
    assert_eq!(
        *kit.events.lock(),
        [Event::MicrophoneInUse(true), Event::MicrophoneInUse(false)]
    );
}
