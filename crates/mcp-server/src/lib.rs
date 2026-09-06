#![allow(
    clippy::result_large_err,
    reason = "MCP delegates to local-core and preserves its structured public errors"
)]

use std::sync::{Arc, Mutex};

use local_core::{CreateObjectInput, ObjectPatch, SearchInput, WorkspaceEngine};
use rmcp::{
    ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    pub query: String,
    pub limit: Option<u32>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadParams {
    pub id: String,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateParams {
    pub title: String,
    pub body: Option<String>,
    pub relative_path: Option<String>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateParams {
    pub id: String,
    pub title: Option<String>,
    pub body: Option<String>,
    pub properties: Option<serde_json::Map<String, serde_json::Value>>,
    pub expected_revision: String,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskListParams {
    pub status: Option<String>,
    pub project: Option<String>,
}
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct TaskCreateParams {
    pub title: String,
    pub body: Option<String>,
    pub relative_path: Option<String>,
    pub status: Option<String>,
    pub priority: Option<String>,
    pub due: Option<String>,
    pub project: Option<String>,
}

#[derive(Clone)]
pub struct NouraMcp {
    engine: Arc<Mutex<WorkspaceEngine>>,
    tool_router: ToolRouter<Self>,
}
impl NouraMcp {
    pub fn new(engine: WorkspaceEngine) -> Self {
        Self {
            engine: Arc::new(Mutex::new(engine)),
            tool_router: Self::tool_router(),
        }
    }
    fn with_engine<T>(
        &self,
        run: impl FnOnce(&WorkspaceEngine) -> local_core::Result<T>,
    ) -> Result<T, String> {
        let engine = self
            .engine
            .lock()
            .map_err(|_| "workspace lock unavailable".to_owned())?;
        run(&engine).map_err(|error| format!("{}: {}", error.code, error.message))
    }

    pub async fn run_reconciliation(self) {
        let mut last_periodic = std::time::Instant::now();
        loop {
            tokio::time::sleep(std::time::Duration::from_millis(750)).await;
            let result = self.with_engine(|engine| {
                engine.poll_external_changes(std::time::Duration::from_millis(25))?;
                if last_periodic.elapsed() >= std::time::Duration::from_secs(60) {
                    engine.reconcile()?;
                    last_periodic = std::time::Instant::now();
                }
                Ok(())
            });
            if let Err(error) = result {
                tracing::warn!(%error, "workspace reconciliation failed");
            }
        }
    }
}

#[tool_router(router=tool_router)]
impl NouraMcp {
    #[tool(
        name = "workspace.search",
        description = "Search workspace files and managed objects"
    )]
    async fn workspace_search(
        &self,
        Parameters(input): Parameters<SearchParams>,
    ) -> Result<String, String> {
        let values = self.with_engine(|engine| {
            engine.search(&SearchInput {
                query: input.query,
                object_type: None,
                path_prefix: None,
                limit: input.limit,
            })
        })?;
        serde_json::to_string(&values).map_err(|_| "response serialization failed".into())
    }
    #[tool(
        name = "workspace.read",
        description = "Read a managed workspace object by stable ID"
    )]
    async fn workspace_read(
        &self,
        Parameters(input): Parameters<ReadParams>,
    ) -> Result<String, String> {
        let value = self
            .with_engine(|engine| engine.get_object(&input.id))?
            .ok_or_else(|| "object_not_found: The object does not exist".to_owned())?;
        serde_json::to_string(&value).map_err(|_| "response serialization failed".into())
    }
    #[tool(name = "notes.create", description = "Create a Markdown Note")]
    async fn notes_create(
        &self,
        Parameters(input): Parameters<CreateParams>,
    ) -> Result<String, String> {
        self.create(
            "note",
            input.title,
            input.body,
            input.relative_path,
            std::collections::BTreeMap::new(),
        )
    }
    #[tool(
        name = "notes.update",
        description = "Update a Note after checking its content revision"
    )]
    async fn notes_update(
        &self,
        Parameters(input): Parameters<UpdateParams>,
    ) -> Result<String, String> {
        self.update("note", input)
    }
    #[tool(
        name = "tasks.list",
        description = "List Tasks with optional status and Project filters"
    )]
    async fn tasks_list(
        &self,
        Parameters(input): Parameters<TaskListParams>,
    ) -> Result<String, String> {
        let mut values = self.with_engine(|engine| engine.query_objects(Some("task")))?;
        values.retain(|value| {
            input.status.as_ref().is_none_or(|status| {
                value
                    .properties
                    .get("status")
                    .and_then(serde_json::Value::as_str)
                    == Some(status)
            }) && input.project.as_ref().is_none_or(|project| {
                value
                    .properties
                    .get("project")
                    .and_then(serde_json::Value::as_str)
                    == Some(project)
            })
        });
        serde_json::to_string(&values).map_err(|_| "response serialization failed".into())
    }
    #[tool(name = "tasks.create", description = "Create a Markdown Task")]
    async fn tasks_create(
        &self,
        Parameters(input): Parameters<TaskCreateParams>,
    ) -> Result<String, String> {
        let mut properties = std::collections::BTreeMap::new();
        if let Some(value) = input.status {
            properties.insert("status".into(), serde_json::json!(value));
        }
        if let Some(value) = input.priority {
            properties.insert("priority".into(), serde_json::json!(value));
        }
        if let Some(value) = input.due {
            properties.insert("due".into(), serde_json::json!(value));
        }
        if let Some(value) = input.project {
            properties.insert("project".into(), serde_json::json!(value));
        }
        self.create(
            "task",
            input.title,
            input.body,
            input.relative_path,
            properties,
        )
    }
    #[tool(
        name = "tasks.update",
        description = "Update Task fields after checking its content revision"
    )]
    async fn tasks_update(
        &self,
        Parameters(input): Parameters<UpdateParams>,
    ) -> Result<String, String> {
        self.update("task", input)
    }
    #[tool(name = "projects.list", description = "List managed Projects")]
    async fn projects_list(&self) -> Result<String, String> {
        let values = self.with_engine(|engine| engine.query_objects(Some("project")))?;
        serde_json::to_string(&values).map_err(|_| "response serialization failed".into())
    }

    fn create(
        &self,
        object_type: &str,
        title: String,
        body: Option<String>,
        relative_path: Option<String>,
        properties: std::collections::BTreeMap<String, serde_json::Value>,
    ) -> Result<String, String> {
        let value = self.with_engine(|engine| {
            engine.create_object(CreateObjectInput {
                object_type: object_type.into(),
                title,
                body: body.unwrap_or_default(),
                relative_path,
                properties,
            })
        })?;
        serde_json::to_string(&value).map_err(|_| "response serialization failed".into())
    }
    fn update(&self, expected_type: &str, input: UpdateParams) -> Result<String, String> {
        let properties = input.properties.unwrap_or_default().into_iter().collect();
        let value = self.with_engine(|engine| {
            engine.update_object_typed(
                &input.id,
                expected_type,
                ObjectPatch {
                    title: input.title,
                    body: input.body,
                    properties,
                    remove_properties: Vec::new(),
                    expected_revision: input.expected_revision,
                },
            )
        })?;
        serde_json::to_string(&value).map_err(|_| "response serialization failed".into())
    }
}

