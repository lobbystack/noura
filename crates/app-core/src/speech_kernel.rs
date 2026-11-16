//! Speech for plugins (ADR 0006): the `speech`, `global-shortcuts`,
//! `paste` and `menu-bar` interfaces, served from the kernel's speech
//! service in `crates/speech`, and the kernel events that answer them.
//!
//! Every window shares one [`SpeechKernel`]. Its recordings, models and
//! choices live in `speech` under the app data folder, never in the
//! workspace. A shortcut press, a menu item you choose and the microphone
//! reach the running plugin that declared them as kernel events, which
//! count as something you did, so the plugin may paste in answer. Noura
//! opens one workspace at a time, so "the last-used workspace that has
//! the plugin on" is the open workspace when the plugin runs in it.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use plugin_host::{
    Capability, KernelEvent, MenuBarEntry, MenuBarService, MicrophoneUse, PasteMethod,
    PasteService, Recording, RecordingOptions, RecordingState, ServiceError, ShortcutDeclaration,
    ShortcutService, ShortcutState, SpeechChannel, SpeechMicrophone, SpeechModel,
    SpeechPreferences, SpeechSegment, SpeechService, SpeechSource, WorkspacePath,
};
use speech::audio::{AudioBackend, FakeAudio, FakeBackend, Source};
use speech::engine::{Engine, FakeEngine};
use speech::paste::Paster;
use speech::recording::{Change, State};
use speech::state::PluginSpeech;
use speech::{Event, Speech, SpeechConfig, SpeechError};

use crate::Workspace;
use crate::plugins::navigation::Navigator;

/// The devices speech runs on: the real ones in the desktop app, fakes
/// that record nothing elsewhere.
pub struct SpeechDevices {
    pub engine: Arc<dyn Engine>,
    pub audio: Arc<dyn AudioBackend>,
    pub paster: Arc<dyn Paster>,
    /// How often to check whether another app turned the microphone on.
    pub microphone_watch: Option<Duration>,
}

impl SpeechDevices {
    /// Devices that hear nothing, for tests and other clients.
    pub fn fake() -> Self {
        Self {
            engine: Arc::new(FakeEngine::default()),
            audio: Arc::new(FakeBackend(FakeAudio::new())),
            paster: Arc::new(NoPaste),
            microphone_watch: None,
        }
    }
}

/// A paster that refuses, where Noura can't press keys.
pub struct NoPaste;

impl Paster for NoPaste {
    fn paste(&self, _text: &str, _typing: bool) -> speech::Result<()> {
        Err(SpeechError::failed(
            "paste_unavailable",
            "Noura can't paste here.",
        ))
    }

    fn copy(&self, _text: &str) -> speech::Result<()> {
        Err(SpeechError::failed(
            "paste_unavailable",
            "Noura can't copy here.",
        ))
    }
}

/// A shortcut a running plugin declared, with the keys it answers to now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalShortcut {
    pub plugin: String,
    pub id: String,
    pub title: String,
    pub keys: String,
    /// The keys the manifest declares.
    pub declared: String,
}

/// One plugin's menu bar entries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuBarItems {
    pub plugin: String,
    pub entries: Vec<MenuBarEntry>,
}

/// What windows read and plugins reach: the speech service, the menu bar
/// entries and the shortcuts the system took.
pub(crate) struct SpeechKernel {
    pub(crate) speech: Arc<Speech>,
    menus: Mutex<BTreeMap<String, Vec<MenuBarEntry>>>,
    registered: Mutex<BTreeMap<(String, String), bool>>,
    /// Kernel events for plugins, and plugins to draw again, until the
    /// plugin service takes them.
    pending: Mutex<Vec<(String, Option<KernelEvent>)>>,
    /// Counts the changes Settings and the menu bar show.
    version: AtomicU64,
    wake: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
}

