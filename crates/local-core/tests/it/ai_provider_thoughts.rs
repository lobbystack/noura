//! A model that reasons before it asks for tools gets its reasoning back
//! unchanged with the tool results, as Anthropic requires within a
//! tool-use turn and Gemini 3 for a call's thought signature.

use local_core::{AiContentPart, AiStreamEvent, AiTransportMessage};
use serde_json::{Value, json};

use super::ai_provider::{Answer, LocalProvider, events as collect, settings, stream, text};

const SIGNATURE: &str = "EqQBCgIYAhIM1gbcDa9GJwZA2b3hGgxBdjrkzLoky3dl1pkiMOYds";
const REDACTED: &str = "EmwKAhgBEgy3va3pzix/LafPsn4aDFIT2Xlxh0L5L8rLVyIwxtE3rAFBa8cr3qpP";

/// The thoughts and the tool call of a finished stream.
fn thoughts_and_calls(events: &[AiStreamEvent]) -> (Vec<AiContentPart>, Vec<AiContentPart>) {
    let thoughts = events
        .iter()
        .find_map(|event| match event {
            AiStreamEvent::Thoughts { parts } => Some(parts.clone()),
            _ => None,
        })
        .unwrap_or_default();
    let calls = events
        .iter()
        .filter_map(|event| match event {
            AiStreamEvent::ToolCall { call } => Some(AiContentPart::ToolCall {
                call_id: call.call_id.clone(),
                name: call.name.clone(),
                arguments: call.arguments.clone(),
            }),
            _ => None,
        })
        .collect();
    (thoughts, calls)
}

/// The request after the tool ran: your message, the answer with its
/// thoughts and calls, and the result.
fn follow_up(answer: Vec<AiContentPart>, call_id: &str) -> Vec<AiTransportMessage> {
    vec![
        text("user", "Add a task to plant."),
        AiTransportMessage {
            role: "assistant".into(),
            content: answer,
        },
        AiTransportMessage {
            role: "tool".into(),
            content: vec![AiContentPart::ToolResult {
                call_id: call_id.into(),
                name: Some("add_task".into()),
                content: "Added.".into(),
            }],
        },
    ]
}