#[tool_handler(router=self.tool_router)]
impl ServerHandler for NouraMcp {}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    #[test]
    fn tool_catalog_exposes_the_initial_shared_service_adapters() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "MCP", app_data.path())
                .unwrap();
        let server = NouraMcp::new(engine);
        let names = server
            .tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect::<Vec<_>>();
        assert!(names.contains(&"workspace.search".to_owned()));
        assert!(names.contains(&"notes.create".to_owned()));
        assert!(names.contains(&"tasks.update".to_owned()));
        assert!(names.contains(&"projects.list".to_owned()));
    }

    #[tokio::test]
    async fn task_update_rejects_a_note_id() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "MCP", app_data.path())
                .unwrap();
        let note = engine
            .create_object(CreateObjectInput {
                object_type: "note".into(),
                title: "Note".into(),
                body: String::new(),
                relative_path: Some("note.md".into()),
                properties: std::collections::BTreeMap::new(),
            })
            .unwrap();
        let server = NouraMcp::new(engine);
        let error = server
            .tasks_update(Parameters(UpdateParams {
                id: note.value.id,
                title: Some("Wrong adapter".into()),
                body: None,
                properties: None,
                expected_revision: note.revision,
            }))
            .await
            .unwrap_err();
        assert!(error.starts_with("object_type_mismatch:"));
    }

    #[tokio::test]
    async fn task_create_writes_a_durable_project_task() {
        let workspace = tempdir().unwrap();
        let app_data = tempdir().unwrap();
        let engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "MCP", app_data.path())
                .unwrap();
        let project = engine
            .create_object(CreateObjectInput {
                object_type: "project".into(),
                title: "Demo".into(),
                body: String::new(),
                relative_path: None,
                properties: std::collections::BTreeMap::new(),
            })
            .unwrap()
            .value;
        let server = NouraMcp::new(engine);

        let output = server
            .tasks_create(Parameters(TaskCreateParams {
                title: "Verify board refresh".into(),
                body: Some("- Accepts external edits without data loss".into()),
                relative_path: None,
                status: Some("todo".into()),
                priority: Some("high".into()),
                due: None,
                project: Some(project.id.clone()),
            }))
            .await
            .unwrap();
        let result: serde_json::Value = serde_json::from_str(&output).unwrap();
        let task = &result["value"];
        let id = task["id"].as_str().unwrap();
        let relative_path = task["relativePath"].as_str().unwrap();

        assert!(workspace.path().join(relative_path).is_file());
        let stored = server
            .with_engine(|engine| engine.get_object(id))
            .unwrap()
            .unwrap();
        assert_eq!(stored.properties["priority"], "high");
        assert_eq!(stored.properties["project"], project.id);
        assert_eq!(stored.body, "- Accepts external edits without data loss");
    }

    #[tokio::test]
    async fn local_app_and_mcp_share_canonical_files_and_rebuilt_projections() {
        // A workspace under the current directory keeps notify's reported path
        // representation identical on macOS (`/var` can otherwise become
        // `/private/var` and fail a lexical strip_prefix in the test fixture).
        let workspace = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
        let app_data = tempdir().unwrap();
        let mcp_data = tempdir().unwrap();
        let bootstrap_engine =
            WorkspaceEngine::create_with_app_data(workspace.path(), "Local alpha", app_data.path())
                .unwrap();
        let project = bootstrap_engine
            .create_object(CreateObjectInput {
                object_type: "project".into(),
                title: "Alpha project".into(),
                body: "Canonical project body".into(),
                relative_path: Some("projects/alpha/project.md".into()),
                properties: std::collections::BTreeMap::from([(
                    "status".into(),
                    serde_json::json!("active"),
                )]),
            })
            .unwrap()
            .value;
        let first_task = bootstrap_engine
            .create_object(CreateObjectInput {
                object_type: "task".into(),
                title: "Initial local task".into(),
                body: "Created by the desktop engine".into(),
                relative_path: Some("projects/alpha/tasks/initial.md".into()),
                properties: std::collections::BTreeMap::from([
                    ("status".into(), serde_json::json!("todo")),
                    ("priority".into(), serde_json::json!("medium")),
                    ("due".into(), serde_json::json!("2026-09-15")),
                    ("project".into(), serde_json::json!(project.id.clone())),
                ]),
            })
            .unwrap();

        let project_bytes =
            std::fs::read_to_string(workspace.path().join(&project.relative_path)).unwrap();
        let task_bytes =
            std::fs::read_to_string(workspace.path().join(&first_task.value.relative_path))
                .unwrap();
        assert!(project_bytes.contains(&format!("id: {}", project.id)));
        assert!(project_bytes.contains("Canonical project body"));
        assert!(task_bytes.contains(&format!("project: {}", project.id)));
        assert!(task_bytes.contains("due: 2026-09-15"));

        drop(bootstrap_engine);
        let app_engine =
            WorkspaceEngine::open_with_app_data(workspace.path(), app_data.path()).unwrap();

        let mcp_engine =
            WorkspaceEngine::open_with_app_data(workspace.path(), mcp_data.path()).unwrap();
        let server = NouraMcp::new(mcp_engine);
        std::thread::sleep(std::time::Duration::from_millis(100));
        let updated = server
            .tasks_update(Parameters(UpdateParams {
                id: first_task.value.id.clone(),
                title: Some("MCP revised task".into()),
                body: Some("Updated through the shared MCP adapter".into()),
                properties: Some(serde_json::Map::from_iter([(
                    "status".into(),
                    serde_json::json!("in-progress"),
                )])),
                expected_revision: first_task.revision,
            }))
            .await
            .unwrap();
        let updated: serde_json::Value = serde_json::from_str(&updated).unwrap();
        assert_eq!(updated["value"]["title"], "MCP revised task");

        let created = server
            .tasks_create(Parameters(TaskCreateParams {
                title: "MCP follow-up".into(),
                body: Some("Second canonical task".into()),
                relative_path: Some("projects/alpha/tasks/follow-up.md".into()),
                status: Some("todo".into()),
                priority: Some("high".into()),
                due: Some("2026-09-16".into()),
                project: Some(project.id.clone()),
            }))
            .await
            .unwrap();
        let created: serde_json::Value = serde_json::from_str(&created).unwrap();
        let second_id = created["value"]["id"].as_str().unwrap().to_owned();

        let mut reconciled_paths = Vec::new();
        for _ in 0..10 {
            reconciled_paths.extend(
                app_engine
                    .poll_external_changes(std::time::Duration::from_millis(50))
                    .unwrap(),
            );
            let tasks = app_engine.query_objects(Some("task")).unwrap();
            if tasks.len() == 2 && tasks.iter().any(|task| task.title == "MCP revised task") {
                break;
            }
        }
        // Some sandboxed test hosts suppress native filesystem notifications.
        // The app's focus recovery performs this same full-file reconciliation.
        if reconciled_paths.is_empty() {
            app_engine.reconcile().unwrap();
        }

        let tasks = app_engine.query_objects(Some("task")).unwrap();
        assert_eq!(tasks.len(), 2);
        let revised = tasks
            .iter()
            .find(|task| task.id == first_task.value.id)
            .unwrap();
        assert_eq!(revised.title, "MCP revised task");
        assert_eq!(revised.properties["status"], "in-progress");
        assert_eq!(revised.properties["project"], project.id);
        let follow_up = tasks.iter().find(|task| task.id == second_id).unwrap();
        assert_eq!(follow_up.properties["status"], "todo");
        assert_eq!(follow_up.properties["priority"], "high");
        assert_eq!(follow_up.properties["project"], project.id);

        let search = app_engine
            .search(&SearchInput {
                query: "MCP".into(),
                object_type: None,
                path_prefix: None,
                limit: None,
            })
            .unwrap();
        assert_eq!(search.len(), 2);
        let calendar = app_engine.calendar("2026-09-15", "2026-09-17").unwrap();
        assert_eq!(calendar.len(), 2);
        assert!(calendar.iter().any(|entry| entry.source_id == second_id));
    }
}
