//! The Transcription plugin in a running app, with the fake speech
//! engine, recorded WAV audio and a fake AI provider: a recording starts
//! from the shortcut, the menu bar, the menu that shows while another
//! app uses the microphone, the command palette or the panel, and never
//! by itself. Once it stops, the Transcript holds one paragraph per
//! speaker turn, the Summary goes below your notes and replaces only its
//! own section, and each action item becomes a task through Tasks with
//! `source` linking back. The audio follows the retention setting.

mod support;

use std::{
    future::Future,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use app_core::{
    App, Field, InputValue, Platform, PluginEvent, SpeechDevices, System, TabId, ViewKind, ViewNode,
};
use local_core::{
    AiContentPart, AiProviderEventStream, AiProviderStreamEvent, AiStopReason, AiStreamer,
    ResolvedAiStreamRequest,
};
use speech::audio::{FakeAudio, FakeBackend, Source};
use speech::engine::{FakeEngine, SAMPLE_RATE};
use support::{
    FakeSystem, Fixture,
    plugins::{self, PluginFolder, wait_until},
};

const MANIFEST: &str = include_str!("../../plugins/transcription/plugin.toml");
const SETTINGS: &str = "plugin-settings:transcription";
const TITLE: &str = "Transcription 2026-11-16";
const FOLDER: &str = "Meetings/Transcription 2026-11-16";
const NOTE: &str = "Meetings/Transcription 2026-11-16/Transcription 2026-11-16.md";
const TRANSCRIPT: &str = "Meetings/Transcription 2026-11-16/Transcript.md";

const GARDEN: &str = "---\nid: project_01jbcdefghjkmnpqrstvwxyz0h\ntype: project\nstatus: active\n\
                      created: 2026-10-01T10:00:00Z\nupdated: 2026-10-01T10:00:00Z\n---\nGrow.\n";

const SUMMARY: &str = "# Decisions\n- Ship on Friday.\n\n# Next Steps\n- Ada sends the notes.";
const ITEMS: &str = r#"{"action_items":[{"title":"Send the notes","due":"2026-11-20"},
    {"title":"Book the room","due":null}],"project":"Garden"}"#;

/// A system whose speech hears the test's fake audio, through the fake
/// engine, and which checks the microphone when `watch` is set.
struct SpeechSystem {
    inner: Arc<FakeSystem>,
    audio: Arc<FakeAudio>,
    engine: Arc<FakeEngine>,
    watch: Option<Duration>,
    notifier: Arc<support::FakeNotifier>,
}

impl System for SpeechSystem {
    fn platform(&self) -> Platform {
        self.inner.platform()
    }
    fn speech_devices(&self) -> SpeechDevices {
        SpeechDevices {
            engine: self.engine.clone(),
            audio: Arc::new(FakeBackend(self.audio.clone())),
            paster: Arc::new(app_core::NoPaste),
            microphone_watch: self.watch,
            notifier: self.notifier.clone(),
            ..SpeechDevices::fake()
        }
    }
    fn has_trash(&self) -> bool {
        self.inner.has_trash()
    }
    fn trash(&self, path: &Path) -> io::Result<()> {
        self.inner.trash(path)
    }
    fn reveal(&self, path: &Path) -> io::Result<()> {
        self.inner.reveal(path)
    }
    fn open_folder(&self, folder: &Path) -> io::Result<()> {
        self.inner.open_folder(folder)
    }
    fn open_terminal(&self, folder: &Path) -> io::Result<()> {
        self.inner.open_terminal(folder)
    }
    fn copy_text(&self, text: &str) {
        self.inner.copy_text(text);
    }
    fn open_url(&self, url: &str) -> io::Result<()> {
        self.inner.open_url(url)
    }
    fn open_file(&self, path: &Path) -> io::Result<()> {
        self.inner.open_file(path)
    }
    fn now(&self) -> jiff::Timestamp {
        self.inner.now()
    }
    fn time_zone(&self) -> jiff::tz::TimeZone {
        self.inner.time_zone()
    }
    fn credentials(&self) -> Arc<dyn local_core::CredentialStore> {
        self.inner.credentials()
    }
}

/// What one request to the fake provider carried.
#[derive(Debug, Clone)]
struct Asked {
    provider: String,
    system: String,
    user: String,
}

/// A provider that answers the Summary's request with [`SUMMARY`] and the
/// action items' request with [`ITEMS`], at once.
#[derive(Default)]
struct Provider {
    asked: Mutex<Vec<Asked>>,
}

