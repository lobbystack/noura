//! The AI plugin in a running app with a fake provider: a provider client
//! that records each request and streams what the test tells it to, and
//! helpers that set up providers and read and drive the AI page.

use std::{
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
};

use app_core::{
    AiProviderInput, App, Field, InputValue, PluginEvent, Submission, Target, ViewKind, ViewNode,
};
use local_core::{
    AiContentPart, AiProviderEventStream, AiProviderStreamEvent, AiStopReason, AiStreamer,
    AiToolCall, AiTransportMessage, CoreError, ErrorCategory, ResolvedAiStreamRequest,
};
use tokio::sync::mpsc::{UnboundedSender, unbounded_channel};

use super::plugins::{self, PluginFolder};

/// The AI page.
pub const PAGE: &str = "plugin:ai:ai";

/// The AI plugin's manifest, as its crate ships it.
pub const MANIFEST: &str = include_str!("../../../plugins/ai/plugin.toml");

/// What the fake provider's failure carries: a provider's own answer, a
/// header and a key, none of which may reach a plugin or a file.
pub const RAW_FAILURE: &str = "HTTP 401 {\"error\":\"bad key\"} authorization: Bearer sk-raw-key";

/// What a request carried to the fake provider.
#[derive(Debug, Clone)]
pub struct Request {
    pub provider: String,
    pub kind: String,
    pub model: String,
    pub endpoint: Option<String>,
    pub key: Option<String>,
    /// Each message as `role: text`, tool calls and results as JSON.
    pub messages: Vec<String>,
    pub tools: Vec<String>,
}

/// What the fake provider sends next on one request's stream.
enum Say {
    Text(String),
    Done(Vec<AiToolCall>),
    Fail,
}

/// One request's stream, open until the test ends it or Noura drops it.
struct Open {
    sender: UnboundedSender<Say>,
    /// Set when Noura dropped the stream before it ended.
    dropped: Arc<Mutex<bool>>,
}

/// A provider client that answers each request as the test says. It
/// streams nothing until the test calls [`FakeProvider::say`].
#[derive(Default)]
pub struct FakeProvider {
    requests: Mutex<Vec<Request>>,
    open: Mutex<Vec<Open>>,
}

/// Records that Noura dropped a stream it hadn't finished, as when a
/// request is cancelled.
struct DropGuard {
    dropped: Arc<Mutex<bool>>,
    ended: bool,
}

impl Drop for DropGuard {
    fn drop(&mut self) {
        if !self.ended {
            *self.dropped.lock().unwrap() = true;
        }
    }
}

impl FakeProvider {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Every request so far, oldest first.
    pub fn requests(&self) -> Vec<Request> {
        self.requests.lock().unwrap().clone()
    }

    /// Stream `text` on request `index`.
    pub fn say(&self, index: usize, text: &str) {
        self.send(index, Say::Text(text.to_owned()));
    }

    /// End request `index`'s answer.
    pub fn finish(&self, index: usize) {
        self.send(index, Say::Done(Vec::new()));
    }

    /// End request `index`'s answer with calls to these tools.
    pub fn call_tools(&self, index: usize, calls: Vec<AiToolCall>) {
        self.send(index, Say::Done(calls));
    }

    /// Fail request `index` as a provider that answered with an error.
    pub fn fail(&self, index: usize) {
        self.send(index, Say::Fail);
    }

    /// Whether Noura dropped request `index`'s stream before it ended.
    pub fn cancelled(&self, index: usize) -> bool {
        *self.open.lock().unwrap()[index].dropped.lock().unwrap()
    }

    fn send(&self, index: usize, say: Say) {
        let open = self.open.lock().unwrap();
        open[index]
            .sender
            .send(say)
            .expect("the request's stream is open");
    }
}

