use crate::{CoreError, ErrorCategory, Result};
use std::{
    fs::{File, Metadata},
    io::{Read, Seek, SeekFrom},
    path::Path,
};

const MAX_RANGE: u64 = 1024 * 1024;
const MAX_SAFE_LENGTH: u64 = 9_007_199_254_740_991;
const OP: &str = "files_pdf";

/// Cheap file-version descriptor. The version is an optimistic filesystem
/// stamp, not a content hash or permanent identity. No document bytes are cached.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PdfInfo {
    pub relative_path: String,
    pub workspace_id: String,
    pub version: String,
    #[ts(type = "number")]
    pub length: u64,
}

#[derive(Debug, serde::Deserialize, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct PdfRangeInput {
    pub relative_path: String,
    pub workspace_id: String,
    pub version: String,
    #[ts(type = "number")]
    pub offset: u64,
    #[ts(type = "number")]
    pub length: u64,
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

fn stamp(file: &File, metadata: &Metadata) -> Result<String> {
    #[cfg(unix)]
    let identity = {
        use std::os::unix::fs::MetadataExt;
        let _ = file;
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            metadata.dev(),
            metadata.ino(),
            metadata.len(),
            metadata.mtime(),
            metadata.mtime_nsec(),
            metadata.ctime(),
            metadata.ctime_nsec()
        )
    };
    #[cfg(windows)]
    let identity = {
        use std::os::windows::{fs::MetadataExt, io::AsRawHandle};
        use windows_sys::Win32::Storage::FileSystem::{
            BY_HANDLE_FILE_INFORMATION, FILE_BASIC_INFO, FileBasicInfo, GetFileInformationByHandle,
            GetFileInformationByHandleEx,
        };
        let mut info = std::mem::MaybeUninit::<BY_HANDLE_FILE_INFORMATION>::uninit();
        // SAFETY: file owns a valid handle; info is a correctly sized output buffer.
        if unsafe { GetFileInformationByHandle(file.as_raw_handle(), info.as_mut_ptr()) } == 0 {
            return Err(CoreError::io(std::io::Error::last_os_error(), OP, None));
        }
        // SAFETY: a successful call initialized the entire output structure.
        let info = unsafe { info.assume_init() };
        let mut basic = std::mem::MaybeUninit::<FILE_BASIC_INFO>::uninit();
        // SAFETY: the handle is live and the output buffer matches FileBasicInfo.
        if unsafe {
            GetFileInformationByHandleEx(
                file.as_raw_handle(),
                FileBasicInfo,
                basic.as_mut_ptr().cast(),
                std::mem::size_of::<FILE_BASIC_INFO>() as u32,
            )
        } == 0
        {
            return Err(CoreError::io(std::io::Error::last_os_error(), OP, None));
        }
        // SAFETY: the successful call initialized the output structure.
        let basic = unsafe { basic.assume_init() };
        format!(
            "{}:{}:{}:{}:{}:{}:{}",
            info.dwVolumeSerialNumber,
            info.nFileIndexHigh,
            info.nFileIndexLow,
            metadata.len(),
            metadata.creation_time(),
            metadata.last_write_time(),
            basic.ChangeTime
        )
    };
    Ok(blake3::hash(identity.as_bytes()).to_hex().to_string())
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

fn open_pdf(root: &Path, relative: &str) -> Result<File> {
    let path = crate::path::validate_relative(relative, OP)?;
    if !path
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    {
        return Err(invalid("invalid_pdf", "Choose a PDF file"));
    }
    crate::path::resolve_for_write(root, relative, OP)?;
    let file =
        open_scoped(root, &path).map_err(|error| CoreError::io(error, OP, Some(relative)))?;
    if !file
        .metadata()
        .map_err(|error| CoreError::io(error, OP, Some(relative)))?
        .is_file()
    {
        return Err(invalid("invalid_pdf", "The PDF must be a regular file"));
    }
    Ok(file)
}

fn verify(root: &Path, relative: &str, file: &File, expected: &str) -> Result<()> {
    let metadata = file.metadata().map_err(|_| changed())?;
    let current = open_pdf(root, relative).map_err(|_| changed())?;
    let current_metadata = current.metadata().map_err(|_| changed())?;
    if stamp(file, &metadata)? != expected || stamp(&current, &current_metadata)? != expected {
        return Err(changed());
    }
    Ok(())
}

