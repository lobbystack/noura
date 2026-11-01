//! Seam 1: a task opens in the note editor, so typing in its body follows
//! the same saves as a note: it survives chunks of typing across saves, a
//! change in the Tasks panel in the middle of typing, and an outside edit
//! of other text. Each test reads the file.

mod support;

use std::time::{Duration, Instant};

use app_core::{App, Field, InputValue, PluginEvent, SAVE_PAUSE, TabId};
use support::{
    Fixture, open_from_tree, settle, settle_saves,
    tasks::{A, find, property, task_file, with_tasks},
};

const PATH: &str = "tasks/Plan trip.md";

fn workspace() -> Fixture {
    Fixture::new(&[(
        PATH,
        &task_file(
            A,
            "status: todo\npriority: medium\n",
            "2026-10-01T10:00:00Z",
        ),
    )])
}

/// `added` typed at the end of the task's body at `at`.
fn type_at_end(app: &mut App, tab: TabId, added: &str, at: Instant) {
    let text = format!("{}{added}", app.note(tab).unwrap().text());
    app.edit_note(tab, 1, &text, at);
}

fn save(app: &mut App, at: Instant) {
    app.save_due_notes(at + SAVE_PAUSE);
    settle_saves(app);
}

fn body(fixture: &Fixture) -> String {
    let text = String::from_utf8(fixture.read(PATH)).unwrap();
    text.split_once("\n---\n").unwrap().1.to_owned()
}

fn shown(app: &mut App, tab: TabId, node: &str) -> Option<InputValue> {
    let view = app.plugin_panel(tab)?.ok()?;
    match &find(view.root(), node)?.kind {
        app_core::ViewKind::Input { value, .. } => Some(value.clone()),
        _ => None,
    }
}

/// Wait until the panel shows the task's `status`.
fn wait_for_panel(app: &mut App, tab: TabId, node: &str, id: &str) {
    let value = InputValue::Choice(Some(id.to_owned()));
    let deadline = Instant::now() + Duration::from_secs(20);
    while shown(app, tab, node).as_ref() != Some(&value) {
        assert!(Instant::now() < deadline, "the panel never showed {id}");
        app.poll();
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn change(app: &mut App, tab: TabId, node: &str, id: &str) {
    app.send_panel_event(
        tab,
        &PluginEvent::Changed(Field {
            node: node.to_owned(),
            value: InputValue::Choice(Some(id.to_owned())),
        }),
    );
    settle(app);
}

#[test]
fn typing_a_task_s_body_across_saves_loses_nothing() {
    let fixture = workspace();
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let tab = open_from_tree(&mut app, PATH);
    settle(&mut app);

    let mut typed = app.note(tab).unwrap().text().to_owned();
    for chunk in ["First line ", "with spaces  ", "and more "] {
        let at = Instant::now();
        type_at_end(&mut app, tab, chunk, at);
        typed.push_str(chunk);
        save(&mut app, at);
        assert_eq!(body(&fixture), typed);
    }
    let at = Instant::now();
    type_at_end(&mut app, tab, "\n- item one", at);
    typed.push_str("\n- item one");
    save(&mut app, at);

    assert_eq!(body(&fixture), typed);
    assert_eq!(app.note(tab).unwrap().text(), typed);
    assert!(!app.note(tab).unwrap().is_unsaved());
    assert_eq!(property(&fixture.read(PATH), "status"), Some("todo".into()));
    assert!(app.notice().is_none());
}

#[test]
fn changing_a_task_s_properties_while_typing_keeps_both() {
    let fixture = workspace();
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let tab = open_from_tree(&mut app, PATH);
    wait_for_panel(&mut app, tab, "status", "todo");

    // Far enough ahead that no save starts on its own before the change.
    let later = Instant::now() + Duration::from_secs(600);
    type_at_end(&mut app, tab, "pack bags ", later);
    change(&mut app, tab, "status", "in-progress");
    settle_saves(&mut app);
    wait_for_panel(&mut app, tab, "status", "in-progress");
    type_at_end(&mut app, tab, "and passport", later);
    change(&mut app, tab, "priority", "high");
    settle_saves(&mut app);
    wait_for_panel(&mut app, tab, "priority", "high");
    save(&mut app, later);

    let bytes = fixture.read(PATH);
    assert_eq!(property(&bytes, "status"), Some("in-progress".into()));
    assert_eq!(property(&bytes, "priority"), Some("high".into()));
    assert_eq!(body(&fixture), "Notes.\npack bags and passport");
    assert_eq!(
        app.note(tab).unwrap().text(),
        "Notes.\npack bags and passport"
    );
    assert!(!app.note(tab).unwrap().is_unsaved());
    // The panel is Noura's own, so nothing says another app edited the task.
    assert!(app.notice().is_none());
}

#[test]
fn an_outside_edit_to_the_open_task_merges_with_typing() {
    let fixture = workspace();
    let mut app = fixture.app();
    let Some(_folder) = with_tasks(&mut app) else {
        return;
    };
    let tab = open_from_tree(&mut app, PATH);
    settle(&mut app);
    let at = Instant::now();
    type_at_end(&mut app, tab, "local text\n", at);
    save(&mut app, at);
    assert_eq!(body(&fixture), "Notes.\nlocal text\n");

    // Typing waits while another app adds a line further down.
    let at = Instant::now();
    let text = app
        .note(tab)
        .unwrap()
        .text()
        .replacen("Notes.", "My notes.", 1);
    app.edit_note(tab, 1, &text, at);
    let outside = String::from_utf8(fixture.read(PATH)).unwrap();
    support::write(
        &fixture.root,
        PATH,
        format!("{outside}from outside\n").as_bytes(),
    );
    app.rescan();
    settle_saves(&mut app);
    save(&mut app, at);

    assert_eq!(body(&fixture), "My notes.\nlocal text\nfrom outside\n");
    assert_eq!(
        app.note(tab).unwrap().text(),
        "My notes.\nlocal text\nfrom outside\n"
    );
    assert!(app.note(tab).unwrap().conflict().is_none());

    let at = Instant::now();
    type_at_end(&mut app, tab, "!", at);
    save(&mut app, at);
    assert_eq!(body(&fixture), "My notes.\nlocal text\nfrom outside\n!");
    assert_eq!(property(&fixture.read(PATH), "status"), Some("todo".into()));
}
