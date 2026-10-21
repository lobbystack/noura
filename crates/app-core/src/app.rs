//! A window's view state and its change stream.
//!
//! Opening a workspace walks its folder, which can take a while, so it runs
//! on a background thread. A client calls [`App::poll`] on its own thread to
//! take the result, and [`App::subscribe`] tells it when to. The windows of
//! one app share a session: the open workspace, its files and the rail
//! order.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, TryRecvError, channel},
    },
};

use local_core::WorkspaceEngine;

use crate::{
    AppError, FileTree, Layout, LoginItem, NoLoginItem, NoPendingSaves, NoSystem, OpenedFile,
    PendingSaves, Preferences, RecentWorkspace, Settings, SettingsSection, System, Tab, Workspace,
    WorkspaceService,
    commands::Registry,
    layout::LayoutStore,
    preferences::PreferencesStore,
    rail::Rail,
    search::Searcher,
    session::{Session, WindowKey},
    settings::kernel_sections,
    tabs::Tabs,
    view_state::StoredWindow,
    workspace::identity_changed,
};

mod commands;
mod drops;
mod file_actions;
mod finder;
mod ignored_files;
mod notes;
mod palette;
mod preferences;
mod settings;
mod view;

pub use drops::DragOut;
pub use file_actions::{Question, Rename};
pub use finder::{QuickOpen, Search};
pub use ignored_files::IgnoredFiles;
pub use notes::UnsavedPrompt;
pub use palette::{CommandPalette, Palette, PaletteMode};

/// What the window shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Screen {
    /// The last workspace is reopening in the background.
    Loading,
    /// No workspace is open. `problem` says why the last one couldn't open.
    Chooser { problem: Option<String> },
    /// A workspace is open.
    Workspace,
}

/// A message about an action: why it failed, or where its result went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub message: String,
    /// The recent workspace that failed to open, which the notice offers to
    /// remove from the recent list.
    pub remove_recent: Option<String>,
    pub kind: NoticeKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoticeKind {
    /// The action failed.
    Problem,
    /// The action worked, and the notice says where its result went.
    Done,
}

/// One part of the view state changed. Listeners hear about changes made
/// on any thread, including background work that [`App::poll`] can now
/// collect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Screen,
    Recents,
    Tree,
    /// The open tabs or the active tab changed.
    Tabs,
    /// The rail's pages or their order changed.
    Rail,
    /// The sidebar showed, hid, resized or folded a section.
    Layout,
    Document,
    Notice,
    /// Settings or the Ignored files page changed.
    Settings,
    /// The appearance, text size or zoom changed.
    Preferences,
    /// A command registered.
    Commands,
    /// The palette opened, closed, or changed what it shows.
    Palette,
    /// The window closed, opened another window, or Quit ran; see
    /// [`App::is_closed`], [`App::take_opened_windows`] and
    /// [`App::take_quit`].
    Window,
    /// An open note changed: its text, a save, its properties or its name,
    /// or the prompt about a note that couldn't save.
    Note,
    /// Background work finished, or another window changed what this one
    /// shows, such as the files; call [`App::poll`] to apply it.
    WorkFinished,
}

type Listener = Box<dyn Fn(Change) + Send + Sync>;

#[derive(Clone, Default)]
pub(crate) struct Listeners(Arc<Mutex<Vec<Listener>>>);

impl Listeners {
    pub(crate) fn emit(&self, change: Change) {
        let listeners = self.0.lock().unwrap_or_else(|error| error.into_inner());
        for listener in listeners.iter() {
            listener(change);
        }
    }
}

#[derive(Clone)]
enum Job {
    /// Reopen the last workspace at launch, or again with Try again.
    Restore {
        workspace_id: String,
    },
    OpenFolder(PathBuf),
    Create {
        name: String,
        folder: PathBuf,
    },
    OpenRecent {
        workspace_id: String,
        name: String,
    },
}

struct Opened {
    workspace: Workspace,
    tree: FileTree,
}

/// The workspace the windows show when a job starts. No job opens it a
/// second time, from its own folder or from a copy.
struct Shown {
    id: String,
    root: String,
    name: String,
}

