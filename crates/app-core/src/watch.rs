//! Keeps an open workspace in step with its folder.
//!
//! A background thread drives `local-core`'s watcher and its full rescans,
//! so the UI thread never walks the folder. Whenever the files change, the
//! thread lists the workspace again and hands the new view to the session
//! every window shares, which tells the windows who made the change: Noura
//! or another app.

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::JoinHandle,
    time::{Duration, Instant},
};

use local_core::{ChangeCounts, WorkspaceEngine, WorkspaceEntry};

use crate::{
    AppError, FileTree, Problem, app::Origin, error::Access, inbox::RECENT_NOTES, problems,
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
    /// The paths of the notes changed last, newest first.
    pub(crate) recent: Vec<String>,
}

impl Snapshot {
    /// List the workspace, its problems and the notes changed last, with
    /// the engine's change counts at that moment. The listing reconciles
    /// the index first when the watcher saw a change nobody reconciled
    /// yet. No change lands while it lists, so the counts tell exactly
    /// which changes it shows.
    pub(crate) fn of(
        engine: &WorkspaceEngine,
        root: &str,
    ) -> Result<(Self, ChangeCounts), AppError> {
        let (snapshot, counts) = engine.read_settled(|| {
            let failed = |error| AppError::from_core(error, root, Access::ReadWorkspace);
            let entries = engine.list_workspace_entries().map_err(failed)?;
            let problems = problems::collect(engine, root)?;
            let recent = engine.recent_notes(RECENT_NOTES).map_err(failed)?;
            Ok::<_, AppError>(Self {
                entries,
                problems,
                recent,
            })
        });
        Ok((snapshot?, counts))
    }
}

/// A new view of the files, and who changed them.
pub(crate) struct Update {
    pub(crate) snapshot: Snapshot,
    pub(crate) tree: FileTree,
    pub(crate) origins: Vec<Origin>,
    /// The engine's change counts when the snapshot was listed.
    pub(crate) counts: ChangeCounts,
}

/// A listing of the files and the change counts it shows.
type Listing = (Snapshot, ChangeCounts);

/// Hands an update to the views. Returns false once nobody shows the
/// workspace anymore, which stops the thread.
pub(crate) type Publish = Box<dyn Fn(Update) -> bool + Send>;

/// The background thread that watches one open workspace. Dropping it
/// tells the thread to stop.
pub(crate) struct Watch {
    stop: Arc<AtomicBool>,
    /// The windows' own newest listing, waiting for the thread.
    listed: Arc<Mutex<Option<Listing>>>,
    thread: Option<JoinHandle<()>>,
}

