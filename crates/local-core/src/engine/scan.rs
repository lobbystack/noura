//! One walk over the visible workspace, shared by reconciliation, index
//! rebuilds, and the file browser listing.

use super::*;

pub(super) type MarkdownIndexEntry = (String, Vec<u8>, i64, ParsedMarkdown);

/// What one walk of the workspace found.
#[derive(Default)]
pub(super) struct WorkspaceScan {
    /// Markdown files whose bytes changed since they were indexed.
    pub(super) changed: Vec<MarkdownIndexEntry>,
    /// Markdown paths that still exist, including files that could not be
    /// read and files that iCloud has not downloaded.
    pub(super) seen: HashSet<String>,
    /// Folders the walk could not enter. Indexed files below them are kept.
    pub(super) unreadable_folders: Vec<String>,
    /// The walk failed somewhere it could not attribute to a path, so it
    /// must not treat missing files as deleted.
    pub(super) incomplete: bool,
    pub(super) diagnostics: Vec<crate::Diagnostic>,
    /// Markdown paths whose content lives only in the cloud for now.
    pub(super) not_downloaded: BTreeSet<String>,
    /// Every visible file and folder, when the caller asked for a listing.
    pub(super) entries: Vec<ScannedEntry>,
    /// Markdown paths whose index rows this walk replaced or removed.
    pub(super) touched: Vec<String>,
}

impl WorkspaceScan {
    /// Indexed paths the walk proved absent.
    pub(super) fn removed(&self, indexed: &HashMap<String, (i64, i64)>) -> Vec<String> {
        if self.incomplete {
            return Vec::new();
        }
        indexed
            .keys()
            .filter(|path| {
                !self.seen.contains(*path)
                    && !self
                        .unreadable_folders
                        .iter()
                        .any(|folder| path.starts_with(&format!("{folder}/")))
            })
            .cloned()
            .collect()
    }
}

pub(super) struct ScannedEntry {
    pub(super) relative_path: String,
    pub(super) name: String,
    pub(super) is_folder: bool,
    pub(super) not_downloaded: bool,
}

/// Findings of the last full walk that do not live in the index.
#[derive(Default)]
pub(super) struct ScanState {
    pub(super) diagnostics: Vec<crate::Diagnostic>,
    pub(super) not_downloaded: BTreeSet<String>,
}

pub(super) fn scan_into(
    root: &Path,
    ignore_patterns: &[String],
    index: &mut IndexStore,
) -> Result<()> {
    let scan = scan_workspace(
        root,
        ignore_patterns,
        Some((&HashMap::new(), &HashSet::new())),
        false,
    )?;
    index.replace_markdown(&scan.changed)
}

/// Indexed `(size, mtime)` per path, and the paths to re-read regardless.
pub(super) type IndexedFiles<'a> = (&'a HashMap<String, (i64, i64)>, &'a HashSet<String>);

