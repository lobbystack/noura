//! Requests that use your ChatGPT plan: the public Responses API with the
//! plan's access token, under the rules OpenAI sets for this route
//! (<https://developers.openai.com/siwc/token-sharing-open-source/preview-limitations>).
//! Each request sets `store: false` and `stream: true`, carries the whole
//! conversation in `input`, puts Noura's instructions in `instructions`
//! rather than system items, and groups the tools in a namespace.

use super::*;

/// The namespace Noura's tools go in.
const NAMESPACE: &str = "noura";

/// Stream an answer from the ChatGPT plan, with the access token `request`
/// carries as its credential, to the API base its provider's endpoint
/// names.
pub(super) async fn stream(request: ResolvedAiStreamRequest) -> Result<AiProviderEventStream> {
    let kind = request.provider.kind.clone();
    let Some(token) = request.credential.clone() else {
        return Err(failures::failure_error(
            failures::PROVIDER_SIGNED_OUT,
            "ai_stream",
        ));
    };
    let body = request_body(&request);
    let http = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(30))
        .build()
        .map_err(|_| failures::failure_error(failures::PROVIDER_UNREACHABLE, "ai_stream"))?;
    let response = http
        .post(format!(
            "{}/responses",
            request
                .provider
                .endpoint
                .as_deref()
                .unwrap_or(chatgpt::RESOURCE)
                .trim_end_matches('/')
        ))
        .bearer_auth(token)
        .header("accept", "text/event-stream")
        .json(&body)
        .send()
        .await
        .map_err(|error| failures::of_reqwest(&error).into_error(&kind, "ai_stream"))?;
    let status = response.status();
    if !status.is_success() {
        let request_id = response
            .headers()
            .get("x-request-id")
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);
        let body = response.text().await.unwrap_or_default();
        return Err(failures::ProviderFailure::from_http(
            status.as_u16(),
            &body,
            request_id.as_deref(),
        )
        .into_error(&kind, "ai_stream"));
    }
    let bytes = response.bytes_stream();
    let state = Parser {
        kind,
        buffer: String::new(),
        calls: Vec::new(),
        done: false,
    };
    let stream = futures::stream::unfold((bytes, state), |(mut bytes, mut state)| async move {
        loop {
            if state.done {
                return None;
            }
            if let Some(events) = state.next_block() {
                if events.is_empty() {
                    continue;
                }
                return Some((futures::stream::iter(events), (bytes, state)));
            }
            match bytes.next().await {
                Some(Ok(chunk)) => state
                    .buffer
                    .push_str(&String::from_utf8_lossy(&chunk).replace("\r\n", "\n")),
                Some(Err(error)) => {
                    state.done = true;
                    let error = failures::of_reqwest(&error).into_error(&state.kind, "ai_stream");
                    return Some((futures::stream::iter(vec![Err(error)]), (bytes, state)));
                }
                None => return None,
            }
        }
    })
    .flatten();
    let stream: AiProviderEventStream = Box::pin(stream);
    Ok(stream)
}

/// The request body: the model, Noura's instructions, the conversation
/// and the tools.
pub(super) fn request_body(request: &ResolvedAiStreamRequest) -> Value {
    let mut instructions: Vec<&str> = Vec::new();
    let mut input: Vec<Value> = Vec::new();
    for message in &request.messages {
        for part in &message.content {
            match (message.role.as_str(), part) {
                ("system", AiContentPart::Text { text }) => instructions.push(text),
                ("user", AiContentPart::Text { text }) => input.push(serde_json::json!({
                    "type": "message",
                    "role": "user",
                    "content": [{ "type": "input_text", "text": text }],
                })),
                ("assistant", AiContentPart::Text { text }) if !text.is_empty() => {
                    input.push(serde_json::json!({
                        "type": "message",
                        "role": "assistant",
                        "content": [{ "type": "output_text", "text": text }],
                    }));
                }
                (
                    "assistant",
                    AiContentPart::ToolCall {
                        call_id,
                        name,
                        arguments,
                    },
                ) => input.push(serde_json::json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": name,
                    "namespace": NAMESPACE,
                    "arguments": arguments.to_string(),
                })),
                (
                    "tool",
                    AiContentPart::ToolResult {
                        call_id, content, ..
                    },
                ) => input.push(serde_json::json!({
                    "type": "function_call_output",
                    "call_id": call_id,
                    "output": content,
                })),
                _ => {}
            }
        }
    }
    let mut body = serde_json::json!({
        "model": request.provider.model,
        "input": input,
        "store": false,
        "stream": true,
    });
    if !instructions.is_empty() {
        body["instructions"] = Value::String(instructions.join("\n\n"));
    }
    if !request.tools.is_empty() {
        let tools: Vec<Value> = request
            .tools
            .iter()
            .map(|tool| {
                let mut function = serde_json::json!({
                    "type": "function",
                    "name": tool.name,
                    "parameters": tool.input_schema,
                });
                if let Some(description) = &tool.description {
                    function["description"] = Value::String(description.clone());
                }
                function
            })
            .collect();
        body["tools"] = serde_json::json!([{
            "type": "namespace",
            "name": NAMESPACE,
            "description": "Tools Noura offers in this chat.",
            "tools": tools,
        }]);
    }
    if let Some(reasoning) = request.reasoning {
        let effort = match reasoning {
            AiReasoning::Zero => "none",
            AiReasoning::Minimal => "minimal",
            AiReasoning::Low => "low",
            AiReasoning::Medium => "medium",
            AiReasoning::High => "high",
        };
        body["reasoning"] = serde_json::json!({ "effort": effort });
    }
    body
}

