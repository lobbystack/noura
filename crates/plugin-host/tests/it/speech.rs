//! Seam 3: the speech, global-shortcuts, paste and menu-bar interfaces in
//! the real host (ADR 0006). Each call reaches its service only when the
//! plugin holds the capability, and the plugin gets kernel events only
//! for the capabilities it declares. Every call returns at once: the
//! services stand in for the kernel's own threads.

use crate::support;
use std::sync::{Arc, Mutex};

use plugin_host::{
    KernelEvent, Limits, MenuBarEntry, MenuBarService, MicrophoneUse, PasteMethod, PasteService,
    Recording, RecordingOptions, RecordingState, RunningPlugin, ServiceError, Services,
    ShortcutService, ShortcutState, SpeechChannel, SpeechMicrophone, SpeechModel,
    SpeechPreferences, SpeechSegment, SpeechService, WorkspacePath,
};
use support::plugins;

/// Records every call that reaches a service, in order.
#[derive(Default)]
struct Calls(Mutex<Vec<String>>);

impl Calls {
    fn push(&self, call: impl Into<String>) {
        self.0.lock().unwrap().push(call.into());
    }

    fn take(&self) -> Vec<String> {
        std::mem::take(&mut self.0.lock().unwrap())
    }
}

/// A speech service with one model, one microphone and one recording.
struct FakeSpeech(Arc<Calls>);

impl SpeechService for FakeSpeech {
    fn models(&self) -> Result<Vec<SpeechModel>, ServiceError> {
        self.0.push("models");
        Ok(vec![SpeechModel {
            id: "parakeet-ultra".into(),
            name: "Parakeet Ultra".into(),
            description: "Fast and accurate".into(),
            languages: vec!["en".into(), "fr".into()],
            size: 650_000_000,
            streaming: false,
            translation: false,
            downloaded: true,
            recommended: true,
        }])
    }

    fn microphones(&self) -> Result<Vec<SpeechMicrophone>, ServiceError> {
        self.0.push("microphones");
        Ok(vec![SpeechMicrophone {
            id: "built-in".into(),
            name: "MacBook Microphone".into(),
            is_default: true,
        }])
    }

    fn preferences(&self) -> Result<SpeechPreferences, ServiceError> {
        self.0.push("preferences");
        Ok(SpeechPreferences {
            model: None,
            microphone: None,
            system_audio: false,
            language: None,
        })
    }

    fn set_preferences(&self, preferences: SpeechPreferences) -> Result<(), ServiceError> {
        self.0
            .push(format!("set-preferences {:?}", preferences.model));
        Ok(())
    }

    fn start(&self, options: RecordingOptions) -> Result<String, ServiceError> {
        self.0.push(format!("start {:?}", options.sources));
        Ok("recording_1".into())
    }

    fn read_recording(&self, id: &str) -> Result<Recording, ServiceError> {
        self.0.push(format!("read {id}"));
        if id != "recording_1" {
            return Err(ServiceError::NotFound(id.to_owned()));
        }
        Ok(Recording {
            id: id.to_owned(),
            state: RecordingState::Recording,
            elapsed_ms: 3_200,
            segments: vec![SpeechSegment {
                channel: SpeechChannel::Me,
                start_ms: 0,
                end_ms: 1_500,
                text: "Hello".into(),
            }],
            pending: vec![SpeechSegment {
                channel: SpeechChannel::Others,
                start_ms: 1_500,
                end_ms: 3_200,
                text: "Hi there".into(),
            }],
            failure: None,
        })
    }

    fn stop(&self, id: &str) -> Result<(), ServiceError> {
        self.0.push(format!("stop {id}"));
        Ok(())
    }

    fn cancel(&self, id: &str) -> Result<(), ServiceError> {
        self.0.push(format!("cancel {id}"));
        Ok(())
    }

    fn save_audio(&self, id: &str, object: &str) -> Result<WorkspacePath, ServiceError> {
        self.0.push(format!("save-audio {id} {object}"));
        WorkspacePath::new("Meetings/Standup/Audio.wav")
    }