/// Walk the visible workspace once.
///
/// With `index`, Markdown files whose size or modification time differ from
/// the indexed values (or that are `forced`) are read and parsed. Without it
/// the walk only lists entries. Unreadable files and folders become
/// diagnostics instead of aborting the walk, and files that iCloud has not
/// downloaded keep their index rows instead of looking deleted.
pub(super) fn scan_workspace(
    root: &Path,
    ignore_patterns: &[String],
    index: Option<IndexedFiles<'_>>,
    list_entries: bool,
) -> Result<WorkspaceScan> {
    let mut scan = WorkspaceScan::default();
    for entry in index_walker(root, ignore_patterns)? {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                record_walk_error(root, &error, &mut scan);
                continue;
            }
        };
        if entry.path() == root {
            continue;
        }
        let Some(file_type) = entry.file_type() else {
            continue;
        };
        let Ok(relative) = normalized_relative_path(root, entry.path(), "workspace_scan") else {
            tracing::warn!("skipped a workspace path that is not UTF-8");
            scan.diagnostics.push(crate::Diagnostic {
                code: "unsupported_path".into(),
                message: "A file or folder name can't be shown. Rename it to use it here.".into(),
                relative_path: None,
                object_id: None,
            });
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if file_type.is_dir() {
            if list_entries {
                scan.entries.push(ScannedEntry {
                    relative_path: relative,
                    name,
                    is_folder: true,
                    not_downloaded: false,
                });
            }
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        if let Some(real_name) = placeholder_target(&name) {
            // `.Note.md.icloud` stands in for `Note.md` until it downloads.
            let real = match relative.rsplit_once('/') {
                Some((parent, _)) => format!("{parent}/{real_name}"),
                None => real_name.to_owned(),
            };
            if is_markdown_path(&real) {
                scan.seen.insert(real.clone());
                scan.not_downloaded.insert(real.clone());
            }
            if list_entries {
                scan.entries.push(ScannedEntry {
                    relative_path: real,
                    name: real_name.to_owned(),
                    is_folder: false,
                    not_downloaded: true,
                });
            }
            continue;
        }
        let markdown = is_markdown_path(&relative);
        let metadata = if markdown {
            match entry.metadata() {
                Ok(metadata) => Some(metadata),
                Err(error) => {
                    scan.seen.insert(relative.clone());
                    unreadable_file(&relative, &error.to_string(), &mut scan);
                    None
                }
            }
        } else {
            None
        };
        let dataless = metadata.as_ref().is_some_and(is_dataless);
        if list_entries {
            scan.entries.push(ScannedEntry {
                relative_path: relative.clone(),
                name,
                is_folder: false,
                not_downloaded: dataless,
            });
        }
        let Some(metadata) = metadata else { continue };
        scan.seen.insert(relative.clone());
        if dataless {
            // Reading would start a download and block; keep the last index.
            scan.not_downloaded.insert(relative);
            continue;
        }
        let Some((indexed, forced)) = index else {
            continue;
        };
        let size = metadata.len().min(i64::MAX as u64) as i64;
        let modified = modified_ns(&metadata);
        if !forced.contains(&relative) && indexed.get(&relative) == Some(&(size, modified)) {
            continue;
        }
        match std::fs::read(entry.path()) {
            Ok(bytes) => {
                let parsed = markdown::parse_markdown(&relative, &bytes);
                scan.changed.push((relative, bytes, modified, parsed));
            }
            Err(error) => unreadable_file(&relative, &error.to_string(), &mut scan),
        }
    }
    Ok(scan)
}

fn unreadable_file(relative: &str, reason: &str, scan: &mut WorkspaceScan) {
    tracing::warn!(path = %relative, reason, "a workspace file could not be read");
    scan.diagnostics.push(crate::Diagnostic {
        code: "unreadable_file".into(),
        message: "This file can't be read. Check that you have permission to open it.".into(),
        relative_path: Some(relative.to_owned()),
        object_id: None,
    });
}

/// Keep walking past a folder or file the walker could not open. The OS
/// reason goes to the log; the diagnostic names the path in plain words.
fn record_walk_error(root: &Path, error: &ignore::Error, scan: &mut WorkspaceScan) {
    let path = walk_error_path(error);
    tracing::warn!(path = ?path, %error, "the workspace walk skipped an entry");
    let relative =
        path.and_then(|path| normalized_relative_path(root, path, "workspace_scan").ok());
    let Some(relative) = relative.filter(|relative| !relative.is_empty()) else {
        scan.incomplete = true;
        scan.diagnostics.push(crate::Diagnostic {
            code: "scan_incomplete".into(),
            message: "Some files couldn't be checked. They stay as they were until the next check."
                .into(),
            relative_path: None,
            object_id: None,
        });
        return;
    };
    if path.is_some_and(Path::is_file) {
        scan.seen.insert(relative.clone());
        unreadable_file(&relative, &error.to_string(), scan);
        return;
    }
    scan.diagnostics.push(crate::Diagnostic {
        code: "unreadable_folder".into(),
        message: "This folder can't be opened. Check that you have permission to open it.".into(),
        relative_path: Some(relative.clone()),
        object_id: None,
    });
    scan.unreadable_folders.push(relative);
}

fn walk_error_path(error: &ignore::Error) -> Option<&Path> {
    match error {
        ignore::Error::WithPath { path, .. } => Some(path),
        ignore::Error::WithDepth { err, .. } | ignore::Error::WithLineNumber { err, .. } => {
            walk_error_path(err)
        }
        ignore::Error::Loop { child, .. } => Some(child),
        _ => None,
    }
}

pub(super) fn is_markdown_path(relative: &str) -> bool {
    Path::new(relative)
        .extension()
        .and_then(|value| value.to_str())
        == Some("md")
}

