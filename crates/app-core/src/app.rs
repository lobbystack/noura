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

use crate::{
    AppError, FileTree, Layout, NoSystem, OpenedFile, RecentWorkspace, System, Tab, Workspace,
    WorkspaceService,
    layout::LayoutStore,
    rail::Rail,
    session::{Session, WindowKey},
    tabs::Tabs,
    view_state::StoredWindow,
};

mod file_actions;
mod ignored_files;
mod view;

pub use file_actions::{Question, Rename};
pub use ignored_files::IgnoredFiles;

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
    /// The Ignored files page changed.
    Settings,
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

/// The job running in the background and where its result arrives.
struct Pending {
    job: Job,
    result: Receiver<Result<Opened, AppError>>,
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
    /// The versions of the shared listing, rail order and recent list this
    /// window shows.
    listing_seen: u64,
    rail_seen: u64,
    recents_seen: u64,
    closed: bool,
    /// Windows this one opened for its workspace's other saved windows,
    /// until the client takes them.
    opened_windows: Vec<App>,
    service: Arc<WorkspaceService>,
    system: Arc<dyn System>,
    screen: Screen,
    recents: Vec<RecentWorkspace>,
    workspace: Option<Arc<Workspace>>,
    tree: Option<FileTree>,
    tabs: Tabs,
    rail: Rail,
    layout: Layout,
    layouts: LayoutStore,
    notice: Option<Notice>,
    /// The tree row being renamed in place.
    renaming: Option<Rename>,
    /// The Ignored files page, while it is open.
    ignored_files: Option<IgnoredFiles>,
    /// A folder waiting to go to the trash until you confirm.
    pending_trash: Option<(String, Question)>,
    /// The workspace the launch couldn't reopen, which Try again retries.
    failed_restore: Option<String>,
    pending: Option<Pending>,
    listeners: Listeners,
}

impl App {
    /// Start the app with per-device state in `app_data` and no operating
    /// system services: deleted items go to the workspace's trash folder.
    /// See [`App::launch_with`].
    pub fn launch(app_data: impl AsRef<Path>) -> Self {
        Self::launch_with(app_data, Arc::new(NoSystem))
    }

    /// Start the app with per-device state in `app_data`, using `system`
    /// for the trash, the file manager, terminals and the clipboard. When a
    /// workspace was open last time, it reopens in the background while
    /// the screen shows [`Screen::Loading`].
    pub fn launch_with(app_data: impl AsRef<Path>, system: Arc<dyn System>) -> Self {
        let session = Session::new(app_data.as_ref(), system);
        let mut app = Self::join(&session);
        app.recents = app.service.recents();
        app.layout = app.layouts.load();
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
        Self {
            session: session.clone(),
            key,
            opening: session.opening(),
            listing_seen: 0,
            rail_seen: 0,
            recents_seen: 0,
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
            notice: None,
            renaming: None,
            ignored_files: None,
            pending_trash: None,
            failed_restore: None,
            pending: None,
            listeners,
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
        window.restore_view(stored);
        window
    }

    /// Close this window. The other windows keep their tabs, and this
    /// one's tabs leave the windows that come back at the next launch,
    /// unless it was the last window open: then it comes back, as when
    /// Noura quits.
    pub fn close_window(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.session.close(self.key);
        self.release();
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

    /// Apply finished background work and what other windows changed,
    /// such as the files. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        if self.closed {
            return false;
        }
        if self.is_closed() {
            self.closed_by_another_window();
            return true;
        }
        let opened = self.poll_job();
        let listed = self.poll_listing();
        let rail = self.poll_rail();
        let recents = self.poll_recents();
        opened || listed || rail || recents
    }

    fn poll_job(&mut self) -> bool {
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
                self.rail.set_saved_order(Vec::new());
                self.restore_view(StoredWindow::default());
                self.renaming = None;
                self.ignored_files = None;
                self.pending_trash = None;
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

    /// Another window opened a different workspace, so this one closed.
    /// Its tabs stay saved with the workspace it showed.
    fn closed_by_another_window(&mut self) {
        self.closed = true;
        self.release();
    }

    fn adopt(&mut self, opened: Opened) {
        // The workspace was open already: every window keeps sharing the
        // one that is open, with its index.
        if self
            .workspace
            .as_ref()
            .is_some_and(|workspace| workspace.id() == opened.workspace.id())
        {
            drop(opened);
            self.refresh_tree(None);
            return;
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

impl Drop for App {
    fn drop(&mut self) {
        // Quitting drops every window without closing it, so each one's
        // tabs stay saved for the next launch.
        self.session.leave(self.key);
    }
}
