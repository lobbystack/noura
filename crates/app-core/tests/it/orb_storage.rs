//! Orbs as workspace files. A new Orb is a folder with its file and its
//! memory, in canonical order, on disk before the call returns, and a name
//! can't lead out of `Orbs/`. A file that breaks the format keeps its
//! bytes and doesn't run, and two files with one ID both stop. Changes
//! keep the bytes nobody changed and refuse a stale revision. An Orb moved
//! in another app keeps its thread and its runs, an outside edit shows
//! before the index catches up, and a deleted index rebuilds the same
//! Orbs and runs from the files.

use crate::support;
use std::{path::Path, sync::Arc};

use app_core::{AiOrbChanges, AiOrbState, App};
use support::{
    FakeSystem, Fixture, ai,
    orbs::{self, MEETINGS, MEETINGS_FILE, Reply, Scripted},
};

fn fixture_with(files: &[(String, Vec<u8>)]) -> Fixture {
    let files: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(path, bytes)| (path.as_str(), bytes.as_slice()))
        .collect();
    Fixture::new(&files)
}

fn launch(fixture: &Fixture) -> App {
    let system = FakeSystem::new();
    system.set_clock("2026-11-16T14:00:00Z", "America/Toronto");
    let mut app = fixture.app_with(system);
    let provider: Arc<Scripted> = Scripted::new(|_| Reply::Text("Noted.".to_owned()));
    app.set_ai_client(provider);
    if app.ai_providers().is_empty() {
        app.save_ai_provider(ai::provider("Local", "ollama", "llama3", None))
            .unwrap();
    }
    app
}

fn text(root: &Path, path: &str) -> String {
    std::fs::read_to_string(root.join(path)).unwrap()
}

fn no_changes() -> AiOrbChanges {
    AiOrbChanges {
        title: None,
        instructions: None,
        avatar: None,
        color: None,
        provider: None,
        model: None,
        tools: None,
        triggers: None,
    }
}

#[test]
fn a_new_orb_is_a_folder_with_its_file_and_memory_on_disk_in_canonical_order() {
    let fixture = Fixture::new(&[]);
    let mut app = launch(&fixture);

    let orb = app.create_orb("Weekly review").unwrap();

    // On disk before the call returned.
    let file = text(&fixture.root, "Orbs/Weekly review/Weekly review.md");
    let lines: Vec<&str> = file.lines().collect();
    assert_eq!(lines[1], format!("id: {}", orb.id));
    assert_eq!(
        lines[2..12],
        [
            "type: orb",
            "avatar: null",
            "color: null",
            "provider: null",
            "model: null",
            "tools: []",
            "reads: []",
            "memory: '[[Memory]]'",
            "triggers: []",
            "paused: false",
        ]
    );
    assert!(lines[12].starts_with("created: "));
    assert!(file.ends_with("\n---\n"), "{file}");
    let memory = text(&fixture.root, "Orbs/Weekly review/Memory.md");
    assert!(memory.starts_with("---\nid: note_"), "{memory}");
    assert_eq!(orb.title, "Weekly review");
    assert_eq!(orb.state, AiOrbState::Idle);
    assert_eq!(orbs::orb(&app, &orb.id).revision, orb.revision);

    // A name can't lead out of Orbs/, and a taken one is refused.
    let escaped = app.create_orb("../../Escape").unwrap();
    assert!(fixture.exists("Orbs/Escape/Escape.md"));
    assert!(!fixture.root.parent().unwrap().join("Escape").exists());
    assert_eq!(escaped.title, "Escape");
    let taken = app.create_orb("Weekly review").unwrap_err();
    assert_eq!(taken.code, "already_exists");
    assert!(app.create_orb("  ").is_err());
}

#[test]
fn an_orb_file_that_breaks_the_format_keeps_its_bytes_and_never_runs() {
    let broken = orbs::orb_file(
        MEETINGS,
        "triggers:\n- every weekday at 8:30\n",
        "Odd.\n",
        "2026-11-01T10:00:00Z",
    );
    let unclosed = b"---\nid: orb_01jd8x3k9q2m7v4c6b1n0p5r9c\ntype: orb\ntools: [\n".to_vec();
    let fixture = fixture_with(&[
        (MEETINGS_FILE.to_owned(), broken.clone()),
        ("Orbs/Unclosed/Unclosed.md".to_owned(), unclosed.clone()),
    ]);
    let mut app = launch(&fixture);

    app.check_orbs();
    support::settle(&mut app);

    let listed = app.orbs().unwrap();
    assert_eq!(
        listed.len(),
        2,
        "the default Orb and the broken one: {listed:?}"
    );
    let shown = orbs::orb(&app, MEETINGS);
    assert_eq!(shown.state, AiOrbState::Problem);
    assert_eq!(shown.problem.as_deref(), Some("triggers: invalid_trigger"));
    assert_eq!(fixture.read(MEETINGS_FILE), broken);
    assert_eq!(fixture.read("Orbs/Unclosed/Unclosed.md"), unclosed);
    let refused = app.send_to_orb(MEETINGS, "Hello.").unwrap_err();
    assert!(refused.message.contains("breaks the format"), "{refused:?}");
    assert!(orbs::run_files(&fixture.root).is_empty());
}