/// The file an iCloud placeholder (`.Name.ext.icloud`) stands in for.
pub(super) fn placeholder_target(name: &str) -> Option<&str> {
    name.strip_prefix('.')?
        .strip_suffix(".icloud")
        .filter(|real| !real.is_empty() && !real.starts_with('.'))
}

/// Where iCloud keeps the placeholder for a file it has not downloaded.
pub(super) fn icloud_placeholder(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.icloud"))
}

/// Whether macOS reports the file as dataless: its content lives only in
/// the cloud and reading it would block on a download.
#[cfg(target_os = "macos")]
pub(super) fn is_dataless(metadata: &std::fs::Metadata) -> bool {
    use std::os::macos::fs::MetadataExt;
    const SF_DATALESS: u32 = 0x4000_0000;
    metadata.st_flags() & SF_DATALESS != 0
}

#[cfg(not(target_os = "macos"))]
pub(super) fn is_dataless(_metadata: &std::fs::Metadata) -> bool {
    false
}

pub(super) fn modified_ns(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
}

/// Paths the index can contain: nothing under a dot folder, no dot files
/// except iCloud placeholders, and none of the reserved folders. Workspace
/// ignore patterns apply on top of this.
pub(super) fn is_index_candidate(relative: &Path) -> bool {
    let mut components = relative.components().peekable();
    let mut first = true;
    while let Some(component) = components.next() {
        let std::path::Component::Normal(name) = component else {
            return false;
        };
        let name = name.to_string_lossy();
        if first && matches!(name.as_ref(), "node_modules" | "target") {
            return false;
        }
        first = false;
        if name.starts_with('.') {
            let last = components.peek().is_none();
            if !(last && placeholder_target(&name).is_some()) {
                return false;
            }
        }
    }
    true
}

/// Walker over the paths the index and file browser show: the workspace
/// walker minus dot folders and dot files (iCloud placeholders stay).
fn index_walker(root: &Path, ignore_patterns: &[String]) -> Result<ignore::Walk> {
    let ignores = compile_workspace_ignores(root, ignore_patterns)?;
    let filter_root = root.to_owned();
    Ok(WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .filter_entry(move |entry| {
            let relative = entry
                .path()
                .strip_prefix(&filter_root)
                .unwrap_or(entry.path());
            if relative.as_os_str().is_empty() {
                return true;
            }
            let is_directory = entry.file_type().is_some_and(|kind| kind.is_dir());
            let name = entry.file_name().to_string_lossy();
            if name.starts_with('.') && (is_directory || placeholder_target(&name).is_none()) {
                return false;
            }
            is_visible_workspace_path(relative, is_directory, &ignores)
        })
        .build())
}

/// The watcher-thread hook that marks the index dirty when a visible path
/// changes. Paths this engine just wrote are skipped: the poller verifies
/// them against the journal and reconciles if they turn out to be external.
pub(super) fn dirty_marker(
    root: PathBuf,
    journal: Arc<Mutex<HashMap<String, String>>>,
    dirty: Arc<AtomicBool>,
) -> impl Fn(&notify::Event) + Send + 'static {
    move |event| {
        if dirty.load(Ordering::Relaxed) {
            return;
        }
        for path in &event.paths {
            let Ok(relative) = path.strip_prefix(&root) else {
                continue;
            };
            if relative.as_os_str().is_empty() || !is_index_candidate(relative) {
                continue;
            }
            let relevant = path
                .extension()
                .is_some_and(|extension| extension == "md" || extension == "icloud")
                || !path.is_file();
            if !relevant {
                continue;
            }
            let key = relative.to_string_lossy().replace('\\', "/");
            if journal
                .lock()
                .map(|journal| journal.contains_key(&key))
                .unwrap_or(false)
            {
                continue;
            }
            dirty.store(true, Ordering::SeqCst);
            return;
        }
    }
}

pub(super) fn compile_workspace_ignores(
    root: &Path,
    patterns: &[String],
) -> Result<ignore::gitignore::Gitignore> {
    let mut builder = GitignoreBuilder::new(root);
    for pattern in patterns {
        builder.add_line(None, pattern).map_err(|error| {
            tracing::warn!(pattern, %error, "a workspace ignore pattern is invalid");
            let mut value = CoreError::validation(
                "invalid_ignore_pattern",
                "A workspace ignore pattern is invalid",
                "workspace_open",
            );
            value.details = Some(serde_json::json!({ "pattern": pattern }));
            value
        })?;
    }
    builder.build().map_err(|error| {
        tracing::warn!(%error, "the workspace ignore patterns could not be compiled");
        CoreError::validation(
            "invalid_ignore_pattern",
            "The workspace ignore patterns are invalid",
            "workspace_open",
        )
    })
}