    fn keep_audio(&self, id: &str, days: u32) -> Result<(), ServiceError> {
        self.0.push(format!("keep-audio {id} {days}"));
        Ok(())
    }

    fn forget(&self, id: &str) -> Result<(), ServiceError> {
        self.0.push(format!("forget {id}"));
        Ok(())
    }

    fn remember_dictation(&self, text: &str) -> Result<(), ServiceError> {
        self.0.push(format!("remember {text}"));
        Ok(())
    }

    fn last_dictation(&self) -> Result<Option<String>, ServiceError> {
        self.0.push("last-dictation");
        Ok(Some("Buy milk".into()))
    }
}

struct FakeShortcuts(Arc<Calls>);

impl ShortcutService for FakeShortcuts {
    fn shortcuts(&self) -> Result<Vec<ShortcutState>, ServiceError> {
        self.0.push("shortcuts");
        Ok(vec![ShortcutState {
            id: "paste".into(),
            title: "Dictation".into(),
            keys: "Alt+Space".into(),
            registered: true,
        }])
    }
}

struct FakePaste(Arc<Calls>);

impl PasteService for FakePaste {
    fn paste(&self, text: &str, method: PasteMethod) -> Result<(), ServiceError> {
        self.0.push(format!("paste {method:?} {text}"));
        Ok(())
    }

    fn copy(&self, text: &str) -> Result<(), ServiceError> {
        self.0.push(format!("copy {text}"));
        Ok(())
    }
}

struct FakeMenu(Arc<Calls>);

impl MenuBarService for FakeMenu {
    fn set_entries(&self, entries: Vec<MenuBarEntry>) -> Result<(), ServiceError> {
        let labels: Vec<String> = entries.iter().map(|entry| entry.label.clone()).collect();
        self.0.push(format!("menu {}", labels.join(",")));
        Ok(())
    }
}

/// Every new service, sharing one record of calls.
fn services(calls: &Arc<Calls>) -> Services {
    Services {
        speech: Some(Arc::new(FakeSpeech(calls.clone()))),
        shortcuts: Some(Arc::new(FakeShortcuts(calls.clone()))),
        paste: Some(Arc::new(FakePaste(calls.clone()))),
        menu_bar: Some(Arc::new(FakeMenu(calls.clone()))),
        ..Services::default()
    }
}

const SHORTCUTS: &str = "name = \"global-shortcuts\"\n\
     shortcuts = [{ id = \"paste\", title = \"Dictation\", keys = \"Alt+Space\" }]";

fn probe(
    capabilities: &[&str],
    calls: &Arc<Calls>,
) -> Option<(Arc<RunningPlugin>, Vec<tempfile::TempDir>)> {
    let component = plugins::built("probe_plugin")?;
    let (plugin, folders) = plugins::start(
        "probe",
        &component,
        capabilities,
        services(calls),
        Limits::default(),
    );
    Some((plugin, folders.into()))
}

fn lines(plugin: &RunningPlugin, page: &str) -> Vec<String> {
    plugins::texts(&plugin.render(page).expect("the probe renders"))
}