#[test]
fn changes_keep_the_bytes_nobody_changed_and_refuse_a_stale_revision() {
    let body = "Keep  this   spacing.\r\nAnd this line.";
    let fixture = fixture_with(&[(
        MEETINGS_FILE.to_owned(),
        orbs::orb_file(MEETINGS, "tools: []\n", body, "2026-11-01T10:00:00Z"),
    )]);
    let mut app = launch(&fixture);
    let revision = orbs::orb(&app, MEETINGS).revision;

    let changed = app
        .update_orb(
            MEETINGS,
            AiOrbChanges {
                tools: Some(vec!["tasks__create".to_owned()]),
                triggers: Some(vec!["event transcription.finished".to_owned()]),
                color: Some("blue".to_owned()),
                ..no_changes()
            },
            &revision,
        )
        .unwrap();

    assert_eq!(changed.tools, ["tasks__create"]);
    assert_eq!(changed.triggers, ["event transcription.finished"]);
    let file = text(&fixture.root, MEETINGS_FILE);
    assert!(
        file.ends_with(&format!("---\n{body}")),
        "the body keeps its bytes: {file:?}"
    );
    assert!(file.contains("\ncolor: blue\n"), "{file}");

    // A change that breaks the format writes nothing.
    let before = fixture.read(MEETINGS_FILE);
    let invalid = app
        .update_orb(
            MEETINGS,
            AiOrbChanges {
                triggers: Some(vec!["every day".to_owned()]),
                ..no_changes()
            },
            &changed.revision,
        )
        .unwrap_err();
    assert_eq!(invalid.code, "invalid_property");
    assert_eq!(fixture.read(MEETINGS_FILE), before);

    // Another app edits the file: the revision Noura read is stale.
    support::write(
        &fixture.root,
        MEETINGS_FILE,
        String::from_utf8(before)
            .unwrap()
            .replace("color: blue", "color: red")
            .as_bytes(),
    );
    let edited = fixture.read(MEETINGS_FILE);
    let stale = app
        .set_orb_paused(MEETINGS, true, &changed.revision)
        .unwrap_err();
    assert_eq!(stale.code, "revision_conflict");
    assert_eq!(fixture.read(MEETINGS_FILE), edited);
    let current = orbs::orb(&app, MEETINGS);
    assert_eq!(
        current.color.as_deref(),
        Some("red"),
        "the outside edit shows at once"
    );
    let paused = app
        .set_orb_paused(MEETINGS, true, &current.revision)
        .unwrap();
    assert_eq!(paused.state, AiOrbState::Paused);
    assert!(text(&fixture.root, MEETINGS_FILE).contains("\npaused: true\n"));

    // A new name renames its folder and its file, and keeps its ID.
    let renamed = app
        .update_orb(
            MEETINGS,
            AiOrbChanges {
                title: Some("Standups".to_owned()),
                ..no_changes()
            },
            &paused.revision,
        )
        .unwrap();
    assert_eq!(renamed.id, MEETINGS);
    assert_eq!(renamed.title, "Standups");
    assert!(fixture.exists("Orbs/Standups/Standups.md"));
    assert!(!fixture.exists(MEETINGS_FILE));
}

