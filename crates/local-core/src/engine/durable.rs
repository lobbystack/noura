//! Path normalization, atomic writes, and the fsync calls that make a
//! write durable before a mutation reports success.

use super::*;

pub(super) fn normalized_relative_path(
    root: &Path,
    path: &Path,
    operation: &str,
) -> Result<String> {
    path.strip_prefix(root)
        .ok()
        .and_then(Path::to_str)
        .map(|value| value.replace('\\', "/"))
        .ok_or_else(|| {
            CoreError::validation(
                "non_utf8_path",
                "The workspace contains a path that is not UTF-8",
                operation,
            )
        })
}

pub(super) fn atomic_write(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    operation: &str,
) -> Result<()> {
    atomic_write_checked(root, relative, bytes, None, operation)
}

pub(super) fn atomic_write_checked(
    root: &Path,
    relative: &Path,
    bytes: &[u8],
    expected_revision: Option<&str>,
    operation: &str,
) -> Result<()> {
    let destination = root.join(relative);
    if let Some(parent) = destination.parent() {
        create_folders(parent, operation)?;
    }
    let mut file = atomic_write_file::AtomicWriteFile::open(&destination)
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    if let Some(expected) = expected_revision {
        let shown = relative.to_string_lossy().replace('\\', "/");
        // The file may have gone online-only since the caller read it.
        let current = read_downloaded(&destination, &shown, operation)?;
        check_revision(&current, expected, operation)?;
    }
    file.commit()
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    sync_parent(&destination, operation)?;
    Ok(())
}

/// Create `folder` and every missing folder above it, then sync the folder
/// that holds each new one, so the whole chain is on disk before this
/// returns. A folder that already exists is left as it is.
pub(super) fn create_folders(folder: &Path, operation: &str) -> Result<()> {
    let mut missing = Vec::new();
    let mut current = folder;
    while !current.as_os_str().is_empty() {
        match std::fs::symlink_metadata(current) {
            Ok(_) => break,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(current);
                match current.parent() {
                    Some(parent) => current = parent,
                    None => break,
                }
            }
            Err(error) => return Err(CoreError::io(error, operation, current.to_str())),
        }
    }
    if missing.is_empty() && !folder.is_dir() {
        let error = std::io::Error::new(std::io::ErrorKind::AlreadyExists, "not a folder");
        return Err(CoreError::io(error, operation, folder.to_str()));
    }
    // Top down, so each folder's parent exists when it is created. Another
    // writer may create one of them meanwhile, which is fine.
    for created in missing.iter().rev() {
        if let Err(error) = std::fs::create_dir(created)
            && !(error.kind() == std::io::ErrorKind::AlreadyExists && created.is_dir())
        {
            return Err(CoreError::io(error, operation, created.to_str()));
        }
    }
    for created in missing.iter().rev() {
        sync_parent(created, operation)?;
    }
    Ok(())
}

/// Sync the folder holding `path`, so the entry for `path` is on disk.
#[cfg(unix)]
pub(super) fn sync_parent(path: &Path, operation: &str) -> Result<()> {
    let parent = match path.parent() {
        Some(parent) if parent.as_os_str().is_empty() => Path::new("."),
        Some(parent) => parent,
        None => path,
    };
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| CoreError::io(error, operation, parent.to_str()))?;
    #[cfg(any(test, feature = "durability-probe"))]
    crate::durability_probe::synced(parent);
    Ok(())
}

/// The standard library can't open a folder to sync it on Windows, so
/// this does nothing there.
#[cfg(not(unix))]
pub(super) fn sync_parent(path: &Path, _operation: &str) -> Result<()> {
    #[cfg(any(test, feature = "durability-probe"))]
    crate::durability_probe::synced(path.parent().unwrap_or(path));
    let _ = path;
    Ok(())
}

/// Whether two paths name the same file, as in a case-only rename on a
/// case-insensitive volume.
pub(super) fn same_file(left: &Path, right: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        match (std::fs::metadata(left), std::fs::metadata(right)) {
            (Ok(a), Ok(b)) => a.dev() == b.dev() && a.ino() == b.ino(),
            _ => false,
        }
    }

    #[cfg(not(unix))]
    {
        match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
            (Ok(a), Ok(b)) => a == b,
            _ => false,
        }
    }
}

pub(super) fn sync_rename_parents(
    source: &Path,
    destination: &Path,
    operation: &str,
) -> Result<()> {
    sync_parent(source, operation)?;
    if source.parent() != destination.parent() {
        sync_parent(destination, operation)?;
    }
    Ok(())
}

