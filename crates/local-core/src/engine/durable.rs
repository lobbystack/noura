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
        std::fs::create_dir_all(parent)
            .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    }
    let mut file = atomic_write_file::AtomicWriteFile::open(&destination)
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    if let Some(expected) = expected_revision {
        let current = std::fs::read(&destination)
            .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
        check_revision(&current, expected, operation)?;
    }
    file.commit()
        .map_err(|error| CoreError::io(error, operation, destination.to_str()))?;
    sync_parent(&destination, operation)?;
    Ok(())
}

#[cfg(unix)]
pub(super) fn sync_parent(path: &Path, operation: &str) -> Result<()> {
    let parent = path.parent().unwrap_or(path);
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|error| CoreError::io(error, operation, parent.to_str()))
}

#[cfg(not(unix))]
pub(super) fn sync_parent(_path: &Path, _operation: &str) -> Result<()> {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn relative_path_normalization_rejects_non_utf8_paths() {
        use std::{ffi::OsString, os::unix::ffi::OsStringExt};

        let root = PathBuf::from("/workspace");
        let invalid = OsString::from_vec(vec![b'i', b'n', b'v', 0xff]);
        let error = normalized_relative_path(&root, &root.join(invalid), "files_list").unwrap_err();

        assert_eq!(error.code, "non_utf8_path");
    }
}
