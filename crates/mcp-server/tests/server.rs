//! Seam 1: the MCP server over an in-process connection, against a real
//! workspace folder and app data, with the app open beside it: the tool
//! list, the kernel tools, the Tasks and Projects tools, the app's file
//! rules, plugin tools that crash, a plugin turned off while the server
//! runs, and a plugin whose code is slow to load.

mod support;

use std::time::{Duration, Instant};

use noura_mcp::NouraMcp;
use rmcp::model::CallToolRequestParams;
use serde_json::json;
use support::{Fixture, ListChanges, answer, call, connect, connect_as, failure, note, tool_names};

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

#[tokio::test(flavor = "multi_thread")]
async fn a_plugin_turned_off_after_the_server_started_leaves_the_tool_list() {
    let fixture = Fixture::new(&[]);
    let mut app = fixture.app();
    let Some(_plugins) = support::with_tasks_and_projects(&mut app) else {
        return;
    };
    let changes = ListChanges::default();
    let client = connect_as(NouraMcp::new(fixture.agent()), changes.clone()).await;
    let tools = &client.peer_info().unwrap().capabilities.tools;
    assert_eq!(tools.as_ref().unwrap().list_changed, Some(true));
    assert!(
        tool_names(&client)
            .await
            .contains(&"projects.list".to_owned())
    );

    assert!(app.set_plugin_enabled("projects", false));
    let refused = failure(&call(&client, "projects.list", json!({})).await);

    assert_eq!(refused["code"], "plugin_disabled");
    let deadline = Instant::now() + Duration::from_secs(10);
    while changes.count() == 0 {
        assert!(
            Instant::now() < deadline,
            "the server never said the list changed"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let names = tool_names(&client).await;
    assert!(
        !names.iter().any(|name| name.starts_with("projects.")),
        "{names:?}"
    );
    assert!(names.contains(&"tasks.list".to_owned()), "{names:?}");
    let gone = client
        .call_tool(CallToolRequestParams::new("projects.list"))
        .await;
    assert!(gone.is_err(), "{gone:?}");
    answer(&call(&client, "tasks.list", json!({})).await);
    client.cancel().await.unwrap();
}

/// The server answers MCP's initialize request and runs the kernel tools
/// while a plugin's code still loads, so Test connection's 10 seconds
/// don't depend on plugins. The test holds the Tasks plugin's code back
/// by turning its installed component into a pipe nobody writes to yet.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread")]
async fn the_server_answers_before_a_plugins_code_loads_and_lists_its_tools_once_it_does() {
    use std::io::Write;

    let fixture = Fixture::new(&[("Plan.md", &note(A, "Plant tomatoes.\n"))]);
    let mut app = fixture.app();
    let Some(_plugins) = support::with_tasks_and_projects(&mut app) else {
        return;
    };
    let component = fixture
        .app_data
        .path()
        .join("plugins/library/tasks/component.wasm");
    let bytes = std::fs::read(&component).unwrap();
    std::fs::remove_file(&component).unwrap();
    let made = std::process::Command::new("mkfifo")
        .arg(&component)
        .status()
        .unwrap();
    assert!(made.success());

    let (app_data, root) = (fixture.app_data.path().to_owned(), fixture.root.clone());
    let opened = tokio::time::timeout(Duration::from_secs(10), async move {
        let agent = tokio::task::spawn_blocking(move || {
            app_core::AgentWorkspace::open(app_data, &root).unwrap()
        })
        .await
        .unwrap();
        let agent = std::sync::Arc::new(agent);
        (agent.clone(), connect(NouraMcp::new(agent)).await)
    })
    .await;
    let (agent, client) = opened.expect("the server answered while a plugin's code loaded");

    assert_eq!(
        client
            .peer_info()
            .unwrap()
            .server_info
            .as_ref()
            .unwrap()
            .name,
        "noura"
    );
    let hits = answer(&call(&client, "workspace.search", json!({ "query": "tomatoes" })).await);
    assert_eq!(hits[0]["path"], "Plan.md");
    assert!(!agent.plugins_started());

    // The code arrives: the host reads it from the pipe, and any later
    // read finds the file again.
    let writer = std::thread::spawn(move || {
        let mut pipe = std::fs::OpenOptions::new()
            .write(true)
            .open(&component)
            .unwrap();
        let staged = component.with_extension("staged");
        std::fs::write(&staged, &bytes).unwrap();
        std::fs::rename(&staged, &component).unwrap();
        pipe.write_all(&bytes).unwrap();
    });
    let names = tool_names(&client).await;
    writer.join().unwrap();

    assert!(names.contains(&"tasks.list".to_owned()), "{names:?}");
    assert!(names.contains(&"projects.list".to_owned()), "{names:?}");
    answer(&call(&client, "tasks.list", json!({})).await);
    client.cancel().await.unwrap();
}