/// The stream's server-sent events, read block by block.
struct Parser {
    kind: String,
    buffer: String,
    calls: Vec<AiToolCall>,
    done: bool,
}

impl Parser {
    /// The events of the next complete block in the buffer, or `None`
    /// until one arrives.
    fn next_block(&mut self) -> Option<Vec<Result<AiProviderStreamEvent>>> {
        let end = self.buffer.find("\n\n")?;
        let block: String = self.buffer.drain(..end + 2).collect();
        let data: String = block
            .lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .map(str::trim_start)
            .collect::<Vec<_>>()
            .join("\n");
        if data.is_empty() || data == "[DONE]" {
            return Some(Vec::new());
        }
        let Ok(event) = serde_json::from_str::<Value>(&data) else {
            return Some(Vec::new());
        };
        Some(self.handle(&event))
    }

    fn handle(&mut self, event: &Value) -> Vec<Result<AiProviderStreamEvent>> {
        let kind = event
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        match kind {
            "response.output_text.delta" => event
                .get("delta")
                .and_then(Value::as_str)
                .map(|delta| vec![Ok(AiProviderStreamEvent::TextDelta(delta.to_owned()))])
                .unwrap_or_default(),
            "response.output_item.done" => {
                let item = event.get("item").unwrap_or(&Value::Null);
                if item.get("type").and_then(Value::as_str) == Some("function_call")
                    && let (Some(call_id), Some(name)) = (
                        item.get("call_id").and_then(Value::as_str),
                        item.get("name").and_then(Value::as_str),
                    )
                {
                    let arguments = item
                        .get("arguments")
                        .and_then(Value::as_str)
                        .map(|text| {
                            serde_json::from_str(text).unwrap_or(Value::String(text.to_owned()))
                        })
                        .unwrap_or(Value::Object(Default::default()));
                    self.calls.push(AiToolCall {
                        call_id: call_id.to_owned(),
                        name: name.to_owned(),
                        arguments,
                    });
                }
                Vec::new()
            }
            "response.completed" | "response.incomplete" => {
                self.done = true;
                let response = event.get("response").unwrap_or(&Value::Null);
                let usage = response.get("usage").map(|usage| {
                    let count = |name: &str| {
                        usage
                            .get(name)
                            .and_then(Value::as_i64)
                            .and_then(|value| i32::try_from(value).ok())
                    };
                    AiUsage {
                        prompt_tokens: count("input_tokens"),
                        completion_tokens: count("output_tokens"),
                        total_tokens: count("total_tokens"),
                    }
                });
                let stop_reason = if kind == "response.incomplete" {
                    let raw = response
                        .pointer("/incomplete_details/reason")
                        .and_then(Value::as_str)
                        .unwrap_or("incomplete")
                        .to_owned();
                    if raw == "content_filter" {
                        AiStopReason::ContentFilter { raw }
                    } else {
                        AiStopReason::MaxTokens { raw }
                    }
                } else if self.calls.is_empty() {
                    AiStopReason::Completed {
                        raw: "completed".to_owned(),
                    }
                } else {
                    AiStopReason::ToolCall {
                        raw: "function_call".to_owned(),
                    }
                };
                vec![Ok(AiProviderStreamEvent::Completed {
                    usage,
                    stop_reason: Some(stop_reason),
                    tool_calls: std::mem::take(&mut self.calls),
                })]
            }
            "response.failed" => {
                self.done = true;
                let error = event.pointer("/response/error").unwrap_or(&Value::Null);
                vec![Err(
                    failures::ProviderFailure::from_body(error).into_error(&self.kind, "ai_stream")
                )]
            }
            "error" => {
                self.done = true;
                vec![Err(
                    failures::ProviderFailure::from_body(event).into_error(&self.kind, "ai_stream")
                )]
            }
            _ => Vec::new(),
        }
    }
}
