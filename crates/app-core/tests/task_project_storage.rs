//! Seam 1: the AGENTS.md storage matrix for task and project files, which
//! the kernel stores as generic managed objects and checks only against
//! the schemas the Tasks and Projects plugins declare. Another plugin
//! changes them through the commands those plugins offer: malformed input,
//! round trips that keep unknown properties and line endings, external
//! edits, stale indexes, interrupted index updates, moves, duplicate IDs,
//! titles that try to leave their folder, and rebuilds. Every check reads
//! the disk as soon as the call returns.

mod support;

use app_core::App;
use serde_json::{Value, json};
use support::{
    Fixture, index_files,
    plugins::{self, PluginFolder},
    projects::{project_file, with_projects},
    recipes::revision,
    tasks::{A, B, C, property, task_file, with_tasks},
};

const CALLER_PAGE: &str = "plugin:caller:hello";
const CALLER_EVENTS: &str = "plugin:caller:events";

/// Tasks and Projects, and the probe as the plugin `caller`, which names
/// them as command targets, installed and turned on, or `None` when the
/// plugins aren't built.
fn plugins_on(app: &mut App) -> Option<[PluginFolder; 3]> {
    let tasks = with_tasks(app)?;
    let projects = with_projects(app)?;
    let caller = plugins::folder_for(
        "probe_plugin",
        "caller",
        &[
            "name = \"views\"",
            "name = \"commands\"\ntargets = [\"tasks\", \"projects\"]",
        ],
    )?;
    plugins::install_and_enable(app, caller.path(), "caller");
    Some([tasks, projects, caller])
}

/// What `plugin`'s command `command` answered the caller with `input`:
/// the JSON it answered, or the JSON of its refusal.
fn call(app: &mut App, plugin: &str, command: &str, input: &Value) -> Result<Value, Value> {
    app.press_plugin_button(CALLER_PAGE, &format!("call:{plugin}/{command}:{input}"));
    support::settle(app);
    let lines = plugins::texts(app, CALLER_EVENTS).expect("the events page draws");
    let last = lines.last().expect("the caller heard back");
    let answer = last.strip_prefix("hello call ").expect("a call's answer");
    if let Some(answer) = answer.strip_prefix("answer ") {
        Ok(serde_json::from_str(answer).unwrap())
    } else if let Some(refusal) = answer.strip_prefix("error failed ") {
        Err(serde_json::from_str(refusal).unwrap())
    } else {
        panic!("{answer}")
    }
}

/// The task or project a command answered with, which must be on disk.
fn written(fixture: &Fixture, answer: &Value) -> Vec<u8> {
    let bytes = fixture.read(answer["path"].as_str().unwrap());
    assert_eq!(answer["revision"], revision(&bytes), "the answer's bytes");
    bytes
}

/// Each task's ID, path, revision and properties, as Tasks lists them.
fn tasks(app: &mut App) -> Vec<(Value, Value, Value, Value)> {
    let listed = call(app, "tasks", "list", &json!({})).expect("list");
    let mut tasks: Vec<_> = listed["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| {
            (
                task["id"].clone(),
                task["path"].clone(),
                task["revision"].clone(),
                task["properties"].clone(),
            )
        })
        .collect();
    tasks.sort_by_key(|task| task.0.to_string());
    tasks
}

#[test]
fn malformed_task_and_project_files_stay_as_they_are_and_no_write_reaches_them() {
    let task = format!("---\nid: task_{A}\ntype: task\nstatus: [\n---\nBody\n");
    let project = format!("---\nid: project_{B}\ntype: project\nstatus: active\nBody\n");
    let fixture = Fixture::new(&[
        ("tasks/Broken.md", task.as_bytes()),
        ("projects/Open/Open.md", project.as_bytes()),
    ]);
    let mut app = fixture.app();
    let Some(_plugins) = plugins_on(&mut app) else {
        return;
    };
    let before = fixture.snapshot();

    assert_eq!(tasks(&mut app), []);
    for (plugin, id) in [
        ("tasks", format!("task_{A}")),
        ("projects", format!("project_{B}")),
    ] {
        let refused = call(
            &mut app,
            plugin,
            "update",
            &json!({"id": id, "expected_revision": "any", "properties": {"status": "done"}}),
        )
        .expect_err(plugin);
        assert_eq!(refused["code"], "not_found", "{plugin}");
    }
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn round_trips_keep_unknown_properties_the_body_and_line_endings() {
    let task = format!(
        "\u{feff}---\r\nid: task_{A}\r\ntype: task\r\nstatus: todo\r\npriority: low\r\n\
         estimate: 3\r\ntags:\r\n- weekend\r\ncreated: 2026-10-01T10:00:00Z\r\n\
         updated: 2026-10-01T10:00:00Z\r\n---\r\nTwo coats.\r\nNo final break"
    );
    let project = project_file(B, "status: planned\nowner: Ada\n", "Beds.\r\nPaths.\r\n");
    let fixture = Fixture::new(&[
        ("tasks/Paint.md", task.as_bytes()),
        ("projects/Garden/Garden.md", &project),
    ]);
    let mut app = fixture.app();
    let Some(_plugins) = plugins_on(&mut app) else {
        return;
    };

    let done = call(
        &mut app,
        "tasks",
        "complete",
        &json!({"id": format!("task_{A}"), "expected_revision": revision(task.as_bytes())}),
    )
    .expect("complete");
    let bytes = written(&fixture, &done);
    assert_eq!(property(&bytes, "status"), Some("done".into()));
    assert_eq!(property(&bytes, "estimate"), Some(3.into()));
    assert_eq!(property(&bytes, "tags"), Some(json!(["weekend"])));
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.starts_with("\u{feff}---\r\nid: task_"), "{text:?}");
    assert!(
        text.ends_with("---\r\nTwo coats.\r\nNo final break"),
        "{text:?}"
    );

    let active = call(
        &mut app,
        "projects",
        "update",
        &json!({
            "id": format!("project_{B}"),
            "expected_revision": revision(&project),
            "properties": {"status": "active"},
        }),
    )
    .expect("update");
    let bytes = written(&fixture, &active);
    assert_eq!(property(&bytes, "status"), Some("active".into()));
    assert_eq!(property(&bytes, "owner"), Some("Ada".into()));
    assert!(bytes.ends_with(b"---\nBeds.\r\nPaths.\r\n"));
}

