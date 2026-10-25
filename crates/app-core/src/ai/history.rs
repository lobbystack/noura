//! A chat's messages as a provider request carries them.

use local_core::{
    AiContentPart, AiTransportMessage, ChatMessage, ChatMessageKind, ChatMessageStatus,
};
use plugin_host::AiToolDefinition;

/// The messages of a chat for its next request, oldest first: what you
/// wrote, the answers so far, and the tool calls and their results. The
/// answer `answering` that the request is for stays out, as do messages
/// still in progress, failed messages and empty ones.
pub(super) fn transport(messages: &[ChatMessage], answering: &str) -> Vec<AiTransportMessage> {
    let mut transport: Vec<AiTransportMessage> = Vec::new();
    for message in messages {
        if message.id == answering || message.status == ChatMessageStatus::Failed {
            continue;
        }
        match message.kind {
            ChatMessageKind::User if !message.content.is_empty() => {
                transport.push(text("user", &message.content));
            }
            ChatMessageKind::Assistant
                if message.status != ChatMessageStatus::InProgress
                    && !message.content.is_empty() =>
            {
                transport.push(text("assistant", &message.content));
            }
            ChatMessageKind::ToolCall => {
                let (Some(call_id), Some(name)) = (&message.tool_call_id, &message.tool_name)
                else {
                    continue;
                };
                let arguments =
                    serde_json::from_str(&message.content).unwrap_or(serde_json::Value::Null);
                let part = AiContentPart::ToolCall {
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments,
                };
                // A provider reads the calls of one answer as one assistant
                // message, after the answer's text.
                match transport.last_mut() {
                    Some(last) if last.role == "assistant" => last.content.push(part),
                    _ => transport.push(AiTransportMessage {
                        role: "assistant".to_owned(),
                        content: vec![part],
                    }),
                }
            }
            ChatMessageKind::ToolResult if !message.content.is_empty() => {
                let Some(call_id) = &message.tool_call_id else {
                    continue;
                };
                transport.push(AiTransportMessage {
                    role: "tool".to_owned(),
                    content: vec![AiContentPart::ToolResult {
                        call_id: call_id.clone(),
                        name: message.tool_name.clone(),
                        content: message.content.clone(),
                    }],
                });
            }
            _ => {}
        }
    }
    transport
}

/// The tools a plugin offers, as a provider request carries them. Their
/// schemas are JSON already.
pub(super) fn tools(tools: &[AiToolDefinition]) -> Vec<local_core::AiToolDefinition> {
    tools
        .iter()
        .map(|tool| local_core::AiToolDefinition {
            name: tool.name.clone(),
            description: Some(tool.description.clone()).filter(|text| !text.trim().is_empty()),
            input_schema: serde_json::from_str(&tool.input_schema)
                .unwrap_or(serde_json::Value::Bool(true)),
        })
        .collect()
}

fn text(role: &str, content: &str) -> AiTransportMessage {
    AiTransportMessage {
        role: role.to_owned(),
        content: vec![AiContentPart::Text {
            text: content.to_owned(),
        }],
    }
}
