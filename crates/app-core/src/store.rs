//! Per-device state kept as JSON files in the app data folder, such as
//! the recent list, the window layout and each workspace's tabs.

use std::{io::Write, marker::PhantomData, path::PathBuf, sync::Mutex};

use serde::{Serialize, de::DeserializeOwned};

/// One JSON file of per-device state. A missing or unreadable file loads
/// as the default: this state is a convenience, never workspace data.
pub(crate) struct JsonStore<T> {
    file: PathBuf,
    /// What the file holds, for the diagnostic log.
    what: &'static str,
    /// Held across each load, change and save, so a change made on the UI
    /// thread and one made by a background open never undo each other.
    writing: Mutex<()>,
    value: PhantomData<fn() -> T>,
}

impl<T: Serialize + DeserializeOwned + Default> JsonStore<T> {
    pub(crate) fn new(file: PathBuf, what: &'static str) -> Self {
        Self {
            file,
            what,
            writing: Mutex::new(()),
            value: PhantomData,
        }
    }

    /// The saved value, or the default when there is none or it can't be
    /// read.
    pub(crate) fn load(&self) -> T {
        let bytes = match std::fs::read(&self.file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return T::default(),
            Err(error) => {
                tracing::warn!(%error, what = self.what, "a per-device state file could not be read");
                return T::default();
            }
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(%error, what = self.what, "a per-device state file is invalid");
            T::default()
        })
    }

    /// Load the value and change it, holding the store's lock throughout.
    /// `change` returns whether it changed anything; only then does the
    /// store save. Returns the value as it is now.
    pub(crate) fn update(&self, change: impl FnOnce(&mut T) -> bool) -> std::io::Result<T> {
        let _writing = self.lock();
        let mut value = self.load();
        if change(&mut value) {
            self.write(&value)?;
        }
        Ok(value)
    }

    /// Replace the saved value.
    pub(crate) fn save(&self, value: &T) -> std::io::Result<()> {
        let _writing = self.lock();
        self.write(value)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.writing
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Write through a temporary file that replaces the old one, so a
    /// crash leaves the old value or the new one.
    fn write(&self, value: &T) -> std::io::Result<()> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let bytes = serde_json::to_vec_pretty(value).map_err(std::io::Error::other)?;
        let mut file = atomic_write_file::AtomicWriteFile::open(&self.file)?;
        file.write_all(&bytes)?;
        file.commit()
    }
}
