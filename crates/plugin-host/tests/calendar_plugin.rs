//! Seam 3: the Calendar plugin's manifest in the real host. Calendar reads
//! the workspace, adds a page and an Inbox section, and declares nothing
//! that writes, so code running under its capabilities can't write an
//! object. A file that fails to read is read again on the next draw, and
//! a file read before the index saw an edit is read again once the
//! listing's revision differs from the bytes read. The app-core suite
//! runs the plugin against real files.

mod support;

use std::sync::{Arc, Mutex};

use plugin_host::{
    Capability, ClockReading, DeviceClock, Entry, EntryKind, FileRead, Listed, NodeKind,
    ObjectRead, RunningPlugin, ServiceError, Services, WorkspacePath, WorkspaceReads,
    read_manifest,
};
use support::plugins;

/// The Calendar plugin's manifest, as its crate ships it.
const MANIFEST: &str = include_str!("../../plugins/calendar/plugin.toml");

#[test]
fn calendar_declares_workspace_reads_a_view_and_an_inbox_section_and_nothing_that_writes() {
    let Some(calendar) = plugins::built("calendar_plugin") else {
        return;
    };
    let folder = tempfile::tempdir().unwrap();
    plugins::write_folder(folder.path(), MANIFEST, "calendar_plugin.wasm", &calendar);

    let manifest = read_manifest(folder.path()).unwrap();

    assert_eq!(manifest.id, "calendar");
    let names: Vec<&str> = manifest.capabilities.iter().map(Capability::name).collect();
    assert_eq!(names, ["workspace-read", "views", "inbox-sections"]);
}

#[test]
fn code_running_under_the_calendar_capabilities_is_denied_a_write() {
    let Some(probe) = plugins::built("probe_plugin") else {
        return;
    };
    // The probe's code under the Calendar manifest: the same ID and the
    // same capabilities, approved in full.
    let folder = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    plugins::write_folder(folder.path(), MANIFEST, "calendar_plugin.wasm", &probe);
    let host = plugins::host(library.path());
    host.install(&host.inspect(folder.path()).unwrap()).unwrap();
    let loaded = host.load("calendar").unwrap();
    let plugin = host.start(
        &loaded,
        &loaded.manifest().capabilities,
        Services::default(),
    );

    let nodes = plugin.render("create-object:note:Plan").unwrap();

    assert_eq!(
        plugins::texts(&nodes),
        ["error capability-not-declared object-types"]
    );
}

/// 11:00 on Wednesday 21 October 2026 in Toronto.
struct FixedClock;

impl DeviceClock for FixedClock {
    fn now(&self) -> ClockReading {
        ClockReading {
            instant: "2026-10-21T15:00:00Z".into(),
            time_zone: "America/Toronto".into(),
        }
    }
}

/// A workspace holding one note, `Plan.md`, whose listing, bytes and
/// readability a test changes as it goes.
struct ChangingWorkspace {
    /// The revision the index lists for the file.
    listed: Mutex<String>,
    /// The object the file holds now.
    object: Mutex<ObjectRead>,
    /// Whether a read fails, as for a file that lives only in the cloud.
    unreadable: Mutex<bool>,
    reads: Mutex<usize>,
}

impl ChangingWorkspace {
    fn new(revision: &str, date: &str) -> Self {
        Self {
            listed: Mutex::new(revision.to_owned()),
            object: Mutex::new(plan(revision, date)),
            unreadable: Mutex::new(false),
            reads: Mutex::new(0),
        }
    }

    fn reads(&self) -> usize {
        *self.reads.lock().unwrap()
    }
}

/// The note `Plan.md` at `revision`, dated `date`.
fn plan(revision: &str, date: &str) -> ObjectRead {
    ObjectRead {
        id: "note_01jbcdefghjkmnpqrstvwxyz0a".into(),
        object_type: "note".into(),
        title: "Plan".into(),
        path: WorkspacePath::new("Plan.md").unwrap(),
        body: String::new(),
        properties: format!("{{\"date\":\"{date}\"}}"),
        revision: revision.into(),
    }
}

impl WorkspaceReads for ChangingWorkspace {
    fn list_entries(&self) -> Result<Vec<Listed>, ServiceError> {
        Ok(vec![Listed::Entry(Entry {
            path: WorkspacePath::new("Plan.md").unwrap(),
            kind: EntryKind::File,
            object_id: Some("note_01jbcdefghjkmnpqrstvwxyz0a".into()),
            object_type: Some("note".into()),
            revision: Some(self.listed.lock().unwrap().clone()),
        })])
    }