impl SpeechKernel {
    pub(crate) fn new(app_data: &std::path::Path, devices: SpeechDevices) -> Arc<Self> {
        let speech = Speech::open(SpeechConfig {
            folder: app_data.join("speech"),
            engine: devices.engine,
            audio: devices.audio,
            paster: devices.paster,
            catalog: speech::catalog::catalog(),
            download: speech::download::DownloadOptions::default(),
            microphone_watch: devices.microphone_watch,
        });
        let kernel = Arc::new(Self {
            speech,
            menus: Mutex::new(BTreeMap::new()),
            registered: Mutex::new(BTreeMap::new()),
            pending: Mutex::new(Vec::new()),
            version: AtomicU64::new(0),
            wake: Mutex::new(None),
        });
        let weak = Arc::downgrade(&kernel);
        kernel.speech.subscribe(move |event| {
            if let Some(kernel) = weak.upgrade() {
                kernel.heard(event);
            }
        });
        kernel
    }

    pub(crate) fn set_wake(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        *self.wake.lock() = Some(wake);
    }

    fn wake(&self) {
        let wake = self.wake.lock().clone();
        if let Some(wake) = wake {
            wake();
        }
    }

    pub(crate) fn changed(&self) {
        self.version.fetch_add(1, Ordering::SeqCst);
        self.wake();
    }

    pub(crate) fn version(&self) -> u64 {
        self.version.load(Ordering::SeqCst)
    }

    fn heard(&self, event: Event) {
        match event {
            Event::Recording { plugin, id, change } => {
                let event = (change == Change::State).then_some(KernelEvent::RecordingChanged(id));
                self.pending.lock().push((plugin, event));
            }
            Event::MicrophoneInUse(on) => {
                let event = if on {
                    KernelEvent::MicrophoneInUse(MicrophoneUse { app: None })
                } else {
                    KernelEvent::MicrophoneFree
                };
                // Every plugin that holds `speech` hears it.
                self.pending.lock().push((String::new(), Some(event)));
            }
            Event::Models => self.changed(),
        }
        self.wake();
    }

    /// Take the queued events, and plugins to draw again (`None`).
    pub(crate) fn take(&self) -> Vec<(String, Option<KernelEvent>)> {
        std::mem::take(&mut *self.pending.lock())
    }

    /// The plugin `plugin` stopped: its recordings and menu entries go.
    pub(crate) fn forget_plugin(&self, plugin: &str) {
        self.speech.forget_plugin(plugin);
        if self.menus.lock().remove(plugin).is_some() {
            self.changed();
        }
        self.registered
            .lock()
            .retain(|(owner, _), _| owner != plugin);
    }

    pub(crate) fn menus(&self) -> Vec<MenuBarItems> {
        self.menus
            .lock()
            .iter()
            .map(|(plugin, entries)| MenuBarItems {
                plugin: plugin.clone(),
                entries: entries.clone(),
            })
            .collect()
    }

    pub(crate) fn set_registered(&self, plugin: &str, id: &str, registered: bool) {
        self.registered
            .lock()
            .insert((plugin.to_owned(), id.to_owned()), registered);
    }

    /// The services one plugin reaches.
    pub(crate) fn services(
        self: &Arc<Self>,
        plugin: &str,
        shortcuts: Vec<ShortcutDeclaration>,
        navigator: Arc<Navigator>,
        workspace: Arc<Workspace>,
    ) -> PluginSpeechServices {
        let access = Arc::new(Access {
            plugin: plugin.to_owned(),
            kernel: self.clone(),
            shortcuts,
            navigator,
            workspace,
        });
        PluginSpeechServices {
            speech: access.clone(),
            shortcuts: access.clone(),
            paste: access.clone(),
            menu_bar: access,
        }
    }
}

/// The services of one plugin.
pub(crate) struct PluginSpeechServices {
    pub(crate) speech: Arc<dyn SpeechService>,
    pub(crate) shortcuts: Arc<dyn ShortcutService>,
    pub(crate) paste: Arc<dyn PasteService>,
    pub(crate) menu_bar: Arc<dyn MenuBarService>,
}

/// The shortcuts `approved` grants.
pub(crate) fn approved_shortcuts(approved: &[Capability]) -> Vec<ShortcutDeclaration> {
    Capability::shortcuts(approved)
}

