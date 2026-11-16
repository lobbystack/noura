//! This device's speech choices, in `speech/state.json` under the app data
//! folder: the language you speak, the keys you gave each shortcut, each
//! plugin's model, microphone and system audio choice, the last dictation
//! of each plugin, and the recordings whose audio a plugin kept. None of it
//! is workspace content, so none of it syncs.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::{Result, SpeechError};

const VERSION: u32 = 1;

/// One plugin's speech choices on this device.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PluginSpeech {
    pub model: Option<String>,
    pub microphone: Option<String>,
    pub system_audio: bool,
    pub language: Option<String>,
    pub last_dictation: Option<String>,
}

/// A recording's audio a plugin kept, and when Noura deletes it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptAudio {
    /// The file's name in `speech/kept`.
    pub file: String,
    /// Seconds since the Unix epoch.
    pub until: u64,
}

/// Everything in the file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DeviceState {
    pub version: u32,
    /// The language you speak, as an ISO 639-1 code.
    pub language: Option<String>,
    /// Keys you gave a shortcut, by `<plugin>/<shortcut>`.
    pub shortcuts: BTreeMap<String, String>,
    pub plugins: BTreeMap<String, PluginSpeech>,
    pub kept: Vec<KeptAudio>,
    /// Whether you finished the speech onboarding.
    pub onboarded: bool,
    /// Paste by typing key by key instead of through the clipboard.
    pub paste_by_typing: bool,
}

impl Default for DeviceState {
    fn default() -> Self {
        Self {
            version: VERSION,
            language: None,
            shortcuts: BTreeMap::new(),
            plugins: BTreeMap::new(),
            kept: Vec::new(),
            onboarded: false,
            paste_by_typing: false,
        }
    }
}

/// The file, and the state it held when read last.
pub struct StateFile {
    path: PathBuf,
}

impl StateFile {
    pub fn new(speech_folder: &Path) -> Self {
        Self {
            path: speech_folder.join("state.json"),
        }
    }

    /// The state in the file. A file Noura can't read moves aside, to the
    /// same name with `.unreadable` added, and Noura starts again from the
    /// defaults. A file from a newer version is left alone and read as the
    /// defaults, so this version never overwrites it.
    pub fn read(&self) -> (DeviceState, bool) {
        let bytes = match std::fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(_) => return (DeviceState::default(), true),
        };
        match serde_json::from_slice::<DeviceState>(&bytes) {
            Ok(state) if state.version == VERSION => (state, true),
            Ok(state) if state.version > VERSION => {
                tracing::warn!(version = state.version, "speech state from a newer Noura");
                (DeviceState::default(), false)
            }
            _ => {
                let aside = self.path.with_extension("json.unreadable");
                if let Err(error) = std::fs::rename(&self.path, &aside) {
                    tracing::warn!(%error, "the unreadable speech state couldn't move aside");
                }
                (DeviceState::default(), true)
            }
        }
    }

    /// Write `state` in place of the file, durably.
    pub fn write(&self, state: &DeviceState) -> Result<()> {
        let folder = self.path.parent().unwrap_or(Path::new("."));
        std::fs::create_dir_all(folder).map_err(|error| SpeechError::io(&error, "state folder"))?;
        let bytes = serde_json::to_vec_pretty(state)
            .map_err(|_| SpeechError::failed("io", "Noura couldn't save its speech choices."))?;
        let mut file = atomic_write_file::AtomicWriteFile::open(&self.path)
            .map_err(|error| SpeechError::io(&error, "state open"))?;
        file.write_all(&bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| SpeechError::io(&error, "state write"))?;
        file.commit()
            .map_err(|error| SpeechError::io(&error, "state commit"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn choices_round_trip_and_an_unreadable_file_moves_aside() {
        let folder = tempfile::tempdir().unwrap();
        let file = StateFile::new(folder.path());
        assert_eq!(file.read().0, DeviceState::default());

        let mut state = DeviceState {
            language: Some("fr".into()),
            ..DeviceState::default()
        };
        state
            .shortcuts
            .insert("dictation/dictation".into(), "Alt+Space".into());
        state.plugins.insert(
            "dictation".into(),
            PluginSpeech {
                model: Some("parakeet-ultra".into()),
                last_dictation: Some("Buy milk".into()),
                ..PluginSpeech::default()
            },
        );
        file.write(&state).unwrap();
        assert_eq!(file.read().0, state);

        std::fs::write(folder.path().join("state.json"), b"{not json").unwrap();
        assert_eq!(file.read(), (DeviceState::default(), true));
        assert!(folder.path().join("state.json.unreadable").exists());
    }

    #[test]
    fn a_newer_version_is_never_overwritten() {
        let folder = tempfile::tempdir().unwrap();
        std::fs::write(folder.path().join("state.json"), br#"{"version":9}"#).unwrap();
        let (state, writable) = StateFile::new(folder.path()).read();
        assert_eq!(state, DeviceState::default());
        assert!(!writable);
        assert!(folder.path().join("state.json").exists());
    }
}