impl Shown {
    fn of(workspace: &Workspace) -> Self {
        Self {
            id: workspace.id().to_owned(),
            root: workspace.root().to_owned(),
            name: workspace.name(),
        }
    }
}

/// The job running in the background and where its result arrives:
/// `None` when the folder holds the workspace that is open already.
struct Pending {
    job: Job,
    result: Receiver<Result<Option<Opened>, AppError>>,
}

/// One window's state, which a client draws: which screen shows, the
/// recent list, the open workspace with its file tree, and the window's
/// tabs. Every window of the app shares one open workspace, with one index
/// and one watcher; [`App::new_window`] opens another.
pub struct App {
    session: Arc<Session>,
    key: WindowKey,
    /// Which workspace opening this window shows. Once another window
    /// opens a different workspace, this one has closed.
    opening: u64,
    /// The versions of the shared listing, rail order, recent list and
    /// preferences this window shows.
    listing_seen: u64,
    rail_seen: u64,
    recents_seen: u64,
    preferences_seen: u64,
    closed: bool,
    /// Windows this one opened for its workspace's other saved windows,
    /// until the client takes them.
    opened_windows: Vec<App>,
    service: Arc<WorkspaceService>,
    system: Arc<dyn System>,
    screen: Screen,
    recents: Vec<RecentWorkspace>,
    /// The open workspace, which the other windows and background
    /// searches share.
    workspace: Option<Arc<Workspace>>,
    tree: Option<FileTree>,
    tabs: Tabs,
    rail: Rail,
    layout: Layout,
    layouts: LayoutStore,
    /// This device's appearance, text size and zoom.
    preferences: Preferences,
    preferences_store: PreferencesStore,
    notice: Option<Notice>,
    /// The tree row being renamed in place.
    renaming: Option<Rename>,
    /// The Ignored files page, while it is open.
    ignored_files: Option<IgnoredFiles>,
    /// Settings, while it is open.
    settings: Option<Settings>,
    /// Settings' sections, in the order they registered.
    settings_sections: Vec<SettingsSection>,
    /// The system's apps that open at login.
    login_item: Arc<dyn LoginItem>,
    /// Where the local diagnostic log lives.
    log_folder: PathBuf,
    /// A folder waiting to go to the trash until you confirm.
    pending_trash: Option<(String, Question)>,
    /// The workspace the launch couldn't reopen, which Try again retries.
    failed_restore: Option<String>,
    pending: Option<Pending>,
    listeners: Listeners,
    /// Changes waiting to be saved, which a drag out of the window saves
    /// first.
    pending_saves: Arc<dyn PendingSaves>,
    /// The native drag waiting for the desktop shell to start it.
    drag_out: Option<DragOut>,
    /// The item the last native drag carried, so a drop of it back on the
    /// tree moves it instead of copying it.
    dragged_out: Option<String>,
    registry: Registry,
    /// Commands, quick open or search, while this window's palette is
    /// open.
    palette: Option<Palette>,
    /// This window's searches, which run off the UI thread.
    searcher: Searcher,
    /// Quit ran and the app hasn't closed yet.
    quit: bool,
    /// The Markdown files open in tabs, and their saves.
    notes: crate::notes::Notes,
    /// What waits for notes to save before it happens.
    leaving: Option<notes::Leaving>,
    /// The last quit or workspace switch this window saved its notes
    /// for, which any window may ask for.
    departure_seen: u64,
}

impl App {
    /// Start the app with per-device state in `app_data` and no operating
    /// system services: deleted items go to the workspace's trash folder.
    /// See [`App::launch_with`].
    pub fn launch(app_data: impl AsRef<Path>) -> Self {
        Self::launch_with(app_data, Arc::new(NoSystem))
    }

    /// Start the app with per-device state in `app_data`, using `system`
    /// for the trash, the file manager, terminals, the clipboard and the
    /// browser. When a workspace was open last time, it reopens in the
    /// background while the screen shows [`Screen::Loading`].
    pub fn launch_with(app_data: impl AsRef<Path>, system: Arc<dyn System>) -> Self {
        let session = Session::new(app_data.as_ref(), system);
        let mut app = Self::join(&session);
        app.recents = app.service.recents();
        app.register_kernel_commands();
        app.layout = app.layouts.load();
        app.preferences = app.preferences_store.load();
        if let Some(last) = app.recents.first() {
            let workspace_id = last.workspace_id.clone();
            app.screen = Screen::Loading;
            app.start(Job::Restore { workspace_id });
        }
        app
    }

