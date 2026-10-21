//! Seam 1: the workspace service against real folders on disk.

use std::path::Path;

use app_core::WorkspaceService;

fn manifest(folder: &Path) -> String {
    std::fs::read_to_string(folder.join(".noura/workspace.yaml")).unwrap()
}

#[test]
fn new_workspace_writes_a_manifest_with_a_new_id_and_the_name() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(app_data.path());

    let workspace = service.create("Research", folder.path()).unwrap();

    let on_disk = manifest(folder.path());
    assert!(on_disk.contains("name: Research"), "{on_disk}");
    assert!(workspace.id().starts_with("workspace_"));
    assert!(
        on_disk.contains(&format!("id: {}", workspace.id())),
        "{on_disk}"
    );
    assert_eq!(workspace.name(), "Research");
}

#[test]
fn new_workspace_leaves_existing_files_byte_for_byte() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let note = b"---\ntitle: kept\n---\r\nCRLF body without final newline";
    std::fs::write(folder.path().join("Note.md"), note).unwrap();
    std::fs::create_dir(folder.path().join("Sub")).unwrap();
    std::fs::write(folder.path().join("Sub/data.bin"), [0u8, 159, 146, 150]).unwrap();

    WorkspaceService::new(app_data.path())
        .create("Notes", folder.path())
        .unwrap();

    assert_eq!(std::fs::read(folder.path().join("Note.md")).unwrap(), note);
    assert_eq!(
        std::fs::read(folder.path().join("Sub/data.bin")).unwrap(),
        [0u8, 159, 146, 150]
    );
}

#[test]
fn new_workspace_with_a_blank_name_changes_nothing() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Not yet");

    let error = WorkspaceService::new(app_data.path())
        .create("   ", &folder)
        .err()
        .unwrap();

    assert_eq!(error.code, "workspace_name_required");
    assert_eq!(error.message, "Enter a name for the workspace.");
    assert!(!folder.exists(), "a failed create must not make the folder");
}

#[test]
fn new_workspace_where_one_exists_changes_nothing_and_says_so() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(app_data.path());
    drop(service.create("First", folder.path()).unwrap());
    let before = manifest(folder.path());

    let error = service.create("Second", folder.path()).err().unwrap();

    assert_eq!(error.code, "workspace_exists");
    assert_eq!(
        error.message,
        "This folder already has a workspace. Use Open folder to open it."
    );
    assert_eq!(manifest(folder.path()), before);
}

#[test]
fn open_folder_opens_an_existing_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(app_data.path());
    let id = service
        .create("Existing", folder.path())
        .unwrap()
        .id()
        .to_owned();
    let before = manifest(folder.path());

    let workspace = service.open_folder(folder.path()).unwrap();

    assert_eq!(
        (workspace.id(), workspace.name().as_str()),
        (id.as_str(), "Existing")
    );
    assert_eq!(manifest(folder.path()), before);
}

#[test]
fn open_folder_opens_the_workspace_now_in_a_folder_recents_remember_differently() {
    let app_data = tempfile::tempdir().unwrap();
    let other_app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Reused");
    std::fs::create_dir(&folder).unwrap();
    let service = WorkspaceService::new(app_data.path());
    service.create("Old", &folder).unwrap();
    rename_with_retry(&folder, &parent.path().join("Old moved"));
    std::fs::create_dir(&folder).unwrap();
    let new_id = WorkspaceService::new(other_app_data.path())
        .create("New", &folder)
        .unwrap()
        .id()
        .to_owned();

    let workspace = service.open_folder(&folder).unwrap();

    assert_eq!(
        (workspace.id(), workspace.name().as_str()),
        (new_id.as_str(), "New")
    );
    assert_eq!(service.recents()[0].workspace_id, new_id);
}

#[test]
fn open_folder_gives_a_folder_that_lost_its_manifest_a_new_id() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Known folder");
    std::fs::create_dir(&folder).unwrap();
    let service = WorkspaceService::new(app_data.path());
    let id = service.create("Known", &folder).unwrap().id().to_owned();
    std::fs::remove_file(folder.join(".noura/workspace.yaml")).unwrap();

    let workspace = service.open_folder(&folder).unwrap();

    assert_ne!(workspace.id(), id);
    assert_eq!(workspace.name(), "Known folder");
    assert!(manifest(&folder).contains(&format!("id: {}", workspace.id())));
}

