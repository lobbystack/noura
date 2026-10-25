//! A window's view state and its change stream.
//!
//! Opening a workspace walks its folder, which can take a while, so it runs
//! on a background thread. A client calls [`App::poll`] on its own thread to
//! take the result, and [`App::subscribe`] tells it when to. The windows of
//! one app share a session: the open workspace, the thread that keeps it
//! in step with its folder, its files, the rail order and the plugins that
//! run.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, TryRecvError, channel},
    },
    time::Instant,
};

use local_core::{ChangeCounts, WorkspaceEngine};

use crate::{
    AppError, FileTree, Layout, LoginItem, NoLoginItem, NoPendingSaves, NoSystem, OpenedFile,
    PendingSaves, PendingWrites, PluginInstall, Preferences, Problem, RecentWorkspace, Settings,
    SettingsSection, System, Tab, UpdateSource, Updates, Workspace, WorkspaceService,
    commands::Registry,
    inbox::Sections,
    layout::LayoutStore,
    preferences::PreferencesStore,
    rail::Rail,
    search::Searcher,
    session::{Session, WindowKey},
    settings::kernel_sections,
    tabs::Tabs,
    view_state::StoredWindow,
    watch::{Rescans, Snapshot},
    workspace::identity_changed,
};

mod ai;
mod commands;
mod drops;
mod file_actions;
mod finder;
mod ignored_files;
mod inbox;
mod links;
mod notes;
mod palette;
mod plugins;
mod preferences;
mod settings;
mod view;
mod viewers;
mod web;

pub use drops::DragOut;
pub use file_actions::{Question, Rename};
pub use finder::{QuickOpen, Search};
pub use ignored_files::IgnoredFiles;
pub use notes::UnsavedPrompt;
pub use palette::{CommandPalette, Palette, PaletteMode};
pub use plugins::{NameDraft, PluginPlace};

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

