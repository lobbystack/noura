//! Seams 1 and 3: the tools of the plugins a workspace enables, as an AI
//! tool outside Noura calls them through `AgentWorkspace`, beside the app
//! on the same app data, in the real plugin host: which tools appear, the
//! Tasks and Projects tools, calls to a plugin turned off or revoked since
//! the tool started, and tools that crash or run past their limits.

mod support;

use std::time::{Duration, Instant};

use app_core::{AgentWorkspace, Grant, PluginTool};
use serde_json::{Value, json};
use support::{
    Fixture, note,
    plugins::{self, PluginFolder},
    projects, tasks,
};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";

fn agent(fixture: &Fixture) -> AgentWorkspace {
    AgentWorkspace::open(fixture.app_data.path(), &fixture.root).unwrap()
}

fn names(tools: &[PluginTool]) -> Vec<String> {
    let mut names: Vec<String> = tools
        .iter()
        .map(|tool| format!("{}.{}", tool.plugin_id, tool.name))
        .collect();
    names.sort();
    names
}

fn probe(capabilities: &[&str]) -> Option<PluginFolder> {
    plugins::folder_for("probe_plugin", "probe", capabilities)
}

/// The JSON a tool answered with.
fn answer(result: Result<String, app_core::AppError>) -> Value {
    serde_json::from_str(&result.expect("the tool answers")).unwrap()
}

/// The JSON refusal a tool failed with.
fn refusal(result: Result<String, app_core::AppError>) -> Value {
    let error = result.expect_err("the tool refuses");
    assert_eq!(error.code, "plugin_tool_failed", "{}", error.message);
    serde_json::from_str(&error.message).unwrap()
}