#[test]
fn open_folder_never_lends_a_remembered_id_to_another_folder_at_the_same_path() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Projects");
    std::fs::create_dir(&folder).unwrap();
    let service = WorkspaceService::new(app_data.path());
    let old_id = service.create("Old", &folder).unwrap().id().to_owned();
    let archive = parent.path().join("Projects archive");
    rename_with_retry(&folder, &archive);
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("unrelated.md"), b"x").unwrap();

    let workspace = service.open_folder(&folder).unwrap();

    assert_ne!(workspace.id(), old_id);
    assert_eq!(workspace.name(), "Projects");
    assert!(manifest(&archive).contains(&format!("id: {old_id}")));
    assert_eq!(recent_ids(&service), [workspace.id().to_owned(), old_id]);
}

#[test]
fn open_folder_adds_a_manifest_named_after_a_plain_folder() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Field notes");
    std::fs::create_dir(&folder).unwrap();
    let note = b"# Kept\n\nNo final newline";
    std::fs::write(folder.join("Kept.md"), note).unwrap();
    std::fs::write(folder.join(".hidden"), b"dot").unwrap();

    let workspace = WorkspaceService::new(app_data.path())
        .open_folder(&folder)
        .unwrap();

    assert_eq!(workspace.name(), "Field notes");
    assert!(manifest(&folder).contains(&format!("id: {}", workspace.id())));
    assert_eq!(std::fs::read(folder.join("Kept.md")).unwrap(), note);
    assert_eq!(std::fs::read(folder.join(".hidden")).unwrap(), b"dot");
}

#[test]
fn open_folder_with_an_invalid_manifest_opens_nothing_and_keeps_its_bytes() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    std::fs::create_dir(folder.path().join(".noura")).unwrap();
    let broken = b"id: [this is not\nname: valid yaml";
    std::fs::write(folder.path().join(".noura/workspace.yaml"), broken).unwrap();

    let error = WorkspaceService::new(app_data.path())
        .open_folder(folder.path())
        .err()
        .unwrap();

    assert_eq!(error.code, "invalid_workspace_manifest");
    assert_eq!(
        error.message,
        "Noura can't read the settings file .noura/workspace.yaml, so it left the file as it was."
    );
    assert_eq!(
        std::fs::read(folder.path().join(".noura/workspace.yaml")).unwrap(),
        broken
    );
    assert!(!folder.path().join(".noura/trash").exists());
}

#[test]
fn open_folder_with_an_unsupported_format_version_opens_nothing_and_keeps_its_bytes() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    std::fs::create_dir(folder.path().join(".noura")).unwrap();
    let future = b"id: workspace_01j00000000000000000000000\nformat_version: 2\nname: Future\ncreated: 2026-08-27T12:00:00Z\nupdated: 2026-08-27T12:00:00Z\nenabled_plugins: []\nignore: []\n";
    std::fs::write(folder.path().join(".noura/workspace.yaml"), future).unwrap();

    let error = WorkspaceService::new(app_data.path())
        .open_folder(folder.path())
        .err()
        .unwrap();

    assert_eq!(error.code, "unsupported_workspace_version");
    assert_eq!(
        error.message,
        "This version of Noura can't read this workspace's format, so it left the files as they were."
    );
    assert_eq!(
        std::fs::read(folder.path().join(".noura/workspace.yaml")).unwrap(),
        future
    );
}

#[cfg(unix)]
#[test]
fn open_folder_noura_cannot_represent_returns_unsupported_path() {
    use std::os::unix::ffi::OsStrExt;
    let app_data = tempfile::tempdir().unwrap();
    let folder = std::path::PathBuf::from(std::ffi::OsStr::from_bytes(b"/tmp/not \xff utf-8"));

    let error = WorkspaceService::new(app_data.path())
        .open_folder(&folder)
        .err()
        .unwrap();

    assert_eq!(error.code, "unsupported_path");
}

