//! Reconciliation keeps the index in step with the files: full walks,
//! watcher-reported paths, and the object events that follow.

use super::*;

/// More object changes than this in one reconciliation go out as a single
/// `objects:changed` event, so bulk edits cannot flood the event channel.
pub(super) const OBJECT_EVENT_BATCH_LIMIT: usize = 32;

impl WorkspaceEngine {
    pub fn rebuild_index(&self) -> Result<()> {
        // A write that lands mid-rebuild would update the index this
        // replaces and be lost.
        let _changes = self.hold_changes();
        self.emit(
            "workspace:rebuilding",
            EventSource::Application,
            serde_json::json!({}),
        );
        // A rebuild that stopped midway may have left a partial database
        // and its journal behind.
        let next = self.index_path.with_extension("sqlite.next");
        super::remove_index_files(&next, "index_rebuild")?;
        let mut replacement = IndexStore::open(&next)?;
        scan_into(&self.root, &self.current_ignore(), &mut replacement)?;
        drop(replacement);
        let previous = self.index_path.with_extension("sqlite.previous");
        super::remove_index_files(&previous, "index_rebuild")?;
        // Close the active connection before replacing the database. Windows does not allow an
        // open SQLite file to be renamed, while Unix happens to tolerate it.
        let mut index = self.index.lock().map_err(|_| lock_error("index_rebuild"))?;
        let active = std::mem::replace(&mut *index, IndexStore::in_memory()?);
        drop(active);
        if self.index_path.exists()
            && let Err(error) = std::fs::rename(&self.index_path, &previous)
        {
            *index = IndexStore::open(&self.index_path)?;
            return Err(CoreError::io(
                error,
                "index_rebuild",
                self.index_path.to_str(),
            ));
        }
        if let Err(error) = std::fs::rename(&next, &self.index_path) {
            if previous.exists() {
                let _ = std::fs::rename(&previous, &self.index_path);
            }
            *index = IndexStore::open(&self.index_path)?;
            return Err(CoreError::io(error, "index_rebuild", next.to_str()));
        }
        *index = IndexStore::open(&self.index_path)?;
        drop(index);
        let _ = super::remove_index_files(&previous, "index_rebuild");
        self.emit(
            "workspace:ready",
            EventSource::Application,
            serde_json::json!({ "rebuilt": true }),
        );
        Ok(())
    }

    /// Walk the whole workspace and bring the index in line with the files.
    pub fn reconcile(&self) -> Result<()> {
        self.reconcile_forced_with_source(&HashSet::new(), EventSource::Reconciliation)
    }

    /// Walk after a file operation this app performed, so the resulting events
    /// read as the app's own changes rather than edits from another program.
    pub(super) fn reconcile_as_application(&self) -> Result<()> {
        self.reconcile_forced_with_source(&HashSet::new(), EventSource::Application)
    }

    /// Whether the native watcher reports changes. When it doesn't, callers
    /// must rescan the workspace to see changes other apps make.
    pub fn is_watching(&self) -> bool {
        self.watcher.is_available()
    }

