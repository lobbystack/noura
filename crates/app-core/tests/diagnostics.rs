//! Seam 1: Copy diagnostics puts a report on the clipboard that describes
//! the app and the workspace's health without naming anything in it.

mod support;

use std::path::Path;

use app_core::{App, NoticeKind, VERSION, WorkspaceService, settings_section};
use support::{Call, FakeSystem, note, rows, settle, write};

/// Names that appear only in the workspace, its folder's path and the log,
/// so the report must contain none of them.
const DISTINCTIVE: [&str; 11] = [
    "Zephyrine",
    "Marrowvale",
    "Okapi",
    "Tamarind",
    "Wombat",
    "Quokka",
    "Pangolin",
    "Bramblewick",
    "someone",
    "Permission denied",
    "secret agenda",
];

/// A workspace named Zephyrine Ledger in the folder Marrowvale Notes, with
/// a note whose properties can't be read and two notes that share an ID.
fn workspace(parent: &Path, app_data: &Path) -> std::path::PathBuf {
    let folder = parent.join("Marrowvale Notes");
    std::fs::create_dir_all(&folder).unwrap();
    let root = WorkspaceService::new(app_data)
        .create("Zephyrine Ledger", &folder)
        .unwrap()
        .root()
        .to_owned();
    let root = std::path::PathBuf::from(root);
    write(
        &root,
        "Okapi Plans/Tamarind.md",
        b"---\nid: note_tamarind\ntype: [unclosed\n---\nbody",
    );
    write(&root, "Okapi Plans/Wombat budget.md", b"plain text");
    for name in ["Quokka.md", "Pangolin.md"] {
        write(&root, name, &note("01jbcdefghjkmnpqrstvwxyz22", "body"));
    }
    write(&root, "Bramblewick.pdf", b"%PDF-1.7");
    root
}

/// A log of 250 ordinary lines, then lines that name the workspace's
/// files and folders in every shape a log line might.
fn log(app_data: &Path, root: &Path) {
    let mut lines: Vec<String> = (0..250)
        .map(|index| format!("2026-10-20T10:00:00.000000Z  INFO desktop: frame {index}"))
        .collect();
    let root = root.display();
    lines.extend([
        "2026-10-20T10:01:00.000000Z  WARN local_core::engine::scan: a workspace file could \
         not be read path=Okapi Plans/Tamarind.md reason=\"Permission denied (os error 13)\""
            .to_owned(),
        "2026-10-20T10:01:01.000000Z  WARN app_core: opened Tamarind.md and Bramblewick.pdf"
            .to_owned(),
        "2026-10-20T10:01:02.000000Z  WARN app_core: rename failed name=Wombat".to_owned(),
        format!("2026-10-20T10:01:03.000000Z ERROR app_core: could not watch {root}/Okapi Plans"),
        "2026-10-20T10:01:04.000000Z ERROR app_core: open failed for /Users/someone/Notes and \
         C:\\Users\\someone\\Marrowvale Notes"
            .to_owned(),
        "2026-10-20T10:01:05.000000Z  WARN app_core: a workspace operation failed \
         details=Some(Object {\"path\": String(\"Quokka.md\")})"
            .to_owned(),
        "2026-10-20T10:01:06.000000Z  WARN app_core: moved Pangolin beside Quokka in \
         Zephyrine Ledger"
            .to_owned(),
        "2026-10-20T10:01:07.000000Z  INFO app_core: searched for 'secret agenda' and \
         `secret agenda`"
            .to_owned(),
    ]);
    let folder = app_core::log_folder(app_data);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("noura.log"), lines.join("\n") + "\n").unwrap();
}

fn copied(system: &FakeSystem) -> String {
    let copies: Vec<String> = system
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Copy(text) => Some(text),
            _ => None,
        })
        .collect();
    assert_eq!(copies.len(), 1, "one report reaches the clipboard");
    copies.into_iter().next().unwrap()
}

