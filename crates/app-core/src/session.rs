//! What every window of the app shares: the open workspace, with its one
//! index and watcher, the newest listing of its files, the rail order,
//! this device's preferences, the plugins service, and each window's view
//! state, which it saves together.
//!
//! Each [`App`](crate::App) is one window. A window that changes what
//! the others show publishes it here and wakes them; each one applies it
//! on its next [`App::poll`](crate::App::poll).

use std::{
    collections::VecDeque,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use crate::{
    FileTree, Preferences, System, Workspace, WorkspaceService,
    app::{Change, Listeners},
    plugins::Plugins,
    view_state::{StoredView, StoredWindow, ViewStore},
};

/// How many moves the session remembers for windows that haven't caught
/// up yet. A window further behind lets its tabs follow their files by
/// stable ID alone.
const MOVES_KEPT: usize = 256;

/// A window's identity for this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WindowKey(u64);

pub(crate) struct Session {
    pub(crate) app_data: PathBuf,
    pub(crate) service: Arc<WorkspaceService>,
    pub(crate) system: Arc<dyn System>,
    state: Mutex<State>,
    /// Held apart from `state`, so a plugin call never blocks a window
    /// that only reads the session.
    plugins: Mutex<Plugins>,
}

struct State {
    workspace: Option<Arc<Workspace>>,
    /// Counts the workspaces opened so far. A window that shows an
    /// earlier one has closed.
    opening: u64,
    /// The newest listing of the open workspace's files, with every
    /// folder closed.
    tree: Option<FileTree>,
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
}

struct Window {
    key: WindowKey,
    /// How to wake the window. `None` once its `App` is gone without
    /// closing, as when Noura quits: its view state stays saved.
    listeners: Option<Listeners>,
    view: StoredWindow,
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
    /// What moved since the window's listing, in order.
    pub(crate) moves: Vec<(String, String)>,
}

/// The state a new window starts from.
pub(crate) struct Current {
    pub(crate) listing: u64,
    pub(crate) tree: Option<FileTree>,
    pub(crate) rail: u64,
    pub(crate) recents: u64,
}

impl Session {
    pub(crate) fn new(app_data: &Path, system: Arc<dyn System>) -> Arc<Self> {
        Arc::new_cyclic(|session: &std::sync::Weak<Self>| {
            let mut plugins = Plugins::new(app_data);
            // A plugin's reload watcher wakes every window once it read a
            // new build.
            let session = session.clone();
            plugins.set_wake(Arc::new(move || {
                if let Some(session) = session.upgrade() {
                    session.wake_all();
                }
            }));
            Self::with_plugins(app_data, system, plugins)
        })
    }

    fn with_plugins(app_data: &Path, system: Arc<dyn System>, plugins: Plugins) -> Self {
        Self {
            app_data: app_data.to_owned(),
            service: Arc::new(WorkspaceService::with_system(app_data, system.clone())),
            system,
            state: Mutex::new(State {
                workspace: None,
                opening: 0,
                tree: None,
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
            }),
            plugins: Mutex::new(plugins),
        }
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
        let windows: Vec<Listeners> = self
            .lock()
            .windows
            .iter()
            .filter_map(|window| window.listeners.clone())
            .collect();
        wake(windows);
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
        });
        key
    }

    /// The window's `App` is gone without closing, as when Noura quits.
    /// Its view state stays saved, and it no longer counts as open.
    pub(crate) fn leave(&self, key: WindowKey) {
        if let Some(window) = self.lock().window(key) {
            window.listeners = None;
        }
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
    }

    pub(crate) fn opening(&self) -> u64 {
        self.lock().opening
    }

    /// The window `key` opened `workspace`, listed as `tree`. Every other
    /// window closes. Returns the workspace's saved view state for the
    /// window to restore.
    pub(crate) fn adopt(
        &self,
        key: WindowKey,
        workspace: Arc<Workspace>,
        tree: &FileTree,
    ) -> Adopted {
        let mut state = self.lock();
        state.opening += 1;
        state.listing += 1;
        state.tree = Some(tree.clone());
        state.moves.clear();
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
        let adopted = Adopted {
            opening: state.opening,
            listing: state.listing,
            rail: state.rail,
            recents: state.recents,
            view,
        };
        drop(state);
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
            rail: state.rail,
            recents: state.recents,
        }
    }

    /// The window `key` listed the files again as `tree`, after the item
    /// at `moved.0` moved to `moved.1`. The other windows wake to show
    /// it. Returns the listing's version.
    pub(crate) fn publish_listing(
        &self,
        key: WindowKey,
        opening: u64,
        tree: &FileTree,
        moved: Option<(&str, &str)>,
    ) -> u64 {
        let mut state = self.lock();
        if opening != state.opening {
            return state.listing;
        }
        state.listing += 1;
        state.tree = Some(tree.clone());
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
        let others = state.others(key);
        drop(state);
        wake(others);
        listing
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

    /// How to wake every open window except `key`.
    fn others(&self, key: WindowKey) -> Vec<Listeners> {
        self.windows
            .iter()
            .filter(|window| window.key != key)
            .filter_map(|window| window.listeners.clone())
            .collect()
    }
}

/// Wake windows to apply a change another window made. Called without
/// the session's lock, so a listener may call back into the session.
fn wake(windows: Vec<Listeners>) {
    for listeners in windows {
        listeners.emit(Change::WorkFinished);
    }
}