    /// A window on `session` showing the chooser.
    fn join(session: &Arc<Session>) -> Self {
        let listeners = Listeners::default();
        let key = session.join(listeners.clone());
        let answered = listeners.clone();
        Self {
            session: session.clone(),
            key,
            opening: session.opening(),
            listing_seen: 0,
            rail_seen: 0,
            recents_seen: 0,
            preferences_seen: 0,
            closed: false,
            opened_windows: Vec::new(),
            service: session.service.clone(),
            system: session.system.clone(),
            screen: Screen::Chooser { problem: None },
            recents: Vec::new(),
            workspace: None,
            tree: None,
            tabs: Tabs::default(),
            rail: Rail::default(),
            layout: Layout::default(),
            layouts: LayoutStore::new(&session.app_data),
            preferences: Preferences::default(),
            preferences_store: PreferencesStore::new(&session.app_data),
            notice: None,
            renaming: None,
            ignored_files: None,
            settings: None,
            settings_sections: kernel_sections(),
            login_item: Arc::new(NoLoginItem),
            log_folder: crate::log_folder(&session.app_data),
            pending_trash: None,
            failed_restore: None,
            pending: None,
            listeners,
            pending_saves: Arc::new(NoPendingSaves),
            drag_out: None,
            dragged_out: None,
            registry: Registry::default(),
            palette: None,
            searcher: Searcher::new(move || answered.emit(Change::WorkFinished)),
            quit: false,
            notes: crate::notes::Notes::default(),
            leaving: None,
            departure_seen: 0,
        }
    }

    /// Open another window on the open workspace, with no tabs and the
    /// file tree's folders open as here. The new window shares this
    /// one's workspace, index and watcher, and joins the windows that
    /// come back at the next launch. Returns `None` when no workspace is
    /// open, or while one is opening.
    pub fn new_window(&mut self) -> Option<App> {
        if self.is_closed() || self.workspace.is_none() || self.pending.is_some() {
            return None;
        }
        let expanded_folders = self
            .tree
            .as_ref()
            .map(FileTree::expanded_folders)
            .unwrap_or_default();
        let mut window = self.spawn_window(StoredWindow {
            expanded_folders,
            ..StoredWindow::default()
        });
        window.save_view();
        Some(window)
    }

    /// The windows this one opened for its workspace's other saved
    /// windows, such as at launch. The client shows each one.
    pub fn take_opened_windows(&mut self) -> Vec<App> {
        std::mem::take(&mut self.opened_windows)
    }

    /// A window on this one's workspace showing `stored`.
    fn spawn_window(&self, stored: StoredWindow) -> App {
        let mut window = Self::join(&self.session);
        let current = self.session.current();
        window.opening = self.opening;
        window.listing_seen = current.listing;
        window.rail_seen = current.rail;
        window.recents_seen = current.recents;
        window.screen = Screen::Workspace;
        window.recents = self.recents.clone();
        window.workspace = self.workspace.clone();
        window.tree = current.tree;
        window.rail = self.rail.clone();
        window.layout = self.layout.clone();
        window.preferences = self.preferences;
        window.preferences_seen = self.preferences_seen;
        // The commands and Settings sections registered so far, what saves
        // pending changes and the login item serve every window.
        window.registry = self.registry.copy();
        window.settings_sections = self.settings_sections.clone();
        window.pending_saves = self.pending_saves.clone();
        window.login_item = self.login_item.clone();
        window.restore_view(stored);
        window
    }

    /// Close this window once its notes are saved, as Close Window and
    /// the window's close button do: the window stays while saves run,
    /// and asks when one fails. The other windows keep their tabs, and
    /// this one's tabs leave the windows that come back at the next
    /// launch, unless it was the last window open: then it comes back, as
    /// when Noura quits.
    pub fn close_window(&mut self) {
        if self.closed {
            return;
        }
        self.leave(notes::Leave::CloseWindow);
    }

