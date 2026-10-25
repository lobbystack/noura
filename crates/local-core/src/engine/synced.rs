//! The files the v2 sync client carries (docs/workspace-format/sync-v2.md
//! §9, §15): one walk that lists every file the workspace scanner reports,
//! with what sync needs to notice a change or a move without reading it,
//! and the entries sync leaves out, and the writes that apply another
//! device's changes. Each write checks that the file still holds the bytes
//! the caller expects, so an outside edit that lands meanwhile is never
//! overwritten, and is durable before it returns.

use super::*;

/// The operation name the writes report.
const OPERATION: &str = "sync_apply";

/// A file the workspace scanner reports, as sync sees it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SyncedFile {
    /// The workspace-relative path, with `/` between components.
    pub relative_path: String,
    /// The size in bytes.
    pub size: u64,
    /// The modification time in nanoseconds since the Unix epoch.
    pub modified_ns: i64,
    /// The operating system's file ID where the platform reports one.
    pub file_id: Option<u64>,
}

/// Why sync leaves a workspace entry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkipReason {
    /// A symbolic link, which sync never follows.
    Symlink,
    /// A name with no UTF-8 form; the entry names the folder that holds it.
    UnsupportedName,
    /// The file or folder can't be read.
    Unreadable,
    /// The file's content lives only in the cloud for now.
    NotDownloaded,
}

/// An entry sync leaves out, and why.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SkippedEntry {
    /// The workspace-relative path, or the folder that holds an entry whose
    /// name has no UTF-8 form, or `None` at the workspace's top.
    pub relative_path: Option<String>,
    /// Why.
    pub reason: SkipReason,
}

/// What one walk found for sync.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncedListing {
    /// Every file sync may carry, sorted by path.
    pub files: Vec<SyncedFile>,
    /// The entries it leaves out.
    pub skipped: Vec<SkippedEntry>,
    /// Whether the walk saw the whole workspace. When it didn't, a file
    /// missing from `files` may still exist, so sync must not read it as
    /// deleted.
    pub complete: bool,
}

/// What a sync write expects to find where it writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SyncedExpect {
    /// Nothing at the path.
    Absent,
    /// A file whose bytes have this revision.
    Revision(String),
}

#[cfg(unix)]
fn file_id(metadata: &std::fs::Metadata) -> Option<u64> {
    use std::os::unix::fs::MetadataExt;
    Some(metadata.ino())
}

#[cfg(not(unix))]
fn file_id(_metadata: &std::fs::Metadata) -> Option<u64> {
    None
}

