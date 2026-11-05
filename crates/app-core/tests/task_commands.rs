//! Seam 3 against a real workspace folder: another plugin changes tasks
//! through the commands Tasks declares, and every write passes the task
//! schema. A value outside it fails with a structured error and leaves the
//! file unchanged, properties another app wrote survive, and Tasks can't
//! write an object of another type.

mod support;

use app_core::App;
use support::{
    Fixture, note,
    plugins::{self, PluginFolder},
    recipes::revision,
    tasks::{A, B, property, with_tasks},
};

const CALLER_PAGE: &str = "plugin:caller:hello";
const CALLER_EVENTS: &str = "plugin:caller:events";

/// The probe as the plugin `caller`, which names Tasks as a command
/// target, installed and turned on.
fn caller(app: &mut App) -> Option<PluginFolder> {
    let folder = plugins::folder_for(
        "probe_plugin",
        "caller",
        &[
            "name = \"views\"",
            "name = \"commands\"\ntargets = [\"tasks\"]",
        ],
    )?;
    plugins::install_and_enable(app, folder.path(), "caller");
    Some(folder)
}

/// What the Tasks command `command` answered the caller with `input`: the
/// JSON it answered, or the JSON of its refusal.
fn call(app: &mut App, command: &str, input: &str) -> Result<serde_json::Value, serde_json::Value> {
    app.press_plugin_button(CALLER_PAGE, &format!("call:tasks/{command}:{input}"));
    support::settle(app);
    let lines = plugins::texts(app, CALLER_EVENTS).expect("the events page draws");
    let last = lines.last().expect("the caller heard back");
    let answer = last.strip_prefix("hello call ").expect("a call's answer");
    if let Some(json) = answer.strip_prefix("answer ") {
        Ok(serde_json::from_str(json).unwrap())
    } else if let Some(json) = answer.strip_prefix("error failed ") {
        Err(serde_json::from_str(json).unwrap())
    } else {
        panic!("{answer}")
    }
}

const PAINT: &str = "tasks/Paint the fence.md";

fn paint() -> Vec<u8> {
    format!(
        "---\nid: task_{A}\ntype: task\nstatus: todo\npriority: medium\nestimate: 3\n\
         tags:\n- outdoors\n- weekend\ncreated: 2026-10-01T10:00:00Z\n\
         updated: 2026-10-01T10:00:00Z\n---\nTwo coats.\n"
    )
    .into_bytes()
}