fn text_of(parts: &[AiContentPart]) -> String {
    parts
        .iter()
        .map(|part| match part {
            AiContentPart::Text { text } => text.clone(),
            other => format!("{other:?}"),
        })
        .collect()
}

impl AiStreamer for Provider {
    fn stream(
        &self,
        request: ResolvedAiStreamRequest,
    ) -> Pin<Box<dyn Future<Output = local_core::Result<AiProviderEventStream>> + Send + '_>> {
        let role = |role: &str| {
            request
                .messages
                .iter()
                .filter(|message| message.role == role)
                .map(|message| text_of(&message.content))
                .collect::<Vec<_>>()
                .join("\n")
        };
        let asked = Asked {
            provider: request.provider.id.clone(),
            system: role("system"),
            user: role("user"),
        };
        let answer = if asked.system.contains("Answer with one JSON value") {
            ITEMS
        } else {
            SUMMARY
        };
        self.asked.lock().unwrap().push(asked);
        let events = vec![
            Ok(AiProviderStreamEvent::TextDelta(answer.to_owned())),
            Ok(AiProviderStreamEvent::Completed {
                usage: None,
                stop_reason: Some(AiStopReason::Completed { raw: "stop".into() }),
                tool_calls: Vec::new(),
            }),
        ];
        Box::pin(async move {
            let stream: AiProviderEventStream = Box::pin(futures::stream::iter(events));
            Ok(stream)
        })
    }
}

struct Kit {
    fixture: Fixture,
    app: App,
    audio: Arc<FakeAudio>,
    notifier: Arc<support::FakeNotifier>,
    provider: Arc<Provider>,
    _plugins: Vec<PluginFolder>,
}

impl Kit {
    fn read(&self, path: &str) -> String {
        String::from_utf8(self.fixture.read(path)).unwrap()
    }

    /// The body of the file at `path`, after its frontmatter.
    fn body(&self, path: &str) -> String {
        let text = self.read(path);
        let end = text[4..].find("\n---\n").unwrap() + 4 + 5;
        text[end..].to_owned()
    }

    fn settings(&mut self, node: &str, value: InputValue) {
        self.app.send_plugin_settings_event(
            SETTINGS,
            &PluginEvent::Changed(Field {
                node: node.to_owned(),
                value,
            }),
        );
        support::settle(&mut self.app);
    }

    fn press_shortcut(&mut self) {
        self.app
            .press_shortcut("transcription", "transcription", true, Instant::now());
        self.app
            .press_shortcut("transcription", "transcription", false, Instant::now());
    }
}

/// The labels of the menu bar entries the plugin shows.
fn menu(app: &App) -> Vec<String> {
    {
        app.tray_menu()
            .into_iter()
            .filter(|items| items.plugin == "transcription")
            .flat_map(|items| items.entries)
            .map(|entry| entry.label)
            .collect()
    }
}

impl Kit {
    /// Feed two channels of a call, from WAV files: you speak at 0 s and
    /// at 6.5 s, the others at 3.5 s.
    fn speak(&self) {
        let folder = self.fixture.outside();
        let me = recorded(
            &folder.join("me.wav"),
            &[(0, 1500), (5000, 1000), (1500, 0)],
        );
        let others = recorded(&folder.join("others.wav"), &[(3500, 2000), (3500, 0)]);
        feed(&self.audio, Source::Microphone, &me);
        feed(&self.audio, Source::SystemAudio, &others);
    }

    /// Wait until the Transcription's file holds `needle`.
    fn wait_for(&mut self, path: &str, needle: &str) {
        let file = self.fixture.root.join(path);
        let needle = needle.to_owned();
        wait_until(
            &mut self.app,
            &format!("{path} holds {needle}"),
            move |_| std::fs::read_to_string(&file).is_ok_and(|text| text.contains(&needle)),
        );
    }
}

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

/// Pretend the recommended model is on this computer.
fn model_downloaded(app_data: &Path) {
    let model = speech::catalog::catalog()
        .model(speech::catalog::ONBOARDING_MODEL)
        .unwrap();
    let path = app_data.join("speech/models").join(&model.file.filename);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, b"GGUF").unwrap();
}

fn transcription_folder() -> Option<PluginFolder> {
    plugins::built("transcription_plugin")
        .map(|component| PluginFolder::new(MANIFEST, "transcription_plugin.wasm", &component))
}

/// The app with Tasks and Transcription on, a downloaded model, a stopped
/// clock and, when `provider`, an AI provider. `None` when the plugins
/// aren't built.
fn kit(files: &[(&str, &[u8])], provider: bool, watch: Option<Duration>) -> Option<Kit> {
    kit_with(files, provider, watch, FakeEngine::default())
}