#[cfg(windows)]
#[test]
fn open_folder_noura_cannot_represent_returns_unsupported_path() {
    use std::os::windows::ffi::OsStringExt;
    let app_data = tempfile::tempdir().unwrap();
    // An unpaired surrogate has no UTF-8 form.
    let mut wide: Vec<u16> = "C:\\not ".encode_utf16().collect();
    wide.push(0xD800);
    let folder = std::path::PathBuf::from(std::ffi::OsString::from_wide(&wide));

    let error = WorkspaceService::new(app_data.path())
        .open_folder(&folder)
        .err()
        .unwrap();

    assert_eq!(error.code, "unsupported_path");
}

fn recent_ids(service: &WorkspaceService) -> Vec<String> {
    service
        .recents()
        .into_iter()
        .map(|recent| recent.workspace_id)
        .collect()
}

#[test]
fn recents_list_the_most_recent_first_once_per_workspace_id() {
    let app_data = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(app_data.path());
    let a = service.create("A", first.path()).unwrap().id().to_owned();
    let b = service.create("B", second.path()).unwrap().id().to_owned();

    service.open_folder(first.path()).unwrap();

    assert_eq!(recent_ids(&service), [a.clone(), b]);
    let recent = &service.recents()[0];
    assert_eq!(recent.name, "A");
    assert_eq!(
        std::path::Path::new(&recent.path),
        first.path().canonicalize().unwrap()
    );
}

#[test]
fn recents_keep_at_most_twenty_workspaces() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(app_data.path());
    let ids: Vec<String> = (0..21)
        .map(|index| {
            let folder = parent.path().join(format!("w{index}"));
            std::fs::create_dir(&folder).unwrap();
            service.create("W", &folder).unwrap().id().to_owned()
        })
        .collect();

    let recents = recent_ids(&service);

    assert_eq!(recents.len(), 20);
    assert_eq!(recents[0], ids[20]);
    assert!(!recents.contains(&ids[0]));
}

#[test]
fn recents_live_in_app_data_and_never_in_the_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let folder = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(app_data.path());
    service.create("Private", folder.path()).unwrap();

    let mut written: Vec<String> = walk(folder.path());
    written.sort();

    assert_eq!(written, [".noura/trash", ".noura/workspace.yaml"]);
    assert_eq!(recent_ids(&WorkspaceService::new(app_data.path())).len(), 1);
}

fn walk(root: &std::path::Path) -> Vec<String> {
    let mut found = Vec::new();
    let mut stack = vec![root.to_owned()];
    while let Some(folder) = stack.pop() {
        for entry in std::fs::read_dir(&folder).unwrap() {
            let path = entry.unwrap().path();
            let is_empty_dir = path.is_dir() && std::fs::read_dir(&path).unwrap().next().is_none();
            if path.is_dir() && !is_empty_dir {
                stack.push(path);
            } else {
                let relative = path.strip_prefix(root).unwrap();
                found.push(relative.to_string_lossy().replace('\\', "/"));
            }
        }
    }
    found
}

#[test]
fn the_switcher_opens_another_recent_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(app_data.path());
    let a = service.create("A", first.path()).unwrap().id().to_owned();
    let b = service.create("B", second.path()).unwrap().id().to_owned();

    let workspace = service.open_recent(&a).unwrap();

    assert_eq!(workspace.name(), "A");
    assert_eq!(recent_ids(&service), [a, b]);
}

#[test]
fn remove_from_recents_drops_the_entry_and_leaves_its_folder_alone() {
    let app_data = tempfile::tempdir().unwrap();
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(app_data.path());
    let a = service.create("A", first.path()).unwrap().id().to_owned();
    let b = service.create("B", second.path()).unwrap().id().to_owned();
    std::fs::write(first.path().join("Note.md"), b"stays").unwrap();
    let before = manifest(first.path());

    let remaining = service.remove_recent(&a).unwrap();

    let remaining: Vec<&str> = remaining
        .iter()
        .map(|recent| recent.workspace_id.as_str())
        .collect();
    assert_eq!(remaining, [b.as_str()]);
    assert_eq!(recent_ids(&service), [b]);
    assert_eq!(manifest(first.path()), before);
    assert_eq!(
        std::fs::read(first.path().join("Note.md")).unwrap(),
        b"stays"
    );
}

