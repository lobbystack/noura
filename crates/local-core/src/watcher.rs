use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{CoreError, ErrorCategory, Result};

pub struct WatchCoordinator {
    _watcher: Option<RecommendedWatcher>,
    receiver: Mutex<mpsc::Receiver<notify::Result<Event>>>,
}

impl WatchCoordinator {
    pub fn new(root: &Path) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(1024);
        let mut watcher = RecommendedWatcher::new(
            move |event| {
                let _ = sender.try_send(event);
            },
            Config::default(),
        )
        .map_err(|_| {
            CoreError::new(
                "watcher_unavailable",
                ErrorCategory::Filesystem,
                "The filesystem watcher could not start",
                "watcher_start",
            )
        })?;
        watcher.watch(root, RecursiveMode::Recursive).map_err(|_| {
            CoreError::new(
                "watcher_unavailable",
                ErrorCategory::Filesystem,
                "The workspace could not be watched",
                "watcher_start",
            )
        })?;
        Ok(Self {
            _watcher: Some(watcher),
            receiver: Mutex::new(receiver),
        })
    }

    pub fn drain_coalesced(&self, wait: Duration) -> Result<Vec<PathBuf>> {
        let receiver = self.receiver.lock().map_err(|_| {
            CoreError::new(
                "watcher_lock_unavailable",
                ErrorCategory::Transient,
                "The filesystem watcher is unavailable",
                "watcher_poll",
            )
        })?;
        let deadline = Instant::now() + wait;
        let mut paths = BTreeSet::new();
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = if paths.is_empty() {
                receiver.recv_timeout(remaining).ok()
            } else {
                receiver.try_recv().ok()
            };
            let Some(event) = event else { break };
            let event = event.map_err(|_| {
                CoreError::new(
                    "watcher_event_error",
                    ErrorCategory::Transient,
                    "A filesystem event could not be read",
                    "watcher_poll",
                )
            })?;
            paths.extend(event.paths);
            if Instant::now() >= deadline {
                break;
            }
        }
        Ok(paths.into_iter().collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::EventKind;
    use tempfile::tempdir;

    #[test]
    fn watcher_starts_for_a_workspace() {
        let root = tempdir().unwrap();
        WatchCoordinator::new(root.path()).unwrap();
    }

    #[test]
    fn watcher_coalesces_duplicate_and_editor_replacement_events() {
        let (sender, receiver) = mpsc::sync_channel(16);
        let temporary = PathBuf::from("/workspace/.note.md.tmp");
        let destination = PathBuf::from("/workspace/note.md");
        sender
            .send(Ok(Event::new(EventKind::Any).add_path(temporary.clone())))
            .unwrap();
        sender
            .send(Ok(Event::new(EventKind::Any).add_path(temporary.clone())))
            .unwrap();
        sender
            .send(Ok(Event::new(EventKind::Any).add_path(destination.clone())))
            .unwrap();
        drop(sender);

        let watcher = WatchCoordinator {
            _watcher: None,
            receiver: Mutex::new(receiver),
        };
        let paths = watcher.drain_coalesced(Duration::from_secs(1)).unwrap();
        assert_eq!(paths, vec![temporary, destination]);
    }
}
