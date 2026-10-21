//! Each workspace's view state on this device: the windows open on it,
//! with each window's tabs, active tab and open folders, and the rail's
//! order. It lives in the app data folder, never in the workspace.

use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, Sender, channel},
    thread::JoinHandle,
};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::JsonStore;

/// The view state format this version writes.
const VERSION: u64 = 2;

/// A workspace's saved view state: its windows, in the order they opened,
/// and the rail order they share. Anything Noura can't read comes back
/// empty, and windows and tabs it can't read drop out one by one: view
/// state is a convenience, never workspace data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Saved", into = "Saved")]
pub(crate) struct StoredView {
    pub(crate) windows: Vec<StoredWindow>,
    /// The rail's page IDs in the order you gave them. Empty until you
    /// move a page.
    pub(crate) rail_order: Vec<String>,
}

/// One window's saved view state.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct StoredWindow {
    pub(crate) tabs: Vec<StoredTab>,
    /// The index of the active tab in `tabs`.
    pub(crate) active: Option<usize>,
    /// The file tree's open folders.
    pub(crate) expanded_folders: Vec<String>,
}

/// One saved tab: a file by workspace-relative path, or a rail page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StoredTab {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) file: Option<String>,
    /// The stable ID of the managed object the file held.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) object_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) page: Option<String>,
    #[serde(default)]
    pub(crate) preview: bool,
}

impl StoredTab {
    /// A saved tab for the rail page `page_id`.
    pub(crate) fn page(page_id: &str, preview: bool) -> Self {
        Self {
            file: None,
            object_id: None,
            page: Some(page_id.to_owned()),
            preview,
        }
    }
}

/// The file's shape, read field by field so one bad value drops only
/// itself.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Saved {
    version: Value,
    windows: Vec<Value>,
    rail_order: Vec<Value>,
}

/// One window's shape in the file.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct SavedWindow {
    tabs: Vec<Value>,
    active: Value,
    expanded_folders: Vec<Value>,
}

impl From<Saved> for StoredView {
    fn from(saved: Saved) -> Self {
        if saved.version.as_u64() != Some(VERSION) {
            tracing::warn!("a workspace's view state has an unknown version");
            return Self::default();
        }
        Self {
            // A window Noura can't read drops out as if closed.
            windows: saved
                .windows
                .into_iter()
                .filter_map(|window| serde_json::from_value::<SavedWindow>(window).ok())
                .map(StoredWindow::from)
                .collect(),
            rail_order: strings(saved.rail_order),
        }
    }
}

impl From<SavedWindow> for StoredWindow {
    fn from(saved: SavedWindow) -> Self {
        let tabs: Vec<Option<StoredTab>> = saved
            .tabs
            .into_iter()
            .map(|tab| serde_json::from_value(tab).ok())
            .collect();
        // Unreadable tabs drop out as if closed.
        let kept: Vec<bool> = tabs.iter().map(Option::is_some).collect();
        let active = saved
            .active
            .as_u64()
            .and_then(|active| usize::try_from(active).ok())
            .and_then(|active| crate::tabs::takes_place(&kept, active));
        Self {
            tabs: tabs.into_iter().flatten().collect(),
            active,
            expanded_folders: strings(saved.expanded_folders),
        }
    }
}

impl From<StoredView> for Saved {
    fn from(view: StoredView) -> Self {
        Self {
            version: VERSION.into(),
            windows: view
                .windows
                .into_iter()
                .map(SavedWindow::from)
                .filter_map(|window| serde_json::to_value(window).ok())
                .collect(),
            rail_order: values(view.rail_order),
        }
    }
}

impl From<StoredWindow> for SavedWindow {
    fn from(window: StoredWindow) -> Self {
        Self {
            tabs: window
                .tabs
                .into_iter()
                .filter_map(|tab| serde_json::to_value(tab).ok())
                .collect(),
            active: window.active.map_or(Value::Null, Into::into),
            expanded_folders: values(window.expanded_folders),
        }
    }
}

fn values(items: Vec<String>) -> Vec<Value> {
    items.into_iter().map(Value::String).collect()
}

/// The strings in `values`, leaving out anything else.
fn strings(values: Vec<Value>) -> Vec<String> {
    values
        .into_iter()
        .filter_map(|value| match value {
            Value::String(text) => Some(text),
            _ => None,
        })
        .collect()
}