#[test]
fn a_recent_workspace_that_fails_to_open_stays_listed_for_removal() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Gone");
    std::fs::create_dir(&folder).unwrap();
    let service = WorkspaceService::new(app_data.path());
    let id = service.create("Gone", &folder).unwrap().id().to_owned();
    rename_with_retry(&folder, &parent.path().join("Elsewhere"));

    let error = service.open_recent(&id).err().unwrap();

    assert_eq!(error.code, "folder_missing");
    assert!(!folder.exists(), "opening must not recreate the folder");
    assert_eq!(recent_ids(&service), [id]);
}

#[test]
fn restore_reopens_the_last_workspace() {
    let app_data = tempfile::tempdir().unwrap();
    let older = tempfile::tempdir().unwrap();
    let last = tempfile::tempdir().unwrap();
    let service = WorkspaceService::new(app_data.path());
    service.create("Older", older.path()).unwrap();
    let id = service.create("Last", last.path()).unwrap().id().to_owned();

    let restored = WorkspaceService::new(app_data.path())
        .restore_last()
        .unwrap()
        .unwrap();

    assert_eq!(restored.id(), id);
}

#[test]
fn restore_with_no_recent_workspace_opens_nothing() {
    let app_data = tempfile::tempdir().unwrap();

    let restored = WorkspaceService::new(app_data.path())
        .restore_last()
        .unwrap();

    assert!(restored.is_none());
}

#[test]
fn restore_says_why_while_the_folder_is_missing_and_opens_it_once_it_is_back() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let older = parent.path().join("Older");
    let folder = parent.path().join("Last");
    std::fs::create_dir(&older).unwrap();
    std::fs::create_dir(&folder).unwrap();
    let service = WorkspaceService::new(app_data.path());
    service.create("Older", &older).unwrap();
    let id = service.create("Last", &folder).unwrap().id().to_owned();
    let saved_path = service.recents()[0].path.clone();
    let away = parent.path().join("Away");
    rename_with_retry(&folder, &away);

    let error = service.restore_last().err().unwrap();
    assert_eq!(error.code, "folder_missing");
    assert_eq!(
        error.message,
        format!(
            "The folder {saved_path} isn't there. If it's on a drive that isn't connected, \
             connect the drive and try again."
        )
    );
    assert!(!folder.exists(), "restore must not recreate the folder");

    rename_with_retry(&away, &folder);
    let restored = service.restore_last().unwrap().unwrap();
    assert_eq!(restored.id(), id);
}

#[test]
fn restore_and_open_recent_refuse_a_saved_folder_that_lost_its_manifest() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Bare");
    std::fs::create_dir(&folder).unwrap();
    let service = WorkspaceService::new(app_data.path());
    let id = service.create("Bare", &folder).unwrap().id().to_owned();
    let saved_path = service.recents()[0].path.clone();
    std::fs::remove_file(folder.join(".noura/workspace.yaml")).unwrap();

    let restored = service.restore_last().err().unwrap();
    let reopened = service.open_recent(&id).err().unwrap();

    assert_eq!(restored, reopened);
    assert_eq!(restored.code, "workspace_manifest_missing");
    assert_eq!(restored.message, missing_manifest_message(&saved_path));
    assert!(
        !folder.join(".noura/workspace.yaml").exists(),
        "a saved ID must not be written into the folder"
    );
    assert_eq!(recent_ids(&service), [id]);
}

#[test]
fn restore_refuses_a_different_workspace_at_the_saved_path() {
    let app_data = tempfile::tempdir().unwrap();
    let other_app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Shared path");
    std::fs::create_dir(&folder).unwrap();
    let service = WorkspaceService::new(app_data.path());
    service.create("Original", &folder).unwrap();
    let saved_path = service.recents()[0].path.clone();
    // Another workspace replaces the folder at the same path.
    rename_with_retry(&folder, &parent.path().join("Original moved"));
    std::fs::create_dir(&folder).unwrap();
    WorkspaceService::new(other_app_data.path())
        .create("Impostor", &folder)
        .unwrap();

    let error = service.restore_last().err().unwrap();

    assert_eq!(error.code, "workspace_identity_changed");
    assert_eq!(
        error.message,
        format!("A different workspace is now at {saved_path}.")
    );
}

