//! Keeps an open workspace in step with its folder.
//!
//! A background thread drives `local-core`'s watcher and its full rescans,
//! so the UI thread never walks the folder. Whenever the files change, the
//! thread lists the workspace again and hands the app the new view, then
//! tells listeners who made the change: Noura or another app.

use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, Sender, TryRecvError, channel},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use local_core::{ChangeCounts, WorkspaceEngine, WorkspaceEntry, WorkspaceEntryKind};

use crate::{
    AppError, FileTree, Problem,
    app::{Change, Listeners, Origin},
    error::Access,
    problems,
};

/// How long one wait on the native watcher lasts. The thread notices that
/// the workspace closed within this time.
const POLL_WAIT: Duration = Duration::from_millis(100);

/// How often Noura walks the whole workspace on its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rescans {
    /// A full rescan runs this often even while the native watcher works,
    /// in case it missed a change.
    pub periodic: Duration,
    /// A full rescan runs this often while the native watcher can't run.
    pub without_watcher: Duration,
}

impl Default for Rescans {
    fn default() -> Self {
        Self {
            periodic: Duration::from_secs(60),
            without_watcher: Duration::from_secs(5),
        }
    }
}

/// What the views show of the workspace's files at one moment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Snapshot {
    pub(crate) entries: Vec<WorkspaceEntry>,
    pub(crate) problems: Vec<Problem>,
}

impl Snapshot {
    /// List the workspace and its problems. The listing reconciles the
    /// index first when the watcher saw a change nobody reconciled yet.
    pub(crate) fn of(engine: &WorkspaceEngine, root: &str) -> Result<Self, AppError> {
        let failed = |error| AppError::from_core(error, root, Access::ReadWorkspace);
        let entries = engine.list_workspace_entries().map_err(failed)?;
        let problems = problems::collect(engine, root)?;
        Ok(Self { entries, problems })
    }

    /// The file listed at `relative_path`.
    pub(crate) fn file(&self, relative_path: &str) -> Option<&WorkspaceEntry> {
        self.entries.iter().find(|entry| {
            entry.kind == WorkspaceEntryKind::File && entry.relative_path == relative_path
        })
    }

    /// The one file that holds object `id`, or `None` when no file or
    /// several files hold it.
    pub(crate) fn only_file_with_id(&self, id: &str) -> Option<&WorkspaceEntry> {
        let mut holders = self
            .entries
            .iter()
            .filter(|entry| entry.object_id.as_deref() == Some(id));
        let first = holders.next()?;
        holders.next().is_none().then_some(first)
    }
}

/// A new view of the files, and who changed them.
pub(crate) struct Update {
    pub(crate) snapshot: Snapshot,
    pub(crate) tree: FileTree,
    pub(crate) origins: Vec<Origin>,
}

/// The background thread that watches one open workspace. Dropping it
/// tells the thread to stop.
pub(crate) struct Watch {
    stop: Arc<AtomicBool>,
    updates: Receiver<Update>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Start watching. `current` is what the views show now, so only later
    /// changes produce updates.
    pub(crate) fn start(
        engine: Arc<WorkspaceEngine>,
        root: String,
        current: Snapshot,
        rescans: Rescans,
        listeners: Listeners,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let (sender, updates) = channel();
        let heard = engine.change_counts();
        let worker = Worker {
            engine,
            root,
            last: current,
            rescans,
            stop: stop.clone(),
            sender,
            listeners,
            heard,
        };
        let spawned = std::thread::Builder::new()
            .name("workspace-watch".into())
            .spawn(move || worker.run());
        match spawned {
            Ok(thread) => Some(Self {
                stop,
                updates,
                thread: Some(thread),
            }),
            Err(error) => {
                tracing::error!(%error, "could not start the workspace watch thread");
                None
            }
        }
    }