impl WorkspaceEngine {
    /// Lists every file the workspace scanner reports, with its size,
    /// modification time and file ID, and the entries sync leaves out:
    /// symbolic links, names with no UTF-8 form, entries it can't read and
    /// files that live only in the cloud. Ignore patterns, dot entries and
    /// empty folders never show up, as in the scanner.
    pub fn synced_listing(&self) -> Result<SyncedListing> {
        let _changes = self.hold_changes();
        let mut listing = SyncedListing {
            complete: true,
            ..SyncedListing::default()
        };
        for entry in index_walker(&self.root, &self.current_ignore())? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    let mut scan = WorkspaceScan::default();
                    record_walk_error(&self.root, &error, &mut scan);
                    listing.complete &= !scan.incomplete;
                    listing.skipped.push(SkippedEntry {
                        relative_path: scan.unreadable_folders.pop().or_else(|| {
                            scan.diagnostics
                                .iter()
                                .find_map(|d| d.relative_path.clone())
                        }),
                        reason: SkipReason::Unreadable,
                    });
                    continue;
                }
            };
            if entry.path() == self.root {
                continue;
            }
            let Some(kind) = entry.file_type() else {
                continue;
            };
            let Ok(relative) = normalized_relative_path(&self.root, entry.path(), "sync_list")
            else {
                let folder = entry
                    .path()
                    .parent()
                    .and_then(|parent| {
                        normalized_relative_path(&self.root, parent, "sync_list").ok()
                    })
                    .filter(|folder| !folder.is_empty());
                listing.skipped.push(SkippedEntry {
                    relative_path: folder,
                    reason: SkipReason::UnsupportedName,
                });
                continue;
            };
            if kind.is_symlink() {
                listing.skipped.push(SkippedEntry {
                    relative_path: Some(relative),
                    reason: SkipReason::Symlink,
                });
                continue;
            }
            if !kind.is_file() {
                continue;
            }
            let name = entry.file_name().to_string_lossy();
            if let Some(real) = placeholder_target(&name) {
                let real = match relative.rsplit_once('/') {
                    Some((parent, _)) => format!("{parent}/{real}"),
                    None => real.to_owned(),
                };
                listing.skipped.push(SkippedEntry {
                    relative_path: Some(real),
                    reason: SkipReason::NotDownloaded,
                });
                continue;
            }
            match entry.metadata() {
                Ok(metadata) if is_dataless(&metadata) => listing.skipped.push(SkippedEntry {
                    relative_path: Some(relative),
                    reason: SkipReason::NotDownloaded,
                }),
                Ok(metadata) => listing.files.push(SyncedFile {
                    relative_path: relative,
                    size: metadata.len(),
                    modified_ns: modified_ns(&metadata),
                    file_id: file_id(&metadata),
                }),
                Err(_) => listing.skipped.push(SkippedEntry {
                    relative_path: Some(relative),
                    reason: SkipReason::Unreadable,
                }),
            }
        }
        listing
            .files
            .sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
        Ok(listing)
    }

    /// The bytes of the file at `relative`, or `None` when nothing is
    /// there. A path that leaves the workspace or crosses a link fails, and
    /// a file that lives only in the cloud fails with `file_not_downloaded`.
    pub fn read_synced(&self, relative: &str) -> Result<Option<Vec<u8>>> {
        let path = resolve_for_write(&self.root, relative, OPERATION)?;
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {
                read_downloaded(&path, relative, OPERATION).map(Some)
            }
            Ok(_) => Ok(None),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ensure_downloaded(&path, relative, OPERATION)?;
                Ok(None)
            }
            Err(error) => Err(CoreError::io(error, OPERATION, Some(relative))),
        }
    }

    /// Writes another device's `bytes` to `relative` when the file there is
    /// still as `expected`, else fails with `revision_conflict` and leaves it
    /// alone. The bytes and the folders they land in are on disk before this
    /// returns, the index follows, and the views hear of the change.
    pub fn write_synced(
        &self,
        relative: &str,
        bytes: &[u8],
        expected: &SyncedExpect,
    ) -> Result<()> {
        let path = resolve_for_write(&self.root, relative, OPERATION)?;
        let _changes = self.hold_changes();
        let guard = self.write_lock(OPERATION)?;
        let revision = markdown::revision(bytes);
        self.expect_own_write(relative, Some(&revision));
        let written = match expected {
            SyncedExpect::Revision(expected) => atomic_write_checked(
                &self.root,
                Path::new(relative),
                bytes,
                Some(expected),
                OPERATION,
            ),
            SyncedExpect::Absent => write_new(&path, relative, bytes),
        };
        if let Err(error) = written {
            self.forget_own_write(relative);
            return Err(error);
        }
        drop(guard);
        self.follow_sync_change(&[relative]);
        Ok(())
    }

    /// Moves the file at `from`, whose bytes must still have
    /// `expected_revision`, to `to`, where nothing may be, except the file
    /// itself after a case-only rename. Folders the move empties go too.
    /// The move is durable before this returns.
    pub fn move_synced(&self, from: &str, to: &str, expected_revision: &str) -> Result<()> {
        let source = resolve_for_write(&self.root, from, OPERATION)?;
        let destination = resolve_for_write(&self.root, to, OPERATION)?;
        let _changes = self.hold_changes();
        let guard = self.write_lock(OPERATION)?;
        let current = read_downloaded(&source, from, OPERATION)?;
        check_revision(&current, expected_revision, OPERATION)?;
        if std::fs::symlink_metadata(&destination).is_ok() && !same_file(&source, &destination) {
            return Err(CoreError::new(
                "path_exists",
                ErrorCategory::Conflict,
                "A file already exists at the destination",
                OPERATION,
            ));
        }
        self.move_item(&source, &destination, OPERATION)?;
        self.remove_emptied_folders(&source);
        drop(guard);
        self.follow_sync_change(&[from, to]);
        Ok(())
    }

    /// Moves the file at `relative` to the trash when its bytes still have
    /// `expected_revision`, and removes the folders that leaves empty. The
    /// removal is durable before this returns; the trash keeps the bytes.
    pub fn delete_synced(&self, relative: &str, expected_revision: &str) -> Result<()> {
        let source = resolve_for_write(&self.root, relative, OPERATION)?;
        let _changes = self.hold_changes();
        let guard = self.write_lock(OPERATION)?;
        let current = read_downloaded(&source, relative, OPERATION)?;
        check_revision(&current, expected_revision, OPERATION)?;
        let noted = self.expect_own_move(relative, None);
        if let Err(error) = self.discard(&source, relative, OPERATION) {
            if std::fs::symlink_metadata(&source).is_ok() {
                self.forget_own_writes(&noted);
            }
            return Err(error);
        }
        self.remove_emptied_folders(&source);
        drop(guard);
        self.follow_sync_change(&[relative]);
        Ok(())
    }

    /// Removes each folder above `left`, which a sync move or delete just
    /// took away, that the change left empty, up to the workspace's top.
    fn remove_emptied_folders(&self, left: &Path) {
        let mut folder = left.parent();
        while let Some(current) = folder {
            if current == self.root || !current.starts_with(&self.root) {
                break;
            }
            if std::fs::remove_dir(current).is_err() {
                break;
            }
            let _ = sync_parent(current, OPERATION);
            folder = current.parent();
        }
    }

    /// Brings the index up to date with a sync change at `paths` and tells
    /// the views, which list the workspace again. An index that can't follow
    /// catches up on the next read.
    fn follow_sync_change(&self, paths: &[&str]) {
        let markdown: Vec<String> = paths
            .iter()
            .filter(|path| is_markdown_path(path))
            .map(|path| (*path).to_owned())
            .collect();
        if let Err(error) = self.reconcile_paths(&markdown, EventSource::Sync) {
            tracing::warn!(code = %error.code, "the index couldn't follow a synced change");
            self.dirty.store(true, Ordering::SeqCst);
        }
        self.emit(
            "file:changed",
            EventSource::Sync,
            serde_json::json!({ "paths": paths }),
        );
    }
}

/// Writes `bytes` to a new file at `path` through a temporary file, once
/// nothing is there, then syncs the file and its folder.
fn write_new(path: &Path, relative: &str, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        create_folders(parent, OPERATION)?;
    }
    let mut file = atomic_write_file::AtomicWriteFile::open(path)
        .map_err(|error| CoreError::io(error, OPERATION, Some(relative)))?;
    file.write_all(bytes)
        .and_then(|()| file.sync_all())
        .map_err(|error| CoreError::io(error, OPERATION, Some(relative)))?;
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(CoreError::new(
            "revision_conflict",
            ErrorCategory::Conflict,
            "A file appeared at this path since sync looked",
            OPERATION,
        ));
    }
    file.commit()
        .map_err(|error| CoreError::io(error, OPERATION, Some(relative)))?;
    sync_parent(path, OPERATION)
}
