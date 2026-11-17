//! Dictation against a fake kernel: shortcut events in each activation
//! mode, a fake engine's recordings, a paste that writes down what it
//! pasted, and a settings file in memory.

use std::cell::{Cell, RefCell};

use plugin_sdk::{
    Event, Field, Node, NodeKind, Submission, Value,
    menu_bar::Entry,
    paste::Method,
    speech::{Channel, Model, Preferences, Recording, RecordingState, Segment},
};

use super::{Dictation, SHORTCUT};
use crate::{
    activation::Mode,
    host::{Host, Stored, WriteError},
    menu::tests::{model, preferences},
    settings::{PasteMethod, Settings},
};

/// The kernel, in memory. A recording finishes with the text the fake
/// engine heard once the plugin stops it.
#[derive(Default)]
struct Fake {
    /// What the engine hears in the next recording, one segment each.
    heard: RefCell<Vec<String>>,
    recordings: RefCell<Vec<Recording>>,
    next: Cell<u32>,
    /// Why the next start fails.
    refuse: RefCell<Option<String>>,
    /// Every call that reaches outside the plugin, in order.
    calls: RefCell<Vec<String>>,
    last: RefCell<Option<String>>,
    models: Vec<Model>,
    preferences: RefCell<Option<Preferences>>,
    menu: RefCell<Vec<Entry>>,
    menu_sets: Cell<u32>,
    settings: RefCell<Option<Stored>>,
    /// Writes that find the file changed, as after an edit elsewhere.
    conflicts: Cell<u32>,
}

impl Fake {
    fn new() -> Self {
        Self {
            models: vec![model(
                "parakeet-ultra",
                "Parakeet Ultra",
                &["en", "fr"],
                true,
            )],
            preferences: RefCell::new(Some(preferences(None, None))),
            ..Self::default()
        }
    }

    fn with_settings(settings: Settings) -> Self {
        let fake = Self::new();
        *fake.settings.borrow_mut() = Some(Stored {
            id: "dictation-settings_01".into(),
            revision: "r1".into(),
            settings,
        });
        fake
    }

    fn hears(&self, segments: &[&str]) {
        *self.heard.borrow_mut() = segments.iter().map(|text| (*text).to_owned()).collect();
    }

    fn calls(&self) -> Vec<String> {
        self.calls.borrow_mut().drain(..).collect()
    }

    fn push(&self, call: String) {
        self.calls.borrow_mut().push(call);
    }

    fn state(&self, id: &str) -> Option<RecordingState> {
        self.recordings
            .borrow()
            .iter()
            .find(|recording| recording.id == id)
            .map(|recording| recording.state)
    }

    /// The engine finishes the recording `id`, as Noura does on its own
    /// threads after `stop`.
    fn finish(&self, id: &str, state: RecordingState) {
        let heard: Vec<String> = self.heard.borrow_mut().drain(..).collect();
        for recording in self.recordings.borrow_mut().iter_mut() {
            if recording.id == id {
                recording.state = state;
                recording.segments = heard
                    .iter()
                    .enumerate()
                    .map(|(index, text)| Segment {
                        channel: Channel::Me,
                        start_ms: index as u64 * 1_000,
                        end_ms: index as u64 * 1_000 + 900,
                        text: text.clone(),
                    })
                    .collect();
            }
        }
    }

    fn pasted(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|call| call.starts_with("paste") || call.starts_with("copy"))
            .collect()
    }
}

impl Host for Fake {
    fn start(&self) -> Result<String, String> {
        if let Some(why) = self.refuse.borrow_mut().take() {
            self.push("start refused".into());
            return Err(why);
        }
        self.next.set(self.next.get() + 1);
        let id = format!("recording_{}", self.next.get());
        self.recordings.borrow_mut().push(Recording {
            id: id.clone(),
            state: RecordingState::Recording,
            elapsed_ms: 0,
            segments: Vec::new(),
            pending: Vec::new(),
            failure: None,
        });
        self.push(format!("start {id}"));
        Ok(id)
    }

    fn stop(&self, id: &str) -> Result<(), String> {
        self.push(format!("stop {id}"));
        for recording in self.recordings.borrow_mut().iter_mut() {
            if recording.id == id {
                recording.state = RecordingState::Finishing;
                return Ok(());
            }
        }
        Err("gone".into())
    }

