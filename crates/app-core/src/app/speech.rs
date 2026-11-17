//! Speech in Settings and in the desktop shell: the models on this
//! computer and in the catalog, searched, filtered and graded, the model
//! in use, the language you speak, the shortcuts
//! plugins declared and the keys you gave them, the menu bar entries, and
//! the presses and choices that reach plugins as kernel events.

use std::sync::Arc;
use std::time::Instant;

use plugin_host::{KernelEvent, ShortcutEvent};

use super::App;
use crate::speech_kernel::SpeechKernel;
use crate::{AppError, GlobalShortcut, MenuBarItems};

/// A model as Settings > Speech lists it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechModelRow {
    pub id: String,
    pub name: String,
    /// One line from the catalog.
    pub description: String,
    pub license: String,
    /// Bytes.
    pub size: u64,
    /// It shows your words while you talk.
    pub streaming: bool,
    pub translation: bool,
    pub downloaded: bool,
    /// The model in use, among the downloaded ones.
    pub selected: bool,
    pub recommended: bool,
    /// A letter per column of [`SpeechModelsPage::languages`], such as
    /// `A`, or `None` where the model doesn't hear that language.
    pub grades: Vec<Option<&'static str>>,
    /// Bars out of four, from the wait after 30 seconds of speech on a
    /// CPU, or `None` when nobody timed it.
    pub speed: Option<u8>,
    /// Bytes so far and in all, while it downloads.
    pub downloading: Option<(u64, u64)>,
    /// Why the last download failed.
    pub problem: Option<String>,
}

/// What narrows the models in Settings > Speech.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SpeechModelFilter {
    /// Words that each must appear in a model's name, description, tags or
    /// languages.
    pub search: String,
    /// Only models that show your words while you talk.
    pub live_preview: bool,
    /// Only models that translate.
    pub translates: bool,
}

impl SpeechModelFilter {
    /// Whether a search or a filter is on.
    pub fn narrows(&self) -> bool {
        !self.search.trim().is_empty() || self.live_preview || self.translates
    }
}

/// The groups of models in Settings > Speech.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpeechModelSection {
    /// Downloaded, the one in use first.
    OnThisComputer,
    /// Not downloaded, hearing your language, best first.
    ForYourLanguages,
    /// Everything else that matches, while you search or filter.
    Others,
}

impl SpeechModelSection {
    pub fn title(self) -> &'static str {
        match self {
            Self::OnThisComputer => "On this computer",
            Self::ForYourLanguages => "For your languages",
            Self::Others => "Other models",
        }
    }
}

/// Settings > Speech's model list.
#[derive(Debug, Clone, PartialEq)]
pub struct SpeechModelsPage {
    /// The grade columns, as ISO 639-1 codes: English, then the language
    /// you speak when it isn't English.
    pub languages: Vec<String>,
    /// The sections with their rows. While nothing narrows the list, On
    /// this computer shows even when empty; otherwise empty sections go.
    pub sections: Vec<(SpeechModelSection, Vec<SpeechModelRow>)>,
}

/// The wait in seconds under which a model earns each bar, as Handy draws
/// its speed: four bars under 2 s, none from 20 s.
const SPEED_CUTOFFS: [f64; 4] = [2.0, 3.5, 7.5, 20.0];

fn speed(wait: Option<f64>) -> Option<u8> {
    wait.map(|wait| {
        SPEED_CUTOFFS
            .iter()
            .filter(|cutoff| wait < **cutoff)
            .count() as u8
    })
}

/// The name of the language with ISO 639-1 code `code`, or the code.
pub fn speech_language_name(code: &str) -> &str {
    SPEECH_LANGUAGES
        .iter()
        .find(|(known, _)| *known == code)
        .map_or(code, |(_, name)| name)
}

/// Whether every word of `search` shows in `model`'s name, description,
/// tags or language names.
fn matches(model: &speech::catalog::CatalogModel, search: &str) -> bool {
    let mut hay = format!("{} {}", model.name, model.description);
    if model.streaming {
        hay.push_str(" live preview");
    }
    if model.translation {
        hay.push_str(" translates");
    }
    for code in &model.languages {
        hay.push(' ');
        hay.push_str(speech_language_name(code));
    }
    let hay = hay.to_lowercase();
    search
        .to_lowercase()
        .split_whitespace()
        .all(|word| hay.contains(word))
}

