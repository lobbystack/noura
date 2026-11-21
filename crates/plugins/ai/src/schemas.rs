//! The object types the AI plugin registers: chats and their messages,
//! with today's v1 fields in the workspace format's chat layout, and Orbs
//! and their run records. The shared fixtures under
//! `docs/workspace-format/fixtures` hold the same schemas.
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
  {"name":"orb_id","kind":{"id":"orb"},"nullable":true},
  {"name":"retention","kind":{"enum":["ephemeral","permanent"]}},
  {"name":"retention_days","kind":"number","nullable":true}
]}"#;

pub(crate) const CHAT_MESSAGE_SCHEMA: &str = r#"{"properties":[
  {"name":"chat_id","kind":{"id":"chat"}},
  {"name":"run_id","kind":"string"},
  {"name":"kind","kind":{"enum":["user","assistant","tool-call","tool-result","context-summary"]}},
  {"name":"status","kind":{"enum":["in-progress","completed","interrupted","cancelled","failed"]}},
  {"name":"content_type","kind":{"enum":["text/markdown","application/json"]}},
  {"name":"provider_id","kind":"string","nullable":true},
  {"name":"model_id","kind":"string","nullable":true},
  {"name":"tool_call_id","kind":"string","nullable":true},
  {"name":"tool_name","kind":"string","nullable":true},
  {"name":"error_code","kind":"string","nullable":true},
  {"name":"summarizes_through_message_id","kind":{"id":"chat-message"},"nullable":true}
]}"#;

/// An Orb's own file.
pub(crate) const ORB: &str = "orb";

/// The record of one run of an Orb.
pub(crate) const ORB_RUN: &str = "orb-run";

pub(crate) const ORB_SCHEMA: &str = r#"{"properties":[
  {"name":"avatar","kind":"string","nullable":true},
  {"name":"color","kind":{"enum":["red","orange","yellow","green","blue","purple","brown","gray"]},"nullable":true},
  {"name":"provider","kind":"string","nullable":true},
  {"name":"model","kind":"string","nullable":true},
  {"name":"tools","kind":{"list":"string"},"default":[]},
  {"name":"reads","kind":{"list":{"reference":"*"}},"default":[]},
  {"name":"memory","kind":{"reference":"*"},"nullable":true},
  {"name":"triggers","kind":{"list":"string"},"default":[]},
  {"name":"paused","kind":"boolean","default":false}
]}"#;

pub(crate) const ORB_RUN_SCHEMA: &str = r#"{"properties":[
  {"name":"orb_id","kind":{"id":"orb"}},
  {"name":"chat_id","kind":{"id":"chat"}},
  {"name":"started_by","kind":{"enum":["you","schedule","event"]}},
  {"name":"trigger","kind":"string","nullable":true},
  {"name":"trigger_key","kind":"string","nullable":true},
  {"name":"object_id","kind":"string","nullable":true},
  {"name":"slot","kind":"date","nullable":true},
  {"name":"status","kind":{"enum":["running","succeeded","failed","interrupted","cancelled"]}},
  {"name":"error_code","kind":"string","nullable":true},
  {"name":"tool_calls","kind":"number","default":0},
  {"name":"quiet","kind":"boolean","default":false},
  {"name":"resumed","kind":"number","default":0},
  {"name":"finished","kind":"date","nullable":true}
]}"#;
