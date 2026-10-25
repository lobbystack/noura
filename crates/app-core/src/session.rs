//! What every window of the app shares: the open workspace, with its one
//! index and watcher, the newest listing of its files, their problems and
//! the notes changed last, the rail order, this device's preferences, the
//! plugins service, each window's view state, which it saves together, the
//! writes still on their way to disk, the update service, and a quit or
//! workspace switch that waits for every window's notes to save.
//!
//! Each [`App`](crate::App) is one window. A window that changes what
//! the others show publishes it here and wakes them; each one applies it
//! on its next [`App::poll`](crate::App::poll). The watch thread publishes
//! the changes it finds here too, and tells every window who made them.

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, Weak},
};

use local_core::ChangeCounts;

use crate::{
    FileTree, PendingWrites, Preferences, Problem, System, Updates, Workspace, WorkspaceService,
    ai::Ai,
    app::{Change, Listeners},
    links::LinkFiles,
    plugins::{Plugins, SystemClock},
    remote_images::RemoteImages,
    view_state::{StoredView, StoredWindow, ViewStore},
    watch::{Rescans, Snapshot, Update, Watch},
};

/// How many moves the session remembers for windows that haven't caught
/// up yet. A window further behind lets its tabs follow their files by
/// stable ID alone.
const MOVES_KEPT: usize = 256;

/// A window's identity for this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowKey(u64);

#[cfg(test)]
impl WindowKey {
    /// The key of the `index`th window a session opened.
    pub(crate) fn nth(index: u64) -> Self {
        Self(index)
    }
}

pub(crate) struct Session {
    pub(crate) app_data: PathBuf,
    pub(crate) service: Arc<WorkspaceService>,
    pub(crate) system: Arc<dyn System>,
    /// Writes that must reach disk before Noura restarts or quits.
    pub(crate) writes: PendingWrites,
    pub(crate) updates: Updates,
    /// How often the watch thread walks the whole workspace.
    rescans: Rescans,
    /// The files of the open workspace's newest listing, which the links
    /// in its notes resolve against.
    pub(crate) link_files: LinkFiles,
    /// The remote images notes asked for, which every window shares.
    pub(crate) remote_images: Arc<RemoteImages>,
    state: Mutex<State>,
    /// Held apart from `state`, so a plugin call never blocks a window
    /// that only reads the session.
    plugins: Mutex<Plugins>,
    /// The sync account every window's account screens show.
    account: Mutex<Arc<dyn crate::account::AccountBackend>>,
    /// The AI providers, consents and runs every window shares.
    pub(crate) ai: Arc<Ai>,
}

struct State {
    workspace: Option<Arc<Workspace>>,
    /// Counts the workspaces opened so far. A window that shows an
    /// earlier one has closed.
    opening: u64,
    /// The newest listing of the open workspace's files, with every
    /// folder closed.
    tree: Option<FileTree>,
    /// The open workspace's problems in that listing.
    problems: Vec<Problem>,
    /// The notes changed last in that listing, newest first.
    recent: Vec<String>,
    /// The engine's change counts when that listing was made. An older
    /// listing never replaces it.
    counts: ChangeCounts,
    /// Keeps the open workspace in step with its folder.
    watch: Option<Watch>,
    /// Counts the listings so far.
    listing: u64,
    /// Items that moved, each with the listing that first showed it.
    moves: VecDeque<(u64, String, String)>,
    /// The rail order the open workspace's windows share.
    rail_order: Vec<String>,
    /// Counts the changes to the rail order.
    rail: u64,
    /// Counts the changes to the recent list.
    recents: u64,
    /// This device's preferences as a window changed them last, and how
    /// many changes there were.
    preferences: Option<Preferences>,
    preferences_version: u64,
    /// The windows on the open workspace, in the order they opened.
    windows: Vec<Window>,
    views: ViewStore,
    next_key: u64,
    /// The quit or workspace switch waiting for the windows' notes.
    departure: Option<Departure>,
    /// Counts the departures so far.
    departures: u64,
}