/// Who changed files in the workspace.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Noura wrote the change.
    Noura,
    /// Another app, a cloud drive or another device changed the files.
    Outside,
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
    /// The update status changed.
    Update,
    /// Settings or the Ignored files page changed.
    Settings,
    /// The appearance, text size or zoom changed.
    Preferences,
    /// A command registered.
    Commands,
    /// A plugin installed, uninstalled, turned on or off, or drew its
    /// pages again, or the install dialog changed.
    Plugins,
    /// The palette opened, closed, or changed what it shows.
    Palette,
    /// The window closed, opened another window, or Quit ran; see
    /// [`App::is_closed`], [`App::take_opened_windows`] and
    /// [`App::take_quit`].
    Window,
    /// An open note changed: its text, a save, its properties or its name,
    /// or the prompt about a note that couldn't save.
    Note,
    /// A PDF or image tab changed: its file opened or changed, a page
    /// drew, a search ended, or you moved to another page or zoom.
    Viewer,
    /// Background work finished, or another window changed what this one
    /// shows, such as the files; call [`App::poll`] to apply it.
    WorkFinished,
    /// Files in the open workspace changed on disk; call [`App::poll`] to
    /// apply the change to the views.
    Files(Origin),
    /// The open workspace's problems changed.
    Problems,
    /// The Inbox's recent notes or plugin sections changed.
    Inbox,
    /// The AI providers, your consent for them, or the consent prompt
    /// changed.
    Ai,
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
    snapshot: Snapshot,
    /// The engine's change counts when the snapshot was listed.
    counts: ChangeCounts,
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
    /// The version of the plugins service this window shows.
    plugins_seen: u64,
    /// The version of the AI service this window shows.
    ai_seen: u64,
    /// How many times running plugins ran code that may write files, as
    /// far as this window's open notes looked at their files.
    plugin_acts_seen: u64,
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
    /// The plugin install dialog, while it is open.
    plugin_install: Option<PluginInstall>,
    /// Brings the install dialog of a release that is downloading.
    plugin_download: Option<Receiver<PluginInstall>>,
    /// Settings' sections, in the order they registered.
    settings_sections: Vec<SettingsSection>,
    /// The system's apps that open at login.
    login_item: Arc<dyn LoginItem>,
    /// Where the local diagnostic log lives.
    log_folder: PathBuf,
    /// The third-party license list this copy's package ships, if any.
    third_party_licenses: Option<PathBuf>,
    /// A folder waiting to go to the trash until you confirm.
    pending_trash: Option<(String, Question)>,
    /// The workspace the launch couldn't reopen, which Try again retries.
    failed_restore: Option<String>,
    pending: Option<Pending>,
    writes: PendingWrites,
    updates: Updates,
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
    /// The PDF and image tabs' viewers.
    viewers: crate::viewers::Viewers,
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
    /// The open workspace's problems, such as files that share an ID, as
    /// this window shows them.
    problems: Vec<Problem>,
    /// The paths of the notes changed last, newest first.
    recent: Vec<String>,
    /// The Inbox sections enabled plugins add.
    inbox_sections: Sections,
    /// What you typed in plugins' name fields that hasn't reached them.
    name_drafts: Vec<NameDraft>,
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
        Self::launch_with_rescans(app_data, system, Rescans::default())
    }

    /// [`App::launch_with`] with other intervals for the full rescans that
    /// back up the native watcher.
    pub fn launch_with_rescans(
        app_data: impl AsRef<Path>,
        system: Arc<dyn System>,
        rescans: Rescans,
    ) -> Self {
        let session = Session::new(app_data.as_ref(), system, rescans);
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
            plugins_seen: 0,
            ai_seen: 0,
            plugin_acts_seen: 0,
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
            plugin_install: None,
            plugin_download: None,
            settings_sections: kernel_sections(),
            login_item: Arc::new(NoLoginItem),
            log_folder: crate::log_folder(&session.app_data),
            third_party_licenses: None,
            pending_trash: None,
            failed_restore: None,
            pending: None,
            updates: session.updates.clone(),
            writes: session.writes.clone(),
            listeners,
            pending_saves: Arc::new(NoPendingSaves),
            drag_out: None,
            dragged_out: None,
            registry: Registry::default(),
            viewers: crate::viewers::Viewers::default(),
            palette: None,
            searcher: Searcher::new(move || answered.emit(Change::WorkFinished)),
            quit: false,
            notes: crate::notes::Notes::default(),
            leaving: None,
            departure_seen: 0,
            problems: Vec::new(),
            recent: Vec::new(),
            inbox_sections: Sections::default(),
            name_drafts: Vec::new(),
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
        window.problems = current.problems;
        window.recent = current.recent;
        window.rail = self.rail.clone();
        window.layout = self.layout.clone();
        window.preferences = self.preferences;
        window.preferences_seen = self.preferences_seen;
        window.plugins_seen = self.plugins_seen;
        window.plugin_acts_seen = self.plugin_acts_seen;
        // The commands and Settings sections registered so far, what saves
        // pending changes, the login item and the packaged license list
        // serve every window.
        window.registry = self.registry.copy();
        window.settings_sections = self.settings_sections.clone();
        window.inbox_sections = self.inbox_sections.clone();
        window.pending_saves = self.pending_saves.clone();
        window.login_item = self.login_item.clone();
        window
            .third_party_licenses
            .clone_from(&self.third_party_licenses);
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
        self.session.plugins().forget_openings(self.key);
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
        self.viewers.clear();
        self.leaving = None;
        self.plugin_install = None;
        self.plugin_download = None;
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
    /// [`App::click_tree_row`] and [`App::collapse_all`], so the
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

    /// The open workspace's problems, such as files that share an ID.
    pub fn problems(&self) -> &[Problem] {
        &self.problems
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

    /// Writes that must reach disk before Noura restarts. Services that
    /// write files on another thread register each write here. Every
    /// window shares the list.
    pub fn pending_writes(&self) -> &PendingWrites {
        &self.writes
    }

    /// Where updating stands. Every window shares the update service.
    pub fn updates(&self) -> &Updates {
        &self.updates
    }

    /// Check `source` for updates, first [`crate::FIRST_UPDATE_CHECK`]
    /// after `launched` and then every [`crate::UPDATE_CHECK_INTERVAL`].
    pub fn enable_updates(
        &mut self,
        current_version: &str,
        source: Arc<dyn UpdateSource>,
        launched: Instant,
    ) {
        self.updates.enable(current_version, source, launched);
    }

    /// Say that this build doesn't update itself, and why.
    pub fn disable_updates(&mut self, current_version: &str, reason: &str) {
        self.updates.disable(current_version, reason);
    }

    /// Check for updates now, as Settings' Check for updates does.
    pub fn check_for_updates(&mut self) {
        self.updates.check();
    }

    /// Save every window's notes, then save every change still waiting,
    /// wait for pending writes, install the downloaded update, save and
    /// wait again for changes made during the install, and restart. A note
    /// that couldn't save asks first, as when Noura quits, and Keep editing
    /// keeps this version running. If a save, a write or the install
    /// fails, this version keeps running and the update status says why.
    pub fn restart_to_update(&mut self) {
        self.depart(notes::Departure::Restart);
    }

    /// Stop offering Restart outside Settings until the next update, in
    /// every window.
    pub fn hide_update_offer(&mut self) {
        self.updates.hide_offer();
    }

    /// Apply finished background work, such as note saves, and what other
    /// windows changed, such as the files, and start an update check that
    /// is due. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        self.poll_at(Instant::now())
    }

    /// [`App::poll`] with the time given, so tests can step through the
    /// update schedule.
    pub fn poll_at(&mut self, now: Instant) -> bool {
        if self.closed {
            return false;
        }
        if self.is_closed() {
            self.closed_by_another_window();
            return true;
        }
        let saved = self.collect_note_writes();
        let updated = self.updates.poll(now);
        let searched = self.take_search_answer();
        let opened = self.take_opened();
        let listed = self.poll_listing();
        let rail = self.poll_rail();
        let recents = self.poll_recents();
        let preferences = self.poll_preferences();
        let departing = self.poll_departure();
        let plugins = self.poll_plugins();
        let ai = self.poll_ai();
        let viewed = self.poll_viewers();
        saved
            | updated
            | searched
            | opened
            | listed
            | rail
            | recents
            | preferences
            | departing
            | plugins
            | ai
            | viewed
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
                self.set_problems(Vec::new());
                self.set_recent(Vec::new());
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

    /// Show the problems of a new listing of the files.
    pub(super) fn set_problems(&mut self, problems: Vec<Problem>) {
        if self.problems != problems {
            self.problems = problems;
            self.listeners.emit(Change::Problems);
        }
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
        let adopted = self.session.adopt(
            self.key,
            workspace.clone(),
            &opened.tree,
            &opened.snapshot,
            opened.counts,
        );
        self.opening = adopted.opening;
        self.listing_seen = adopted.listing;
        self.rail_seen = adopted.rail;
        self.recents_seen = adopted.recents;
        self.failed_restore = None;
        self.close_palette();
        self.workspace = Some(workspace.clone());
        self.tree = Some(opened.tree);
        self.problems = opened.snapshot.problems;
        self.recent = opened.snapshot.recent;
        self.rail.set_saved_order(adopted.view.rail_order);
        self.forget_plugin_pages();
        self.listeners.emit(Change::Rail);
        let mut windows = adopted.view.windows.into_iter();
        self.restore_view(windows.next().unwrap_or_default());
        // The workspace's plugins start, and their pages bring back the
        // tabs that waited for them.
        self.session.ai.open_workspace(Some(workspace.clone()));
        self.session.plugins().open_workspace(Some(workspace));
        self.poll_plugins();
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
        self.listeners.emit(Change::Problems);
        self.inbox_sections.reload();
        self.listeners.emit(Change::Inbox);
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
        // Opening a workspace can write its manifest and the recent list,
        // so restarting to update and quitting wait for it. Its notice
        // reports a failure.
        let writing = self.writes.begin_work("workspace");
        let spawned = std::thread::Builder::new()
            .name("workspace-open".into())
            .spawn(move || {
                let result = run(&service, &work, shown.as_ref());
                drop(writing);
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
    let (snapshot, counts) = Snapshot::of(workspace.engine(), workspace.root())?;
    let tree = FileTree::from_entries(&snapshot.entries);
    Ok(Some(Opened {
        workspace,
        snapshot,
        counts,
        tree,
    }))
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

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicBool, Ordering},
            mpsc::{Receiver, channel},
        },
        time::{Duration, Instant},
    };

    use local_core::{CreateObjectInput, WorkspaceEngine};

    use super::*;

    /// How long a test waits for the change stream before it fails.
    const PATIENCE: Duration = Duration::from_secs(15);

    /// How long a test keeps listening once the view shows a write, to
    /// catch a late report that would mark it as from outside. The watch
    /// thread polls every 100 ms, and platforms report within a second.
    const SETTLE: Duration = Duration::from_secs(2);

    /// An app with an empty workspace open and every change it announces.
    /// The tests write through the engine, as the app's file operations
    /// do.
    struct Live {
        app: App,
        changes: Receiver<Change>,
        seen: Vec<Change>,
        _app_data: tempfile::TempDir,
        _folder: tempfile::TempDir,
    }

    impl Live {
        fn open() -> Self {
            Self::open_with(Rescans::default())
        }

        fn open_with(rescans: Rescans) -> Self {
            let app_data = tempfile::tempdir().unwrap();
            let folder = tempfile::tempdir().unwrap();
            let mut app = App::launch_with_rescans(app_data.path(), Arc::new(NoSystem), rescans);
            let (sender, changes) = channel();
            app.subscribe(move |change| {
                let _ = sender.send(change);
            });
            app.open_folder(folder.path().to_owned());
            let mut live = Self {
                app,
                changes,
                seen: Vec::new(),
                _app_data: app_data,
                _folder: folder,
            };
            live.wait_until("the workspace opens", |app| {
                !app.is_busy() && app.workspace().is_some()
            });
            live.seen.clear();
            live
        }

        fn engine(&self) -> &WorkspaceEngine {
            self.app.workspace().unwrap().engine()
        }

        /// Create a note at `relative_path` as Noura does, and return its ID.
        fn create_note(&self, relative_path: &str) -> String {
            self.engine()
                .create_object(CreateObjectInput {
                    object_type: "note".into(),
                    title: "Mine".into(),
                    body: String::new(),
                    relative_path: Some(relative_path.into()),
                    properties: BTreeMap::new(),
                })
                .unwrap()
                .value
                .id
        }

        /// Wait on the change stream, polling after each change, until
        /// `done` holds for the view state.
        fn wait_until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
            let deadline = Instant::now() + PATIENCE;
            loop {
                self.app.poll();
                self.seen.extend(self.changes.try_iter());
                if done(&self.app) {
                    return;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                assert!(!left.is_zero(), "timed out waiting until {what}");
                if let Ok(change) = self.changes.recv_timeout(left) {
                    self.seen.push(change);
                }
            }
        }

        /// Keep polling and recording changes for `SETTLE`.
        fn settle(&mut self) {
            let deadline = Instant::now() + SETTLE;
            loop {
                self.app.poll();
                self.seen.extend(self.changes.try_iter());
                let left = deadline.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return;
                }
                if let Ok(change) = self
                    .changes
                    .recv_timeout(left.min(Duration::from_millis(100)))
                {
                    self.seen.push(change);
                }
            }
        }
    }

    fn rows(app: &App) -> Vec<String> {
        app.tree()
            .map(|tree| {
                tree.rows()
                    .iter()
                    .map(|row| row.relative_path.clone())
                    .collect()
            })
            .unwrap_or_default()
    }

    fn path_of(app: &App, id: &str) -> Option<String> {
        app.workspace()?
            .find_object(id)
            .ok()
            .flatten()
            .map(|object| object.relative_path)
    }

    #[test]
    fn a_file_noura_writes_reaches_the_tree_marked_as_noura_s_own() {
        let mut live = Live::open();

        live.create_note("Mine.md");

        live.wait_until("the note shows in the tree", |app| rows(app) == ["Mine.md"]);
        live.settle();
        assert!(live.seen.contains(&Change::Files(Origin::Noura)));
        assert!(!live.seen.contains(&Change::Files(Origin::Outside)));
    }

    /// A burst of Noura's own writes is the usual reason the watcher loses
    /// events. The full rescan that follows finds nothing Noura didn't
    /// announce, so the writes stay Noura's own.
    #[test]
    fn noura_s_writes_stay_its_own_when_the_watcher_loses_their_events() {
        let mut live = Live::open();
        live.app
            .workspace()
            .unwrap()
            .force_watch_fault(crate::WatchFault::Overflow);

        live.create_note("Mine.md");

        live.wait_until("the note shows in the tree", |app| rows(app) == ["Mine.md"]);
        live.settle();
        assert!(live.seen.contains(&Change::Files(Origin::Noura)));
        assert!(
            !live.seen.contains(&Change::Files(Origin::Outside)),
            "{:?}",
            live.seen
        );
    }

    /// The folders a write creates, and the folders whose listing it
    /// changes, are Noura's own changes too.
    #[test]
    fn notes_noura_writes_into_new_folders_moves_and_deletes_are_never_marked_as_from_outside() {
        let mut live = Live::open();

        let id = live.create_note("Inbox/Deep/Mine.md");
        live.wait_until("the note's folders show", |app| {
            rows(app) == ["Inbox"] && path_of(app, &id).as_deref() == Some("Inbox/Deep/Mine.md")
        });

        let revision = live.engine().get_object(&id).unwrap().unwrap().revision;
        live.engine()
            .move_object(&id, "Archive/2026/Mine.md", &revision)
            .unwrap();
        live.wait_until("the moved note shows", |app| {
            rows(app) == ["Archive", "Inbox"]
                && path_of(app, &id).as_deref() == Some("Archive/2026/Mine.md")
        });

        live.engine().delete_object(&id, &revision).unwrap();
        live.wait_until("the note is gone", |app| path_of(app, &id).is_none());

        live.settle();
        assert!(live.seen.contains(&Change::Files(Origin::Noura)));
        assert!(
            !live.seen.contains(&Change::Files(Origin::Outside)),
            "{:?}",
            live.seen
        );
    }

    /// Noura writes back to back while the watch thread lists and walks
    /// the whole workspace in every round. No walk catches a write between
    /// its disk change and its index update, so none reads as another
    /// app's, and a moved note never shows as two files with one ID.
    #[test]
    fn a_quick_run_of_noura_writes_never_reads_as_from_outside_or_as_a_shared_id() {
        const NOTES: usize = 60;
        let mut live = Live::open_with(Rescans {
            periodic: Duration::ZERO,
            without_watcher: Duration::ZERO,
        });
        let engine = live.app.workspace().unwrap().shared_engine();
        // Another thread reads the index's shared IDs the whole time.
        let stop = Arc::new(AtomicBool::new(false));
        let prober = std::thread::spawn({
            let engine = engine.clone();
            let stop = stop.clone();
            move || {
                let mut shared = Vec::new();
                while !stop.load(Ordering::SeqCst) {
                    shared.extend(engine.identity_conflicts().unwrap());
                    std::thread::sleep(Duration::from_millis(1));
                }
                shared
            }
        });

        let mut shared_in_view = Vec::new();
        for note in 0..NOTES {
            let id = live.create_note(&format!("Inbox/{note}/Mine.md"));
            let revision = engine.get_object(&id).unwrap().unwrap().revision;
            engine
                .move_object(&id, &format!("Archive/{note}/Mine.md"), &revision)
                .unwrap();
            live.app.poll();
            shared_in_view.extend(
                live.app
                    .problems()
                    .iter()
                    .filter(|problem| problem.code == "identity_conflict")
                    .cloned(),
            );
        }
        live.wait_until("every moved note shows", |app| {
            app.workspace().is_some_and(|workspace| {
                workspace.objects().is_ok_and(|objects| {
                    objects.len() == NOTES
                        && objects
                            .iter()
                            .all(|object| object.relative_path.starts_with("Archive/"))
                })
            }) && rows(app) == ["Archive", "Inbox"]
        });
        live.settle();
        stop.store(true, Ordering::SeqCst);
        let shared_in_index = prober.join().unwrap();

        assert!(live.seen.contains(&Change::Files(Origin::Noura)));
        assert!(
            !live.seen.contains(&Change::Files(Origin::Outside)),
            "{:?}",
            live.seen
        );
        assert!(!live.seen.contains(&Change::Problems), "{:?}", live.seen);
        assert_eq!(shared_in_view, []);
        assert_eq!(shared_in_index, []);
        assert_eq!(live.app.problems(), []);
    }

    /// A file Noura wrote that the cloud provider then takes online-only
    /// shows as not downloaded, and Noura doesn't read it to compare it
    /// with what it wrote. The offline attribute is the one online-only
    /// mark a test can set without a cloud sync root, and Windows still
    /// lets Noura read the file, so reading it would hide the change.
    #[cfg(windows)]
    #[test]
    fn a_file_noura_wrote_that_goes_online_only_shows_as_not_downloaded() {
        use std::os::windows::ffi::OsStrExt;

        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_OFFLINE, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
        };

        let mut live = Live::open();
        // The test delivers the watcher's events itself.
        live.app
            .workspace()
            .unwrap()
            .force_watch_fault(crate::WatchFault::Silent);
        live.create_note("Mine.md");
        live.app
            .workspace()
            .unwrap()
            .inject_watch_event(&["Mine.md"]);
        live.wait_until("the note shows", |app| rows(app) == ["Mine.md"]);
        let not_downloaded = |app: &App| {
            app.tree().and_then(|tree| {
                tree.find_row("Mine.md")
                    .map(|row| tree.rows()[row].not_downloaded)
            })
        };
        assert_eq!(not_downloaded(&live.app), Some(false));

        let path = live.engine().root().join("Mine.md");
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: `wide` is a NUL-terminated path that outlives both calls.
        unsafe {
            let attributes = GetFileAttributesW(wide.as_ptr());
            assert_ne!(attributes, INVALID_FILE_ATTRIBUTES);
            assert_ne!(
                SetFileAttributesW(wide.as_ptr(), attributes | FILE_ATTRIBUTE_OFFLINE),
                0
            );
        }
        live.app
            .workspace()
            .unwrap()
            .inject_watch_event(&["Mine.md"]);

        live.wait_until("the note shows as not downloaded", |app| {
            not_downloaded(app) == Some(true)
        });
    }
}
