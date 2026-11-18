//! What Dictation asks of Noura, behind a trait so the tests can stand in
//! for the kernel: a fake engine's recordings, a paste that writes down
//! what it pasted, and a settings file in memory. [`Kernel`] is the real
//! one, through the plugin interface.

use plugin_sdk::{
    Error,
    menu_bar::{self, Entry},
    navigation::{self, SettingsSection},
    notifications::{self, Action, Notification},
    objects::{self, Changes, NewObject},
    paste::{self, Method},
    speech::{self, Model, Preferences, Recording, RecordingOptions, Source},
};

use crate::settings::{SETTINGS_PATH, SETTINGS_TYPE, Settings};

/// The settings file as the plugin read it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Stored {
    pub(crate) id: String,
    pub(crate) revision: String,
    pub(crate) settings: Settings,
}

/// Why writing the settings file failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WriteError {
    /// The file changed since the plugin read it, or appeared meanwhile.
    Changed,
    Failed,
}

pub(crate) trait Host {
    /// Start recording the microphone with this plugin's model, microphone
    /// and language. The error is in words for you.
    fn start(&self) -> Result<String, String>;
    fn stop(&self, id: &str) -> Result<(), String>;
    fn cancel(&self, id: &str);
    fn read(&self, id: &str) -> Option<Recording>;
    fn forget(&self, id: &str);
    /// Paste `text` into the focused app. The error is in words for you.
    fn paste(&self, text: &str, method: Method) -> Result<(), String>;
    fn copy(&self, text: &str);
    fn remember(&self, text: &str);
    fn last_dictation(&self) -> Option<String>;
    fn models(&self) -> Vec<Model>;
    fn preferences(&self) -> Option<Preferences>;
    fn set_preferences(&self, preferences: &Preferences);
    fn set_menu(&self, entries: &[Entry]);
    /// Show the notification `id`, with actions by ID and label.
    fn notify(&self, id: &str, title: &str, body: &str, actions: &[(&str, &str)]);
    fn withdraw(&self, id: &str);
    /// Open Settings on Speech, where you download models.
    fn open_speech_settings(&self);
    /// The settings file, or `None` when there is none yet.
    fn read_settings(&self) -> Result<Option<Stored>, ()>;
    fn create_settings(&self, settings: &Settings) -> Result<(), WriteError>;
    fn update_settings(
        &self,
        id: &str,
        revision: &str,
        settings: &Settings,
    ) -> Result<(), WriteError>;
}

/// Noura, through the plugin interface.
pub(crate) struct Kernel;

fn words(error: Error) -> String {
    match error {
        Error::Failed(message) => message,
        Error::CapabilityNotDeclared(_) => "Noura's speech isn't allowed for Dictation.".to_owned(),
        _ => "Dictation couldn't start. Try again.".to_owned(),
    }
}

fn write_error(error: &Error) -> WriteError {
    match error {
        Error::RevisionConflict(_) | Error::AlreadyExists(_) => WriteError::Changed,
        _ => WriteError::Failed,
    }
}

impl Host for Kernel {
    fn start(&self) -> Result<String, String> {
        speech::start(&RecordingOptions {
            sources: vec![Source::Microphone],
            model: None,
            microphone: None,
            language: None,
        })
        .map_err(words)
    }

    fn stop(&self, id: &str) -> Result<(), String> {
        speech::stop(id).map_err(words)
    }

    fn cancel(&self, id: &str) {
        let _ = speech::cancel(id);
    }

    fn read(&self, id: &str) -> Option<Recording> {
        speech::read_recording(id).ok()
    }

    fn forget(&self, id: &str) {
        let _ = speech::forget(id);
    }

    fn paste(&self, text: &str, method: Method) -> Result<(), String> {
        paste::paste(text, method).map_err(|error| match error {
            Error::Failed(message) => message,
            Error::CapabilityNotDeclared(_) => "Pasting isn't allowed for Dictation.".to_owned(),
            _ => "Dictation couldn't paste. Use Copy Last Dictation.".to_owned(),
        })
    }

    fn copy(&self, text: &str) {
        let _ = paste::copy(text);
    }

    fn remember(&self, text: &str) {
        let _ = speech::remember_dictation(text);
    }

    fn last_dictation(&self) -> Option<String> {
        speech::last_dictation().ok().flatten()
    }

    fn models(&self) -> Vec<Model> {
        speech::models().unwrap_or_default()
    }

    fn preferences(&self) -> Option<Preferences> {
        speech::read_preferences().ok()
    }

    fn set_preferences(&self, preferences: &Preferences) {
        let _ = speech::set_preferences(preferences);
    }

    fn set_menu(&self, entries: &[Entry]) {
        let _ = menu_bar::set_entries(entries);
    }

    fn notify(&self, id: &str, title: &str, body: &str, actions: &[(&str, &str)]) {
        let _ = notifications::notify(&Notification {
            id: id.to_owned(),
            title: title.to_owned(),
            body: body.to_owned(),
            actions: actions
                .iter()
                .map(|(id, label)| Action {
                    id: (*id).to_owned(),
                    label: (*label).to_owned(),
                })
                .collect(),
        });
    }

    fn withdraw(&self, id: &str) {
        let _ = notifications::withdraw(id);
    }

    fn open_speech_settings(&self) {
        let _ = navigation::open_settings(SettingsSection::Speech);
    }

    fn read_settings(&self) -> Result<Option<Stored>, ()> {
        let mut files = objects::list(SETTINGS_TYPE).map_err(|_| ())?;
        // The file at the usual path wins, then the first by path, as
        // after a copy in another app.
        files.sort_by(|a, b| {
            (a.path != SETTINGS_PATH, &a.path).cmp(&(b.path != SETTINGS_PATH, &b.path))
        });
        Ok(files.into_iter().next().map(|file| Stored {
            settings: Settings::from_properties(&file.properties),
            id: file.id,
            revision: file.revision,
        }))
    }

    fn create_settings(&self, settings: &Settings) -> Result<(), WriteError> {
        objects::create(&NewObject {
            object_type: SETTINGS_TYPE.to_owned(),
            title: String::new(),
            path: Some(SETTINGS_PATH.to_owned()),
            body: String::new(),
            properties: settings.to_properties(),
        })
        .map(|_| ())
        .map_err(|error| write_error(&error))
    }

    fn update_settings(
        &self,
        id: &str,
        revision: &str,
        settings: &Settings,
    ) -> Result<(), WriteError> {
        objects::update(
            id,
            revision,
            &Changes {
                body: None,
                properties: settings.to_properties(),
                remove: Vec::new(),
            },
        )
        .map(|_| ())
        .map_err(|error| write_error(&error))
    }
}