    fn cancel(&self, id: &str) {
        self.push(format!("cancel {id}"));
        for recording in self.recordings.borrow_mut().iter_mut() {
            if recording.id == id {
                recording.state = RecordingState::Cancelled;
            }
        }
    }

    fn read(&self, id: &str) -> Option<Recording> {
        self.recordings
            .borrow()
            .iter()
            .find(|recording| recording.id == id)
            .cloned()
    }

    fn forget(&self, id: &str) {
        self.push(format!("forget {id}"));
        self.recordings
            .borrow_mut()
            .retain(|recording| recording.id != id);
    }

    fn paste(&self, text: &str, method: Method) {
        let how = match method {
            Method::Clipboard => "paste",
            Method::Typing => "paste by typing",
        };
        self.push(format!("{how} {text}"));
    }

    fn copy(&self, text: &str) {
        self.push(format!("copy {text}"));
    }

    fn remember(&self, text: &str) {
        *self.last.borrow_mut() = Some(text.to_owned());
    }

    fn last_dictation(&self) -> Option<String> {
        self.last.borrow().clone()
    }

    fn models(&self) -> Vec<Model> {
        self.models.clone()
    }

    fn preferences(&self) -> Option<Preferences> {
        self.preferences.borrow().clone()
    }

    fn set_preferences(&self, preferences: &Preferences) {
        *self.preferences.borrow_mut() = Some(preferences.clone());
    }

    fn set_menu(&self, entries: &[Entry]) {
        *self.menu.borrow_mut() = entries.to_vec();
        self.menu_sets.set(self.menu_sets.get() + 1);
    }

    fn notify(&self, id: &str, title: &str, body: &str, actions: &[(&str, &str)]) {
        let actions: Vec<String> = actions
            .iter()
            .map(|(id, label)| format!("{id}={label}"))
            .collect();
        self.push(format!(
            "notify {id}: {title}: {body} [{}]",
            actions.join(", ")
        ));
    }

    fn withdraw(&self, id: &str) {
        self.push(format!("withdraw {id}"));
    }

    fn open_speech_settings(&self) {
        self.push("open Settings > Speech".into());
    }

    fn read_settings(&self) -> Result<Option<Stored>, ()> {
        Ok(self.settings.borrow().clone())
    }

    fn create_settings(&self, settings: &Settings) -> Result<(), WriteError> {
        *self.settings.borrow_mut() = Some(Stored {
            id: "dictation-settings_01".into(),
            revision: "r1".into(),
            settings: settings.clone(),
        });
        Ok(())
    }

    fn update_settings(
        &self,
        id: &str,
        revision: &str,
        settings: &Settings,
    ) -> Result<(), WriteError> {
        let mut stored = self.settings.borrow_mut();
        let Some(file) = stored.as_mut().filter(|file| file.id == id) else {
            return Err(WriteError::Failed);
        };
        if self.conflicts.get() > 0 || file.revision != revision {
            self.conflicts.set(self.conflicts.get().saturating_sub(1));
            // Someone added a word in another app meanwhile.
            file.settings.words.push("Elsewhere".into());
            file.revision.push('+');
            return Err(WriteError::Changed);
        }
        file.settings = settings.clone();
        file.revision.push('+');
        Ok(())
    }
}

fn mode(activation: Mode) -> Fake {
    Fake::with_settings(Settings {
        activation,
        ..Settings::default()
    })
}

/// Press, wait `held` ms, release, then let the engine finish with what
/// it heard.
fn dictate(dictation: &mut Dictation, fake: &Fake, at: u64, held: u64) {
    dictation.shortcut(fake, true, at);
    dictation.shortcut(fake, false, at + held);
}

fn finish_last(dictation: &mut Dictation, fake: &Fake) {
    let id = format!("recording_{}", fake.next.get());
    fake.finish(&id, RecordingState::Finished);
    dictation.recording_changed(fake, &id);
}

#[test]
fn hold_or_toggle_a_hold_pastes_what_you_said_when_you_let_go() {
    let fake = Fake::new();
    let mut dictation = Dictation::new();
    fake.hears(&["Buy milk", " and eggs. "]);
    dictate(&mut dictation, &fake, 0, 1_200);
    assert_eq!(fake.calls(), ["start recording_1", "stop recording_1"]);
    finish_last(&mut dictation, &fake);
    assert_eq!(
        fake.calls(),
        ["paste Buy milk and eggs.", "forget recording_1"]
    );
    assert_eq!(fake.last_dictation().as_deref(), Some("Buy milk and eggs."));
}

