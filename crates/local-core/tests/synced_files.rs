//! The files the v2 sync client carries (sync-v2.md §9, §15): the listing
//! of what the scanner reports, with the entries sync leaves out, and the
//! writes that apply another device's changes. Each write refuses to
//! overwrite an outside edit, stays inside the workspace, reaches the disk
//! before it returns, and brings the index along.

use local_core::{ManifestUpdateInput, SkipReason, SyncedExpect, WorkspaceEngine};
use tempfile::tempdir;

fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Synced", app_data.path()).unwrap();
    (workspace, app_data, engine)
}

fn revision(bytes: &[u8]) -> String {
    blake3::hash(bytes).to_hex().to_string()
}

const NOTE: &[u8] = b"---\nid: note_01j9z3y7q8r2s4t6v8w0x2y4z6\ntype: note\ncreated: 2026-10-24T12:00:00Z\nupdated: 2026-10-24T12:00:00Z\n---\n# Plan\n";

#[test]
fn the_listing_carries_what_the_scanner_reports_and_names_what_it_skips() {
    let (workspace, _app_data, engine) = engine();
    let root = workspace.path();
    std::fs::create_dir_all(root.join("Notes/Empty")).unwrap();
    std::fs::write(root.join("Notes/Plan.md"), b"# Plan\n").unwrap();
    std::fs::write(root.join("photo.png"), [1, 2, 3]).unwrap();
    std::fs::write(root.join("Notes/.hidden.md"), b"hidden").unwrap();
    std::fs::write(root.join("build.log"), b"log").unwrap();
    std::fs::write(root.join("Notes/.Later.md.icloud"), b"").unwrap();
    engine
        .manifest_update(ManifestUpdateInput {
            ignore: Some(vec!["*.log".into()]),
            ..ManifestUpdateInput::default()
        })
        .unwrap();
    #[cfg(unix)]
    std::os::unix::fs::symlink(root.join("photo.png"), root.join("link.png")).unwrap();

    let listing = engine.synced_listing().unwrap();
    assert!(listing.complete);
    let paths: Vec<&str> = listing
        .files
        .iter()
        .map(|f| f.relative_path.as_str())
        .collect();
    assert_eq!(paths, ["Notes/Plan.md", "photo.png"]);
    let plan = &listing.files[0];
    assert_eq!(plan.size, 7);
    assert!(plan.modified_ns > 0);
    #[cfg(unix)]
    assert!(plan.file_id.is_some());
    let skipped: Vec<(Option<&str>, SkipReason)> = listing
        .skipped
        .iter()
        .map(|s| (s.relative_path.as_deref(), s.reason))
        .collect();
    assert!(skipped.contains(&(Some("Notes/Later.md"), SkipReason::NotDownloaded)));
    #[cfg(unix)]
    assert!(skipped.contains(&(Some("link.png"), SkipReason::Symlink)));
}

#[cfg(unix)]
#[test]
fn a_name_with_no_utf8_form_is_skipped_with_its_folder() {
    use std::os::unix::ffi::OsStrExt;
    let (workspace, _app_data, engine) = engine();
    std::fs::create_dir_all(workspace.path().join("Notes")).unwrap();
    let name = std::ffi::OsStr::from_bytes(b"bad\xff.md");
    // Some file systems refuse such a name; then there is nothing to skip.
    if std::fs::write(workspace.path().join("Notes").join(name), b"x").is_err() {
        return;
    }
    let listing = engine.synced_listing().unwrap();
    assert!(listing.files.is_empty());
    assert_eq!(listing.skipped.len(), 1);
    assert_eq!(listing.skipped[0].reason, SkipReason::UnsupportedName);
    assert_eq!(listing.skipped[0].relative_path.as_deref(), Some("Notes"));
}

#[test]
fn a_write_creates_the_file_and_its_folders_on_disk_and_in_the_index() {
    let (workspace, _app_data, engine) = engine();
    let mut events = engine.subscribe();
    let before = engine.change_counts();
    let (result, synced) = local_core::durability_probe::record(|| {
        engine.write_synced("Notes/Week/Plan.md", NOTE, &SyncedExpect::Absent)
    });
    result.unwrap();
    assert_eq!(
        std::fs::read(workspace.path().join("Notes/Week/Plan.md")).unwrap(),
        NOTE
    );
    // The file's folder, and each folder the write created, reached the disk
    // before the write returned.
    let root = workspace.path().canonicalize().unwrap();
    let synced: Vec<_> = synced
        .iter()
        .map(|p| p.canonicalize().unwrap_or_else(|_| p.clone()))
        .collect();
    for folder in ["Notes/Week", "Notes", ""] {
        assert!(
            synced.contains(&root.join(folder).canonicalize().unwrap()),
            "{folder} wasn't synced: {synced:?}"
        );
    }
    let object = engine
        .get_object("note_01j9z3y7q8r2s4t6v8w0x2y4z6")
        .unwrap()
        .expect("the index holds the note");
    assert_eq!(object.relative_path, "Notes/Week/Plan.md");
    assert!(engine.change_counts().outside > before.outside);
    let mut heard = false;
    while let Ok(event) = events.try_recv() {
        heard |=
            event.event_type == "file:changed" && event.source == local_core::EventSource::Sync;
    }
    assert!(heard, "the views hear of the synced change");
}

