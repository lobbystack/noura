//! The plugin's settings and its page in Settings. They live in one
//! workspace file, `Meetings/Transcription settings.md` when Noura makes
//! it, whose body is the Summary's prompt: anarlog's
//! `custom_summary_instructions`, which replace the default format
//! requirements. System audio is a choice of this device, which the
//! kernel's speech preferences keep.

use std::collections::BTreeSet;

use plugin_sdk::{
    Error, Event, Node, Value, View, ai,
    objects::{self, Changes, NewObject, Object},
    speech,
};
use serde_json::{Map, json};

use crate::{prompt, types::SETTINGS};

/// Where Noura writes the settings file when there is none.
pub(crate) const SETTINGS_PATH: &str = "Meetings/Transcription settings.md";

/// Where a new Transcription goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FolderRule {
    /// In the folder of the project you pick, otherwise `Meetings/`.
    Project,
    /// Always in `Meetings/`.
    Meetings,
}

/// What happens to a recording's audio once its Transcript is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Audio {
    /// Noura drops it.
    None,
    /// A WAV file in the Transcription's folder, which syncs.
    Folder,
    /// In this device's app data folder for `audio_days` days.
    Device,
}

/// The settings, as the file holds them, or the defaults.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Settings {
    pub(crate) folder: FolderRule,
    /// The AI provider's ID, or none for your first.
    pub(crate) provider: Option<String>,
    /// The Summary's language, an ISO 639-1 code, or none for the one
    /// Settings, Speech holds.
    pub(crate) language: Option<String>,
    pub(crate) audio: Audio,
    pub(crate) audio_days: u32,
    /// Your prompt, or empty for anarlog's default format requirements.
    pub(crate) prompt: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            folder: FolderRule::Project,
            provider: None,
            language: None,
            audio: Audio::None,
            audio_days: 30,
            prompt: String::new(),
        }
    }
}

/// The settings file, the first by path when there are several.
fn file() -> Option<Object> {
    let mut files = objects::list(SETTINGS).ok()?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files.into_iter().next()
}

impl Settings {
    /// The settings now. A property that doesn't fit takes its default.
    pub(crate) fn read() -> Self {
        file().map_or_else(Self::default, |file| Self::of(&file))
    }

    fn of(file: &Object) -> Self {
        let properties: Map<String, serde_json::Value> =
            serde_json::from_str(&file.properties).unwrap_or_default();
        let text = |key: &str| {
            properties
                .get(key)
                .and_then(serde_json::Value::as_str)
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_owned)
        };
        let defaults = Self::default();
        Self {
            folder: match text("folder").as_deref() {
                Some("meetings") => FolderRule::Meetings,
                _ => FolderRule::Project,
            },
            provider: text("provider"),
            language: text("language"),
            audio: match text("audio").as_deref() {
                Some("folder") => Audio::Folder,
                Some("device") => Audio::Device,
                _ => Audio::None,
            },
            audio_days: properties
                .get("audio_days")
                .and_then(serde_json::Value::as_u64)
                .and_then(|days| u32::try_from(days).ok())
                .filter(|days| *days > 0)
                .unwrap_or(defaults.audio_days),
            prompt: file.body.trim().to_owned(),
        }
    }

    /// The language the Summary is written in: the setting, then the one
    /// Settings, Speech holds.
    pub(crate) fn summary_language(&self) -> Option<String> {
        self.language.clone().or_else(|| {
            speech::read_preferences()
                .ok()
                .and_then(|preferences| preferences.language)
        })
    }
}

/// Write `properties` and, when given, `body` to the settings file,
/// creating it when there is none.
fn save(properties: serde_json::Value, body: Option<String>) -> Result<(), Error> {
    match file() {
        Some(file) => objects::update(
            &file.id,
            &file.revision,
            &Changes {
                body,
                properties: properties.to_string(),
                remove: Vec::new(),
            },
        )
        .map(drop),
        None => objects::create(&NewObject {
            object_type: SETTINGS.to_owned(),
            title: String::new(),
            path: Some(SETTINGS_PATH.to_owned()),
            body: body.unwrap_or_default(),
            properties: properties.to_string(),
        })
        .map(drop),
    }
}