impl AiStreamer for FakeProvider {
    fn stream(
        &self,
        request: ResolvedAiStreamRequest,
    ) -> Pin<Box<dyn Future<Output = local_core::Result<AiProviderEventStream>> + Send + '_>> {
        let recorded = Request {
            provider: request.provider.id.clone(),
            kind: request.provider.kind.clone(),
            model: request.provider.model.clone(),
            endpoint: request.provider.endpoint.clone(),
            key: request.credential.clone(),
            messages: request.messages.iter().map(message_line).collect(),
            tools: request.tools.iter().map(|tool| tool.name.clone()).collect(),
        };
        let (sender, receiver) = unbounded_channel();
        let dropped = Arc::new(Mutex::new(false));
        self.open.lock().unwrap().push(Open {
            sender,
            dropped: dropped.clone(),
        });
        self.requests.lock().unwrap().push(recorded);
        let guard = DropGuard {
            dropped,
            ended: false,
        };
        Box::pin(async move {
            let stream = futures::stream::unfold(
                (receiver, guard),
                |(mut receiver, mut guard)| async move {
                    let event = match receiver.recv().await? {
                        Say::Text(text) => Ok(AiProviderStreamEvent::TextDelta(text)),
                        Say::Done(tool_calls) => {
                            guard.ended = true;
                            Ok(AiProviderStreamEvent::Completed {
                                usage: None,
                                stop_reason: Some(AiStopReason::Completed { raw: "stop".into() }),
                                tool_calls,
                            })
                        }
                        Say::Fail => {
                            guard.ended = true;
                            // A careless client could pass along what the
                            // provider answered, its headers and the key.
                            let mut error = CoreError::new(
                                "provider_request_failed",
                                ErrorCategory::Provider,
                                RAW_FAILURE,
                                "ai_stream",
                            );
                            error.details = Some(serde_json::json!({ "body": RAW_FAILURE }));
                            Err(error)
                        }
                    };
                    Some((event, (receiver, guard)))
                },
            );
            let stream: AiProviderEventStream = Box::pin(stream);
            Ok(stream)
        })
    }
}

fn message_line(message: &AiTransportMessage) -> String {
    let parts: Vec<String> = message
        .content
        .iter()
        .map(|part| match part {
            AiContentPart::Text { text } => text.clone(),
            AiContentPart::ToolCall {
                call_id,
                name,
                arguments,
            } => format!("call {call_id} {name} {arguments}"),
            AiContentPart::ToolResult {
                call_id, content, ..
            } => format!("result {call_id} {content}"),
        })
        .collect();
    format!("{}: {}", message.role, parts.join(" | "))
}

/// A provider for `kind` and `model`, turned on, with `key` when given.
pub fn provider(name: &str, kind: &str, model: &str, key: Option<&str>) -> AiProviderInput {
    AiProviderInput {
        id: None,
        name: name.to_owned(),
        kind: kind.to_owned(),
        model: model.to_owned(),
        endpoint: None,
        enabled: true,
        key: key.map(str::to_owned),
    }
}

/// The AI plugin's folder, or `None` when it isn't built.
pub fn folder() -> Option<PluginFolder> {
    plugins::built("ai_plugin")
        .map(|component| PluginFolder::new(MANIFEST, "ai_plugin.wasm", &component))
}

/// The AI plugin installed and on in `app`'s workspace, talking to
/// `provider`, with its page drawn, or `None` when it isn't built.
pub fn with_ai(app: &mut App, provider: &Arc<FakeProvider>) -> Option<PluginFolder> {
    let folder = folder()?;
    app.set_ai_client(provider.clone());
    plugins::install_and_enable(app, folder.path(), "ai");
    plugins::wait_until(app, "the AI page shows", |app| {
        app.plugin_view(PAGE).is_some()
    });
    if let Some(Err(problem)) = app.plugin_view(PAGE) {
        panic!("the AI page doesn't draw: {problem}");
    }
    Some(folder)
}

/// The node with `id` under `node`.
pub fn find<'a>(node: &'a ViewNode, id: &str) -> Option<&'a ViewNode> {
    if node.id == id {
        return Some(node);
    }
    node.children.iter().find_map(|child| find(child, id))
}

/// The page's view, which must draw.
pub fn page(app: &App) -> app_core::View {
    app.plugin_view(PAGE)
        .expect("the AI page shows")
        .expect("the AI page draws")
}

