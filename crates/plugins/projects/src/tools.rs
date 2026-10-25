//! The tools Projects offers AI assistants through MCP: `list`, `create`
//! and `update`. `create` and `update` take the JSON input of the command
//! with their name, which [`crate::commands`] documents, and answer as the
//! command does. `list` takes an optional `status` and answers with
//! `projects`, each without its body. Noura checks each call's arguments
//! against the tool's schema before the plugin sees them.

use plugin_sdk::Tool;

use crate::commands;

/// The project fields a tool sets, with the values each takes.
const PROPERTIES: &str = r#"{
  "type": "object",
  "properties": {
    "status": {"enum": ["planned", "active", "on-hold", "completed", "cancelled"]}
  },
  "additionalProperties": false
}"#;

/// The tools, each with its schema.
pub(crate) fn tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list".into(),
            description: "List projects, each with its stable ID, title, path, revision and \
                          status, but not its body. Filter by status."
                .into(),
            input_schema: r#"{
  "type": "object",
  "properties": {
    "status": {"enum": ["planned", "active", "on-hold", "completed", "cancelled"]}
  },
  "additionalProperties": false
}"#
            .into(),
        },
        Tool {
            name: "create".into(),
            description: "Create a project with a title, and optionally an overview as its \
                          body and a status. It starts planned, in a folder of its own."
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
            description: "Change a project's status or overview, by its stable ID. Pass the \
                          revision you read: a project that changed since stays as it is."
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
        "list" => commands::list(arguments),
        "create" | "update" => commands::run(name, arguments),
        _ => Err(format!("Projects has no tool {name}.")),
    }
}