/// The languages Settings offers, as ISO 639-1 codes with their names.
pub const SPEECH_LANGUAGES: [(&str, &str); 25] = [
    ("en", "English"),
    ("fr", "French"),
    ("de", "German"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("pt", "Portuguese"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("cs", "Czech"),
    ("sv", "Swedish"),
    ("da", "Danish"),
    ("fi", "Finnish"),
    ("el", "Greek"),
    ("hu", "Hungarian"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("uk", "Ukrainian"),
    ("tr", "Turkish"),
    ("ar", "Arabic"),
    ("hi", "Hindi"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh", "Chinese"),
    ("vi", "Vietnamese"),
    ("id", "Indonesian"),
];

impl App {
    fn speech_kernel(&self) -> Option<Arc<SpeechKernel>> {
        self.session.plugins().speech().cloned()
    }

    /// Counts the changes to downloads, models and menu bar entries, so a
    /// window knows to draw again.
    pub fn speech_version(&self) -> u64 {
        self.speech_kernel().map_or(0, |kernel| kernel.version())
    }

    /// The language you speak, as an ISO 639-1 code.
    pub fn speech_language(&self) -> Option<String> {
        self.speech_kernel()
            .and_then(|kernel| kernel.speech.state().language)
    }

    pub fn set_speech_language(&mut self, language: Option<&str>) -> Result<(), AppError> {
        let kernel = self.speech_kernel().ok_or_else(unavailable)?;
        kernel
            .speech
            .set_language(language.map(str::to_owned))
            .map_err(speech_error)
    }

    /// The models for Settings > Speech, narrowed by `filter`, in their
    /// sections and graded for English and the language you speak.
    pub fn speech_models(&self, filter: &SpeechModelFilter) -> SpeechModelsPage {
        let Some(kernel) = self.speech_kernel() else {
            return SpeechModelsPage {
                languages: Vec::new(),
                sections: Vec::new(),
            };
        };
        let language = kernel
            .speech
            .state()
            .language
            .unwrap_or_else(|| "en".to_owned());
        let mut languages = vec!["en".to_owned()];
        if language != "en" {
            languages.push(language.clone());
        }
        let selected = kernel.speech.selected_model().map(|model| model.id.clone());
        let narrows = filter.narrows();
        let mut here = Vec::new();
        let mut yours = Vec::new();
        let mut others = Vec::new();
        for row in kernel.speech.models(Some(&language)) {
            let model = row.model;
            if (filter.live_preview && !model.streaming)
                || (filter.translates && !model.translation)
                || !matches(model, &filter.search)
            {
                continue;
            }
            let cost = model.cost(&language);
            let shown = SpeechModelRow {
                id: model.id.clone(),
                name: model.name.clone(),
                description: model.description.clone(),
                license: model.license.clone(),
                size: model.file.size,
                streaming: model.streaming,
                translation: model.translation,
                downloaded: row.downloaded,
                selected: row.downloaded && selected.as_deref() == Some(model.id.as_str()),
                recommended: row.recommended,
                grades: languages
                    .iter()
                    .map(|code| model.grade(code).map(|grade| grade.letter()))
                    .collect(),
                speed: speed(model.wait_seconds()),
                downloading: row.downloading,
                problem: row.problem,
            };
            // Best first: the recommendation, then by Handy's cost.
            let rank = (!shown.recommended, cost.unwrap_or(f64::MAX));
            if shown.downloaded {
                here.push((!shown.selected, rank, shown));
            } else if cost.is_some() && (model.curated || narrows) {
                yours.push((false, rank, shown));
            } else if narrows {
                others.push((false, rank, shown));
            }
        }
        let sorted = |mut rows: Vec<(bool, (bool, f64), SpeechModelRow)>| {
            rows.sort_by(|a, b| {
                (a.0, a.1.0)
                    .cmp(&(b.0, b.1.0))
                    .then(a.1.1.total_cmp(&b.1.1))
            });
            rows.into_iter().map(|(_, _, row)| row).collect::<Vec<_>>()
        };
        let sections = [
            (SpeechModelSection::OnThisComputer, sorted(here)),
            (SpeechModelSection::ForYourLanguages, sorted(yours)),
            (SpeechModelSection::Others, sorted(others)),
        ]
        .into_iter()
        .filter(|(section, rows)| {
            !rows.is_empty() || (!narrows && *section == SpeechModelSection::OnThisComputer)
        })
        .collect();
        SpeechModelsPage {
            languages,
            sections,
        }
    }

    /// Use the downloaded model `id` for speech, in every plugin.
    pub fn select_speech_model(&mut self, id: &str) -> Result<(), AppError> {
        let kernel = self.speech_kernel().ok_or_else(unavailable)?;
        kernel.speech.select_model(id).map_err(speech_error)?;
        kernel.changed();
        Ok(())
    }

    pub fn download_speech_model(&mut self, id: &str) -> Result<(), AppError> {
        let kernel = self.speech_kernel().ok_or_else(unavailable)?;
        kernel.speech.download(id).map_err(speech_error)
    }

    pub fn cancel_speech_download(&mut self, id: &str) {
        if let Some(kernel) = self.speech_kernel() {
            kernel.speech.cancel_download(id);
        }
    }

    pub fn delete_speech_model(&mut self, id: &str) -> Result<(), AppError> {
        let kernel = self.speech_kernel().ok_or_else(unavailable)?;
        kernel.speech.delete_model(id).map_err(speech_error)
    }

    /// The shortcuts of the plugins running in the open workspace, which
    /// the desktop app registers with the system.
    pub fn global_shortcuts(&self) -> Vec<GlobalShortcut> {
        self.session.plugins().global_shortcuts()
    }

    /// Give `plugin`'s shortcut `id` the keys `keys`, such as `Ctrl+Alt+D`
    /// or, as macOS writes them, `⌃⌥D`, or its declared ones back with
    /// `None`.
    pub fn set_shortcut_keys(
        &mut self,
        plugin: &str,
        id: &str,
        keys: Option<&str>,
    ) -> Result<(), AppError> {
        let keys = keys
            .map(|typed| plugin_host::keys_from_label(typed).ok_or_else(invalid_keys))
            .transpose()?;
        let keys = keys.as_deref();
        if !self
            .global_shortcuts()
            .iter()
            .any(|shortcut| shortcut.plugin == plugin && shortcut.id == id)
        {
            return Err(AppError::new(
                "shortcut_not_found",
                "That shortcut isn't on.",
            ));
        }
        let kernel = self.speech_kernel().ok_or_else(unavailable)?;
        kernel
            .speech
            .set_shortcut_keys(plugin, id, keys.map(str::to_owned))
            .map_err(speech_error)?;
        kernel.changed();
        Ok(())
    }

    /// Note whether the system took the keys of `plugin`'s shortcut `id`.
    pub fn shortcut_registered(&mut self, plugin: &str, id: &str, registered: bool) {
        if let Some(kernel) = self.speech_kernel() {
            kernel.set_registered(plugin, id, registered);
        }
    }

    /// The keys of `plugin`'s shortcut `id` went down at `at`, or up with
    /// `pressed` false. It reaches the plugin as something you did.
    pub fn press_shortcut(&mut self, plugin: &str, id: &str, pressed: bool, at: Instant) {
        let Some(kernel) = self.speech_kernel() else {
            return;
        };
        let event = ShortcutEvent {
            id: id.to_owned(),
            at_ms: kernel.since_start(at),
        };
        let event = if pressed {
            KernelEvent::ShortcutPressed(event)
        } else {
            KernelEvent::ShortcutReleased(event)
        };
        self.session.plugins().kernel_event(plugin, event);
    }

    /// Hand the running plugins what the kernel queued for them, such as a
    /// recording that finished. [`App::poll`] does this too, but only a
    /// window that draws polls, and a window under the app you dictate
    /// into draws nothing, so the desktop calls this in every pass.
    pub fn deliver_plugin_events(&mut self) {
        self.session.plugins().poll();
    }

    /// The menu bar entries of the running plugins.
    pub fn tray_menu(&self) -> Vec<MenuBarItems> {
        self.speech_kernel()
            .map_or_else(Vec::new, |kernel| kernel.menus())
    }

    /// You chose `plugin`'s entry `id` in the tray or menu bar extra.
    pub fn choose_menu_item(&mut self, plugin: &str, id: &str) {
        self.session
            .plugins()
            .kernel_event(plugin, KernelEvent::MenuItemChosen(id.to_owned()));
    }

    /// Where the models live on this computer.
    pub fn speech_models_folder(&self) -> Option<std::path::PathBuf> {
        self.speech_kernel()
            .map(|kernel| crate::speech_kernel::models_folder(&kernel))
    }
}

fn unavailable() -> AppError {
    AppError::new("speech_unavailable", "Speech isn't available here.")
}

fn speech_error(error: speech::SpeechError) -> AppError {
    AppError::new(&error.code, error.message)
}

/// Keys you typed that aren't a shortcut, with an example in the form this
/// computer's system writes keys.
fn invalid_keys() -> AppError {
    let hint = if cfg!(target_os = "macos") {
        "Use modifiers such as \u{2303} Control or \u{2325} Option, then one key, as in \
         \u{2325}Space."
    } else {
        "Use modifiers such as Ctrl or Alt, then one key, as in Alt+Space."
    };
    AppError::new("invalid_keys", hint)
}
