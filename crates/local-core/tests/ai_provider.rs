//! The real provider client, `GenAiStreamer`, behind `AiFoundation` against
//! a provider on this computer that speaks each provider's HTTP streaming
//! format. The request carries the key from the credential store, the
//! messages and the tools. Answers come back as frames, in order, and Stop
//! ends the request.

use std::{sync::Arc, time::Duration};

use local_core::{
    AiConsentDataCategory, AiConsentGrantInput, AiContentPart, AiFoundation, AiModelRef,
    AiProviderConfig, AiStopReason, AiStreamEvent, AiStreamInput, AiStreamOperation, AiToolCall,
    AiToolDefinition, AiTransportMessage, AiUsage, MemoryCredentials,
};
use serde_json::{Value, json};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};

const WORKSPACE: &str = "workspace_01jbcdefghjkmnpqrstvwxyz0a";
const POLICY: &str = "2026-09";
const OPERATION: &str = "01890a5d-ac96-7000-8000-000000000001";
const KEY: &str = "sk-local-test-key";
const PATIENCE: Duration = Duration::from_secs(20);

/// What the provider heard.
struct Heard {
    /// The request line, such as `POST /v1/chat/completions HTTP/1.1`.
    request_line: String,
    /// Header lines, with lowercase names.
    headers: Vec<String>,
    body: Value,
}

impl Heard {
    fn header(&self, name: &str) -> Option<&str> {
        let prefix = format!("{name}: ");
        self.headers
            .iter()
            .find_map(|line| line.strip_prefix(&prefix))
    }
}

/// How the provider answers one request.
enum Answer {
    /// A status line and a body, then the connection closes.
    Status(&'static str, &'static str),
    /// These server-sent events, then the connection closes.
    Events(Vec<Value>),
    /// These events, then the connection stays open. The second value of
    /// the provider's result says whether the client closed it.
    EventsThenWait(Vec<Value>),
}

/// A provider on this computer that answers one request.
struct LocalProvider {
    endpoint: String,
    answered: JoinHandle<(Heard, bool)>,
}

impl LocalProvider {
    async fn start(answer: Answer) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
        let answered = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let heard = read_request(&mut socket).await;
            let closed = answer_request(&mut socket, answer).await;
            (heard, closed)
        });
        Self { endpoint, answered }
    }

    async fn heard(self) -> (Heard, bool) {
        tokio::time::timeout(PATIENCE, self.answered)
            .await
            .expect("the provider heard nothing")
            .unwrap()
    }
}

async fn read_request(socket: &mut TcpStream) -> Heard {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end;
        }
        let read = socket.read(&mut chunk).await.unwrap();
        assert_ne!(read, 0, "the client closed before its request ended");
        bytes.extend_from_slice(&chunk[..read]);
    };
    let head = String::from_utf8(bytes[..head_end].to_vec()).unwrap();
    let mut lines = head.split("\r\n");
    let request_line = lines.next().unwrap().to_owned();
    let headers: Vec<String> = lines
        .map(|line| {
            let (name, value) = line.split_once(':').unwrap();
            format!("{}: {}", name.to_lowercase(), value.trim())
        })
        .collect();
    let length: usize = headers
        .iter()
        .find_map(|line| line.strip_prefix("content-length: "))
        .expect("the request has a length")
        .parse()
        .unwrap();
    let body_start = head_end + 4;
    while bytes.len() < body_start + length {
        let read = socket.read(&mut chunk).await.unwrap();
        assert_ne!(read, 0, "the client closed before its request ended");
        bytes.extend_from_slice(&chunk[..read]);
    }
    Heard {
        request_line,
        headers,
        body: serde_json::from_slice(&bytes[body_start..body_start + length]).unwrap(),
    }
}

