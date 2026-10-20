//! Each workspace's view state on this device: its tabs and active tab.
//! It lives in the app data folder, never in the workspace.

use std::{
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

/// The view state format this version writes.
const VERSION: u32 = 1;

/// A workspace's saved view state. Anything Noura can't read comes back
/// empty: view state is a convenience, never workspace data.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
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

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Written<'a> {
    version: u32,
    #[serde(flatten)]
    view: &'a StoredView,
}

/// Reads and writes `workspaces/<workspace ID>/view-state.json` under the
/// app data folder, next to `local-core`'s index for that workspace.
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

    fn file(&self, workspace_id: &str) -> Option<PathBuf> {
        // The ID comes from a manifest that `local-core` validated; this
        // check keeps a bad one from naming another folder.
        let safe = !workspace_id.is_empty()
            && workspace_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'));
        safe.then(|| {
            self.app_data
                .join("workspaces")
                .join(workspace_id)
                .join("view-state.json")
        })
    }

    /// The saved view state, or an empty one when there is none or it
    /// can't be read. Tabs that can't be read are left out one by one.
    pub(crate) fn load(&self, workspace_id: &str) -> StoredView {
        let Some(file) = self.file(workspace_id) else {
            return StoredView::default();
        };
        let bytes = match std::fs::read(&file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return StoredView::default();
            }
            Err(error) => {
                tracing::warn!(%error, "the workspace's view state could not be read");
                return StoredView::default();
            }
        };
        let value: serde_json::Value = match serde_json::from_slice(&bytes) {
            Ok(value) => value,
            Err(error) => {
                tracing::warn!(%error, "the workspace's view state is invalid");
                return StoredView::default();
            }
        };
        if value.get("version").and_then(serde_json::Value::as_u64) != Some(VERSION.into()) {
            tracing::warn!("the workspace's view state has an unknown version");
            return StoredView::default();
        }
        let tabs = value
            .get("tabs")
            .and_then(serde_json::Value::as_array)
            .map(|tabs| {
                tabs.iter()
                    .map(|tab| serde_json::from_value::<StoredTab>(tab.clone()).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        // Unreadable tabs drop out as if closed.
        let kept: Vec<bool> = tabs.iter().map(Option::is_some).collect();
        let active = value
            .get("active")
            .and_then(serde_json::Value::as_u64)
            .and_then(|active| usize::try_from(active).ok())
            .and_then(|active| crate::tabs::takes_place(&kept, active));
        StoredView {
            tabs: tabs.into_iter().flatten().collect(),
            active,
            expanded_folders: strings(&value, "expandedFolders"),
            rail_order: strings(&value, "railOrder"),
        }
    }

    /// Save the view state. A failure goes to the log: the view still
    /// works for this session.
    pub(crate) fn save(&self, workspace_id: &str, view: &StoredView) {
        let Some(file) = self.file(workspace_id) else {
            return;
        };
        if let Err(error) = write(&file, view) {
            tracing::warn!(%error, "the workspace's view state could not be saved");
        }
    }
}

/// The strings in the array `key` of `value`, leaving out anything else.
fn strings(value: &serde_json::Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn write(file: &Path, view: &StoredView) -> std::io::Result<()> {
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let bytes = serde_json::to_vec_pretty(&Written {
        version: VERSION,
        view,
    })
    .map_err(std::io::Error::other)?;
    let mut out = atomic_write_file::AtomicWriteFile::open(file)?;
    out.write_all(&bytes)?;
    out.commit()
}