    /// The newest update since the last call, carrying the origins of every
    /// update it replaces.
    pub(crate) fn take(&self) -> Option<Update> {
        let mut latest: Option<Update> = None;
        loop {
            match self.updates.try_recv() {
                Ok(mut update) => {
                    if let Some(previous) = latest.take() {
                        for origin in previous.origins {
                            if !update.origins.contains(&origin) {
                                update.origins.push(origin);
                            }
                        }
                    }
                    latest = Some(update);
                }
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => return latest,
            }
        }
    }

    /// Stop the thread and wait for it, so the workspace's files and index
    /// are free once this returns.
    pub(crate) fn finish(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for Watch {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

struct Worker {
    engine: Arc<WorkspaceEngine>,
    root: String,
    last: Snapshot,
    rescans: Rescans,
    stop: Arc<AtomicBool>,
    sender: Sender<Update>,
    listeners: Listeners,
    /// The file changes `local-core` had announced at the last look.
    heard: ChangeCounts,
}

impl Worker {
    fn run(mut self) {
        let mut last_rescan = Instant::now();
        // Origins heard while listing, which the next listing may show.
        let mut carried: Vec<Origin> = Vec::new();
        while !self.stop.load(Ordering::SeqCst) {
            let rescanned = self.wait_and_rescan(&mut last_rescan);
            let mut origins = std::mem::take(&mut carried);
            self.hear(&mut origins);
            if origins.is_empty() && !rescanned {
                continue;
            }
            let snapshot = match Snapshot::of(&self.engine, &self.root) {
                Ok(snapshot) => snapshot,
                Err(error) => {
                    tracing::warn!(code = %error.code, "could not list the workspace");
                    continue;
                }
            };
            // Changes that landed while listing may or may not show in this
            // listing, so the next round lists again under their origin.
            self.hear(&mut carried);
            for origin in &carried {
                if !origins.contains(origin) {
                    origins.push(*origin);
                }
            }
            if snapshot == self.last {
                continue;
            }
            if origins.is_empty() {
                origins.push(Origin::Outside);
            }
            let tree = FileTree::from_entries(&snapshot.entries);
            self.last = snapshot.clone();
            let update = Update {
                snapshot,
                tree,
                origins: origins.clone(),
            };
            if self.sender.send(update).is_err() {
                return;
            }
            for origin in origins {
                self.listeners.emit(Change::Files(origin));
            }
        }
    }

    /// Wait for the native watcher and reconcile what it reports, or sleep
    /// while it can't run. Returns whether a full rescan ran.
    fn wait_and_rescan(&self, last_rescan: &mut Instant) -> bool {
        let watching = self.engine.is_watching();
        let interval = if watching {
            if let Err(error) = self.engine.poll_external_changes(POLL_WAIT) {
                tracing::warn!(code = %error.code, "could not apply outside changes");
            }
            self.rescans.periodic
        } else {
            let left = self
                .rescans
                .without_watcher
                .saturating_sub(last_rescan.elapsed());
            std::thread::sleep(left.min(POLL_WAIT));
            self.rescans.without_watcher
        };
        if last_rescan.elapsed() < interval {
            return false;
        }
        // Without the watcher, the listing that follows walks and
        // reconciles the whole workspace by itself.
        if watching && let Err(error) = self.engine.reconcile() {
            tracing::warn!(code = %error.code, "the full rescan failed");
        }
        *last_rescan = Instant::now();
        true
    }

    /// Collect the origins of the file changes `local-core` announced
    /// since the last look. The counts never miss a change, however many
    /// Noura makes at once.
    fn hear(&mut self, origins: &mut Vec<Origin>) {
        let now = self.engine.change_counts();
        let heard = std::mem::replace(&mut self.heard, now);
        for (origin, changed) in [
            (Origin::Noura, now.own != heard.own),
            (Origin::Outside, now.outside != heard.outside),
        ] {
            if changed && !origins.contains(&origin) {
                origins.push(origin);
            }
        }
    }
}