#[test]
fn the_report_describes_the_app_and_the_workspace_s_health_without_naming_anything() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let root = workspace(parent.path(), app_data.path());
    log(app_data.path(), &root);
    let system = FakeSystem::new();
    let mut app = App::launch_with(app_data.path(), system.clone());
    settle(&mut app);
    app.open_settings_at(settings_section::ABOUT);

    app.copy_diagnostics();
    let report = copied(&system);

    assert!(report.contains(&format!("Noura {VERSION}")), "{report}");
    assert!(
        report.contains(&format!(
            "System: {} {}",
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
        "{report}"
    );
    // The four notes; the index leaves the PDF out.
    assert!(report.contains("Indexed files: 4"), "{report}");
    assert!(
        report.contains("Problems: identity_conflict 1, parse_error 1"),
        "{report}"
    );
    // The last 200 log lines, and no earlier ones.
    assert!(report.contains("frame 249\n"), "{report}");
    assert!(report.contains("frame 58\n"), "{report}");
    assert!(!report.contains("frame 57\n"), "{report}");
    // What happened stays readable.
    assert!(
        report.contains("a workspace file could not be read"),
        "{report}"
    );
    assert!(report.contains("rename failed"), "{report}");
    for name in DISTINCTIVE {
        assert!(!report.contains(name), "the report names {name}:\n{report}");
    }
    for part in root.iter().filter_map(|part| part.to_str()) {
        if part.len() > 3 {
            assert!(!report.contains(part), "the report names {part}:\n{report}");
        }
    }
    let notice = app.settings().unwrap().notice.clone().unwrap();
    assert_eq!(notice.kind, NoticeKind::Done);
}

#[test]
fn without_a_workspace_or_a_log_the_report_says_so() {
    let app_data = tempfile::tempdir().unwrap();
    let system = FakeSystem::new();
    let mut app = App::launch_with(app_data.path(), system.clone());
    app.open_settings_at(settings_section::ABOUT);

    app.copy_diagnostics();
    let report = copied(&system);

    assert!(report.contains(&format!("Noura {VERSION}")), "{report}");
    assert!(report.contains("Workspace: none open"), "{report}");
    assert!(report.contains("(no log entries)"), "{report}");
}

/// Words the log holds that no list of known names covers: a path in
/// another workspace, a file's name before it was renamed, a path outside
/// any workspace, and a quoted path with quotes in it.
const UNKNOWN: [&str; 19] = [
    "Divorce",
    "Lawyer",
    "Meeting",
    "Custody",
    "Lantern",
    "Keeper",
    "Diary",
    "someone",
    "Desktop",
    "Tax",
    "Return",
    "Final",
    "Draft",
    "Hidden",
    "Affair",
    "Harbor",
    "Ledger",
    "Volumes",
    "Saltmarsh",
];

#[test]
fn names_no_list_knows_stay_out_of_the_report() {
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    std::fs::create_dir_all(&folder).unwrap();
    let root = WorkspaceService::new(app_data.path())
        .create("Garden", &folder)
        .unwrap()
        .root()
        .to_owned();
    let root = std::path::PathBuf::from(root);
    write(&root, "Lantern Keeper Diary.md", b"before");
    let system = FakeSystem::new();
    let mut app = App::launch_with(app_data.path(), system.clone());
    settle(&mut app);
    // The file's old name is in the log, but no longer in the tree.
    std::fs::rename(
        root.join("Lantern Keeper Diary.md"),
        root.join("Fresh start.md"),
    )
    .unwrap();
    app.rescan();
    settle(&mut app);
    assert_eq!(rows(&app), ["Fresh start"]);

    let lines = [
        // Another workspace's file, logged unquoted.
        "2026-10-21T09:00:00.000000Z  WARN local_core::engine::reconcile: a Markdown file \
         could not be read path=Divorce Lawyer Meeting/Custody Plan.md error=denied",
        // The renamed file, by its old name, quoted and unquoted.
        "2026-10-21T09:00:01.000000Z  WARN local_core::engine::scan: a workspace file could \
         not be read path=\"Lantern Keeper Diary.md\" reason=\"busy\"",
        "2026-10-21T09:00:02.000000Z  INFO app_core: moved from=Lantern Keeper Diary.md \
         to=Fresh start.md",
        // A path outside any workspace, in an error and in a message.
        "2026-10-21T09:00:03.000000Z  WARN app_core: the file manager didn't open \
         error=failed for /Users/someone/Desktop/Tax Return Final Draft.pdf",
        "2026-10-21T09:00:04.000000Z  WARN app_core: could not watch /Volumes/Harbor \
         Ledger/Saltmarsh notes",
        // A Debug path with escaped quotes, and the walker's error naming it.
        "2026-10-21T09:00:05.000000Z  WARN local_core::engine::scan: the workspace walk \
         skipped an entry path=Some(\"/x/My \\\"Hidden Affair\\\" notes\") error=IO error \
         for operation on /x/My \"Hidden Affair\" notes: Permission denied (os error 13)",
    ];
    let folder = app_core::log_folder(app_data.path());
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("noura.log"), lines.join("\n") + "\n").unwrap();

    app.copy_diagnostics();
    let report = copied(&system);

    for word in UNKNOWN {
        assert!(!report.contains(word), "the report names {word}:\n{report}");
    }
    for readable in [
        "a Markdown file could not be read path=<redacted> error=denied",
        "a workspace file could not be read path=<redacted> reason=<text>",
        // The tree knows the new name.
        "moved from=<redacted> to=<name>",
        "the file manager didn't open error=<redacted>",
        "could not watch <path>",
        "the workspace walk skipped an entry path=<redacted> error=<redacted>",
    ] {
        assert!(report.contains(readable), "{readable}:\n{report}");
    }
}
