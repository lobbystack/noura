//! The Transcription plugin through AccessKit: the panel beside a
//! Transcription holds Start, then Stop with the timer while it records,
//! and the project picker; after a recording it holds Summarize, the
//! Summary's status, the tasks whose source is the Transcription and the
//! suggested project. The settings page holds the folder rule, the
//! provider, the language, system audio, the audio retention and the
//! prompt.

mod support;

use std::time::{Duration, Instant};

use egui::accesskit::Role;
use egui_kittest::{
    Harness,
    kittest::{NodeT, Queryable},
};
use support::{FakePicker, create_workspace, harness, plugins, settle};

type Ui = Harness<'static, app_ui::NouraUi>;

const TASKS: &str = include_str!("../../plugins/tasks/plugin.toml");
const TRANSCRIPTION: &str = include_str!("../../plugins/transcription/plugin.toml");

const STANDUP: &str = "---\nid: transcription_01jbcdefghjkmnpqrstvwxyz0a\ntype: transcription\n\
                       start: 2026-11-16T09:00:00-05:00\nend: 2026-11-16T09:20:00-05:00\n\
                       attendees: []\ncreated: 2026-11-16T14:00:00Z\nupdated: 2026-11-16T14:20:00Z\n\
                       ---\nAgenda.\n\n## Summary\n\n- Ship on Friday.\n";
const TRANSCRIPT: &str = "---\nid: transcript_01jbcdefghjkmnpqrstvwxyz0b\ntype: transcript\n\
                          transcription: '[[Standup]]'\ncreated: 2026-11-16T14:00:00Z\n\
                          updated: 2026-11-16T14:20:00Z\n---\n**Me** · 00:00:02\nShip on Friday.\n";
const TASK: &str = "---\nid: task_01jbcdefghjkmnpqrstvwxyz0c\ntype: task\nstatus: todo\n\
                    priority: medium\nsource: '[[Standup]]'\ncreated: 2026-11-16T14:20:00Z\n\
                    updated: 2026-11-16T14:20:00Z\n---\n";
const EMPTY: &str = "---\nid: transcription_01jbcdefghjkmnpqrstvwxyz0d\ntype: transcription\n\
                     attendees: []\ncreated: 2026-11-16T14:00:00Z\nupdated: 2026-11-16T14:00:00Z\n\
                     ---\nAgenda.\n";

/// Run frames, waiting in real time for the plugins' answers, until
/// `done` holds.
fn wait_until(harness: &mut Ui, what: &str, mut done: impl FnMut(&Ui) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !done(harness) {
        assert!(
            Instant::now() < deadline,
            "the screen never showed {what}: {:?}",
            harness
                .query_by_role_and_label(Role::Group, "Properties")
                .map(|_| panel(harness))
        );
        std::thread::sleep(Duration::from_millis(20));
        harness.step();
    }
    harness.run_steps(4);
}

struct Opened {
    harness: Ui,
    _folders: [tempfile::TempDir; 4],
}

/// A workspace with Standup, its Transcript and a task it created, and
/// the Tasks and Transcription plugins on. `None` when they aren't built.
fn opened() -> Option<Opened> {
    let tasks = plugins::built("tasks_plugin")?;
    let transcription = plugins::built("transcription_plugin")?;
    let app_data = tempfile::tempdir().unwrap();
    let parent = tempfile::tempdir().unwrap();
    let folder = parent.path().join("Garden");
    create_workspace(app_data.path(), &folder);
    for (path, text) in [
        ("Meetings/Standup/Standup.md", STANDUP),
        ("Meetings/Standup/Transcript.md", TRANSCRIPT),
        ("Meetings/Retro/Retro.md", EMPTY),
        ("Tasks/Send the notes.md", TASK),
    ] {
        let path = folder.join(path);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }
    // The recommended model, Parakeet Ultra, is on this computer.
    let models = app_data.path().join("speech/models");
    std::fs::create_dir_all(&models).unwrap();
    std::fs::write(models.join("parakeet-ultra-Q8_0.gguf"), b"GGUF").unwrap();
    let tasks_parent = tempfile::tempdir().unwrap();
    let transcription_parent = tempfile::tempdir().unwrap();
    let tasks = plugins::folder(tasks_parent.path(), TASKS, "tasks_plugin.wasm", &tasks);
    let transcription = plugins::folder(
        transcription_parent.path(),
        TRANSCRIPTION,
        "transcription_plugin.wasm",
        &transcription,
    );
    plugins::install_and_enable(app_data.path(), &tasks, "tasks");
    plugins::install_and_enable(app_data.path(), &transcription, "transcription");
    let mut harness = harness(app_data.path(), FakePicker::default());
    settle(&mut harness);
    Some(Opened {
        harness,
        _folders: [app_data, parent, tasks_parent, transcription_parent],
    })
}