pub(super) fn workspace_walker(root: &Path, ignore_patterns: &[String]) -> Result<ignore::Walk> {
    let ignores = compile_workspace_ignores(root, ignore_patterns)?;
    let filter_root = root.to_owned();
    Ok(WalkBuilder::new(root)
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .parents(false)
        .filter_entry(move |entry| {
            let relative = entry
                .path()
                .strip_prefix(&filter_root)
                .unwrap_or(entry.path());
            is_visible_workspace_path(
                relative,
                entry.file_type().is_some_and(|kind| kind.is_dir()),
                &ignores,
            )
        })
        .build())
}

pub(super) fn is_visible_workspace_path(
    relative: &Path,
    is_directory: bool,
    ignores: &ignore::gitignore::Gitignore,
) -> bool {
    if relative.as_os_str().is_empty() {
        return true;
    }
    if relative.starts_with(".noura")
        || relative.starts_with(".git")
        || relative.starts_with("node_modules")
        || relative.starts_with("target")
    {
        return false;
    }
    !ignores
        .matched_path_or_any_parents(relative, is_directory)
        .is_ignore()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::reconcile::OBJECT_EVENT_BATCH_LIMIT;
    use crate::{CreateObjectInput, ManagedDraftInput, ManagedDraftResult};
    use tempfile::tempdir;

    fn open() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Scan", app_data.path())
                .unwrap();
        (workspace, app_data, engine)
    }

    fn note(engine: &WorkspaceEngine, path: &str) -> WorkspaceObject {
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Note".into(),
                body: "Body\n".into(),
                relative_path: Some(path.into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        engine.get_object(&created.value.id).unwrap().unwrap()
    }

    fn diagnostic_codes(engine: &WorkspaceEngine) -> Vec<(String, Option<String>)> {
        engine
            .state()
            .diagnostics
            .into_iter()
            .map(|diagnostic| (diagnostic.code, diagnostic.relative_path))
            .collect()
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn a_folder_without_permission_is_skipped_and_its_files_stay_indexed() {
        let (workspace, _app, engine) = open();
        let object = note(&engine, "locked/kept.md");
        std::fs::write(workspace.path().join("open.md"), "# Open\n").unwrap();
        let locked = workspace.path().join("locked");
        set_mode(&locked, 0o000);
        // Permission bits do not stop root; nothing to test there.
        if std::fs::read_dir(&locked).is_ok() {
            set_mode(&locked, 0o755);
            return;
        }
        let reconciled = engine.reconcile();
        let entries = engine.list_workspace_entries();
        let codes = diagnostic_codes(&engine);
        let messages = engine.state().diagnostics;
        let kept = engine.get_object(&object.id);
        set_mode(&locked, 0o755);

        reconciled.unwrap();
        assert!(kept.unwrap().is_some());
        assert!(
            entries
                .unwrap()
                .iter()
                .any(|entry| entry.relative_path == "open.md")
        );
        assert!(codes.contains(&("unreadable_folder".into(), Some("locked".into()))));
        // Diagnostics carry plain words; OS error text stays in the log.
        assert!(
            messages
                .iter()
                .all(|diagnostic| !diagnostic.message.contains("os error"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn an_unreadable_file_is_skipped_and_keeps_its_index_row() {
        let (workspace, _app, engine) = open();
        let object = note(&engine, "private.md");
        let path = workspace.path().join("private.md");
        std::fs::write(workspace.path().join("fresh.md"), "# Fresh\n").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, format!("{text}\nChanged externally\n")).unwrap();
        set_mode(&path, 0o000);
        if std::fs::read(&path).is_ok() {
            set_mode(&path, 0o644);
            return;
        }
        let reconciled = engine.reconcile();
        let kept = engine.get_object(&object.id);
        let codes = diagnostic_codes(&engine);
        let entries = engine.list_workspace_entries();
        set_mode(&path, 0o644);

        reconciled.unwrap();
        assert!(kept.unwrap().is_some());
        assert!(codes.contains(&("unreadable_file".into(), Some("private.md".into()))));
        assert!(
            entries
                .unwrap()
                .iter()
                .any(|entry| entry.relative_path == "fresh.md")
        );
        // Once readable again, the next walk picks up the external change.
        engine.reconcile().unwrap();
        assert!(
            engine
                .get_object(&object.id)
                .unwrap()
                .unwrap()
                .body
                .contains("Changed externally")
        );
        assert!(diagnostic_codes(&engine).is_empty());
    }

    #[test]
    fn an_icloud_placeholder_keeps_the_note_indexed_as_not_downloaded() {
        let (workspace, _app, engine) = open();
        let object = note(&engine, "Journal/today.md");
        let folder = workspace.path().join("Journal");
        std::fs::remove_file(folder.join("today.md")).unwrap();
        std::fs::write(folder.join(".today.md.icloud"), b"bplist").unwrap();
        std::fs::write(folder.join(".later.md.icloud"), b"bplist").unwrap();

        engine.reconcile().unwrap();

        assert!(engine.get_object(&object.id).unwrap().is_some());
        let entries = engine.list_workspace_entries().unwrap();
        let today = entries
            .iter()
            .find(|entry| entry.relative_path == "Journal/today.md")
            .unwrap();
        assert!(today.not_downloaded);
        assert_eq!(today.object_id.as_deref(), Some(object.id.as_str()));
        let later = entries
            .iter()
            .find(|entry| entry.relative_path == "Journal/later.md")
            .unwrap();
        assert!(later.not_downloaded);
        assert!(later.parse_status.is_none());
        assert!(
            !entries
                .iter()
                .any(|entry| entry.relative_path.contains(".icloud"))
        );
        let error = engine.read_raw_markdown("Journal/later.md").unwrap_err();
        assert_eq!(error.code, "file_not_downloaded");

        // Downloading replaces the placeholder with the file.
        std::fs::remove_file(folder.join(".later.md.icloud")).unwrap();
        std::fs::write(folder.join("later.md"), "# Later\n").unwrap();
        engine.reconcile().unwrap();
        let later = engine
            .list_workspace_entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.relative_path == "Journal/later.md")
            .unwrap();
        assert!(!later.not_downloaded);
        assert!(later.parse_status.is_some());
    }

    #[test]
    fn dot_paths_are_not_indexed_listed_or_reported_as_changes() {
        let (workspace, _app, engine) = open();
        std::fs::create_dir_all(workspace.path().join(".obsidian")).unwrap();
        std::fs::write(workspace.path().join(".obsidian/workspace.json"), b"{}").unwrap();
        std::fs::write(workspace.path().join(".obsidian/notes.md"), b"# Hidden\n").unwrap();
        std::fs::write(workspace.path().join(".DS_Store"), b"x").unwrap();
        std::fs::write(workspace.path().join("visible.md"), b"# Visible\n").unwrap();
        engine.reconcile().unwrap();

        let entries = engine.list_workspace_entries().unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.relative_path.as_str())
                .collect::<Vec<_>>(),
            vec!["visible.md"]
        );
        assert_eq!(engine.state().indexed_files, 1);

        let mut events = engine.subscribe();
        let external = engine
            .process_external_changes(vec![
                workspace.path().join(".DS_Store"),
                workspace.path().join(".obsidian/workspace.json"),
                workspace.path().join(".obsidian/notes.md"),
            ])
            .unwrap();
        assert!(external.is_empty());
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn a_changed_file_becomes_visible_once_the_watcher_reports_it() {
        let (workspace, _app, engine) = open();
        engine.reconcile().unwrap();
        assert!(!engine.dirty.load(Ordering::SeqCst));
        std::fs::write(workspace.path().join("late.md"), "# Late\n").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !engine.dirty.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(engine.dirty.load(Ordering::SeqCst));
        let files = engine.list_non_managed_markdown().unwrap();
        assert!(files.iter().any(|file| file.relative_path == "late.md"));
    }

    #[test]
    fn autosave_sees_an_unreported_external_edit_to_its_own_file() {
        let (workspace, _app, engine) = open();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Draft".into(),
                body: "First\n\nSecond\n\nThird\n\nFourth".into(),
                relative_path: Some("draft.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        let object = created.value;
        let path = workspace.path().join("draft.md");
        let text = std::fs::read_to_string(&path).unwrap();
        std::fs::write(&path, text.replace("Fourth", "Fourth External")).unwrap();
        // Do not wait for the watcher: the save must check its own file.
        engine.dirty.store(false, Ordering::SeqCst);
        let result = engine
            .save_managed_draft(ManagedDraftInput {
                id: object.id.clone(),
                base_revision: object.revision.clone(),
                base_title: object.title.clone(),
                base_body: object.body.clone(),
                base_properties: BTreeMap::new(),
                local_title: object.title.clone(),
                local_body: object.body.replace("First", "First Local"),
                local_properties: BTreeMap::new(),
            })
            .unwrap();
        let ManagedDraftResult::Merged { body, .. } = result else {
            panic!("the external edit must be merged, not overwritten");
        };
        assert!(body.contains("Local") && body.contains("External"));
    }

    #[test]
    fn repeated_autosaves_from_returned_objects_never_conflict() {
        let (_workspace, _app, engine) = open();
        let mut current = note(&engine, "typing.md");
        for step in 0..3 {
            let result = engine
                .save_managed_draft(ManagedDraftInput {
                    id: current.id.clone(),
                    base_revision: current.revision.clone(),
                    base_title: current.title.clone(),
                    base_body: current.body.clone(),
                    base_properties: current.properties.clone(),
                    local_title: current.title.clone(),
                    // A trailing newline is not canonical; the file trims it.
                    local_body: format!("{}\nline {step}\n", current.body),
                    local_properties: current.properties.clone(),
                })
                .unwrap();
            current = match result {
                ManagedDraftResult::Unchanged { current } => current,
                other => panic!("step {step} must save cleanly, got {other:?}"),
            };
        }
        assert!(current.body.ends_with("line 2"));
    }

    #[test]
    fn a_watcher_overflow_rescans_and_reports_the_changed_paths() {
        let (workspace, _app, engine) = open();
        engine.reconcile().unwrap();
        std::fs::write(workspace.path().join("missed.md"), "# Missed\n").unwrap();
        // Drop whatever the native watcher queued, then simulate the loss.
        let _ = engine
            .watcher
            .drain_coalesced(std::time::Duration::from_millis(200));
        engine.watcher.simulate_overflow();
        let mut events = engine.subscribe();
        let changed = engine
            .poll_external_changes(std::time::Duration::from_millis(1))
            .unwrap();
        assert_eq!(changed, vec!["missed.md".to_owned()]);
        let mut types = Vec::new();
        while let Ok(event) = events.try_recv() {
            types.push(event.event_type);
        }
        assert!(types.contains(&"file:changed".to_owned()));
    }

    #[test]
    fn bulk_external_changes_arrive_as_one_batched_event() {
        let (workspace, _app, engine) = open();
        engine.reconcile().unwrap();
        for index in 0..(OBJECT_EVENT_BATCH_LIMIT + 8) {
            let id = crate::new_object_id("note");
            std::fs::write(
                workspace.path().join(format!("bulk-{index}.md")),
                format!("---\nid: {id}\ntype: note\n---\n\n# Bulk {index}\n"),
            )
            .unwrap();
        }
        let mut events = engine.subscribe();
        engine.reconcile().unwrap();
        let mut received = Vec::new();
        while let Ok(event) = events.try_recv() {
            received.push(event);
        }
        let batches = received
            .iter()
            .filter(|event| event.event_type == "objects:changed")
            .collect::<Vec<_>>();
        assert_eq!(batches.len(), 1);
        assert_eq!(
            batches[0].payload["changes"].as_array().unwrap().len(),
            OBJECT_EVENT_BATCH_LIMIT + 8
        );
        assert_eq!(batches[0].payload["changes"][0]["event"], "object:created");
        assert!(
            !received
                .iter()
                .any(|event| event.event_type == "object:created")
        );
    }

    #[test]
    fn index_candidates_exclude_dot_paths_but_keep_icloud_placeholders() {
        for (path, expected) in [
            ("note.md", true),
            ("folder/note.md", true),
            ("folder/.note.md.icloud", true),
            (".DS_Store", false),
            (".obsidian/workspace.json", false),
            (".obsidian/.note.md.icloud", false),
            ("folder/.hidden.md", false),
            ("node_modules/pkg/readme.md", false),
            ("target/doc.md", false),
            ("docs/target/doc.md", true),
        ] {
            assert_eq!(is_index_candidate(Path::new(path)), expected, "{path}");
        }
    }
}
