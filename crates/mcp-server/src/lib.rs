//! Noura's MCP server. An AI tool such as Claude Desktop or Cursor starts
//! it with a workspace folder and talks to it over stdio.
//!
//! The kernel tools `workspace.search`, `workspace.read`, `notes.create`
//! and `notes.update` adapt `app-core`'s workspace services, and each
//! tool of a plugin the workspace enables becomes `<plugin>.<tool>`, such
//! as `tasks.list`, which `app-core` runs through the plugin host with
//! this device's approvals. The server adds no rules of its own: every
//! change follows the app's file rules and is on disk before the tool
//! answers. [`test_connection`] starts a configured command as an AI tool
//! would, for Settings' Test connection.

mod probe;

use std::{ffi::OsString, future::Future, path::PathBuf, pin::Pin, sync::Arc};

use app_core::{AgentWorkspace, AppError, Item, NoteChanges, PluginTool, SearchHit, SearchQuery};
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
        wrapper::Parameters,
    },
    model::{CallToolResponse, CallToolResult, ContentBlock, JsonObject, Tool},
    schemars, tool, tool_handler, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};

pub use probe::{Answered, test_connection};

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SearchParams {
    /// The words to find. Every word must match, a partial word matches
    /// the start of a word, and punctuation counts as plain text.
    pub query: String,
    /// Only items of this type, such as `note`, `task` or `project`.
    #[serde(rename = "type")]
    pub object_type: Option<String>,
    /// Only files in this workspace folder and its subfolders, such as
    /// `Notes/Ideas`.
    pub folder: Option<String>,
    /// Only notes with this tag.
    pub tag: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ReadParams {
    /// The stable ID of a note, task, project or other object.
    pub id: Option<String>,
    /// The workspace path of a Markdown or plain-text file, as search
    /// returns it.
    pub path: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct CreateNoteParams {
    /// The note's title, which names its file.
    pub title: String,
    /// The note's text.
    pub body: Option<String>,
    /// The workspace folder to put it in, such as `Notes/Ideas`. The top
    /// of the workspace when left out.
    pub folder: Option<String>,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct UpdateNoteParams {
    /// The note's stable ID.
    pub id: String,
    /// The revision you read. A note that changed since stays as it is.
    pub expected_revision: String,
    /// A new title, which renames the note's file.
    pub title: Option<String>,
    /// The note's new text.
    pub body: Option<String>,
    /// Properties to set. A null value removes one.
    pub properties: Option<serde_json::Map<String, Value>>,
}

/// The server for one workspace.
#[derive(Clone)]
pub struct NouraMcp {
    agent: Arc<AgentWorkspace>,
    tool_router: ToolRouter<Self>,
}

impl NouraMcp {
    /// The server for `agent`'s workspace: the kernel tools, then the
    /// tools of the plugins that started. A plugin tool whose name a
    /// kernel tool already uses is left out.
    pub fn new(agent: Arc<AgentWorkspace>) -> Self {
        let mut tool_router = Self::tool_router();
        for tool in agent.plugin_tools() {
            let name = format!("{}.{}", tool.plugin_id, tool.name);
            if tool_router.has_route(&name) {
                tracing::warn!(
                    name,
                    "a plugin tool has a kernel tool's name and was left out"
                );
                continue;
            }
            match plugin_route(name, tool) {
                Some(route) => tool_router.add_route(route),
                None => tracing::warn!("a plugin tool's schema isn't a JSON object"),
            }
        }
        Self { agent, tool_router }
    }

    /// The names of the tools the server offers, sorted.
    pub fn tool_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self
            .tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect();
        names.sort();
        names
    }

    /// Run `work` against the workspace off the async threads, and answer
    /// with its JSON or its error.
    async fn run(
        &self,
        work: impl FnOnce(&AgentWorkspace) -> Result<Value, AppError> + Send + 'static,
    ) -> Result<String, String> {
        let agent = self.agent.clone();
        match tokio::task::spawn_blocking(move || work(&agent)).await {
            Ok(Ok(value)) => Ok(value.to_string()),
            Ok(Err(error)) => Err(error_json(&error)),
            Err(_) => Err(stopped()),
        }
    }
}

#[tool_router(router = tool_router)]
impl NouraMcp {
    #[tool(
        name = "workspace.search",
        description = "Search the workspace's notes and other files. Each hit has the file's \
                       path, title, type, folder and the text around the match."
    )]
    async fn workspace_search(
        &self,
        Parameters(input): Parameters<SearchParams>,
    ) -> Result<String, String> {
        let query = SearchQuery {
            text: input.query,
            object_type: input.object_type,
            folder: input.folder.unwrap_or_default(),
            tag: input.tag.unwrap_or_default(),
        };
        self.run(move |agent| {
            let hits = agent.workspace().search(&query)?;
            Ok(Value::Array(hits.iter().map(hit_json).collect()))
        })
        .await
    }

    #[tool(
        name = "workspace.read",
        description = "Read a note, task, project or other Markdown file by its stable ID or \
                       its path: its properties, its text and the revision a change must name."
    )]
    async fn workspace_read(
        &self,
        Parameters(input): Parameters<ReadParams>,
    ) -> Result<String, String> {
        self.run(move |agent| {
            let item = match (input.id, input.path) {
                (Some(id), None) => agent.workspace().read_item(&id)?,
                (None, Some(path)) => agent.workspace().read_item_at(&path)?,
                _ => {
                    return Err(AppError::new(
                        "id_or_path_required",
                        "Pass either id or path.",
                    ));
                }
            };
            Ok(item_json(&item))
        })
        .await
    }

    #[tool(
        name = "notes.create",
        description = "Create a note with a title and text, in a workspace folder or at the \
                       top of the workspace. The title names its file and must be free there."
    )]
    async fn notes_create(
        &self,
        Parameters(input): Parameters<CreateNoteParams>,
    ) -> Result<String, String> {
        self.run(move |agent| {
            let note = agent.workspace().create_named_note(
                input.folder.as_deref().unwrap_or_default(),
                &input.title,
                input.body.as_deref().unwrap_or_default(),
            )?;
            Ok(item_json(&note))
        })
        .await
    }

    #[tool(
        name = "notes.update",
        description = "Change a note's title, text or properties, by its stable ID. Pass the \
                       revision you read: a note that changed since stays as it is."
    )]
    async fn notes_update(
        &self,
        Parameters(input): Parameters<UpdateNoteParams>,
    ) -> Result<String, String> {
        let changes = NoteChanges {
            title: input.title,
            body: input.body,
            properties: input.properties.unwrap_or_default().into_iter().collect(),
            expected_revision: input.expected_revision,
        };
        self.run(move |agent| {
            let note = agent.workspace().update_note(&input.id, &changes)?;
            Ok(item_json(&note))
        })
        .await
    }
}

