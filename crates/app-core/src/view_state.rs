//! Each workspace's view state on this device: its tabs and active tab,
//! the file tree's open folders and the rail's order. It lives in the app
//! data folder, never in the workspace.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::store::JsonStore;

/// The view state format this version writes.
const VERSION: u64 = 1;

/// A workspace's saved view state. Anything Noura can't read comes back
/// empty, and tabs it can't read drop out one by one: view state is a
/// convenience, never workspace data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Saved", into = "Saved")]
pub(crate) struct StoredView {
    pub(crate) tabs: Vec<StoredTab>,
    /// The index of the active tab in `tabs`.
    pub(crate) active: Option<usize>,
    /// The file tree's open folders.
    pub(crate) expanded_folders: Vec<String>,
    /// The rail's page IDs in the order you gave them. Empty until you
    /// move a page.
    pub(crate) rail_order: Vec<String>,
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

/// The file's shape, read field by field so one bad value drops only
/// itself.
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Saved {
    version: Value,
    tabs: Vec<Value>,
    active: Value,
    expanded_folders: Vec<Value>,
    rail_order: Vec<Value>,
}

impl From<Saved> for StoredView {
    fn from(saved: Saved) -> Self {
        if saved.version.as_u64() != Some(VERSION) {
            tracing::warn!("a workspace's view state has an unknown version");
            return Self::default();
        }
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
            rail_order: strings(saved.rail_order),
        }
    }
}

impl From<StoredView> for Saved {
    fn from(view: StoredView) -> Self {
        let values = |items: Vec<String>| items.into_iter().map(Value::String).collect();
        Self {
            version: VERSION.into(),
            tabs: view
                .tabs
                .into_iter()
                .filter_map(|tab| serde_json::to_value(tab).ok())
                .collect(),
            active: view.active.map_or(Value::Null, Into::into),
            expanded_folders: values(view.expanded_folders),
            rail_order: values(view.rail_order),
        }
    }
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
#[derive(Debug, Clone)]
pub(crate) struct ViewStore {
    app_data: PathBuf,
}

impl ViewStore {
    pub(crate) fn new(app_data: &Path) -> Self {
        Self {
            app_data: app_data.to_owned(),
        }
    }

    fn store(&self, workspace_id: &str) -> Option<JsonStore<StoredView>> {
        // The ID comes from a manifest that `local-core` validated; this
        // check keeps a bad one from naming another folder.
        let safe = !workspace_id.is_empty()
            && workspace_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        safe.then(|| {
            let file = self
                .app_data
                .join("workspaces")
                .join(workspace_id)
                .join("view-state.json");
            JsonStore::new(file, "workspace view state")
        })
    }

    /// The workspace's saved view state, or an empty one.
    pub(crate) fn load(&self, workspace_id: &str) -> StoredView {
        self.store(workspace_id)
            .map(|store| store.load())
            .unwrap_or_default()
    }

    /// Save the workspace's view state. A failure goes to the log: the
    /// view still works for this session.
    pub(crate) fn save(&self, workspace_id: &str, view: &StoredView) {
        if let Some(store) = self.store(workspace_id)
            && let Err(error) = store.save(view)
        {
            tracing::warn!(%error, "a workspace's view state could not be saved");
        }
    }
}