/// Whether the client closed the connection while the provider waited.
async fn answer_request(socket: &mut TcpStream, answer: Answer) -> bool {
    const STREAM: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                          cache-control: no-cache\r\nconnection: close\r\n\r\n";
    let send_events = |events: Vec<Value>| {
        events
            .into_iter()
            .map(|event| match event.as_str() {
                Some(line) => format!("{line}\n\n"),
                None => {
                    let name = event.get("type").and_then(Value::as_str);
                    let name = name.map_or(String::new(), |name| format!("event: {name}\n"));
                    format!("{name}data: {event}\n\n")
                }
            })
            .collect::<String>()
    };
    match answer {
        Answer::Status(status, body) => {
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
                 connection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            false
        }
        Answer::Events(events) => {
            let response = format!("{STREAM}{}", send_events(events));
            socket.write_all(response.as_bytes()).await.unwrap();
            socket.shutdown().await.unwrap();
            false
        }
        Answer::EventsThenWait(events) => {
            let response = format!("{STREAM}{}", send_events(events));
            socket.write_all(response.as_bytes()).await.unwrap();
            let mut byte = [0u8; 1];
            let read = tokio::time::timeout(PATIENCE, socket.read(&mut byte)).await;
            matches!(read, Ok(Ok(0) | Err(_)))
        }
    }
}

/// Noura's AI settings with the provider at `endpoint`, its key in the
/// credential store, and your consent.
struct Settings {
    _folder: tempfile::TempDir,
    ai: AiFoundation,
}

fn settings(kind: &str, endpoint: &str) -> Settings {
    let folder = tempfile::tempdir().unwrap();
    let ai = AiFoundation::open_with(
        folder.path().join("providers.json"),
        folder.path().join("consents.json"),
        Arc::new(MemoryCredentials::new()),
    )
    .unwrap();
    ai.save_provider(AiProviderConfig {
        id: "local".into(),
        kind: kind.into(),
        display_name: "Local".into(),
        model: "test-model".into(),
        endpoint: Some(endpoint.to_owned()),
        credential_ref: None,
        enabled: true,
        context_window: None,
    })
    .unwrap();
    ai.set_credential("local", KEY).unwrap();
    ai.grant_consent(
        WORKSPACE,
        AiConsentGrantInput {
            provider_id: "local".into(),
            policy_version: POLICY.into(),
            data_category: AiConsentDataCategory::WorkspaceContent,
        },
    )
    .unwrap();
    Settings {
        _folder: folder,
        ai,
    }
}

fn text(role: &str, text: &str) -> AiTransportMessage {
    AiTransportMessage {
        role: role.into(),
        content: vec![AiContentPart::Text { text: text.into() }],
    }
}

fn add_task() -> AiToolDefinition {
    AiToolDefinition {
        name: "add_task".into(),
        description: Some("Add a task.".into()),
        input_schema: json!({
            "type": "object",
            "properties": { "title": { "type": "string" } },
            "required": ["title"],
        }),
    }
}

fn stream(ai: &AiFoundation, messages: Vec<AiTransportMessage>) -> AiStreamOperation {
    ai.start_stream(
        WORKSPACE,
        AiStreamInput {
            operation_id: OPERATION.into(),
            model: AiModelRef {
                provider_id: "local".into(),
                model: "test-model".into(),
            },
            messages,
            tools: Some(vec![add_task()]),
            policy_version: Some(POLICY.into()),
        },
    )
    .unwrap()
}

/// The next frame's event, after checking it carries the next sequence
/// number.
async fn next(operation: &mut AiStreamOperation, sequence: &mut u64) -> AiStreamEvent {
    let frame = tokio::time::timeout(PATIENCE, operation.receiver.recv())
        .await
        .expect("no frame arrived")
        .expect("the stream ended without a last frame");
    assert_eq!(frame.sequence, *sequence);
    *sequence += 1;
    frame.event
}

/// Every event until the stream ends.
async fn events(mut operation: AiStreamOperation) -> Vec<AiStreamEvent> {
    let mut sequence = 1;
    let mut events = Vec::new();
    loop {
        let event = next(&mut operation, &mut sequence).await;
        let last = matches!(
            event,
            AiStreamEvent::Completed { .. }
                | AiStreamEvent::Cancelled
                | AiStreamEvent::Error { .. }
        );
        events.push(event);
        if last {
            return events;
        }
    }
}

