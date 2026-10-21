//! The self-write journal: the changes this engine is about to make, so
//! the watcher's reports of them never read as another app's.

use std::{
    collections::HashMap,
    fs::Metadata,
    path::Path,
    time::{Duration, Instant, SystemTime},
};

use crate::markdown;

use super::scan::is_dataless;

/// How long a journal entry lasts. Platforms report one write several
/// times and sometimes late, and the poller may be busy with a full
/// rescan, so a report within this time of the write still counts as
/// ours while the path shows what we wrote. A later report reads as
/// outside, which costs one re-read and shows no change.
pub(super) const OWN_WRITE_WINDOW: Duration = Duration::from_secs(30);

/// What a path shows once the engine's change lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Expected {
    /// A file with this revision.
    Revision(String),
    /// No file or folder.
    Deleted,
    /// A folder: one the write creates, or one whose listing it changes.
    Folder,
    /// A file that kept its size, modification time and download state,
    /// as one does when it moves. Checking it never reads the file.
    Unchanged(Stat),
}

/// What a file's metadata says about its content without reading it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Stat {
    len: u64,
    modified: Option<SystemTime>,
    dataless: bool,
}

impl Stat {
    pub(super) fn of(metadata: &Metadata) -> Self {
        Self {
            len: metadata.len(),
            modified: metadata.modified().ok(),
            dataless: is_dataless(metadata),
        }
    }
}

#[derive(Debug, Clone)]
struct Entry {
    expected: Expected,
    at: Instant,
}

/// The engine's own recent writes, by workspace-relative path.
#[derive(Debug, Default)]
pub(super) struct OwnWrites {
    entries: HashMap<String, Entry>,
}

impl OwnWrites {
    /// Note that the engine is about to make `relative` show `expected`.
    /// For a file it also notes the folders the platform may report: the
    /// ones the write creates, and the folder each is created in, up to
    /// the workspace root. Call it before touching the disk.
    pub(super) fn expect(&mut self, root: &Path, relative: &str, expected: Expected, now: Instant) {
        self.prune(now);
        for folder in changed_folders(root, relative) {
            self.insert(folder, Expected::Folder, now);
        }
        self.insert(relative.to_owned(), expected, now);
    }

    /// Note that the engine is about to make `relative` show `expected`,
    /// without the folders above it: for a path inside an item that moves
    /// or goes, whose folders the item's own entry covers.
    pub(super) fn expect_only(&mut self, relative: &str, expected: Expected, now: Instant) {
        self.insert(relative.to_owned(), expected, now);
    }

    fn insert(&mut self, relative: String, expected: Expected, at: Instant) {
        self.entries.insert(relative, Entry { expected, at });
    }

    /// Forget an expected write that failed, with the folders it never
    /// created.
    pub(super) fn forget(&mut self, root: &Path, relative: &str) {
        self.entries.remove(relative);
        for folder in changed_folders(root, relative) {
            if !root.join(&folder).is_dir() {
                self.entries.remove(&folder);
            }
        }
    }

    /// Whether a recent write of ours covers `relative`. The watcher
    /// thread asks this without touching the disk.
    pub(super) fn covers(&self, relative: &str, now: Instant) -> bool {
        self.entries
            .get(relative)
            .is_some_and(|entry| fresh(entry, now))
    }

    /// Whether a watcher report of `relative`, at `path`, is our own
    /// write: an entry covers it and the path still shows what we wrote.
    /// A path that shows something else ends the entry. Platforms report
    /// one write several times, so a match keeps the entry until it
    /// expires.
    pub(super) fn reported(&mut self, relative: &str, path: &Path, now: Instant) -> bool {
        self.prune(now);
        let Some(entry) = self.entries.get(relative) else {
            return false;
        };
        if shows(path, &entry.expected) {
            return true;
        }
        self.entries.remove(relative);
        false
    }

    /// Drop the entries older than the window, so the journal holds only
    /// recent writes however long the workspace stays open.
    fn prune(&mut self, now: Instant) {
        self.entries.retain(|_, entry| fresh(entry, now));
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.entries.len()
    }
}

fn fresh(entry: &Entry, now: Instant) -> bool {
    now.saturating_duration_since(entry.at) < OWN_WRITE_WINDOW
}