#[test]
fn an_outside_edit_stops_a_write_with_the_old_revision() {
    let task = task_file(A, "status: todo\n", "2026-10-01T10:00:00Z");
    let fixture = Fixture::new(&[("tasks/Paint.md", &task)]);
    let mut app = fixture.app();
    let Some(_plugins) = plugins_on(&mut app) else {
        return;
    };
    let edited = task_file(A, "status: todo\npriority: high\n", "2026-10-02T10:00:00Z");
    std::fs::write(fixture.root.join("tasks/Paint.md"), &edited).unwrap();

    let refused = call(
        &mut app,
        "tasks",
        "complete",
        &json!({"id": format!("task_{A}"), "expected_revision": revision(&task)}),
    )
    .expect_err("a stale revision");

    assert_eq!(refused["code"], "revision_conflict");
    assert_eq!(fixture.read("tasks/Paint.md"), edited);
}

#[test]
fn writes_by_id_follow_a_file_another_app_moved_before_the_index_knew() {
    let task = task_file(A, "status: todo\n", "2026-10-01T10:00:00Z");
    let fixture = Fixture::new(&[("tasks/Paint.md", &task)]);
    let mut app = fixture.app();
    let Some(_plugins) = plugins_on(&mut app) else {
        return;
    };
    std::fs::create_dir_all(fixture.root.join("Archive")).unwrap();
    std::fs::rename(
        fixture.root.join("tasks/Paint.md"),
        fixture.root.join("Archive/Paint.md"),
    )
    .unwrap();

    let done = call(
        &mut app,
        "tasks",
        "complete",
        &json!({"id": format!("task_{A}"), "expected_revision": revision(&task)}),
    )
    .expect("complete");

    assert_eq!(done["path"], "Archive/Paint.md");
    assert!(!fixture.exists("tasks/Paint.md"));
    assert_eq!(
        property(&written(&fixture, &done), "status"),
        Some("done".into())
    );
}

#[test]
fn creates_whose_index_update_fails_are_on_disk_and_the_index_catches_up() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_plugins) = plugins_on(&mut app) else {
        return;
    };
    let connection = rusqlite::Connection::open(&index_files(&fixture, &app)[0]).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER refuse_insert BEFORE INSERT ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;
             CREATE TRIGGER refuse_update BEFORE UPDATE ON files \
               BEGIN SELECT RAISE(ABORT, 'the index refuses writes'); END;",
        )
        .unwrap();

    let task = call(&mut app, "tasks", "create", &json!({"title": "Weed"})).expect("a task");
    let project =
        call(&mut app, "projects", "create", &json!({"title": "Orchard"})).expect("a project");

    assert_eq!(task["path"], "tasks/Weed.md");
    assert_eq!(
        property(&written(&fixture, &task), "status"),
        Some("todo".into())
    );
    assert_eq!(project["path"], "projects/Orchard/Orchard.md");
    assert_eq!(
        property(&written(&fixture, &project), "status"),
        Some("planned".into())
    );
    connection
        .execute_batch("DROP TRIGGER refuse_insert; DROP TRIGGER refuse_update;")
        .unwrap();
    let listed = tasks(&mut app);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].0, task["id"]);
}

