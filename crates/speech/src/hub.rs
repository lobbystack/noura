//! [`Speech`]: the kernel's speech service for every window and plugin.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;

use crate::audio::{AudioBackend, Microphone, Source};
use crate::catalog::{Catalog, CatalogModel, VAD_MODEL};
use crate::chunker::Leveled;
use crate::engine::{EnergyVad, Engine, Model, Options, Vad};
use crate::paste::Paster;
use crate::recording::{Change, Plan, Recording, Snapshot, State};
use crate::state::{DeviceState, KeptAudio, PluginSpeech, StateFile};
use crate::{Result, SpeechError};

/// What the service is built from.
pub struct SpeechConfig {
    /// `speech` under the app data folder.
    pub folder: PathBuf,
    pub engine: Arc<dyn Engine>,
    pub audio: Arc<dyn AudioBackend>,
    pub paster: Arc<dyn Paster>,
    pub catalog: &'static Catalog,
    #[cfg(feature = "download")]
    pub download: crate::download::DownloadOptions,
    /// How often Noura checks whether another app turned the microphone
    /// on. `None` doesn't check.
    pub microphone_watch: Option<Duration>,
}

/// Something the service tells its listeners.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// A plugin's recording changed.
    Recording {
        plugin: String,
        id: String,
        change: Change,
    },
    /// Another app turned the microphone on, or off.
    MicrophoneInUse(bool),
    /// A download moved on, finished or failed, or a model went.
    Models,
}

type Listener = Arc<dyn Fn(Event) + Send + Sync>;

/// A model in the catalog, as Settings and plugins see it.
#[derive(Debug, Clone)]
pub struct ModelRow {
    pub model: &'static CatalogModel,
    pub downloaded: bool,
    pub recommended: bool,
    /// Bytes so far and in all, while it downloads.
    pub downloading: Option<(u64, u64)>,
    /// Why its last download failed.
    pub problem: Option<String>,
}

struct Owned {
    plugin: String,
    recording: Arc<Recording>,
}

/// The speech service.
pub struct Speech {
    config: SpeechConfig,
    file: StateFile,
    state: Mutex<DeviceState>,
    /// Whether this version may write the state file.
    writable: bool,
    recordings: Mutex<BTreeMap<String, Owned>>,
    /// The model loaded last, by ID.
    loaded: Mutex<Option<(String, Arc<dyn Model>)>>,
    #[cfg(feature = "download")]
    downloads: Mutex<BTreeMap<String, Arc<crate::download::Progress>>>,
    problems: Mutex<BTreeMap<String, String>>,
    listeners: Mutex<Vec<Listener>>,
    next: AtomicU64,
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs())
}

impl Speech {
    /// The service, with its files in `config.folder`. Leftover audio of a
    /// session that ended goes, and so does kept audio past its date.
    pub fn open(config: SpeechConfig) -> Arc<Self> {
        let file = StateFile::new(&config.folder);
        let (mut state, writable) = file.read();
        let _ = std::fs::remove_dir_all(config.folder.join("audio"));
        let now = now_seconds();
        let kept_folder = config.folder.join("kept");
        state.kept.retain(|kept| {
            let keep = kept.until > now;
            if !keep {
                let _ = std::fs::remove_file(kept_folder.join(&kept.file));
            }
            keep
        });
        let watch = config.microphone_watch;
        let speech = Arc::new(Self {
            config,
            file,
            state: Mutex::new(state),
            writable,
            recordings: Mutex::new(BTreeMap::new()),
            loaded: Mutex::new(None),
            #[cfg(feature = "download")]
            downloads: Mutex::new(BTreeMap::new()),
            problems: Mutex::new(BTreeMap::new()),
            listeners: Mutex::new(Vec::new()),
            next: AtomicU64::new(1),
        });
        let _ = speech.save();
        if let Some(every) = watch {
            let weak = Arc::downgrade(&speech);
            let _ = std::thread::Builder::new()
                .name("noura-microphone-watch".into())
                .spawn(move || {
                    let mut was = None;
                    loop {
                        std::thread::sleep(every);
                        let Some(speech) = weak.upgrade() else {
                            return;
                        };
                        speech.check_microphone(&mut was);
                    }
                });
        }
        speech
    }

    /// Hear about recordings, the microphone and downloads.
    pub fn subscribe(&self, listener: impl Fn(Event) + Send + Sync + 'static) {
        self.listeners.lock().push(Arc::new(listener));
    }

    fn tell(&self, event: Event) {
        let listeners: Vec<Listener> = self.listeners.lock().clone();
        for listener in listeners {
            listener(event.clone());
        }
    }