#[test]
fn an_orb_moved_in_another_app_keeps_its_thread_runs_and_memory() {
    let fixture = fixture_with(&orbs::meetings(&[], &[], "2026-11-01T10:00:00Z"));
    let mut app = launch(&fixture);
    app.send_to_orb(MEETINGS, "Hello.").unwrap();
    orbs::wait_for_runs(&mut app, MEETINGS, 1);
    let thread = orbs::orb(&app, MEETINGS).thread.unwrap();

    std::fs::rename(
        fixture.root.join("Orbs/Meetings"),
        fixture.root.join("Orbs/Standups"),
    )
    .unwrap();
    std::fs::rename(
        fixture.root.join("Orbs/Standups/Meetings.md"),
        fixture.root.join("Orbs/Standups/Standups.md"),
    )
    .unwrap();
    support::plugins::wait_until(&mut app, "Noura sees the move", |app| {
        app.orbs()
            .unwrap()
            .iter()
            .any(|orb| orb.id == MEETINGS && orb.title == "Standups")
    });

    let moved = orbs::orb(&app, MEETINGS);
    assert_eq!(moved.title, "Standups");
    assert_eq!(moved.thread.as_deref(), Some(thread.as_str()));
    assert_eq!(orbs::runs(&app, MEETINGS).len(), 1);
    app.send_to_orb(MEETINGS, "Still there?").unwrap();
    orbs::wait_for_runs(&mut app, MEETINGS, 2);
    let records = orbs::run_files(&fixture.root);
    assert!(
        records
            .iter()
            .all(|(path, _)| path.starts_with("Orbs/Standups/Runs/")),
        "{records:?}"
    );
    assert_eq!(
        orbs::orb(&app, MEETINGS).thread.as_deref(),
        Some(thread.as_str())
    );
}

#[test]
fn two_files_with_one_orb_id_both_stop() {
    let mut files = orbs::meetings(&[], &["schedule 30 8 * * *"], "2026-11-01T10:00:00Z");
    let copy = files[0].1.clone();
    files.push(("Orbs/Copy/Copy.md".to_owned(), copy));
    let fixture = fixture_with(&files);
    let mut app = launch(&fixture);

    app.check_orbs();
    support::settle(&mut app);

    let listed: Vec<_> = app
        .orbs()
        .unwrap()
        .into_iter()
        .filter(|orb| orb.id == MEETINGS)
        .collect();
    assert_eq!(listed.len(), 2);
    assert!(listed.iter().all(|orb| orb.state == AiOrbState::Problem
        && orb.problem.as_deref() == Some("id: identity_conflict")));
    let refused = app.send_to_orb(MEETINGS, "Hello.").unwrap_err();
    assert_eq!(refused.code, "identity_conflict");
    assert!(
        orbs::run_files(&fixture.root).is_empty(),
        "no schedule fires"
    );
}

#[test]
fn an_outside_edit_shows_before_the_index_catches_up() {
    let fixture = fixture_with(&orbs::meetings(&[], &[], "2026-11-01T10:00:00Z"));
    let mut app = launch(&fixture);
    app.send_to_orb(MEETINGS, "Hello.").unwrap();
    let runs = orbs::wait_for_runs(&mut app, MEETINGS, 1);

    // Another app changes the Orb and its run record; Noura reads them
    // before its watcher reports either.
    let orb =
        text(&fixture.root, MEETINGS_FILE).replace("triggers: []", "triggers:\n- schedule @daily");
    std::fs::write(fixture.root.join(MEETINGS_FILE), orb).unwrap();
    let (path, record) = orbs::run_files(&fixture.root).remove(0);
    std::fs::write(
        fixture.root.join(&path),
        record.replace("quiet: false", "quiet: true"),
    )
    .unwrap();

    assert_eq!(orbs::orb(&app, MEETINGS).triggers, ["schedule @daily"]);
    let read = orbs::runs(&app, MEETINGS);
    assert_eq!(read[0].id, runs[0].id);
    assert!(read[0].quiet);
}

#[test]
fn orbs_and_their_runs_come_back_from_the_files_after_the_index_goes() {
    let fixture = fixture_with(&orbs::meetings(
        &[],
        &["event transcription.finished"],
        "2026-11-01T10:00:00Z",
    ));
    let mut app = launch(&fixture);
    app.send_to_orb(MEETINGS, "Hello.").unwrap();
    let runs = orbs::wait_for_runs(&mut app, MEETINGS, 1);
    let before = app.orbs().unwrap();
    let indexes = support::index_files(&fixture, &app);
    app.close_window();
    drop(app);

    for file in indexes {
        let _ = std::fs::remove_file(file);
    }
    let mut app = launch(&fixture);
    support::settle(&mut app);

    assert_eq!(app.orbs().unwrap(), before);
    let rebuilt = orbs::runs(&app, MEETINGS);
    assert_eq!(rebuilt.len(), 1);
    assert_eq!(rebuilt[0].id, runs[0].id);
    assert_eq!(rebuilt[0].status, runs[0].status);
    app.send_to_orb(MEETINGS, "Again.").unwrap();
    orbs::wait_for_runs(&mut app, MEETINGS, 2);
    let chats = ai::chat_files(&fixture.root)
        .into_iter()
        .filter(|(path, _)| path.ends_with("/chat.md"))
        .count();
    assert_eq!(chats, 1, "the rebuilt index finds the same thread");
}
