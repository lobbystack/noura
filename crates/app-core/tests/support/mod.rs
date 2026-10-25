//! Shared helpers for the seam 1 tests of file operations.

#![allow(dead_code, reason = "each test file uses a different subset")]

pub mod ai;
pub mod calendar;
pub mod plugins;
pub mod projects;
pub mod recipes;
pub mod tasks;

use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, Instant},
};

use app_core::{App, Platform, System, WorkspaceService};

/// What the fake system was asked to do, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Call {
    Trash(PathBuf),
    Reveal(PathBuf),
    OpenFolder(PathBuf),
    OpenTerminal(PathBuf),
    Copy(String),
    OpenUrl(String),
    OpenFile(PathBuf),
}

/// A system that records every request. Its trash moves items to a folder
/// of its own, or refuses, and it can claim to be any platform.
pub struct FakeSystem {
    pub platform: Platform,
    /// `None`: the system has no trash. `Some(false)`: it has one but
    /// refuses every move.
    trash: Option<bool>,
    bin: tempfile::TempDir,
    calls: Mutex<Vec<Call>>,
    /// The hour of the day on the system's clock, 9 am until a test sets
    /// another.
    hour: AtomicU8,
    /// The instant the system's clock reads, this computer's until a test
    /// sets one.
    now: Mutex<Option<jiff::Timestamp>>,
    /// The system's time zone, this computer's until a test sets one.
    time_zone: Mutex<Option<jiff::tz::TimeZone>>,
    /// The system's credential store, in memory, so no test reaches this
    /// computer's own.
    pub credentials: Arc<local_core::MemoryCredentials>,
}

impl FakeSystem {
    /// A system with a working trash, on this computer's platform.
    pub fn new() -> Arc<Self> {
        Self::build(Platform::current(), Some(true))
    }

    pub fn on(platform: Platform) -> Arc<Self> {
        Self::build(platform, Some(true))
    }

    /// A system whose trash refuses every move.
    pub fn refusing_trash() -> Arc<Self> {
        Self::build(Platform::current(), Some(false))
    }

    /// A system with no trash at all.
    pub fn without_trash() -> Arc<Self> {
        Self::build(Platform::current(), None)
    }

    fn build(platform: Platform, trash: Option<bool>) -> Arc<Self> {
        Arc::new(Self {
            platform,
            trash,
            bin: tempfile::tempdir().unwrap(),
            calls: Mutex::new(Vec::new()),
            hour: AtomicU8::new(9),
            now: Mutex::new(None),
            time_zone: Mutex::new(None),
            credentials: Arc::new(local_core::MemoryCredentials::new()),
        })
    }

    /// Stop the system's clock at `instant`, an RFC 3339 instant, in the
    /// IANA time zone `zone`.
    pub fn set_clock(&self, instant: &str, zone: &str) {
        *self.now.lock().unwrap() = Some(instant.parse().unwrap());
        *self.time_zone.lock().unwrap() = Some(jiff::tz::TimeZone::get(zone).unwrap());
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// Where the system trash put the item it was given as `name`.
    pub fn in_bin(&self, name: &str) -> PathBuf {
        self.bin.path().join(name)
    }

    /// Set the system's clock to `hour` o'clock.
    pub fn set_hour(&self, hour: u8) {
        self.hour.store(hour, Ordering::SeqCst);
    }

    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }
}

impl System for FakeSystem {
    fn platform(&self) -> Platform {
        self.platform
    }

    fn has_trash(&self) -> bool {
        self.trash.is_some()
    }

    fn trash(&self, path: &Path) -> io::Result<()> {
        self.record(Call::Trash(path.to_owned()));
        match self.trash {
            Some(true) => std::fs::rename(path, self.bin.path().join(path.file_name().unwrap())),
            Some(false) => Err(io::Error::other("the trash refused")),
            None => Err(io::Error::new(io::ErrorKind::Unsupported, "no trash")),
        }
    }

    fn reveal(&self, path: &Path) -> io::Result<()> {
        self.record(Call::Reveal(path.to_owned()));
        Ok(())
    }

    fn open_folder(&self, folder: &Path) -> io::Result<()> {
        self.record(Call::OpenFolder(folder.to_owned()));
        Ok(())
    }