/// `text` as a JSON string, or `null` when it's empty or missing.
fn null_or(text: Option<&str>) -> serde_json::Value {
    text.filter(|text| !text.is_empty())
        .map_or(serde_json::Value::Null, |text| json!(text))
}

/// The page's state between calls: what the last save said.
pub(crate) struct Page {
    pub(crate) notice: Option<String>,
}

pub(crate) static PAGE: std::sync::Mutex<Page> = std::sync::Mutex::new(Page { notice: None });

fn page() -> std::sync::MutexGuard<'static, Page> {
    PAGE.lock().unwrap_or_else(|error| error.into_inner())
}

const DAYS: [(&str, &str); 4] = [
    ("1", "1 day"),
    ("7", "7 days"),
    ("30", "30 days"),
    ("90", "90 days"),
];

/// The languages the downloaded models transcribe, or every model's when
/// none is downloaded, with their names, by name.
fn languages() -> Vec<(String, String)> {
    let models = speech::models().unwrap_or_default();
    let downloaded: Vec<_> = models.iter().filter(|model| model.downloaded).collect();
    let chosen = if downloaded.is_empty() {
        models.iter().collect()
    } else {
        downloaded
    };
    let codes: BTreeSet<String> = chosen
        .iter()
        .flat_map(|model| model.languages.iter().cloned())
        .collect();
    let mut named: Vec<(String, String)> = codes
        .into_iter()
        .map(|code| {
            let name = prompt::language_name(Some(&code));
            (code, name)
        })
        .collect();
    named.sort_by(|a, b| a.1.cmp(&b.1));
    named
}

/// The page's nodes.
pub(crate) fn render() -> Vec<Node> {
    let settings = Settings::read();
    let mut view = View::panel("settings", Some("Transcription"));
    view.choice_input(
        View::ROOT,
        "folder",
        "Folder",
        &[("project", "Project folder"), ("meetings", "Meetings")],
        Some(match settings.folder {
            FolderRule::Project => "project",
            FolderRule::Meetings => "meetings",
        }),
    );

    let providers = ai::providers().unwrap_or_default();
    let mut provider_choices: Vec<(String, String)> =
        vec![("first".into(), "First provider".into())];
    provider_choices.extend(
        providers
            .iter()
            .map(|provider| (provider.id.clone(), provider.name.clone())),
    );
    if let Some(id) = &settings.provider
        && !providers.iter().any(|provider| &provider.id == id)
    {
        provider_choices.push((id.clone(), "Missing provider".into()));
    }
    let provider_refs: Vec<(&str, &str)> = provider_choices
        .iter()
        .map(|(id, name)| (id.as_str(), name.as_str()))
        .collect();
    view.choice_input(
        View::ROOT,
        "provider",
        "Provider",
        &provider_refs,
        Some(settings.provider.as_deref().unwrap_or("first")),
    );

    let mut language_choices: Vec<(String, String)> =
        vec![("speech".into(), "Same as Speech".into())];
    language_choices.extend(languages());
    if let Some(code) = &settings.language
        && !language_choices.iter().any(|(known, _)| known == code)
    {
        language_choices.push((code.clone(), prompt::language_name(Some(code))));
    }
    let language_refs: Vec<(&str, &str)> = language_choices
        .iter()
        .map(|(id, name)| (id.as_str(), name.as_str()))
        .collect();
    view.choice_input(
        View::ROOT,
        "language",
        "Language",
        &language_refs,
        Some(settings.language.as_deref().unwrap_or("speech")),
    );

    let system_audio = speech::read_preferences()
        .map(|preferences| preferences.system_audio)
        .unwrap_or(false);
    view.input(
        View::ROOT,
        "system-audio",
        "System audio",
        Value::Checkbox(system_audio),
    );

    view.choice_input(
        View::ROOT,
        "audio",
        "Keep audio",
        &[
            ("none", "Don't keep"),
            ("folder", "In the folder"),
            ("device", "On this device"),
        ],
        Some(match settings.audio {
            Audio::None => "none",
            Audio::Folder => "folder",
            Audio::Device => "device",
        }),
    );
    match settings.audio {
        Audio::Folder => {
            view.text(View::ROOT, "audio-syncs", "Audio in the folder syncs.");
        }
        Audio::Device => {
            let days = settings.audio_days.to_string();
            let chosen = DAYS
                .iter()
                .any(|(id, _)| *id == days)
                .then_some(days.as_str())
                .unwrap_or("30");
            view.choice_input(View::ROOT, "audio-days", "Keep for", &DAYS, Some(chosen));
        }
        Audio::None => {}
    }

    let form = view.form(View::ROOT, "prompt-form", "Summary prompt", "Save prompt");
    let shown = if settings.prompt.is_empty() {
        prompt::DEFAULT_FORMAT.trim()
    } else {
        settings.prompt.as_str()
    };
    view.input(form, "prompt", "Prompt", Value::Multiline(shown.to_owned()));
    if !settings.prompt.is_empty() {
        view.button(View::ROOT, "reset-prompt", "Reset prompt");
    }
    if let Some(notice) = &page().notice {
        view.text(View::ROOT, "notice", notice);
    }
    view.into_nodes()
}

