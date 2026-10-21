//! Seam 1: Noura never reads a file that lives only in the cloud, since
//! reading it would start a download and block. Every operation that needs
//! the file's bytes stops with `file_not_downloaded` and changes nothing.

use std::path::Path;

use local_core::{CreateObjectInput, ObjectPatch, WorkspaceEngine};
use tempfile::tempdir;

fn engine() -> (tempfile::TempDir, tempfile::TempDir, WorkspaceEngine) {
    let workspace = tempdir().unwrap();
    let app_data = tempdir().unwrap();
    let engine =
        WorkspaceEngine::create_with_app_data(workspace.path(), "Cloud tests", app_data.path())
            .unwrap();
    (workspace, app_data, engine)
}

/// Create a note at `relative` and return its ID and revision.
fn note(engine: &WorkspaceEngine, relative: &str) -> (String, String) {
    let created = engine
        .create_object(CreateObjectInput {
            object_type: "note".into(),
            title: "Plan".into(),
            body: "The plan.\n".into(),
            relative_path: Some(relative.into()),
            properties: Default::default(),
        })
        .unwrap();
    (created.value.id, created.revision)
}

/// Every file and folder under `root` outside `.noura`, sorted.
fn listing(root: &Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut folders = vec![root.to_owned()];
    while let Some(folder) = folders.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            let relative = path.strip_prefix(root).unwrap().to_string_lossy();
            if relative == ".noura" {
                continue;
            }
            found.push(relative.replace('\\', "/"));
            if path.is_dir() {
                folders.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Run each operation that reads `relative`, the note `id` at `revision`,
/// and check that each one stops with `file_not_downloaded` and leaves the
/// workspace as it was.
fn assert_never_read(
    engine: &WorkspaceEngine,
    root: &Path,
    relative: &str,
    id: &str,
    revision: &str,
) {
    let before = listing(root);
    let errors = [
        ("copy", engine.copy_file(relative, "Copy.md").err()),
        ("move", engine.move_object(id, "Moved.md", revision).err()),
        ("delete", engine.delete_object(id, revision).err()),
        (
            "update",
            engine
                .update_object(
                    id,
                    ObjectPatch {
                        title: Some("Changed".into()),
                        body: None,
                        properties: Default::default(),
                        remove_properties: Vec::new(),
                        expected_revision: revision.into(),
                    },
                )
                .err(),
        ),
        ("read", engine.read_raw_markdown(relative).err()),
    ];
    for (operation, error) in errors {
        let error = error.unwrap_or_else(|| panic!("{operation} went ahead"));
        assert_eq!(error.code, "file_not_downloaded", "{operation}: {error:?}");
        assert_eq!(error.path.as_deref(), Some(relative), "{operation}");
        assert!(error.retryable, "{operation}");
    }
    assert_eq!(listing(root), before);
    assert_eq!(
        engine.get_object(id).unwrap().unwrap().relative_path,
        relative
    );
}

#[test]
fn a_note_only_an_icloud_placeholder_stands_for_is_never_read() {
    let (workspace, _app_data, engine) = engine();
    let (id, revision) = note(&engine, "Plan.md");
    // iCloud swaps the file for a placeholder named after it.
    std::fs::remove_file(workspace.path().join("Plan.md")).unwrap();
    std::fs::write(workspace.path().join(".Plan.md.icloud"), b"bplist00").unwrap();
    engine.reconcile().unwrap();

    assert_never_read(&engine, workspace.path(), "Plan.md", &id, &revision);
}

#[test]
fn copying_any_file_only_an_icloud_placeholder_stands_for_reads_nothing() {
    let (workspace, _app_data, engine) = engine();
    std::fs::write(workspace.path().join(".Scan.pdf.icloud"), b"bplist00").unwrap();

    let error = engine.copy_file("Scan.pdf", "Scan 1.pdf").unwrap_err();

    assert_eq!(error.code, "file_not_downloaded");
    assert_eq!(listing(workspace.path()), [".Scan.pdf.icloud"]);
}

/// Set the attribute a cloud provider gives a file whose content lives
/// only online. It is the one such mark a test can set without a cloud
/// sync root, and Windows still lets the test read the file, so an
/// operation that went ahead would mean Noura read it.
#[cfg(windows)]
fn set_online_only(path: &Path) {
    use std::os::windows::ffi::OsStrExt;

    use windows_sys::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_OFFLINE, GetFileAttributesW, INVALID_FILE_ATTRIBUTES, SetFileAttributesW,
    };

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
}

#[cfg(windows)]
#[test]
fn a_note_the_cloud_provider_marks_online_only_is_never_read() {
    let (workspace, _app_data, engine) = engine();
    let (id, revision) = note(&engine, "Plan.md");
    std::fs::write(workspace.path().join("Scan.pdf"), b"%PDF-1.7\n").unwrap();
    set_online_only(&workspace.path().join("Plan.md"));
    set_online_only(&workspace.path().join("Scan.pdf"));

    assert_never_read(&engine, workspace.path(), "Plan.md", &id, &revision);
    let error = engine.copy_file("Scan.pdf", "Scan 1.pdf").unwrap_err();
    assert_eq!(error.code, "file_not_downloaded");
    assert!(!workspace.path().join("Scan 1.pdf").exists());
}