#[test]
fn hold_or_toggle_a_tap_records_until_the_next_press() {
    let fake = Fake::new();
    let mut dictation = Dictation::new();
    dictate(&mut dictation, &fake, 0, 100);
    assert_eq!(fake.calls(), ["start recording_1"]);
    assert_eq!(fake.state("recording_1"), Some(RecordingState::Recording));
    fake.hears(&["Hello"]);
    dictate(&mut dictation, &fake, 5_000, 80);
    assert_eq!(fake.calls(), ["stop recording_1"]);
    finish_last(&mut dictation, &fake);
    assert_eq!(fake.pasted(), ["paste Hello"]);
}

#[test]
fn push_to_talk_stops_on_a_short_release() {
    let fake = mode(Mode::PushToTalk);
    let mut dictation = Dictation::new();
    dictate(&mut dictation, &fake, 0, 100);
    assert_eq!(fake.calls(), ["start recording_1", "stop recording_1"]);
}

#[test]
fn toggle_starts_and_stops_on_presses_only() {
    let fake = mode(Mode::Toggle);
    let mut dictation = Dictation::new();
    dictate(&mut dictation, &fake, 0, 2_000);
    assert_eq!(fake.calls(), ["start recording_1"]);
    fake.hears(&["Done"]);
    dictation.shortcut(&fake, true, 6_000);
    assert_eq!(fake.calls(), ["stop recording_1"]);
    finish_last(&mut dictation, &fake);
    assert_eq!(fake.pasted(), ["paste Done"]);
}

#[test]
fn custom_words_apply_before_the_paste() {
    let fake = Fake::with_settings(Settings {
        words: vec!["Noura".into(), "ChargeBee".into()],
        ..Settings::default()
    });
    let mut dictation = Dictation::new();
    fake.hears(&["Ask noora about Charge B."]);
    dictate(&mut dictation, &fake, 0, 1_000);
    finish_last(&mut dictation, &fake);
    assert_eq!(fake.pasted(), ["paste Ask Noura about ChargeBee."]);
}

#[test]
fn the_paste_method_comes_from_the_settings_file() {
    let fake = Fake::with_settings(Settings {
        paste: PasteMethod::Typing,
        ..Settings::default()
    });
    let mut dictation = Dictation::new();
    fake.hears(&["Typed"]);
    dictate(&mut dictation, &fake, 0, 1_000);
    finish_last(&mut dictation, &fake);
    assert_eq!(fake.pasted(), ["paste by typing Typed"]);
}

#[test]
fn an_empty_result_pastes_nothing_and_keeps_the_last_dictation() {
    let fake = Fake::new();
    let mut dictation = Dictation::new();
    fake.remember("Earlier");
    let cases: [&[&str]; 3] = [&[], &["  "], &["."]];
    for (index, heard) in (1..).zip(cases) {
        fake.hears(heard);
        dictate(&mut dictation, &fake, index * 10_000, 1_000);
        finish_last(&mut dictation, &fake);
        assert!(fake.pasted().is_empty());
    }
    assert_eq!(fake.last_dictation().as_deref(), Some("Earlier"));
}

#[test]
fn a_failed_recording_pastes_nothing_but_copy_last_dictation_has_it() {
    let fake = Fake::new();
    let mut dictation = Dictation::new();
    fake.hears(&["Half a thought"]);
    dictate(&mut dictation, &fake, 0, 1_000);
    fake.calls();
    fake.finish("recording_1", RecordingState::Failed);
    dictation.recording_changed(&fake, "recording_1");
    assert_eq!(fake.calls(), ["forget recording_1"]);
    dictation.menu_chosen(&fake, "copy-last");
    assert_eq!(fake.pasted(), ["copy Half a thought"]);
}

#[test]
fn copy_last_dictation_copies_and_nothing_else_touches_the_clipboard() {
    let fake = Fake::new();
    let mut dictation = Dictation::new();
    dictation.menu_chosen(&fake, "copy-last");
    assert!(fake.calls().is_empty(), "nothing to copy yet");
    fake.hears(&["Remember this"]);
    dictate(&mut dictation, &fake, 0, 1_000);
    finish_last(&mut dictation, &fake);
    // The paste puts the clipboard back; only the menu copies.
    assert!(!fake.calls().iter().any(|call| call.starts_with("copy")));
    dictation.menu_chosen(&fake, "copy-last");
    assert_eq!(fake.calls(), ["copy Remember this"]);
}