pub(crate) fn inspect(root: &Path, workspace_id: &str, relative: &str) -> Result<PdfInfo> {
    let mut file = open_pdf(root, relative)?;
    let metadata = file
        .metadata()
        .map_err(|error| CoreError::io(error, OP, Some(relative)))?;
    if metadata.len() > MAX_SAFE_LENGTH {
        return Err(invalid(
            "pdf_length_unsupported",
            "This PDF exceeds the viewer's addressable file size",
        ));
    }
    let version = stamp(&file, &metadata)?;
    let mut header = [0; 5];
    let read = file.read_exact(&mut header);
    verify(root, relative, &file, &version)?;
    if metadata.len() < 5 || read.is_err() || &header != b"%PDF-" {
        return Err(invalid("invalid_pdf", "The file is empty or is not a PDF"));
    }
    Ok(PdfInfo {
        relative_path: relative.to_owned(),
        workspace_id: workspace_id.to_owned(),
        version,
        length: metadata.len(),
    })
}

pub(crate) fn read_range(
    root: &Path,
    workspace_id: &str,
    input: &PdfRangeInput,
) -> Result<Vec<u8>> {
    read_range_with(root, workspace_id, input, || {})
}

fn read_range_with(
    root: &Path,
    workspace_id: &str,
    input: &PdfRangeInput,
    after_read: impl FnOnce(),
) -> Result<Vec<u8>> {
    if input.workspace_id != workspace_id {
        return Err(changed());
    }
    if input.length == 0 || input.length > MAX_RANGE || input.offset > MAX_SAFE_LENGTH {
        return Err(invalid(
            "invalid_pdf_range",
            "A PDF read must request between 1 byte and 1 MiB",
        ));
    }
    let end = input
        .offset
        .checked_add(input.length)
        .ok_or_else(|| invalid("invalid_pdf_range", "Invalid PDF range"))?;
    let mut file = open_pdf(root, &input.relative_path)?;
    let metadata = file
        .metadata()
        .map_err(|error| CoreError::io(error, OP, Some(&input.relative_path)))?;
    if stamp(&file, &metadata)? != input.version {
        return Err(changed());
    }
    if end > metadata.len() {
        return Err(invalid(
            "invalid_pdf_range",
            "The PDF read exceeds the file length",
        ));
    }
    file.seek(SeekFrom::Start(input.offset))
        .map_err(|error| CoreError::io(error, OP, Some(&input.relative_path)))?;
    let mut bytes = vec![0; input.length as usize];
    file.read_exact(&mut bytes).map_err(|error| {
        if error.kind() == std::io::ErrorKind::UnexpectedEof {
            changed()
        } else {
            CoreError::io(error, OP, Some(&input.relative_path))
        }
    })?;
    after_read();
    verify(root, &input.relative_path, &file, &input.version)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn fixture(root: &Path) -> PdfInfo {
        std::fs::write(root.join("sample.pdf"), b"%PDF-1.7\nfixture").unwrap();
        inspect(root, "workspace", "sample.pdf").unwrap()
    }
    fn input(info: &PdfInfo, offset: u64, length: u64) -> PdfRangeInput {
        PdfRangeInput {
            relative_path: info.relative_path.clone(),
            workspace_id: info.workspace_id.clone(),
            version: info.version.clone(),
            offset,
            length,
        }
    }
    #[test]
    fn reads_ranges_from_large_sparse_files_without_a_document_size_cap() {
        let root = tempfile::tempdir().unwrap();
        let mut file = File::create(root.path().join("large.pdf")).unwrap();
        file.write_all(b"%PDF-1.7\n").unwrap();
        let size = 256 * 1024 * 1024;
        file.set_len(size).unwrap();
        file.seek(SeekFrom::Start(size - 5)).unwrap();
        file.write_all(b"%%EOF").unwrap();
        drop(file);
        let info = inspect(root.path(), "workspace", "large.pdf").unwrap();
        assert_eq!(info.length, size);
        assert_eq!(
            read_range(root.path(), "workspace", &input(&info, 0, 5)).unwrap(),
            b"%PDF-"
        );
        assert_eq!(
            read_range(root.path(), "workspace", &input(&info, size - 5, 5)).unwrap(),
            b"%%EOF"
        );
        assert_eq!(
            read_range(root.path(), "workspace", &input(&info, 1024, MAX_RANGE))
                .unwrap()
                .len(),
            MAX_RANGE as usize
        );
    }
    #[test]
    fn rejects_invalid_and_unbounded_ranges_and_wrong_workspace() {
        let root = tempfile::tempdir().unwrap();
        let info = fixture(root.path());
        for (offset, length) in [
            (0, 0),
            (0, MAX_RANGE + 1),
            (info.length, 1),
            (u64::MAX, 2),
            (MAX_SAFE_LENGTH + 1, 1),
        ] {
            assert_eq!(
                read_range(root.path(), "workspace", &input(&info, offset, length))
                    .unwrap_err()
                    .code,
                "invalid_pdf_range"
            );
        }
        let error = read_range(root.path(), "other", &input(&info, 0, 5)).unwrap_err();
        assert_eq!(error.code, "pdf_changed");
        assert!(error.retryable);
    }
    #[test]
    fn detects_external_edits_before_and_during_range_reads() {
        let root = tempfile::tempdir().unwrap();
        let info = fixture(root.path());
        let error = read_range_with(root.path(), "workspace", &input(&info, 0, 5), || {
            std::fs::write(root.path().join("sample.pdf"), b"%PDF-1.7\nchanged").unwrap();
        })
        .unwrap_err();
        assert_eq!(error.code, "pdf_changed");
        assert!(error.retryable);
        assert_eq!(
            read_range(root.path(), "workspace", &input(&info, 0, 5))
                .unwrap_err()
                .code,
            "pdf_changed"
        );
        let current = inspect(root.path(), "workspace", "sample.pdf").unwrap();
        assert_ne!(current.version, info.version);
        assert!(read_range(root.path(), "workspace", &input(&current, 0, 5)).is_ok());
    }
    #[test]
    fn detects_same_size_replacement_and_deletion() {
        let root = tempfile::tempdir().unwrap();
        let info = fixture(root.path());
        std::fs::write(root.path().join("replacement.pdf"), b"%PDF-1.7\nfixture").unwrap();
        std::fs::rename(
            root.path().join("replacement.pdf"),
            root.path().join("sample.pdf"),
        )
        .unwrap();
        assert_eq!(
            read_range(root.path(), "workspace", &input(&info, 0, 5))
                .unwrap_err()
                .code,
            "pdf_changed"
        );
        std::fs::remove_file(root.path().join("sample.pdf")).unwrap();
        assert!(inspect(root.path(), "workspace", "sample.pdf").is_err());
        assert!(read_range(root.path(), "workspace", &input(&info, 0, 5)).is_err());
    }
    #[test]
    fn rejects_malformed_files_unsafe_paths_and_directories() {
        let root = tempfile::tempdir().unwrap();
        fixture(root.path());
        for bytes in [b"".as_slice(), b"text", b"not a PDF file"] {
            std::fs::write(root.path().join("invalid.pdf"), bytes).unwrap();
            assert_eq!(
                inspect(root.path(), "workspace", "invalid.pdf")
                    .unwrap_err()
                    .code,
                "invalid_pdf"
            );
        }
        std::fs::create_dir(root.path().join("directory.pdf")).unwrap();
        for path in [
            "../sample.pdf",
            "/sample.pdf",
            "a/../../sample.pdf",
            "sample.pdf\0",
            "directory.pdf",
            "sample.txt",
        ] {
            assert!(inspect(root.path(), "workspace", path).is_err(), "{path}");
        }
        assert!(decode_target("%FF.pdf").is_err());
        assert!(decode_target("%00.pdf").is_err());
        assert!(decode_target("%ZZ.pdf").is_err());
        assert_eq!(
            decode_target("%C3%A9tude%20notes.pdf").unwrap(),
            "étude notes.pdf"
        );
    }
    #[cfg(unix)]
    #[test]
    fn rejects_symlinks_at_every_component_and_replacement() {
        use std::os::unix::fs::symlink;
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let info = fixture(root.path());
        fixture(outside.path());
        symlink(root.path().join("sample.pdf"), root.path().join("link.pdf")).unwrap();
        symlink(outside.path(), root.path().join("linked")).unwrap();
        for path in ["link.pdf", "linked/sample.pdf"] {
            assert!(inspect(root.path(), "workspace", path).is_err());
        }
        let error = read_range_with(root.path(), "workspace", &input(&info, 0, 5), || {
            std::fs::remove_file(root.path().join("sample.pdf")).unwrap();
            symlink(
                outside.path().join("sample.pdf"),
                root.path().join("sample.pdf"),
            )
            .unwrap();
        })
        .unwrap_err();
        assert_eq!(error.code, "pdf_changed");
    }
}