    fn read_file(&self, path: &WorkspacePath) -> Result<FileRead, ServiceError> {
        Err(ServiceError::NotFound(path.to_string()))
    }

    fn read_object(&self, id: &str) -> Result<ObjectRead, ServiceError> {
        Err(ServiceError::NotFound(id.to_owned()))
    }

    fn read_object_at(&self, _path: &WorkspacePath) -> Result<ObjectRead, ServiceError> {
        *self.reads.lock().unwrap() += 1;
        if *self.unreadable.lock().unwrap() {
            return Err(ServiceError::Failed(
                "The file isn't downloaded from the cloud.".into(),
            ));
        }
        Ok(self.object.lock().unwrap().clone())
    }
}

/// The Calendar plugin as it ships, reading `workspace` on the fixed
/// clock, and the folders it lives in.
fn calendar(
    workspace: Arc<ChangingWorkspace>,
) -> Option<(Arc<RunningPlugin>, [tempfile::TempDir; 2])> {
    let calendar = plugins::built("calendar_plugin")?;
    let folder = tempfile::tempdir().unwrap();
    let library = tempfile::tempdir().unwrap();
    plugins::write_folder(folder.path(), MANIFEST, "calendar_plugin.wasm", &calendar);
    let host = plugins::host(library.path());
    host.install(&host.inspect(folder.path()).unwrap()).unwrap();
    let loaded = host.load("calendar").unwrap();
    let plugin = host.start(
        &loaded,
        &loaded.manifest().capabilities,
        Services {
            workspace: Some(workspace),
            clock: Some(Arc::new(FixedClock)),
            ..Services::default()
        },
    );
    Some((plugin, [folder, library]))
}

/// The starts of the entries the Calendar page draws now.
fn starts(plugin: &RunningPlugin) -> Vec<String> {
    plugin
        .render("calendar")
        .unwrap()
        .into_iter()
        .filter_map(|node| match node.kind {
            NodeKind::Entry(entry) => Some(entry.start),
            _ => None,
        })
        .collect()
}

#[test]
fn a_file_that_failed_to_read_shows_its_date_once_it_reads_with_the_same_bytes() {
    let workspace = Arc::new(ChangingWorkspace::new("rev-1", "2026-10-21"));
    *workspace.unreadable.lock().unwrap() = true;
    let Some((plugin, _folders)) = calendar(workspace.clone()) else {
        return;
    };

    assert!(starts(&plugin).is_empty());
    assert_eq!(workspace.reads(), 1);

    // The file downloads again with the same bytes, so the index's
    // revision never changes.
    *workspace.unreadable.lock().unwrap() = false;
    assert_eq!(starts(&plugin), ["2026-10-21"]);
    assert_eq!(workspace.reads(), 2);

    // A read that succeeded isn't repeated while the revision holds.
    assert_eq!(starts(&plugin), ["2026-10-21"]);
    assert_eq!(workspace.reads(), 2);
}

#[test]
fn a_file_that_fails_to_read_after_an_edit_keeps_its_last_date_until_it_reads() {
    let workspace = Arc::new(ChangingWorkspace::new("rev-1", "2026-10-21"));
    let Some((plugin, _folders)) = calendar(workspace.clone()) else {
        return;
    };
    assert_eq!(starts(&plugin), ["2026-10-21"]);

    // The file changes and then leaves the device before Calendar reads it.
    *workspace.object.lock().unwrap() = plan("rev-2", "2026-10-22");
    *workspace.listed.lock().unwrap() = "rev-2".into();
    *workspace.unreadable.lock().unwrap() = true;
    assert_eq!(starts(&plugin), ["2026-10-21"]);

    *workspace.unreadable.lock().unwrap() = false;
    assert_eq!(starts(&plugin), ["2026-10-22"]);
}

#[test]
fn a_file_read_before_the_index_saw_an_edit_reads_again_when_the_edit_is_undone() {
    // The index still lists the first revision, but the file already
    // holds an edit that moved the date.
    let workspace = Arc::new(ChangingWorkspace::new("rev-1", "2026-10-21"));
    *workspace.object.lock().unwrap() = plan("rev-2", "2026-10-22");
    let Some((plugin, _folders)) = calendar(workspace.clone()) else {
        return;
    };
    assert_eq!(starts(&plugin), ["2026-10-22"]);

    // The edit is undone before the index looks, so the listing never
    // changes.
    *workspace.object.lock().unwrap() = plan("rev-1", "2026-10-21");
    assert_eq!(starts(&plugin), ["2026-10-21"]);
    assert_eq!(workspace.reads(), 2);
    assert_eq!(starts(&plugin), ["2026-10-21"]);
    assert_eq!(workspace.reads(), 2);
}
