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

use local_core::ChangeCounts;

use crate::{
    AppError, FileTree, Layout, NoSystem, OpenedFile, RecentWorkspace, System, Tab, Workspace,
    WorkspaceService,
    layout::LayoutStore,
    rail::Rail,
    tabs::Tabs,
    view_state::ViewStore,
    watch::{self, Rescans, Snapshot, Watch},
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
    /// The Ignored files page changed.
    Settings,
    /// Background work finished; call [`App::poll`] to apply it.
    WorkFinished,
    /// Files in the open workspace changed on disk; call [`App::poll`] to
    /// apply the change to the views.
    Files(Origin),
    /// The open workspace's problems changed.
    Problems,
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
/// open workspace with its file tree, and its tabs.
pub struct App {
    service: Arc<WorkspaceService>,
    system: Arc<dyn System>,
    screen: Screen,
    recents: Vec<RecentWorkspace>,
    workspace: Option<Workspace>,
    tree: Option<FileTree>,
    tabs: Tabs,
    rail: Rail,
    /// Each workspace's tabs, kept in this device's app data.
    views: ViewStore,
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
    rescans: Rescans,
    /// Keeps the open workspace in step with its folder.
    watch: Option<Watch>,
    /// The listing the views show, which tells which object each file is.
    snapshot: Option<Snapshot>,
    /// The engine's change counts just before the app last listed the files
    /// itself, after one of its tree operations. A watch update listed
    /// before then would undo that operation in the views.
    listed: ChangeCounts,
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
        Self::launch_with_rescans(app_data, system, Rescans::default())
    }

    /// [`App::launch_with`] with other intervals for the full rescans that
    /// back up the native watcher.
    pub fn launch_with_rescans(
        app_data: impl AsRef<Path>,
        system: Arc<dyn System>,
        rescans: Rescans,
    ) -> Self {
        let service = Arc::new(WorkspaceService::with_system(
            app_data.as_ref(),
            system.clone(),
        ));
        let recents = service.recents();
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
            notice: None,
            renaming: None,
            ignored_files: None,
            pending_trash: None,
            failed_restore: None,
            pending: None,
            listeners: Listeners::default(),
            rescans,
            watch: None,
            snapshot: None,
            listed: ChangeCounts::default(),
        };
        app.layout = app.layouts.load();
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

    /// The open workspace's problems, such as files that share an ID.
    pub fn problems(&self) -> &[crate::Problem] {
        self.snapshot
            .as_ref()
            .map_or(&[], |snapshot| snapshot.problems.as_slice())
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
                self.restore_view();
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

    /// Apply the newest view of the open workspace's files. The tree keeps
    /// its open folders, current row and a drag in progress, and the tabs
    /// follow their files into it.
    fn poll_watch(&mut self) -> bool {
        let Some(update) = self.watch.as_ref().and_then(Watch::take) else {
            return false;
        };
        // The views show a newer listing already. The watch thread lists
        // again after the change that made it newer.
        if watch::is_older(&update, self.listed) {
            return false;
        }
        let mut tree = update.tree;
        let mut folders_changed = false;
        if let Some(old) = &self.tree {
            tree.carry_over(old, None);
            folders_changed = old.expanded_folders() != tree.expanded_folders();
        }
        let rows_changed = self
            .tree
            .as_ref()
            .is_none_or(|old| old.rows() != tree.rows());
        self.tree = Some(tree);
        if rows_changed {
            self.listeners.emit(Change::Tree);
        }
        self.set_snapshot(update.snapshot);
        self.follow_files();
        if folders_changed {
            self.save_view();
        }
        true
    }

    /// Show the problems of a new listing of the files.
    fn set_snapshot(&mut self, snapshot: Snapshot) {
        let previous = self.snapshot.replace(snapshot);
        let before = previous
            .as_ref()
            .map(|snapshot| snapshot.problems.as_slice());
        if before != Some(self.problems()) {
            self.listeners.emit(Change::Problems);
        }
    }

    fn adopt(&mut self, opened: Opened) {
        self.failed_restore = None;
        // Stop watching the previous workspace before watching this one.
        self.watch = None;
        self.snapshot = Some(opened.snapshot.clone());
        self.listed = ChangeCounts::default();
        self.watch = Watch::start(
            opened.workspace.shared_engine(),
            opened.workspace.root().to_owned(),
            opened.snapshot,
            self.rescans,
            self.listeners.clone(),
        );
        self.workspace = Some(opened.workspace);
        self.tree = Some(opened.tree);
        self.restore_view();
        self.notice = None;
        self.renaming = None;
        self.ignored_files = None;
        self.pending_trash = None;
        self.listeners.emit(Change::Tree);
        self.listeners.emit(Change::Tabs);
        self.listeners.emit(Change::Document);
        self.listeners.emit(Change::Notice);
        self.listeners.emit(Change::Problems);
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