/// Whether `path` shows `expected`. It never reads a file whose content
/// lives only in the cloud, which would start a download and block: such
/// a file shows nothing we wrote, so the caller treats the report as
/// outside and the walk marks the file not downloaded.
fn shows(path: &Path, expected: &Expected) -> bool {
    match (expected, std::fs::symlink_metadata(path)) {
        (Expected::Deleted, Err(error)) => error.kind() == std::io::ErrorKind::NotFound,
        (Expected::Folder, Ok(metadata)) => metadata.is_dir(),
        (Expected::Revision(revision), Ok(metadata)) => {
            metadata.is_file()
                && !is_dataless(&metadata)
                && std::fs::read(path).is_ok_and(|bytes| markdown::revision(&bytes) == *revision)
        }
        (Expected::Unchanged(stat), Ok(metadata)) => {
            !metadata.is_dir() && Stat::of(&metadata) == *stat
        }
        _ => false,
    }
}

/// The item at `source` and everything in it, by path inside the item
/// (`""` for the item itself), with what each path shows once the item
/// moves unchanged. Dot folders are left out, as the watcher's reports of
/// them are skipped, and so are names with no UTF-8 form, which no report
/// can name.
pub(super) fn moved_items(source: &Path) -> Vec<(String, Expected)> {
    let mut items = Vec::new();
    let mut pending = vec![(source.to_path_buf(), String::new())];
    while let Some((path, inner)) = pending.pop() {
        let Ok(metadata) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_dir() {
            items.push((inner, Expected::Unchanged(Stat::of(&metadata))));
            continue;
        }
        for child in std::fs::read_dir(&path).into_iter().flatten().flatten() {
            let name = child.file_name();
            let Some(name) = name.to_str() else { continue };
            if name.starts_with('.') && child.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let child_inner = match inner.as_str() {
                "" => name.to_owned(),
                inner => format!("{inner}/{name}"),
            };
            pending.push((child.path(), child_inner));
        }
        items.push((inner, Expected::Folder));
    }
    items
}

