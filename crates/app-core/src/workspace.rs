//! The workspace service: create, open and reopen workspaces.

use std::path::{Path, PathBuf};

use local_core::WorkspaceEngine;

use crate::{
    AppError,
    recents::{RecentStore, RecentWorkspace},
};

/// An open workspace.
pub struct Workspace {
    engine: WorkspaceEngine,
    id: String,
    name: String,
    root: String,
}

impl Workspace {
    fn new(engine: WorkspaceEngine) -> Result<Self, AppError> {
        let manifest = engine.manifest();
        let root = engine
            .root()
            .to_str()
            .ok_or_else(AppError::unsupported_path)?
            .to_owned();
        Ok(Self {
            engine,
            id: manifest.id,
            name: manifest.name,
            root,
        })
    }

    /// The stable workspace ID from the manifest.
    pub fn id(&self) -> &str {
        &self.id
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// The workspace folder's absolute path.
    pub fn root(&self) -> &str {
        &self.root
    }

    pub(crate) fn engine(&self) -> &WorkspaceEngine {
        &self.engine
    }

    fn recent(&self) -> RecentWorkspace {
        RecentWorkspace {
            workspace_id: self.id.clone(),
            name: self.name.clone(),
            path: self.root.clone(),
        }
    }
}

/// Creates and opens workspaces, and keeps the recent list. Per-device
/// state, such as the indexes and the recent list, lives under the app
/// data folder and never in a workspace.
pub struct WorkspaceService {
    app_data: PathBuf,
    recents: RecentStore,
}

impl WorkspaceService {
    pub fn new(app_data: impl Into<PathBuf>) -> Self {
        let app_data = app_data.into();
        Self {
            recents: RecentStore::new(&app_data),
            app_data,
        }
    }

    /// Recent workspaces, most recent first.
    pub fn recents(&self) -> Vec<RecentWorkspace> {
        self.recents.load()
    }

    /// Drop a workspace from the recent list. Its folder stays as it is.
    pub fn remove_recent(&self, workspace_id: &str) -> Result<Vec<RecentWorkspace>, AppError> {
        self.recents.remove(workspace_id)
    }

    /// Create a workspace named `name` in `folder`. Files already in the
    /// folder keep their bytes, and a folder that already holds a workspace
    /// stays unchanged.
    pub fn create(&self, name: &str, folder: &Path) -> Result<Workspace, AppError> {
        let path = representable(folder)?;
        let engine = WorkspaceEngine::create_with_app_data(folder, name, &self.app_data)
            .map_err(|error| AppError::from_core(error, path))?;
        self.opened(engine)
    }

    /// Open the workspace in `folder`. A plain folder becomes a workspace
    /// named after the folder, and its files keep their bytes. An invalid
    /// manifest opens nothing and stays as it is.
    pub fn open_folder(&self, folder: &Path) -> Result<Workspace, AppError> {
        let path = representable(folder)?;
        check_folder(folder, path)?;
        // A folder this device knew keeps its identity if its manifest went
        // missing.
        let recents = self.recents.load();
        let known = recent_at(&recents, folder);
        let name = known.map_or_else(|| folder_name(folder), |recent| recent.name.clone());
        let saved_id = known.map(|recent| recent.workspace_id.as_str());
        let engine = WorkspaceEngine::open_or_initialize_with_app_data(
            folder,
            &name,
            saved_id,
            &self.app_data,
        )
        .map_err(|error| AppError::from_core(error, path))?;
        if let Some(saved_id) = saved_id {
            check_identity(&engine, saved_id, path)?;
        }
        self.opened(engine)
    }

    /// Open a workspace from the recent list. The entry stays listed when
    /// the workspace fails to open, so it can be removed.
    pub fn open_recent(&self, workspace_id: &str) -> Result<Workspace, AppError> {
        let recent = self
            .recents
            .load()
            .into_iter()
            .find(|recent| recent.workspace_id == workspace_id)
            .ok_or_else(|| {
                AppError::new(
                    "recent_not_found",
                    "That workspace isn't in the recent list anymore.",
                )
            })?;
        self.open_saved(&recent)
    }

    /// Reopen the most recent workspace. Returns `None` when the list is
    /// empty. Never opens an older entry instead, and never creates the
    /// folder.
    pub fn restore_last(&self) -> Result<Option<Workspace>, AppError> {
        match self.recents.load().first() {
            Some(last) => self.open_saved(last).map(Some),
            None => Ok(None),
        }
    }

    /// Open a saved entry. The path locates the folder and the manifest ID
    /// must match, so a different workspace at a reused path never opens in
    /// its place. A missing manifest is recreated with the saved identity.
    fn open_saved(&self, recent: &RecentWorkspace) -> Result<Workspace, AppError> {
        let folder = Path::new(&recent.path);
        check_folder(folder, &recent.path)?;
        let engine = WorkspaceEngine::open_or_initialize_with_app_data(
            folder,
            &recent.name,
            Some(&recent.workspace_id),
            &self.app_data,
        )
        .map_err(|error| AppError::from_core(error, &recent.path))?;
        check_identity(&engine, &recent.workspace_id, &recent.path)?;
        self.opened(engine)
    }

    fn opened(&self, engine: WorkspaceEngine) -> Result<Workspace, AppError> {
        let workspace = Workspace::new(engine)?;
        // The workspace is open either way; a stale recent list is not
        // worth failing for.
        if let Err(error) = self.recents.record(workspace.recent()) {
            tracing::warn!(code = %error.code, "the workspace opened but recents were not saved");
        }
        Ok(workspace)
    }
}

/// The folder as UTF-8, or the unsupported-path error. Noura never stores
/// or shows a lossy copy of a path.
fn representable(folder: &Path) -> Result<&str, AppError> {
    folder.to_str().ok_or_else(AppError::unsupported_path)
}

/// Explain a folder that is gone, unreadable or not a folder before
/// `local-core` turns it into a generic filesystem error.
fn check_folder(folder: &Path, path: &str) -> Result<(), AppError> {
    match std::fs::metadata(folder) {
        Ok(metadata) if metadata.is_dir() => Ok(()),
        Ok(_) => Err(AppError::new(
            "not_a_folder",
            format!("{path} isn't a folder."),
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(AppError::new(
            "folder_missing",
            format!(
                "The folder {path} isn't there. If it's on a drive that isn't connected, \
                 connect the drive and try again."
            ),
        )),
        Err(error) => {
            tracing::warn!(%error, "a workspace folder could not be checked");
            Err(AppError::new(
                "folder_unreadable",
                format!("Noura doesn't have permission to read {path}."),
            ))
        }
    }
}

fn check_identity(engine: &WorkspaceEngine, saved_id: &str, path: &str) -> Result<(), AppError> {
    if engine.manifest().id == saved_id {
        return Ok(());
    }
    Err(AppError::new(
        "workspace_identity_changed",
        format!("A different workspace is now at {path}."),
    ))
}

fn recent_at<'a>(recents: &'a [RecentWorkspace], folder: &Path) -> Option<&'a RecentWorkspace> {
    let requested = folder.canonicalize().ok()?;
    recents.iter().find(|recent| {
        Path::new(&recent.path)
            .canonicalize()
            .is_ok_and(|candidate| candidate == requested)
    })
}

fn folder_name(folder: &Path) -> String {
    folder
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Workspace")
        .to_owned()
}