#[tool_handler(
    router = self.tool_router,
    name = "noura",
    instructions = "Search, read and change the notes, tasks and projects of one Noura \
                    workspace. Read an item before you change it, and pass the revision you \
                    read. Your changes reach the files at once, without asking the person."
)]
impl ServerHandler for NouraMcp {}

type ToolFuture<'a> =
    Pin<Box<dyn Future<Output = Result<CallToolResponse, ErrorData>> + Send + 'a>>;

/// The route for a plugin's tool, named `name`, or `None` when its schema
/// isn't a JSON object.
fn plugin_route(name: String, tool: PluginTool) -> Option<ToolRoute<NouraMcp>> {
    let schema: JsonObject = serde_json::from_str(&tool.input_schema).ok()?;
    let PluginTool {
        plugin_id,
        name: tool_name,
        description,
        ..
    } = tool;
    let attr = Tool::new(name, description, Arc::new(schema));
    Some(ToolRoute::new_dyn(
        attr,
        move |context: ToolCallContext<'_, NouraMcp>| -> ToolFuture<'_> {
            let agent = context.service.agent.clone();
            let arguments = Value::Object(context.arguments.unwrap_or_default()).to_string();
            let (plugin_id, tool_name) = (plugin_id.clone(), tool_name.clone());
            Box::pin(async move {
                let answer = tokio::task::spawn_blocking(move || {
                    agent.call_plugin_tool(&plugin_id, &tool_name, &arguments)
                })
                .await;
                let result = match answer {
                    Ok(Ok(answer)) => CallToolResult::success(vec![ContentBlock::text(answer)]),
                    Ok(Err(error)) => {
                        CallToolResult::error(vec![ContentBlock::text(error_json(&error))])
                    }
                    Err(_) => CallToolResult::error(vec![ContentBlock::text(stopped())]),
                };
                Ok(CallToolResponse::Complete(result))
            })
        },
    ))
}

/// An error as the agent reads it: its stable code and a message.
fn error_json(error: &AppError) -> String {
    json!({ "code": error.code, "message": error.message }).to_string()
}

fn stopped() -> String {
    json!({
        "code": "tool_stopped",
        "message": "The tool stopped before it answered. Nothing else changed.",
    })
    .to_string()
}

fn hit_json(hit: &SearchHit) -> Value {
    json!({
        "path": hit.relative_path,
        "title": hit.title,
        "type": hit.object_type,
        "folder": hit.folder,
        "snippet": hit.snippet,
    })
}

fn item_json(item: &Item) -> Value {
    json!({
        "id": item.id,
        "type": item.object_type,
        "title": item.title,
        "path": item.relative_path,
        "revision": item.revision,
        "properties": item.properties,
        "body": item.body,
    })
}

/// Serve the workspace in `folder`, with per-device state in `app_data`,
/// over stdio until the AI tool disconnects. Both the desktop binary's
/// `mcp` mode and the standalone `noura-mcp` binary use this.
pub async fn serve_stdio(app_data: PathBuf, folder: PathBuf) -> Result<(), String> {
    use rmcp::ServiceExt;
    let agent = tokio::task::spawn_blocking(move || AgentWorkspace::open(app_data, &folder))
        .await
        .map_err(|_| "Noura stopped while it opened the workspace.".to_owned())?
        .map_err(|error| error.message)?;
    let server = NouraMcp::new(Arc::new(agent))
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| {
            tracing::warn!(%error, "the MCP connection didn't start");
            "The AI tool didn't start an MCP connection.".to_owned()
        })?;
    server.waiting().await.map_err(|error| {
        tracing::warn!(%error, "the MCP connection ended with an error");
        "The MCP connection ended with an error.".to_owned()
    })?;
    Ok(())
}

/// The workspace folder in `--workspace <path>`, the only argument the
/// server takes, read without assuming the path is UTF-8.
pub fn workspace_argument(
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<PathBuf, String> {
    use lexopt::prelude::*;
    let usage = || "Usage: noura mcp --workspace <folder>".to_owned();
    let mut parser = lexopt::Parser::from_args(arguments);
    let mut workspace = None;
    while let Some(argument) = parser.next().map_err(|error| error.to_string())? {
        match argument {
            Long("workspace") => {
                let value = parser.value().map_err(|_| usage())?;
                workspace = Some(PathBuf::from(value));
            }
            _ => return Err(usage()),
        }
    }
    workspace.ok_or_else(usage)
}
