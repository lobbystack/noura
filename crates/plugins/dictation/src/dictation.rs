//! What Dictation does with each event: the shortcut starts and stops a
//! recording, a finished recording becomes text with your custom words
//! applied and goes into the focused app, and the menu and the settings
//! page answer your choices. Everything goes through [`Host`], so the
//! tests drive it with a fake kernel.

use plugin_sdk::{
    Event, Node, Value, View,
    menu_bar::Entry,
    speech::{Channel, Recording, RecordingState},
};

use crate::{
    activation::{Activation, Effect, Mode},
    host::{Host, WriteError},
    menu,
    settings::{Change, PasteMethod, Settings},
    words,
};

/// The shortcut the manifest declares.
pub(crate) const SHORTCUT: &str = "dictation";

/// The notification that says why a recording didn't start, as when no
/// model is downloaded.
pub(crate) const START_FAILED: &str = "start-failed";
/// The notification that says why a recording you stopped didn't paste.
pub(crate) const FINISH_FAILED: &str = "finish-failed";
const OPEN_SETTINGS: &str = "open-settings";

const ACTIVATION: &str = "activation";
/// The button that opens Settings > Speech, where you record the keys.
const CHANGE_KEYS: &str = "change-keys";
const PASTE: &str = "paste";
const ADD_WORD: &str = "add-word";
const WORD: &str = "word";
const REMOVE_PREFIX: &str = "remove-";

/// What Dictation keeps between calls.
pub(crate) struct Dictation {
    activation: Activation,
    /// The recording the shortcut started, until its text is in.
    recording: Option<String>,
    /// The menu entries Noura shows now.
    menu: Option<Vec<Entry>>,
    /// The custom words as the settings page drew them, by row.
    drawn_words: Vec<String>,
    /// What you typed in the word field, kept when it couldn't be added.
    typed: String,
    /// Why the last change on the settings page didn't go in.
    problem: Option<String>,
    /// The start-failed notification may still show.
    notified: bool,
    /// The finish-failed notification may still show.
    notified_finish: bool,
}

impl Dictation {
    pub(crate) const fn new() -> Self {
        Self {
            activation: Activation::new(),
            recording: None,
            menu: None,
            drawn_words: Vec::new(),
            typed: String::new(),
            problem: None,
            notified: false,
            notified_finish: false,
        }
    }

    fn settings(host: &impl Host) -> Settings {
        host.read_settings()
            .ok()
            .flatten()
            .map(|stored| stored.settings)
            .unwrap_or_default()
    }

    /// The shortcut went down, or came up, at `at_ms` on Noura's clock.
    pub(crate) fn shortcut(&mut self, host: &impl Host, pressed: bool, at_ms: u64) {
        let mode = Self::settings(host).activation;
        let now = i64::try_from(at_ms).unwrap_or(i64::MAX);
        let effect = if pressed {
            self.activation.press(mode, now)
        } else {
            self.activation.release(mode, now)
        };
        self.run(host, effect);
    }

    fn run(&mut self, host: &impl Host, effect: Option<Effect>) {
        match effect {
            Some(Effect::Start) => match host.start() {
                Ok(id) => {
                    if std::mem::take(&mut self.notified) {
                        host.withdraw(START_FAILED);
                    }
                    if std::mem::take(&mut self.notified_finish) {
                        host.withdraw(FINISH_FAILED);
                    }
                    self.recording = Some(id);
                }
                Err(why) => {
                    // Handy shows its overlay; Noura says why in a
                    // notification, which opens Settings > Speech.
                    host.notify(
                        START_FAILED,
                        "Dictation didn't start",
                        &why,
                        &[(OPEN_SETTINGS, "Open Settings")],
                    );
                    self.notified = true;
                    self.activation.start_failed();
                }
            },
            Some(Effect::Stop) => {
                let stopped = self.recording.as_deref().map(|id| host.stop(id));
                // A recording that is gone can't finish: end it here.
                if !matches!(stopped, Some(Ok(()))) {
                    self.end(host);
                }
            }
            None => {}
        }
    }

    /// You chose the notification `id`, or its action `action`.
    pub(crate) fn notification_chosen(&mut self, host: &impl Host, id: &str) {
        if id == START_FAILED {
            host.open_speech_settings();
        }
    }

