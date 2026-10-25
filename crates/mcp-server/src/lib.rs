//! Noura's MCP server. An AI tool such as Claude Desktop or Cursor starts
//! it with a workspace folder and talks to it over stdio.
//!
//! The kernel tools `workspace.search`, `workspace.read`, `notes.create`
//! and `notes.update` adapt `app-core`'s workspace services, and each
//! tool of a plugin the workspace enables becomes `<plugin>.<tool>`, such
//! as `tasks.list`, which `app-core` runs through the plugin host with
//! this device's approvals. The server answers MCP's initialize request
//! before the plugins finish starting, and a plugin turned off since then
//! leaves the tool list on its next call. The server adds no rules of its
//! own: every change follows the app's file rules and is on disk before
//! the tool answers. [`test_connection`] starts a configured command as an
//! AI tool would, for Settings' Test connection.

mod probe;

use std::{
    collections::BTreeMap,
    ffi::OsString,
    future::Future,
    path::PathBuf,
    pin::Pin,
    sync::{Arc, OnceLock, PoisonError, RwLock, RwLockReadGuard, RwLockWriteGuard},
};

use app_core::{AgentWorkspace, AppError, Item, NoteChanges, PluginTool, SearchHit, SearchQuery};
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{
        router::tool::{ToolRoute, ToolRouter},
        tool::ToolCallContext,
        wrapper::Parameters,
    },
    model::{
        CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock,
        Implementation, JsonObject, ListToolsResult, PaginatedRequestParams, ProtocolVersion,
        ServerCapabilities, ServerInfo, Tool,
    },
    schemars,
    service::{NotificationContext, Peer, RequestContext},
    tool, tool_router,
};
use serde::Deserialize;
use serde_json::{Value, json};

pub use probe::{Answered, test_connection};

/// What the server tells the AI tool about itself.
const INSTRUCTIONS: &str = "Search, read and change the notes, tasks and projects of one Noura \
                            workspace. Read an item before you change it, and pass the revision \
                            you read. Your changes reach the files at once, without asking the \
                            person.";

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
    tools: Arc<RwLock<Tools>>,
    /// Set once the plugins finished starting and their tools joined the
    /// router.
    plugins_added: Arc<tokio::sync::OnceCell<()>>,
    /// The AI tool, told when a plugin's tools go away.
    peer: Arc<OnceLock<Peer<RoleServer>>>,
}

/// The tools the server offers.
struct Tools {
    router: ToolRouter<NouraMcp>,
    /// Each plugin's tool names, as the router holds them.
    by_plugin: BTreeMap<String, Vec<String>>,
}

impl NouraMcp {
    /// The server for `agent`'s workspace, with the kernel tools. The
    /// tools of the plugins that start join them once the plugins finish
    /// starting, before the first tool list or plugin tool call answers,
    /// so the server answers MCP's initialize request without waiting for
    /// plugin code to load. A plugin tool whose name a kernel tool already
    /// uses is left out.
    pub fn new(agent: Arc<AgentWorkspace>) -> Self {
        Self {
            agent,
            tools: Arc::new(RwLock::new(Tools {
                router: Self::tool_router(),
                by_plugin: BTreeMap::new(),
            })),
            plugins_added: Arc::new(tokio::sync::OnceCell::new()),
            peer: Arc::new(OnceLock::new()),
        }
    }

    /// The router with every tool, once the plugins' tools joined it.
    async fn router(&self) -> ToolRouter<Self> {
        self.plugins_added
            .get_or_init(|| self.add_plugin_tools())
            .await;
        self.read_tools().router.clone()
    }

    /// Wait for the plugins to finish starting, and add their tools.
    async fn add_plugin_tools(&self) {
        let agent = self.agent.clone();
        let started = tokio::task::spawn_blocking(move || agent.plugin_tools()).await;
        let Ok(plugin_tools) = started else {
            tracing::warn!("the plugins' tools weren't read");
            return;
        };
        let mut tools = self.write_tools();
        for tool in plugin_tools {
            let name = format!("{}.{}", tool.plugin_id, tool.name);
            if tools.router.has_route(&name) {
                tracing::warn!(
                    name,
                    "a plugin tool has a kernel tool's name and was left out"
                );
                continue;
            }
            let plugin_id = tool.plugin_id.clone();
            match plugin_route(name.clone(), tool) {
                Some(route) => {
                    tools.router.add_route(route);
                    tools.by_plugin.entry(plugin_id).or_default().push(name);
                }
                None => tracing::warn!("a plugin tool's schema isn't a JSON object"),
            }
        }
    }