#[test]
fn a_press_while_the_text_is_on_its_way_starts_the_next_dictation_after_the_paste() {
    let fake = mode(Mode::Toggle);
    let mut dictation = Dictation::new();
    dictation.shortcut(&fake, true, 0);
    dictation.shortcut(&fake, true, 1_000);
    dictation.shortcut(&fake, true, 1_200);
    assert_eq!(fake.calls(), ["start recording_1", "stop recording_1"]);
    fake.hears(&["First"]);
    finish_last(&mut dictation, &fake);
    assert_eq!(
        fake.calls(),
        ["paste First", "forget recording_1", "start recording_2"]
    );
}

#[test]
fn a_start_that_fails_leaves_the_shortcut_ready() {
    let fake = mode(Mode::Toggle);
    let mut dictation = Dictation::new();
    *fake.refuse.borrow_mut() = Some("Download Parakeet Ultra in Settings first.".into());
    dictation.shortcut(&fake, true, 0);
    // A notification says why, and choosing it opens Settings > Speech.
    assert_eq!(
        fake.calls(),
        [
            "start refused",
            "notify start-failed: Dictation didn't start: Download Parakeet Ultra in Settings \
             first. [open-settings=Open Settings]"
        ]
    );
    dictation.notification_chosen(&fake, "start-failed");
    assert_eq!(fake.calls(), ["open Settings > Speech"]);
    // The next start takes the notification away.
    dictation.shortcut(&fake, true, 1_000);
    assert_eq!(fake.calls(), ["start recording_1", "withdraw start-failed"]);
}

#[test]
fn other_shortcut_events_and_old_recordings_change_nothing() {
    let fake = Fake::new();
    let mut dictation = Dictation::new();
    dictation.shortcut(&fake, false, 0);
    assert!(fake.calls().is_empty());
    // A recording from before the plugin started again goes once it ends.
    fake.start().unwrap();
    fake.calls();
    dictation.recording_changed(&fake, "recording_1");
    assert!(fake.calls().is_empty());
    fake.finish("recording_1", RecordingState::Finished);
    dictation.recording_changed(&fake, "recording_1");
    assert_eq!(fake.calls(), ["forget recording_1"]);
    assert_eq!(SHORTCUT, "dictation");
}

#[test]
fn cancel_dictation_drops_the_recording() {
    let fake = mode(Mode::Toggle);
    let mut dictation = Dictation::new();
    dictation.shortcut(&fake, true, 0);
    dictation.refresh_menu(&fake);
    assert_eq!(fake.menu.borrow()[0].label, "Cancel Dictation");
    dictation.menu_chosen(&fake, "cancel");
    assert_eq!(
        fake.calls(),
        [
            "start recording_1",
            "cancel recording_1",
            "forget recording_1"
        ]
    );
    dictation.refresh_menu(&fake);
    assert_eq!(fake.menu.borrow()[0].label, "Copy Last Dictation");
    // The next press starts afresh.
    dictation.shortcut(&fake, true, 2_000);
    assert_eq!(fake.calls(), ["start recording_2"]);
}

#[test]
fn the_menu_picks_the_model_and_the_language() {
    let fake = Fake::new();
    let mut dictation = Dictation::new();
    dictation.menu_chosen(&fake, "model:parakeet-ultra");
    dictation.menu_chosen(&fake, "language:fr");
    let chosen = fake.preferences().unwrap();
    assert_eq!(chosen.model.as_deref(), Some("parakeet-ultra"));
    assert_eq!(chosen.language.as_deref(), Some("fr"));
    dictation.menu_chosen(&fake, "language:default");
    assert_eq!(fake.preferences().unwrap().language, None);
}

#[test]
fn the_menu_is_set_only_when_it_changes() {
    let fake = Fake::new();
    let mut dictation = Dictation::new();
    dictation.refresh_menu(&fake);
    dictation.refresh_menu(&fake);
    assert_eq!(fake.menu_sets.get(), 1);
    assert!(!fake.menu.borrow()[0].enabled, "no last dictation yet");
    fake.remember("Now there is one");
    dictation.refresh_menu(&fake);
    assert_eq!(fake.menu_sets.get(), 2);
    assert!(fake.menu.borrow()[0].enabled);
}