#[test]
fn without_their_capabilities_the_calls_reach_no_service() {
    let calls = Arc::new(Calls::default());
    let Some((plugin, _folders)) = probe(&["name = \"views\""], &calls) else {
        return;
    };

    assert_eq!(
        lines(&plugin, "speech:models"),
        ["error capability-not-declared speech"]
    );
    assert_eq!(
        lines(&plugin, "speech:start"),
        ["error capability-not-declared speech"]
    );
    assert_eq!(
        lines(&plugin, "speech:last-dictation"),
        ["error capability-not-declared speech"]
    );
    assert_eq!(
        lines(&plugin, "shortcuts"),
        ["error capability-not-declared global-shortcuts"]
    );
    assert_eq!(
        lines(&plugin, "paste:hello"),
        ["error capability-not-declared paste"]
    );
    assert_eq!(
        lines(&plugin, "copy:hello"),
        ["error capability-not-declared paste"]
    );
    assert_eq!(
        lines(&plugin, "menu-bar:Model,Parakeet"),
        ["error capability-not-declared menu-bar-items"]
    );
    assert_eq!(
        lines(&plugin, "notify"),
        ["error capability-not-declared notifications"]
    );
    assert_eq!(calls.take(), Vec::<String>::new());

    // Kernel events need their capabilities too.
    for event in [
        KernelEvent::ShortcutPressed(shortcut("paste", 0)),
        KernelEvent::MenuItemChosen("item-0".into()),
        KernelEvent::NotificationChosen(plugin_host::NotificationChoice {
            id: "hello".into(),
            action: None,
        }),
        KernelEvent::MicrophoneFree,
    ] {
        assert!(matches!(
            plugin.handle_kernel_event(&event),
            Err(plugin_host::PluginError::CapabilityNotDeclared { .. })
        ));
    }
    assert!(lines(&plugin, "events").is_empty());
}

#[test]
fn speech_calls_reach_the_kernel_and_return_at_once() {
    let calls = Arc::new(Calls::default());
    let Some((plugin, _folders)) = probe(&["name = \"views\"", "name = \"speech\""], &calls) else {
        return;
    };

    assert_eq!(
        lines(&plugin, "speech:models"),
        ["model parakeet-ultra Parakeet Ultra downloaded=true streaming=false"]
    );
    assert_eq!(
        lines(&plugin, "speech:microphones"),
        ["microphone built-in MacBook Microphone"]
    );
    assert_eq!(
        lines(&plugin, "speech:preferences"),
        ["preferences None None false None"]
    );
    assert_eq!(
        lines(&plugin, "speech:set-preferences:parakeet-ultra"),
        ["preferences set"]
    );
    assert_eq!(lines(&plugin, "speech:start"), ["started recording_1"]);
    assert_eq!(
        lines(&plugin, "speech:read:recording_1"),
        [
            "recording recording_1 RecordingState::Recording 3200",
            "segment Channel::Me 0 1500 Hello",
            "segment Channel::Others 1500 3200 Hi there",
        ]
    );
    assert_eq!(
        lines(&plugin, "speech:read:recording_9"),
        ["error not-found recording_9"]
    );
    assert_eq!(lines(&plugin, "speech:stop:recording_1"), ["stopped"]);
    assert_eq!(lines(&plugin, "speech:keep-audio:recording_1"), ["kept"]);
    assert_eq!(lines(&plugin, "speech:forget:recording_1"), ["forgotten"]);
    assert_eq!(lines(&plugin, "speech:remember:Buy milk"), ["remembered"]);
    assert_eq!(
        lines(&plugin, "speech:last-dictation"),
        ["last Some(\"Buy milk\")"]
    );
    // Audio goes beside the plugin's own objects only, so without
    // `object-types` the call reaches nothing.
    assert_eq!(
        lines(
            &plugin,
            "speech:save-audio:recording_1:transcription_01jbcdefghjkmnpqrstvwxyz0a"
        ),
        ["error capability-not-declared object-types"]
    );
    assert_eq!(
        calls.take(),
        [
            "models",
            "microphones",
            "preferences",
            "set-preferences Some(\"parakeet-ultra\")",
            "start [Source::Microphone, Source::SystemAudio]",
            "read recording_1",
            "read recording_9",
            "stop recording_1",
            "keep-audio recording_1 7",
            "forget recording_1",
            "remember Buy milk",
            "last-dictation",
        ]
    );

    // The microphone and recordings reach the plugin as kernel events.
    plugin
        .handle_kernel_event(&KernelEvent::MicrophoneInUse(MicrophoneUse {
            app: Some("Zoom".into()),
        }))
        .unwrap();
    plugin
        .handle_kernel_event(&KernelEvent::RecordingChanged("recording_1".into()))
        .unwrap();
    assert_eq!(
        lines(&plugin, "events"),
        [
            "kernel KernelEvent::MicrophoneInUse(MicrophoneUse { app: Some(\"Zoom\") })",
            "kernel KernelEvent::RecordingChanged(\"recording_1\")",
        ]
    );
}

