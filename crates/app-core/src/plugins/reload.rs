//! Reloads a plugin installed from a folder when a new build lands there.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc::Sender,
    },
    time::Duration,
};

use local_core::WatchCoordinator;
use plugin_host::{Candidate, InstallError, PluginHost};

use super::Wake;

/// How long a plugin folder must stay quiet after a change before a reload
/// reads it, so a build tool can finish writing.
const QUIET: Duration = Duration::from_millis(200);

/// How long a reload watcher waits for changes before it checks whether it
/// should stop.
const WATCH_WAIT: Duration = Duration::from_millis(500);

/// A new build a watcher found in a plugin's folder, or why it can't load.
pub(super) struct Reload {
    pub(super) id: String,
    pub(super) result: Result<Candidate, InstallError>,
}

/// Stops its reload watcher when dropped.
pub(super) struct Watcher {
    pub(super) stop: Arc<AtomicBool>,
    /// The digest of the build the watcher offered last, shared with it.
    pub(super) offered: Offered,
}

/// The digest of the build a reload watcher offered last. The watcher
/// skips a build with this digest; clearing it makes the next change in
/// the folder offer the build again.
pub(super) type Offered = Arc<Mutex<String>>;

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

impl Watcher {
    /// Offer the build in the folder again when the folder next changes.
    pub(super) fn forget_offered(&self) {
        self.offered
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clear();
    }
}

/// Watch `folder`, where the plugin `id` came from, for new builds, and
/// send each one on `sender`. `installed` is the installed build's digest.
pub(super) fn watch(
    id: &str,
    folder: &str,
    installed: String,
    host: Arc<PluginHost>,
    sender: Sender<Reload>,
    wake: Option<Wake>,
) -> Option<Watcher> {
    let stop = Arc::new(AtomicBool::new(false));
    let offered = Arc::new(Mutex::new(installed));
    let watcher = ReloadWatcher {
        id: id.to_owned(),
        folder: PathBuf::from(folder),
        host,
        offered: offered.clone(),
        sender,
        wake,
        stop: stop.clone(),
    };
    let spawned = std::thread::Builder::new()
        .name("plugin-reload".into())
        .spawn(move || watcher.run());
    match spawned {
        Ok(_) => Some(Watcher { stop, offered }),
        Err(error) => {
            tracing::warn!(%error, "a plugin's folder isn't watched for new builds");
            None
        }
    }
}

/// Watches a plugin's folder and reads each new build there.
struct ReloadWatcher {
    id: String,
    folder: PathBuf,
    host: Arc<PluginHost>,
    /// The digest of the build offered last, which a change must differ
    /// from. It starts as the installed build's.
    offered: Offered,
    sender: Sender<Reload>,
    wake: Option<Wake>,
    stop: Arc<AtomicBool>,
}

impl ReloadWatcher {
    fn offered(&self) -> std::sync::MutexGuard<'_, String> {
        self.offered.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn run(self) {
        let watcher = WatchCoordinator::new(&self.folder, |_| {});
        let mut last_problem: Option<String> = None;
        // The folder may have changed while Noura wasn't running.
        let mut look = true;
        while !self.stop.load(Ordering::Relaxed) {
            if !look {
                let changed = !watcher
                    .drain_coalesced(WATCH_WAIT)
                    .unwrap_or_default()
                    .is_empty()
                    || watcher.take_overflow()
                    || !watcher.is_available();
                if !changed {
                    continue;
                }
                // Let the build tool finish writing.
                while !self.stop.load(Ordering::Relaxed)
                    && !watcher
                        .drain_coalesced(QUIET)
                        .unwrap_or_default()
                        .is_empty()
                {}
            }
            look = false;
            if self.stop.load(Ordering::Relaxed) {
                return;
            }
            let result = match self.host.folder_digest(&self.folder) {
                Ok(digest) if digest == *self.offered() => continue,
                Ok(_) => self.host.inspect_update(&self.id, &self.folder),
                Err(error) => Err(error),
            };
            match &result {
                Ok(candidate) => {
                    *self.offered() = candidate.digest();
                    last_problem = None;
                }
                Err(error) if last_problem.as_ref() == Some(&error.message) => continue,
                Err(error) => last_problem = Some(error.message.clone()),
            }
            let reload = Reload {
                id: self.id.clone(),
                result,
            };
            if self.sender.send(reload).is_err() {
                return;
            }
            if let Some(wake) = &self.wake {
                wake();
            }
        }
    }
}
