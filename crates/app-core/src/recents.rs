//! The recent-workspace list, kept in this device's app data.

use std::{io::Write, path::PathBuf};

use serde::{Deserialize, Serialize};

use crate::AppError;

/// How many workspaces the recent list keeps.
pub const RECENT_LIMIT: usize = 20;

/// A workspace this device opened, keyed by its workspace ID. The path
/// locates the folder; the ID in its manifest proves it is the same
/// workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecentWorkspace {
    pub workspace_id: String,
    pub name: String,
    pub path: String,
}

pub(crate) struct RecentStore {
    file: PathBuf,
}

impl RecentStore {
    pub(crate) fn new(app_data: &std::path::Path) -> Self {
        Self {
            file: app_data.join("recent-workspaces.json"),
        }
    }

    /// The list, most recent first. A missing or unreadable file is an
    /// empty list: recents are a convenience, never workspace data.
    pub(crate) fn load(&self) -> Vec<RecentWorkspace> {
        let bytes = match std::fs::read(&self.file) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
            Err(error) => {
                tracing::warn!(%error, "the recent workspaces could not be read");
                return Vec::new();
            }
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            tracing::warn!(%error, "the recent workspaces file is invalid");
            Vec::new()
        })
    }

    /// Put `entry` first, dropping any older entry with the same ID.
    pub(crate) fn record(&self, entry: RecentWorkspace) -> Result<Vec<RecentWorkspace>, AppError> {
        let mut values = self.load();
        values.retain(|value| value.workspace_id != entry.workspace_id);
        values.insert(0, entry);
        values.truncate(RECENT_LIMIT);
        self.save(&values)?;
        Ok(values)
    }

    /// Drop one workspace from the list. Its folder stays as it is.
    pub(crate) fn remove(&self, workspace_id: &str) -> Result<Vec<RecentWorkspace>, AppError> {
        let mut values = self.load();
        let before = values.len();
        values.retain(|value| value.workspace_id != workspace_id);
        if values.len() != before {
            self.save(&values)?;
        }
        Ok(values)
    }

    fn save(&self, values: &[RecentWorkspace]) -> Result<(), AppError> {
        let failed = |error: std::io::Error| {
            tracing::warn!(%error, "the recent workspaces could not be saved");
            AppError::new(
                "recents_save_failed",
                "Noura couldn't update the list of recent workspaces.",
            )
        };
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent).map_err(failed)?;
        }
        let bytes = serde_json::to_vec_pretty(values)
            .map_err(|error| failed(std::io::Error::other(error)))?;
        let mut file = atomic_write_file::AtomicWriteFile::open(&self.file).map_err(failed)?;
        file.write_all(&bytes).map_err(failed)?;
        file.commit().map_err(failed)
    }
}