fn labels(nodes: &[Node]) -> Vec<String> {
    nodes
        .iter()
        .filter_map(|node| match &node.kind {
            NodeKind::Input(input) => Some(input.label.clone()),
            NodeKind::Item(item) => Some(item.label.clone()),
            NodeKind::Heading(heading) => Some(heading.text.clone()),
            NodeKind::Form(form) => Some(form.submit.clone()),
            NodeKind::Text(text) => Some(text.clone()),
            NodeKind::MenuItem(item) => Some(item.label.clone()),
            _ => None,
        })
        .collect()
}

fn add(dictation: &mut Dictation, fake: &Fake, word: &str) {
    dictation.settings_event(
        fake,
        Event::Submitted(Submission {
            node: "add-word".into(),
            fields: vec![Field {
                node: "word".into(),
                value: Value::Text(word.into()),
            }],
        }),
    );
}

#[test]
fn the_settings_page_writes_the_file_the_first_time_you_change_something() {
    let fake = Fake::new();
    let mut dictation = Dictation::new();
    assert_eq!(
        labels(&dictation.render_settings(&fake)),
        ["Shortcut", "Paste with", "Custom Words", "Add", "Word"]
    );
    assert!(fake.settings.borrow().is_none());

    add(&mut dictation, &fake, "Noura");
    dictation.settings_event(
        &fake,
        Event::Changed(Field {
            node: "activation".into(),
            value: Value::Choice(Some("toggle".into())),
        }),
    );
    dictation.settings_event(
        &fake,
        Event::Changed(Field {
            node: "paste".into(),
            value: Value::Choice(Some("typing".into())),
        }),
    );
    let stored = fake.settings.borrow().clone().unwrap().settings;
    assert_eq!(
        stored,
        Settings {
            words: vec!["Noura".into()],
            activation: Mode::Toggle,
            paste: PasteMethod::Typing,
        }
    );
    assert_eq!(
        labels(&dictation.render_settings(&fake)),
        [
            "Shortcut",
            "Paste with",
            "Custom Words",
            "Noura",
            "Remove",
            "Add",
            "Word"
        ]
    );
}

#[test]
fn a_word_comes_off_the_list_from_its_menu() {
    let fake = Fake::with_settings(Settings {
        words: vec!["Noura".into(), "ChargeBee".into()],
        ..Settings::default()
    });
    let mut dictation = Dictation::new();
    dictation.render_settings(&fake);
    dictation.settings_event(&fake, Event::Chosen("remove-1".into()));
    assert_eq!(
        fake.settings.borrow().as_ref().unwrap().settings.words,
        ["Noura"]
    );
}

#[test]
fn a_word_already_in_the_list_stays_in_the_field_with_why() {
    let fake = Fake::with_settings(Settings {
        words: vec!["Noura".into()],
        ..Settings::default()
    });
    let mut dictation = Dictation::new();
    add(&mut dictation, &fake, "Noura");
    let nodes = dictation.render_settings(&fake);
    assert!(labels(&nodes).contains(&"\u{201c}Noura\u{201d} is already in the list.".to_owned()));
    let field = nodes
        .iter()
        .find(|node| node.id == "word")
        .map(|node| match &node.kind {
            NodeKind::Input(input) => input.value.clone(),
            _ => unreachable!(),
        });
    assert!(matches!(field, Some(Value::Text(text)) if text == "Noura"));
    // A word that goes in clears the field and the message.
    add(&mut dictation, &fake, "Handy");
    let nodes = dictation.render_settings(&fake);
    assert!(!labels(&nodes).iter().any(|label| label.contains("already")));
}

#[test]
fn a_change_made_elsewhere_meanwhile_is_kept() {
    let fake = Fake::with_settings(Settings::default());
    fake.conflicts.set(1);
    let mut dictation = Dictation::new();
    add(&mut dictation, &fake, "Noura");
    assert_eq!(
        fake.settings.borrow().as_ref().unwrap().settings.words,
        ["Elsewhere", "Noura"]
    );
}

#[test]
fn a_file_that_keeps_changing_says_to_try_again() {
    let fake = Fake::with_settings(Settings::default());
    fake.conflicts.set(10);
    let mut dictation = Dictation::new();
    add(&mut dictation, &fake, "Noura");
    let nodes = dictation.render_settings(&fake);
    assert!(labels(&nodes).contains(&"Dictation couldn't save the change. Try again.".to_owned()));
}