    fn open_terminal(&self, folder: &Path) -> io::Result<()> {
        self.record(Call::OpenTerminal(folder.to_owned()));
        Ok(())
    }

    fn copy_text(&self, text: &str) {
        self.record(Call::Copy(text.to_owned()));
    }

    fn open_url(&self, url: &str) -> io::Result<()> {
        self.record(Call::OpenUrl(url.to_owned()));
        Ok(())
    }

    /// Opens files that exist, as the system's default app would.
    fn open_file(&self, path: &Path) -> io::Result<()> {
        self.record(Call::OpenFile(path.to_owned()));
        if path.is_file() {
            Ok(())
        } else {
            Err(io::Error::new(io::ErrorKind::NotFound, "no such file"))
        }
    }

    fn local_hour(&self) -> u8 {
        self.hour.load(Ordering::SeqCst)
    }

    fn now(&self) -> jiff::Timestamp {
        self.now
            .lock()
            .unwrap()
            .unwrap_or_else(jiff::Timestamp::now)
    }

    fn time_zone(&self) -> jiff::tz::TimeZone {
        self.time_zone
            .lock()
            .unwrap()
            .clone()
            .unwrap_or_else(jiff::tz::TimeZone::system)
    }

    fn credentials(&self) -> Arc<dyn local_core::CredentialStore> {
        self.credentials.clone()
    }
}

/// A workspace folder with files, and the app data beside it.
pub struct Fixture {
    pub app_data: tempfile::TempDir,
    parent: tempfile::TempDir,
    pub root: PathBuf,
}

impl Fixture {
    /// A workspace named Garden holding `files`, as (path, bytes).
    pub fn new(files: &[(&str, &[u8])]) -> Self {
        let app_data = tempfile::tempdir().unwrap();
        let parent = tempfile::tempdir().unwrap();
        let folder = parent.path().join("Garden");
        std::fs::create_dir_all(&folder).unwrap();
        let workspace = WorkspaceService::new(app_data.path())
            .create("Garden", &folder)
            .unwrap();
        let root = PathBuf::from(workspace.root());
        drop(workspace);
        for (path, bytes) in files {
            write(&root, path, bytes);
        }
        Self {
            app_data,
            parent,
            root,
        }
    }

    /// The app with this workspace open, using `system`.
    pub fn app_with(&self, system: Arc<FakeSystem>) -> App {
        let mut app = App::launch_with(self.app_data.path(), system);
        settle(&mut app);
        assert!(app.workspace().is_some(), "the workspace didn't open");
        app
    }

    pub fn app(&self) -> App {
        self.app_with(FakeSystem::new())
    }

    /// A folder next to the workspace, outside it.
    pub fn outside(&self) -> PathBuf {
        let outside = self.parent.path().join("Outside");
        std::fs::create_dir_all(&outside).unwrap();
        outside
    }

    pub fn read(&self, relative: &str) -> Vec<u8> {
        std::fs::read(self.root.join(relative))
            .unwrap_or_else(|error| panic!("{relative} can't be read: {error}"))
    }

    pub fn exists(&self, relative: &str) -> bool {
        std::fs::symlink_metadata(self.root.join(relative)).is_ok()
    }

    /// Every file and folder under the workspace, outside `.noura`, with
    /// each file's bytes.
    pub fn snapshot(&self) -> Vec<(String, Option<Vec<u8>>)> {
        let mut found = Vec::new();
        walk(&self.root, &self.root, &mut found);
        found.sort();
        found
    }
}

fn walk(root: &Path, folder: &Path, found: &mut Vec<(String, Option<Vec<u8>>)>) {
    for entry in std::fs::read_dir(folder).unwrap() {
        let path = entry.unwrap().path();
        let relative = path
            .strip_prefix(root)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if relative == ".noura" {
            continue;
        }
        let metadata = std::fs::symlink_metadata(&path).unwrap();
        if metadata.is_dir() {
            found.push((relative, None));
            walk(root, &path, found);
        } else {
            found.push((relative, Some(std::fs::read(&path).unwrap_or_default())));
        }
    }
}