fn answer_text(events: &[AiStreamEvent]) -> String {
    events
        .iter()
        .filter_map(|event| match event {
            AiStreamEvent::TextDelta { text } => Some(text.as_str()),
            _ => None,
        })
        .collect()
}

fn tool_calls(events: &[AiStreamEvent]) -> Vec<AiToolCall> {
    events
        .iter()
        .filter_map(|event| match event {
            AiStreamEvent::ToolCall { call } => Some(call.clone()),
            _ => None,
        })
        .collect()
}

fn openai_delta(delta: Value, finish_reason: Value) -> Value {
    json!({
        "id": "chatcmpl-1",
        "object": "chat.completion.chunk",
        "model": "test-model",
        "choices": [{ "index": 0, "delta": delta, "finish_reason": finish_reason }],
    })
}

fn openai_answer() -> Vec<Value> {
    vec![
        openai_delta(
            json!({ "role": "assistant", "content": "Adding " }),
            Value::Null,
        ),
        openai_delta(json!({ "content": "it." }), Value::Null),
        openai_delta(
            json!({ "tool_calls": [{
                "index": 0,
                "id": "call_1",
                "type": "function",
                "function": { "name": "add_task", "arguments": "" },
            }] }),
            Value::Null,
        ),
        openai_delta(
            json!({ "tool_calls": [{ "index": 0, "function": { "arguments": "{\"title\":" } }] }),
            Value::Null,
        ),
        openai_delta(
            json!({ "tool_calls": [{ "index": 0, "function": { "arguments": "\"Plant\"}" } }] }),
            Value::Null,
        ),
        openai_delta(json!({}), json!("tool_calls")),
        json!({
            "id": "chatcmpl-1",
            "object": "chat.completion.chunk",
            "model": "test-model",
            "choices": [],
            "usage": { "prompt_tokens": 11, "completion_tokens": 7, "total_tokens": 18 },
        }),
        json!("data: [DONE]"),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn an_openai_style_provider_streams_an_answer_and_a_tool_call() {
    let provider = LocalProvider::start(Answer::Events(openai_answer())).await;
    let settings = settings("openai", &provider.endpoint);

    let events = events(stream(
        &settings.ai,
        vec![
            text("system", "Be brief."),
            text("user", "Add a task to plant."),
        ],
    ))
    .await;

    assert!(matches!(&events[0], AiStreamEvent::Started { model } if model.model == "test-model"));
    assert_eq!(answer_text(&events), "Adding it.");
    assert_eq!(
        tool_calls(&events),
        [AiToolCall {
            call_id: "call_1".into(),
            name: "add_task".into(),
            arguments: json!({ "title": "Plant" }),
        }]
    );
    let Some(AiStreamEvent::Completed { summary }) = events.last() else {
        panic!("the stream didn't complete: {events:?}");
    };
    assert_eq!(
        summary.usage,
        Some(AiUsage {
            prompt_tokens: Some(11),
            completion_tokens: Some(7),
            total_tokens: Some(18),
        })
    );
    assert!(
        matches!(summary.stop_reason, Some(AiStopReason::ToolCall { .. })),
        "{summary:?}"
    );

    let (heard, _) = provider.heard().await;
    assert_eq!(heard.request_line, "POST /v1/chat/completions HTTP/1.1");
    assert_eq!(
        heard.header("authorization"),
        Some(format!("Bearer {KEY}").as_str())
    );
    assert_eq!(heard.body["model"], "test-model");
    assert_eq!(heard.body["stream"], true);
    let roles: Vec<&str> = heard.body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .map(|message| message["role"].as_str().unwrap())
        .collect();
    assert_eq!(roles, ["system", "user"]);
    let sent = heard.body["messages"].to_string();
    assert!(sent.contains("Add a task to plant."), "{sent}");
    assert_eq!(heard.body["tools"][0]["function"]["name"], "add_task");
    assert_eq!(
        heard.body["tools"][0]["function"]["parameters"]["required"],
        json!(["title"])
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn an_anthropic_provider_streams_an_answer_and_a_tool_call() {
    let answer = vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_1",
                "type": "message",
                "role": "assistant",
                "model": "test-model",
                "content": [],
                "stop_reason": null,
                "usage": { "input_tokens": 11, "output_tokens": 0 },
            },
        }),
        json!({
            "type": "content_block_start",
            "index": 0,
            "content_block": { "type": "text", "text": "" },
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "Adding " },
        }),
        json!({
            "type": "content_block_delta",
            "index": 0,
            "delta": { "type": "text_delta", "text": "it." },
        }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({
            "type": "content_block_start",
            "index": 1,
            "content_block": { "type": "tool_use", "id": "toolu_1", "name": "add_task", "input": {} },
        }),
        json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "{\"title\":" },
        }),
        json!({
            "type": "content_block_delta",
            "index": 1,
            "delta": { "type": "input_json_delta", "partial_json": "\"Plant\"}" },
        }),
        json!({ "type": "content_block_stop", "index": 1 }),
        json!({
            "type": "message_delta",
            "delta": { "stop_reason": "tool_use", "stop_sequence": null },
            "usage": { "output_tokens": 7 },
        }),
        json!({ "type": "message_stop" }),
    ];
    let provider = LocalProvider::start(Answer::Events(answer)).await;
    let settings = settings("anthropic", &provider.endpoint);

    let events = events(stream(
        &settings.ai,
        vec![
            text("system", "Be brief."),
            text("user", "Add a task to plant."),
        ],
    ))
    .await;

    assert_eq!(answer_text(&events), "Adding it.");
    assert_eq!(
        tool_calls(&events),
        [AiToolCall {
            call_id: "toolu_1".into(),
            name: "add_task".into(),
            arguments: json!({ "title": "Plant" }),
        }]
    );
    let Some(AiStreamEvent::Completed { summary }) = events.last() else {
        panic!("the stream didn't complete: {events:?}");
    };
    assert!(
        matches!(summary.stop_reason, Some(AiStopReason::ToolCall { .. })),
        "{summary:?}"
    );
    let usage = summary.usage.as_ref().unwrap();
    assert_eq!(
        (usage.prompt_tokens, usage.completion_tokens),
        (Some(11), Some(7))
    );

    let (heard, _) = provider.heard().await;
    assert_eq!(heard.request_line, "POST /v1/messages HTTP/1.1");
    assert_eq!(heard.header("x-api-key"), Some(KEY));
    assert!(heard.header("anthropic-version").is_some());
    assert_eq!(heard.body["stream"], true);
    assert_eq!(heard.body["system"], "Be brief.");
    assert_eq!(heard.body["tools"][0]["name"], "add_task");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_next_request_carries_the_tool_call_and_its_result() {
    let provider = LocalProvider::start(Answer::Events(vec![
        openai_delta(
            json!({ "role": "assistant", "content": "Done." }),
            Value::Null,
        ),
        openai_delta(json!({}), json!("stop")),
        json!("data: [DONE]"),
    ]))
    .await;
    let settings = settings("openai", &provider.endpoint);

    let events = events(stream(
        &settings.ai,
        vec![
            text("user", "Add a task to plant."),
            AiTransportMessage {
                role: "assistant".into(),
                content: vec![AiContentPart::ToolCall {
                    call_id: "call_1".into(),
                    name: "add_task".into(),
                    arguments: json!({ "title": "Plant" }),
                }],
            },
            AiTransportMessage {
                role: "tool".into(),
                content: vec![AiContentPart::ToolResult {
                    call_id: "call_1".into(),
                    name: Some("add_task".into()),
                    content: "Added.".into(),
                }],
            },
        ],
    ))
    .await;

    assert_eq!(answer_text(&events), "Done.");
    assert!(matches!(
        events.last(),
        Some(AiStreamEvent::Completed { summary })
            if matches!(summary.stop_reason, Some(AiStopReason::Completed { .. }))
    ));
    let (heard, _) = provider.heard().await;
    let messages = heard.body["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3, "{messages:?}");
    let call = &messages[1]["tool_calls"][0];
    assert_eq!(call["id"], "call_1");
    assert_eq!(call["function"]["name"], "add_task");
    let arguments: Value =
        serde_json::from_str(call["function"]["arguments"].as_str().unwrap()).unwrap();
    assert_eq!(arguments, json!({ "title": "Plant" }));
    assert_eq!(messages[2]["role"], "tool");
    assert_eq!(messages[2]["tool_call_id"], "call_1");
    assert_eq!(messages[2]["content"], "Added.");
}

#[tokio::test(flavor = "multi_thread")]
async fn stop_ends_the_stream_and_closes_the_connection_to_the_provider() {
    let provider = LocalProvider::start(Answer::EventsThenWait(vec![openai_delta(
        json!({ "role": "assistant", "content": "A long" }),
        Value::Null,
    )]))
    .await;
    let settings = settings("openai", &provider.endpoint);
    let mut operation = stream(&settings.ai, vec![text("user", "Write a lot.")]);
    let mut sequence = 1;
    assert!(matches!(
        next(&mut operation, &mut sequence).await,
        AiStreamEvent::Started { .. }
    ));
    let first = next(&mut operation, &mut sequence).await;
    assert!(
        matches!(&first, AiStreamEvent::TextDelta { text } if text == "A long"),
        "{first:?}"
    );

    assert!(settings.ai.cancel_stream(OPERATION).unwrap().cancelled);

    assert!(matches!(
        next(&mut operation, &mut sequence).await,
        AiStreamEvent::Cancelled
    ));
    let (_, closed) = provider.heard().await;
    assert!(closed, "the request kept running after Stop");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refused_key_reaches_you_as_a_plain_error_without_the_providers_details() {
    let provider = LocalProvider::start(Answer::Status(
        "401 Unauthorized",
        r#"{"error":{"message":"bad key, ask support-internal-detail"}}"#,
    ))
    .await;
    let settings = settings("openai", &provider.endpoint);

    let events = events(stream(&settings.ai, vec![text("user", "Hello.")])).await;

    let Some(AiStreamEvent::Error { error }) = events.last() else {
        panic!("the stream didn't fail: {events:?}");
    };
    let shown = serde_json::to_string(error).unwrap();
    assert!(!shown.contains("support-internal-detail"), "{shown}");
    assert!(!shown.contains(KEY), "{shown}");
    provider.heard().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_connection_that_breaks_mid_answer_fails_the_stream_and_keeps_what_arrived() {
    let (gone, cut) = oneshot::channel::<()>();
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}/v1/", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        // A chunked body that ends without its last chunk reads as a broken
        // connection, which a server that crashes leaves behind.
        let event = format!(
            "data: {}\n\n",
            openai_delta(
                json!({ "role": "assistant", "content": "Half" }),
                Value::Null
            )
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
             transfer-encoding: chunked\r\n\r\n{:x}\r\n{event}\r\n",
            event.len()
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        cut.await.ok();
    });
    let settings = settings("openai", &endpoint);
    let mut operation = stream(&settings.ai, vec![text("user", "Hello.")]);
    let mut sequence = 1;
    next(&mut operation, &mut sequence).await;
    let first = next(&mut operation, &mut sequence).await;
    assert!(
        matches!(&first, AiStreamEvent::TextDelta { text } if text == "Half"),
        "{first:?}"
    );

    drop(gone);
    server.await.unwrap();

    let last = next(&mut operation, &mut sequence).await;
    assert!(matches!(last, AiStreamEvent::Error { .. }), "{last:?}");
}