/// Each workspace's view state, in `workspaces/<workspace ID>/view-state.json`
/// under the app data folder, next to `local-core`'s index for that
/// workspace.
///
/// Saves happen on a background thread, so a click never waits for the
/// disk. It writes only the newest state for each workspace, a load first
/// waits for the saves before it, and dropping the store waits for every
/// save.
pub(crate) struct ViewStore {
    app_data: PathBuf,
    writer: Option<Writer>,
}

/// The background thread that writes view state.
struct Writer {
    sender: Sender<Message>,
    thread: JoinHandle<()>,
}

enum Message {
    Save {
        workspace_id: String,
        view: StoredView,
    },
    /// Answer once every save sent before this is on disk.
    Flush(Sender<()>),
}

impl ViewStore {
    pub(crate) fn new(app_data: &Path) -> Self {
        Self {
            app_data: app_data.to_owned(),
            writer: None,
        }
    }

    /// The workspace's saved view state, or an empty one.
    pub(crate) fn load(&mut self, workspace_id: &str) -> StoredView {
        self.flush();
        store(&self.app_data, workspace_id)
            .map(|store| store.load())
            .unwrap_or_default()
    }

    /// Save the workspace's view state in the background. A failure goes
    /// to the log: the view still works for this session.
    pub(crate) fn save(&mut self, workspace_id: &str, view: StoredView) {
        let message = Message::Save {
            workspace_id: workspace_id.to_owned(),
            view,
        };
        let message = match self.writer() {
            Some(writer) => match writer.sender.send(message) {
                Ok(()) => return,
                Err(unsent) => unsent.0,
            },
            None => message,
        };
        // Without the thread, save here.
        self.writer = None;
        if let Message::Save { workspace_id, view } = message {
            write(&self.app_data, &workspace_id, &view);
        }
    }

    /// Wait until every save sent so far is on disk.
    fn flush(&mut self) {
        let Some(writer) = &self.writer else {
            return;
        };
        let (done, wait) = channel();
        if writer.sender.send(Message::Flush(done)).is_err() || wait.recv().is_err() {
            self.writer = None;
        }
    }

    /// The writer thread, started on the first save.
    fn writer(&mut self) -> Option<&Writer> {
        if self.writer.is_none() {
            let (sender, receiver) = channel();
            let app_data = self.app_data.clone();
            let spawned = std::thread::Builder::new()
                .name("view-state".into())
                .spawn(move || run_writer(&app_data, &receiver));
            match spawned {
                Ok(thread) => self.writer = Some(Writer { sender, thread }),
                Err(error) => {
                    tracing::warn!(%error, "could not start the view state thread");
                }
            }
        }
        self.writer.as_ref()
    }
}

impl Drop for ViewStore {
    fn drop(&mut self) {
        if let Some(Writer { sender, thread }) = self.writer.take() {
            // The thread writes what it has, then stops.
            drop(sender);
            if thread.join().is_err() {
                tracing::warn!("the view state thread stopped early");
            }
        }
    }
}

/// Write each batch of saves as it arrives, keeping only the newest state
/// for each workspace, until the store is dropped.
fn run_writer(app_data: &Path, receiver: &Receiver<Message>) {
    while let Ok(first) = receiver.recv() {
        let mut views: Vec<(String, StoredView)> = Vec::new();
        let mut flushes = Vec::new();
        for message in std::iter::once(first).chain(receiver.try_iter()) {
            match message {
                Message::Save { workspace_id, view } => {
                    match views.iter_mut().find(|(saved, _)| *saved == workspace_id) {
                        Some(entry) => entry.1 = view,
                        None => views.push((workspace_id, view)),
                    }
                }
                Message::Flush(done) => flushes.push(done),
            }
        }
        for (workspace_id, view) in &views {
            write(app_data, workspace_id, view);
        }
        for done in flushes {
            // The store may have stopped waiting; nothing to answer then.
            let _ = done.send(());
        }
    }
}

fn store(app_data: &Path, workspace_id: &str) -> Option<JsonStore<StoredView>> {
    // The ID comes from a manifest that `local-core` validated; this
    // check keeps a bad one from naming another folder.
    let safe = !workspace_id.is_empty()
        && workspace_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
    safe.then(|| {
        let file = app_data
            .join("workspaces")
            .join(workspace_id)
            .join("view-state.json");
        JsonStore::new(file, "workspace view state")
    })
}

fn write(app_data: &Path, workspace_id: &str, view: &StoredView) {
    if let Some(store) = store(app_data, workspace_id)
        && let Err(error) = store.save(view)
    {
        tracing::warn!(%error, "a workspace's view state could not be saved");
    }
}