    /// Close this window now: its notes are on disk.
    fn close_window_now(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.session.close(self.key);
        self.release();
        self.listeners.emit(Change::Window);
    }

    /// Let go of the workspace once the window closed, so a workspace
    /// another window replaced closes its index without waiting for the
    /// client to drop this window.
    fn release(&mut self) {
        self.pending = None;
        self.workspace = None;
        self.tree = None;
        self.renaming = None;
        self.ignored_files = None;
        self.pending_trash = None;
        self.drag_out = None;
        self.dragged_out = None;
        self.searcher.cancel();
        self.palette = None;
        self.settings = None;
        self.notes.sessions.clear();
        self.notes.unreadable.clear();
        self.leaving = None;
        self.listeners.emit(Change::Screen);
    }

    /// Whether this window has closed: with [`App::close_window`], or
    /// because another window opened a different workspace. A client
    /// stops drawing a closed window.
    pub fn is_closed(&self) -> bool {
        self.closed || self.opening != self.session.opening()
    }

    /// Call `listener` after each change, from whichever thread made it.
    pub fn subscribe(&self, listener: impl Fn(Change) + Send + Sync + 'static) {
        self.listeners
            .0
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .push(Box::new(listener));
    }

    pub fn screen(&self) -> &Screen {
        &self.screen
    }

    /// Whether a workspace is opening.
    pub fn is_busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Recent workspaces, most recent first.
    pub fn recents(&self) -> &[RecentWorkspace] {
        &self.recents
    }

    pub fn workspace(&self) -> Option<&Workspace> {
        self.workspace.as_deref()
    }

    pub fn tree(&self) -> Option<&FileTree> {
        self.tree.as_ref()
    }

    /// The file tree, to move its focus or type ahead in it. Opening and
    /// closing folders goes through `App`'s own methods, such as
    /// [`App::activate_tree_row`] and [`App::collapse_all`], so the
    /// workspace remembers which folders are open.
    pub fn tree_mut(&mut self) -> Option<&mut FileTree> {
        self.tree.as_mut()
    }

    /// The file shown in the document area: the active tab's file.
    pub fn document(&self) -> Option<&OpenedFile> {
        self.tabs.active().and_then(Tab::file)
    }

    /// The open tabs, in order.
    pub fn tabs(&self) -> &[Tab] {
        self.tabs.list()
    }