/// Every text and Markdown node of the page, top to bottom, Markdown as
/// `md: <text>`.
pub fn lines(app: &App) -> Vec<String> {
    let view = page(app);
    let mut lines = Vec::new();
    collect(view.root(), &mut lines);
    lines
}

fn collect(node: &ViewNode, lines: &mut Vec<String>) {
    match &node.kind {
        ViewKind::Text(text) => lines.push(text.clone()),
        ViewKind::Markdown(text) => lines.push(format!("md: {text}")),
        _ => {}
    }
    for child in &node.children {
        collect(child, lines);
    }
}

/// The page's buttons' labels.
pub fn buttons(app: &App) -> Vec<String> {
    let view = page(app);
    let mut found = Vec::new();
    buttons_under(view.root(), &mut found);
    found
}

fn buttons_under(node: &ViewNode, found: &mut Vec<String>) {
    if let ViewKind::Button { label } = &node.kind {
        found.push(label.clone());
    }
    for child in &node.children {
        buttons_under(child, found);
    }
}

/// The titles the sidebar lists, top to bottom.
pub fn chat_titles(app: &App) -> Vec<String> {
    let Some(Ok(view)) = app.plugin_sidebar(PAGE) else {
        return Vec::new();
    };
    find(view.root(), "chats")
        .map(|list| {
            list.children
                .iter()
                .filter_map(|item| match &item.kind {
                    ViewKind::Item { label, .. } => Some(label.clone()),
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The chat IDs the sidebar lists, top to bottom.
pub fn chat_ids(app: &App) -> Vec<String> {
    let Some(Ok(view)) = app.plugin_sidebar(PAGE) else {
        return Vec::new();
    };
    find(view.root(), "chats")
        .map(|list| list.children.iter().map(|item| item.id.clone()).collect())
        .unwrap_or_default()
}

/// Write `text` in the message box and send it to the provider `provider`.
pub fn send(app: &mut App, text: &str, provider: &str) {
    let view = page(app);
    let composer = find(view.root(), "composer").expect("the message box shows");
    let input = composer
        .children
        .iter()
        .find(|child| child.id.starts_with("message-"))
        .expect("the message box has a message input")
        .id
        .clone();
    app.send_plugin_event(
        PAGE,
        &PluginEvent::Submitted(Submission {
            node: "composer".to_owned(),
            fields: vec![
                Field {
                    node: input,
                    value: InputValue::Multiline(text.to_owned()),
                },
                Field {
                    node: "provider".to_owned(),
                    value: InputValue::Choice(Some(provider.to_owned())),
                },
            ],
        }),
    );
    super::settle(app);
}

/// Press the button `node` on the page.
pub fn press(app: &mut App, node: &str) {
    app.send_plugin_event(PAGE, &PluginEvent::Pressed(node.to_owned()));
    super::settle(app);
}

/// Open the chat `id` from the sidebar.
pub fn open_chat(app: &mut App, id: &str) {
    app.send_plugin_sidebar_event(
        PAGE,
        &PluginEvent::Activated(Target {
            node: "chats".to_owned(),
            item: id.to_owned(),
        }),
    );
    super::settle(app);
}

/// Start a new chat from the sidebar.
pub fn new_chat(app: &mut App) {
    app.send_plugin_sidebar_event(PAGE, &PluginEvent::Pressed("new-chat".to_owned()));
    super::settle(app);
}

/// The files under the workspace's `chats` folder, by path, with their
/// text.
pub fn chat_files(root: &std::path::Path) -> Vec<(String, String)> {
    let mut found = Vec::new();
    walk(&root.join("chats"), root, &mut found);
    found.sort();
    found
}

fn walk(folder: &std::path::Path, root: &std::path::Path, found: &mut Vec<(String, String)>) {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            walk(&path, root, found);
        } else {
            let relative = path
                .strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            found.push((relative, std::fs::read_to_string(&path).unwrap_or_default()));
        }
    }
}

/// Poll until `done` holds, for up to 20 seconds.
pub fn wait_until(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    plugins::wait_until(app, what, done);
}
