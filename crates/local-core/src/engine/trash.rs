//! Deleted files go to the operating system trash, where the user restores
//! them like any other file. When the system has no trash or refuses the
//! move, they go to `.noura/trash/<timestamp>/<original relative path>`.

use super::*;

/// Moves one file or directory to the operating system trash.
pub type SystemTrash = Arc<dyn Fn(&Path) -> std::io::Result<()> + Send + Sync>;

/// This computer's trash: the macOS Trash, the Windows Recycle Bin, or the
/// freedesktop.org trash on Linux. `None` where the platform has none.
#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn os_trash() -> Option<SystemTrash> {
    Some(Arc::new(|path: &Path| {
        #[allow(unused_mut)]
        let mut context = ::trash::TrashContext::default();
        // The default Finder method asks for permission to control Finder.
        #[cfg(target_os = "macos")]
        ::trash::macos::TrashContextExtMacos::set_delete_method(
            &mut context,
            ::trash::macos::DeleteMethod::NsFileManager,
        );
        context.delete(path).map_err(std::io::Error::other)
    }))
}

#[cfg(any(target_os = "ios", target_os = "android"))]
pub fn os_trash() -> Option<SystemTrash> {
    None
}

impl WorkspaceEngine {
    /// Choose where deletions go. Without a system trash, which is the
    /// default, they go to `.noura/trash`.
    pub fn set_system_trash(&self, trash: Option<SystemTrash>) {
        if let Ok(mut current) = self.system_trash.write() {
            *current = trash;
        }
    }

    /// Move `source` out of the workspace. Returns the `.noura/trash` path
    /// when the file went there instead of the system trash. The removal is
    /// durable before this returns.
    pub(super) fn discard(
        &self,
        source: &Path,
        relative_path: &str,
        operation: &str,
    ) -> Result<Option<String>> {
        let system = self
            .system_trash
            .read()
            .ok()
            .and_then(|trash| trash.clone());
        if let Some(system) = system {
            match system(source) {
                Ok(()) => {
                    sync_parent(source, operation)?;
                    return Ok(None);
                }
                // A failed move that still removed the source can't fall
                // back, so report it instead of guessing where it went.
                Err(error) if std::fs::symlink_metadata(source).is_err() => {
                    return Err(CoreError::io(error, operation, Some(relative_path)));
                }
                Err(error) => {
                    tracing::warn!(%error, operation, "system trash unavailable, using .noura/trash");
                }
            }
        }
        let timestamp = now_rfc3339().replace([':', '.'], "-");
        let trash_relative = format!("{WORKSPACE_TRASH}/{timestamp}/{relative_path}");
        let trash = resolve_for_write(&self.root, &trash_relative, "trash_write")?;
        if std::fs::symlink_metadata(&trash).is_ok() {
            return Err(CoreError::new(
                "trash_destination_exists",
                ErrorCategory::Conflict,
                "Something already exists at the trash destination",
                operation,
            ));
        }
        if let Some(parent) = trash.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| CoreError::io(error, operation, parent.to_str()))?;
        }
        std::fs::rename(source, &trash)
            .map_err(|error| CoreError::io(error, operation, Some(relative_path)))?;
        sync_rename_parents(source, &trash, operation)?;
        Ok(Some(trash_relative))
    }
}

const WORKSPACE_TRASH: &str = ".noura/trash";