    /// The recording `id` changed state.
    pub(crate) fn recording_changed(&mut self, host: &impl Host, id: &str) {
        if self.recording.as_deref() != Some(id) {
            // Left from before the plugin started again: let it go.
            if host.read(id).is_some_and(|recording| ended(&recording)) {
                host.forget(id);
            }
            return;
        }
        let Some(recording) = host.read(id) else {
            self.end(host);
            return;
        };
        match recording.state {
            RecordingState::Recording | RecordingState::Finishing => {}
            RecordingState::Finished => {
                let settings = Self::settings(host);
                let text = words::finish(&said(&recording), &settings.words);
                if !text.is_empty() {
                    // Remembered first, so Copy Last Dictation has it
                    // when the paste fails.
                    host.remember(&text);
                    if let Err(why) = host.paste(&text, settings.paste.method()) {
                        self.finish_failed(host, "Dictation didn't paste", &why);
                    }
                }
                self.end(host);
            }
            RecordingState::Failed => {
                // What it heard before failing stays for Copy Last
                // Dictation, but goes into no app.
                let settings = Self::settings(host);
                let text = words::finish(&said(&recording), &settings.words);
                if !text.is_empty() {
                    host.remember(&text);
                }
                let why = recording
                    .failure
                    .as_deref()
                    .filter(|why| !why.trim().is_empty())
                    .unwrap_or("Noura couldn't turn your speech into text. Try again.");
                self.finish_failed(host, "Dictation didn't finish", why);
                self.end(host);
            }
            RecordingState::Cancelled => self.end(host),
        }
    }

    /// Say why the recording you stopped didn't go into the app.
    fn finish_failed(&mut self, host: &impl Host, title: &str, why: &str) {
        host.notify(FINISH_FAILED, title, why, &[]);
        self.notified_finish = true;
    }

    /// The recording is over: forget it, and start the next one if a
    /// press came meanwhile.
    fn end(&mut self, host: &impl Host) {
        if let Some(id) = self.recording.take() {
            host.forget(&id);
        }
        let next = self.activation.finished();
        self.run(host, next);
    }

    /// You chose the menu entry `id`.
    pub(crate) fn menu_chosen(&mut self, host: &impl Host, id: &str) {
        if id == menu::COPY_LAST {
            if let Some(text) = host.last_dictation() {
                host.copy(&text);
            }
        } else if id == menu::CANCEL {
            if let Some(recording) = self.recording.take() {
                host.cancel(&recording);
                host.forget(&recording);
            }
            self.activation.cancelled();
        } else if let Some(model) = id.strip_prefix(menu::MODEL_PREFIX) {
            if let Some(mut preferences) = host.preferences() {
                preferences.model = Some(model.to_owned());
                host.set_preferences(&preferences);
            }
        } else if let Some(language) = id.strip_prefix(menu::LANGUAGE_PREFIX)
            && let Some(mut preferences) = host.preferences()
        {
            preferences.language = (id != menu::DEFAULT_LANGUAGE).then(|| language.to_owned());
            host.set_preferences(&preferences);
        }
    }

    /// Show the menu as it should be now, if it changed.
    pub(crate) fn refresh_menu(&mut self, host: &impl Host) {
        let models = host.models();
        let preferences = host.preferences();
        let entries = menu::entries(&menu::Inputs {
            busy: self.activation.is_busy(),
            has_last: host.last_dictation().is_some(),
            models: &models,
            preferences: preferences.as_ref(),
        });
        if !self
            .menu
            .as_ref()
            .is_some_and(|shown| menu::same(shown, &entries))
        {
            host.set_menu(&entries);
            self.menu = Some(entries);
        }
    }