    /// Check once whether another app turned the microphone on.
    pub fn check_microphone(&self, was: &mut Option<bool>) {
        let recording_mic = self.recordings.lock().values().any(|owned| {
            !owned.recording.snapshot().state.ended()
                && owned
                    .recording
                    .audio_files()
                    .iter()
                    .any(|(source, _)| *source == Source::Microphone)
        });
        // Our own recording keeps the microphone running; that's no call.
        let now = if recording_mic {
            None
        } else {
            self.config.audio.microphone_in_use()
        };
        if let Some(now) = now
            && *was != Some(now)
        {
            if was.is_some() || now {
                self.tell(Event::MicrophoneInUse(now));
            }
            *was = Some(now);
        }
    }

    fn save(&self) -> Result<()> {
        if !self.writable {
            return Ok(());
        }
        let state = self.state.lock().clone();
        self.file.write(&state)
    }

    fn update(&self, change: impl FnOnce(&mut DeviceState)) -> Result<()> {
        change(&mut self.state.lock());
        self.save()
    }

    pub fn state(&self) -> DeviceState {
        self.state.lock().clone()
    }

    // Models.

    pub fn catalog(&self) -> &'static Catalog {
        self.config.catalog
    }

    pub fn models_folder(&self) -> PathBuf {
        self.config.folder.join("models")
    }

    fn model_path(&self, model: &CatalogModel) -> PathBuf {
        self.models_folder().join(&model.file.filename)
    }

    pub fn is_downloaded(&self, model: &CatalogModel) -> bool {
        self.model_path(model).is_file()
    }

    /// The languages you speak: the device's language, or English.
    fn languages(&self, language: Option<&str>) -> Vec<String> {
        vec![
            language
                .map(str::to_owned)
                .or_else(|| self.state.lock().language.clone())
                .unwrap_or_else(|| "en".to_owned()),
        ]
    }

    /// Every model, downloaded ones first, then the curated ones.
    pub fn models(&self, language: Option<&str>) -> Vec<ModelRow> {
        let recommended = self
            .catalog()
            .recommended(&self.languages(language))
            .map(|model| model.id.clone());
        let problems = self.problems.lock().clone();
        let mut rows: Vec<ModelRow> = self
            .catalog()
            .models()
            .iter()
            .map(|model| ModelRow {
                model,
                downloaded: self.is_downloaded(model),
                recommended: recommended.as_deref() == Some(model.id.as_str()),
                downloading: self.download_progress(&model.id),
                problem: problems.get(&model.id).cloned(),
            })
            .collect();
        rows.sort_by_key(|row| (!row.downloaded, !row.recommended, !row.model.curated));
        rows
    }

    #[cfg(feature = "download")]
    fn download_progress(&self, id: &str) -> Option<(u64, u64)> {
        self.downloads.lock().get(id).map(|progress| {
            (
                progress.received.load(Ordering::SeqCst),
                progress.total.load(Ordering::SeqCst),
            )
        })
    }

    #[cfg(not(feature = "download"))]
    fn download_progress(&self, _id: &str) -> Option<(u64, u64)> {
        None
    }

    /// Start downloading the model `id`, with the voice activity model a
    /// model that can't stream needs, on a thread of its own.
    #[cfg(feature = "download")]
    pub fn download(self: &Arc<Self>, id: &str) -> Result<()> {
        let model = self
            .catalog()
            .model(id)
            .ok_or_else(|| SpeechError::not_found(id))?;
        let progress = {
            let mut downloads = self.downloads.lock();
            if downloads.contains_key(id) {
                return Ok(());
            }
            let progress = Arc::new(crate::download::Progress::default());
            downloads.insert(id.to_owned(), progress.clone());
            progress
        };
        self.problems.lock().remove(id);
        let mut files = vec![model.clone()];
        if !model.streaming
            && let Some(vad) = self.catalog().helper(VAD_MODEL)
            && !self.is_downloaded(vad)
        {
            files.insert(0, vad.clone());
        }
        let speech = self.clone();
        let id = id.to_owned();
        std::thread::Builder::new()
            .name("noura-model-download".into())
            .spawn(move || {
                let mut outcome = Ok(());
                for file in &files {
                    outcome = crate::download::download(
                        &file.file,
                        &speech.model_path(file),
                        &speech.config.download,
                        &progress,
                    );
                    if outcome.is_err() {
                        break;
                    }
                }
                speech.downloads.lock().remove(&id);
                if let Err(error) = outcome
                    && error.code != "cancelled"
                {
                    speech.problems.lock().insert(id, error.message);
                }
                speech.tell(Event::Models);
            })
            .map_err(|error| SpeechError::io(&error, "download thread"))?;
        self.tell(Event::Models);
        Ok(())
    }

    /// Stop the download of `id`, keeping what came for the next one.
    #[cfg(feature = "download")]
    pub fn cancel_download(&self, id: &str) {
        if let Some(progress) = self.downloads.lock().get(id) {
            progress.cancel();
        }
    }

    /// Delete the model `id` from this device.
    pub fn delete_model(&self, id: &str) -> Result<()> {
        let model = self
            .catalog()
            .model(id)
            .ok_or_else(|| SpeechError::not_found(id))?;
        if self
            .loaded
            .lock()
            .as_ref()
            .is_some_and(|(loaded, _)| loaded == id)
        {
            *self.loaded.lock() = None;
        }
        let path = self.model_path(model);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(SpeechError::io(&error, "model delete")),
        }
        #[cfg(feature = "download")]
        let _ = std::fs::remove_file(crate::download::part_path(&path));
        if self.state.lock().model.as_deref() == Some(id) {
            self.update(|state| state.model = None)?;
        }
        self.tell(Event::Models);
        Ok(())
    }

    fn load(&self, model: &CatalogModel) -> Result<Arc<dyn Model>> {
        if let Some((id, loaded)) = self.loaded.lock().as_ref()
            && *id == model.id
        {
            return Ok(loaded.clone());
        }
        if !self.is_downloaded(model) {
            return Err(SpeechError::failed(
                "model_missing",
                &format!("Download {} in Settings first.", model.name),
            ));
        }
        let began = std::time::Instant::now();
        let loaded = self
            .config
            .engine
            .load(&self.model_path(model))
            .inspect_err(|error| {
                tracing::warn!(model = %model.id, code = %error.code, "a speech model didn't load");
            })?;
        tracing::info!(
            model = %model.id,
            streams = loaded.streams(),
            took_ms = began.elapsed().as_millis() as u64,
            "loaded a speech model"
        );
        *self.loaded.lock() = Some((model.id.clone(), loaded.clone()));
        Ok(loaded)
    }

    // Choices.

    pub fn microphones(&self) -> Result<Vec<Microphone>> {
        self.config.audio.microphones()
    }

    pub fn set_language(&self, language: Option<String>) -> Result<()> {
        self.update(|state| state.language = language)
    }

    /// The model in use for plugins without a choice of their own: the
    /// one you picked, the recommendation, or any downloaded model, as
    /// long as it's on this device.
    pub fn selected_model(&self) -> Option<&'static CatalogModel> {
        let catalog = self.catalog();
        let picked = self.state.lock().model.clone();
        picked
            .and_then(|id| catalog.model(&id))
            .filter(|model| self.is_downloaded(model))
            .or_else(|| {
                catalog
                    .recommended(&self.languages(None))
                    .filter(|model| self.is_downloaded(model))
            })
            .or_else(|| {
                catalog
                    .models()
                    .iter()
                    .find(|model| self.is_downloaded(model))
            })
    }

    /// Use the downloaded model `id` wherever no plugin chose another,
    /// and drop the plugins' own choices.
    pub fn select_model(&self, id: &str) -> Result<()> {
        let model = self
            .catalog()
            .model(id)
            .ok_or_else(|| SpeechError::not_found(id))?;
        if !self.is_downloaded(model) {
            return Err(SpeechError::failed(
                "model_missing",
                &format!("Download {} first.", model.name),
            ));
        }
        self.update(|state| {
            state.model = Some(id.to_owned());
            for plugin in state.plugins.values_mut() {
                plugin.model = None;
            }
        })
    }

    /// `plugin`'s choices. Without a model of its own, it gets the one you
    /// picked in Settings, while that one is on this device.
    pub fn preferences(&self, plugin: &str) -> PluginSpeech {
        let (mut preferences, picked) = {
            let state = self.state.lock();
            (
                state.plugins.get(plugin).cloned().unwrap_or_default(),
                state.model.clone(),
            )
        };
        if preferences.model.is_none() {
            preferences.model = picked.filter(|id| {
                self.catalog()
                    .model(id)
                    .is_some_and(|model| self.is_downloaded(model))
            });
        }
        preferences
    }

    /// Keep `plugin`'s choices, once the model is downloaded and the
    /// microphone connected. The last dictation stays.
    pub fn set_preferences(&self, plugin: &str, preferences: PluginSpeech) -> Result<()> {
        if let Some(id) = &preferences.model {
            let model = self
                .catalog()
                .model(id)
                .ok_or_else(|| SpeechError::failed("model_unknown", "Noura has no such model."))?;
            if !self.is_downloaded(model) {
                return Err(SpeechError::failed(
                    "model_missing",
                    &format!("Download {} in Settings first.", model.name),
                ));
            }
        }
        if let Some(microphone) = &preferences.microphone
            && !self
                .microphones()?
                .iter()
                .any(|known| &known.id == microphone)
        {
            return Err(SpeechError::failed(
                "microphone_missing",
                "That microphone isn't connected.",
            ));
        }
        self.update(|state| {
            // The model you picked in Settings stays the plugin's until
            // the plugin picks another.
            let model = preferences
                .model
                .filter(|id| state.model.as_ref() != Some(id));
            let entry = state.plugins.entry(plugin.to_owned()).or_default();
            let last = entry.last_dictation.take();
            *entry = PluginSpeech {
                model,
                last_dictation: last,
                ..preferences
            };
        })
    }

    pub fn remember_dictation(&self, plugin: &str, text: &str) -> Result<()> {
        self.update(|state| {
            state
                .plugins
                .entry(plugin.to_owned())
                .or_default()
                .last_dictation = Some(text.to_owned());
        })
    }

    pub fn last_dictation(&self, plugin: &str) -> Option<String> {
        self.preferences(plugin).last_dictation
    }

    /// The keys of `plugin`'s shortcut `shortcut`: the ones you set, or
    /// `declared`.
    pub fn shortcut_keys(&self, plugin: &str, shortcut: &str, declared: &str) -> String {
        self.state
            .lock()
            .shortcuts
            .get(&format!("{plugin}/{shortcut}"))
            .cloned()
            .unwrap_or_else(|| declared.to_owned())
    }

    /// Give `plugin`'s shortcut `shortcut` the keys `keys`, or its declared
    /// ones back with `None`.
    pub fn set_shortcut_keys(
        &self,
        plugin: &str,
        shortcut: &str,
        keys: Option<String>,
    ) -> Result<()> {
        let key = format!("{plugin}/{shortcut}");
        self.update(|state| match keys {
            Some(keys) => {
                state.shortcuts.insert(key, keys);
            }
            None => {
                state.shortcuts.remove(&key);
            }
        })
    }

    // Recordings.

    /// The model a recording of `plugin` uses: the one asked for, the
    /// plugin's choice, the recommendation, or any downloaded model.
    fn choose_model(
        &self,
        plugin: &str,
        asked: Option<&str>,
        language: Option<&str>,
    ) -> Result<&'static CatalogModel> {
        let catalog = self.catalog();
        let preferred = asked
            .map(str::to_owned)
            .or_else(|| self.preferences(plugin).model);
        if let Some(id) = preferred {
            return catalog
                .model(&id)
                .ok_or_else(|| SpeechError::failed("model_unknown", "Noura has no such model."));
        }
        if let Some(model) = catalog.recommended(&self.languages(language))
            && self.is_downloaded(model)
        {
            return Ok(model);
        }
        catalog
            .models()
            .iter()
            .find(|model| self.is_downloaded(model))
            .ok_or_else(|| {
                SpeechError::failed(
                    "model_missing",
                    "Download a speech model in Settings first.",
                )
            })
    }

    /// Start a recording of `sources` for `plugin`.
    pub fn start(
        self: &Arc<Self>,
        plugin: &str,
        sources: Vec<Source>,
        model: Option<&str>,
        microphone: Option<&str>,
        language: Option<&str>,
    ) -> Result<String> {
        let preferences = self.preferences(plugin);
        let language = language
            .map(str::to_owned)
            .or(preferences.language.clone())
            .or_else(|| self.state.lock().language.clone());
        let chosen = self
            .choose_model(plugin, model, language.as_deref())
            .inspect_err(|error| {
                tracing::warn!(plugin, code = %error.code, "no speech model for a recording");
            })?;
        let loaded = self.load(chosen)?;
        let vad_path = self
            .catalog()
            .helper(VAD_MODEL)
            .map(|vad| self.model_path(vad))
            .filter(|path| path.is_file());
        tracing::info!(
            plugin,
            model = %chosen.id,
            ?sources,
            voice_activity_model = vad_path.is_some(),
            "starting a recording"
        );
        let engine = self.config.engine.clone();
        let id = format!("recording_{}", self.next.fetch_add(1, Ordering::SeqCst));
        let speech = Arc::downgrade(self);
        let owner = plugin.to_owned();
        let recording = Recording::start(
            Plan {
                id: id.clone(),
                sources,
                microphone: microphone.map(str::to_owned).or(preferences.microphone),
                options: Options { language },
                model: loaded,
                vad: Box::new(move || {
                    vad_path
                        .as_ref()
                        .and_then(|path| match engine.load_vad(path) {
                            Ok(vad) => Some(Box::new(Leveled::new(vad)) as Box<dyn Vad>),
                            Err(error) => {
                                tracing::warn!(code = %error.code, "the voice activity model didn't load");
                                None
                            }
                        })
                        .unwrap_or_else(|| Box::new(EnergyVad::default()))
                }),
                audio_folder: self.config.folder.join("audio"),
            },
            self.config.audio.as_ref(),
            Arc::new(move |id: &str, change| {
                if let Some(speech) = speech.upgrade() {
                    speech.tell(Event::Recording {
                        plugin: owner.clone(),
                        id: id.to_owned(),
                        change,
                    });
                }
            }),
        )?;
        self.recordings.lock().insert(
            id.clone(),
            Owned {
                plugin: plugin.to_owned(),
                recording,
            },
        );
        Ok(id)
    }

    fn owned(&self, plugin: &str, id: &str) -> Result<Arc<Recording>> {
        self.recordings
            .lock()
            .get(id)
            .filter(|owned| owned.plugin == plugin)
            .map(|owned| owned.recording.clone())
            .ok_or_else(|| SpeechError::not_found(id))
    }

    /// The plugins with a recording that hasn't ended, each once.
    pub fn recording_plugins(&self) -> Vec<String> {
        let mut plugins: Vec<String> = self
            .recordings
            .lock()
            .values()
            .filter(|owned| !owned.recording.snapshot().state.ended())
            .map(|owned| owned.plugin.clone())
            .collect();
        plugins.sort();
        plugins.dedup();
        plugins
    }

    pub fn read(&self, plugin: &str, id: &str) -> Result<Snapshot> {
        Ok(self.owned(plugin, id)?.snapshot())
    }

    pub fn stop(&self, plugin: &str, id: &str) -> Result<()> {
        self.owned(plugin, id)?.stop();
        Ok(())
    }

    pub fn cancel(&self, plugin: &str, id: &str) -> Result<()> {
        self.owned(plugin, id)?.cancel();
        Ok(())
    }

    fn finished(&self, plugin: &str, id: &str) -> Result<Arc<Recording>> {
        let recording = self.owned(plugin, id)?;
        if recording.snapshot().state != State::Finished {
            return Err(SpeechError::failed(
                "not_finished",
                "The recording hasn't finished yet.",
            ));
        }
        Ok(recording)
    }

    /// A finished recording's audio as WAV bytes.
    pub fn wav(&self, plugin: &str, id: &str) -> Result<Vec<u8>> {
        self.finished(plugin, id)?.wav_bytes()
    }

    /// Keep a finished recording's audio for `days` days.
    pub fn keep_audio(&self, plugin: &str, id: &str, days: u32) -> Result<()> {
        let recording = self.finished(plugin, id)?;
        let folder = self.config.folder.join("kept");
        std::fs::create_dir_all(&folder).map_err(|error| SpeechError::io(&error, "kept folder"))?;
        let name = format!("{plugin}-{id}.wav");
        std::fs::write(folder.join(&name), recording.wav_bytes()?)
            .map_err(|error| SpeechError::io(&error, "kept audio"))?;
        let until = now_seconds() + u64::from(days) * 86_400;
        self.update(|state| {
            state.kept.retain(|kept| kept.file != name);
            state.kept.push(KeptAudio { file: name, until });
        })
    }

    /// Forget a recording that ended, or stop and forget one that didn't.
    pub fn forget(&self, plugin: &str, id: &str) -> Result<()> {
        let recording = self.owned(plugin, id)?;
        if !recording.snapshot().state.ended() {
            recording.cancel();
        }
        recording.delete_audio();
        self.recordings.lock().remove(id);
        Ok(())
    }

    /// Stop and forget every recording of `plugin`, as when it turns off.
    pub fn forget_plugin(&self, plugin: &str) {
        let ids: Vec<String> = self
            .recordings
            .lock()
            .iter()
            .filter(|(_, owned)| owned.plugin == plugin)
            .map(|(id, _)| id.clone())
            .collect();
        for id in ids {
            let _ = self.forget(plugin, &id);
        }
    }

    // Pasting.

    pub fn paste(&self, text: &str, typing: bool) -> Result<()> {
        self.config.paster.paste(text, typing)
    }

    pub fn copy(&self, text: &str) -> Result<()> {
        self.config.paster.copy(text)
    }
}

impl Drop for Speech {
    fn drop(&mut self) {
        for owned in self.recordings.lock().values() {
            owned.recording.cancel();
        }
        let _ = std::fs::remove_dir_all(self.config.folder.join("audio"));
    }
}