#[test]
fn only_enabled_plugins_approved_for_tools_offer_them_and_calls_stop_once_they_are_turned_off() {
    let Some(folder) = probe(&["name = \"views\"", "name = \"tools\""]) else {
        return;
    };
    let Some(quiet) = plugins::folder_for("probe_plugin", "quiet", &["name = \"views\""]) else {
        return;
    };
    let Some(off) = plugins::folder_for("probe_plugin", "off", &["name = \"tools\""]) else {
        return;
    };
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, folder.path(), "probe");
    // Installed with tools but not turned on in this workspace.
    plugins::install(&mut app, off.path());
    // Turned on, but it doesn't declare tools.
    plugins::install_and_enable(&mut app, quiet.path(), "quiet");

    let agent = agent(&fixture);
    let tools = agent.plugin_tools();
    assert_eq!(names(&tools), ["probe.echo"]);
    assert_eq!(tools[0].description, "Repeats the text it gets.");
    assert_eq!(
        agent.call_plugin_tool("probe", "echo", r#"{"text":"hi"}"#),
        Ok(r#"{"text":"hi"}"#.to_owned())
    );
    assert_eq!(
        agent
            .call_plugin_tool("probe", "echo", r#"{"text":5}"#)
            .unwrap_err()
            .code,
        "plugin_tool_arguments_invalid"
    );
    assert_eq!(
        agent
            .call_plugin_tool("off", "echo", r#"{"text":"hi"}"#)
            .unwrap_err()
            .code,
        "plugin_disabled"
    );

    // You revoke the tools capability in the app: the next call stops.
    assert!(app.revoke_plugin_grant("probe", &Grant::Capability("tools".into())));
    assert_eq!(
        agent
            .call_plugin_tool("probe", "echo", r#"{"text":"hi"}"#)
            .unwrap_err()
            .code,
        "plugin_tools_revoked"
    );
    assert!(app.allow_plugin_grant("probe", &Grant::Capability("tools".into())));
    assert!(
        agent
            .call_plugin_tool("probe", "echo", r#"{"text":"hi"}"#)
            .is_ok()
    );

    // The app turns the plugin off: calls stop, and a new agent doesn't
    // list it.
    assert!(app.set_plugin_enabled("probe", false));
    assert_eq!(
        agent
            .call_plugin_tool("probe", "echo", r#"{"text":"hi"}"#)
            .unwrap_err()
            .code,
        "plugin_disabled"
    );
    assert!(
        agent
            .call_plugin_tool("probe", "echo", r#"{"text":"hi"}"#)
            .is_err()
    );
    assert!(self::agent(&fixture).plugin_tools().is_empty());
}

#[test]
fn the_tasks_tools_list_create_and_update_tasks_and_refuse_a_note() {
    let Some(folder) = tasks::folder() else {
        return;
    };
    let fixture = Fixture::new(&[
        ("Plan.md", &note(A, "Body\n")),
        (
            "tasks/Water.md",
            &tasks::task_file(A, "status: todo\n", "2026-10-01T10:00:00Z"),
        ),
        ("projects/Garden/Garden.md", &tasks::project_file(A)),
    ]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, folder.path(), "tasks");
    let agent = agent(&fixture);
    assert_eq!(
        names(&agent.plugin_tools()),
        ["tasks.create", "tasks.list", "tasks.update"]
    );

    let created = answer(
        agent.call_plugin_tool(
            "tasks",
            "create",
            &json!({
                "title": "Sow beans",
                "body": "Two rows.\n",
                "properties": { "priority": "high", "project": format!("project_{A}") },
            })
            .to_string(),
        ),
    );
    let path = created["path"].as_str().unwrap().to_owned();
    let on_disk = String::from_utf8(fixture.read(&path)).unwrap();
    assert!(on_disk.contains("priority: high"), "{on_disk}");
    assert!(on_disk.ends_with("Two rows.\n"), "{on_disk}");

    let listed = answer(agent.call_plugin_tool(
        "tasks",
        "list",
        &json!({ "project": format!("project_{A}") }).to_string(),
    ));
    let titles: Vec<&str> = listed["tasks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|task| task["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["Sow beans"]);
    let todo = answer(agent.call_plugin_tool("tasks", "list", r#"{"status":"todo"}"#));
    assert_eq!(todo["tasks"].as_array().unwrap().len(), 2);

    let updated = answer(
        agent.call_plugin_tool(
            "tasks",
            "update",
            &json!({
                "id": created["id"],
                "expected_revision": created["revision"],
                "properties": { "status": "done" },
            })
            .to_string(),
        ),
    );
    assert_eq!(updated["properties"]["status"], "done");
    assert!(
        String::from_utf8(fixture.read(&path))
            .unwrap()
            .contains("status: done")
    );

    let before = fixture.snapshot();
    // A stale revision, a note's ID and a due date that isn't one change
    // nothing.
    let stale = refusal(
        agent.call_plugin_tool(
            "tasks",
            "update",
            &json!({
                "id": created["id"],
                "expected_revision": created["revision"],
                "properties": { "status": "todo" },
            })
            .to_string(),
        ),
    );
    assert_eq!(stale["code"], "revision_conflict");
    let plan = agent.workspace().read_item(&format!("note_{A}")).unwrap();
    let not_a_task = refusal(
        agent.call_plugin_tool(
            "tasks",
            "update",
            &json!({
                "id": format!("note_{A}"),
                "expected_revision": plan.revision,
                "properties": { "status": "done" },
            })
            .to_string(),
        ),
    );
    assert_eq!(not_a_task["code"], "not_a_task", "{not_a_task}");
    let not_a_date = refusal(
        agent.call_plugin_tool(
            "tasks",
            "update",
            &json!({
                "id": updated["id"],
                "expected_revision": updated["revision"],
                "properties": { "due": "next Tuesday" },
            })
            .to_string(),
        ),
    );
    assert_eq!(not_a_date["code"], "invalid_property", "{not_a_date}");
    let unknown = agent
        .call_plugin_tool(
            "tasks",
            "update",
            &json!({
                "id": updated["id"],
                "expected_revision": updated["revision"],
                "properties": { "owner": "me" },
            })
            .to_string(),
        )
        .unwrap_err();
    assert_eq!(unknown.code, "plugin_tool_arguments_invalid");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn the_projects_tools_list_create_and_update_projects_and_refuse_invalid_properties() {
    let (Some(tasks_folder), Some(projects_folder)) = (tasks::folder(), projects::folder()) else {
        return;
    };
    let fixture = Fixture::new(&[(
        "projects/Garden/Garden.md",
        &projects::project_file(A, "status: active\n", "Overview.\n"),
    )]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, tasks_folder.path(), "tasks");
    plugins::install_and_enable(&mut app, projects_folder.path(), "projects");
    let agent = agent(&fixture);
    let tools = names(&agent.plugin_tools());
    for name in ["projects.create", "projects.list", "projects.update"] {
        assert!(tools.contains(&name.to_owned()), "{tools:?}");
    }

    let created = answer(agent.call_plugin_tool(
        "projects",
        "create",
        r#"{"title":"Orchard","body":"Apples and pears.\n"}"#,
    ));
    assert_eq!(created["properties"]["status"], "planned");
    let path = created["path"].as_str().unwrap().to_owned();
    assert_eq!(path, "projects/Orchard/Orchard.md");
    assert!(
        String::from_utf8(fixture.read(&path))
            .unwrap()
            .ends_with("Apples and pears.\n")
    );

    let listed = answer(agent.call_plugin_tool("projects", "list", "{}"));
    let mut titles: Vec<&str> = listed["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|project| project["title"].as_str().unwrap())
        .collect();
    titles.sort_unstable();
    assert_eq!(titles, ["Garden", "Orchard"]);
    let active = answer(agent.call_plugin_tool("projects", "list", r#"{"status":"active"}"#));
    assert_eq!(active["projects"][0]["title"], "Garden");
    assert_eq!(active["projects"].as_array().unwrap().len(), 1);

    let updated = answer(
        agent.call_plugin_tool(
            "projects",
            "update",
            &json!({
                "id": created["id"],
                "expected_revision": created["revision"],
                "properties": { "status": "active" },
            })
            .to_string(),
        ),
    );
    assert_eq!(updated["properties"]["status"], "active");
    assert!(
        String::from_utf8(fixture.read(&path))
            .unwrap()
            .contains("status: active")
    );

    let before = fixture.snapshot();
    for properties in [json!({ "status": "someday" }), json!({ "owner": "me" })] {
        let error = agent
            .call_plugin_tool(
                "projects",
                "update",
                &json!({
                    "id": updated["id"],
                    "expected_revision": updated["revision"],
                    "properties": properties,
                })
                .to_string(),
            )
            .unwrap_err();
        assert_eq!(error.code, "plugin_tool_arguments_invalid", "{properties}");
    }
    let stale = refusal(
        agent.call_plugin_tool(
            "projects",
            "update",
            &json!({
                "id": updated["id"],
                "expected_revision": created["revision"],
                "properties": { "status": "completed" },
            })
            .to_string(),
        ),
    );
    assert_eq!(stale["code"], "revision_conflict");
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn a_tool_that_crashes_or_runs_past_its_limits_answers_with_an_error_and_the_next_call_works() {
    let Some(folder) = plugins::folder_for("misbehave_plugin", "misbehave", &["name = \"tools\""])
    else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    let mut app = fixture.app();
    plugins::install_and_enable(&mut app, folder.path(), "misbehave");
    let agent = agent(&fixture);
    assert_eq!(
        names(&agent.plugin_tools()),
        [
            "misbehave.allocate",
            "misbehave.fine",
            "misbehave.loop",
            "misbehave.panic"
        ]
    );

    for tool in ["panic", "loop", "allocate"] {
        let started = Instant::now();
        let error = agent.call_plugin_tool("misbehave", tool, "{}").unwrap_err();
        assert_eq!(error.code, "plugin_failed", "{tool}: {}", error.message);
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "{tool} took {:?}",
            started.elapsed()
        );
        // The kernel still answers.
        assert!(agent.workspace().read_item(&format!("note_{A}")).is_ok());
        if tool == "panic" {
            assert_eq!(
                agent.call_plugin_tool("misbehave", "fine", "{}"),
                Ok("still fine".to_owned())
            );
        }
    }
    // Three failures in a minute stop the plugin for this connection, and
    // its calls say so.
    let stopped = agent
        .call_plugin_tool("misbehave", "fine", "{}")
        .unwrap_err();
    assert_eq!(stopped.code, "plugin_failed", "{}", stopped.message);
}
