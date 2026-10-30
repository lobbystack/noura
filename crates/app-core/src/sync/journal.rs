//! The client journal (docs/workspace-format/sync-v2.md §15): this device's
//! sync state for one folder, in ordinary files under
//! `.noura/sync/<device-id>/`, never only in SQLite and never uploaded.
//!
//! Each journal file carries a BLAKE3 keyed hash under the journal key the
//! credential store keeps, over the file's name and its bytes, so a file
//! that another program changed, or one copied in from another journal
//! file, fails. The client takes a lock on its own journal while the app
//! runs and ignores a journal named for another device. The review store
//! of conflict branches and held local versions sits beside it, as plain
//! copies of the files, which the journal names with their revisions.

use std::io::Write;
use std::path::{Path, PathBuf};

use local_core::sync::SyncCredentials;
use serde::Serialize;
use serde::de::DeserializeOwned;
use sync_protocol::crypto::keyed_hash;
use sync_protocol::types::{Digest, Id, SecretKey};

use super::SyncError;
use super::account_heads::journal_key;

/// The folder every device's journal lives in, under the workspace root.
pub const JOURNALS: &str = ".noura/sync";
/// The journal's state file.
pub const STATE_FILE: &str = "journal.json";
/// The folder of conflict branches inside a journal (§16).
pub const CONFLICTS: &str = "conflicts";
/// The folder of held local versions inside a journal (§15).
pub const HELD: &str = "held";
/// The folder of downloaded attachments inside a journal, which sync
/// checks there before it writes them into the workspace (§21).
pub const DOWNLOADS: &str = "downloads";
/// The folder an earlier journal's review store moves to when you connect a
/// folder again (§15).
pub const ARCHIVE: &str = "archive";
const LOCK_FILE: &str = "lock";
/// Binds a journal file's keyed hash to journal files.
const MAC_CONTEXT: &[u8] = b"noura.sync.v2 journal file\0";

/// Why a journal can't be read.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// A journal file's keyed hash fails, or it doesn't parse: the client
    /// refuses it and syncs the folder as after a lost journal.
    #[error("a journal file failed its check")]
    Refused,
    /// Another copy of the app holds this journal.
    #[error("another Noura window or app syncs this folder")]
    Locked,
    /// The disk failed.
    #[error("the journal can't be read or written: {0}")]
    Io(String),
}

impl From<JournalError> for SyncError {
    fn from(error: JournalError) -> Self {
        match error {
            JournalError::Refused => SyncError::Check("sync_journal_rollback"),
            JournalError::Locked => SyncError::Invalid(error.to_string()),
            JournalError::Io(_) => SyncError::Invalid(error.to_string()),
        }
    }
}

fn io(error: impl std::fmt::Display) -> JournalError {
    JournalError::Io(error.to_string())
}

/// The folder of `device`'s journal in the workspace at `root`.
pub fn folder(root: &Path, device: &Id) -> PathBuf {
    root.join(JOURNALS).join(device.to_hex())
}

/// The devices whose journals sit in the workspace at `root`, other than
/// `device`: the client ignores them (§15).
pub fn other_journals(root: &Path, device: &Id) -> Vec<String> {
    let own = device.to_hex();
    let Ok(entries) = std::fs::read_dir(root.join(JOURNALS)) else {
        return Vec::new();
    };
    let mut found: Vec<String> = entries
        .flatten()
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().to_str().map(str::to_owned))
        .filter(|name| name.len() == 32 && Id::from_hex(name).is_some() && *name != own)
        .collect();
    found.sort();
    found
}

/// One device's journal of one folder, locked while it's open.
pub struct Journal {
    dir: PathBuf,
    key: SecretKey,
    _lock: std::fs::File,
}

impl std::fmt::Debug for Journal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Journal")
            .field("dir", &self.dir)
            .finish_non_exhaustive()
    }
}

