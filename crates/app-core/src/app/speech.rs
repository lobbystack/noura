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
use crate::{AppError, GlobalShortcut, Key, KeyPress, MenuBarItems, Platform, Shortcut};

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

/// A plugin's shortcut as Settings > Speech lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SpeechShortcut {
    pub plugin: String,
    /// The plugin's name, such as `Dictation`.
    pub plugin_name: String,
    pub id: String,
    pub title: String,
    /// The keys, as manifests write them, such as `Alt+Space`.
    pub keys: String,
    /// The keys the plugin declares.
    pub declared: String,
    pub status: ShortcutStatus,
}

/// Whether a shortcut's keys work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortcutStatus {
    /// The system took the keys, or hasn't answered yet.
    On,
    /// The system refused the keys, as when another app holds them.
    Refused,
    /// Its plugin is installed here and off in the open workspace, so
    /// the keys do nothing.
    PluginOff,
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

    /// The shortcuts Settings > Speech lists: those of the running
    /// plugins, with whether the system took their keys, then those of
    /// the plugins installed here that are off in the open workspace.
    pub fn speech_shortcuts(&self) -> Vec<SpeechShortcut> {
        let Some(kernel) = self.speech_kernel() else {
            return Vec::new();
        };
        let plugins = self.session.plugins();
        let row = |shortcut: GlobalShortcut, status: ShortcutStatus| SpeechShortcut {
            plugin_name: plugins.shown_name(&shortcut.plugin),
            plugin: shortcut.plugin,
            id: shortcut.id,
            title: shortcut.title,
            keys: shortcut.keys,
            declared: shortcut.declared,
            status,
        };
        let running = plugins.global_shortcuts().into_iter().map(|shortcut| {
            let status = match kernel.registered(&shortcut.plugin, &shortcut.id) {
                Some(false) => ShortcutStatus::Refused,
                _ => ShortcutStatus::On,
            };
            row(shortcut, status)
        });
        let idle = plugins
            .idle_shortcuts()
            .into_iter()
            .map(|shortcut| row(shortcut, ShortcutStatus::PluginOff));
        running.chain(idle).collect()
    }

    /// Give `plugin`'s shortcut `id` the keys `keys`, such as `Ctrl+Alt+D`
    /// or, as macOS writes them, `⌃⌥D`, or its declared ones back with
    /// `None`. Keys need a modifier other than Shift, or a function key,
    /// and can't be another shortcut's, one of Noura's or one the system
    /// keeps. Saving them ends a recording.
    pub fn set_shortcut_keys(
        &mut self,
        plugin: &str,
        id: &str,
        keys: Option<&str>,
    ) -> Result<(), AppError> {
        let platform = self.platform();
        let keys = keys
            .map(|typed| plugin_host::keys_from_label(typed).ok_or_else(|| invalid_keys(platform)))
            .transpose()?;
        let keys = keys.as_deref();
        let shortcuts = self.global_shortcuts();
        let Some(shortcut) = shortcuts
            .iter()
            .find(|shortcut| shortcut.plugin == plugin && shortcut.id == id)
        else {
            return Err(AppError::new(
                "shortcut_not_found",
                "That shortcut isn't on.",
            ));
        };
        if let Some(keys) = keys {
            self.check_new_keys(shortcut, keys, platform)?;
        }
        let kernel = self.speech_kernel().ok_or_else(unavailable)?;
        kernel
            .speech
            .set_shortcut_keys(plugin, id, keys.map(str::to_owned))
            .map_err(speech_error)?;
        self.stop_recording_shortcut();
        kernel.changed();
        Ok(())
    }

    /// Why `keys` can't be `shortcut`'s on `platform`, if they can't.
    fn check_new_keys(
        &self,
        shortcut: &GlobalShortcut,
        keys: &str,
        platform: Platform,
    ) -> Result<(), AppError> {
        if keys == shortcut.keys {
            return Ok(());
        }
        if !holds_a_modifier(keys) {
            return Err(invalid_keys(platform));
        }
        let others = self
            .global_shortcuts()
            .into_iter()
            .chain(self.session.plugins().idle_shortcuts());
        for other in others {
            if (other.plugin != shortcut.plugin || other.id != shortcut.id) && other.keys == keys {
                return Err(AppError::new(
                    "keys_in_use",
                    format!("{} already uses these keys.", other.title),
                ));
            }
        }
        if let Some(press) = key_press(keys) {
            for entry in self
                .shortcut_groups()
                .into_iter()
                .flat_map(|group| group.entries)
            {
                if entry
                    .shortcuts
                    .iter()
                    .any(|own| presses(own, &press, platform))
                {
                    return Err(AppError::new(
                        "keys_in_use",
                        format!("Noura uses these keys for {}.", entry.label),
                    ));
                }
            }
        }
        if system_keys(platform).contains(&keys) {
            let system = match platform {
                Platform::MacOs => "macOS",
                Platform::Windows => "Windows",
                Platform::Linux => "Your desktop",
            };
            return Err(AppError::new(
                "keys_reserved",
                format!("{system} keeps these keys for itself."),
            ));
        }
        Ok(())
    }

    /// The shortcut this window records new keys for, as (plugin, ID).
    pub fn recording_shortcut(&self) -> Option<(String, String)> {
        let recording = self.recording_shortcut.clone()?;
        let kernel = self.speech_kernel()?;
        (kernel.recording() == Some(self.key)).then_some(recording)
    }

    /// Record new keys for `plugin`'s shortcut `id`: the desktop app sets
    /// the system's shortcuts aside, so the keys you press reach this
    /// window, and the window's own shortcuts wait too.
    pub fn record_shortcut(&mut self, plugin: &str, id: &str) {
        let Some(kernel) = self.speech_kernel() else {
            return;
        };
        self.recording_shortcut = Some((plugin.to_owned(), id.to_owned()));
        kernel.set_recording(Some(self.key));
    }

    /// Stop recording keys, keeping the ones the shortcut had.
    pub fn stop_recording_shortcut(&mut self) {
        if self.recording_shortcut.take().is_some()
            && let Some(kernel) = self.speech_kernel()
            && kernel.recording() == Some(self.key)
        {
            kernel.set_recording(None);
        }
    }

    /// Whether the desktop app should set the system's shortcuts aside,
    /// while a window records new keys for one.
    pub fn global_shortcuts_paused(&self) -> bool {
        self.speech_kernel()
            .is_some_and(|kernel| kernel.recording().is_some())
    }

    /// `keys`, such as `Alt+Space`, as the app's platform writes them:
    /// `⌥Space` on macOS and `Alt+Space` elsewhere.
    pub fn keys_label(&self, keys: &str) -> String {
        plugin_host::keys_label_on(keys, self.platform() == Platform::MacOs)
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

/// Keys you typed that aren't a shortcut, with an example in the form
/// `platform` writes keys.
fn invalid_keys(platform: Platform) -> AppError {
    let hint = if platform == Platform::MacOs {
        "Use modifiers such as \u{2303} Control or \u{2325} Option, then one key, as in \
         \u{2325}Space."
    } else {
        "Use modifiers such as Ctrl or Alt, then one key, as in Alt+Space."
    };
    AppError::new("invalid_keys", hint)
}

/// Whether `keys`, as manifests write them, hold a modifier other than
/// Shift or end in a function key, so typing never presses them.
fn holds_a_modifier(keys: &str) -> bool {
    let parts: Vec<&str> = keys.split('+').collect();
    let Some((key, modifiers)) = parts.split_last() else {
        return false;
    };
    let function = key
        .strip_prefix('F')
        .is_some_and(|number| number.parse::<u8>().is_ok());
    function || modifiers.iter().any(|modifier| *modifier != "Shift")
}

/// `keys`, as manifests write them, as the key press that presses them,
/// for keys Noura's own shortcuts can use.
fn key_press(keys: &str) -> Option<KeyPress> {
    let parts: Vec<&str> = keys.split('+').collect();
    let (key, modifiers) = parts.split_last()?;
    let key = match *key {
        "Space" => Key::Space,
        "Enter" => Key::Enter,
        "Tab" => Key::Tab,
        "Escape" => Key::Escape,
        "Backspace" => Key::Backspace,
        "Delete" => Key::Delete,
        "Up" => Key::ArrowUp,
        "Down" => Key::ArrowDown,
        "Left" => Key::ArrowLeft,
        "Right" => Key::ArrowRight,
        "Home" => Key::Home,
        "End" => Key::End,
        "PageUp" => Key::PageUp,
        "PageDown" => Key::PageDown,
        name => {
            let letters = [
                Key::A,
                Key::B,
                Key::C,
                Key::D,
                Key::E,
                Key::F,
                Key::G,
                Key::H,
                Key::I,
                Key::J,
                Key::K,
                Key::L,
                Key::M,
                Key::N,
                Key::O,
                Key::P,
                Key::Q,
                Key::R,
                Key::S,
                Key::T,
                Key::U,
                Key::V,
                Key::W,
                Key::X,
                Key::Y,
                Key::Z,
            ];
            let functions = [
                Key::F1,
                Key::F2,
                Key::F3,
                Key::F4,
                Key::F5,
                Key::F6,
                Key::F7,
                Key::F8,
                Key::F9,
                Key::F10,
                Key::F11,
                Key::F12,
            ];
            let mut chars = name.chars();
            match (chars.next(), chars.next()) {
                (Some(letter @ 'A'..='Z'), None) => letters[(letter as u8 - b'A') as usize],
                (Some(digit @ '0'..='9'), None) => Key::DIGITS[(digit as u8 - b'0') as usize],
                _ => {
                    let number: usize = name.strip_prefix('F')?.parse().ok()?;
                    *functions.get(number.checked_sub(1)?)?
                }
            }
        }
    };
    let mut press = KeyPress::new(key);
    for modifier in modifiers {
        press = match *modifier {
            "Ctrl" => press.with_ctrl(),
            "Alt" => press.with_alt(),
            "Shift" => press.with_shift(),
            "Cmd" => press.with_cmd(),
            _ => return None,
        };
    }
    Some(press)
}

/// Whether pressing `press` on `platform` presses Noura's shortcut `own`,
/// counting `own`'s A to Z and 1 to 8 as each of their keys.
fn presses(own: &Shortcut, press: &KeyPress, platform: Platform) -> bool {
    let stands_for = match own.key {
        Key::Letters => (Key::A as usize..=Key::Z as usize).contains(&(press.key as usize)),
        Key::OneToEight => {
            (Key::Num1 as usize..=Key::Num8 as usize).contains(&(press.key as usize))
        }
        _ => false,
    };
    let own = if stands_for {
        Shortcut {
            key: press.key,
            ..*own
        }
    } else {
        *own
    };
    own.matches(press, platform)
}

/// Keys the system on `platform` keeps for itself, as manifests write
/// them: on macOS Spotlight, the input sources, the app switcher,
/// screenshots, Force Quit, Lock Screen, the Dock, Hide, Minimize and
/// Mission Control.
fn system_keys(platform: Platform) -> &'static [&'static str] {
    match platform {
        Platform::MacOs => &[
            "Cmd+Space",
            "Alt+Cmd+Space",
            "Ctrl+Space",
            "Ctrl+Alt+Space",
            "Ctrl+Cmd+Space",
            "Cmd+Tab",
            "Shift+Cmd+Tab",
            "Shift+Cmd+3",
            "Shift+Cmd+4",
            "Shift+Cmd+5",
            "Ctrl+Shift+Cmd+3",
            "Ctrl+Shift+Cmd+4",
            "Alt+Cmd+Escape",
            "Ctrl+Cmd+Q",
            "Alt+Cmd+D",
            "Cmd+H",
            "Alt+Cmd+H",
            "Cmd+M",
            "Ctrl+Up",
            "Ctrl+Down",
            "Ctrl+Left",
            "Ctrl+Right",
        ],
        Platform::Windows => &[
            "Alt+Tab",
            "Alt+Shift+Tab",
            "Alt+F4",
            "Alt+Space",
            "Ctrl+Escape",
            "Ctrl+Shift+Escape",
            "Ctrl+Alt+Delete",
            "Cmd+D",
            "Cmd+E",
            "Cmd+L",
            "Cmd+R",
            "Cmd+Tab",
            "Cmd+Space",
        ],
        Platform::Linux => &["Alt+Tab", "Alt+Shift+Tab", "Alt+F4", "Ctrl+Alt+Delete"],
    }
}
