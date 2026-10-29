//! Seam 1: the app and the MCP server on one workspace and one device's
//! app data, as when an AI tool works while Noura is open. Changes made
//! through MCP reach the open app as outside edits, and an index rebuilt
//! from the files matches what both saw.

mod support;

use std::{
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

use app_core::{AgentWorkspace, App, Change, OpenAs, Origin, SearchQuery};
use noura_mcp::NouraMcp;
use serde_json::json;
use support::{Fixture, answer, call, connect, note};

const A: &str = "01jbcdefghjkmnpqrstvwxyz0a";

/// Poll the app until `done` holds, collecting the changes it announces.
fn wait_for(
    app: &mut App,
    changes: &Receiver<Change>,
    seen: &mut Vec<Change>,
    what: &str,
    done: impl Fn(&App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        app.poll();
        seen.extend(changes.try_iter());
        if done(app) {
            return;
        }
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        if let Ok(change) = changes.recv_timeout(Duration::from_millis(50)) {
            seen.push(change);
        }
    }
}

/// Every object the workspace knows, as (ID, type, path), by ID.
fn objects(workspace: &app_core::Workspace) -> Vec<(String, String, String)> {
    workspace
        .objects()
        .unwrap()
        .into_iter()
        .map(|object| (object.id, object.object_type, object.relative_path))
        .collect()
}

fn search(workspace: &app_core::Workspace, text: &str) -> Vec<String> {
    let mut paths: Vec<String> = workspace
        .search(&SearchQuery {
            text: text.to_owned(),
            ..SearchQuery::default()
        })
        .unwrap()
        .into_iter()
        .map(|hit| hit.relative_path)
        .collect();
    paths.sort();
    paths
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_made_through_mcp_show_in_the_open_app_and_survive_an_index_rebuild() {
    let fixture = Fixture::new(&[("Docs/Plan.md", &note(A, "Plant tomatoes.\n"))]);
    let mut app = fixture.app();
    let plugins = support::with_tasks_and_projects(&mut app);
    let (sender, changes) = channel();
    app.subscribe(move |change| {
        let _ = sender.send(change);
    });
    app.tree_mut().unwrap().select("Docs/Plan.md");
    let row = app.tree().unwrap().focused().unwrap();
    app.click_tree_row(row, OpenAs::Preview);
    let tab = app.active_tab().unwrap().id();
    support::settle(&mut app);

    let agent = fixture.agent();
    let client = connect(NouraMcp::new(agent.clone())).await;
    let plan = answer(&call(&client, "workspace.read", json!({ "path": "Docs/Plan.md" })).await);
    answer(
        &call(
            &client,
            "notes.update",
            json!({
                "id": plan["id"],
                "expected_revision": plan["revision"],
                "body": "Plant tomatoes and beans, said MCP.\n",
            }),
        )
        .await,
    );
    answer(
        &call(
            &client,
            "notes.create",
            json!({ "title": "Ideas", "folder": "Docs", "body": "An MCP idea.\n" }),
        )
        .await,
    );
    if plugins.is_some() {
        answer(&call(&client, "tasks.create", json!({ "title": "MCP chore" })).await);
    }

    let mut seen = Vec::new();
    wait_for(
        &mut app,
        &changes,
        &mut seen,
        "the app shows the MCP changes",
        |app| {
            app.tree().unwrap().find_row("Docs/Ideas.md").is_some()
                && app
                    .note(tab)
                    .is_some_and(|note| note.text().contains("said MCP"))
        },
    );
    assert!(seen.contains(&Change::Files(Origin::Outside)), "{seen:?}");
    assert!(!app.note(tab).unwrap().is_unsaved());

    // Both processes see the same objects and search hits.
    let expected = objects(app.workspace().unwrap());
    assert_eq!(objects(agent.workspace()), expected);
    let found = search(app.workspace().unwrap(), "MCP");
    assert_eq!(search(agent.workspace(), "MCP"), found);
    let mut wanted = vec!["Docs/Ideas.md".to_owned(), "Docs/Plan.md".to_owned()];
    if plugins.is_some() {
        wanted.push("tasks/MCP chore.md".to_owned());
    }
    wanted.sort();
    assert_eq!(found, wanted);

    // Delete the index: one rebuilt from the files holds the same.
    client.cancel().await.unwrap();
    drop(agent);
    let workspace_id = app.workspace().unwrap().id().to_owned();
    app.close_window();
    drop(app);
    let index = fixture
        .app_data
        .path()
        .join("workspaces")
        .join(&workspace_id);
    for name in ["index.sqlite", "index.sqlite-wal", "index.sqlite-shm"] {
        let path = index.join(name);
        if path.exists() {
            std::fs::remove_file(path).unwrap();
        }
    }
    let rebuilt = AgentWorkspace::open(fixture.app_data.path(), &fixture.root).unwrap();
    assert_eq!(objects(rebuilt.workspace()), expected);
    assert_eq!(search(rebuilt.workspace(), "MCP"), found);
}