fn choice(value: &Value) -> Option<&str> {
    match value {
        Value::Choice(choice) => choice.as_deref(),
        _ => None,
    }
}

/// Answer an event on the page.
pub(crate) fn handle(event: Event) {
    let result = match event {
        Event::Changed(field) => match (field.node.as_str(), &field.value) {
            ("folder", value) => save(json!({ "folder": choice(value) }), None),
            ("provider", value) => save(
                json!({ "provider": null_or(choice(value).filter(|id| *id != "first")) }),
                None,
            ),
            ("language", value) => save(
                json!({ "language": null_or(choice(value).filter(|code| *code != "speech")) }),
                None,
            ),
            ("audio", value) => save(json!({ "audio": choice(value) }), None),
            ("audio-days", value) => {
                let days: u32 = choice(value)
                    .and_then(|days| days.parse().ok())
                    .unwrap_or(30);
                save(json!({ "audio_days": days }), None)
            }
            ("system-audio", Value::Checkbox(on)) => set_system_audio(*on),
            _ => Ok(()),
        },
        Event::Submitted(submission) if submission.node == "prompt-form" => {
            let text = submission
                .fields
                .iter()
                .find(|field| field.node == "prompt")
                .and_then(|field| match &field.value {
                    Value::Multiline(text) | Value::Text(text) => Some(text.clone()),
                    _ => None,
                })
                .unwrap_or_default();
            // The default text saved as is stays the default.
            let text = if text.trim() == prompt::DEFAULT_FORMAT.trim() {
                String::new()
            } else {
                text.trim().to_owned()
            };
            if let Err(problem) = prompt::system(None, "2026-01-01", &text) {
                page().notice = Some(problem.to_string());
                return;
            }
            save_prompt(text)
        }
        Event::Pressed(node) if node == "reset-prompt" => save_prompt(String::new()),
        _ => Ok(()),
    };
    page().notice = result
        .err()
        .map(|error| format!("Noura couldn't save the setting: {}", crate::reason(&error)));
}

fn save_prompt(text: String) -> Result<(), Error> {
    let body = if text.is_empty() {
        String::new()
    } else {
        format!("{text}\n")
    };
    save(json!({}), Some(body))
}

fn set_system_audio(on: bool) -> Result<(), Error> {
    let mut preferences = speech::read_preferences()?;
    preferences.system_audio = on;
    speech::set_preferences(&preferences)
}
