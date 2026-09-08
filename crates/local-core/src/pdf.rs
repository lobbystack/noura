use crate::{CoreError, ErrorCategory, Result};
use std::{
    fs::{File, Metadata},
    io::Read,
    path::Path,
};

const LIMIT: u64 = 64 * 1024 * 1024;
const OP: &str = "files_read_pdf";

/// Canonical bytes for a read-only PDF preview. Never sourced from the index.
pub struct PdfRead {
    pub relative_path: String,
    pub revision: String,
    pub bytes: Vec<u8>,
}

fn invalid(code: &str, message: &str) -> CoreError {
    CoreError::validation(code, message, OP)
}

pub(crate) fn decode_target(target: &str) -> Result<String> {
    let mut bytes = Vec::with_capacity(target.len());
    let mut source = target.as_bytes().iter().copied();
    while let Some(byte) = source.next() {
        if byte == b'%' {
            let high = source.next().and_then(|c| (c as char).to_digit(16));
            let low = source.next().and_then(|c| (c as char).to_digit(16));
            let (Some(high), Some(low)) = (high, low) else {
                return Err(invalid(
                    "invalid_markdown_target",
                    "Invalid URL escape in file link",
                ));
            };
            bytes.push((high * 16 + low) as u8);
        } else {
            bytes.push(byte);
        }
    }
    let value = String::from_utf8(bytes)
        .map_err(|_| invalid("non_utf8_path", "The file path is not UTF-8"))?;
    if value.contains('\0') || value.contains('\\') {
        return Err(invalid(
            "unsafe_path",
            "The file link contains an unsupported path",
        ));
    }
    Ok(value)
}

// Open each component relative to an already-open directory. NOFOLLOW applies
// at every step, including when another process replaces a parent concurrently.
#[cfg(unix)]
fn open_scoped(root: &Path, relative: &Path) -> std::io::Result<File> {
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    let mut current = File::open(root)?;
    let components: Vec<_> = relative.components().collect();
    for (index, component) in components.iter().enumerate() {
        let name = std::ffi::CString::new(component.as_os_str().as_bytes())?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if index + 1 < components.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        // SAFETY: name is NUL terminated; current owns a live directory fd.
        let fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        // SAFETY: openat returned a new owned descriptor.
        current = unsafe { File::from_raw_fd(fd) };
    }
    Ok(current)
}

#[cfg(windows)]
fn open_scoped(root: &Path, relative: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::{MetadataExt, OpenOptionsExt};
    let mut guards = Vec::new();
    let mut path = root.to_owned();
    // Deny write/delete sharing while descending, preventing directory swaps.
    let final_index = relative.components().count();
    for (index, component) in std::iter::once(None)
        .chain(relative.components().map(Some))
        .enumerate()
    {
        if let Some(component) = component {
            path.push(component);
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .share_mode(if index == final_index { 3 } else { 1 })
            .custom_flags(0x00200000 | 0x02000000)
            .open(&path)?;
        if file.metadata()?.file_attributes() & 0x400 != 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::PermissionDenied,
                "reparse point",
            ));
        }
        guards.push(file);
    }
    guards
        .pop()
        .ok_or_else(|| std::io::Error::other("missing file"))
}

fn unchanged(before: &Metadata, after: &Metadata) -> bool {
    let same = before.len() == after.len() && before.modified().ok() == after.modified().ok();
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        same && before.dev() == after.dev()
            && before.ino() == after.ino()
            && before.ctime() == after.ctime()
            && before.ctime_nsec() == after.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        same && before.created().ok() == after.created().ok()
    }
}

fn changed() -> CoreError {
    let mut error = CoreError::new(
        "pdf_changed",
        ErrorCategory::Transient,
        "The PDF changed while being read. Please retry.",
        OP,
    );
    error.retryable = true;
    error
}

pub(crate) fn read(root: &Path, relative: &str) -> Result<PdfRead> {
    read_with(root, relative, || {})
}