#[cfg(unix)]
#[test]
fn restore_says_when_the_folder_is_unreadable() {
    use std::os::unix::fs::PermissionsExt;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Locked");
    std::fs::create_dir(&folder).unwrap();
    let service = WorkspaceService::new(app_data.path());
    service.create("Locked", &folder).unwrap();
    let saved_path = service.recents()[0].path.clone();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o000)).unwrap();
    if std::fs::read_dir(&folder).is_ok() {
        // Running as root: permissions don't apply.
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }

    let error = service.restore_last().err().unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(
        error.message,
        format!("Noura doesn't have permission to read {saved_path}.")
    );
}

#[cfg(unix)]
#[test]
fn a_folder_that_fails_for_another_reason_is_not_called_a_permission_problem() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    // A name longer than any file system allows.
    let folder = parent.path().join("n".repeat(300));
    let path = folder.to_str().unwrap().to_owned();

    let error = WorkspaceService::new(app_data.path())
        .open_folder(&folder)
        .err()
        .unwrap();

    assert_eq!(error.code, "folder_unavailable");
    assert_eq!(error.message, format!("Noura couldn't open {path}."));
}

fn missing_manifest_message(path: &str) -> String {
    format!(
        "The settings file .noura/workspace.yaml is missing from {path}. Noura left the \
         folder as it was. Choose Open folder to use it as a new workspace."
    )
}

