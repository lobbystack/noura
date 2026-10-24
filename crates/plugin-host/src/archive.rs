//! A plugin's release archive: a zip file holding `plugin.toml` at its
//! root and the component at the path the manifest's `entry` names, as the
//! TypeScript author kit's package command writes it. Other files may sit
//! beside them, such as a README, and stay unread.
//!
//! The host reads only those two files, each up to its size limit, and
//! refuses an archive that holds a link, an entry whose path leaves the
//! plugin's folder, or two entries with one path, before it reads either.

use std::collections::HashSet;
use std::io::{Cursor, Read};
use std::path::Path;

use zip::ZipArchive;
use zip::result::ZipError;

use crate::manifest::{MAX_MANIFEST_BYTES, manifest_too_large, stays_inside};
use crate::{InstallError, InstallErrorCode, MANIFEST_FILE};

/// The largest archive the host reads: a component at its limit, a
/// manifest and room for the zip format's own records.
const MAX_ARCHIVE_BYTES: u64 = 80 << 20;

/// An archive whose entries passed the checks.
pub(crate) struct Archive {
    zip: ZipArchive<Cursor<Vec<u8>>>,
}

fn invalid(message: String) -> InstallError {
    InstallError::new(InstallErrorCode::InvalidArchive, message)
}

impl Archive {
    /// Read the archive at `path` and check every entry's path.
    pub(crate) fn open(path: &Path) -> Result<Self, InstallError> {
        let size = std::fs::metadata(path)
            .map_err(|error| InstallError::storage("read the archive", &error))?
            .len();
        if size > MAX_ARCHIVE_BYTES {
            return Err(invalid("The archive is larger than 80 MB.".to_owned()));
        }
        let bytes = std::fs::read(path)
            .map_err(|error| InstallError::storage("read the archive", &error))?;
        let mut zip = ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
            tracing::warn!(%error, "a plugin archive didn't read");
            invalid("The file isn't a zip archive that Noura can read.".to_owned())
        })?;
        let mut names = HashSet::new();
        for index in 0..zip.len() {
            let entry = zip.by_index_raw(index).map_err(|error| {
                tracing::warn!(%error, "a plugin archive's entry didn't read");
                invalid("The archive is damaged.".to_owned())
            })?;
            let name = entry
                .name()
                .map_err(|_| invalid("The archive holds a name Noura can't read.".to_owned()))?
                .into_owned();
            if entry.is_symlink() {
                return Err(invalid(format!(
                    "The archive holds a link, {name}. A plugin's archive holds only files."
                )));
            }
            let path = name.strip_suffix('/').unwrap_or(&name);
            if entry.enclosed_name().is_none() || !stays_inside(path) {
                return Err(invalid(format!(
                    "The archive holds {name}, which isn't inside the plugin's folder."
                )));
            }
            if !names.insert(path.to_owned()) {
                return Err(invalid(format!("The archive holds {name} twice.")));
            }
        }
        Ok(Self { zip })
    }

    /// The manifest's bytes.
    pub(crate) fn manifest(&mut self) -> Result<Vec<u8>, InstallError> {
        self.file(MANIFEST_FILE, MAX_MANIFEST_BYTES, manifest_too_large)?
            .ok_or_else(|| {
                InstallError::new(
                    InstallErrorCode::ManifestMissing,
                    format!(
                        "The archive has no {MANIFEST_FILE} at its root, so it isn't a plugin."
                    ),
                )
            })
    }

    /// The bytes of the entry component `entry`, up to `limit` bytes.
    pub(crate) fn component(&mut self, entry: &str, limit: u64) -> Result<Vec<u8>, InstallError> {
        let too_large = || {
            InstallError::new(
                InstallErrorCode::NotAComponent,
                format!("{entry} is larger than {} MB.", limit >> 20),
            )
        };
        self.file(entry, limit, too_large)?.ok_or_else(|| {
            InstallError::new(
                InstallErrorCode::EntryMissing,
                format!("The archive has no {entry}, the component plugin.toml names."),
            )
        })
    }

    /// The bytes of the file `name`, or `None` when the archive holds no
    /// such file. A file larger than `limit` stops the read with
    /// `too_large`, whatever its header claims.
    fn file(
        &mut self,
        name: &str,
        limit: u64,
        too_large: impl FnOnce() -> InstallError,
    ) -> Result<Option<Vec<u8>>, InstallError> {
        let damaged = |error: &dyn std::fmt::Display| {
            tracing::warn!(%error, "a plugin archive's file didn't read");
            invalid(format!("The archive's {name} is damaged."))
        };
        let mut file = match self.zip.by_name(name) {
            Ok(file) if file.is_file() => file,
            Ok(_) | Err(ZipError::FileNotFound) => return Ok(None),
            Err(error) => return Err(damaged(&error)),
        };
        if file.size() > limit {
            return Err(too_large());
        }
        let mut bytes = Vec::new();
        file.by_ref()
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| damaged(&error))?;
        if bytes.len() as u64 > limit {
            return Err(too_large());
        }
        Ok(Some(bytes))
    }
}