fn read_with(root: &Path, relative: &str, after_read: impl FnOnce()) -> Result<PdfRead> {
    let path = crate::path::validate_relative(relative, OP)?;
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return Err(invalid("invalid_pdf", "Choose a PDF file"));
    }
    // Give deterministic validation errors for static symlinks; open_scoped
    // also protects against races after this check.
    crate::path::resolve_for_write(root, relative, OP)?;
    let io = |error| CoreError::io(error, OP, Some(relative));
    let mut file = open_scoped(root, &path).map_err(io)?;
    let before = file.metadata().map_err(io)?;
    if !before.is_file() {
        return Err(invalid("invalid_pdf", "The PDF must be a regular file"));
    }
    if before.len() > LIMIT {
        return Err(invalid(
            "pdf_too_large",
            "PDF previews are limited to 64 MiB",
        ));
    }
    let mut bytes = Vec::new();
    file.by_ref()
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() as u64 > LIMIT {
        return Err(invalid(
            "pdf_too_large",
            "PDF previews are limited to 64 MiB",
        ));
    }
    after_read();
    let after = file.metadata().map_err(io)?;
    let current = open_scoped(root, &path)
        .map_err(|_| changed())?
        .metadata()
        .map_err(io)?;
    if !unchanged(&before, &after)
        || !unchanged(&after, &current)
        || bytes.len() as u64 != after.len()
    {
        return Err(changed());
    }
    if !bytes.starts_with(b"%PDF-") {
        return Err(invalid("invalid_pdf", "The file is empty or is not a PDF"));
    }
    Ok(PdfRead {
        relative_path: relative.to_owned(),
        revision: blake3::hash(&bytes).to_hex().to_string(),
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reads_canonical_bytes_and_external_changes_without_an_index() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("lecture.PDF");
        std::fs::write(&path, b"%PDF-1.7\nfirst").unwrap();
        let first = read(root.path(), "lecture.PDF").unwrap();
        assert_eq!(first.bytes, b"%PDF-1.7\nfirst");
        std::fs::write(&path, b"%PDF-1.7\nsecond").unwrap();
        assert_ne!(
            first.revision,
            read(root.path(), "lecture.PDF").unwrap().revision
        );
    }
    #[test]
    fn rejects_invalid_large_and_escaping_files() {
        let root = tempfile::tempdir().unwrap();
        for value in [b"".as_slice(), b"not PDF"] {
            std::fs::write(root.path().join("bad.pdf"), value).unwrap();
            assert_eq!(
                read(root.path(), "bad.pdf").err().unwrap().code,
                "invalid_pdf"
            );
        }
        File::create(root.path().join("large.pdf"))
            .unwrap()
            .set_len(LIMIT + 1)
            .unwrap();
        assert_eq!(
            read(root.path(), "large.pdf").err().unwrap().code,
            "pdf_too_large"
        );
        assert!(read(root.path(), "../escape.pdf").is_err());
        assert!(read(root.path(), "/escape.pdf").is_err());
        assert!(read(root.path(), "missing.pdf").is_err());
        std::fs::create_dir(root.path().join("directory.pdf")).unwrap();
        assert!(read(root.path(), "directory.pdf").is_err());
    }
    #[test]
    fn detects_interrupted_read() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("file.pdf");
        std::fs::write(&path, b"%PDF-1.7\nfirst").unwrap();
        let result = read_with(root.path(), "file.pdf", || {
            std::fs::write(&path, b"%PDF-1.7\nchanged").unwrap();
        });
        let error = result.err().unwrap();
        assert_eq!(error.code, "pdf_changed");
        assert!(error.retryable);
    }
    #[cfg(unix)]
    #[test]
    fn rejects_file_and_parent_symlinks_and_non_utf8_links() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("file.pdf"), b"%PDF-1.7").unwrap();
        std::os::unix::fs::symlink(outside.path(), root.path().join("escape")).unwrap();
        std::os::unix::fs::symlink(
            outside.path().join("file.pdf"),
            root.path().join("file.pdf"),
        )
        .unwrap();
        assert!(read(root.path(), "escape/file.pdf").is_err());
        assert!(read(root.path(), "file.pdf").is_err());
        assert!(decode_target("%FF.pdf").is_err());
        assert_eq!(decode_target("my%20file.pdf").unwrap(), "my file.pdf");
    }
}