/// Windows can hold a just-closed watcher handle on the folder for a moment.
fn rename_with_retry(from: &std::path::Path, to: &std::path::Path) {
    for attempt in 0.. {
        match std::fs::rename(from, to) {
            Ok(()) => return,
            Err(error) if attempt < 50 => {
                let _ = error;
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            Err(error) => panic!("could not move {from:?}: {error}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn new_workspace_in_a_folder_noura_cannot_represent_returns_unsupported_path() {
    use std::os::unix::ffi::OsStrExt;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent
        .path()
        .join(std::ffi::OsStr::from_bytes(b"not \xff utf-8"));

    let error = WorkspaceService::new(app_data.path())
        .create("Odd", &folder)
        .err()
        .unwrap();

    assert_eq!(error.code, "unsupported_path");
    assert_eq!(std::fs::read_dir(parent.path()).unwrap().count(), 0);
}

/// Every file and folder under `root`, with each file's bytes.
fn snapshot(root: &std::path::Path) -> Vec<(String, Option<Vec<u8>>)> {
    walk(root)
        .into_iter()
        .map(|relative| {
            let path = root.join(&relative);
            let bytes = std::fs::symlink_metadata(&path)
                .unwrap()
                .is_file()
                .then(|| std::fs::read(&path).unwrap());
            (relative, bytes)
        })
        .collect()
}

#[cfg(unix)]
#[test]
fn a_folder_noura_cannot_write_to_says_so_and_stays_empty() {
    use std::os::unix::fs::PermissionsExt;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Read only");
    std::fs::create_dir(&folder).unwrap();
    std::fs::write(folder.join("Kept.md"), b"kept").unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o555)).unwrap();
    if std::fs::create_dir(folder.join("probe")).is_ok() {
        // Running as root: permissions don't apply.
        std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        return;
    }
    let path = folder.to_str().unwrap().to_owned();
    let service = WorkspaceService::new(app_data.path());

    let created = service.create("Read only", &folder).err().unwrap();
    let opened = service.open_folder(&folder).err().unwrap();
    std::fs::set_permissions(&folder, std::fs::Permissions::from_mode(0o755)).unwrap();

    let message =
        format!("Noura doesn't have permission to add its .noura settings folder to {path}.");
    assert_eq!(created.message, message);
    assert_eq!(opened.message, message);
    assert_eq!(walk(&folder), ["Kept.md"]);
    assert!(service.recents().is_empty());
}

#[cfg(unix)]
#[test]
fn a_noura_folder_that_is_a_link_opens_nothing_and_says_so() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let original = parent.path().join("Original");
    std::fs::create_dir(&original).unwrap();
    let service = WorkspaceService::new(app_data.path());
    let id = service
        .create("Original", &original)
        .unwrap()
        .id()
        .to_owned();
    let linked = parent.path().join("Linked");
    std::fs::create_dir(&linked).unwrap();
    std::os::unix::fs::symlink(original.join(".noura"), linked.join(".noura")).unwrap();
    let path = linked.to_str().unwrap().to_owned();
    let before = snapshot(&original);

    let opened = service.open_folder(&linked).err().unwrap();
    let created = service.create("Linked", &linked).err().unwrap();

    let message = format!("The .noura folder in {path} is a link, so Noura won't use it.");
    for error in [opened, created] {
        assert_eq!(error.code, "symlink_escape");
        assert_eq!(error.message, message);
    }
    assert_eq!(recent_ids(&service), [id]);
    assert_eq!(std::fs::read_dir(&linked).unwrap().count(), 1);
    assert!(
        std::fs::symlink_metadata(linked.join(".noura"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(snapshot(&original), before);
}

#[cfg(unix)]
#[test]
fn a_settings_file_that_links_to_another_workspace_never_lends_its_id() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let original = parent.path().join("Original");
    std::fs::create_dir(&original).unwrap();
    let service = WorkspaceService::new(app_data.path());
    let id = service
        .create("Original", &original)
        .unwrap()
        .id()
        .to_owned();
    let linked = parent.path().join("Linked");
    std::fs::create_dir_all(linked.join(".noura")).unwrap();
    std::os::unix::fs::symlink(
        original.join(".noura/workspace.yaml"),
        linked.join(".noura/workspace.yaml"),
    )
    .unwrap();
    let path = linked.to_str().unwrap().to_owned();

    let error = service.open_folder(&linked).err().unwrap();

    assert_eq!(error.code, "symlink_escape");
    assert_eq!(
        error.message,
        format!(
            "The settings file .noura/workspace.yaml in {path} is a link, so Noura won't use it."
        )
    );
    assert_eq!(recent_ids(&service), [id]);
    assert_eq!(
        std::path::Path::new(&service.recents()[0].path),
        original.canonicalize().unwrap()
    );
}

#[cfg(unix)]
#[test]
fn restore_refuses_a_saved_workspace_whose_settings_file_became_a_link() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Saved");
    std::fs::create_dir(&folder).unwrap();
    let service = WorkspaceService::new(app_data.path());
    service.create("Saved", &folder).unwrap();
    let saved_path = service.recents()[0].path.clone();
    let elsewhere = parent.path().join("workspace.yaml");
    std::fs::rename(folder.join(".noura/workspace.yaml"), &elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, folder.join(".noura/workspace.yaml")).unwrap();

    let restored = service.restore_last().err().unwrap();
    let reopened = service
        .open_recent(&service.recents()[0].workspace_id)
        .err()
        .unwrap();

    for error in [restored, reopened] {
        assert_eq!(error.code, "symlink_escape");
        assert_eq!(
            error.message,
            format!(
                "The settings file .noura/workspace.yaml in {saved_path} is a link, so Noura \
                 won't use it."
            )
        );
    }
}

/// Known gap, recorded in ticket 06: a copy of a workspace folder keeps the
/// original's ID. It opens as the same workspace, takes over the original's
/// recent list entry and shares its index. The legacy app behaved the same
/// way. When `local-core` learns to give a copy its own ID, this test
/// changes with it.
#[test]
fn a_copied_workspace_folder_opens_with_the_original_id_today() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let original = parent.path().join("Garden");
    std::fs::create_dir(&original).unwrap();
    let service = WorkspaceService::new(app_data.path());
    let id = service.create("Garden", &original).unwrap().id().to_owned();
    std::fs::write(original.join("Note.md"), b"x").unwrap();
    let copy = parent.path().join("Garden copy");
    std::fs::create_dir_all(copy.join(".noura")).unwrap();
    for relative in [".noura/workspace.yaml", "Note.md"] {
        std::fs::copy(original.join(relative), copy.join(relative)).unwrap();
    }

    let workspace = service.open_folder(&copy).unwrap();

    assert_eq!(workspace.id(), id);
    let recents = service.recents();
    assert_eq!(recents.len(), 1);
    assert_eq!(
        recents[0].path,
        copy.canonicalize().unwrap().to_str().unwrap()
    );
}