impl Journal {
    /// Opens `device`'s journal in the workspace at `root`, creating its
    /// folder, and takes its lock. Another holder of the lock, such as a
    /// second copy of the app, gets [`JournalError::Locked`].
    pub fn open(
        root: &Path,
        device: &Id,
        credentials: &dyn SyncCredentials,
    ) -> Result<Self, SyncError> {
        let key = journal_key(credentials, device)?;
        let dir = folder(root, device);
        std::fs::create_dir_all(&dir).map_err(io)?;
        let lock = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.join(LOCK_FILE))
            .map_err(io)?;
        match lock.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => return Err(JournalError::Locked.into()),
            Err(std::fs::TryLockError::Error(error)) => return Err(io(error).into()),
        }
        Ok(Self {
            dir,
            key,
            _lock: lock,
        })
    }

    /// The journal's folder.
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Whether the journal holds a state file.
    pub fn exists(&self) -> bool {
        self.dir.join(STATE_FILE).exists()
    }

    /// Reads the journal file `name`, or `None` when there is none. A file
    /// whose keyed hash fails, or that doesn't parse, is refused.
    pub fn read<T: DeserializeOwned>(&self, name: &str) -> Result<Option<T>, JournalError> {
        let bytes = match std::fs::read(self.dir.join(name)) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(io(error)),
        };
        let split = bytes
            .iter()
            .position(|b| *b == b'\n')
            .ok_or(JournalError::Refused)?;
        let (mac, json) = (&bytes[..split], &bytes[split + 1..]);
        let mac = std::str::from_utf8(mac)
            .ok()
            .and_then(Digest::from_hex)
            .ok_or(JournalError::Refused)?;
        if self.mac(name, json) != mac {
            return Err(JournalError::Refused);
        }
        serde_json::from_slice(json)
            .map(Some)
            .map_err(|_| JournalError::Refused)
    }

    /// Writes the journal file `name`, with its keyed hash, durably: the
    /// bytes and the folder entry are on disk before this returns.
    pub fn write<T: Serialize>(&self, name: &str, value: &T) -> Result<(), JournalError> {
        let json = serde_json::to_vec(value).map_err(io)?;
        let mut bytes = self.mac(name, &json).to_hex().into_bytes();
        bytes.push(b'\n');
        bytes.extend_from_slice(&json);
        durable_write(&self.dir.join(name), &bytes)
    }

    fn mac(&self, name: &str, json: &[u8]) -> Digest {
        let mut input = Vec::with_capacity(MAC_CONTEXT.len() + name.len() + 1 + json.len());
        input.extend_from_slice(MAC_CONTEXT);
        input.extend_from_slice(name.as_bytes());
        input.push(0);
        input.extend_from_slice(json);
        keyed_hash(&self.key, &input)
    }

    /// Keeps `bytes` in the review store at `relative`, such as
    /// `conflicts/<rid>/<digest>`, as a plain copy of the file.
    pub fn keep(&self, relative: &str, bytes: &[u8]) -> Result<(), JournalError> {
        let path = self.dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        durable_write(&path, bytes)
    }

    /// The bytes the review store keeps at `relative`, when they still have
    /// `revision`; a copy another program changed is refused.
    pub fn kept(&self, relative: &str, revision: &Digest) -> Result<Vec<u8>, JournalError> {
        let bytes = std::fs::read(self.dir.join(relative)).map_err(io)?;
        if sync_protocol::crypto::hash(&bytes) != *revision {
            return Err(JournalError::Refused);
        }
        Ok(bytes)
    }

    /// Keeps the bytes `write` produces at `relative` in the journal folder,
    /// through a temporary file that is on disk, with its folder entry,
    /// before this returns, so a large file streams through without sitting
    /// in memory.
    pub fn keep_with<T, E: From<JournalError>>(
        &self,
        relative: &str,
        write: impl FnOnce(&mut dyn Write) -> Result<T, E>,
    ) -> Result<T, E> {
        let path = self.dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let mut file = atomic_write_file::AtomicWriteFile::open(&path).map_err(io)?;
        let value = {
            let mut buffered = std::io::BufWriter::with_capacity(1 << 16, &mut file);
            let value = write(&mut buffered)?;
            buffered.flush().map_err(io)?;
            value
        };
        file.sync_all().map_err(io)?;
        file.commit().map_err(io)?;
        sync_folder(&path)?;
        Ok(value)
    }

    /// Keeps a copy of the file at `source` in the review store at
    /// `relative`, streaming it, when its bytes hash to `revision`; a file
    /// that changed meanwhile is refused and nothing is kept.
    pub fn keep_file(
        &self,
        relative: &str,
        source: &Path,
        revision: &Digest,
    ) -> Result<(), JournalError> {
        let path = self.dir.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        let mut reader = std::fs::File::open(source).map_err(io)?;
        let mut file = atomic_write_file::AtomicWriteFile::open(&path).map_err(io)?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; 1 << 16];
        loop {
            let n = std::io::Read::read(&mut reader, &mut buffer).map_err(io)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
            file.write_all(&buffer[..n]).map_err(io)?;
        }
        if Digest(*hasher.finalize().as_bytes()) != *revision {
            return Err(JournalError::Refused);
        }
        file.sync_all().map_err(io)?;
        file.commit().map_err(io)?;
        sync_folder(&path)
    }

    /// The path of the review store's copy at `relative`, when it still has
    /// `revision`, hashed as it streams; a copy another program changed is
    /// refused.
    pub fn kept_path(&self, relative: &str, revision: &Digest) -> Result<PathBuf, JournalError> {
        let path = self.dir.join(relative);
        if hash_file(&path).map_err(io)? != *revision {
            return Err(JournalError::Refused);
        }
        Ok(path)
    }

    /// Removes the review store's copy at `relative`, once you chose.
    pub fn forget(&self, relative: &str) {
        let path = self.dir.join(relative);
        let _ = std::fs::remove_file(&path);
        if let Some(parent) = path.parent()
            && parent != self.dir
        {
            let _ = std::fs::remove_dir(parent);
        }
    }

    /// Moves this journal's review store aside to
    /// `.noura/sync/archive/<replica>/`, and removes the rest of the
    /// journal, as connecting a folder again does (§15). Returns the
    /// archive's folder.
    pub fn archive(self, replica: &Id) -> Result<PathBuf, JournalError> {
        let journals = self
            .dir
            .parent()
            .map(Path::to_owned)
            .ok_or_else(|| JournalError::Io("the journal has no folder above it".into()))?;
        let archive = journals.join(ARCHIVE).join(replica.to_hex());
        std::fs::create_dir_all(&archive).map_err(io)?;
        for review in [CONFLICTS, HELD] {
            let from = self.dir.join(review);
            if from.exists() {
                let to = archive.join(review);
                if to.exists() {
                    std::fs::remove_dir_all(&to).map_err(io)?;
                }
                std::fs::rename(&from, &to).map_err(io)?;
            }
        }
        let _ = std::fs::remove_file(self.dir.join(STATE_FILE));
        Ok(archive)
    }
}