/// The folders a write to `relative` can make the platform report: its
/// folder, and while a folder doesn't exist yet, the one above it, which
/// gains it. The workspace root is never one: its reports are skipped.
fn changed_folders(root: &Path, relative: &str) -> Vec<String> {
    let mut folders = Vec::new();
    let mut folder = Path::new(relative).parent();
    while let Some(current) = folder.filter(|path| !path.as_os_str().is_empty()) {
        let Some(name) = current.to_str() else { break };
        folders.push(name.replace('\\', "/"));
        if root.join(current).is_dir() {
            break;
        }
        folder = current.parent();
    }
    folders
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn a_write_into_new_folders_notes_each_folder_and_the_one_that_gains_them() {
        let root = tempdir().unwrap();
        std::fs::create_dir(root.path().join("Inbox")).unwrap();
        let mut writes = OwnWrites::default();
        let now = Instant::now();

        writes.expect(
            root.path(),
            "Inbox/Deep/Deeper/Mine.md",
            Expected::Revision("r".into()),
            now,
        );

        let mut noted = writes.entries.keys().cloned().collect::<Vec<_>>();
        noted.sort();
        assert_eq!(
            noted,
            [
                "Inbox",
                "Inbox/Deep",
                "Inbox/Deep/Deeper",
                "Inbox/Deep/Deeper/Mine.md"
            ]
        );
    }

    #[test]
    fn a_write_at_the_top_notes_no_folder() {
        let root = tempdir().unwrap();
        let mut writes = OwnWrites::default();

        writes.expect(root.path(), "Mine.md", Expected::Deleted, Instant::now());

        assert_eq!(writes.len(), 1);
    }

    #[test]
    fn a_failed_write_forgets_the_folders_it_never_created() {
        let root = tempdir().unwrap();
        std::fs::create_dir(root.path().join("Inbox")).unwrap();
        let mut writes = OwnWrites::default();
        let now = Instant::now();
        writes.expect(
            root.path(),
            "Inbox/Deep/Mine.md",
            Expected::Revision("r".into()),
            now,
        );

        writes.forget(root.path(), "Inbox/Deep/Mine.md");

        assert!(writes.covers("Inbox", now));
        assert!(!writes.covers("Inbox/Deep", now));
        assert!(!writes.covers("Inbox/Deep/Mine.md", now));
    }

    #[test]
    fn entries_expire_so_the_journal_stays_small() {
        let root = tempdir().unwrap();
        let path = root.path().join("Mine.md");
        std::fs::write(&path, "# Mine\n").unwrap();
        let revision = markdown::revision(b"# Mine\n");
        let mut writes = OwnWrites::default();
        let start = Instant::now();
        writes.expect(root.path(), "Mine.md", Expected::Revision(revision), start);
        let later = start + OWN_WRITE_WINDOW;

        assert!(writes.reported("Mine.md", &path, start));
        assert!(writes.reported("Mine.md", &path, start));
        assert!(!writes.covers("Mine.md", later));
        assert!(!writes.reported("Mine.md", &path, later));
        assert_eq!(writes.len(), 0);
    }

    #[test]
    fn another_change_to_the_path_ends_the_entry() {
        let root = tempdir().unwrap();
        let path = root.path().join("Mine.md");
        std::fs::write(&path, "# Mine\n").unwrap();
        let mut writes = OwnWrites::default();
        let now = Instant::now();
        writes.expect(
            root.path(),
            "Mine.md",
            Expected::Revision(markdown::revision(b"# Mine\n")),
            now,
        );

        std::fs::write(&path, "# Theirs\n").unwrap();

        assert!(!writes.reported("Mine.md", &path, now));
        std::fs::write(&path, "# Mine\n").unwrap();
        assert!(!writes.reported("Mine.md", &path, now));
    }

    #[test]
    fn a_folder_counts_as_ours_while_it_is_a_folder() {
        let root = tempdir().unwrap();
        let folder = root.path().join("Inbox");
        let mut writes = OwnWrites::default();
        let now = Instant::now();
        writes.expect(root.path(), "Inbox/Mine.md", Expected::Deleted, now);
        std::fs::create_dir(&folder).unwrap();

        assert!(writes.reported("Inbox", &folder, now));
        std::fs::remove_dir(&folder).unwrap();
        assert!(!writes.reported("Inbox", &folder, now));
    }

    #[test]
    fn a_moved_file_counts_as_ours_until_its_content_changes() {
        let root = tempdir().unwrap();
        let from = root.path().join("Scan.pdf");
        let to = root.path().join("Moved.pdf");
        std::fs::write(&from, "%PDF-1.7\n").unwrap();
        let stat = Stat::of(&std::fs::symlink_metadata(&from).unwrap());
        let mut writes = OwnWrites::default();
        let now = Instant::now();
        writes.expect(root.path(), "Scan.pdf", Expected::Deleted, now);
        writes.expect_only("Moved.pdf", Expected::Unchanged(stat), now);
        std::fs::rename(&from, &to).unwrap();

        assert!(writes.reported("Scan.pdf", &from, now));
        assert!(writes.reported("Moved.pdf", &to, now));
        std::fs::write(&to, "%PDF-1.7 edited\n").unwrap();
        assert!(!writes.reported("Moved.pdf", &to, now));
    }

    #[test]
    fn a_path_inside_a_moved_item_notes_no_folder() {
        let root = tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("Docs/Photos")).unwrap();
        let mut writes = OwnWrites::default();

        writes.expect_only("Docs/Photos/cat.jpg", Expected::Deleted, Instant::now());

        assert_eq!(writes.len(), 1);
    }

    /// The offline attribute is the one online-only mark a test can set
    /// without a cloud sync root. Windows still lets the test read the
    /// file, so a match here would mean the check read it.
    #[cfg(windows)]
    #[test]
    fn a_file_the_cloud_provider_took_online_only_never_counts_as_ours() {
        use std::os::windows::ffi::OsStrExt;

        use windows_sys::Win32::Storage::FileSystem::{
            FILE_ATTRIBUTE_OFFLINE, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
        };

        let root = tempdir().unwrap();
        let path = root.path().join("Mine.md");
        std::fs::write(&path, "# Mine\n").unwrap();
        let expected = Expected::Revision(markdown::revision(b"# Mine\n"));
        assert!(shows(&path, &expected));

        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        // SAFETY: `wide` is a NUL-terminated path that outlives both calls.
        unsafe {
            let attributes = GetFileAttributesW(wide.as_ptr());
            assert_ne!(attributes, INVALID_FILE_ATTRIBUTES);
            assert_ne!(
                SetFileAttributesW(wide.as_ptr(), attributes | FILE_ATTRIBUTE_OFFLINE),
                0
            );
        }

        assert!(!shows(&path, &expected));
    }
}
