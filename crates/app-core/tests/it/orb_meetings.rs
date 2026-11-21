//! The Meetings Orb end to end: a recording ends in the Transcription
//! plugin, which writes its Transcript and Summary and tells Noura
//! `transcription.finished`. The Meetings Orb's trigger names that event,
//! so Noura starts a run in the Orb's thread with the Transcription's
//! notes and Summary. The model asks to create a task, and the Orb runs
//! `tasks/create` in the Tasks plugin without asking you. The thread and
//! the run's record hold every step, and the same Transcription starts no
//! second run.

use crate::support;
use std::{path::Path, sync::Arc, time::Instant};

use app_core::{AiOrbRunStatus, AiOrbStart, App, Field, InputValue, PluginEvent};
use speech::audio::{FakeAudio, Source};
use speech::engine::SAMPLE_RATE;
use support::{
    FakeSystem, Fixture,
    orbs::{self, MEETINGS, Reply, Scripted},
    plugins::{self, PluginFolder, wait_until},
};

const MANIFEST: &str = include_str!("../../../plugins/transcription/plugin.toml");
const NOTE: &str = "Meetings/Transcription 2026-11-16/Transcription 2026-11-16.md";
const SUMMARY: &str = "# Next Steps\n- Ada sends the deck on Friday.";

/// A WAV file of someone who stays quiet, then talks, for each
/// `(quiet, talk)` in ms, written to `path` and read back as samples.
fn recorded(path: &Path, turns: &[(u64, u64)]) -> Vec<f32> {
    let samples = |ms: u64| (ms * u64::from(SAMPLE_RATE) / 1000) as usize;
    let mut pcm: Vec<i16> = Vec::new();
    for (quiet, talk) in turns {
        pcm.extend(std::iter::repeat_n(0, samples(*quiet)));
        pcm.extend((0..samples(*talk)).map(|n| ((n as f32 * 0.05).sin() * 9000.0) as i16));
    }
    let data_len = u32::try_from(pcm.len() * 2).unwrap();
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    wav.extend_from_slice(&(SAMPLE_RATE * 2).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in &pcm {
        wav.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, &wav).unwrap();
    let read = std::fs::read(path).unwrap();
    read[44..]
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| f32::from(i16::from_le_bytes(*pair)) / f32::from(i16::MAX))
        .collect()
}

fn feed(audio: &FakeAudio, source: Source, samples: &[f32]) {
    for piece in samples.chunks(480) {
        audio.feed(source, piece);
    }
}

/// Pretend the recommended speech model is on this computer.
fn model_downloaded(app_data: &Path) {
    let model = speech::catalog::catalog()
        .model(speech::catalog::ONBOARDING_MODEL)
        .unwrap();
    let path = app_data.join("speech/models").join(&model.file.filename);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"GGUF").unwrap();
}

