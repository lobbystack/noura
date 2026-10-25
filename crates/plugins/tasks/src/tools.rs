//! The tools Tasks offers AI assistants through MCP: `list`, `create` and
//! `update`. Each takes the JSON input of the command with its name, which
//! [`crate::commands`] documents, and answers as the command does. Noura
//! checks each call's arguments against the tool's schema before the
//! plugin sees them, and a write the workspace format refuses, such as a
//! due date that isn't a date, changes nothing and answers with the
//! command's refusal.

use plugin_sdk::Tool;

use crate::commands;

/// The task fields a tool sets, with the values each takes. `null` clears
/// a field on update.
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
            name: "list".into(),
            description: "List tasks, each with its stable ID, title, path, revision and \
                          properties, but not its body. Filter by status, by project, or both."
                .into(),
            input_schema: r#"{
  "type": "object",
  "properties": {
    "status": {"enum": ["todo", "in-progress", "done", "cancelled"]},
    "project": {"type": "string", "description": "A project's stable ID."}
  },
  "additionalProperties": false
}"#
            .into(),
        },
        Tool {
            name: "create".into(),
            description: "Create a task with a title, and optionally a body and task fields. \
                          Its file goes in the tasks folder, named after the title."
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
            name: "update".into(),
            description: "Change a task's fields or body, by its stable ID. Pass the revision \
                          you read: a task that changed since stays as it is. A null field \
                          clears it."
                .into(),
            input_schema: format!(
                r#"{{
  "type": "object",
  "properties": {{
    "id": {{"type": "string"}},
    "expected_revision": {{"type": "string"}},
    "body": {{"type": "string"}},
    "properties": {PROPERTIES}
  }},
  "required": ["id", "expected_revision"],
  "additionalProperties": false
}}"#
            ),
        },
    ]
}

/// Run the tool `name` with `arguments`, which matched its schema.
pub(crate) fn call(name: &str, arguments: &str) -> Result<String, String> {
    match name {
        "list" | "create" | "update" => commands::run(name, arguments),
        _ => Err(format!("Tasks has no tool {name}.")),
    }
}