/// [`kit`] with the speech engine `engine`.
fn kit_with(
    files: &[(&str, &[u8])],
    provider: bool,
    watch: Option<Duration>,
    engine: FakeEngine,
) -> Option<Kit> {
    let fixture = Fixture::new(files);
    model_downloaded(fixture.app_data.path());
    let inner = FakeSystem::new();
    inner.set_clock("2026-11-16T14:00:00Z", "America/Toronto");
    let audio = FakeAudio::new();
    let notifier = inner.notifier.clone();
    let system = Arc::new(SpeechSystem {
        inner,
        audio: audio.clone(),
        engine: Arc::new(engine),
        watch,
        notifier: notifier.clone(),
    });
    let mut app = App::launch_with(fixture.app_data.path(), system);
    support::settle(&mut app);
    let ai = Arc::new(Provider::default());
    app.set_ai_client(ai.clone());
    if provider {
        app.save_ai_provider(support::ai::provider("Local", "ollama", "llama3", None))
            .unwrap();
    }
    let tasks = support::tasks::with_tasks(&mut app)?;
    let transcription = transcription_folder()?;
    plugins::install_and_enable(&mut app, transcription.path(), "transcription");
    wait_until(&mut app, "the menu shows Start Transcription", |app| {
        app.tray_menu()
            .iter()
            .any(|items| items.plugin == "transcription")
    });
    Some(Kit {
        fixture,
        app,
        audio,
        notifier,
        provider: ai,
        _plugins: vec![tasks, transcription],
    })
}

fn find<'a>(node: &'a ViewNode, id: &str) -> Option<&'a ViewNode> {
    if node.id == id {
        return Some(node);
    }
    node.children.iter().find_map(|child| find(child, id))
}

fn texts(node: &ViewNode, found: &mut Vec<String>) {
    match &node.kind {
        ViewKind::Text(text) => found.push(text.clone()),
        ViewKind::Button { label } => found.push(format!("[{label}]")),
        ViewKind::Item { label, .. } => found.push(format!("- {label}")),
        ViewKind::Markdown(text) => found.push(format!("md: {text}")),
        _ => {}
    }
    for child in &node.children {
        texts(child, found);
    }
}

/// The panel's texts, buttons as `[Label]` and list items as `- Label`,
/// once it shows `node`.
fn panel(app: &mut App, tab: TabId, node: &str) -> Vec<String> {
    wait_until(app, &format!("the panel shows {node}"), |app| {
        app.plugin_panel(tab)
            .and_then(Result::ok)
            .is_some_and(|view| find(view.root(), node).is_some())
    });
    let view = app.plugin_panel(tab).unwrap().unwrap();
    let mut found = Vec::new();
    texts(view.root(), &mut found);
    found
}

/// Open the file at `path` in a tab, as a click in the file tree does.
fn open(app: &mut App, path: &str) -> TabId {
    app.open_file(path, app_core::OpenAs::Kept);
    support::settle(app);
    app.active_tab().expect("a tab is open").id()
}

/// The task files under `Tasks/`, with their text.
fn tasks(fixture: &Fixture) -> Vec<(String, String)> {
    let folder: PathBuf = fixture.root.join("Tasks");
    let mut found: Vec<(String, String)> = plugins::files_under(&folder)
        .into_iter()
        .map(|name| {
            let text = std::fs::read_to_string(folder.join(&name)).unwrap();
            (name, text)
        })
        .collect();
    found.sort();
    found
}

/// The paragraphs of a Transcript's body.
fn paragraphs(body: &str) -> Vec<String> {
    body.split("\n\n")
        .map(str::trim)
        .filter(|paragraph| !paragraph.is_empty())
        .map(str::to_owned)
        .collect()
}