impl Watch {
    /// Start watching. `current` is what the views show now, listed when
    /// the engine's change counts were `counts`, so only later changes
    /// produce updates, which go to `publish`.
    pub(crate) fn start(
        engine: Arc<WorkspaceEngine>,
        root: String,
        current: Snapshot,
        counts: ChangeCounts,
        rescans: Rescans,
        publish: Publish,
    ) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let listed = Arc::new(Mutex::new(None));
        let worker = Worker {
            engine,
            root,
            seen: current.clone(),
            heard: counts,
            shown: (current, counts),
            listed: listed.clone(),
            rescans,
            stop: stop.clone(),
            publish,
        };
        let spawned = std::thread::Builder::new()
            .name("workspace-watch".into())
            .spawn(move || worker.run());
        match spawned {
            Ok(thread) => Some(Self {
                stop,
                listed,
                thread: Some(thread),
            }),
            Err(error) => {
                tracing::error!(%error, "could not start the workspace watch thread");
                None
            }
        }
    }

    /// Tell the thread the views now show a listing a window made itself,
    /// after one of its own changes. The thread compares later listings
    /// with it, so a change another app makes right after, back to what
    /// the thread listed before, still reaches the views.
    pub(crate) fn show(&self, snapshot: &Snapshot, counts: ChangeCounts) {
        let mut listed = self
            .listed
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *listed = Some((snapshot.clone(), counts));
    }

    /// Stop the thread and wait for it, so the workspace's files and index
    /// are free once this returns. On the thread itself, as when its
    /// update outlived every window, it only asks the thread to stop.
    pub(crate) fn finish(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take()
            && thread.thread().id() != std::thread::current().id()
        {
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
    /// The thread's own last listing, which the change stream follows.
    seen: Snapshot,
    /// The change counts that listing shows.
    heard: ChangeCounts,
    /// What the views show, or will once the app takes the last update:
    /// the newer of that update and the app's own last listing.
    shown: Listing,
    listed: Arc<Mutex<Option<Listing>>>,
    rescans: Rescans,
    stop: Arc<AtomicBool>,
    publish: Publish,
}

impl Worker {
    fn run(mut self) {
        let mut last_rescan = Instant::now();
        while !self.stop.load(Ordering::SeqCst) {
            let rescanned = self.wait_and_rescan(&mut last_rescan);
            if !rescanned && self.engine.change_counts() == self.heard {
                continue;
            }
            let (snapshot, counts) = match Snapshot::of(&self.engine, &self.root) {
                Ok(listing) => listing,
                Err(error) => {
                    tracing::warn!(code = %error.code, "could not list the workspace");
                    continue;
                }
            };
            self.take_app_listing();
            // The app listed the files after this listing, so the views
            // show a newer state. The changes since then make the next
            // round list again.
            if counts.is_before(self.shown.1) {
                continue;
            }
            // Compared with what the views show too: after the app lists
            // the files itself, another app can put them back the way this
            // thread saw them last.
            let stale = snapshot != self.shown.0;
            let mut origins = origins_between(self.heard, counts);
            // Another app's edit of a file outside the index, such as a
            // plain-text file, leaves the listing as it was. The views still
            // hear of it, so an open file shows the edit.
            if snapshot == self.seen && !stale && !origins.contains(&Origin::Outside) {
                self.heard = counts;
                continue;
            }
            // Only another app changes files without a count, so a listing
            // that differs with nothing counted in between shows its change.
            let unannounced = stale && origins_between(self.shown.1, counts).is_empty();
            if (origins.is_empty() || unannounced) && !origins.contains(&Origin::Outside) {
                origins.push(Origin::Outside);
            }
            let tree = FileTree::from_entries(&snapshot.entries);
            self.seen = snapshot.clone();
            self.heard = counts;
            self.shown = (snapshot.clone(), counts);
            let update = Update {
                snapshot,
                tree,
                origins,
                counts,
            };
            if !(self.publish)(update) {
                return;
            }
        }
    }

    /// Take the app's newest own listing, unless the views will show a
    /// newer one.
    fn take_app_listing(&mut self) {
        let listed = self
            .listed
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(listed) = listed
            && !listed.1.is_before(self.shown.1)
        {
            self.shown = listed;
        }
    }

    /// Wait for the native watcher and reconcile what it reports, or sleep
    /// while it can't run. Returns whether a full rescan ran, including the
    /// one that follows events the watcher lost.
    fn wait_and_rescan(&self, last_rescan: &mut Instant) -> bool {
        let watching = self.engine.is_watching();
        let interval = if watching {
            match self.engine.poll_watcher(POLL_WAIT) {
                Ok(poll) if poll.rescanned => {
                    *last_rescan = Instant::now();
                    return true;
                }
                Ok(_) => {}
                Err(error) => {
                    tracing::warn!(code = %error.code, "could not apply outside changes");
                }
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
}

/// Who made the changes counted in `later` and not in `earlier`.
fn origins_between(earlier: ChangeCounts, later: ChangeCounts) -> Vec<Origin> {
    let mut origins = Vec::new();
    if later.own != earlier.own {
        origins.push(Origin::Noura);
    }
    if later.outside != earlier.outside {
        origins.push(Origin::Outside);
    }
    origins
}
