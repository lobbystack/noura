//! Seam 1: the MCP server over an in-process connection, against a real
//! workspace folder and app data, with the app open beside it: the tool
//! list, the kernel tools, the Tasks and Projects tools, the app's file
//! rules, and plugin tools that crash.

mod support;

use noura_mcp::NouraMcp;
use serde_json::json;
use support::{Fixture, answer, call, connect, failure, note};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";

#[tokio::test(flavor = "multi_thread")]
async fn the_server_names_itself_and_lists_todays_tools_and_the_new_project_tools() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_plugins) = support::with_tasks_and_projects(&mut app) else {
        return;
    };
    let client = connect(NouraMcp::new(fixture.agent())).await;

    let info = client.peer_info().unwrap();
    assert_eq!(info.server_info.as_ref().unwrap().name, "noura");
    assert!(info.capabilities.tools.is_some());
    let mut names: Vec<String> = client
        .list_all_tools()
        .await
        .unwrap()
        .into_iter()
        .map(|tool| tool.name.into_owned())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "notes.create",
            "notes.update",
            "projects.create",
            "projects.list",
            "projects.update",
            "tasks.create",
            "tasks.list",
            "tasks.update",
            "workspace.read",
            "workspace.search",
        ]
    );
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_kernel_tools_follow_the_apps_file_rules() {
    let fixture = Fixture::new(&[("Docs/Plan.md", &note(A, "Plant tomatoes.\n"))]);
    let _app = fixture.app();
    let client = connect(NouraMcp::new(fixture.agent())).await;

    let hits = answer(&call(&client, "workspace.search", json!({ "query": "tomatoes" })).await);
    assert_eq!(hits[0]["path"], "Docs/Plan.md");
    let punctuation = call(
        &client,
        "workspace.search",
        json!({ "query": "\"tomatoes\" * OR (NOT) ^:" }),
    )
    .await;
    assert!(answer(&punctuation).is_array());

    let created = answer(
        &call(
            &client,
            "notes.create",
            json!({ "title": "Ideas", "body": "First idea\n", "folder": "Docs" }),
        )
        .await,
    );
    // The note is on disk when the tool answers.
    assert!(fixture.read("Docs/Ideas.md").ends_with("First idea\n"));
    let read = answer(&call(&client, "workspace.read", json!({ "id": created["id"] })).await);
    assert_eq!(read, created);
    let by_path = answer(
        &call(
            &client,
            "workspace.read",
            json!({ "path": "Docs/Ideas.md" }),
        )
        .await,
    );
    assert_eq!(by_path, created);

    let updated = answer(
        &call(
            &client,
            "notes.update",
            json!({
                "id": created["id"],
                "expected_revision": created["revision"],
                "body": "Second idea\n",
            }),
        )
        .await,
    );
    assert!(fixture.read("Docs/Ideas.md").ends_with("Second idea\n"));
    assert_ne!(updated["revision"], created["revision"]);

    let stale = failure(
        &call(
            &client,
            "notes.update",
            json!({
                "id": created["id"],
                "expected_revision": created["revision"],
                "body": "Lost edit\n",
            }),
        )
        .await,
    );
    assert_eq!(stale["code"], "revision_conflict");
    assert!(fixture.read("Docs/Ideas.md").ends_with("Second idea\n"));

    let escaped = failure(
        &call(
            &client,
            "notes.create",
            json!({ "title": "Escaped", "folder": "../Outside" }),
        )
        .await,
    );
    assert_eq!(escaped["code"], "unsafe_path");
    let outside = failure(&call(&client, "workspace.read", json!({ "path": "../Plan.md" })).await);
    assert_eq!(outside["code"], "unsafe_path");
    let neither = failure(&call(&client, "workspace.read", json!({})).await);
    assert_eq!(neither["code"], "id_or_path_required");
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tasks_and_projects_tools_work_through_the_plugin_host() {
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Body\n"))]);
    let mut app = fixture.app();
    let Some(_plugins) = support::with_tasks_and_projects(&mut app) else {
        return;
    };
    let client = connect(NouraMcp::new(fixture.agent())).await;

    let project = answer(&call(&client, "projects.create", json!({ "title": "Orchard" })).await);
    assert!(
        fixture
            .read("projects/Orchard/Orchard.md")
            .contains("status: planned")
    );
    let task = answer(
        &call(
            &client,
            "tasks.create",
            json!({ "title": "Prune", "properties": { "project": project["id"] } }),
        )
        .await,
    );
    let listed = answer(&call(&client, "tasks.list", json!({ "project": project["id"] })).await);
    assert_eq!(listed["tasks"][0]["id"], task["id"]);
    let updated = answer(
        &call(
            &client,
            "tasks.update",
            json!({
                "id": task["id"],
                "expected_revision": task["revision"],
                "properties": { "status": "done" },
            }),
        )
        .await,
    );
    assert_eq!(updated["properties"]["status"], "done");
    let active = answer(
        &call(
            &client,
            "projects.update",
            json!({
                "id": project["id"],
                "expected_revision": project["revision"],
                "properties": { "status": "active" },
            }),
        )
        .await,
    );
    assert_eq!(active["properties"]["status"], "active");
    let projects = answer(&call(&client, "projects.list", json!({ "status": "active" })).await);
    assert_eq!(projects["projects"][0]["title"], "Orchard");

    // A task update with a note's ID and a project with a status it can't
    // have change nothing.
    let plan = std::fs::read(fixture.root.join("Plan.md")).unwrap();
    let read = answer(&call(&client, "workspace.read", json!({ "path": "Plan.md" })).await);
    let not_a_task = failure(
        &call(
            &client,
            "tasks.update",
            json!({
                "id": read["id"],
                "expected_revision": read["revision"],
                "properties": { "status": "done" },
            }),
        )
        .await,
    );
    assert_eq!(not_a_task["code"], "plugin_tool_failed");
    assert!(
        not_a_task["message"]
            .as_str()
            .unwrap()
            .contains("not_a_task"),
        "{not_a_task}"
    );
    assert_eq!(std::fs::read(fixture.root.join("Plan.md")).unwrap(), plan);
    let project_file = fixture.read("projects/Orchard/Orchard.md");
    let invalid = failure(
        &call(
            &client,
            "projects.update",
            json!({
                "id": project["id"],
                "expected_revision": active["revision"],
                "properties": { "status": "someday" },
            }),
        )
        .await,
    );
    assert_eq!(invalid["code"], "plugin_tool_arguments_invalid");
    assert_eq!(fixture.read("projects/Orchard/Orchard.md"), project_file);
    client.cancel().await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_tool_that_crashes_or_loops_answers_with_an_error_and_the_server_keeps_going() {
    let manifest = "id = \"misbehave\"\nname = \"Misbehave\"\nversion = \"0.1.0\"\n\
                    interface-version = \"0.1.0\"\nentry = \"plugin.wasm\"\n\
                    platforms = [\"desktop\"]\n\n[[capabilities]]\nname = \"tools\"\n";
    let Some(folder) = support::plugin_folder(manifest, "plugin.wasm", "misbehave_plugin") else {
        return;
    };
    let fixture = Fixture::new(&[("Plan.md", &note(A, "Plant tomatoes.\n"))]);
    let mut app = fixture.app();
    support::install_and_enable(&mut app, folder.path(), "misbehave");
    let client = connect(NouraMcp::new(fixture.agent())).await;

    for tool in ["misbehave.panic", "misbehave.loop"] {
        let failed = failure(&call(&client, tool, json!({})).await);
        assert_eq!(failed["code"], "plugin_failed", "{tool}: {failed}");
        let hits = answer(&call(&client, "workspace.search", json!({ "query": "tomatoes" })).await);
        assert_eq!(hits[0]["path"], "Plan.md", "{tool}");
    }
    client.cancel().await.unwrap();
}
