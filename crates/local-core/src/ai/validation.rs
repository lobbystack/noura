//! Checks on the messages and tool definitions a stream request carries.

use super::*;

pub(super) fn validate_transport_messages(
    messages: &[AiTransportMessage],
    operation: &str,
) -> Result<()> {
    if messages.is_empty() {
        return Err(CoreError::validation(
            "messages_empty",
            "At least one AI message is required",
            operation,
        ));
    }
    for message in messages {
        if !matches!(
            message.role.as_str(),
            "system" | "user" | "assistant" | "tool"
        ) {
            return Err(CoreError::validation(
                "message_role_invalid",
                "The AI message role is not supported",
                operation,
            ));
        }
        if message.content.is_empty() {
            return Err(CoreError::validation(
                "message_content_empty",
                "AI messages require content",
                operation,
            ));
        }
        for part in &message.content {
            match part {
                AiContentPart::Text { text } if !text.is_empty() => {
                    if message.role == "tool" {
                        return Err(invalid_message_content(operation));
                    }
                }
                AiContentPart::ToolCall { call_id, name, .. } if message.role == "assistant" => {
                    validate_tool_identifier(call_id, "tool call IDs", operation)?;
                    validate_tool_identifier(name, "tool names", operation)?;
                }
                AiContentPart::ToolResult {
                    call_id,
                    name,
                    content,
                } if message.role == "tool" && !content.is_empty() => {
                    validate_tool_identifier(call_id, "tool call IDs", operation)?;
                    if let Some(name) = name {
                        validate_tool_identifier(name, "tool names", operation)?;
                    }
                }
                _ => return Err(invalid_message_content(operation)),
            }
        }
    }
    Ok(())
}

pub(super) fn validate_tools(tools: &[AiToolDefinition], operation: &str) -> Result<()> {
    let mut names = std::collections::HashSet::new();
    for tool in tools {
        validate_tool_identifier(&tool.name, "tool names", operation)?;
        if !names.insert(&tool.name)
            || !(tool.input_schema.is_object() || tool.input_schema.is_boolean())
        {
            return Err(CoreError::validation(
                "tool_definition_invalid",
                "AI tool definitions must have unique names and a JSON Schema input schema",
                operation,
            ));
        }
        if let Some(description) = &tool.description
            && (description.is_empty()
                || description.len() > 8_192
                || description.chars().any(char::is_control))
        {
            return Err(CoreError::validation(
                "tool_definition_invalid",
                "AI tool descriptions must be plain text",
                operation,
            ));
        }
    }
    Ok(())
}

fn validate_tool_identifier(value: &str, field: &str, operation: &str) -> Result<()> {
    if value.is_empty() || value.len() > 128 || value.chars().any(char::is_control) {
        return Err(CoreError::validation(
            "tool_content_invalid",
            format!("AI {field} must be non-empty plain text"),
            operation,
        ));
    }
    Ok(())
}

fn invalid_message_content(operation: &str) -> CoreError {
    CoreError::validation(
        "message_content_invalid",
        "AI message content does not match its role",
        operation,
    )
}