fn open(harness: &mut Ui, path: &str) {
    harness
        .state_mut()
        .app_mut()
        .open_file(path, app_core::OpenAs::Kept);
    wait_until(harness, "the Transcription panel", |harness| {
        harness
            .query_by_role_and_label(Role::Group, "Properties")
            .is_some_and(|panel| {
                panel
                    .query_by_role_and_label(Role::Group, "Transcription")
                    .is_some()
            })
    });
}

/// Each node in the properties panel with a label, as `Role: label`.
fn panel(harness: &Ui) -> Vec<String> {
    let mut found = Vec::new();
    for node in harness
        .get_by_role_and_label(Role::Group, "Properties")
        .query_all(egui_kittest::kittest::by())
    {
        let access = node.accesskit_node();
        if let Some(label) = access.label().or(access.value()) {
            found.push(format!("{:?}: {label}", access.role()));
        }
    }
    found
}

fn has(found: &[String], expected: &str) -> bool {
    found.iter().any(|line| line == expected)
}

#[test]
fn the_panel_shows_the_summary_the_tasks_and_the_project_picker() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;
    open(harness, "Meetings/Standup/Standup.md");
    wait_until(harness, "the task", |harness| {
        has(&panel(harness), "ListBoxOption: Send the notes")
    });
    let shown = panel(harness);
    for expected in [
        "Button: Summarize",
        "Label: Summary added",
        "ListBoxOption: Send the notes",
        "Label: Summary",
        "Label: Tasks",
    ] {
        assert!(has(&shown, expected), "{expected}: {shown:?}");
    }
    assert!(
        shown
            .iter()
            .any(|line| line.ends_with(": Project") && !line.starts_with("Label")),
        "{shown:?}"
    );
    assert!(!has(&shown, "Button: Start"), "{shown:?}");
}

#[test]
fn start_records_and_shows_stop_and_the_timer() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;
    open(harness, "Meetings/Retro/Retro.md");
    wait_until(harness, "Start", |harness| {
        has(&panel(harness), "Button: Start")
    });
    // A pointer click, which once reached the editor's toolbar under
    // the panel and made the note a checklist.
    harness
        .get_by_role_and_label(Role::Group, "Properties")
        .get_by_role_and_label(Role::Button, "Start")
        .click();
    wait_until(harness, "Stop", |harness| {
        has(&panel(harness), "Button: Stop")
    });
    let shown = panel(harness);
    assert!(has(&shown, "Label: 00:00:00"), "{shown:?}");
    assert!(!has(&shown, "Button: Start"), "{shown:?}");
    let tab = harness
        .state()
        .app()
        .active_tab()
        .map(|tab| tab.id())
        .unwrap();
    assert_eq!(harness.state().app().note(tab).unwrap().text(), "Agenda.\n");

    harness
        .get_by_role_and_label(Role::Group, "Properties")
        .get_by_role_and_label(Role::Button, "Stop")
        .click_accesskit();
    // Nothing was said, so the Transcription waits for another recording.
    wait_until(harness, "Start again", |harness| {
        has(&panel(harness), "Button: Start")
    });
}

#[test]
fn the_settings_page_holds_each_setting() {
    let Some(mut opened) = opened() else {
        return;
    };
    let harness = &mut opened.harness;
    harness
        .state_mut()
        .app_mut()
        .open_settings_at("plugin-settings:transcription");
    wait_until(harness, "the settings page", |harness| {
        harness
            .query_by_role_and_label(Role::Button, "Save prompt")
            .is_some()
    });
    let mut labels = Vec::new();
    for node in harness.query_all(egui_kittest::kittest::by()) {
        let access = node.accesskit_node();
        if let Some(label) = access.label() {
            labels.push(format!("{:?}: {label}", access.role()));
        }
    }
    for expected in [
        "Folder",
        "Provider",
        "Language",
        "System audio",
        "Keep audio",
        "Prompt",
        "Save prompt",
    ] {
        assert!(
            labels
                .iter()
                .any(|line| line.ends_with(&format!(": {expected}"))),
            "{expected}: {labels:?}"
        );
    }
    harness.get_by_role_and_label(Role::CheckBox, "System audio");
    harness.get_by_role_and_label(Role::MultilineTextInput, "Prompt");
}