/// A quit, or a switch to another workspace, which closes every window,
/// or a restart to update: it waits until each open window that shows
/// notes has them on disk.
struct Departure {
    round: u64,
    /// The window that asked, which quits or opens the workspace once
    /// every window is ready.
    by: WindowKey,
    /// The windows whose notes are on disk.
    ready: Vec<WindowKey>,
}

struct Window {
    key: WindowKey,
    /// How to wake the window. `None` once its `App` is gone without
    /// closing, as when Noura quits: its view state stays saved.
    listeners: Option<Listeners>,
    view: StoredWindow,
    /// Whether the window shows notes, which a departure waits for.
    has_notes: bool,
}

/// What a window needs once it opened a workspace.
pub(crate) struct Adopted {
    pub(crate) opening: u64,
    pub(crate) listing: u64,
    pub(crate) rail: u64,
    pub(crate) recents: u64,
    /// The workspace's saved windows and rail order.
    pub(crate) view: StoredView,
}

/// A listing newer than the one a window shows.
pub(crate) struct Listing {
    pub(crate) version: u64,
    pub(crate) tree: FileTree,
    pub(crate) problems: Vec<Problem>,
    pub(crate) recent: Vec<String>,
    /// What moved since the window's listing, in order.
    pub(crate) moves: Vec<(String, String)>,
}

/// The state a new window starts from.
pub(crate) struct Current {
    pub(crate) listing: u64,
    pub(crate) tree: Option<FileTree>,
    pub(crate) problems: Vec<Problem>,
    pub(crate) recent: Vec<String>,
    pub(crate) rail: u64,
    pub(crate) recents: u64,
}

impl Session {
    pub(crate) fn new(app_data: &Path, system: Arc<dyn System>, rescans: Rescans) -> Arc<Self> {
        Arc::new_cyclic(|session: &Weak<Self>| {
            let writes = PendingWrites::default();
            // The update service wakes every open window when its status
            // changes.
            let woken = session.clone();
            let updates = Updates::new(
                writes.clone(),
                Arc::new(move |change| {
                    if let Some(session) = woken.upgrade() {
                        session.emit_all(change);
                    }
                }),
            );
            let mut plugins = Plugins::new(app_data);
            // A plugin's reload watcher wakes every window once it read a
            // new build.
            let reloaded = session.clone();
            plugins.set_wake(Arc::new(move || {
                if let Some(session) = reloaded.upgrade() {
                    session.wake_all();
                }
            }));
            // Plugins read the time and time zone from the system, which
            // tests fix.
            plugins.set_clock(Arc::new(SystemClock(system.clone())));
            // AI runs wake every window as their answers arrive, and keep
            // provider keys in the system's credential store.
            let ai = Ai::new(app_data, system.credentials());
            let woken = session.clone();
            ai.set_wake(Arc::new(move || {
                if let Some(session) = woken.upgrade() {
                    session.wake_all();
                }
            }));
            plugins.set_ai(ai.clone());
            Self::with(app_data, system, rescans, writes, updates, plugins, ai)
        })
    }

    fn with(
        app_data: &Path,
        system: Arc<dyn System>,
        rescans: Rescans,
        writes: PendingWrites,
        updates: Updates,
        plugins: Plugins,
        ai: Arc<Ai>,
    ) -> Self {
        Self {
            app_data: app_data.to_owned(),
            service: Arc::new(WorkspaceService::with_system(app_data, system.clone())),
            system,
            writes,
            updates,
            rescans,
            link_files: LinkFiles::default(),
            remote_images: Arc::default(),
            state: Mutex::new(State {
                workspace: None,
                opening: 0,
                tree: None,
                problems: Vec::new(),
                recent: Vec::new(),
                counts: ChangeCounts::default(),
                watch: None,
                listing: 0,
                moves: VecDeque::new(),
                rail_order: Vec::new(),
                rail: 0,
                recents: 0,
                preferences: None,
                preferences_version: 0,
                windows: Vec::new(),
                views: ViewStore::new(app_data),
                next_key: 0,
                departure: None,
                departures: 0,
            }),
            plugins: Mutex::new(plugins),
            account: Mutex::new(Arc::new(crate::account::NoAccount::default())),
            ai,
        }
    }