pub fn write(root: &Path, relative: &str, bytes: &[u8]) {
    let path = root.join(relative);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

/// Run `action` on another thread while this test holds the workspace's
/// write lock, call `meanwhile` once the action is waiting for the lock,
/// then let it go on. This stands in for another app changing the files
/// after Noura checked them and before it changes them. Under heavy load
/// `meanwhile` may run before Noura's own checks, which then catch it.
pub fn race<T: Send>(
    fixture: &Fixture,
    app: &mut App,
    action: impl FnOnce(&mut App) -> T + Send,
    meanwhile: impl FnOnce(),
) -> T {
    let lock_path = fixture
        .app_data
        .path()
        .join("workspaces")
        .join(app.workspace().unwrap().id())
        .join("workspace.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .unwrap();
    lock.lock().unwrap();
    std::thread::scope(|scope| {
        let running = scope.spawn(|| action(app));
        std::thread::sleep(Duration::from_millis(300));
        meanwhile();
        lock.unlock().unwrap();
        running.join().unwrap()
    })
}

/// The workspace's index files in the app data folder: the database
/// first, then its write-ahead log and shared memory.
pub fn index_files(fixture: &Fixture, app: &App) -> Vec<PathBuf> {
    let folder = fixture
        .app_data
        .path()
        .join("workspaces")
        .join(app.workspace().unwrap().id());
    ["index.sqlite", "index.sqlite-wal", "index.sqlite-shm"]
        .into_iter()
        .map(|name| folder.join(name))
        .collect()
}

/// Poll until every note save finishes.
pub fn settle_saves(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        if !app.is_saving_notes() {
            return;
        }
        assert!(Instant::now() < deadline, "a note save never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Hold the workspace's write lock, as another Noura process writing would,
/// until the returned file is unlocked or dropped. Writes wait meanwhile.
pub fn hold_writes(fixture: &Fixture, app: &App) -> std::fs::File {
    let lock_path = fixture
        .app_data
        .path()
        .join("workspaces")
        .join(app.workspace().unwrap().id())
        .join("workspace.lock");
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(lock_path)
        .unwrap();
    lock.lock().unwrap();
    lock
}

/// Open the file at `relative_path` from the file tree, as a click on its
/// row does, and return its tab.
pub fn open_from_tree(app: &mut App, relative_path: &str) -> app_core::TabId {
    select(app, relative_path);
    let row = app.tree().unwrap().focused().unwrap();
    app.click_tree_row(row, app_core::OpenAs::Preview);
    let tab = app.active_tab().unwrap();
    assert_eq!(
        tab.file().map(|file| file.relative_path.as_str()),
        Some(relative_path)
    );
    tab.id()
}

/// Poll until background work finishes.
pub fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        if !app.is_busy() && !app.is_plugin_work_pending() {
            return;
        }
        assert!(Instant::now() < deadline, "background work never finished");
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// The names of the visible tree rows, top to bottom.
pub fn rows(app: &App) -> Vec<String> {
    app.tree()
        .unwrap()
        .rows()
        .iter()
        .map(|row| row.name.clone())
        .collect()
}

/// The workspace path of the tree's current row.
pub fn current(app: &App) -> Option<String> {
    let tree = app.tree().unwrap();
    tree.focused()
        .map(|row| tree.rows()[row].relative_path.clone())
}

/// The workspace path of the open file.
pub fn document(app: &App) -> Option<String> {
    app.document().map(|file| file.relative_path.clone())
}

/// The stable ID in a managed file's frontmatter.
pub fn object_id(bytes: &[u8]) -> String {
    match local_core::parse_markdown("x.md", bytes) {
        local_core::ParsedMarkdown::Managed(object) => object.id,
        other => panic!("not a managed file: {other:?}"),
    }
}

/// A managed note's bytes with `id`, as Noura writes them today.
pub fn note(id_suffix: &str, body: &str) -> Vec<u8> {
    format!(
        "---\nid: note_{id_suffix}\ntype: note\ncolor: green\ncreated: 2026-10-20T10:00:00Z\n\
         updated: 2026-10-20T10:00:00Z\n---\n\n{body}"
    )
    .into_bytes()
}

/// Open the folders above `relative_path` and make its row current.
pub fn select(app: &mut App, relative_path: &str) {
    app.tree_mut().unwrap().select(relative_path);
}

/// The notice's title and message.
pub fn notice(app: &App) -> Option<(String, String)> {
    app.notice()
        .map(|notice| (notice.title.clone(), notice.message.clone()))
}
