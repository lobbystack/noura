//! Shared helpers for the seam 1 tests of file operations.

#![allow(dead_code, reason = "each test file uses a different subset")]

use std::{
    io,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
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
        })
    }

    pub fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// Where the system trash put the item it was given as `name`.
    pub fn in_bin(&self, name: &str) -> PathBuf {
        self.bin.path().join(name)
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

/// Poll until background work finishes.
pub fn settle(app: &mut App) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        if !app.is_busy() {
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