/// The task files Tasks wrote, by name.
fn tasks(root: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(root.join("Tasks"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The provider: the Summary and the action items for Transcription, and
/// for the Orb a call to `tasks__create`, then a closing answer once the
/// task exists.
fn provider() -> Arc<Scripted> {
    Scripted::new(|request| {
        if orbs::is_orb(request) {
            if orbs::has_result(request) {
                Reply::Text("I created the task for Ada.".to_owned())
            } else {
                Reply::Call(
                    "tasks__create".to_owned(),
                    serde_json::json!({ "title": "Ada sends the deck" }),
                )
            }
        } else if request
            .system
            .iter()
            .any(|text| text.contains("Answer with one JSON value"))
        {
            Reply::Text(r#"{"action_items":[],"project":null}"#.to_owned())
        } else {
            Reply::Text(SUMMARY.to_owned())
        }
    })
}

fn launch(fixture: &Fixture, system: Arc<FakeSystem>) -> App {
    let mut app = App::launch_with(fixture.app_data.path(), system);
    support::settle(&mut app);
    app
}

#[test]
fn a_finished_transcription_starts_the_meetings_orb_which_creates_tasks_without_asking() {
    let files = orbs::meetings(
        &["tasks__create"],
        &["event transcription.finished"],
        "2026-11-01T10:00:00Z",
    );
    let files: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    let fixture = Fixture::new(&files);
    model_downloaded(fixture.app_data.path());
    let system = FakeSystem::new();
    system.set_clock("2026-11-16T14:00:00Z", "America/Toronto");
    let audio = system.audio.clone();
    let mut app = launch(&fixture, system);
    let provider = provider();
    app.set_ai_client(provider.clone());
    app.save_ai_provider(support::ai::provider("Local", "ollama", "llama3", None))
        .unwrap();
    let Some(_tasks) = support::tasks::with_tasks(&mut app) else {
        return;
    };
    let Some(transcription) = plugins::built("transcription_plugin")
        .map(|component| PluginFolder::new(MANIFEST, "transcription_plugin.wasm", &component))
    else {
        return;
    };
    plugins::install_and_enable(&mut app, transcription.path(), "transcription");
    wait_until(&mut app, "the menu shows Start Transcription", |app| {
        app.tray_menu()
            .iter()
            .any(|items| items.plugin == "transcription")
    });
    app.send_plugin_settings_event(
        "plugin-settings:transcription",
        &PluginEvent::Changed(Field {
            node: "system-audio".to_owned(),
            value: InputValue::Checkbox(true),
        }),
    );
    support::settle(&mut app);

    // A call: you speak, someone answers, and the menu bar stops it.
    app.press_shortcut("transcription", "transcription", true, Instant::now());
    app.press_shortcut("transcription", "transcription", false, Instant::now());
    let note = fixture.root.join(NOTE);
    wait_until(&mut app, "the Transcription records", |_| {
        note.exists() && audio.open_count() == 2
    });
    let folder = fixture.outside();
    let me = recorded(&folder.join("me.wav"), &[(0, 1500), (1500, 0)]);
    let others = recorded(&folder.join("others.wav"), &[(2000, 1500), (1500, 0)]);
    feed(&audio, Source::Microphone, &me);
    feed(&audio, Source::SystemAudio, &others);
    app.choose_menu_item("transcription", "stop");
    wait_until(&mut app, "the Summary is written", |_| {
        std::fs::read_to_string(&note).is_ok_and(|text| text.contains("## Summary"))
    });
    let transcription_id = support::ai::frontmatter(&std::fs::read_to_string(&note).unwrap(), "id")
        .expect("the Transcription has an ID");

    // The event starts the Orb, which creates the task without a prompt.
    let deadline = Instant::now() + std::time::Duration::from_secs(120);
    loop {
        app.check_orbs();
        support::settle(&mut app);
        assert!(
            app.ai_tool_prompt().is_none(),
            "an Orb never asks about a call"
        );
        let runs = orbs::runs(&app, MEETINGS);
        if runs.iter().any(|run| run.status != AiOrbRunStatus::Running) {
            break;
        }
        assert!(Instant::now() < deadline, "the Orb's run never ended");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert_eq!(tasks(&fixture.root), ["Ada sends the deck.md"]);

    // The run's record says what started it and what it did.
    let runs = orbs::runs(&app, MEETINGS);
    assert_eq!(runs.len(), 1, "{runs:?}");
    let run = &runs[0];
    assert_eq!(run.status, AiOrbRunStatus::Succeeded, "{run:?}");
    assert_eq!(run.started_by, AiOrbStart::Event);
    assert_eq!(run.trigger.as_deref(), Some("event transcription.finished"));
    assert_eq!(run.tool_calls, 1);
    assert!(!run.quiet);
    let records = orbs::run_files(&fixture.root);
    assert_eq!(records.len(), 1, "{records:?}");
    let (path, record) = &records[0];
    assert!(
        path.starts_with("Orbs/Meetings/Runs/2026-11-16/orb-run_"),
        "{path}"
    );
    assert_eq!(
        support::ai::frontmatter(record, "object_id").as_deref(),
        Some(transcription_id.as_str())
    );
    let key = support::ai::frontmatter(record, "trigger_key").unwrap();
    assert!(
        key.starts_with(&format!(
            "{MEETINGS}|event transcription.finished|{transcription_id}@"
        )),
        "{key}"
    );
    assert!(
        record.ends_with("---\n- `tasks__create` · completed\n"),
        "{record}"
    );

    // The Orb's request carried its instructions, its memory and the
    // Transcription's Summary, and offered only the tools it lists.
    let asked = provider.orb_requests();
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert_eq!(asked[0].tools, ["tasks__create"]);
    let system_text = &asked[0].system[0];
    assert!(
        system_text.starts_with("You are Meetings, an Orb"),
        "{system_text}"
    );
    assert!(system_text.contains("Create a task for each commitment."));
    assert!(system_text.contains("- Ada owns the deck."));
    let opening = &asked[0].messages[0];
    assert!(
        opening.starts_with("user: **Transcription ends** · [[Transcription 2026-11-16]]"),
        "{opening}"
    );
    assert!(
        opening.contains("Ada sends the deck on Friday."),
        "{opening}"
    );

    // The thread is the Orb's, and holds the event, the call, its result
    // and the answer.
    let orb = orbs::orb(&app, MEETINGS);
    let thread = orb.thread.clone().expect("the Orb has a thread");
    let chats = support::ai::chat_files(&fixture.root);
    let chat = chats
        .iter()
        .find(|(path, _)| path.ends_with("/chat.md"))
        .map(|(_, text)| text.clone())
        .unwrap();
    assert_eq!(
        support::ai::frontmatter(&chat, "orb_id").as_deref(),
        Some(MEETINGS)
    );
    assert_eq!(
        support::ai::frontmatter(&chat, "id").as_deref(),
        Some(thread.as_str())
    );
    assert_eq!(
        support::ai::frontmatter(&chat, "title").as_deref(),
        Some("Meetings")
    );
    let mut kinds: Vec<String> = support::ai::message_files(&fixture.root)
        .iter()
        .filter(|text| support::ai::frontmatter(text, "run_id").as_deref() == Some(run.id.as_str()))
        .map(|text| support::ai::frontmatter(text, "kind").unwrap())
        .collect();
    kinds.sort();
    assert_eq!(
        kinds,
        ["assistant", "assistant", "tool-call", "tool-result", "user"]
    );

    // The same Transcription starts no second run.
    app.check_orbs();
    support::settle(&mut app);
    assert_eq!(orbs::runs(&app, MEETINGS).len(), 1);
    assert_eq!(tasks(&fixture.root).len(), 1);
}
