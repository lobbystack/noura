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
    AppError, FileTree, OpenedFile, RecentWorkspace, Workspace, WorkspaceService,
    files::shown_name,
    watch::{Rescans, Snapshot, Watch},
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
    Document,
    Notice,
    /// Background work finished; call [`App::poll`] to apply it.
    WorkFinished,
    /// Files in the open workspace changed on disk; call [`App::poll`] to
    /// apply the change to the views.
    Files(Origin),
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
    tree: FileTree,
}

/// The job running in the background and where its result arrives.
struct Pending {
    job: Job,
    result: Receiver<Result<Opened, AppError>>,
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
    /// The workspace the launch couldn't reopen, which Try again retries.
    failed_restore: Option<String>,
    pending: Option<Pending>,
    listeners: Listeners,
    rescans: Rescans,
    /// Keeps the open workspace in step with its folder.
    watch: Option<Watch>,
    /// The listing the views show, which tells which object each file is.
    snapshot: Option<Snapshot>,
}

impl App {
    /// Start the app with per-device state in `app_data`. When a workspace
    /// was open last time, it reopens in the background while the screen
    /// shows [`Screen::Loading`].
    pub fn launch(app_data: impl AsRef<Path>) -> Self {
        Self::launch_with(app_data, Rescans::default())
    }

    /// [`App::launch`] with other intervals for the full rescans that back
    /// up the native watcher.
    pub fn launch_with(app_data: impl AsRef<Path>, rescans: Rescans) -> Self {
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
            failed_restore: None,
            pending: None,
            listeners: Listeners::default(),
            rescans,
            watch: None,
            snapshot: None,
        };
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

    /// Apply finished background work and changes to the open workspace's
    /// files. Returns whether anything changed.
    pub fn poll(&mut self) -> bool {
        let opened = self.poll_job();
        let refreshed = self.poll_watch();
        opened || refreshed
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
                self.watch = None;
                self.snapshot = None;
                self.workspace = None;
                self.tree = None;
                self.document = None;
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

    /// Apply the newest view of the open workspace's files.
    fn poll_watch(&mut self) -> bool {
        let Some(update) = self.watch.as_ref().and_then(Watch::take) else {
            return false;
        };
        let mut tree = update.tree;
        if let Some(old) = &self.tree {
            tree.keep_state_from(old);
        }
        let rows_changed = self
            .tree
            .as_ref()
            .is_none_or(|old| old.rows() != tree.rows());
        self.tree = Some(tree);
        if rows_changed {
            self.listeners.emit(Change::Tree);
        }
        let previous = self.snapshot.replace(update.snapshot);
        self.follow_document(previous.as_ref());
        true
    }

    /// Keep the document area on the file it shows. A note that moved keeps
    /// its ID, so the document follows it to the new path. A file that is
    /// gone, or moved without an ID to follow, closes.
    fn follow_document(&mut self, previous: Option<&Snapshot>) {
        let (Some(document), Some(current)) = (&self.document, &self.snapshot) else {
            return;
        };
        let path = document.relative_path.as_str();
        if current.file(path).is_some() {
            return;
        }
        let moved_to = previous
            .and_then(|previous| previous.file(path))
            .and_then(|entry| entry.object_id.as_deref())
            .and_then(|id| current.only_file_with_id(id));
        self.document = moved_to.map(|entry| OpenedFile {
            name: shown_name(&entry.name, crate::EntryKind::File),
            relative_path: entry.relative_path.clone(),
        });
        self.listeners.emit(Change::Document);
    }

    fn adopt(&mut self, opened: Opened) {
        self.failed_restore = None;
        // Stop watching the previous workspace before watching this one.
        self.watch = None;
        self.snapshot = Some(opened.snapshot.clone());
        self.watch = Watch::start(
            opened.workspace.shared_engine(),
            opened.workspace.root().to_owned(),
            opened.snapshot,
            self.rescans,
            self.listeners.clone(),
        );
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
    let snapshot = Snapshot::of(workspace.engine(), workspace.root())?;
    let tree = FileTree::from_entries(&snapshot.entries);
    Ok(Opened {
        workspace,
        snapshot,
        tree,
    })
}

impl Drop for App {
    fn drop(&mut self) {
        // Release the workspace's files and index before the app goes.
        if let Some(watch) = self.watch.take() {
            watch.finish();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::mpsc::channel,
        time::{Duration, Instant},
    };

    use local_core::CreateObjectInput;

    use super::*;

    /// App-core has no file operations of its own yet (ticket 08 adds
    /// them), so this writes through the engine the way they will.
    #[test]
    fn a_file_noura_writes_reaches_the_tree_marked_as_noura_s_own() {
        let app_data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let mut app = App::launch(app_data.path());
        let (sender, changes) = channel();
        app.subscribe(move |change| {
            let _ = sender.send(change);
        });
        app.open_folder(folder.path().to_owned());
        let deadline = Instant::now() + Duration::from_secs(15);
        while app.is_busy() {
            assert!(Instant::now() < deadline, "the workspace never opened");
            let _ = changes.recv_timeout(Duration::from_millis(50));
            app.poll();
        }
        let _ = changes.try_iter().count();

        app.workspace()
            .unwrap()
            .engine()
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Mine".into(),
                body: String::new(),
                relative_path: Some("Mine.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();

        let mut seen = Vec::new();
        loop {
            app.poll();
            seen.extend(changes.try_iter());
            let rows: Vec<&str> = app
                .tree()
                .unwrap()
                .rows()
                .iter()
                .map(|row| row.relative_path.as_str())
                .collect();
            if rows == ["Mine.md"] {
                break;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            assert!(!left.is_zero(), "the note never showed in the tree");
            if let Ok(change) = changes.recv_timeout(left) {
                seen.push(change);
            }
        }
        assert!(seen.contains(&Change::Files(Origin::Noura)));
        assert!(!seen.contains(&Change::Files(Origin::Outside)));
    }
}