    /// The sync account backend every window shares.
    pub(crate) fn account(&self) -> Arc<dyn crate::account::AccountBackend> {
        self.account
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Runs the account screens with `backend`, which wakes every window
    /// when the screens change.
    pub(crate) fn set_account(self: &Arc<Self>, backend: Arc<dyn crate::account::AccountBackend>) {
        let session = Arc::downgrade(self);
        backend.set_wake(Arc::new(move || {
            if let Some(session) = session.upgrade() {
                session.emit_all(Change::Account);
            }
        }));
        *self
            .account
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = backend;
    }

    /// The plugins service every window shares.
    pub(crate) fn plugins(&self) -> MutexGuard<'_, Plugins> {
        self.plugins
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }

    /// Wake every open window to apply a change, such as a plugin's new
    /// build.
    pub(crate) fn wake_all(&self) {
        self.emit_all(Change::WorkFinished);
    }

    /// Tell every open window about `change`.
    fn emit_all(&self, change: Change) {
        let windows: Vec<Listeners> = self
            .lock()
            .windows
            .iter()
            .filter_map(|window| window.listeners.clone())
            .collect();
        for listeners in windows {
            listeners.emit(change);
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Add a window, woken through `listeners`.
    pub(crate) fn join(&self, listeners: Listeners) -> WindowKey {
        let mut state = self.lock();
        let key = WindowKey(state.next_key);
        state.next_key += 1;
        state.windows.push(Window {
            key,
            listeners: Some(listeners),
            view: StoredWindow::default(),
            has_notes: false,
        });
        key
    }

    /// The window's `App` is gone without closing, as when Noura quits.
    /// Its view state stays saved, and it no longer counts as open.
    pub(crate) fn leave(&self, key: WindowKey) {
        let mut state = self.lock();
        if let Some(window) = state.window(key) {
            window.listeners = None;
        }
        let waiting = state.window_gone(key);
        drop(state);
        wake(waiting);
    }

    /// Close the window. Its view state leaves the saved windows while
    /// another window stays open. The last window's stays, so it comes
    /// back at the next launch as when Noura quits.
    pub(crate) fn close(&self, key: WindowKey) {
        let mut state = self.lock();
        let others = state
            .windows
            .iter()
            .any(|window| window.key != key && window.listeners.is_some());
        if others {
            state.windows.retain(|window| window.key != key);
            state.write();
        } else if let Some(window) = state.window(key) {
            window.listeners = None;
        }
        let waiting = state.window_gone(key);
        drop(state);
        wake(waiting);
    }

    pub(crate) fn opening(&self) -> u64 {
        self.lock().opening
    }

    /// The window `key` opened `workspace`, listed as `snapshot` when the
    /// engine's change counts were `counts`, and shown as `tree`. Every
    /// other window closes, the previous workspace's watch thread stops,
    /// and a new one watches `workspace`. Returns the workspace's saved
    /// view state for the window to restore.
    pub(crate) fn adopt(
        self: &Arc<Self>,
        key: WindowKey,
        workspace: Arc<Workspace>,
        tree: &FileTree,
        snapshot: &Snapshot,
        counts: ChangeCounts,
    ) -> Adopted {
        let mut state = self.lock();
        // The previous workspace's watch thread publishes nothing once the
        // opening changes, and stops below, once the lock is free for it.
        let previous = state.watch.take();
        state.opening += 1;
        state.listing += 1;
        state.tree = Some(tree.clone());
        self.link_files.list(tree, snapshot);
        state.problems.clone_from(&snapshot.problems);
        state.recent.clone_from(&snapshot.recent);
        state.counts = counts;
        state.moves.clear();
        state.watch = self.watch(&workspace, snapshot, counts, state.opening);
        let view = state.views.load(workspace.id());
        state.workspace = Some(workspace);
        state.rail_order.clone_from(&view.rail_order);
        state.rail += 1;
        state.recents += 1;
        let closing: Vec<Listeners> = state
            .windows
            .iter()
            .filter(|window| window.key != key)
            .filter_map(|window| window.listeners.clone())
            .collect();
        state.windows.retain(|window| window.key == key);
        state.departure = None;
        let adopted = Adopted {
            opening: state.opening,
            listing: state.listing,
            rail: state.rail,
            recents: state.recents,
            view,
        };
        drop(state);
        if let Some(previous) = previous {
            previous.finish();
        }
        for listeners in closing {
            listeners.emit(Change::Screen);
        }
        adopted
    }

    /// The state a window opening now on the open workspace starts from.
    pub(crate) fn current(&self) -> Current {
        let state = self.lock();
        Current {
            listing: state.listing,
            tree: state.tree.clone(),
            problems: state.problems.clone(),
            recent: state.recent.clone(),
            rail: state.rail,
            recents: state.recents,
        }
    }

    /// The window `key` listed the files again as `snapshot`, shown as
    /// `tree`, when the engine's change counts were `counts`, after the
    /// item at `moved.0` moved to `moved.1`. The other windows wake to
    /// show it, and the watch thread compares its next listings with it.
    /// Returns the listing version the window shows. When the watch thread
    /// published a newer listing meanwhile, the session keeps that one, and
    /// the window wakes to show it too.
    pub(crate) fn publish_listing(
        &self,
        key: WindowKey,
        opening: u64,
        tree: &FileTree,
        moved: Option<(&str, &str)>,
        snapshot: &Snapshot,
        counts: ChangeCounts,
    ) -> u64 {
        let mut state = self.lock();
        if opening != state.opening {
            return state.listing;
        }
        state.listing += 1;
        let newest = !counts.is_before(state.counts);
        if newest {
            state.tree = Some(tree.clone());
            self.link_files.list(tree, snapshot);
            state.problems.clone_from(&snapshot.problems);
            state.recent.clone_from(&snapshot.recent);
            state.counts = counts;
        }
        if let Some(watch) = &state.watch {
            watch.show(snapshot, counts);
        }
        if let Some((from, to)) = moved {
            let listing = state.listing;
            state
                .moves
                .push_back((listing, from.to_owned(), to.to_owned()));
            if state.moves.len() > MOVES_KEPT {
                state.moves.pop_front();
            }
        }
        let listing = state.listing;
        let mut woken = state.others(key);
        if !newest {
            woken.extend(state.listeners(key));
        }
        drop(state);
        wake(woken);
        if newest { listing } else { listing - 1 }
    }

    /// Start the thread that keeps `workspace` in step with its folder,
    /// for the workspace opening `opening`. It holds the session weakly,
    /// so the session closes once its last window goes.
    fn watch(
        self: &Arc<Self>,
        workspace: &Workspace,
        snapshot: &Snapshot,
        counts: ChangeCounts,
        opening: u64,
    ) -> Option<Watch> {
        let session: Weak<Self> = Arc::downgrade(self);
        Watch::start(
            workspace.shared_engine(),
            workspace.root().to_owned(),
            snapshot.clone(),
            counts,
            self.rescans,
            Box::new(move |update| {
                session
                    .upgrade()
                    .is_some_and(|session| session.publish_watch(opening, update))
            }),
        )
    }

    /// The watch thread listed the files of the workspace opening
    /// `opening` again after a change. Every window wakes and hears who
    /// made the change; the listing replaces the session's unless that is
    /// newer. Returns false once that workspace has closed.
    fn publish_watch(&self, opening: u64, update: Update) -> bool {
        let mut state = self.lock();
        if opening != state.opening {
            return false;
        }
        if !update.counts.is_before(state.counts) {
            state.listing += 1;
            self.link_files.list(&update.tree, &update.snapshot);
            state.tree = Some(update.tree);
            state.problems = update.snapshot.problems;
            state.recent = update.snapshot.recent;
            state.counts = update.counts;
        }
        let windows: Vec<Listeners> = state
            .windows
            .iter()
            .filter_map(|window| window.listeners.clone())
            .collect();
        drop(state);
        for listeners in windows {
            for origin in &update.origins {
                listeners.emit(Change::Files(*origin));
            }
        }
        true
    }

    /// The newest listing, when it is newer than `seen`.
    pub(crate) fn listing_since(&self, seen: u64) -> Option<Listing> {
        let state = self.lock();
        if state.listing <= seen {
            return None;
        }
        Some(Listing {
            version: state.listing,
            tree: state.tree.clone()?,
            problems: state.problems.clone(),
            recent: state.recent.clone(),
            moves: state
                .moves
                .iter()
                .filter(|(listing, ..)| *listing > seen)
                .map(|(_, from, to)| (from.clone(), to.clone()))
                .collect(),
        })
    }

    /// The window `key` reordered the rail. Returns the order's version.
    pub(crate) fn set_rail_order(&self, key: WindowKey, opening: u64, order: Vec<String>) -> u64 {
        let mut state = self.lock();
        if opening != state.opening {
            return state.rail;
        }
        state.rail_order = order;
        state.rail += 1;
        let rail = state.rail;
        let others = state.others(key);
        drop(state);
        wake(others);
        rail
    }

    /// The rail order and its version, when it is newer than `seen`.
    pub(crate) fn rail_since(&self, seen: u64) -> Option<(u64, Vec<String>)> {
        let state = self.lock();
        (state.rail > seen).then(|| (state.rail, state.rail_order.clone()))
    }

    /// The window `key` changed the recent list. Returns the list's
    /// version.
    pub(crate) fn recents_changed(&self, key: WindowKey) -> u64 {
        let mut state = self.lock();
        state.recents += 1;
        let recents = state.recents;
        let others = state.others(key);
        drop(state);
        wake(others);
        recents
    }

    /// The recent list's version, when it is newer than `seen`.
    pub(crate) fn recents_since(&self, seen: u64) -> Option<u64> {
        let state = self.lock();
        (state.recents > seen).then_some(state.recents)
    }

    /// The window `key` changed this device's preferences, which every
    /// window draws with. Returns their version.
    pub(crate) fn set_preferences(&self, key: WindowKey, preferences: Preferences) -> u64 {
        let mut state = self.lock();
        state.preferences = Some(preferences);
        state.preferences_version += 1;
        let version = state.preferences_version;
        let others = state.others(key);
        drop(state);
        wake(others);
        version
    }

    /// The preferences and their version, when a window changed them
    /// since `seen`.
    pub(crate) fn preferences_since(&self, seen: u64) -> Option<(u64, Preferences)> {
        let state = self.lock();
        let preferences = state.preferences?;
        (state.preferences_version > seen).then_some((state.preferences_version, preferences))
    }

    /// The window `key` asks to quit or to open another workspace, which
    /// closes every window, or to restart to update. The other windows
    /// wake to save their notes.
    /// Returns the departure's round, or `None` while another departure
    /// waits.
    pub(crate) fn begin_departure(&self, key: WindowKey) -> Option<u64> {
        let mut state = self.lock();
        if state.departure.is_some() {
            return None;
        }
        state.departures += 1;
        let round = state.departures;
        state.departure = Some(Departure {
            round,
            by: key,
            ready: Vec::new(),
        });
        let others = state.others(key);
        drop(state);
        wake(others);
        Some(round)
    }

    /// Say whether the window `key` shows notes. A departure waits only
    /// for the windows that do, since only they can hold edits that aren't
    /// on disk.
    pub(crate) fn set_has_notes(&self, key: WindowKey, has_notes: bool) {
        let mut state = self.lock();
        let Some(window) = state.window(key) else {
            return;
        };
        if window.has_notes == has_notes {
            return;
        }
        window.has_notes = has_notes;
        let asker = state
            .departure
            .as_ref()
            .map(|departure| departure.by)
            .filter(|by| *by != key && state.all_ready())
            .and_then(|by| state.listeners(by));
        drop(state);
        wake(asker.into_iter().collect());
    }

    /// The departure waiting now, and the window that asked for it.
    pub(crate) fn departure(&self) -> Option<(u64, WindowKey)> {
        self.lock()
            .departure
            .as_ref()
            .map(|departure| (departure.round, departure.by))
    }

    /// Say whether the window `key` has its notes on disk for the
    /// departure `round`. Returns whether every open window does; the
    /// window that asked wakes then.
    pub(crate) fn set_ready(&self, key: WindowKey, round: u64, ready: bool) -> bool {
        let mut state = self.lock();
        let Some(departure) = state
            .departure
            .as_mut()
            .filter(|departure| departure.round == round)
        else {
            return false;
        };
        departure.ready.retain(|window| *window != key);
        if ready {
            departure.ready.push(key);
        }
        let by = departure.by;
        let all = state.all_ready();
        let asker = (all && by != key).then(|| state.listeners(by)).flatten();
        drop(state);
        wake(asker.into_iter().collect());
        all
    }

    /// Whether every open window has its notes on disk for the departure
    /// `round`.
    pub(crate) fn all_ready(&self, round: u64) -> bool {
        let state = self.lock();
        state
            .departure
            .as_ref()
            .is_some_and(|departure| departure.round == round)
            && state.all_ready()
    }

    /// The departure `round` is over: it went ahead, or a window kept
    /// editing. The other windows wake to stop waiting.
    pub(crate) fn end_departure(&self, key: WindowKey, round: u64) {
        let mut state = self.lock();
        if state
            .departure
            .as_ref()
            .is_none_or(|departure| departure.round != round)
        {
            return;
        }
        state.departure = None;
        let others = state.others(key);
        drop(state);
        wake(others);
    }

    /// Keep the window's view state with the others, and save them all
    /// in the background when `write` says so.
    pub(crate) fn save_window(
        &self,
        key: WindowKey,
        opening: u64,
        view: StoredWindow,
        write: bool,
    ) {
        let mut state = self.lock();
        if opening != state.opening {
            return;
        }
        let Some(window) = state.window(key) else {
            return;
        };
        window.view = view;
        if write {
            state.write();
        }
    }
}

impl State {
    fn window(&mut self, key: WindowKey) -> Option<&mut Window> {
        self.windows.iter_mut().find(|window| window.key == key)
    }

    /// Whether every open window that shows notes is ready for the
    /// departure.
    fn all_ready(&self) -> bool {
        let Some(departure) = &self.departure else {
            return false;
        };
        self.windows
            .iter()
            .filter(|window| window.listeners.is_some() && window.has_notes)
            .all(|window| departure.ready.contains(&window.key))
    }

    /// The window `key` closed or went away. A departure it asked for
    /// ends, and the windows that saved for it wake to stop waiting; once
    /// the others are ready for another window's departure, that window
    /// wakes to go ahead. Returns who to wake.
    fn window_gone(&mut self, key: WindowKey) -> Vec<Listeners> {
        let Some(departure) = &mut self.departure else {
            return Vec::new();
        };
        departure.ready.retain(|window| *window != key);
        if departure.by == key {
            self.departure = None;
            return self.others(key);
        }
        let by = departure.by;
        if self.all_ready() {
            return self.listeners(by).into_iter().collect();
        }
        Vec::new()
    }

    /// Save the open workspace's windows and rail order.
    fn write(&mut self) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        let view = StoredView {
            windows: self
                .windows
                .iter()
                .map(|window| window.view.clone())
                .collect(),
            rail_order: self.rail_order.clone(),
        };
        let id = workspace.id().to_owned();
        self.views.save(&id, view);
    }

    /// How to wake the window `key`, while it is open.
    fn listeners(&self, key: WindowKey) -> Option<Listeners> {
        self.windows
            .iter()
            .find(|window| window.key == key)
            .and_then(|window| window.listeners.clone())
    }

    /// How to wake every open window except `key`.
    fn others(&self, key: WindowKey) -> Vec<Listeners> {
        self.windows
            .iter()
            .filter(|window| window.key != key)
            .filter_map(|window| window.listeners.clone())
            .collect()
    }
}

impl Drop for Session {
    /// Stop the watch thread and wait for it, so the workspace's files
    /// and index are free once the last window goes.
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(watch) = state.watch.take() {
            watch.finish();
        }
    }
}

/// Wake windows to apply a change another window made. Called without
/// the session's lock, so a listener may call back into the session.
fn wake(windows: Vec<Listeners>) {
    for listeners in windows {
        listeners.emit(Change::WorkFinished);
    }
}