#[test]
fn a_task_and_a_project_moved_in_the_file_tree_keep_their_ids_and_bytes() {
    let task = task_file(A, "status: todo\n", "2026-10-01T10:00:00Z");
    let project = project_file(B, "status: active\n", "Beds.\n");
    let fixture = Fixture::new(&[
        ("tasks/Paint.md", &task),
        ("projects/Garden/Garden.md", &project),
        ("Archive/Keep.md", b"kept"),
    ]);
    let mut app = fixture.app();
    let Some(_plugins) = plugins_on(&mut app) else {
        return;
    };

    app.move_into("tasks/Paint.md", "Archive");
    app.move_into("projects/Garden", "Archive");

    assert_eq!(fixture.read("Archive/Paint.md"), task);
    assert_eq!(fixture.read("Archive/Garden/Garden.md"), project);
    let done = call(
        &mut app,
        "tasks",
        "complete",
        &json!({"id": format!("task_{A}"), "expected_revision": revision(&task)}),
    )
    .expect("complete");
    assert_eq!(done["path"], "Archive/Paint.md");
    assert_eq!(done["id"], format!("task_{A}"));
    let updated = call(
        &mut app,
        "projects",
        "update",
        &json!({
            "id": format!("project_{B}"),
            "expected_revision": revision(&project),
            "properties": {"status": "completed"},
        }),
    )
    .expect("update");
    assert_eq!(updated["path"], "Archive/Garden/Garden.md");
    written(&fixture, &updated);
}

#[test]
fn files_that_share_an_id_stop_writes_by_id_with_the_identity_conflict_error() {
    let task = task_file(A, "status: todo\n", "2026-10-01T10:00:00Z");
    let project = project_file(B, "status: active\n", "Beds.\n");
    let fixture = Fixture::new(&[
        ("tasks/One.md", &task),
        ("tasks/Two.md", &task),
        ("projects/Garden/Garden.md", &project),
        ("projects/Copy/Copy.md", &project),
    ]);
    let mut app = fixture.app();
    let Some(_plugins) = plugins_on(&mut app) else {
        return;
    };
    let before = fixture.snapshot();

    for (plugin, id, file) in [
        ("tasks", format!("task_{A}"), &task),
        ("projects", format!("project_{B}"), &project),
    ] {
        let refused = call(
            &mut app,
            plugin,
            "update",
            &json!({
                "id": id,
                "expected_revision": revision(file),
                "properties": {"status": "cancelled"},
            }),
        )
        .expect_err(plugin);
        assert_eq!(refused["code"], "identity_conflict", "{plugin}");
    }
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn a_title_never_takes_a_new_task_or_project_out_of_its_folder() {
    let fixture = Fixture::new(&[]);
    let outside = fixture.outside();
    let mut app = fixture.app();
    let Some(_plugins) = plugins_on(&mut app) else {
        return;
    };

    let task = call(
        &mut app,
        "tasks",
        "create",
        &json!({"title": "../../Escape"}),
    )
    .expect("a task");
    let project = call(
        &mut app,
        "projects",
        "create",
        &json!({"title": "../..\\Escape: now"}),
    )
    .expect("a project");

    assert_eq!(task["path"], "tasks/Escape.md");
    written(&fixture, &task);
    assert_eq!(project["path"], "projects/Escape now/Escape now.md");
    written(&fixture, &project);
    assert_eq!(std::fs::read_dir(&outside).unwrap().count(), 0);
}

#[test]
fn deleting_every_index_file_rebuilds_the_same_tasks_and_projects_from_the_files() {
    let fixture = Fixture::new(&[(
        "tasks/Paint.md",
        &task_file(C, "status: todo\nestimate: 3\n", "2026-10-01T10:00:00Z"),
    )]);
    let mut app = fixture.app();
    let Some(_plugins) = plugins_on(&mut app) else {
        return;
    };
    let project =
        call(&mut app, "projects", "create", &json!({"title": "Orchard"})).expect("a project");
    call(
        &mut app,
        "tasks",
        "create",
        &json!({"title": "Prune", "properties": {"project": project["id"], "due": "2026-11-02"}}),
    )
    .expect("a task");
    let before = tasks(&mut app);
    let index = index_files(&fixture, &app);
    drop(app);

    for file in &index {
        if file.exists() {
            std::fs::remove_file(file).unwrap();
        }
    }
    let mut app = fixture.app();
    plugins::wait_until(&mut app, "the plugins run again", |app| {
        let rows = app.plugins();
        ["tasks", "projects", "caller"]
            .iter()
            .all(|id| rows.iter().any(|row| row.id == *id && row.running))
    });

    assert_eq!(tasks(&mut app), before);
    assert_eq!(before.len(), 2);
    let projects = call(
        &mut app,
        "projects",
        "update",
        &json!({
            "id": project["id"],
            "expected_revision": project["revision"],
            "properties": {"status": "active"},
        }),
    )
    .expect("the rebuilt index finds the project");
    assert_eq!(projects["path"], "projects/Orchard/Orchard.md");
}
