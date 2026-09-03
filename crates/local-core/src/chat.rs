use std::collections::BTreeMap;

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::{CoreError, ErrorCategory, Result, WorkspaceObject, markdown, valid_object_id};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, TS, JsonSchema, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ChatRetention {
    Ephemeral,
    Permanent,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ChatMessageKind {
    User,
    Assistant,
    ToolCall,
    ToolResult,
    ContextSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, JsonSchema, PartialEq, Eq)]
#[ts(export)]
#[serde(rename_all = "kebab-case")]
pub enum ChatMessageStatus {
    InProgress,
    Completed,
    Interrupted,
    Cancelled,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct Chat {
    pub id: String,
    pub title: String,
    pub retention: ChatRetention,
    pub retention_days: Option<u32>,
    pub relative_path: String,
    pub revision: String,
    pub created: String,
    pub updated: String,
    #[ts(type = "Record<string, unknown>")]
    pub properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChatMessage {
    pub id: String,
    pub chat_id: String,
    pub run_id: String,
    pub kind: ChatMessageKind,
    pub status: ChatMessageStatus,
    pub content_type: String,
    pub content: String,
    pub relative_path: String,
    pub revision: String,
    pub created: String,
    pub updated: String,
    pub provider_id: Option<String>,
    pub model_id: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub error_code: Option<String>,
    pub summarizes_through_message_id: Option<String>,
    #[ts(type = "Record<string, unknown>")]
    pub properties: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS, PartialEq)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChatRead {
    pub chat: Chat,
    pub messages: Vec<ChatMessage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct CreateChatInput {
    pub title: String,
    pub retention: Option<ChatRetention>,
    pub retention_days: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct ChangeChatRetentionInput {
    pub chat_id: String,
    pub retention: ChatRetention,
    pub retention_days: Option<u32>,
    pub expected_chat_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct RenameChatInput {
    pub chat_id: String,
    pub title: String,
    pub expected_chat_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppendChatUserMessageInput {
    pub chat_id: String,
    pub run_id: String,
    pub content: String,
    pub expected_chat_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BeginChatAssistantInput {
    pub chat_id: String,
    pub run_id: String,
    pub provider_id: String,
    pub model_id: String,
    pub expected_chat_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FinishChatAssistantInput {
    pub chat_id: String,
    pub message_id: String,
    pub content: String,
    pub status: ChatMessageStatus,
    pub error_code: Option<String>,
    pub expected_chat_revision: String,
    pub expected_message_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct BeginChatToolCallInput {
    pub chat_id: String,
    pub run_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
    pub expected_chat_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct FinishChatToolCallInput {
    pub chat_id: String,
    pub message_id: String,
    pub content: String,
    pub status: ChatMessageStatus,
    pub error_code: Option<String>,
    pub expected_chat_revision: String,
    pub expected_message_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppendChatToolResultInput {
    pub chat_id: String,
    pub run_id: String,
    pub tool_call_id: String,
    pub tool_name: String,
    pub content: String,
    pub expected_chat_revision: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
#[serde(rename_all = "camelCase")]
pub struct AppendChatContextSummaryInput {
    pub chat_id: String,
    pub run_id: String,
    pub summarizes_through_message_id: String,
    pub content: String,
    pub expected_chat_revision: String,
}

impl Chat {
    pub(crate) fn workspace_object(&self) -> WorkspaceObject {
        let mut properties = self.properties.clone();
        properties.insert("title".into(), self.title.clone().into());
        properties.insert("retention".into(), enum_json(&self.retention));
        properties.insert(
            "retention_days".into(),
            self.retention_days
                .map_or(serde_json::Value::Null, Into::into),
        );
        WorkspaceObject {
            id: self.id.clone(),
            object_type: "chat".into(),
            title: self.title.clone(),
            body: String::new(),
            relative_path: self.relative_path.clone(),
            revision: self.revision.clone(),
            created: Some(self.created.clone()),
            updated: Some(self.updated.clone()),
            properties,
        }
    }
}

impl ChatMessage {
    pub(crate) fn workspace_object(&self) -> WorkspaceObject {
        let mut properties = self.properties.clone();
        properties.insert("chat_id".into(), self.chat_id.clone().into());
        properties.insert("run_id".into(), self.run_id.clone().into());
        properties.insert("kind".into(), enum_json(&self.kind));
        properties.insert("status".into(), enum_json(&self.status));
        properties.insert("content_type".into(), self.content_type.clone().into());
        insert_optional(&mut properties, "provider_id", &self.provider_id);
        insert_optional(&mut properties, "model_id", &self.model_id);
        insert_optional(&mut properties, "tool_call_id", &self.tool_call_id);
        insert_optional(&mut properties, "tool_name", &self.tool_name);
        insert_optional(&mut properties, "error_code", &self.error_code);
        insert_optional(
            &mut properties,
            "summarizes_through_message_id",
            &self.summarizes_through_message_id,
        );
        WorkspaceObject {
            id: self.id.clone(),
            object_type: "chat-message".into(),
            title: String::new(),
            body: self.content.clone(),
            relative_path: self.relative_path.clone(),
            revision: self.revision.clone(),
            created: Some(self.created.clone()),
            updated: Some(self.updated.clone()),
            properties,
        }
    }
}

pub(crate) fn parse_chat(relative_path: &str, bytes: &[u8]) -> Result<Chat> {
    let (mut properties, content) = frontmatter(bytes, "chat_parse")?;
    if !content.is_empty() {
        return Err(invalid("Chat Markdown must not have a body", "chat_parse"));
    }
    let id = take_required_string(&mut properties, "id", "chat_parse")?;
    let object_type = take_required_string(&mut properties, "type", "chat_parse")?;
    if !valid_object_id(&id, "chat") || object_type != "chat" {
        return Err(invalid(
            "The chat stable ID or type is invalid",
            "chat_parse",
        ));
    }
    let title = take_required_string(&mut properties, "title", "chat_parse")?;
    let retention: ChatRetention = take_enum(&mut properties, "retention", "chat_parse")?;
    let retention_days = take_optional_u32(&mut properties, "retention_days", "chat_parse")?;
    validate_retention(retention, retention_days, "chat_parse")?;
    let created = take_timestamp(&mut properties, "created", "chat_parse")?;
    let updated = take_timestamp(&mut properties, "updated", "chat_parse")?;
    Ok(Chat {
        id,
        title,
        retention,
        retention_days,
        relative_path: relative_path.into(),
        revision: markdown::revision(bytes),
        created,
        updated,
        properties,
    })
}

pub(crate) fn parse_chat_message(relative_path: &str, bytes: &[u8]) -> Result<ChatMessage> {
    let (mut properties, content) = frontmatter(bytes, "chat_message_parse")?;
    let id = take_required_string(&mut properties, "id", "chat_message_parse")?;
    let object_type = take_required_string(&mut properties, "type", "chat_message_parse")?;
    if !valid_object_id(&id, "chat-message") || object_type != "chat-message" {
        return Err(invalid(
            "The chat message stable ID or type is invalid",
            "chat_message_parse",
        ));
    }
    let chat_id = take_required_string(&mut properties, "chat_id", "chat_message_parse")?;
    if !valid_object_id(&chat_id, "chat") {
        return Err(invalid(
            "Chat messages require a stable chat_id",
            "chat_message_parse",
        ));
    }
    let message = ChatMessage {
        id,
        chat_id,
        run_id: take_required_string(&mut properties, "run_id", "chat_message_parse")?,
        kind: take_enum(&mut properties, "kind", "chat_message_parse")?,
        status: take_enum(&mut properties, "status", "chat_message_parse")?,
        content_type: take_required_string(&mut properties, "content_type", "chat_message_parse")?,
        content,
        relative_path: relative_path.into(),
        revision: markdown::revision(bytes),
        created: take_timestamp(&mut properties, "created", "chat_message_parse")?,
        updated: take_timestamp(&mut properties, "updated", "chat_message_parse")?,
        provider_id: take_optional_string(&mut properties, "provider_id", "chat_message_parse")?,
        model_id: take_optional_string(&mut properties, "model_id", "chat_message_parse")?,
        tool_call_id: take_optional_string(&mut properties, "tool_call_id", "chat_message_parse")?,
        tool_name: take_optional_string(&mut properties, "tool_name", "chat_message_parse")?,
        error_code: take_optional_string(&mut properties, "error_code", "chat_message_parse")?,
        summarizes_through_message_id: take_optional_string(
            &mut properties,
            "summarizes_through_message_id",
            "chat_message_parse",
        )?,
        properties,
    };
    validate_message_shape(&message, "chat_message_parse")?;
    Ok(message)
}

pub(crate) fn serialize_chat(chat: &Chat) -> Result<Vec<u8>> {
    let mut fields = chat.properties.clone();
    fields.insert("id".into(), chat.id.clone().into());
    fields.insert("type".into(), "chat".into());
    fields.insert("title".into(), chat.title.clone().into());
    fields.insert("retention".into(), enum_json(&chat.retention));
    fields.insert(
        "retention_days".into(),
        chat.retention_days
            .map_or(serde_json::Value::Null, Into::into),
    );
    fields.insert("created".into(), chat.created.clone().into());
    fields.insert("updated".into(), chat.updated.clone().into());
    serialize_frontmatter(
        fields,
        &[
            "id",
            "type",
            "title",
            "retention",
            "retention_days",
            "created",
            "updated",
        ],
        String::new(),
    )
}

pub(crate) fn serialize_chat_message(message: &ChatMessage) -> Result<Vec<u8>> {
    let mut fields = message.properties.clone();
    fields.insert("id".into(), message.id.clone().into());
    fields.insert("type".into(), "chat-message".into());
    fields.insert("chat_id".into(), message.chat_id.clone().into());
    fields.insert("run_id".into(), message.run_id.clone().into());
    fields.insert("kind".into(), enum_json(&message.kind));
    fields.insert("status".into(), enum_json(&message.status));
    fields.insert("content_type".into(), message.content_type.clone().into());
    insert_optional(&mut fields, "provider_id", &message.provider_id);
    insert_optional(&mut fields, "model_id", &message.model_id);
    insert_optional(&mut fields, "tool_call_id", &message.tool_call_id);
    insert_optional(&mut fields, "tool_name", &message.tool_name);
    insert_optional(&mut fields, "error_code", &message.error_code);
    insert_optional(
        &mut fields,
        "summarizes_through_message_id",
        &message.summarizes_through_message_id,
    );
    fields.insert("created".into(), message.created.clone().into());
    fields.insert("updated".into(), message.updated.clone().into());
    let content = if matches!(
        &message.kind,
        ChatMessageKind::ToolCall | ChatMessageKind::ToolResult
    ) {
        canonical_json(&message.content, "chat_serialize")?
    } else {
        message.content.clone()
    };
    serialize_frontmatter(
        fields,
        &[
            "id",
            "type",
            "chat_id",
            "run_id",
            "kind",
            "status",
            "content_type",
            "provider_id",
            "model_id",
            "tool_call_id",
            "tool_name",
            "error_code",
            "summarizes_through_message_id",
            "created",
            "updated",
        ],
        content,
    )
}

pub(crate) fn validate_message_shape(message: &ChatMessage, operation: &str) -> Result<()> {
    if message.run_id.trim().is_empty() || message.run_id.chars().any(char::is_control) {
        return Err(invalid(
            "run_id must be a non-empty single-line string",
            operation,
        ));
    }
    let tool = matches!(
        message.kind,
        ChatMessageKind::ToolCall | ChatMessageKind::ToolResult
    );
    if tool {
        if message.content_type != "application/json"
            || message.tool_call_id.is_none()
            || message.tool_name.is_none()
        {
            return Err(invalid(
                "Tool messages require application/json, tool_call_id, and tool_name",
                operation,
            ));
        }
        canonical_json(&message.content, operation)?;
    } else if message.content_type != "text/markdown" {
        return Err(invalid(
            "Non-tool messages require text/markdown",
            operation,
        ));
    }
    let assistant = message.kind == ChatMessageKind::Assistant;
    if assistant != (message.provider_id.is_some() && message.model_id.is_some()) {
        return Err(invalid(
            "Assistant messages require provider_id and model_id",
            operation,
        ));
    }
    if !assistant && (message.provider_id.is_some() || message.model_id.is_some()) {
        return Err(invalid(
            "Only assistant messages may include provider_id or model_id",
            operation,
        ));
    }
    if !tool && (message.tool_call_id.is_some() || message.tool_name.is_some()) {
        return Err(invalid(
            "Only tool messages may include tool metadata",
            operation,
        ));
    }
    let error_code_is_valid = match message.status {
        ChatMessageStatus::Failed => message.error_code.is_some(),
        ChatMessageStatus::Interrupted => true,
        ChatMessageStatus::InProgress
        | ChatMessageStatus::Completed
        | ChatMessageStatus::Cancelled => message.error_code.is_none(),
    };
    if !error_code_is_valid {
        return Err(invalid(
            "Failed messages require error_code; interrupted messages may include it; other statuses omit it",
            operation,
        ));
    }
    if let Some(id) = &message.summarizes_through_message_id {
        if message.kind != ChatMessageKind::ContextSummary || !valid_object_id(id, "chat-message") {
            return Err(invalid(
                "Only context summaries may reference summarizes_through_message_id",
                operation,
            ));
        }
    } else if message.kind == ChatMessageKind::ContextSummary {
        return Err(invalid(
            "Context summaries require summarizes_through_message_id",
            operation,
        ));
    }
    if message
        .error_code
        .as_ref()
        .is_some_and(|value| !valid_error_code(value))
    {
        return Err(invalid(
            "error_code must use ASCII letters, digits, dots, hyphens, or underscores",
            operation,
        ));
    }
    for value in [
        &message.provider_id,
        &message.model_id,
        &message.tool_call_id,
        &message.tool_name,
    ] {
        if value
            .as_ref()
            .is_some_and(|value| value.is_empty() || value.chars().any(char::is_control))
        {
            return Err(invalid(
                "Optional message metadata must be non-empty and single-line",
                operation,
            ));
        }
    }
    Ok(())
}

fn valid_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
}

pub(crate) fn canonical_json(content: &str, operation: &str) -> Result<String> {
    let value = serde_json::from_str::<serde_json::Value>(content)
        .map_err(|_| invalid("Tool message content must be valid JSON", operation))?;
    serde_json::to_string(&value)
        .map_err(|_| invalid("Tool message content could not be serialized", operation))
}

pub(crate) fn validate_retention(
    retention: ChatRetention,
    days: Option<u32>,
    operation: &str,
) -> Result<()> {
    match (retention, days) {
        (ChatRetention::Ephemeral, Some(30)) | (ChatRetention::Permanent, None) => Ok(()),
        (ChatRetention::Ephemeral, _) => Err(invalid(
            "Ephemeral chats require retention_days: 30",
            operation,
        )),
        (ChatRetention::Permanent, _) => Err(invalid(
            "Permanent chats require retention_days: null",
            operation,
        )),
    }
}

fn frontmatter(
    bytes: &[u8],
    operation: &str,
) -> Result<(BTreeMap<String, serde_json::Value>, String)> {
    let text = std::str::from_utf8(bytes)
        .map_err(|_| invalid("Chat Markdown must be UTF-8", operation))?
        .replace("\r\n", "\n");
    if !text.starts_with("---\n") {
        return Err(invalid(
            "Chat Markdown requires YAML frontmatter",
            operation,
        ));
    }
    let end = text[4..]
        .find("\n---\n")
        .map(|index| index + 4)
        .ok_or_else(|| invalid("Chat frontmatter has no closing delimiter", operation))?;
    let properties = serde_yaml_ng::from_str(&text[4..end])
        .map_err(|_| invalid("Chat frontmatter is invalid", operation))?;
    Ok((
        properties,
        text[end + 5..]
            .strip_prefix('\n')
            .unwrap_or(&text[end + 5..])
            .trim_end_matches('\n')
            .to_owned(),
    ))
}

fn serialize_frontmatter(
    mut fields: BTreeMap<String, serde_json::Value>,
    known: &[&str],
    content: String,
) -> Result<Vec<u8>> {
    let mut mapping = serde_yaml_ng::Mapping::new();
    for key in known {
        let value = fields.remove(*key).unwrap_or(serde_json::Value::Null);
        mapping.insert(
            serde_yaml_ng::Value::String((*key).into()),
            serde_yaml_ng::to_value(value).map_err(|_| {
                invalid("Chat frontmatter could not be serialized", "chat_serialize")
            })?,
        );
    }
    for (key, value) in fields {
        mapping.insert(
            serde_yaml_ng::Value::String(key),
            serde_yaml_ng::to_value(value).map_err(|_| {
                invalid("Chat frontmatter could not be serialized", "chat_serialize")
            })?,
        );
    }
    let yaml = serde_yaml_ng::to_string(&mapping)
        .map_err(|_| invalid("Chat frontmatter could not be serialized", "chat_serialize"))?;
    let mut output = format!("---\n{yaml}---\n");
    if !content.is_empty() {
        output.push('\n');
        output.push_str(content.trim_end());
        output.push('\n');
    }
    Ok(output.into_bytes())
}

fn take_required_string(
    fields: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
    operation: &str,
) -> Result<String> {
    take_optional_string(fields, key, operation)?
        .ok_or_else(|| invalid(&format!("{key} is required"), operation))
}
fn take_optional_string(
    fields: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
    operation: &str,
) -> Result<Option<String>> {
    match fields.remove(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(value)) if !value.is_empty() => Ok(Some(value)),
        _ => Err(invalid(
            &format!("{key} must be a non-empty string or null"),
            operation,
        )),
    }
}
fn take_optional_u32(
    fields: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
    operation: &str,
) -> Result<Option<u32>> {
    match fields.remove(key) {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(value) => serde_json::from_value(value).map(Some).map_err(|_| {
            invalid(
                &format!("{key} must be an unsigned integer or null"),
                operation,
            )
        }),
    }
}
fn take_timestamp(
    fields: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
    operation: &str,
) -> Result<String> {
    let value = take_required_string(fields, key, operation)?;
    value
        .parse::<jiff::Timestamp>()
        .map_err(|_| invalid(&format!("{key} must be an RFC 3339 timestamp"), operation))?;
    Ok(value)
}
fn take_enum<T: for<'de> Deserialize<'de>>(
    fields: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
    operation: &str,
) -> Result<T> {
    fields
        .remove(key)
        .ok_or_else(|| invalid(&format!("{key} is required"), operation))
        .and_then(|value| {
            serde_json::from_value(value)
                .map_err(|_| invalid(&format!("{key} is invalid"), operation))
        })
}
fn enum_json<T: Serialize>(value: &T) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or(serde_json::Value::Null)
}
fn insert_optional(
    fields: &mut BTreeMap<String, serde_json::Value>,
    key: &str,
    value: &Option<String>,
) {
    fields.insert(
        key.into(),
        value.clone().map_or(serde_json::Value::Null, Into::into),
    );
}
fn invalid(message: &str, operation: &str) -> CoreError {
    CoreError::new(
        "invalid_chat_markdown",
        ErrorCategory::Parse,
        message,
        operation,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    #[derive(Deserialize)]
    struct Fixture {
        name: String,
        valid: bool,
        value: serde_json::Value,
    }

    #[derive(Deserialize)]
    struct Fixtures {
        chat: Vec<Fixture>,
        chat_message: Vec<Fixture>,
    }

    #[test]
    fn tool_messages_round_trip_json_and_unknown_properties() {
        let message = ChatMessage {
            id: "chat-message_01j00000000000000000000000".into(),
            chat_id: "chat_01j00000000000000000000000".into(),
            run_id: "run_1".into(),
            kind: ChatMessageKind::ToolCall,
            status: ChatMessageStatus::InProgress,
            content_type: "application/json".into(),
            content: "{\"query\":\"release\"}".into(),
            relative_path: "messages/2026-09-02/chat-message_01j00000000000000000000000.md".into(),
            revision: String::new(),
            created: "2026-09-02T12:00:00Z".into(),
            updated: "2026-09-02T12:00:00Z".into(),
            provider_id: None,
            model_id: None,
            tool_call_id: Some("call_1".into()),
            tool_name: Some("workspace.search".into()),
            error_code: None,
            summarizes_through_message_id: None,
            properties: BTreeMap::from([("future_field".into(), serde_json::json!(true))]),
        };
        let bytes = serialize_chat_message(&message).unwrap();
        let parsed = parse_chat_message(&message.relative_path, &bytes).unwrap();
        assert_eq!(parsed.properties["future_field"], true);
        assert_eq!(serialize_chat_message(&parsed).unwrap(), bytes);
    }

    #[test]
    fn malformed_tool_json_and_invalid_retention_are_rejected() {
        let message = ChatMessage {
            id: "chat-message_01j00000000000000000000000".into(),
            chat_id: "chat_01j00000000000000000000000".into(),
            run_id: "run_1".into(),
            kind: ChatMessageKind::ToolResult,
            status: ChatMessageStatus::Completed,
            content_type: "application/json".into(),
            content: "not json".into(),
            relative_path: String::new(),
            revision: String::new(),
            created: "2026-09-02T12:00:00Z".into(),
            updated: "2026-09-02T12:00:00Z".into(),
            provider_id: None,
            model_id: None,
            tool_call_id: Some("call_1".into()),
            tool_name: Some("workspace.search".into()),
            error_code: None,
            summarizes_through_message_id: None,
            properties: BTreeMap::new(),
        };
        assert!(validate_message_shape(&message, "chat_test").is_err());
        assert!(validate_retention(ChatRetention::Ephemeral, Some(7), "chat_test").is_err());
    }

    #[test]
    fn rust_chat_validation_matches_shared_conformance_fixtures() {
        let fixtures: Fixtures = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../docs/workspace-format/fixtures/conformance-v1.json"
        )))
        .unwrap();

        for fixture in fixtures.chat {
            let yaml = serde_yaml_ng::to_string(&fixture.value).unwrap();
            let bytes = format!("---\n{yaml}---\n");
            assert_eq!(
                parse_chat("chats/test--000000/chat.md", bytes.as_bytes()).is_ok(),
                fixture.valid,
                "chat fixture: {}",
                fixture.name,
            );
        }

        for fixture in fixtures.chat_message {
            let yaml = serde_yaml_ng::to_string(&fixture.value).unwrap();
            let body = match fixture
                .value
                .get("kind")
                .and_then(serde_json::Value::as_str)
            {
                Some("tool-call" | "tool-result") => "{\"value\":true}\n",
                _ => "",
            };
            let bytes = format!("---\n{yaml}---\n{body}");
            assert_eq!(
                parse_chat_message(
                    "chats/test--000000/messages/2026-09-02/chat-message_01j00000000000000000000000.md",
                    bytes.as_bytes(),
                )
                .is_ok(),
                fixture.valid,
                "chat message fixture: {}",
                fixture.name,
            );
        }
    }

    #[test]
    fn export_bindings() -> std::result::Result<(), Box<dyn std::error::Error>> {
        let bindings = concat!(env!("CARGO_MANIFEST_DIR"), "/bindings");
        ChatRetention::export_all_to(bindings)?;
        ChatMessageKind::export_all_to(bindings)?;
        ChatMessageStatus::export_all_to(bindings)?;
        Chat::export_all_to(bindings)?;
        ChatMessage::export_all_to(bindings)?;
        ChatRead::export_all_to(bindings)?;
        CreateChatInput::export_all_to(bindings)?;
        ChangeChatRetentionInput::export_all_to(bindings)?;
        RenameChatInput::export_all_to(bindings)?;
        AppendChatUserMessageInput::export_all_to(bindings)?;
        BeginChatAssistantInput::export_all_to(bindings)?;
        FinishChatAssistantInput::export_all_to(bindings)?;
        BeginChatToolCallInput::export_all_to(bindings)?;
        FinishChatToolCallInput::export_all_to(bindings)?;
        AppendChatToolResultInput::export_all_to(bindings)?;
        AppendChatContextSummaryInput::export_all_to(bindings)?;
        Ok(())
    }
}