    /// Test hook: make the native watcher fail from now on.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn force_watch_fault(&self, fault: crate::WatchFault) {
        self.watcher.force_fault(fault);
    }

    /// Test hook: deliver one native watcher event naming `paths`, which
    /// may lie anywhere, as the platform would.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn inject_watch_event(&self, paths: Vec<PathBuf>) {
        self.watcher.inject(paths);
    }

    /// Test hook: deliver one native watcher event that asks for a full
    /// rescan, as the platform would.
    #[cfg(any(test, feature = "test-hooks"))]
    pub fn inject_watch_rescan_request(&self) {
        self.watcher.inject_rescan_request();
    }

    /// Whether a read must walk the workspace before trusting the index:
    /// the watcher is down, or it saw a change nobody reconciled yet.
    pub(super) fn needs_reconcile(&self) -> bool {
        !self.watcher.is_available() || self.dirty.load(Ordering::SeqCst)
    }

    /// Reconcile only when the watcher reported a change since the last
    /// walk. Reads use this; mutations that move files walk unconditionally.
    pub(super) fn reconcile_if_needed(&self) -> Result<()> {
        if self.needs_reconcile() {
            self.reconcile()
        } else {
            self.recover_pending_chat_mutations()
        }
    }

    pub(crate) fn reconcile_forced_with_source(
        &self,
        forced: &HashSet<String>,
        source: EventSource,
    ) -> Result<()> {
        self.reconcile_walk(forced, source, false).map(|_| ())
    }

    /// One walk that updates the index, records what could not be read, and
    /// optionally lists every visible entry for the file browser.
    pub(super) fn reconcile_walk(
        &self,
        forced: &HashSet<String>,
        source: EventSource,
        list_entries: bool,
    ) -> Result<WorkspaceScan> {
        // A write of ours never lands mid-walk, so the walk can't index it
        // as another app's.
        let _changes = self.hold_changes();
        self.recover_pending_chat_mutations()?;
        // Clear before walking: a change that lands mid-walk marks it again.
        self.dirty.store(false, Ordering::SeqCst);
        let metadata = self
            .index
            .lock()
            .map_err(|_| lock_error("workspace_reconcile"))?
            .file_metadata()?;
        let mut scan = scan_workspace(
            &self.root,
            &self.current_ignore(),
            Some((&metadata, forced)),
            list_entries,
        )?;
        let removed = scan.removed(&metadata);
        self.store_scan_state(&scan);
        let changed = std::mem::take(&mut scan.changed);
        scan.touched = changed
            .iter()
            .map(|(path, ..)| path.clone())
            .chain(removed.iter().cloned())
            .collect();
        if let Err(error) = self.apply_index_changes(changed, removed, source) {
            self.dirty.store(true, Ordering::SeqCst);
            return Err(error);
        }
        Ok(scan)
    }

    fn store_scan_state(&self, scan: &WorkspaceScan) {
        if let Ok(mut state) = self.scan_state.lock() {
            state.diagnostics = scan.diagnostics.clone();
            state.not_downloaded = scan.not_downloaded.clone();
        }
    }

    /// Re-read specific Markdown files and update only their index rows.
    /// A missing file counts as removed unless iCloud still holds it.
    pub(super) fn reconcile_paths(&self, paths: &[String], source: EventSource) -> Result<()> {
        let _changes = self.hold_changes();
        let mut changed = Vec::new();
        let mut removed = Vec::new();
        for relative in paths {
            let path = self.root.join(relative);
            match std::fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() => {
                    if is_dataless(&metadata) {
                        continue;
                    }
                    match std::fs::read(&path) {
                        Ok(bytes) => {
                            let parsed = markdown::parse_markdown(relative, &bytes);
                            changed.push((relative.clone(), bytes, modified_ns(&metadata), parsed));
                        }
                        Err(error) => {
                            tracing::warn!(path = %relative, %error, "a Markdown file could not be read");
                        }
                    }
                }
                Ok(_) => removed.push(relative.clone()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    if !icloud_placeholder(&path).exists() {
                        removed.push(relative.clone());
                    }
                }
                Err(error) => {
                    tracing::warn!(path = %relative, %error, "a Markdown file could not be inspected");
                }
            }
        }
        self.apply_index_changes(changed, removed, source)
    }

    /// Write scanned changes into the index and announce the objects whose
    /// identity, location, or revision changed. Only the touched stable IDs
    /// are compared, so a small change costs the same in a large workspace.
    pub(super) fn apply_index_changes(
        &self,
        changed: Vec<MarkdownIndexEntry>,
        removed: Vec<String>,
        source: EventSource,
    ) -> Result<()> {
        if changed.is_empty() && removed.is_empty() {
            return Ok(());
        }
        let mut index = self
            .index
            .lock()
            .map_err(|_| lock_error("workspace_reconcile"))?;
        let touched = changed
            .iter()
            .map(|(path, ..)| path.clone())
            .chain(removed.iter().cloned())
            .collect::<Vec<_>>();
        let mut ids = index.stable_ids_at_paths(&touched)?;
        ids.extend(changed.iter().filter_map(|(_, _, _, parsed)| match parsed {
            ParsedMarkdown::Managed(object) => Some(object.id.clone()),
            _ => None,
        }));
        let before = index.object_heads(&ids)?;
        index.reconcile_markdown(&changed, &removed)?;
        let after = index.object_heads(&ids)?;
        drop(index);
        self.emit_object_changes(&before, &after, source);
        self.emit("search:index-updated", source, serde_json::json!({}));
        Ok(())
    }

    pub fn poll_external_changes(&self, wait: std::time::Duration) -> Result<Vec<String>> {
        let paths = self.watcher.drain_coalesced(wait)?;
        // Our writes finish before these reports are checked against the
        // journal, so a write caught between its journal entry and its
        // disk change never reads as another app's.
        let _changes = self.hold_changes();
        if self.watcher.take_overflow() {
            // Events were lost, so the paths are incomplete: walk everything.
            self.sync_external_manifest(&[self.root.join(WORKSPACE_MANIFEST_PATH)])?;
            let scan = self.reconcile_walk(&HashSet::new(), EventSource::External, false)?;
            if let Ok(mut journal) = self.self_writes.lock() {
                journal.clear();
            }
            self.emit(
                "file:changed",
                EventSource::External,
                serde_json::json!({ "paths": scan.touched, "rescanned": true }),
            );
            return Ok(scan.touched);
        }
        self.process_external_changes(paths)
    }

    pub(super) fn process_external_changes(&self, paths: Vec<PathBuf>) -> Result<Vec<String>> {
        // The manifest sits outside the indexed workspace (it is never a
        // workspace object), so watcher events for `.noura/workspace.yaml` are
        // reconciled directly against the in-memory snapshot instead of
        // `file:changed`. This must run before the ignore set is compiled:
        // an external edit to `ignore` scopes the very scan below.
        self.sync_external_manifest(&paths)?;
        let ignores = compile_workspace_ignores(&self.root, &self.current_ignore())?;
        let mut external = Vec::new();
        // A name with no UTF-8 form can't be indexed or reported by path. The
        // walk below records it as a problem and handles the rest.
        let mut unrepresentable = false;
        let mut journal = self
            .self_writes
            .lock()
            .map_err(|_| lock_error("watcher_poll"))?;
        let now = std::time::Instant::now();
        for path in paths {
            let Ok(relative) = path.strip_prefix(&self.root) else {
                continue;
            };
            if relative.as_os_str().is_empty() {
                // The watcher can report the workspace root itself, which is not
                // an object and must never surface as an external change.
                continue;
            }
            // Dot paths (.DS_Store, .obsidian/workspace.json, editor swap
            // files) are not workspace content; only iCloud placeholders for
            // Markdown files matter.
            if !is_index_candidate(relative)
                || !is_visible_workspace_path(relative, path.is_dir(), &ignores)
            {
                continue;
            }
            let Some(relative) = relative.to_str().map(|value| value.replace('\\', "/")) else {
                unrepresentable = true;
                continue;
            };
            // A report of a path that still shows our recent write is our
            // own. One that went online-only meanwhile is outside, and
            // the check never reads it.
            if journal.reported(&relative, &path, now) {
                continue;
            }
            external.push(relative);
        }
        drop(journal);
        external.sort();
        external.dedup();
        if !external.is_empty() || unrepresentable {
            self.reconcile_forced_with_source(
                &external.iter().cloned().collect(),
                EventSource::External,
            )?;
            self.emit(
                "file:changed",
                EventSource::External,
                serde_json::json!({ "paths": external }),
            );
        }
        Ok(external)
    }

    /// Announce object changes found by reconciliation. A handful go out as
    /// individual `object:*` events; a bulk change (a sync pull, a git
    /// checkout, a first index) goes out as one `objects:changed` event whose
    /// `changes` list carries the same payloads plus their `event` type.
    fn emit_object_changes(
        &self,
        before: &HashMap<String, crate::index::ObjectHead>,
        after: &HashMap<String, crate::index::ObjectHead>,
        source: EventSource,
    ) {
        let mut changes = Vec::new();
        for (id, head) in after {
            match before.get(id) {
                None => changes.push(("object:created", head, None)),
                Some(previous) if previous.relative_path != head.relative_path => {
                    changes.push(("object:moved", head, Some(previous.relative_path.as_str())))
                }
                Some(previous) if previous.revision != head.revision => {
                    changes.push(("object:updated", head, None));
                }
                Some(_) => {}
            }
        }
        for (id, head) in before {
            if !after.contains_key(id) {
                changes.push(("object:deleted", head, None));
            }
        }
        changes.sort_by(|left, right| left.1.id.cmp(&right.1.id));
        let payload = |head: &crate::index::ObjectHead, previous_path: Option<&str>| {
            serde_json::json!({
                "id": head.id,
                "type": head.object_type,
                "path": head.relative_path,
                "previousPath": previous_path,
                "revision": head.revision,
            })
        };
        if changes.len() <= OBJECT_EVENT_BATCH_LIMIT {
            for (event_type, head, previous_path) in changes {
                self.emit(event_type, source, payload(head, previous_path));
            }
            return;
        }
        let changes = changes
            .into_iter()
            .map(|(event_type, head, previous_path)| {
                let mut value = payload(head, previous_path);
                value["event"] = serde_json::Value::String(event_type.into());
                value
            })
            .collect::<Vec<_>>();
        self.emit(
            "objects:changed",
            source,
            serde_json::json!({ "changes": changes }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn invalid_ignore_pattern_keeps_library_text_out_of_the_message() {
        let workspace = tempdir().unwrap();
        let error =
            compile_workspace_ignores(workspace.path(), &["notes/{a,b".into()]).unwrap_err();
        assert_eq!(error.code, "invalid_ignore_pattern");
        assert_eq!(error.message, "A workspace ignore pattern is invalid");
        assert_eq!(
            error.details,
            Some(serde_json::json!({ "pattern": "notes/{a,b" }))
        );
    }

    #[test]
    fn scanner_uses_manifest_ignore_instead_of_gitignore() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        drop(engine);
        let manifest_path = workspace.path().join(WORKSPACE_MANIFEST_PATH);
        let manifest = std::fs::read_to_string(&manifest_path)
            .unwrap()
            .replace("ignore: []", "ignore:\n- ignored/**");
        std::fs::write(&manifest_path, manifest).unwrap();
        std::fs::write(workspace.path().join(".gitignore"), "visible.md\n").unwrap();
        std::fs::create_dir(workspace.path().join("ignored")).unwrap();
        std::fs::write(
            workspace.path().join("visible.md"),
            "# Visible\n\nneedle-visible",
        )
        .unwrap();
        std::fs::write(
            workspace.path().join("ignored/hidden.md"),
            "# Hidden\n\nneedle-hidden",
        )
        .unwrap();
        let engine =
            WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();
        assert_eq!(
            engine
                .search(&SearchInput {
                    query: "needle-visible".into(),
                    ..Default::default()
                })
                .unwrap()
                .len(),
            1
        );
        assert!(
            engine
                .search(&SearchInput {
                    query: "needle-hidden".into(),
                    ..Default::default()
                })
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn a_write_of_ours_the_platform_reports_twice_never_reads_as_an_outside_change() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Ours".into(),
                body: "Body".into(),
                relative_path: Some("ours.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        let path = engine.root().join("ours.md");

        let first = engine.process_external_changes(vec![path.clone()]).unwrap();
        let second = engine.process_external_changes(vec![path.clone()]).unwrap();
        std::fs::write(&path, "# Theirs\n").unwrap();
        let third = engine.process_external_changes(vec![path]).unwrap();

        assert_eq!(
            (first, second, third),
            (Vec::new(), Vec::new(), vec!["ours.md".to_owned()])
        );
    }

    #[test]
    fn reports_of_the_folders_a_write_of_ours_creates_never_read_as_an_outside_change() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let mut events = engine.subscribe();
        engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Ours".into(),
                body: "Body".into(),
                relative_path: Some("Inbox/Deep/ours.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        let root = engine.root().to_owned();

        let ours = engine
            .process_external_changes(vec![
                root.join("Inbox"),
                root.join("Inbox/Deep"),
                root.join("Inbox/Deep/ours.md"),
            ])
            .unwrap();
        std::fs::create_dir(root.join("Inbox/Theirs")).unwrap();
        let theirs = engine
            .process_external_changes(vec![root.join("Inbox"), root.join("Inbox/Theirs")])
            .unwrap();

        let outside = std::iter::from_fn(|| events.try_recv().ok())
            .filter(|event| event.source == EventSource::External)
            .map(|event| event.event_type)
            .collect::<Vec<_>>();
        assert_eq!(
            (ours, theirs, outside),
            (
                Vec::new(),
                vec!["Inbox/Theirs".to_owned()],
                vec!["file:changed".to_owned()]
            )
        );
    }

    #[test]
    fn change_counts_keep_every_change_and_its_origin_when_listeners_fall_behind() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        // A listener that never reads falls behind once the channel fills.
        let mut events = engine.subscribe();
        let before = engine.change_counts();
        for note in 0..300 {
            engine
                .create_object(CreateObjectInput {
                    object_type: "note".into(),
                    title: "Ours".into(),
                    body: String::new(),
                    relative_path: Some(format!("{note}.md")),
                    properties: BTreeMap::new(),
                })
                .unwrap();
        }
        let ours = engine.change_counts();
        std::fs::write(engine.root().join("theirs.md"), "# Theirs\n").unwrap();
        engine.reconcile().unwrap();
        let theirs = engine.change_counts();

        assert!(matches!(
            events.try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Lagged(_))
        ));
        assert_eq!(
            (ours.own - before.own, ours.outside - before.outside),
            (300, 0)
        );
        assert_eq!(theirs.own, ours.own);
        assert!(theirs.outside > ours.outside);
    }

    #[test]
    fn external_change_event_is_emitted_after_markdown_reconciliation() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let path = engine.root().join("watched.md");
        let mut events = engine.subscribe();
        std::fs::write(&path, "# Watched\n\nBody\n").unwrap();

        let changes = engine.process_external_changes(vec![path]).unwrap();
        let received = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let event = received
            .iter()
            .find(|event| event.event_type == "file:changed");

        assert_eq!(
            (
                changes,
                event.map(|value| value.payload["paths"].clone()),
                engine
                    .list_non_managed_markdown()
                    .unwrap()
                    .iter()
                    .any(|file| file.relative_path == "watched.md"),
            ),
            (
                vec!["watched.md".to_owned()],
                Some(serde_json::json!(["watched.md"])),
                true,
            )
        );
    }

    #[test]
    fn external_managed_changes_emit_semantic_object_events() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Watched".into(),
                body: "before".into(),
                relative_path: Some("watched-managed.md".into()),
                properties: BTreeMap::new(),
            })
            .unwrap();
        let path = engine.root().join("watched-managed.md");
        let external = std::fs::read_to_string(&path)
            .unwrap()
            .replace("before", "after");
        std::fs::write(&path, external).unwrap();
        let mut events = engine.subscribe();

        engine.process_external_changes(vec![path]).unwrap();

        let received = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let updated = received
            .iter()
            .find(|event| event.event_type == "object:updated")
            .unwrap();
        assert_eq!(updated.source, EventSource::External);
        assert_eq!(updated.payload["id"], created.value.id);
    }

    #[test]
    fn external_task_priority_edit_reconciles_before_emitting_the_object_event() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Test", app_data.path())
                .unwrap();
        let created = engine
            .create_object(CreateObjectInput {
                object_type: "task".into(),
                title: "Ship beta".into(),
                body: String::new(),
                relative_path: Some("tasks/ship-beta.md".into()),
                properties: BTreeMap::from([
                    ("status".into(), serde_json::json!("todo")),
                    ("priority".into(), serde_json::json!("medium")),
                ]),
            })
            .unwrap();
        let path = engine.root().join(&created.value.relative_path);
        let bytes = std::fs::read_to_string(&path)
            .unwrap()
            .replace("priority: medium", "priority: high");
        std::fs::write(&path, bytes).unwrap();
        let mut events = engine.subscribe();

        engine.process_external_changes(vec![path]).unwrap();

        let current = engine.get_object(&created.value.id).unwrap().unwrap();
        assert_eq!(current.properties["priority"], serde_json::json!("high"));
        let received = std::iter::from_fn(|| events.try_recv().ok()).collect::<Vec<_>>();
        let updated = received
            .iter()
            .find(|event| event.event_type == "object:updated")
            .unwrap();
        assert_eq!(updated.source, EventSource::External);
        assert_eq!(updated.payload["id"], created.value.id);
    }
}
