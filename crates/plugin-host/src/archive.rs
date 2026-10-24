//! A plugin's release archive: the file a plugin author attaches to a
//! GitHub release, which Noura downloads to install the plugin.
//!
//! The archive is a zip file whose name ends in [`ARCHIVE_SUFFIX`]. It
//! holds `plugin.toml` at its top, and the entry component at the path the
//! manifest's `entry` names. Entries are stored or deflated. The host reads
//! those two files and nothing else, so other files, such as a README or a
//! license, may come along and are never unpacked.
//!
//! The host refuses the whole archive when any entry's path is absolute,
//! has a `.` or `..` part, a backslash, a drive or a control character,
//! when an entry is a symbolic link or encrypted, when two entries share a
//! name, or when the central directory and the entries' own headers
//! disagree. Nothing is written to disk while it reads.

use std::{
    collections::HashSet,
    io::{Cursor, Read},
};

use crate::{
    InstallError, InstallErrorCode, MANIFEST_FILE, PluginManifest,
    manifest::{MAX_MANIFEST_BYTES, stays_inside},
};

/// How a release archive's file name ends.
pub const ARCHIVE_SUFFIX: &str = ".noura-plugin.zip";

/// The most entries an archive may hold.
const MAX_ENTRIES: usize = 1024;

/// The largest entry component the host installs.
pub(crate) const MAX_COMPONENT_BYTES: u64 = 64 << 20;

/// A release archive, read and checked, whose component nothing compiled
/// yet.
#[derive(Clone)]
pub struct Archive {
    pub(crate) manifest: PluginManifest,
    pub(crate) manifest_bytes: Vec<u8>,
    pub(crate) component_bytes: Vec<u8>,
}

impl std::fmt::Debug for Archive {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("Archive")
            .field("manifest", &self.manifest)
            .finish_non_exhaustive()
    }
}

impl Archive {
    /// Read the archive `bytes`: check every entry, then take the manifest
    /// and the entry component it names.
    pub fn read(bytes: &[u8]) -> Result<Self, InstallError> {
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|error| {
            tracing::warn!(%error, "a plugin archive isn't a zip archive");
            invalid("The download isn't a zip archive.")
        })?;
        if zip.len() > MAX_ENTRIES {
            return Err(invalid(format!(
                "The archive holds more than {MAX_ENTRIES} files."
            )));
        }
        check_entries(&mut zip)?;
        let manifest_bytes = read_entry(&mut zip, MANIFEST_FILE, MAX_MANIFEST_BYTES)?
            .ok_or_else(|| {
                InstallError::new(
                    InstallErrorCode::ManifestMissing,
                    format!("The archive has no {MANIFEST_FILE} at its top, so it isn't a plugin."),
                )
            })?;
        let text = std::str::from_utf8(&manifest_bytes).map_err(|_| {
            InstallError::new(
                InstallErrorCode::InvalidManifest,
                format!("{MANIFEST_FILE} isn't UTF-8 text."),
            )
        })?;
        let manifest = PluginManifest::parse(text)?;
        let component_bytes = read_entry(&mut zip, &manifest.entry, MAX_COMPONENT_BYTES)?
            .ok_or_else(|| {
                InstallError::new(
                    InstallErrorCode::EntryMissing,
                    format!("The archive has no {}.", manifest.entry),
                )
            })?;
        Ok(Self {
            manifest,
            manifest_bytes,
            component_bytes,
        })
    }

    pub fn manifest(&self) -> &PluginManifest {
        &self.manifest
    }
}

/// Refuse an archive with an entry that leaves the plugin's folder, a link,
/// an encrypted entry, two entries of one name, or headers that disagree.
fn check_entries(zip: &mut zip::ZipArchive<Cursor<&[u8]>>) -> Result<(), InstallError> {
    let mut names = HashSet::new();
    for index in 0..zip.len() {
        let entry = zip.by_index_raw(index).map_err(unreadable)?;
        let name = entry.name().map_err(unreadable)?.into_owned();
        if entry.is_symlink() {
            return Err(InstallError::new(
                InstallErrorCode::ArchiveLink,
                format!(
                    "The archive holds a symbolic link, {}. Plugins can't hold links.",
                    shown(&name)
                ),
            ));
        }
        let path = name.strip_suffix('/').unwrap_or(&name);
        if !stays_inside(path) || entry.enclosed_name().is_none() {
            return Err(InstallError::new(
                InstallErrorCode::ArchiveEntryOutside,
                format!(
                    "The archive holds {}, whose path leaves the plugin's folder.",
                    shown(&name)
                ),
            ));
        }
        if entry.encrypted() {
            return Err(invalid(format!(
                "The archive's {} is encrypted.",
                shown(&name)
            )));
        }
        if !names.insert(path.to_owned()) {
            return Err(invalid(format!(
                "The archive holds {} twice.",
                shown(&name)
            )));
        }
    }
    zip.validate_header_consistency().map_err(unreadable)
}

/// The bytes of the file `name`, when the archive holds it, refused past
/// `limit` bytes whatever size the archive claims.
fn read_entry(
    zip: &mut zip::ZipArchive<Cursor<&[u8]>>,
    name: &str,
    limit: u64,
) -> Result<Option<Vec<u8>>, InstallError> {
    let Some(index) = zip.index_for_name(name) else {
        return Ok(None);
    };
    let entry = zip.by_index(index).map_err(unreadable)?;
    if entry.is_dir() {
        return Ok(None);
    }
    let too_large = || {
        invalid(format!(
            "The archive's {name} is larger than {} MB.",
            limit.div_ceil(1 << 20)
        ))
    };
    if entry.size() > limit {
        return Err(too_large());
    }
    let mut bytes = Vec::new();
    entry
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| {
            tracing::warn!(%error, "a plugin archive's entry didn't read");
            invalid(format!("Noura can't read the archive's {name}."))
        })?;
    if bytes.len() as u64 > limit {
        return Err(too_large());
    }
    Ok(Some(bytes))
}

fn invalid(message: impl Into<String>) -> InstallError {
    InstallError::new(InstallErrorCode::InvalidArchive, message)
}

fn unreadable(error: zip::result::ZipError) -> InstallError {
    tracing::warn!(%error, "a plugin archive can't be read");
    invalid("The archive is damaged or uses a format Noura can't read.")
}

/// A name from the archive, quoted and with control characters left out,
/// for a message.
fn shown(name: &str) -> String {
    let name: String = name.chars().filter(|c| !c.is_control()).take(200).collect();
    format!("\u{201c}{name}\u{201d}")
}
