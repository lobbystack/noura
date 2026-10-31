//! The registry of synced folders (docs/workspace-format/sync-v2.md §15):
//! in the app's application data, outside every workspace, it maps each
//! account and remote workspace to one folder and a random replica ID,
//! which the folder's journal also stores. A folder whose journal holds
//! another replica ID, such as a copy you made in Finder or a folder
//! another Mac opened through a cloud drive, doesn't sync until you connect
//! it. The registry also keeps whether you paused sync, so a pause holds
//! across relaunch, and whether you turned it off.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sync_protocol::types::Id;

use super::SyncError;
use super::journal::durable_write;

/// The registry's file in the app's application data folder.
pub const REGISTRY_FILE: &str = "sync-folders.json";

/// One synced folder.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncedFolder {
    /// The sync server's address.
    pub server: String,
    /// The account.
    pub account: Id,
    /// The remote workspace.
    pub workspace: Id,
    /// The folder on this computer.
    pub folder: PathBuf,
    /// The random replica ID its journal stores.
    pub replica: Id,
    /// Whether you paused sync.
    #[serde(default)]
    pub paused: bool,
    /// Whether sync is on. Turning it off keeps the entry, and its journal,
    /// so turning it on again continues the same remote workspace.
    #[serde(default = "on")]
    pub on: bool,
}

fn on() -> bool {
    true
}

/// The registry.
#[derive(Debug)]
pub struct Registry {
    path: PathBuf,
    folders: Vec<SyncedFolder>,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    folders: Vec<SyncedFolder>,
}

fn failed(error: impl std::fmt::Display) -> SyncError {
    SyncError::Invalid(format!("the list of synced folders can't be used: {error}"))
}

impl Registry {
    /// Opens the registry in `app_data`.
    pub fn open(app_data: &Path) -> Result<Self, SyncError> {
        let path = app_data.join(REGISTRY_FILE);
        let stored: Stored = match std::fs::read(&path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(failed)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Stored::default(),
            Err(error) => return Err(failed(error)),
        };
        Ok(Self {
            path,
            folders: stored.folders,
        })
    }

    /// Every synced folder.
    pub fn folders(&self) -> &[SyncedFolder] {
        &self.folders
    }

    /// The entry of `workspace` for `account` on `server`.
    pub fn of_workspace(
        &self,
        server: &str,
        account: &Id,
        workspace: &Id,
    ) -> Option<&SyncedFolder> {
        self.folders
            .iter()
            .find(|f| f.server == server && f.account == *account && f.workspace == *workspace)
    }

    /// The entry of the folder at `folder`, whichever workspace it syncs.
    pub fn of_folder(&self, folder: &Path) -> Option<&SyncedFolder> {
        let wanted = canonical(folder);
        self.folders.iter().find(|f| canonical(&f.folder) == wanted)
    }

    /// Records that `folder` syncs `workspace`, with a new replica ID, in
    /// place of any earlier entry for that workspace or that folder, and
    /// returns the replica ID.
    pub fn connect(
        &mut self,
        server: &str,
        account: Id,
        workspace: Id,
        folder: &Path,
    ) -> Result<Id, SyncError> {
        let replica =
            Id::random().map_err(|e| SyncError::Invalid(format!("no randomness: {e}")))?;
        let wanted = canonical(folder);
        self.folders.retain(|f| {
            !(f.server == server && f.account == account && f.workspace == workspace)
                && canonical(&f.folder) != wanted
        });
        self.folders.push(SyncedFolder {
            server: server.to_owned(),
            account,
            workspace,
            folder: folder.to_owned(),
            replica,
            paused: false,
            on: true,
        });
        self.save()?;
        Ok(replica)
    }

    /// Changes the entry of `workspace` with `change`, then saves.
    pub fn update(
        &mut self,
        server: &str,
        account: &Id,
        workspace: &Id,
        change: impl FnOnce(&mut SyncedFolder),
    ) -> Result<(), SyncError> {
        let entry = self
            .folders
            .iter_mut()
            .find(|f| f.server == server && f.account == *account && f.workspace == *workspace)
            .ok_or_else(|| SyncError::Invalid("this workspace doesn't sync here".into()))?;
        change(entry);
        self.save()
    }

    /// Drops the entry of `workspace`.
    pub fn remove(&mut self, server: &str, account: &Id, workspace: &Id) -> Result<(), SyncError> {
        self.folders.retain(|f| {
            !(f.server == server && f.account == *account && f.workspace == *workspace)
        });
        self.save()
    }

    fn save(&self) -> Result<(), SyncError> {
        let bytes = serde_json::to_vec_pretty(&Stored {
            folders: self.folders.clone(),
        })
        .map_err(failed)?;
        durable_write(&self.path, &bytes).map_err(failed)
    }
}

fn canonical(folder: &Path) -> PathBuf {
    folder.canonicalize().unwrap_or_else(|_| folder.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_folder_syncs_one_workspace_and_a_workspace_one_folder() {
        let data = tempfile::tempdir().unwrap();
        let folder = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let mut registry = Registry::open(data.path()).unwrap();
        let account = Id([1; 16]);
        let first = registry
            .connect("https://sync.example", account, Id([2; 16]), folder.path())
            .unwrap();
        let again = registry
            .connect("https://sync.example", account, Id([3; 16]), folder.path())
            .unwrap();
        assert_ne!(first, again);
        assert_eq!(
            registry.folders().len(),
            1,
            "the folder now syncs the other"
        );
        registry
            .connect("https://sync.example", account, Id([3; 16]), other.path())
            .unwrap();
        assert_eq!(registry.folders().len(), 1);
        registry
            .update("https://sync.example", &account, &Id([3; 16]), |f| {
                f.paused = true
            })
            .unwrap();
        let reopened = Registry::open(data.path()).unwrap();
        let entry = reopened.of_folder(other.path()).unwrap();
        assert!(entry.paused, "a pause holds across relaunch");
        assert_eq!(entry.workspace, Id([3; 16]));
        assert!(reopened.of_folder(folder.path()).is_none());
    }
}