#[test]
fn a_write_with_a_value_outside_the_task_fields_fails_with_a_structured_error() {
    let fixture = Fixture::new(&[(PAINT, &paint())]);
    let mut app = fixture.app();
    let (Some(_tasks), Some(_caller)) = (with_tasks(&mut app), caller(&mut app)) else {
        return;
    };
    let before = fixture.snapshot();
    let target = format!(
        r#""id":"task_{A}","expected_revision":"{}""#,
        revision(&paint())
    );

    for (properties, field, problem) in [
        (r#"{"status":"blocked"}"#, "status", "not_allowed"),
        (r#"{"priority":3}"#, "priority", "wrong_type"),
        (r#"{"due":"next week"}"#, "due", "invalid_date"),
        (r#"{"project":"Garden"}"#, "project", "invalid_reference"),
        (r#"{"kanban_order":7}"#, "kanban_order", "wrong_type"),
    ] {
        let refused = call(
            &mut app,
            "update",
            &format!("{{{target},\"properties\":{properties}}}"),
        )
        .expect_err(properties);
        assert_eq!(refused["code"], "invalid_property", "{properties}");
        assert_eq!(refused["property"], field, "{properties}");
        assert_eq!(refused["problem"], problem, "{properties}");
    }
    let refused = call(
        &mut app,
        "create",
        r#"{"title":"Rake the leaves","properties":{"status":"someday"}}"#,
    )
    .expect_err("a create outside the fields");
    assert_eq!(refused["property"], "status");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn properties_another_app_wrote_survive_a_tasks_write() {
    let fixture = Fixture::new(&[(PAINT, &paint())]);
    let mut app = fixture.app();
    let (Some(_tasks), Some(_caller)) = (with_tasks(&mut app), caller(&mut app)) else {
        return;
    };
    let target = format!(
        r#""id":"task_{A}","expected_revision":"{}""#,
        revision(&paint())
    );

    let done = call(&mut app, "complete", &format!("{{{target}}}")).expect("complete");

    let bytes = fixture.read(PAINT);
    assert_eq!(done["revision"], revision(&bytes));
    assert_eq!(property(&bytes, "status"), Some("done".into()));
    assert_eq!(property(&bytes, "estimate"), Some(3.into()));
    assert_eq!(
        property(&bytes, "tags"),
        Some(serde_json::json!(["outdoors", "weekend"]))
    );
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.ends_with("---\nTwo coats.\n"), "{text}");
}

#[test]
fn a_stale_revision_changes_nothing() {
    let fixture = Fixture::new(&[(PAINT, &paint())]);
    let mut app = fixture.app();
    let (Some(_tasks), Some(_caller)) = (with_tasks(&mut app), caller(&mut app)) else {
        return;
    };
    let before = fixture.snapshot();

    let refused = call(
        &mut app,
        "update",
        &format!(
            r#"{{"id":"task_{A}","expected_revision":"stale","properties":{{"kanban_order":"a1"}}}}"#
        ),
    )
    .expect_err("a stale update");

    assert_eq!(refused["code"], "revision_conflict");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn update_accepts_every_task_field_the_board_order_key_included() {
    let fixture = Fixture::new(&[
        (PAINT, &paint()),
        (
            "projects/Garden/Garden.md",
            &support::tasks::project_file(B),
        ),
    ]);
    let mut app = fixture.app();
    let (Some(_tasks), Some(_caller)) = (with_tasks(&mut app), caller(&mut app)) else {
        return;
    };

    let updated = call(
        &mut app,
        "update",
        &format!(
            r#"{{"id":"task_{A}","expected_revision":"{}","properties":{{"status":"in-progress","priority":"high","project":"project_{B}","due":"2026-10-30T09:00:00-04:00","kanban_order":"a0V"}},"body":"Three coats.\n"}}"#,
            revision(&paint())
        ),
    )
    .expect("update");

    let bytes = fixture.read(PAINT);
    assert_eq!(updated["revision"], revision(&bytes));
    let text = String::from_utf8(bytes).unwrap();
    let start = format!(
        "---\nid: task_{A}\ntype: task\nstatus: in-progress\npriority: high\n\
         project: project_{B}\ndue: 2026-10-30T09:00:00-04:00\nkanban_order: a0V\nestimate: 3\n"
    );
    assert!(text.starts_with(&start), "{text}");
    assert!(text.ends_with("---\nThree coats.\n"), "{text}");

    // A null clears a field.
    let cleared = call(
        &mut app,
        "update",
        &format!(
            r#"{{"id":"task_{A}","expected_revision":"{}","properties":{{"due":null,"project":null}}}}"#,
            updated["revision"].as_str().unwrap()
        ),
    )
    .expect("clear");
    assert_eq!(cleared["properties"].get("due"), None);
    assert_eq!(property(&fixture.read(PAINT), "project"), None);
}

#[test]
fn tasks_cant_write_an_object_of_another_type() {
    let note_bytes = note(A, "Plan the garden.\n");
    let fixture = Fixture::new(&[("Plan.md", &note_bytes)]);
    let mut app = fixture.app();
    let (Some(_tasks), Some(_caller)) = (with_tasks(&mut app), caller(&mut app)) else {
        return;
    };
    let target = format!(
        r#""id":"note_{A}","expected_revision":"{}""#,
        revision(&note_bytes)
    );

    for (command, input) in [
        ("complete", format!("{{{target}}}")),
        (
            "update",
            format!(r#"{{{target},"properties":{{"status":"done"}}}}"#),
        ),
    ] {
        let refused = call(&mut app, command, &input).expect_err(command);
        assert_eq!(refused["code"], "not_a_task", "{command}");
    }
    assert_eq!(fixture.read("Plan.md"), note_bytes);
}

#[test]
fn a_task_created_by_another_plugin_gets_the_task_defaults_in_tasks() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let (Some(_tasks), Some(_caller)) = (with_tasks(&mut app), caller(&mut app)) else {
        return;
    };

    let created = call(
        &mut app,
        "create",
        r#"{"title":"Rake the leaves","properties":{"due":"2026-10-25"}}"#,
    )
    .expect("create");

    let id = created["id"].as_str().unwrap();
    assert!(id.starts_with("task_"), "{id}");
    assert_eq!(created["path"], "tasks/Rake the leaves.md");
    let bytes = fixture.read("tasks/Rake the leaves.md");
    assert_eq!(created["revision"], revision(&bytes));
    let text = String::from_utf8(bytes).unwrap();
    let start =
        format!("---\nid: {id}\ntype: task\nstatus: todo\npriority: medium\ndue: 2026-10-25\n");
    assert!(text.starts_with(&start), "{text}");
}

#[test]
fn a_plugin_that_doesnt_name_tasks_as_a_target_is_denied() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_tasks) = with_tasks(&mut app) else {
        return;
    };
    let Some(folder) = plugins::folder_for(
        "probe_plugin",
        "caller",
        &[
            "name = \"views\"",
            "name = \"commands\"\ntargets = [\"hello\"]",
        ],
    ) else {
        return;
    };
    plugins::install_and_enable(&mut app, folder.path(), "caller");
    let before = fixture.snapshot();

    app.press_plugin_button(
        CALLER_PAGE,
        r#"call:tasks/create:{"title":"Rake the leaves"}"#,
    );
    support::settle(&mut app);

    assert_eq!(
        plugins::texts(&app, CALLER_EVENTS).unwrap(),
        ["hello call error target-not-declared tasks/create"]
    );
    assert_eq!(fixture.snapshot(), before);
}