struct Access {
    plugin: String,
    kernel: Arc<SpeechKernel>,
    shortcuts: Vec<ShortcutDeclaration>,
    navigator: Arc<Navigator>,
    workspace: Arc<Workspace>,
}

fn service_error(error: SpeechError) -> ServiceError {
    match error.code.as_str() {
        "not_found" => ServiceError::NotFound(error.message),
        _ => ServiceError::Failed(error.message),
    }
}

fn channel(source: Source) -> SpeechChannel {
    match source {
        Source::Microphone => SpeechChannel::Me,
        Source::SystemAudio => SpeechChannel::Others,
    }
}

fn segment(segment: &speech::recording::Segment) -> SpeechSegment {
    SpeechSegment {
        channel: channel(segment.channel),
        start_ms: segment.start_ms,
        end_ms: segment.end_ms,
        text: segment.text.clone(),
    }
}

impl SpeechService for Access {
    fn models(&self) -> Result<Vec<SpeechModel>, ServiceError> {
        let language = self.kernel.speech.preferences(&self.plugin).language;
        Ok(self
            .kernel
            .speech
            .models(language.as_deref())
            .into_iter()
            .map(|row| SpeechModel {
                id: row.model.id.clone(),
                name: row.model.name.clone(),
                description: row.model.description.clone(),
                languages: row.model.languages.clone(),
                size: row.model.file.size,
                streaming: row.model.streaming,
                translation: row.model.translation,
                downloaded: row.downloaded,
                recommended: row.recommended,
            })
            .collect())
    }

    fn microphones(&self) -> Result<Vec<SpeechMicrophone>, ServiceError> {
        Ok(self
            .kernel
            .speech
            .microphones()
            .map_err(service_error)?
            .into_iter()
            .map(|microphone| SpeechMicrophone {
                id: microphone.id,
                name: microphone.name,
                is_default: microphone.is_default,
            })
            .collect())
    }

    fn preferences(&self) -> Result<SpeechPreferences, ServiceError> {
        let preferences = self.kernel.speech.preferences(&self.plugin);
        Ok(SpeechPreferences {
            model: preferences.model,
            microphone: preferences.microphone,
            system_audio: preferences.system_audio,
            language: preferences.language,
        })
    }

    fn set_preferences(&self, preferences: SpeechPreferences) -> Result<(), ServiceError> {
        self.kernel
            .speech
            .set_preferences(
                &self.plugin,
                PluginSpeech {
                    model: preferences.model,
                    microphone: preferences.microphone,
                    system_audio: preferences.system_audio,
                    language: preferences.language,
                    last_dictation: None,
                },
            )
            .map_err(service_error)
    }

    fn start(&self, options: RecordingOptions) -> Result<String, ServiceError> {
        let mut sources: Vec<Source> = options
            .sources
            .iter()
            .map(|source| match source {
                SpeechSource::Microphone => Source::Microphone,
                SpeechSource::SystemAudio => Source::SystemAudio,
            })
            .collect();
        sources.sort();
        sources.dedup();
        self.kernel
            .speech
            .start(
                &self.plugin,
                sources,
                options.model.as_deref(),
                options.microphone.as_deref(),
                options.language.as_deref(),
            )
            .map_err(service_error)
    }

    fn read_recording(&self, id: &str) -> Result<Recording, ServiceError> {
        let snapshot = self
            .kernel
            .speech
            .read(&self.plugin, id)
            .map_err(service_error)?;
        Ok(Recording {
            id: snapshot.id,
            state: match snapshot.state {
                State::Recording => RecordingState::Recording,
                State::Finishing => RecordingState::Finishing,
                State::Finished => RecordingState::Finished,
                State::Failed => RecordingState::Failed,
                State::Cancelled => RecordingState::Cancelled,
            },
            elapsed_ms: snapshot.elapsed_ms,
            segments: snapshot.segments.iter().map(segment).collect(),
            pending: snapshot.pending.iter().map(segment).collect(),
            failure: snapshot.failure,
        })
    }

    fn stop(&self, id: &str) -> Result<(), ServiceError> {
        self.kernel
            .speech
            .stop(&self.plugin, id)
            .map_err(service_error)
    }