#[test]
fn a_shortcut_press_counts_as_something_you_did_and_may_paste() {
    let calls = Arc::new(Calls::default());
    let Some((plugin, _folders)) = probe(
        &[
            "name = \"views\"",
            SHORTCUTS,
            "name = \"paste\"",
            "name = \"menu-bar-items\"",
        ],
        &calls,
    ) else {
        return;
    };

    assert_eq!(
        lines(&plugin, "shortcuts"),
        ["shortcut paste Alt+Space true"]
    );
    plugin
        .handle_kernel_event(&KernelEvent::ShortcutPressed(shortcut("paste", 10)))
        .unwrap();
    plugin
        .handle_kernel_event(&KernelEvent::ShortcutReleased(shortcut("paste", 400)))
        .unwrap();
    assert_eq!(lines(&plugin, "copy:Buy milk"), ["pasted"]);
    assert_eq!(
        lines(&plugin, "menu-bar:Model,Parakeet Ultra,Whisper Small"),
        ["menu 3"]
    );
    plugin
        .handle_kernel_event(&KernelEvent::MenuItemChosen("item-0".into()))
        .unwrap();

    assert_eq!(
        lines(&plugin, "events"),
        [
            "kernel KernelEvent::ShortcutPressed(ShortcutEvent { id: \"paste\", at-ms: 10 })",
            "kernel paste pasted",
            "kernel KernelEvent::ShortcutReleased(ShortcutEvent { id: \"paste\", at-ms: 400 })",
            "kernel KernelEvent::MenuItemChosen(\"item-0\")",
        ]
    );
    assert_eq!(
        calls.take(),
        [
            "shortcuts",
            "paste Method::Clipboard pasted",
            "copy Buy milk",
            "menu Model,Parakeet Ultra,Whisper Small",
        ]
    );
}

#[test]
fn the_manifest_declares_shortcuts_with_keys_and_refuses_bad_ones() {
    let manifest = |capability: &str| {
        plugin_host::PluginManifest::parse(&plugins::manifest(
            "dictation",
            "plugin.wasm",
            &[capability],
        ))
    };
    let parsed = manifest(SHORTCUTS).unwrap();
    assert_eq!(
        plugin_host::Capability::shortcuts(&parsed.capabilities),
        [plugin_host::ShortcutDeclaration {
            id: "paste".into(),
            title: "Dictation".into(),
            keys: "Alt+Space".into(),
            macos_keys: None,
            windows_keys: None,
            linux_keys: None,
        }]
    );
    // Keys for one system replace `keys` there.
    let parsed = manifest(
        "name = \"global-shortcuts\"\nshortcuts = [{ id = \"p\", title = \"D\", keys = \"Ctrl+Space\", \
         macos-keys = \"Alt+Space\", linux-keys = \"Super+Space\" }]",
    );
    assert!(parsed.is_err(), "Super isn't a modifier");
    let parsed = manifest(
        "name = \"global-shortcuts\"\nshortcuts = [{ id = \"p\", title = \"D\", keys = \"Ctrl+Space\", \
         macos-keys = \"Alt+Space\" }]",
    )
    .unwrap();
    let declared = &plugin_host::Capability::shortcuts(&parsed.capabilities)[0];
    let here = if cfg!(target_os = "macos") {
        "Alt+Space"
    } else {
        "Ctrl+Space"
    };
    assert_eq!(declared.keys_here(), here);
    for bad in [
        "name = \"global-shortcuts\"",
        "name = \"global-shortcuts\"\nshortcuts = []",
        "name = \"global-shortcuts\"\nshortcuts = [{ id = \"Paste\", title = \"D\", keys = \"Alt+Space\" }]",
        "name = \"global-shortcuts\"\nshortcuts = [{ id = \"p\", title = \"D\", keys = \"Hyper+Space\" }]",
        "name = \"global-shortcuts\"\nshortcuts = [{ id = \"p\", title = \"D\", keys = \"Alt+Alt+Space\" }]",
        "name = \"global-shortcuts\"\nshortcuts = [{ id = \"p\", title = \"D\", keys = \"Alt+Space\" }, { id = \"p\", title = \"E\", keys = \"F5\" }]",
        "name = \"speech\"\nshortcuts = [{ id = \"p\", title = \"D\", keys = \"F5\" }]",
    ] {
        assert!(manifest(bad).is_err(), "{bad}");
    }
    for good in ["Ctrl+Shift+D", "F24", "Cmd+Alt+Space", "Space"] {
        assert!(plugin_host::valid_keys(good), "{good}");
    }
    for bad in ["", "Ctrl+", "ctrl+d", "F25", "Ctrl+DD"] {
        assert!(!plugin_host::valid_keys(bad), "{bad}");
    }
    // An update that adds a shortcut waits for your approval.
    let more = manifest(
        "name = \"global-shortcuts\"\nshortcuts = [{ id = \"paste\", title = \"Dictation\", keys = \"Alt+Space\" }, { id = \"copy\", title = \"Copy\", keys = \"F5\" }]",
    )
    .unwrap();
    assert_eq!(
        plugin_host::Capability::added(&parsed.capabilities, &more.capabilities).len(),
        1
    );
    assert!(plugin_host::Capability::added(&parsed.capabilities, &parsed.capabilities).is_empty());
}

