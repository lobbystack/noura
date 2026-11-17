//! Speech in Settings and in the desktop shell: the models on this
//! computer and in the catalog, the language you speak, the shortcuts
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
    pub description: String,
    pub license: String,
    /// ISO 639-1 codes.
    pub languages: Vec<String>,
    /// Bytes.
    pub size: u64,
    pub streaming: bool,
    pub translation: bool,
    pub downloaded: bool,
    pub recommended: bool,
    /// Handy shows it by default.
    pub curated: bool,
    /// The grade for the language you speak, such as `A`, or `None` when
    /// the model doesn't speak it.
    pub grade: Option<&'static str>,
    /// Seconds after 30 seconds of speech, on a CPU.
    pub wait_seconds: Option<f64>,
    /// Bytes so far and in all, while it downloads.
    pub downloading: Option<(u64, u64)>,
    /// Why the last download failed.
    pub problem: Option<String>,
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

    /// Every model Noura offers, the downloaded ones first, graded for the
    /// language you speak.
    pub fn speech_models(&self) -> Vec<SpeechModelRow> {
        let Some(kernel) = self.speech_kernel() else {
            return Vec::new();
        };
        let language = kernel
            .speech
            .state()
            .language
            .unwrap_or_else(|| "en".to_owned());
        kernel
            .speech
            .models(Some(&language))
            .into_iter()
            .map(|row| SpeechModelRow {
                id: row.model.id.clone(),
                name: row.model.name.clone(),
                description: row.model.description.clone(),
                license: row.model.license.clone(),
                languages: row.model.languages.clone(),
                size: row.model.file.size,
                streaming: row.model.streaming,
                translation: row.model.translation,
                downloaded: row.downloaded,
                recommended: row.recommended,
                curated: row.model.curated,
                grade: row.model.grade(&language).map(|grade| grade.letter()),
                wait_seconds: row.model.wait_seconds(),
                downloading: row.downloading,
                problem: row.problem,
            })
            .collect()
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

    /// The model onboarding recommends while you haven't finished it:
    /// Parakeet Ultra.
    pub fn speech_onboarding(&self) -> Option<SpeechModelRow> {
        let kernel = self.speech_kernel()?;
        if kernel.speech.state().onboarded {
            return None;
        }
        self.speech_models()
            .into_iter()
            .find(|row| row.id == speech::catalog::ONBOARDING_MODEL)
    }

    pub fn finish_speech_onboarding(&mut self) -> Result<(), AppError> {
        let kernel = self.speech_kernel().ok_or_else(unavailable)?;
        kernel.speech.set_onboarded().map_err(speech_error)
    }

    /// The shortcuts of the plugins running in the open workspace, which
    /// the desktop app registers with the system.
    pub fn global_shortcuts(&self) -> Vec<GlobalShortcut> {
        self.session.plugins().global_shortcuts()
    }

    /// Give `plugin`'s shortcut `id` the keys `keys`, such as `Ctrl+Alt+D`,
    /// or its declared ones back with `None`.
    pub fn set_shortcut_keys(
        &mut self,
        plugin: &str,
        id: &str,
        keys: Option<&str>,
    ) -> Result<(), AppError> {
        if let Some(keys) = keys
            && !plugin_host::valid_keys(keys)
        {
            return Err(AppError::new(
                "invalid_keys",
                "Use modifiers such as Ctrl or Alt, then one key, as in Alt+Space.",
            ));
        }
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