#[test]
fn a_recording_becomes_a_transcript_a_summary_below_your_notes_and_tasks() {
    let Some(mut kit) = kit(
        &[("Projects/Garden/Garden.md", GARDEN.as_bytes())],
        true,
        None,
    ) else {
        return;
    };
    kit.settings("system-audio", InputValue::Checkbox(true));

    // The shortcut makes a Transcription, opens it and records.
    kit.press_shortcut();
    kit.wait_for(NOTE, "start: 2026-11-16T09:00:00-05:00");
    assert!(kit.fixture.exists(TRANSCRIPT));
    wait_until(&mut kit.app, "both channels record", |_| {
        kit.audio.open_count() == 2
    });
    assert_eq!(menu(&kit.app), ["Stop Transcription"]);
    let tab = open(&mut kit.app, NOTE);
    let live = panel(&mut kit.app, tab, "stop");
    assert!(live.contains(&"[Stop]".to_owned()), "{live:?}");
    assert!(live.contains(&"00:00:00".to_owned()), "{live:?}");

    kit.speak();
    // The live Transcript shows what was heard so far, each time the
    // panel draws. A press the plugin ignores draws it again.
    let deadline = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        kit.app.press_panel_button(tab, "draw-again");
        support::settle(&mut kit.app);
        let live = kit
            .app
            .plugin_panel(tab)
            .and_then(Result::ok)
            .and_then(|view| find(view.root(), "transcript").map(|node| node.kind.clone()));
        if matches!(&live, Some(ViewKind::Markdown(text)) if text.starts_with("**Me** · 00:00:00\n"))
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no live Transcript: {live:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    // You take notes in the editor meanwhile, here as another app would.
    let note = kit.read(NOTE);
    let notes = "Agenda\r\n- pricing  \n\n### Keep this\nno line break at the end";
    support::write(&kit.fixture.root, NOTE, format!("{note}{notes}").as_bytes());
    support::settle(&mut kit.app);

    // The menu bar stops it.
    kit.app.choose_menu_item("transcription", "stop");
    kit.wait_for(NOTE, "## Summary");

    let turns = paragraphs(&kit.body(TRANSCRIPT));
    let headers: Vec<&str> = turns
        .iter()
        .map(|turn| turn.lines().next().unwrap())
        .collect();
    assert_eq!(
        headers,
        [
            "**Me** · 00:00:00",
            "**Others** · 00:00:03",
            "**Me** · 00:00:06"
        ],
        "{turns:?}"
    );
    assert!(turns.iter().all(|turn| turn.lines().count() == 2));
    let note = kit.read(NOTE);
    assert!(note.contains("end: 2026-11-16T09:00:09-05:00"), "{note}");

    // Your notes keep every byte, and the Summary sits below them.
    let body = kit.body(NOTE);
    assert_eq!(
        body,
        format!(
            "{notes}\n\n## Summary\n\n### Decisions\n- Ship on Friday.\n\n### Next Steps\n\
             - Ada sends the notes.\n"
        )
    );

    // anarlog's prompt went out, with your notes, the Transcript and the
    // length guidance, then the request for action items as JSON.
    let asked = kit.provider.asked.lock().unwrap().clone();
    assert_eq!(asked.len(), 2, "{asked:?}");
    assert!(asked[0].system.contains("meeting summaries in English"));
    assert!(asked[0].system.contains("# Format Requirements"));
    assert!(asked[0].user.contains("# Meeting Notes\n\nAgenda"));
    assert!(asked[0].user.contains("Me: "));
    assert!(asked[0].user.contains("Others: "));
    assert!(
        asked[0]
            .user
            .contains("Summary length: the transcript contains about")
    );
    assert!(asked[1].system.contains("action_items"));
    assert!(asked[1].user.contains("# Projects\n\n- Garden"));

    // Each action item is a task, created by Tasks, with its source.
    wait_until(&mut kit.app, "the tasks exist", |_| {
        tasks(&kit.fixture).len() == 2
    });
    let made = tasks(&kit.fixture);
    assert_eq!(made[0].0, "Book the room.md");
    assert_eq!(made[1].0, "Send the notes.md");
    for (_, text) in &made {
        assert!(text.contains(&format!("source: '[[{TITLE}]]'")), "{text}");
        assert!(!text.contains("project:"), "{text}");
    }
    assert!(made[1].1.contains("due: 2026-11-20"));

    // The panel shows the Summary's status, the tasks by their source
    // and the project the AI suggests.
    let shown = panel(&mut kit.app, tab, "use-project");
    for expected in [
        "Summary added",
        "[Summarize]",
        "- Book the room",
        "- Send the notes",
        "[Add to Garden]",
    ] {
        assert!(
            shown.contains(&expected.to_owned()),
            "{expected}: {shown:?}"
        );
    }
    assert!(!shown.contains(&"[Start]".to_owned()));

    // One click puts the Transcription in Garden's folder.
    kit.app.press_panel_button(tab, "use-project");
    let moved = format!("Projects/Garden/{TITLE}/{TITLE}.md");
    let moved_transcript = format!("Projects/Garden/{TITLE}/Transcript.md");
    wait_until(&mut kit.app, "the Transcription moves to Garden", |_| {
        kit.fixture.exists(&moved) && kit.fixture.exists(&moved_transcript)
    });
    assert!(kit.read(&moved).contains("project: '[[Garden]]'"));
    assert!(!kit.fixture.exists(NOTE));

    // Summarizing again replaces only the Summary and adds no task twice.
    let before = kit.body(&moved);
    let tab = support::open_from_tree(&mut kit.app, &moved);
    panel(&mut kit.app, tab, "summarize");
    kit.app.press_panel_button(tab, "summarize");
    wait_until(&mut kit.app, "the Summary is asked for again", |_| {
        kit.provider.asked.lock().unwrap().len() == 4
    });
    support::settle(&mut kit.app);
    assert_eq!(kit.body(&moved), before);
    assert_eq!(tasks(&kit.fixture).len(), 2);
    assert!(menu(&kit.app).contains(&"Start Transcription".to_owned()));
}

#[test]
fn without_a_provider_the_transcript_stays_and_summarize_reads_your_edits() {
    let Some(mut kit) = kit(&[], false, None) else {
        return;
    };
    // The command palette's New Transcription makes one and records.
    assert!(kit.app.run_command(
        "plugin-command:transcription:new-transcription",
        app_core::CommandSource::Palette,
        std::time::Instant::now()
    ));
    kit.wait_for(NOTE, "start:");
    kit.speak();
    kit.press_shortcut();
    kit.wait_for(TRANSCRIPT, "**Me** · 00:00:00");
    // New Transcription in the palette opened it.
    let tab = kit.app.active_tab().expect("a tab is open").id();
    let shown = panel(&mut kit.app, tab, "ai-settings");
    assert!(
        shown.contains(&"Add an AI provider to summarize.".to_owned()),
        "{shown:?}"
    );
    assert!(shown.contains(&"[Summarize]".to_owned()), "{shown:?}");
    assert!(!kit.body(NOTE).contains("## Summary"));
    assert!(kit.provider.asked.lock().unwrap().is_empty());

    // You fix the Transcript, add a provider, and Summarize uses both.
    let transcript = kit.read(TRANSCRIPT);
    let edited = transcript.replacen(
        "**Me** · 00:00:00\n",
        "**Me** · 00:00:00\nThe fixed words ",
        1,
    );
    support::write(&kit.fixture.root, TRANSCRIPT, edited.as_bytes());
    kit.app
        .save_ai_provider(support::ai::provider("Local", "ollama", "llama3", None))
        .unwrap();
    support::settle(&mut kit.app);
    kit.app.press_panel_button(tab, "summarize");
    kit.wait_for(NOTE, "## Summary");
    let asked = kit.provider.asked.lock().unwrap().clone();
    assert!(
        asked[0].user.contains("Me: The fixed words"),
        "{}",
        asked[0].user
    );
    assert_eq!(
        kit.body(NOTE),
        "## Summary\n\n### Decisions\n- Ship on Friday.\n\n### Next Steps\n- Ada sends the notes.\n"
    );
    assert!(kit.read(NOTE).contains("type: transcription"));
}

#[test]
fn the_settings_pick_the_provider_the_prompt_and_the_folder_and_tasks_get_the_project() {
    let standup = "---\nid: transcription_01jbcdefghjkmnpqrstvwxyz0a\ntype: transcription\n\
                   attendees:\n- Ada\ncreated: 2026-11-16T13:00:00Z\nupdated: 2026-11-16T13:00:00Z\n\
                   ---\nAgenda.\n";
    let transcript = "---\nid: transcript_01jbcdefghjkmnpqrstvwxyz0b\ntype: transcript\n\
                      transcription: '[[Standup]]'\ncreated: 2026-11-16T13:00:00Z\n\
                      updated: 2026-11-16T13:00:00Z\n---\n**Me** · 00:00:02\nLet's ship.\n";
    let Some(mut kit) = kit(
        &[
            ("Projects/Garden/Garden.md", GARDEN.as_bytes()),
            ("Meetings/Standup/Standup.md", standup.as_bytes()),
            ("Meetings/Standup/Transcript.md", transcript.as_bytes()),
            ("Meetings/Standup/Standup.wav", b"RIFF"),
        ],
        true,
        None,
    ) else {
        return;
    };
    let other = kit
        .app
        .save_ai_provider(support::ai::provider("Other", "ollama", "qwen3", None))
        .unwrap();
    kit.settings("provider", InputValue::Choice(Some(other.clone())));
    kit.settings("folder", InputValue::Choice(Some("meetings".into())));
    kit.app.send_plugin_settings_event(
        SETTINGS,
        &PluginEvent::Submitted(app_core::Submission {
            node: "prompt-form".into(),
            fields: vec![Field {
                node: "prompt".into(),
                value: InputValue::Multiline("Write a short narrative in {{ language }}.".into()),
            }],
        }),
    );
    support::settle(&mut kit.app);
    let settings = kit.read("Meetings/Transcription settings.md");
    assert!(
        settings.contains(&format!("provider: {other}")),
        "{settings}"
    );
    assert!(settings.contains("folder: meetings"), "{settings}");
    assert!(
        settings.ends_with("---\nWrite a short narrative in {{ language }}.\n"),
        "{settings}"
    );

    // With Transcriptions kept in Meetings, a project stays a link.
    let tab = support::open_from_tree(&mut kit.app, "Meetings/Standup/Standup.md");
    panel(&mut kit.app, tab, "summarize");
    kit.app.send_panel_event(
        tab,
        &PluginEvent::Changed(Field {
            node: "project".into(),
            value: InputValue::Choice(Some("project_01jbcdefghjkmnpqrstvwxyz0h".into())),
        }),
    );
    kit.wait_for("Meetings/Standup/Standup.md", "project: '[[Garden]]'");

    kit.app.press_panel_button(tab, "summarize");
    kit.wait_for("Meetings/Standup/Standup.md", "## Summary");
    let asked = kit.provider.asked.lock().unwrap().clone();
    assert_eq!(asked[0].provider, other);
    assert!(
        asked[0]
            .system
            .contains("Write a short narrative in English.")
    );
    assert!(!asked[0].system.contains("Structure with # (h1) headings"));
    assert!(
        asked[0].user.contains("Participants:\n- Ada"),
        "{}",
        asked[0].user
    );
    assert!(
        asked[0]
            .user
            .contains("Keep the requested structure and stay under")
    );
    wait_until(&mut kit.app, "the tasks exist", |_| {
        tasks(&kit.fixture).len() == 2
    });
    for (_, text) in tasks(&kit.fixture) {
        assert!(text.contains("project: '[[Garden]]'"), "{text}");
        assert!(text.contains("source: '[[Standup]]'"), "{text}");
    }

    // With Transcriptions in their project's folder, picking one moves it.
    kit.settings("folder", InputValue::Choice(Some("project".into())));
    for choice in ["none", "project_01jbcdefghjkmnpqrstvwxyz0h"] {
        kit.app.send_panel_event(
            tab,
            &PluginEvent::Changed(Field {
                node: "project".into(),
                value: InputValue::Choice(Some(choice.into())),
            }),
        );
        support::settle(&mut kit.app);
    }
    // The whole folder moves, its audio too, and leaves nothing behind.
    wait_until(&mut kit.app, "Standup moves to Garden", |_| {
        kit.fixture.exists("Projects/Garden/Standup/Standup.md")
            && kit.fixture.exists("Projects/Garden/Standup/Transcript.md")
    });
    assert!(kit.fixture.exists("Projects/Garden/Standup/Standup.wav"));
    assert!(!kit.fixture.exists("Meetings/Standup"));
}

#[test]
fn a_microphone_in_use_offers_a_recording_and_never_starts_one() {
    let Some(mut kit) = kit(&[], false, Some(Duration::from_millis(20))) else {
        return;
    };
    assert_eq!(menu(&kit.app), ["Start Transcription"]);
    *kit.audio.in_use.lock() = Some(true);
    wait_until(
        &mut kit.app,
        "the menu offers to transcribe the call",
        |app| {
            app.tray_menu()
                .iter()
                .flat_map(|items| items.entries.iter())
                .any(|entry| entry.label == "Transcribe This Call")
        },
    );
    std::thread::sleep(Duration::from_millis(200));
    support::settle(&mut kit.app);
    assert_eq!(kit.audio.open_count(), 0, "nothing records by itself");
    assert!(!kit.fixture.exists("Meetings"));

    // Choosing it records.
    kit.app.choose_menu_item("transcription", "offer");
    kit.wait_for(NOTE, "start:");
    wait_until(&mut kit.app, "the microphone records", |_| {
        kit.audio.open_count() == 1
    });
    assert_eq!(menu(&kit.app), ["Stop Transcription"]);

    // Once the other app lets go, the offer goes too.
    *kit.audio.in_use.lock() = Some(false);
    kit.app.choose_menu_item("transcription", "stop");
    wait_until(&mut kit.app, "the menu goes back", |app| {
        menu(app) == ["Start Transcription"]
    });
}

#[test]
fn the_notification_for_a_call_records_and_opens_the_transcription() {
    let Some(mut kit) = kit(&[], false, Some(Duration::from_millis(20))) else {
        return;
    };
    *kit.audio.in_use.lock() = Some(true);
    let notifier = kit.notifier.clone();
    wait_until(&mut kit.app, "the notification shows", |_| {
        !notifier.shown().is_empty()
    });
    let shown = kit.notifier.shown();
    assert_eq!(shown[0].key, "transcription/offer");
    assert_eq!(shown[0].title, "Transcribe this call?");
    assert_eq!(
        shown[0].actions,
        [("transcribe".to_owned(), "Transcribe".to_owned())]
    );
    assert_eq!(kit.audio.open_count(), 0, "nothing records by itself");

    // Choosing it records into a new Transcription, which opens in the
    // window you used last.
    kit.notifier
        .choose("transcription/offer", Some("transcribe"));
    kit.wait_for(NOTE, "start:");
    wait_until(&mut kit.app, "the Transcription opens", |app| {
        app.active_tab()
            .and_then(|tab| app.note(tab.id()))
            .is_some_and(|note| note.relative_path() == NOTE)
    });
    assert_eq!(kit.audio.open_count(), 1);
    assert!(kit.notifier.shown().is_empty());

    *kit.audio.in_use.lock() = Some(false);
    kit.app.choose_menu_item("transcription", "stop");
    wait_until(&mut kit.app, "the menu goes back", |app| {
        menu(app) == ["Start Transcription"]
    });
}

#[test]
fn the_panel_starts_a_transcription_you_made_and_the_menu_starts_a_new_one() {
    let transcription = "---\nid: transcription_01jbcdefghjkmnpqrstvwxyz0a\ntype: transcription\n\
                         attendees: []\ncreated: 2026-11-16T13:00:00Z\nupdated: 2026-11-16T13:00:00Z\n\
                         ---\nAgenda.\n";
    let Some(mut kit) = kit(
        &[("Meetings/Standup/Standup.md", transcription.as_bytes())],
        false,
        None,
    ) else {
        return;
    };
    let tab = support::open_from_tree(&mut kit.app, "Meetings/Standup/Standup.md");
    let shown = panel(&mut kit.app, tab, "start");
    assert!(shown.contains(&"[Start]".to_owned()), "{shown:?}");
    kit.app.press_panel_button(tab, "start");
    kit.wait_for(
        "Meetings/Standup/Standup.md",
        "start: 2026-11-16T09:00:00-05:00",
    );
    panel(&mut kit.app, tab, "stop");
    // The timer moves each second with no other event: silence makes no
    // text.
    for piece in vec![0.0_f32; 2 * SAMPLE_RATE as usize].chunks(480) {
        kit.audio.feed(Source::Microphone, piece);
    }
    wait_until(&mut kit.app, "the timer moves", |app| {
        app.plugin_panel(tab)
            .and_then(Result::ok)
            .is_some_and(|view| {
                let mut found = Vec::new();
                texts(view.root(), &mut found);
                found.iter().any(|text| text.contains("00:00:02"))
            })
    });
    // A second recording waits for this one.
    kit.app.choose_menu_item("transcription", "start");
    support::settle(&mut kit.app);
    assert!(!kit.fixture.exists(NOTE));
    kit.speak();
    kit.app.press_panel_button(tab, "stop");
    // The Transcript goes beside it, made when it had none.
    kit.wait_for("Meetings/Standup/Transcript.md", "**Me** · 00:00:01");
    assert!(
        kit.read("Meetings/Standup/Transcript.md")
            .contains("transcription: '[[Standup]]'")
    );
    assert_eq!(kit.body("Meetings/Standup/Standup.md"), "Agenda.\n");

    // The menu bar's Start Transcription makes a new one.
    kit.app.choose_menu_item("transcription", "start");
    kit.wait_for(NOTE, "start:");
    assert!(kit.fixture.exists(TRANSCRIPT));
    assert!(
        kit.read(TRANSCRIPT)
            .contains(&format!("transcription: '[[{TITLE}]]'"))
    );
}

#[test]
fn the_audio_follows_the_retention_setting() {
    let Some(mut kit) = kit(&[], false, None) else {
        return;
    };
    let kept = kit.fixture.app_data.path().join("speech/kept");
    let wav = format!("{FOLDER}/{TITLE}.wav");

    // Not kept by default.
    kit.press_shortcut();
    kit.wait_for(NOTE, "start:");
    kit.speak();
    kit.press_shortcut();
    kit.wait_for(TRANSCRIPT, "**Me**");
    support::settle(&mut kit.app);
    assert!(!kit.fixture.exists(&wav));
    assert!(plugins::files_under(&kept).is_empty());

    // Kept in the Transcription's folder, where it syncs.
    kit.settings("audio", InputValue::Choice(Some("folder".into())));
    assert!(
        kit.read("Meetings/Transcription settings.md")
            .contains("audio: folder")
    );
    kit.press_shortcut();
    let second = format!("Meetings/{TITLE} 1/{TITLE} 1.md");
    kit.wait_for(&second, "start:");
    kit.speak();
    kit.press_shortcut();
    let second_wav = format!("Meetings/{TITLE} 1/{TITLE} 1.wav");
    wait_until(&mut kit.app, "the audio is in the folder", |_| {
        kit.fixture.exists(&second_wav)
    });
    assert!(kit.fixture.read(&second_wav).starts_with(b"RIFF"));

    // Kept on this device for a week.
    kit.settings("audio", InputValue::Choice(Some("device".into())));
    kit.settings("audio-days", InputValue::Choice(Some("7".into())));
    let settings = kit.read("Meetings/Transcription settings.md");
    assert!(
        settings.contains("audio: device") && settings.contains("audio_days: 7"),
        "{settings}"
    );
    kit.press_shortcut();
    let third = format!("Meetings/{TITLE} 2/{TITLE} 2.md");
    kit.wait_for(&third, "start:");
    kit.speak();
    kit.press_shortcut();
    wait_until(&mut kit.app, "the audio is kept on the device", |_| {
        !plugins::files_under(&kept).is_empty()
    });
    assert!(
        !kit.fixture
            .exists(&format!("Meetings/{TITLE} 2/{TITLE} 2.wav"))
    );
}

#[test]
fn calendar_shows_a_transcription() {
    let transcription = "---\nid: transcription_01jbcdefghjkmnpqrstvwxyz0a\ntype: transcription\n\
                         start: 2026-11-16T09:00:00-05:00\nend: 2026-11-16T09:42:00-05:00\n\
                         attendees: []\ncreated: 2026-11-16T13:00:00Z\nupdated: 2026-11-16T13:00:00Z\n\
                         ---\nAgenda.\n";
    let Some(mut kit) = kit(
        &[("Meetings/Standup/Standup.md", transcription.as_bytes())],
        false,
        None,
    ) else {
        return;
    };
    let Some(_calendar) = support::calendar::with_calendar(&mut kit.app) else {
        return;
    };
    wait_until(&mut kit.app, "Calendar shows the Transcription", |app| {
        let (_, _, shown) = support::calendar::grid(app);
        shown.iter().any(|entry| {
            entry.source()
                == (
                    "transcription_01jbcdefghjkmnpqrstvwxyz0a".to_owned(),
                    "start".to_owned(),
                )
        })
    });
}

/// The panel's problem, once it shows one.
fn problem(app: &mut App, tab: TabId) -> String {
    wait_until(app, "the panel shows a problem", |app| {
        app.plugin_panel(tab)
            .and_then(Result::ok)
            .is_some_and(|view| find(view.root(), "problem").is_some())
    });
    let view = app.plugin_panel(tab).unwrap().unwrap();
    match &find(view.root(), "problem").unwrap().kind {
        ViewKind::Text(text) => text.clone(),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_transcription_that_fails_says_why_in_the_panel() {
    let Some(mut kit) = kit_with(
        &[],
        false,
        None,
        FakeEngine {
            failing: true,
            ..FakeEngine::default()
        },
    ) else {
        return;
    };
    kit.press_shortcut();
    kit.wait_for(NOTE, "start:");
    let tab = open(&mut kit.app, NOTE);
    kit.speak();
    assert_eq!(
        problem(&mut kit.app, tab),
        "The recording stopped: Noura couldn't transcribe this audio. Try another model in Settings."
    );
    assert!(panel(&mut kit.app, tab, "start").contains(&"[Start]".to_owned()));
}

#[test]
fn a_recording_that_hears_no_speech_says_so_after_stop() {
    let Some(mut kit) = kit(&[], false, None) else {
        return;
    };
    let tab = {
        kit.press_shortcut();
        kit.wait_for(NOTE, "start:");
        open(&mut kit.app, NOTE)
    };
    panel(&mut kit.app, tab, "stop");
    feed(
        &kit.audio,
        Source::Microphone,
        &vec![0.0; SAMPLE_RATE as usize],
    );
    // Stop from the panel, whose press must keep the problem the
    // recording leaves.
    kit.app.press_panel_button(tab, "stop");
    assert_eq!(
        problem(&mut kit.app, tab),
        "Noura heard no speech. Check your microphone and try again."
    );
}
