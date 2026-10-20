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
};

use crate::{
    AppError, FileTree, OpenedFile, RecentWorkspace, Workspace, WorkspaceService, files::shown_name,
};

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

/// A message about an action that failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    pub title: String,
    pub message: String,
    /// The recent workspace that failed to open, which the notice offers to
    /// remove from the recent list.
    pub remove_recent: Option<String>,
}

/// One part of the view state changed. Listeners hear about changes made
/// on any thread, including background work that [`App::poll`] can now
/// collect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    Screen,
    Recents,
    Tree,
    Document,
    Notice,
    /// Background work finished; call [`App::poll`] to apply it.
    WorkFinished,
}

type Listener = Box<dyn Fn(Change) + Send + Sync>;

#[derive(Clone, Default)]
struct Listeners(Arc<Mutex<Vec<Listener>>>);

impl Listeners {
    fn emit(&self, change: Change) {
        let listeners = self.0.lock().unwrap_or_else(|error| error.into_inner());
        for listener in listeners.iter() {
            listener(change);
        }
    }
}

enum Job {
    Restore,
    OpenFolder(PathBuf),
    Create { name: String, folder: PathBuf },
    OpenRecent { workspace_id: String, name: String },
}

struct Opened {
    workspace: Workspace,
    tree: FileTree,
}

struct Outcome {
    job: Job,
    result: Result<Option<Opened>, AppError>,
}

/// The state every client draws: which screen shows, the recent list, the
/// open workspace with its file tree, and the open file.
pub struct App {
    service: Arc<WorkspaceService>,
    screen: Screen,
    recents: Vec<RecentWorkspace>,
    workspace: Option<Workspace>,
    tree: Option<FileTree>,
    document: Option<OpenedFile>,
    notice: Option<Notice>,
    pending: Option<Receiver<Outcome>>,
    listeners: Listeners,
}

impl App {
    /// Start the app with per-device state in `app_data`. When a workspace
    /// was open last time, it reopens in the background while the screen
    /// shows [`Screen::Loading`].
    pub fn launch(app_data: impl AsRef<Path>) -> Self {
        let service = Arc::new(WorkspaceService::new(app_data.as_ref()));
        let recents = service.recents();
        let mut app = Self {
            service,
            screen: Screen::Chooser { problem: None },
            recents,
            workspace: None,
            tree: None,
            document: None,
            notice: None,
            pending: None,
            listeners: Listeners::default(),
        };
        if !app.recents.is_empty() {
            app.screen = Screen::Loading;
            app.start(Job::Restore);
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
        self.workspace.as_ref()
    }

    pub fn tree(&self) -> Option<&FileTree> {
        self.tree.as_ref()
    }

    pub fn tree_mut(&mut self) -> Option<&mut FileTree> {
        self.tree.as_mut()
    }

    /// The file shown in the document area.
    pub fn document(&self) -> Option<&OpenedFile> {
        self.document.as_ref()
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

    /// Reopen the last workspace after it failed to open.
    pub fn try_again(&mut self) {
        self.start(Job::Restore);
    }

    /// Drop a workspace from the recent list. Its folder stays as it is.
    pub fn remove_recent(&mut self, workspace_id: &str) {
        match self.service.remove_recent(workspace_id) {
            Ok(recents) => {
                self.recents = recents;
                self.listeners.emit(Change::Recents);
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

    /// Show a file from the tree in the document area.
    pub fn open_file(&mut self, relative_path: &str) {
        let Some(workspace) = &self.workspace else {
            return;
        };
        match workspace.open_file(relative_path) {
            Ok(file) => {
                self.document = Some(file);
                self.listeners.emit(Change::Document);
            }
            Err(error) => {
                let title = format!(
                    "Couldn't open \u{201c}{}\u{201d}",
                    shown_name(
                        relative_path.rsplit('/').next().unwrap_or(relative_path),
                        crate::EntryKind::File
                    )
                );
                self.show_notice(&title, error, None);
            }
        }
    }

    /// Apply finished background work. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let Some(pending) = &self.pending else {
            return false;
        };
        let outcome = match pending.try_recv() {
            Ok(outcome) => outcome,
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => Outcome {
                job: Job::Restore,
                result: Err(AppError::new(
                    "worker_failed",
                    "Noura couldn't open the workspace.",
                )),
            },
        };
        self.pending = None;
        self.recents = self.service.recents();
        self.listeners.emit(Change::Recents);
        match (outcome.result, outcome.job) {
            (Ok(Some(opened)), _) => self.adopt(opened),
            (Ok(None), _) => self.set_screen(Screen::Chooser { problem: None }),
            (Err(error), Job::Restore) => {
                self.workspace = None;
                self.tree = None;
                self.document = None;
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
        self.workspace = Some(opened.workspace);
        self.tree = Some(opened.tree);
        self.document = None;
        self.notice = None;
        self.listeners.emit(Change::Tree);
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
        let spawned = std::thread::Builder::new()
            .name("workspace-open".into())
            .spawn(move || {
                let result = run(&service, &job);
                // The app may have closed meanwhile; nothing is waiting then.
                let _ = sender.send(Outcome { job, result });
                listeners.emit(Change::WorkFinished);
            });
        match spawned {
            Ok(_) => self.pending = Some(receiver),
            Err(error) => {
                tracing::error!(%error, "could not start the workspace thread");
                let error = AppError::new("worker_failed", "Noura couldn't open the workspace.");
                self.show_notice("Couldn't open the workspace", error, None);
            }
        }
    }
}

fn run(service: &WorkspaceService, job: &Job) -> Result<Option<Opened>, AppError> {
    let workspace = match job {
        Job::Restore => match service.restore_last()? {
            Some(workspace) => workspace,
            None => return Ok(None),
        },
        Job::OpenFolder(folder) => service.open_folder(folder)?,
        Job::Create { name, folder } => service.create(name, folder)?,
        Job::OpenRecent { workspace_id, .. } => service.open_recent(workspace_id)?,
    };
    let tree = workspace.file_tree()?;
    Ok(Some(Opened { workspace, tree }))
}
