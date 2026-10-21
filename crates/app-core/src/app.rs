//! The app's view state and its change stream.
//!
//! Opening a workspace walks its folder, which can take a while, so it runs
//! on a background thread. A client calls [`App::poll`] on its own thread to
//! take the result, and [`App::subscribe`] tells it when to.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        mpsc::{Receiver, TryRecvError, channel},
    },
    time::Instant,
};

use crate::{
    AppError, FileTree, Layout, LoginItem, NoLoginItem, NoPendingSaves, NoSystem, OpenedFile,
    PendingSaves, PendingWrites, Preferences, RecentWorkspace, Settings, SettingsSection, System,
    Tab, UpdateSource, Updates, Workspace, WorkspaceService, commands::Registry,
    layout::LayoutStore, preferences::PreferencesStore, rail::Rail, search::Searcher,
    settings::kernel_sections, tabs::Tabs, view_state::ViewStore,
};

mod commands;
mod drops;
mod file_actions;
mod finder;
mod ignored_files;
mod palette;
mod preferences;
mod settings;
mod view;

pub use drops::DragOut;
pub use file_actions::{Question, Rename};
pub use finder::{QuickOpen, Search};
pub use ignored_files::IgnoredFiles;
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
    /// The update status changed.
    Update,
    /// Settings or the Ignored files page changed.
    Settings,
    /// The appearance, text size or zoom changed.
    Preferences,
    /// A command registered.
    Commands,
    /// The palette opened, closed, or changed what it shows.
    Palette,
    /// Close Window or Quit ran; see [`App::take_close_window`] and
    /// [`App::take_quit`].
    Window,
    /// Background work finished; call [`App::poll`] to apply it.
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

/// The job running in the background and where its result arrives.
struct Pending {
    job: Job,
    result: Receiver<Result<Opened, AppError>>,
}

/// The state every client draws: which screen shows, the recent list, the
/// open workspace with its file tree, and its tabs.
pub struct App {
    service: Arc<WorkspaceService>,
    system: Arc<dyn System>,
    screen: Screen,
    recents: Vec<RecentWorkspace>,
    /// The open workspace, which background searches share.
    workspace: Option<Arc<Workspace>>,
    tree: Option<FileTree>,
    tabs: Tabs,
    rail: Rail,
    /// Each workspace's tabs, kept in this device's app data.
    views: ViewStore,
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
    /// Commands, quick open or search, while the palette is open.
    palette: Option<Palette>,
    searcher: Searcher,
    /// Close Window ran and the window hasn't closed yet.
    close_window: bool,
    /// Quit ran and the app hasn't closed yet.
    quit: bool,
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
        let service = Arc::new(WorkspaceService::with_system(
            app_data.as_ref(),
            system.clone(),
        ));
        let recents = service.recents();
        let listeners = Listeners::default();
        let writes = PendingWrites::default();
        let answered = listeners.clone();
        let mut app = Self {
            service,
            system,
            screen: Screen::Chooser { problem: None },
            recents,
            workspace: None,
            tree: None,
            tabs: Tabs::default(),
            rail: Rail::default(),
            views: ViewStore::new(app_data.as_ref()),
            layout: Layout::default(),
            layouts: LayoutStore::new(app_data.as_ref()),
            preferences: Preferences::default(),
            preferences_store: PreferencesStore::new(app_data.as_ref()),
            notice: None,
            renaming: None,
            ignored_files: None,
            settings: None,
            settings_sections: kernel_sections(),
            login_item: Arc::new(NoLoginItem),
            log_folder: crate::log_folder(app_data.as_ref()),
            third_party_licenses: None,
            pending_trash: None,
            failed_restore: None,
            pending: None,
            updates: Updates::new(writes.clone(), listeners.clone()),
            writes,
            listeners,
            pending_saves: Arc::new(NoPendingSaves),
            drag_out: None,
            dragged_out: None,
            registry: Registry::default(),
            palette: None,
            searcher: Searcher::new(move || answered.emit(Change::WorkFinished)),
            close_window: false,
            quit: false,
        };
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

    /// Open the workspace in `folder`, or make one of a plain folder.
    pub fn open_folder(&mut self, folder: PathBuf) {
        self.start(Job::OpenFolder(folder));
    }

    /// Create a workspace named `name` in `folder` and open it.
    pub fn create_workspace(&mut self, name: &str, folder: PathBuf) {
        self.start(Job::Create {
            name: name.to_owned(),
            folder,
        });
    }

    /// Open a workspace from the recent list.
    pub fn open_recent(&mut self, workspace_id: &str) {
        let name = self
            .recents
            .iter()
            .find(|recent| recent.workspace_id == workspace_id)
            .map_or_else(String::new, |recent| recent.name.clone());
        self.start(Job::OpenRecent {
            workspace_id: workspace_id.to_owned(),
            name,
        });
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
    /// write files on another thread register each write here.
    pub fn pending_writes(&self) -> &PendingWrites {
        &self.writes
    }

    /// Where updating stands.
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

    /// Save every change still waiting, wait for pending writes, then
    /// install the downloaded update and restart. If a save, a write or
    /// the install fails, this version keeps running and the update status
    /// says why.
    pub fn restart_to_update(&mut self) {
        self.updates.restart(self.pending_saves.clone());
    }

    /// Stop offering Restart outside Settings until the next update.
    pub fn hide_update_offer(&mut self) {
        self.updates.hide_offer();
    }

    /// Apply finished background work and start an update check that is
    /// due. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        self.poll_at(Instant::now())
    }

    /// [`App::poll`] with the time given, so tests can step through the
    /// update schedule.
    pub fn poll_at(&mut self, now: Instant) -> bool {
        let updated = self.updates.poll(now);
        let searched = self.take_search_answer();
        let opened = self.take_opened();
        updated | searched | opened
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
        match (result, job) {
            (Ok(opened), _) => self.adopt(opened),
            (Err(error), Job::Restore { workspace_id }) => {
                self.workspace = None;
                self.tree = None;
                self.close_palette();
                self.restore_view();
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

    fn adopt(&mut self, opened: Opened) {
        self.failed_restore = None;
        self.close_palette();
        self.workspace = Some(Arc::new(opened.workspace));
        self.tree = Some(opened.tree);
        self.restore_view();
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
        let spawned = std::thread::Builder::new()
            .name("workspace-open".into())
            .spawn(move || {
                let result = run(&service, &work);
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

fn run(service: &WorkspaceService, job: &Job) -> Result<Opened, AppError> {
    let workspace = match job {
        Job::Restore { workspace_id } | Job::OpenRecent { workspace_id, .. } => {
            service.open_recent(workspace_id)?
        }
        Job::OpenFolder(folder) => service.open_folder(folder)?,
        Job::Create { name, folder } => service.create(name, folder)?,
    };
    let tree = workspace.file_tree()?;
    Ok(Opened { workspace, tree })
}
