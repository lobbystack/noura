use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};

use crate::{CoreError, ErrorCategory, Result};

/// Capacity of the queue between the native watcher thread and the poller.
const QUEUE_CAPACITY: usize = 1024;

/// Watches the workspace and hands coalesced paths to the reconciler.
///
/// Watching is an optimization: the periodic full rescan stays authoritative.
/// When the native watcher cannot start (for example when inotify runs out of
/// watches) the coordinator runs in reduced mode and callers rescan instead.
/// When the queue overflows or the platform asks for a rescan, the
/// coordinator records it so the next poll reconciles the whole workspace
/// instead of silently losing changes.
pub struct WatchCoordinator {
    _watcher: Option<RecommendedWatcher>,
    receiver: Mutex<mpsc::Receiver<notify::Result<Event>>>,
    overflowed: Arc<AtomicBool>,
    available: bool,
}

impl WatchCoordinator {
    /// Start watching `root`. `on_event` runs on the watcher thread for every
    /// native event before it is queued; it must be cheap and must not block.
    pub fn new(root: &Path, on_event: impl Fn(&Event) + Send + 'static) -> Self {
        let (sender, receiver) = mpsc::sync_channel(QUEUE_CAPACITY);
        let overflowed = Arc::new(AtomicBool::new(false));
        let flag = overflowed.clone();
        let watcher = RecommendedWatcher::new(
            move |event: notify::Result<Event>| {
                match &event {
                    Ok(event) => {
                        if event.need_rescan() {
                            flag.store(true, Ordering::SeqCst);
                        }
                        on_event(event);
                    }
                    Err(_) => flag.store(true, Ordering::SeqCst),
                }
                if sender.try_send(event).is_err() {
                    flag.store(true, Ordering::SeqCst);
                }
            },
            Config::default(),
        )
        .and_then(|mut watcher| {
            watcher.watch(root, RecursiveMode::Recursive)?;
            Ok(watcher)
        });
        match watcher {
            Ok(watcher) => Self {
                _watcher: Some(watcher),
                receiver: Mutex::new(receiver),
                overflowed,
                available: true,
            },
            Err(error) => {
                tracing::warn!(
                    %error,
                    "filesystem watcher unavailable; relying on periodic rescans"
                );
                Self {
                    _watcher: None,
                    receiver: Mutex::new(receiver),
                    overflowed,
                    available: false,
                }
            }
        }
    }

    /// Whether native change notifications are flowing. When false, readers
    /// must rescan instead of trusting that the index is current.
    pub fn is_available(&self) -> bool {
        self.available
    }

    /// Report and clear a lost-event condition (queue overflow, a native
    /// rescan request, or a watcher error) since the last call.
    pub fn take_overflow(&self) -> bool {
        self.overflowed.swap(false, Ordering::SeqCst)
    }

    /// Wait up to `wait` for the first event, then drain every queued event.
    /// Returns the distinct paths in sorted order.
    pub fn drain_coalesced(&self, wait: Duration) -> Result<Vec<PathBuf>> {
        let receiver = self.receiver.lock().map_err(|_| {
            CoreError::new(
                "watcher_lock_unavailable",
                ErrorCategory::Transient,
                "The filesystem watcher is unavailable",
                "watcher_poll",
            )
        })?;
        if !self.available {
            drop(receiver);
            std::thread::sleep(wait);
            return Ok(Vec::new());
        }
        let deadline = Instant::now() + wait;
        let mut paths = BTreeSet::new();
        let mut received = false;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            let event = if received {
                receiver.try_recv().ok()
            } else {
                receiver.recv_timeout(remaining).ok()
            };
            let Some(event) = event else { break };
            received = true;
            match event {
                Ok(event) => paths.extend(event.paths),
                // An unreadable event loses paths; reconcile everything.
                Err(_) => self.overflowed.store(true, Ordering::SeqCst),
            }
        }
        Ok(paths.into_iter().collect())
    }

    /// Test hook: behave as if the native queue overflowed.
    #[cfg(test)]
    pub(crate) fn simulate_overflow(&self) {
        self.overflowed.store(true, Ordering::SeqCst);
    }

    #[cfg(test)]
    fn from_receiver(receiver: mpsc::Receiver<notify::Result<Event>>) -> Self {
        Self {
            _watcher: None,
            receiver: Mutex::new(receiver),
            overflowed: Arc::new(AtomicBool::new(false)),
            available: true,
        }
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
        assert!(WatchCoordinator::new(root.path(), |_| {}).is_available());
    }

    #[test]
    fn a_missing_root_runs_in_reduced_mode_instead_of_failing() {
        let root = tempdir().unwrap();
        let watcher = WatchCoordinator::new(&root.path().join("missing"), |_| {});
        assert!(!watcher.is_available());
        assert!(
            watcher
                .drain_coalesced(Duration::from_millis(1))
                .unwrap()
                .is_empty()
        );
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

        let watcher = WatchCoordinator::from_receiver(receiver);
        let paths = watcher.drain_coalesced(Duration::from_secs(1)).unwrap();
        assert_eq!(paths, vec![temporary, destination]);
        assert!(!watcher.take_overflow());
    }

    #[test]
    fn an_event_error_requests_a_full_reconcile() {
        let (sender, receiver) = mpsc::sync_channel(4);
        sender
            .send(Err(notify::Error::generic("queue overflow")))
            .unwrap();
        drop(sender);
        let watcher = WatchCoordinator::from_receiver(receiver);
        assert!(
            watcher
                .drain_coalesced(Duration::from_millis(10))
                .unwrap()
                .is_empty()
        );
        assert!(watcher.take_overflow());
        assert!(!watcher.take_overflow());
    }

    #[test]
    fn a_full_queue_sets_the_overflow_flag() {
        let root = tempdir().unwrap();
        let watcher = WatchCoordinator::new(root.path(), |_| {});
        // Fill well past the queue capacity without draining it.
        for index in 0..(QUEUE_CAPACITY + 200) {
            std::fs::write(root.path().join(format!("{index}.md")), b"x").unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !watcher.overflowed.load(Ordering::SeqCst) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(watcher.take_overflow());
    }
}
