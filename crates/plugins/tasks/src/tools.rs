//! The tools Tasks offers AI assistants: `create` and `complete`. Each
//! takes the JSON input of the command with its name, which
//! [`crate::commands`] documents, and answers as the command does. Noura
//! checks each call's arguments against the tool's schema before the
//! plugin sees them, and a write the workspace format refuses, or one
//! that carries a revision the task's file no longer holds, changes
//! nothing and answers with the command's refusal.

use plugin_sdk::Tool;

use crate::commands;

/// The task fields a tool sets, with the values each takes.
const PROPERTIES: &str = r#"{
  "type": "object",
  "properties": {
    "status": {"enum": ["todo", "in-progress", "done", "cancelled", null]},
    "priority": {"enum": ["low", "medium", "high", "urgent", null]},
    "project": {"type": ["string", "null"], "description": "A project's stable ID."},
    "due": {"type": ["string", "null"], "description": "A day as YYYY-MM-DD, or an RFC 3339 time with its offset."},
    "kanban_order": {"type": ["string", "null"]}
  },
  "additionalProperties": false
}"#;

/// The tools, each with its schema.
pub(crate) fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "create".into(),
            description: "Create a task with a title, and optionally a body and task fields. \
                          Its file goes in the tasks folder, named after the title. Answers \
                          with the task's stable ID and revision."
                .into(),
            input_schema: format!(
                r#"{{
  "type": "object",
  "properties": {{
    "title": {{"type": "string", "minLength": 1}},
    "body": {{"type": "string"}},
    "properties": {PROPERTIES}
  }},
  "required": ["title"],
  "additionalProperties": false
}}"#
            ),
        },
        Tool {
            name: "complete".into(),
            description: "Mark a task done, by its stable ID. Pass the revision you read: a \
                          task that changed since stays as it is."
                .into(),
            input_schema: r#"{
  "type": "object",
  "properties": {
    "id": {"type": "string"},
    "expected_revision": {"type": "string"}
  },
  "required": ["id", "expected_revision"],
  "additionalProperties": false
}"#
            .into(),
        },
    ]
}

/// Run the tool `name` with `arguments`, which matched its schema.
pub(crate) fn call(name: &str, arguments: &str) -> Result<String, String> {
    match name {
        "create" | "complete" => commands::run(name, arguments),
        _ => Err(format!("Tasks has no tool {name}.")),
    }
}