    /// The tab the window shows.
    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.active()
    }

    pub fn notice(&self) -> Option<&Notice> {
        self.notice.as_ref()
    }

    pub fn dismiss_notice(&mut self) {
        if self.notice.take().is_some() {
            self.listeners.emit(Change::Notice);
        }
    }

    /// Open the workspace in `folder`, or make one of a plain folder. Every
    /// window's open notes save first, since the other windows close.
    pub fn open_folder(&mut self, folder: PathBuf) {
        // It's open already, in every window. The background job also
        // catches the folder spelled another way, before it opens anything.
        if self
            .workspace
            .as_ref()
            .is_some_and(|workspace| folder.to_str() == Some(workspace.root()))
        {
            return;
        }
        self.depart(notes::Departure::Open(Job::OpenFolder(folder)));
    }

    /// Create a workspace named `name` in `folder` and open it. Every
    /// window's open notes save first.
    pub fn create_workspace(&mut self, name: &str, folder: PathBuf) {
        self.depart(notes::Departure::Open(Job::Create {
            name: name.to_owned(),
            folder,
        }));
    }

    /// Open a workspace from the recent list. Every window's open notes
    /// save first.
    pub fn open_recent(&mut self, workspace_id: &str) {
        // It's open already, in every window.
        if self
            .workspace
            .as_ref()
            .is_some_and(|workspace| workspace.id() == workspace_id)
        {
            return;
        }
        let name = self
            .recents
            .iter()
            .find(|recent| recent.workspace_id == workspace_id)
            .map_or_else(String::new, |recent| recent.name.clone());
        self.depart(notes::Departure::Open(Job::OpenRecent {
            workspace_id: workspace_id.to_owned(),
            name,
        }));
    }

    /// Retry the workspace the launch couldn't reopen. Never opens another
    /// workspace instead, and does nothing once that workspace has left the
    /// recent list.
    pub fn try_again(&mut self) {
        if let Some(workspace_id) = self.failed_restore.clone() {
            self.start(Job::Restore { workspace_id });
        }
    }

    /// Drop a workspace from the recent list. Its folder stays as it is.
    pub fn remove_recent(&mut self, workspace_id: &str) {
        match self.service.remove_recent(workspace_id) {
            Ok(recents) => {
                self.recents = recents;
                self.recents_seen = self.session.recents_changed(self.key);
                self.listeners.emit(Change::Recents);
                // The problem described a workspace that is no longer listed,
                // so there is nothing left to try again.
                if self.failed_restore.as_deref() == Some(workspace_id) {
                    self.failed_restore = None;
                    if matches!(self.screen, Screen::Chooser { problem: Some(_) }) {
                        self.set_screen(Screen::Chooser { problem: None });
                    }
                }
                if self
                    .notice
                    .as_ref()
                    .is_some_and(|notice| notice.remove_recent.as_deref() == Some(workspace_id))
                {
                    self.dismiss_notice();
                }
            }
            Err(error) => self.show_notice("Couldn't update the recent list", error, None),
        }
    }

    /// Apply finished background work, such as note saves, and what other
    /// windows changed, such as the files. Returns whether anything
    /// changed.
    pub fn poll(&mut self) -> bool {
        if self.closed {
            return false;
        }
        if self.is_closed() {
            self.closed_by_another_window();
            return true;
        }
        let saved = self.collect_note_writes();
        let searched = self.take_search_answer();
        let opened = self.take_opened();
        let listed = self.poll_listing();
        let rail = self.poll_rail();
        let recents = self.poll_recents();
        let preferences = self.poll_preferences();
        let departing = self.poll_departure();
        saved || searched || opened || listed || rail || recents || preferences || departing
    }

    /// Take the workspace a background job opened, or why it couldn't.
    fn take_opened(&mut self) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };
        let result = match pending.result.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Err(AppError::new(
                "worker_failed",
                "Noura couldn't open the workspace.",
            )),
        };
        let Some(Pending { job, .. }) = self.pending.take() else {
            return false;
        };
        self.recents = self.service.recents();
        self.listeners.emit(Change::Recents);
        let result = match result {
            Ok(Some(opened)) => self.adopt(opened),
            Ok(None) => Ok(()),
            Err(error) => Err(error),
        };
        match (result, job) {
            (Ok(()), _) => {}
            (Err(error), Job::Restore { workspace_id }) => {
                self.workspace = None;
                self.tree = None;
                self.close_palette();
                self.rail.set_saved_order(Vec::new());
                self.restore_view(StoredWindow::default());
                self.renaming = None;
                self.ignored_files = None;
                self.pending_trash = None;
                self.drag_out = None;
                self.dragged_out = None;
                self.failed_restore = Some(workspace_id);
                self.set_screen(Screen::Chooser {
                    problem: Some(error.message),
                });
            }
            (Err(error), Job::OpenRecent { workspace_id, name }) => {
                let title = format!("Couldn't open \u{201c}{name}\u{201d}");
                self.show_notice(&title, error, Some(workspace_id));
            }
            (Err(error), Job::OpenFolder(_)) => {
                self.show_notice("Couldn't open the folder", error, None);
            }
            (Err(error), Job::Create { .. }) => {
                self.show_notice("Couldn't create the workspace", error, None);
            }
        }
        true
    }

    /// Show the rail order another window gave the workspace.
    fn poll_rail(&mut self) -> bool {
        let Some((version, order)) = self.session.rail_since(self.rail_seen) else {
            return false;
        };
        self.rail_seen = version;
        self.rail.set_saved_order(order);
        self.listeners.emit(Change::Rail);
        true
    }

    /// Read the recent list again after another window changed it.
    fn poll_recents(&mut self) -> bool {
        let Some(version) = self.session.recents_since(self.recents_seen) else {
            return false;
        };
        self.recents_seen = version;
        self.recents = self.service.recents();
        self.listeners.emit(Change::Recents);
        true
    }

    /// Show the preferences another window changed.
    fn poll_preferences(&mut self) -> bool {
        let Some((version, preferences)) = self.session.preferences_since(self.preferences_seen)
        else {
            return false;
        };
        self.preferences_seen = version;
        if self.preferences == preferences {
            return false;
        }
        self.preferences = preferences;
        self.listeners.emit(Change::Preferences);
        true
    }

    /// Another window opened a different workspace, so this one closed.
    /// Its tabs stay saved with the workspace it showed.
    fn closed_by_another_window(&mut self) {
        self.closed = true;
        self.release();
    }

    /// Show the workspace a job opened in this window, and close the
    /// others. A job that opened the workspace the windows show changes
    /// nothing, and fails when it opened a copy of it.
    fn adopt(&mut self, opened: Opened) -> Result<(), AppError> {
        if let Some(shown) = self
            .workspace
            .clone()
            .filter(|workspace| workspace.id() == opened.workspace.id())
        {
            return self.opened_again(&shown, opened);
        }
        let workspace = Arc::new(opened.workspace);
        let adopted = self
            .session
            .adopt(self.key, workspace.clone(), &opened.tree);
        self.opening = adopted.opening;
        self.listing_seen = adopted.listing;
        self.rail_seen = adopted.rail;
        self.recents_seen = adopted.recents;
        self.failed_restore = None;
        self.close_palette();
        self.workspace = Some(workspace);
        self.tree = Some(opened.tree);
        self.rail.set_saved_order(adopted.view.rail_order);
        self.listeners.emit(Change::Rail);
        let mut windows = adopted.view.windows.into_iter();
        self.restore_view(windows.next().unwrap_or_default());
        self.notice = None;
        self.renaming = None;
        self.ignored_files = None;
        self.pending_trash = None;
        self.drag_out = None;
        self.dragged_out = None;
        self.listeners.emit(Change::Tree);
        self.listeners.emit(Change::Tabs);
        self.listeners.emit(Change::Document);
        self.listeners.emit(Change::Notice);
        self.set_screen(Screen::Workspace);
        // The workspace's other saved windows come back too.
        for stored in windows {
            let window = self.spawn_window(stored);
            self.opened_windows.push(window);
        }
        Ok(())
    }

    /// A job opened the workspace the windows show, which [`run`] checks
    /// for before it opens anything, so a folder changed meanwhile. The
    /// windows keep the engine they share, and the recent list, which the
    /// job's engine changed, goes back to the folder they show. The job's
    /// engine reconciled the shared index with its own folder, so the
    /// open engine reconciles it again.
    fn opened_again(&mut self, shown: &Arc<Workspace>, opened: Opened) -> Result<(), AppError> {
        let copy = opened.workspace.root() != shown.root();
        let folder = opened.workspace.root().to_owned();
        drop(opened);
        if !copy {
            self.refresh_tree(None);
            return Ok(());
        }
        match self.service.remember(shown) {
            Ok(recents) => {
                self.recents = recents;
                self.recents_seen = self.session.recents_changed(self.key);
                self.listeners.emit(Change::Recents);
            }
            Err(error) => {
                tracing::warn!(code = %error.code, "the recent list kept a copy of the workspace");
            }
        }
        self.rescan();
        Err(copy_of_shown(&folder, &Shown::of(shown)))
    }

    fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
        self.listeners.emit(Change::Screen);
    }

    fn show_notice(&mut self, title: &str, error: AppError, remove_recent: Option<String>) {
        self.notice = Some(Notice {
            title: title.to_owned(),
            message: error.message,
            remove_recent,
            kind: NoticeKind::Problem,
        });
        self.listeners.emit(Change::Notice);
        if self.screen == Screen::Loading {
            self.set_screen(Screen::Chooser { problem: None });
        }
    }

    /// Run `job` on a background thread. One job runs at a time; requests
    /// made meanwhile are ignored, as the controls that send them are off.
    fn start(&mut self, job: Job) {
        if self.pending.is_some() {
            return;
        }
        if self.notice.take().is_some() {
            self.listeners.emit(Change::Notice);
        }
        let (sender, receiver) = channel();
        let service = self.service.clone();
        let listeners = self.listeners.clone();
        let work = job.clone();
        let shown = self.workspace.as_deref().map(Shown::of);
        let spawned = std::thread::Builder::new()
            .name("workspace-open".into())
            .spawn(move || {
                let result = run(&service, &work, shown.as_ref());
                // The app may have closed meanwhile; nothing is waiting then.
                let _ = sender.send(result);
                listeners.emit(Change::WorkFinished);
            });
        match spawned {
            Ok(_) => {
                self.pending = Some(Pending {
                    job,
                    result: receiver,
                });
            }
            Err(error) => {
                tracing::error!(%error, "could not start the workspace thread");
                let error = AppError::new("worker_failed", "Noura couldn't open the workspace.");
                self.show_notice("Couldn't open the workspace", error, None);
            }
        }
    }
}