#[test]
fn menu_entries_must_make_a_menu() {
    let calls = Arc::new(Calls::default());
    let Some((plugin, _folders)) =
        probe(&["name = \"views\"", "name = \"menu-bar-items\""], &calls)
    else {
        return;
    };
    // A submenu with an empty label refuses the whole list.
    assert_eq!(
        lines(&plugin, "menu-bar:,Parakeet"),
        ["error failed The menu entry menu needs a label."]
    );
    assert_eq!(calls.take(), Vec::<String>::new());
}

/// The shortcut `id`'s keys at `at_ms`.
fn shortcut(id: &str, at_ms: u64) -> plugin_host::ShortcutEvent {
    plugin_host::ShortcutEvent {
        id: id.into(),
        at_ms,
    }
}

#[test]
fn keys_read_as_macos_menus_write_them_on_macos_and_as_written_elsewhere() {
    use plugin_host::{keys_from_label, keys_label_on};

    for (keys, macos) in [
        ("Alt+Space", "\u{2325}Space"),
        ("Shift+Cmd+D", "\u{21e7}\u{2318}D"),
        (
            "Ctrl+Alt+Shift+Cmd+F5",
            "\u{2303}\u{2325}\u{21e7}\u{2318}F5",
        ),
        ("Space", "Space"),
    ] {
        assert_eq!(keys_label_on(keys, true), macos);
        assert_eq!(keys_label_on(keys, false), keys);
        // What a label shows reads back as the keys it came from.
        assert_eq!(keys_from_label(macos).as_deref(), Some(keys), "{macos}");
        assert_eq!(keys_from_label(keys).as_deref(), Some(keys), "{keys}");
    }
    // Keys that aren't valid show as written.
    assert_eq!(keys_label_on("Hyper+Space", true), "Hyper+Space");

    for (typed, keys) in [
        ("Option+Space", "Alt+Space"),
        ("Command+Control+D", "Ctrl+Cmd+D"),
        ("\u{2325}+Space", "Alt+Space"),
        (" Alt+Ctrl+D ", "Ctrl+Alt+D"),
    ] {
        assert_eq!(keys_from_label(typed).as_deref(), Some(keys), "{typed}");
    }
    for bad in [
        "",
        "\u{2325}",
        "\u{2325}\u{2325}Space",
        "Alt+\u{2325}Space",
        "ctrl+d",
        "Hyper+D",
    ] {
        assert_eq!(keys_from_label(bad), None, "{bad}");
    }
}