fn anthropic_turn() -> Vec<Value> {
    vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_1", "type": "message", "role": "assistant", "model": "test-model",
                "content": [], "stop_reason": null,
                "usage": { "input_tokens": 11, "output_tokens": 0 },
            },
        }),
        json!({ "type": "content_block_start", "index": 0,
                "content_block": { "type": "thinking", "thinking": "", "signature": "" } }),
        json!({ "type": "content_block_delta", "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "The user wants " } }),
        json!({ "type": "content_block_delta", "index": 0,
                "delta": { "type": "thinking_delta", "thinking": "a task." } }),
        json!({ "type": "content_block_delta", "index": 0,
                "delta": { "type": "signature_delta", "signature": &SIGNATURE[..20] } }),
        json!({ "type": "content_block_delta", "index": 0,
                "delta": { "type": "signature_delta", "signature": &SIGNATURE[20..] } }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({ "type": "content_block_start", "index": 1,
                "content_block": { "type": "redacted_thinking", "data": REDACTED } }),
        json!({ "type": "content_block_stop", "index": 1 }),
        json!({ "type": "content_block_start", "index": 2,
                "content_block": { "type": "tool_use", "id": "toolu_1", "name": "add_task", "input": {} } }),
        json!({ "type": "content_block_delta", "index": 2,
                "delta": { "type": "input_json_delta", "partial_json": "{\"title\":\"Plant\"}" } }),
        json!({ "type": "content_block_stop", "index": 2 }),
        json!({ "type": "message_delta",
                "delta": { "stop_reason": "tool_use", "stop_sequence": null },
                "usage": { "output_tokens": 30 } }),
        json!({ "type": "message_stop" }),
    ]
}

fn anthropic_done() -> Vec<Value> {
    vec![
        json!({
            "type": "message_start",
            "message": {
                "id": "msg_2", "type": "message", "role": "assistant", "model": "test-model",
                "content": [], "stop_reason": null,
                "usage": { "input_tokens": 40, "output_tokens": 0 },
            },
        }),
        json!({ "type": "content_block_start", "index": 0,
                "content_block": { "type": "text", "text": "" } }),
        json!({ "type": "content_block_delta", "index": 0,
                "delta": { "type": "text_delta", "text": "Added." } }),
        json!({ "type": "content_block_stop", "index": 0 }),
        json!({ "type": "message_delta",
                "delta": { "stop_reason": "end_turn", "stop_sequence": null },
                "usage": { "output_tokens": 2 } }),
        json!({ "type": "message_stop" }),
    ]
}

#[tokio::test(flavor = "multi_thread")]
async fn anthropic_gets_its_thinking_blocks_back_unchanged_with_the_tool_result() {
    let first = LocalProvider::start(Answer::Events(anthropic_turn())).await;
    let settings = settings("anthropic", &first.endpoint);
    let events = collect(stream(
        &settings.ai,
        vec![text("user", "Add a task to plant.")],
    ))
    .await;
    first.heard().await;
    let (thoughts, calls) = thoughts_and_calls(&events);
    assert_eq!(thoughts.len(), 2, "{events:?}");
    assert_eq!(calls.len(), 1);

    let second = LocalProvider::start(Answer::Events(anthropic_done())).await;
    let settings = super::ai_provider::settings("anthropic", &second.endpoint);
    let answer = thoughts.into_iter().chain(calls).collect();
    let events = collect(stream(&settings.ai, follow_up(answer, "toolu_1"))).await;
    assert!(
        matches!(events.last(), Some(AiStreamEvent::Completed { .. })),
        "{events:?}"
    );

    let (heard, _) = second.heard().await;
    let content = &heard.body["messages"][1]["content"];
    assert_eq!(heard.body["messages"][1]["role"], "assistant");
    assert_eq!(
        content[0],
        json!({ "type": "thinking", "thinking": "The user wants a task.", "signature": SIGNATURE })
    );
    assert_eq!(
        content[1],
        json!({ "type": "redacted_thinking", "data": REDACTED })
    );
    assert_eq!(content[2]["type"], "tool_use");
    assert_eq!(content[2]["id"], "toolu_1");
}

#[tokio::test(flavor = "multi_thread")]
async fn another_kind_of_provider_never_gets_anthropic_s_thinking() {
    let provider = LocalProvider::start(Answer::Events(vec![
        super::ai_provider::openai_delta(
            json!({ "role": "assistant", "content": "Done." }),
            Value::Null,
        ),
        super::ai_provider::openai_delta(json!({}), json!("stop")),
        json!("data: [DONE]"),
    ]))
    .await;
    let settings = settings("openai", &provider.endpoint);
    let answer = vec![
        AiContentPart::Thought {
            kind: "anthropic".into(),
            signature: Some(SIGNATURE.into()),
            text: Some("The user wants a task.".into()),
            block: None,
            call_id: None,
        },
        AiContentPart::ToolCall {
            call_id: "call_1".into(),
            name: "add_task".into(),
            arguments: json!({ "title": "Plant" }),
        },
    ];
    collect(stream(&settings.ai, follow_up(answer, "call_1"))).await;
    let (heard, _) = provider.heard().await;
    let sent = heard.body.to_string();
    assert!(!sent.contains(SIGNATURE), "{sent}");
    assert!(!sent.contains("The user wants"), "{sent}");
}

#[tokio::test(flavor = "multi_thread")]
async fn gemini_gets_a_calls_thought_signature_back_on_that_call() {
    let first = LocalProvider::start(Answer::Events(vec![json!({
        "candidates": [{
            "content": { "role": "model", "parts": [
                { "text": "Adding it." },
                { "functionCall": { "name": "add_task", "args": { "title": "Plant" } },
                  "thoughtSignature": SIGNATURE },
            ] },
            "finishReason": "STOP",
            "index": 0,
        }],
        "usageMetadata": { "promptTokenCount": 5, "candidatesTokenCount": 5, "totalTokenCount": 10 },
    })]))
    .await;
    let settings = settings("gemini", &first.endpoint);
    let events = collect(stream(
        &settings.ai,
        vec![text("user", "Add a task to plant.")],
    ))
    .await;
    first.heard().await;
    let (thoughts, calls) = thoughts_and_calls(&events);
    let AiContentPart::ToolCall { call_id, .. } = &calls[0] else {
        unreachable!()
    };
    let call_id = call_id.clone();
    assert!(
        thoughts.iter().any(|part| matches!(
            part,
            AiContentPart::Thought { signature: Some(signature), call_id: Some(bound), .. }
                if signature == SIGNATURE && *bound == call_id
        )),
        "{thoughts:?}"
    );

    let second = LocalProvider::start(Answer::Events(vec![json!({
        "candidates": [{
            "content": { "role": "model", "parts": [{ "text": "Added." }] },
            "finishReason": "STOP",
            "index": 0,
        }],
    })]))
    .await;
    let settings = super::ai_provider::settings("gemini", &second.endpoint);
    let answer = thoughts
        .into_iter()
        .chain([text("assistant", "Adding it.").content.remove(0)])
        .chain(calls)
        .collect();
    collect(stream(&settings.ai, follow_up(answer, &call_id))).await;
    let (heard, _) = second.heard().await;
    let model = heard.body["contents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|content| content["role"] == "model")
        .cloned()
        .unwrap();
    let call = model["parts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|part| part.get("functionCall").is_some())
        .cloned()
        .unwrap();
    assert_eq!(call["thoughtSignature"], SIGNATURE, "{model}");
}