/// Open what `job` asks for. `shown` is the workspace the windows show, if
/// any. Returns `None` when the job names that workspace's folder.
fn run(
    service: &WorkspaceService,
    job: &Job,
    shown: Option<&Shown>,
) -> Result<Option<Opened>, AppError> {
    if let Some(shown) = shown
        && opens_shown(service, job, shown)?
    {
        return Ok(None);
    }
    let workspace = match job {
        Job::Restore { workspace_id } | Job::OpenRecent { workspace_id, .. } => {
            service.open_recent(workspace_id)?
        }
        Job::OpenFolder(folder) => service.open_folder(folder)?,
        Job::Create { name, folder } => service.create(name, folder)?,
    };
    let tree = workspace.file_tree()?;
    Ok(Some(Opened { workspace, tree }))
}

/// Whether `job` names the folder of the workspace the windows show,
/// spelled any way: through a link, with `.` or `..`, or in other letter
/// case. Fails when the job's folder holds that workspace somewhere else,
/// as a copy made in the file manager does, since a copy keeps the
/// workspace ID. Either way, opening the folder would start a second
/// engine and watcher on the workspace's one index, and record the
/// folder in the recent list, so this runs before anything opens.
fn opens_shown(service: &WorkspaceService, job: &Job, shown: &Shown) -> Result<bool, AppError> {
    let (folder, saved_id) = match job {
        Job::OpenFolder(folder) => (folder.clone(), None),
        Job::Restore { workspace_id } | Job::OpenRecent { workspace_id, .. } => {
            let Some(recent) = service
                .recents()
                .into_iter()
                .find(|recent| &recent.workspace_id == workspace_id)
            else {
                return Ok(false);
            };
            (PathBuf::from(recent.path), Some(workspace_id.as_str()))
        }
        // A new workspace gets its own ID, and a folder that holds a
        // workspace already gets none.
        Job::Create { .. } => return Ok(false),
    };
    // A path Noura can't show opens nothing; the job says why.
    let Some(path) = folder.to_str() else {
        return Ok(false);
    };
    let same_folder = std::fs::canonicalize(&folder)
        .is_ok_and(|folder| folder.as_os_str() == shown.root.as_str());
    // The open itself reports a folder it can't read.
    let holds_shown = same_folder
        || WorkspaceEngine::read_manifest_at(&folder)
            .ok()
            .flatten()
            .is_some_and(|manifest| manifest.id == shown.id);
    if !holds_shown {
        return Ok(false);
    }
    match saved_id {
        // The recent entry names another workspace than the folder holds.
        Some(saved_id) if saved_id != shown.id => Err(identity_changed(path)),
        _ if same_folder => Ok(true),
        _ => Err(copy_of_shown(path, shown)),
    }
}

/// The folder at `path` holds a copy of the workspace the windows show.
fn copy_of_shown(path: &str, shown: &Shown) -> AppError {
    let Shown { name, root, .. } = shown;
    AppError::new(
        "workspace_already_open",
        format!(
            "{path} holds a copy of \u{201c}{name}\u{201d}, which you have open from {root}. \
             Noura opens one copy at a time."
        ),
    )
}

impl Drop for App {
    fn drop(&mut self) {
        // Quitting drops every window without closing it, so each one's
        // tabs stay saved for the next launch.
        self.session.leave(self.key);
    }
}
