//! The object types the AI plugin registers: chats and their messages,
//! with today's v1 fields in the workspace format's chat layout. The
//! shared fixtures under `docs/workspace-format/fixtures` hold the same
//! schemas.
//!
//! A schema states each field's kind. The rules that tie fields together,
//! such as an assistant message's provider and model or a failed
//! message's error code, stay in the chat rules that every chat read and
//! write in Noura goes through.

/// A chat's `chat.md`.
pub(crate) const CHAT: &str = "chat";

/// One message of a chat.
pub(crate) const CHAT_MESSAGE: &str = "chat-message";

pub(crate) const CHAT_SCHEMA: &str = r#"{"properties":[
  {"name":"title","kind":"string"},
  {"name":"retention","kind":{"enum":["ephemeral","permanent"]}},
  {"name":"retention_days","kind":"number","nullable":true}
]}"#;

pub(crate) const CHAT_MESSAGE_SCHEMA: &str = r#"{"properties":[
  {"name":"chat_id","kind":{"reference":"chat"}},
  {"name":"run_id","kind":"string"},
  {"name":"kind","kind":{"enum":["user","assistant","tool-call","tool-result","context-summary"]}},
  {"name":"status","kind":{"enum":["in-progress","completed","interrupted","cancelled","failed"]}},
  {"name":"content_type","kind":{"enum":["text/markdown","application/json"]}},
  {"name":"provider_id","kind":"string","nullable":true},
  {"name":"model_id","kind":"string","nullable":true},
  {"name":"tool_call_id","kind":"string","nullable":true},
  {"name":"tool_name","kind":"string","nullable":true},
  {"name":"error_code","kind":"string","nullable":true},
  {"name":"summarizes_through_message_id","kind":{"reference":"chat-message"},"nullable":true}
]}"#;
