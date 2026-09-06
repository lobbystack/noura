//! Atomic persistence for device-local settings, separate from workspace CAS writes.

use std::{io, io::Write, path::Path};

pub(crate) fn write(path: &Path, bytes: &[u8]) -> io::Result<()> {
    #[cfg(windows)]
    {
        // Opening a directory with File::open fails on Windows. Use the mature
        // implementation's write-through replacement after flushing the file.
        atomicwrites::AtomicFile::new(path, atomicwrites::AllowOverwrite)
            .write(|file| {
                file.write_all(bytes)?;
                file.sync_all()
            })
            .map_err(|error| match error {
                atomicwrites::Error::Internal(error) | atomicwrites::Error::User(error) => error,
            })
    }
    #[cfg(not(windows))]
    {
        let mut file = atomic_write_file::AtomicWriteFile::open(path)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        file.commit()?;
        if let Some(parent) = path.parent() {
            std::fs::File::open(parent)?.sync_all()?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_can_be_created_replaced_and_reopened() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        write(&path, br#"{"version":1}"#).unwrap();
        write(&path, br#"{"version":2}"#).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), br#"{"version":2}"#);
    }

    #[test]
    fn invalid_destination_fails_without_changing_existing_settings() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("settings.json");
        write(&path, b"existing").unwrap();
        assert!(write(&path.join("child"), b"replacement").is_err());
        assert_eq!(std::fs::read(path).unwrap(), b"existing");
    }
}
