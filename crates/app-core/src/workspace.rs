//! The workspace service: create, open and reopen workspaces.

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use local_core::{WORKSPACE_MANIFEST_PATH, WorkspaceEngine};

use crate::{
    AppError, NoSystem, System,
    error::Access,
    recents::{RecentStore, RecentWorkspace},
};

/// An open workspace.
pub struct Workspace {
    engine: Arc<WorkspaceEngine>,
    system: Arc<dyn System>,
    id: String,
    name: String,
    root: String,
}

impl Workspace {
    fn new(engine: WorkspaceEngine, system: Arc<dyn System>) -> Result<Self, AppError> {
        let manifest = engine.manifest();
        let root = engine
            .root()
            .to_str()
            .ok_or_else(AppError::unsupported_path)?
            .to_owned();
        // Deleted items go to the system trash when there is one, and to
        // the workspace's trash folder when it has none or refuses.
        if system.has_trash() {
            let trash = system.clone();
            engine.set_system_trash(Some(Arc::new(move |path: &Path| trash.trash(path))));
        }
        Ok(Self {
            engine: Arc::new(engine),
            system,
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

    /// The engine, for background work that outlives a borrow.
    pub(crate) fn shared_engine(&self) -> Arc<WorkspaceEngine> {
        self.engine.clone()
    }

    pub(crate) fn system(&self) -> &dyn System {
        self.system.as_ref()
    }

    fn recent(&self) -> RecentWorkspace {
        RecentWorkspace {
            workspace_id: self.id.clone(),
            name: self.name.clone(),
            path: self.root.clone(),
        }
    }
}

#[cfg(feature = "test-hooks")]
impl Workspace {
    /// Test hook: make the native watcher fail from now on.
    pub fn force_watch_fault(&self, fault: crate::WatchFault) {
        self.engine.force_watch_fault(fault);
    }

    /// Test hook: deliver one native watcher event that names these
    /// workspace-relative paths, as the platform would.
    pub fn inject_watch_event(&self, relative_paths: &[impl AsRef<Path>]) {
        let root = self.engine.root();
        self.engine
            .inject_watch_event(relative_paths.iter().map(|path| root.join(path)).collect());
    }

    /// Test hook: deliver one native watcher event that asks for a full
    /// rescan, as the platform would.
    pub fn inject_watch_rescan_request(&self) {
        self.engine.inject_watch_rescan_request();
    }
}

/// Creates and opens workspaces, and keeps the recent list. Per-device
/// state, such as the indexes and the recent list, lives under the app
/// data folder and never in a workspace.
pub struct WorkspaceService {
    app_data: PathBuf,
    recents: RecentStore,
    system: Arc<dyn System>,
}

impl WorkspaceService {
    /// A service whose workspaces use no operating system services:
    /// deleted items go to each workspace's trash folder.
    pub fn new(app_data: impl Into<PathBuf>) -> Self {
        Self::with_system(app_data, Arc::new(NoSystem))
    }

    /// A service whose workspaces use `system` for the trash, the file
    /// manager, terminals and the clipboard.
    pub fn with_system(app_data: impl Into<PathBuf>, system: Arc<dyn System>) -> Self {
        let app_data = app_data.into();
        Self {
            recents: RecentStore::new(&app_data),
            app_data,
            system,
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
    /// stays unchanged. A blank name changes nothing on disk.
    pub fn create(&self, name: &str, folder: &Path) -> Result<Workspace, AppError> {
        if name.trim().is_empty() {
            return Err(AppError::new(
                "workspace_name_required",
                "Enter a name for the workspace.",
            ));
        }
        let path = representable(folder)?;
        check_links(folder, path)?;
        let engine = WorkspaceEngine::create_with_app_data(folder, name, &self.app_data)
            .map_err(|error| AppError::from_core(error, path, Access::SetUpWorkspace))?;
        self.opened(engine)
    }

    /// Open the workspace in `folder`, whichever workspace it holds now. A
    /// plain folder becomes a new workspace with a new ID, named after the
    /// folder, and its files keep their bytes. The recent list never lends
    /// a folder an identity because of its path. An invalid manifest opens
    /// nothing and stays as it is.
    pub fn open_folder(&self, folder: &Path) -> Result<Workspace, AppError> {
        let path = representable(folder)?;
        check_folder(folder, path)?;
        check_links(folder, path)?;
        let access = match folder.join(WORKSPACE_MANIFEST_PATH).try_exists() {
            Ok(true) => Access::ReadWorkspace,
            Ok(false) => Access::SetUpWorkspace,
            Err(error) => return Err(unreadable(&error, path)),
        };
        let engine = WorkspaceEngine::open_or_initialize_with_app_data(
            folder,
            &folder_name(folder),
            None,
            &self.app_data,
        )
        .map_err(|error| AppError::from_core(error, path, access))?;
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
    /// its place. A folder without a manifest opens nothing and stays as it
    /// is: Noura can't tell it is the saved workspace, so Open folder makes
    /// it a new one.
    fn open_saved(&self, recent: &RecentWorkspace) -> Result<Workspace, AppError> {
        let folder = Path::new(&recent.path);
        check_folder(folder, &recent.path)?;
        check_links(folder, &recent.path)?;
        check_manifest(folder, &recent.path)?;
        let engine = WorkspaceEngine::open_with_app_data(folder, &self.app_data)
            .map_err(|error| AppError::from_core(error, &recent.path, Access::ReadWorkspace))?;
        check_identity(&engine, &recent.workspace_id, &recent.path)?;
        self.opened(engine)
    }

    fn opened(&self, engine: WorkspaceEngine) -> Result<Workspace, AppError> {
        let workspace = Workspace::new(engine, self.system.clone())?;
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
        Err(error) => Err(unreadable(&error, path)),
    }
}

/// A folder Noura couldn't check. Only a permission error says
/// "permission"; the details go to the log.
fn unreadable(error: &std::io::Error, path: &str) -> AppError {
    tracing::warn!(%error, "a workspace folder could not be checked");
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        AppError::new(
            "folder_unreadable",
            format!("Noura doesn't have permission to read {path}."),
        )
    } else {
        AppError::new("folder_unavailable", format!("Noura couldn't open {path}."))
    }
}

/// Refuse a `.noura` folder or settings file that is a link. `local-core`
/// reads the settings file through a link, so a linked one would lend this
/// folder another workspace's ID, recent list entry and index.
fn check_links(folder: &Path, path: &str) -> Result<(), AppError> {
    let linked = [
        (".noura", format!("The .noura folder in {path} is a link")),
        (
            WORKSPACE_MANIFEST_PATH,
            format!("The settings file .noura/workspace.yaml in {path} is a link"),
        ),
    ];
    for (relative, what) in linked {
        match std::fs::symlink_metadata(folder.join(relative)) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(AppError::new(
                    "symlink_escape",
                    format!("{what}, so Noura won't use it."),
                ));
            }
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) => {}
            Err(error) => return Err(unreadable(&error, path)),
        }
    }
    Ok(())
}

/// Refuse a saved folder whose manifest is gone instead of writing the
/// saved ID into whatever folder now sits at the path.
fn check_manifest(folder: &Path, path: &str) -> Result<(), AppError> {
    match folder.join(WORKSPACE_MANIFEST_PATH).try_exists() {
        Ok(true) => Ok(()),
        Ok(false) => Err(AppError::new(
            "workspace_manifest_missing",
            format!(
                "The settings file .noura/workspace.yaml is missing from {path}. Noura left \
                 the folder as it was. Choose Open folder to use it as a new workspace."
            ),
        )),
        Err(error) => Err(unreadable(&error, path)),
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

fn folder_name(folder: &Path) -> String {
    folder
        .file_name()
        .and_then(|name| name.to_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or("Workspace")
        .to_owned()
}