pub(super) fn mtime_ns(path: &Path) -> i64 {
    path.metadata()
        .and_then(|value| value.modified())
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

pub(super) fn check_revision(bytes: &[u8], expected: &str, operation: &str) -> Result<()> {
    let current = markdown::revision(bytes);
    if current != expected {
        let mut error = CoreError::new(
            "revision_conflict",
            ErrorCategory::Conflict,
            "The file changed since it was loaded",
            operation,
        );
        error.details = Some(serde_json::json!({"currentRevision":current}));
        return Err(error);
    }
    Ok(())
}

// Non-UTF-8 file names exist only on Unix file systems.
#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn relative_path_normalization_rejects_non_utf8_paths() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let root = PathBuf::from("/workspace");
        let invalid = OsString::from_vec(vec![b'i', b'n', b'v', 0xff]);
        let error = normalized_relative_path(&root, &root.join(invalid), "files_list").unwrap_err();

        assert_eq!(error.code, "non_utf8_path");
    }
}

#[cfg(test)]
mod durability_tests {
    use super::*;
    use crate::durability_probe;

    fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
        let workspace = tempfile::tempdir().unwrap();
        let app_data = tempfile::tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        (workspace, app_data, engine)
    }

    fn write(engine: &WorkspaceEngine, relative: &str, bytes: &[u8]) {
        let path = engine.root().join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// Assert that each of `folders`, relative to the workspace, was synced.
    fn assert_synced(engine: &WorkspaceEngine, synced: &[PathBuf], folders: &[&str]) {
        for folder in folders {
            let folder = match *folder {
                "" => engine.root().to_owned(),
                folder => engine.root().join(folder),
            };
            assert!(
                synced.contains(&folder),
                "{} wasn't synced; synced: {synced:?}",
                folder.display()
            );
        }
    }

    #[test]
    fn create_folders_syncs_the_folder_holding_each_new_folder() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::create_dir(root.join("a")).unwrap();

        let (created, synced) =
            durability_probe::record(|| create_folders(&root.join("a/b/c/d"), "test"));

        created.unwrap();
        assert!(root.join("a/b/c/d").is_dir());
        assert_eq!(
            synced,
            [root.join("a"), root.join("a/b"), root.join("a/b/c")]
        );
        let (again, synced) =
            durability_probe::record(|| create_folders(&root.join("a/b"), "test"));
        again.unwrap();
        assert!(synced.is_empty(), "{synced:?}");
    }

    #[test]
    fn create_folders_refuses_a_file_in_the_way() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("file"), b"x").unwrap();

        assert!(create_folders(&root.path().join("file"), "test").is_err());
        assert!(create_folders(&root.path().join("file/below"), "test").is_err());
        assert_eq!(std::fs::read(root.path().join("file")).unwrap(), b"x");
    }

    #[test]
    fn the_workspace_trash_syncs_every_folder_it_creates_and_the_item() {
        let (_workspace, _app_data, engine) = engine();
        std::fs::remove_dir_all(engine.root().join(".noura/trash")).unwrap();
        write(&engine, "Docs/Deep/Plan.md", b"plan");

        let (trashed, synced) = durability_probe::record(|| engine.trash_path("Docs/Deep/Plan.md"));

        let place = trashed.unwrap().unwrap();
        let stamp = place
            .strip_prefix(".noura/trash/")
            .and_then(|rest| rest.split('/').next())
            .unwrap();
        assert_eq!(std::fs::read(engine.root().join(&place)).unwrap(), b"plan");
        assert_synced(
            &engine,
            &synced,
            &[
                // The item left this folder.
                "Docs/Deep",
                // Each folder created on the way, and the item itself.
                ".noura",
                ".noura/trash",
                &format!(".noura/trash/{stamp}"),
                &format!(".noura/trash/{stamp}/Docs"),
                &format!(".noura/trash/{stamp}/Docs/Deep"),
            ],
        );
    }

    #[test]
    fn moves_copies_and_new_folders_sync_every_folder_they_create() {
        let (_workspace, _app_data, engine) = engine();
        write(&engine, "Plan.md", b"plan");
        write(&engine, "Docs/a.txt", b"a");
        write(&engine, "Data.csv", b"1,2");

        let (moved, synced) =
            durability_probe::record(|| engine.move_file("Plan.md", "A/B/Plan.md"));
        moved.unwrap();
        assert_synced(&engine, &synced, &["", "A", "A/B"]);

        let (moved, synced) = durability_probe::record(|| engine.move_folder("Docs", "C/D/Docs"));
        moved.unwrap();
        assert_synced(&engine, &synced, &["", "C", "C/D"]);

        let (copied, synced) =
            durability_probe::record(|| engine.copy_file("Data.csv", "E/F/Data.csv"));
        copied.unwrap();
        assert_synced(&engine, &synced, &["", "E", "E/F"]);

        let (created, synced) = durability_probe::record(|| engine.create_folder("G/H/I"));
        created.unwrap();
        assert_synced(&engine, &synced, &["", "G", "G/H"]);

        assert_eq!(
            std::fs::read(engine.root().join("A/B/Plan.md")).unwrap(),
            b"plan"
        );
        assert_eq!(
            std::fs::read(engine.root().join("C/D/Docs/a.txt")).unwrap(),
            b"a"
        );
        assert_eq!(
            std::fs::read(engine.root().join("E/F/Data.csv")).unwrap(),
            b"1,2"
        );
        assert!(engine.root().join("G/H/I").is_dir());
    }
}