    fn cancel(&self, id: &str) -> Result<(), ServiceError> {
        self.kernel
            .speech
            .cancel(&self.plugin, id)
            .map_err(service_error)
    }

    /// Write the audio beside the object `object`, named after its file,
    /// as `Standup.wav` beside `Standup.md`, with a number when taken.
    fn save_audio(&self, id: &str, object: &str) -> Result<WorkspacePath, ServiceError> {
        let bytes = self
            .kernel
            .speech
            .wav(&self.plugin, id)
            .map_err(service_error)?;
        let engine = self.workspace.engine();
        let path = engine
            .read_object(object)
            .map_err(|_| ServiceError::NotFound(object.to_owned()))?
            .relative_path;
        let stem = path.strip_suffix(".md").unwrap_or(&path).to_owned();
        for number in 0..100 {
            let candidate = if number == 0 {
                format!("{stem}.wav")
            } else {
                format!("{stem} {number}.wav")
            };
            match engine.create_file(&candidate, &bytes) {
                Ok(()) => return WorkspacePath::new(&candidate),
                Err(error) if error.code == "path_exists" => continue,
                Err(error) => {
                    tracing::warn!(code = %error.code, "a recording's audio didn't save");
                    return Err(ServiceError::Failed(
                        "Noura couldn't save the audio in the workspace.".to_owned(),
                    ));
                }
            }
        }
        Err(ServiceError::Failed(
            "Noura couldn't find a free name for the audio.".to_owned(),
        ))
    }

    fn keep_audio(&self, id: &str, days: u32) -> Result<(), ServiceError> {
        self.kernel
            .speech
            .keep_audio(&self.plugin, id, days)
            .map_err(service_error)
    }

    fn forget(&self, id: &str) -> Result<(), ServiceError> {
        self.kernel
            .speech
            .forget(&self.plugin, id)
            .map_err(service_error)
    }

    fn remember_dictation(&self, text: &str) -> Result<(), ServiceError> {
        self.kernel
            .speech
            .remember_dictation(&self.plugin, text)
            .map_err(service_error)
    }

    fn last_dictation(&self) -> Result<Option<String>, ServiceError> {
        Ok(self.kernel.speech.last_dictation(&self.plugin))
    }
}

impl ShortcutService for Access {
    fn shortcuts(&self) -> Result<Vec<ShortcutState>, ServiceError> {
        let registered = self.kernel.registered.lock();
        Ok(self
            .shortcuts
            .iter()
            .map(|shortcut| ShortcutState {
                id: shortcut.id.clone(),
                title: shortcut.title.clone(),
                keys: self
                    .kernel
                    .speech
                    .shortcut_keys(&self.plugin, &shortcut.id, &shortcut.keys),
                registered: registered
                    .get(&(self.plugin.clone(), shortcut.id.clone()))
                    .copied()
                    .unwrap_or(false),
            })
            .collect())
    }
}

impl PasteService for Access {
    fn paste(&self, text: &str, method: PasteMethod) -> Result<(), ServiceError> {
        if !self.navigator.answering() {
            return Err(ServiceError::Failed(
                "Noura pastes only when you act.".to_owned(),
            ));
        }
        self.kernel
            .speech
            .paste(text, matches!(method, PasteMethod::Typing))
            .map_err(service_error)
    }

    fn copy(&self, text: &str) -> Result<(), ServiceError> {
        self.kernel.speech.copy(text).map_err(service_error)
    }
}

impl MenuBarService for Access {
    fn set_entries(&self, entries: Vec<MenuBarEntry>) -> Result<(), ServiceError> {
        let mut menus = self.kernel.menus.lock();
        if entries.is_empty() {
            menus.remove(&self.plugin);
        } else {
            menus.insert(self.plugin.clone(), entries);
        }
        drop(menus);
        self.kernel.changed();
        Ok(())
    }
}

/// A plugin's speech folder path, for Settings' "Show in Finder".
pub(crate) fn models_folder(kernel: &SpeechKernel) -> PathBuf {
    kernel.speech.models_folder()
}