#[test]
fn a_write_never_replaces_an_outside_edit() {
    let (workspace, _app_data, engine) = engine();
    let file = workspace.path().join("Plan.md");
    std::fs::write(&file, b"# Mine\n").unwrap();

    let error = engine
        .write_synced("Plan.md", b"# Theirs\n", &SyncedExpect::Absent)
        .unwrap_err();
    assert_eq!(error.code, "revision_conflict");
    let error = engine
        .write_synced(
            "Plan.md",
            b"# Theirs\n",
            &SyncedExpect::Revision(revision(b"# Before\n")),
        )
        .unwrap_err();
    assert_eq!(error.code, "revision_conflict");
    assert_eq!(std::fs::read(&file).unwrap(), b"# Mine\n");

    engine
        .write_synced(
            "Plan.md",
            b"# Theirs\n",
            &SyncedExpect::Revision(revision(b"# Mine\n")),
        )
        .unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), b"# Theirs\n");
}

#[test]
fn a_move_checks_the_bytes_takes_the_file_along_and_leaves_no_empty_folder() {
    let (workspace, _app_data, engine) = engine();
    engine
        .write_synced("Old/Plan.md", NOTE, &SyncedExpect::Absent)
        .unwrap();
    let error = engine
        .move_synced("Old/Plan.md", "New/Plan.md", &revision(b"other"))
        .unwrap_err();
    assert_eq!(error.code, "revision_conflict");
    std::fs::write(workspace.path().join("Taken.md"), b"x").unwrap();
    let error = engine
        .move_synced("Old/Plan.md", "Taken.md", &revision(NOTE))
        .unwrap_err();
    assert_eq!(error.code, "path_exists");

    engine
        .move_synced("Old/Plan.md", "New/Plan.md", &revision(NOTE))
        .unwrap();
    assert!(!workspace.path().join("Old").exists());
    assert_eq!(
        std::fs::read(workspace.path().join("New/Plan.md")).unwrap(),
        NOTE
    );
    let object = engine
        .get_object("note_01j9z3y7q8r2s4t6v8w0x2y4z6")
        .unwrap()
        .expect("indexed");
    assert_eq!(object.relative_path, "New/Plan.md");
}

#[test]
fn a_delete_checks_the_bytes_and_keeps_them_in_the_trash() {
    let (workspace, _app_data, engine) = engine();
    engine
        .write_synced("Notes/Plan.md", NOTE, &SyncedExpect::Absent)
        .unwrap();
    std::fs::write(workspace.path().join("Notes/Plan.md"), b"# Edited\n").unwrap();
    let error = engine
        .delete_synced("Notes/Plan.md", &revision(NOTE))
        .unwrap_err();
    assert_eq!(error.code, "revision_conflict");
    assert!(workspace.path().join("Notes/Plan.md").exists());

    engine
        .delete_synced("Notes/Plan.md", &revision(b"# Edited\n"))
        .unwrap();
    assert!(!workspace.path().join("Notes").exists());
    let trash = workspace.path().join(".noura/trash");
    let kept = walk(&trash);
    assert_eq!(kept.len(), 1);
    assert_eq!(std::fs::read(&kept[0]).unwrap(), b"# Edited\n");
    assert!(
        engine
            .get_object("note_01j9z3y7q8r2s4t6v8w0x2y4z6")
            .unwrap()
            .is_none()
    );
}

#[test]
fn every_write_stays_inside_the_workspace() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(workspace.path().join("Plan.md"), b"x").unwrap();
    for path in [
        "../outside.md",
        "/etc/hosts",
        ".noura/sync/state",
        ".git/config",
    ] {
        let error = engine
            .write_synced(path, b"x", &SyncedExpect::Absent)
            .unwrap_err();
        assert!(
            ["unsafe_path", "absolute_path", "reserved_path"].contains(&error.code.as_str()),
            "{path}: {}",
            error.code
        );
        assert!(engine.read_synced(path).is_err(), "{path}");
        assert!(
            engine
                .move_synced("Plan.md", path, &revision(b"x"))
                .is_err()
        );
        assert!(engine.delete_synced(path, &revision(b"x")).is_err());
    }
    #[cfg(unix)]
    {
        let outside = tempdir().unwrap();
        std::os::unix::fs::symlink(outside.path(), workspace.path().join("Linked")).unwrap();
        let error = engine
            .write_synced("Linked/Plan.md", b"x", &SyncedExpect::Absent)
            .unwrap_err();
        assert_eq!(error.code, "symlink_escape");
        assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
    }
}

#[test]
fn a_read_returns_the_bytes_or_nothing() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(workspace.path().join("Plan.md"), b"# Plan\n").unwrap();
    assert_eq!(engine.read_synced("Plan.md").unwrap().unwrap(), b"# Plan\n");
    assert!(engine.read_synced("Missing.md").unwrap().is_none());
    std::fs::write(workspace.path().join(".Cloud.md.icloud"), b"").unwrap();
    assert_eq!(
        engine.read_synced("Cloud.md").unwrap_err().code,
        "file_not_downloaded"
    );
}

fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_owned()];
    while let Some(next) = stack.pop() {
        for entry in std::fs::read_dir(&next).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}