    /// The settings page.
    pub(crate) fn render_settings(&mut self, host: &impl Host) -> Vec<Node> {
        let settings = Self::settings(host);
        let mut view = View::panel("root", Some("Dictation"));
        let modes: Vec<(&str, &str)> = Mode::ALL
            .iter()
            .map(|(_, value, label)| (*value, *label))
            .collect();
        view.choice_input(
            View::ROOT,
            ACTIVATION,
            "Shortcut",
            &modes,
            Some(settings.activation.value()),
        );
        view.button(View::ROOT, CHANGE_KEYS, "Change keys");
        let methods: Vec<(&str, &str)> = PasteMethod::ALL
            .iter()
            .map(|(_, value, label)| (*value, *label))
            .collect();
        view.choice_input(
            View::ROOT,
            PASTE,
            "Paste with",
            &methods,
            Some(settings.paste.value()),
        );

        view.section_heading(View::ROOT, "words-heading", "Custom Words", None);
        if !settings.words.is_empty() {
            let list = view.list(View::ROOT, "words", "Custom words");
            for (index, word) in settings.words.iter().enumerate() {
                let row = view.item(list, &format!("word-{index}"), word, None);
                let menu = view.menu(
                    row,
                    &format!("word-{index}-menu"),
                    &format!("Actions for {word}"),
                );
                view.menu_item(menu, &format!("{REMOVE_PREFIX}{index}"), "Remove", true);
            }
        }
        let form = view.form(View::ROOT, ADD_WORD, "New custom word", "Add");
        view.input(form, WORD, "Word", Value::Text(self.typed.clone()));
        view.require(form, WORD);
        if let Some(problem) = &self.problem {
            view.text(View::ROOT, "problem", problem);
        }
        self.drawn_words = settings.words;
        view.into_nodes()
    }

    /// Something happened on the settings page.
    pub(crate) fn settings_event(&mut self, host: &impl Host, event: Event) {
        let change = match event {
            Event::Pressed(node) if node == CHANGE_KEYS => {
                host.open_speech_settings();
                return;
            }
            Event::Changed(field) if field.node == WORD => {
                if let Value::Text(text) = field.value {
                    self.typed = text;
                }
                return;
            }
            Event::Changed(field) if field.node == ACTIVATION => match field.value {
                Value::Choice(Some(value)) => Mode::from_value(&value).map(Change::Activation),
                _ => None,
            },
            Event::Changed(field) if field.node == PASTE => match field.value {
                Value::Choice(Some(value)) => PasteMethod::from_value(&value).map(Change::Paste),
                _ => None,
            },
            Event::Submitted(submission) if submission.node == ADD_WORD => submission
                .fields
                .into_iter()
                .find(|field| field.node == WORD)
                .and_then(|field| match field.value {
                    Value::Text(text) => Some(Change::AddWord(text)),
                    _ => None,
                }),
            Event::Chosen(node) => node
                .strip_prefix(REMOVE_PREFIX)
                .and_then(|index| index.parse::<usize>().ok())
                .and_then(|index| self.drawn_words.get(index))
                .map(|word| Change::RemoveWord(word.clone())),
            _ => None,
        };
        let Some(change) = change else {
            return;
        };
        let adding = matches!(change, Change::AddWord(_));
        match save(host, &change) {
            Ok(()) => {
                self.problem = None;
                if adding {
                    self.typed.clear();
                }
            }
            Err(problem) => {
                if let Change::AddWord(word) = change {
                    self.typed = word;
                }
                self.problem = Some(problem);
            }
        }
    }
}

/// Write `change` to the settings file, creating it the first time. A
/// file that changed meanwhile, as in another app, is read again and the
/// change made to what it holds now.
fn save(host: &impl Host, change: &Change) -> Result<(), String> {
    const FAILED: &str = "Dictation couldn't save the change. Try again.";
    for _ in 0..3 {
        let stored = host.read_settings().map_err(|()| FAILED.to_owned())?;
        let current = stored
            .as_ref()
            .map(|stored| stored.settings.clone())
            .unwrap_or_default();
        let changed = change.apply(&current)?;
        if changed == current && stored.is_some() {
            return Ok(());
        }
        let written = match &stored {
            Some(stored) => host.update_settings(&stored.id, &stored.revision, &changed),
            None => host.create_settings(&changed),
        };
        match written {
            Ok(()) => return Ok(()),
            Err(WriteError::Changed) => {}
            Err(WriteError::Failed) => return Err(FAILED.to_owned()),
        }
    }
    Err(FAILED.to_owned())
}

fn ended(recording: &Recording) -> bool {
    matches!(
        recording.state,
        RecordingState::Finished | RecordingState::Failed | RecordingState::Cancelled
    )
}

/// What you said, in time order.
fn said(recording: &Recording) -> String {
    recording
        .segments
        .iter()
        .filter(|segment| segment.channel == Channel::Me)
        .map(|segment| segment.text.trim())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests;