/// Writes `bytes` to `path` through a temporary file, then syncs the file
/// and the folder that holds it.
pub(crate) fn durable_write(path: &Path, bytes: &[u8]) -> Result<(), JournalError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(io)?;
    }
    let mut file = atomic_write_file::AtomicWriteFile::open(path).map_err(io)?;
    file.write_all(bytes).map_err(io)?;
    file.sync_all().map_err(io)?;
    file.commit().map_err(io)?;
    sync_folder(path)
}

/// Syncs the folder that holds `path`, so its entry is on disk.
fn sync_folder(path: &Path) -> Result<(), JournalError> {
    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        std::fs::File::open(parent)
            .and_then(|folder| folder.sync_all())
            .map_err(io)?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The BLAKE3 hash of the file at `path`, read as it streams.
pub(crate) fn hash_file(path: &Path) -> std::io::Result<Digest> {
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(std::fs::File::open(path)?)?;
    Ok(Digest(*hasher.finalize().as_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;
    use zeroize::Zeroizing;

    #[derive(Default)]
    struct Memory(Mutex<HashMap<String, String>>);

    impl SyncCredentials for Memory {
        fn read(&self, reference: &str) -> local_core::Result<Zeroizing<String>> {
            Ok(self.read_optional(reference)?.unwrap_or_default())
        }
        fn read_optional(&self, reference: &str) -> local_core::Result<Option<Zeroizing<String>>> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .get(reference)
                .cloned()
                .map(Zeroizing::new))
        }
        fn write(&self, reference: &str, value: &str) -> local_core::Result<()> {
            self.0
                .lock()
                .unwrap()
                .insert(reference.into(), value.into());
            Ok(())
        }
    }

    #[test]
    fn a_file_round_trips_and_one_that_changed_or_moved_is_refused() {
        let root = tempfile::tempdir().unwrap();
        let secrets = Memory::default();
        let device = Id([7; 16]);
        let journal = Journal::open(root.path(), &device, &secrets).unwrap();
        journal.write(STATE_FILE, &vec![1u64, 2, 3]).unwrap();
        assert_eq!(
            journal.read::<Vec<u64>>(STATE_FILE).unwrap(),
            Some(vec![1, 2, 3])
        );
        let path = journal.dir().join(STATE_FILE);
        let bytes = std::fs::read(&path).unwrap();
        let mut changed = bytes.clone();
        *changed.last_mut().unwrap() = b'4';
        std::fs::write(&path, &changed).unwrap();
        assert!(matches!(
            journal.read::<Vec<u64>>(STATE_FILE),
            Err(JournalError::Refused)
        ));
        // A file copied under another journal name fails too.
        std::fs::write(journal.dir().join("other.json"), &bytes).unwrap();
        assert!(matches!(
            journal.read::<Vec<u64>>("other.json"),
            Err(JournalError::Refused)
        ));
        std::fs::write(&path, b"not a journal").unwrap();
        assert!(matches!(
            journal.read::<Vec<u64>>(STATE_FILE),
            Err(JournalError::Refused)
        ));
        assert_eq!(journal.read::<Vec<u64>>("missing.json").unwrap(), None);
    }

    #[test]
    fn large_copies_stream_into_the_review_store_and_are_checked_on_the_way_out() {
        let root = tempfile::tempdir().unwrap();
        let secrets = Memory::default();
        let journal = Journal::open(root.path(), &Id([8; 16]), &secrets).unwrap();
        let bytes: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let revision = sync_protocol::crypto::hash(&bytes);
        let source = root.path().join("download");
        std::fs::write(&source, &bytes).unwrap();

        // A source whose bytes don't match keeps nothing.
        let wrong = sync_protocol::crypto::hash(b"other");
        assert!(matches!(
            journal.keep_file("conflicts/a/b", &source, &wrong),
            Err(JournalError::Refused)
        ));
        assert!(!journal.dir().join("conflicts/a/b").exists());

        journal
            .keep_file("conflicts/a/b", &source, &revision)
            .unwrap();
        let kept = journal.kept_path("conflicts/a/b", &revision).unwrap();
        assert!(std::fs::read(&kept).unwrap() == bytes);
        std::fs::write(&kept, b"changed by another program").unwrap();
        assert!(matches!(
            journal.kept_path("conflicts/a/b", &revision),
            Err(JournalError::Refused)
        ));

        // A streamed write lands whole, or not at all.
        let written: Result<u32, SyncError> = journal.keep_with("uploads/x", |out| {
            out.write_all(&bytes)
                .map_err(|e| SyncError::Invalid(e.to_string()))?;
            Ok(7)
        });
        assert_eq!(written.unwrap(), 7);
        assert!(std::fs::read(journal.dir().join("uploads/x")).unwrap() == bytes);
        let failed: Result<(), SyncError> = journal.keep_with("uploads/y", |out| {
            out.write_all(b"part")
                .map_err(|e| SyncError::Invalid(e.to_string()))?;
            Err(SyncError::Invalid("the file changed".into()))
        });
        assert!(failed.is_err());
        assert!(!journal.dir().join("uploads/y").exists());
    }

    #[test]
    fn a_journal_is_locked_while_open_and_others_are_listed() {
        let root = tempfile::tempdir().unwrap();
        let secrets = Memory::default();
        let device = Id([7; 16]);
        let journal = Journal::open(root.path(), &device, &secrets).unwrap();
        assert!(matches!(
            Journal::open(root.path(), &device, &secrets),
            Err(SyncError::Invalid(_))
        ));
        drop(journal);
        let journal = Journal::open(root.path(), &device, &secrets).unwrap();
        let other = Id([8; 16]);
        Journal::open(root.path(), &other, &secrets).unwrap();
        assert_eq!(other_journals(root.path(), &device), vec![other.to_hex()]);
        assert!(other_journals(root.path(), &other).contains(&device.to_hex()));
        drop(journal);
    }

    #[test]
    fn the_review_store_keeps_plain_copies_checked_by_revision() {
        let root = tempfile::tempdir().unwrap();
        let secrets = Memory::default();
        let journal = Journal::open(root.path(), &Id([7; 16]), &secrets).unwrap();
        journal.keep("conflicts/a/b", b"theirs").unwrap();
        let revision = sync_protocol::crypto::hash(b"theirs");
        assert_eq!(journal.kept("conflicts/a/b", &revision).unwrap(), b"theirs");
        std::fs::write(journal.dir().join("conflicts/a/b"), b"edited").unwrap();
        assert!(journal.kept("conflicts/a/b", &revision).is_err());
        journal.keep("held/c/d", b"mine").unwrap();
        let archive = journal.archive(&Id([9; 16])).unwrap();
        assert!(archive.join("conflicts/a/b").exists());
        assert!(archive.join("held/c/d").exists());
    }
}