    /// Take the tools of the plugin `plugin_id` off the list, as after the
    /// app turned it off, and tell the AI tool the list changed.
    fn drop_plugin_tools(&self, plugin_id: &str) {
        let names = {
            let mut tools = self.write_tools();
            let names = tools.by_plugin.remove(plugin_id).unwrap_or_default();
            for name in &names {
                tools.router.remove_route(name);
            }
            names
        };
        if names.is_empty() {
            return;
        }
        if let Some(peer) = self.peer.get().cloned() {
            tokio::spawn(async move {
                if let Err(error) = peer.notify_tool_list_changed().await {
                    tracing::warn!(%error, "the AI tool wasn't told the tool list changed");
                }
            });
        }
    }

    fn read_tools(&self) -> RwLockReadGuard<'_, Tools> {
        self.tools.read().unwrap_or_else(PoisonError::into_inner)
    }

    fn write_tools(&self) -> RwLockWriteGuard<'_, Tools> {
        self.tools.write().unwrap_or_else(PoisonError::into_inner)
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

impl ServerHandler for NouraMcp {
    fn get_info(&self) -> ServerInfo {
        let capabilities = ServerCapabilities::builder()
            .enable_tools()
            .enable_tool_list_changed()
            .build();
        ServerInfo::new(capabilities)
            .with_server_info(Implementation::new("noura", env!("CARGO_PKG_VERSION")))
            .with_instructions(INSTRUCTIONS.to_owned())
    }

    async fn on_initialized(&self, context: NotificationContext<RoleServer>) {
        let _ = self.peer.set(context.peer);
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let mut result = ListToolsResult::with_all_items(self.router().await.list_all());
        if context
            .protocol_version()
            .is_some_and(|version| version >= ProtocolVersion::V_2026_07_28)
        {
            // The list changes when a plugin is turned off.
            result.ttl_ms = Some(0);
            result.cache_scope = Some(CacheScope::Public);
        }
        Ok(result)
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        // A kernel tool answers while the plugins still start.
        let kernel = {
            let tools = self.read_tools();
            (tools.router.has_route(&request.name)
                && !tools
                    .by_plugin
                    .values()
                    .flatten()
                    .any(|name| *name == request.name))
            .then(|| tools.router.clone())
        };
        let router = match kernel {
            Some(router) => router,
            None => self.router().await,
        };
        router
            .call(ToolCallContext::new(self, request, context))
            .await
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.read_tools().router.get(name).cloned()
    }
}

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
            let server = context.service.clone();
            let arguments = Value::Object(context.arguments.unwrap_or_default()).to_string();
            let (plugin_id, tool_name) = (plugin_id.clone(), tool_name.clone());
            Box::pin(async move {
                let agent = server.agent.clone();
                let plugin = plugin_id.clone();
                let answer = tokio::task::spawn_blocking(move || {
                    agent.call_plugin_tool(&plugin, &tool_name, &arguments)
                })
                .await;
                let result = match answer {
                    Ok(Ok(answer)) => CallToolResult::success(vec![ContentBlock::text(answer)]),
                    Ok(Err(error)) => {
                        // A plugin turned off or uninstalled since the
                        // server started won't run again here.
                        if matches!(
                            error.code.as_str(),
                            "plugin_disabled" | "plugin_not_installed"
                        ) {
                            server.drop_plugin_tools(&plugin_id);
                        }
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

/// Run the server as a command: read `--workspace <folder>` from
/// `arguments`, serve that workspace over stdio until the AI tool
/// disconnects, and return the exit code. Stdout carries only MCP, and
/// warnings and the reason for a failure go to stderr, which AI tools
/// keep in their logs. The desktop binary's `mcp` mode and `noura-mcp`
/// both run this.
pub fn run(arguments: impl IntoIterator<Item = OsString>) -> std::process::ExitCode {
    use std::process::ExitCode;
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(false)
        .with_env_filter(filter)
        .try_init();
    let folder = match workspace_argument(arguments) {
        Ok(folder) => folder,
        Err(usage) => {
            eprintln!("{usage}");
            return ExitCode::from(2);
        }
    };
    let Some(app_data) = app_core::default_app_data_dir() else {
        eprintln!("This system has no app data folder for Noura.");
        return ExitCode::FAILURE;
    };
    let served = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|error| {
            tracing::warn!(%error, "the MCP server's runtime didn't start");
            "Noura couldn't start its MCP server.".to_owned()
        })
        .and_then(|runtime| runtime.block_on(serve_stdio(app_data, folder)));
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
