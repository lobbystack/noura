//! The recent-workspace list, kept in this device's app data.

use serde::{Deserialize, Serialize};

use crate::{AppError, store::JsonStore};

/// How many workspaces the recent list keeps.
pub const RECENT_LIMIT: usize = 20;

/// A workspace this device opened, keyed by its workspace ID. The path
/// locates the folder; the ID in its manifest proves it is the same
/// workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentWorkspace {
    pub workspace_id: String,
    /// The folder's name, which is the workspace's name. The list file
    /// doesn't store it, and a name an earlier build stored is ignored.
    #[serde(skip)]
    pub name: String,
    pub path: String,
}

impl RecentWorkspace {
    pub(crate) fn new(workspace_id: String, path: String) -> Self {
        let mut recent = Self {
            workspace_id,
            name: String::new(),
            path,
        };
        recent.name = crate::workspace::folder_name(std::path::Path::new(&recent.path));
        recent
    }
}

/// Name each entry after its folder.
fn named(values: Vec<RecentWorkspace>) -> Vec<RecentWorkspace> {
    values
        .into_iter()
        .map(|value| RecentWorkspace::new(value.workspace_id, value.path))
        .collect()
}

pub(crate) struct RecentStore {
    store: JsonStore<Vec<RecentWorkspace>>,
}

impl RecentStore {
    pub(crate) fn new(app_data: &std::path::Path) -> Self {
        Self {
            store: JsonStore::new(app_data.join("recent-workspaces.json"), "recent workspaces"),
        }
    }

    /// The list, most recent first. A missing or unreadable file is an
    /// empty list: recents are a convenience, never workspace data.
    pub(crate) fn load(&self) -> Vec<RecentWorkspace> {
        named(self.store.load())
    }

    /// Put `entry` first, dropping any older entry with the same ID.
    pub(crate) fn record(&self, entry: RecentWorkspace) -> Result<Vec<RecentWorkspace>, AppError> {
        self.store
            .update(|values| {
                values.retain(|value| value.workspace_id != entry.workspace_id);
                values.insert(0, entry);
                values.truncate(RECENT_LIMIT);
                true
            })
            .map(named)
            .map_err(save_failed)
    }

    /// Drop one workspace from the list. Its folder stays as it is.
    pub(crate) fn remove(&self, workspace_id: &str) -> Result<Vec<RecentWorkspace>, AppError> {
        self.store
            .update(|values| {
                let before = values.len();
                values.retain(|value| value.workspace_id != workspace_id);
                values.len() != before
            })
            .map(named)
            .map_err(save_failed)
    }
}

fn save_failed(error: std::io::Error) -> AppError {
    tracing::warn!(%error, "the recent workspaces could not be saved");
    AppError::new("recents_save_failed", "Couldn't update recent workspaces.")
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    fn entry(workspace_id: &str) -> RecentWorkspace {
        RecentWorkspace::new(workspace_id.to_owned(), format!("/{workspace_id}"))
    }

    /// The list shows each folder's name, never a name an earlier build
    /// stored, and doesn't store one.
    #[test]
    fn each_entry_takes_its_folder_s_name() {
        let app_data = tempfile::tempdir().unwrap();
        let file = app_data.path().join("recent-workspaces.json");
        std::fs::write(
            &file,
            r#"[{"workspaceId":"one","name":"Old name","path":"/work/Garden"}]"#,
        )
        .unwrap();
        let store = RecentStore::new(app_data.path());

        assert_eq!(store.load()[0].name, "Garden");
        let recorded = store.record(entry("two")).unwrap();

        assert_eq!(
            recorded
                .iter()
                .map(|recent| recent.name.as_str())
                .collect::<Vec<_>>(),
            ["two", "Garden"]
        );
        let saved = std::fs::read_to_string(&file).unwrap();
        assert!(!saved.contains("name"), "{saved}");
    }

    #[test]
    fn a_removal_made_while_another_thread_records_is_never_undone() {
        let app_data = tempfile::tempdir().unwrap();
        let store = Arc::new(RecentStore::new(app_data.path()));
        let gone: Vec<String> = (0..10).map(|index| format!("gone-{index}")).collect();
        for workspace_id in &gone {
            store.record(entry(workspace_id)).unwrap();
        }

        let recorder = {
            let store = store.clone();
            std::thread::spawn(move || {
                for _ in 0..300 {
                    store.record(entry("kept")).unwrap();
                }
            })
        };
        for workspace_id in &gone {
            store.remove(workspace_id).unwrap();
            std::thread::yield_now();
        }
        recorder.join().unwrap();

        assert_eq!(store.load(), [entry("kept")]);
    }
}
