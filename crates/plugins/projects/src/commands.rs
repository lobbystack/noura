//! The commands Projects offers other plugins: `create` and `update`.
//! Each takes a JSON object and answers with the project as its file
//! holds it afterwards:
//!
//! - `create`: `title`, and optionally `properties` and `body`. A project
//!   without a status starts planned, and its file goes in a folder of its
//!   own, as `projects/Garden/Garden.md`.
//! - `update`: `id`, `expected_revision`, and `properties` to set, a
//!   `null` clearing one, and optionally a new `body`.
//!
//! `properties` holds only project fields: `status`. A command that fails
//! answers with a JSON object whose `code` says why, such as
//! `revision_conflict` when the file no longer holds the expected
//! revision, or `invalid_property` with the `property` and the workspace
//! format's `problem` code. Nothing is written then.

use plugin_sdk::{
    Error,
    objects::{self, Changes, NewObject, Object},
};
use serde_json::{Map, Value, json};

use crate::project::{FIELDS, PLANNED, PROJECT};

/// Run the command `id` with `input`.
pub(crate) fn run(id: &str, input: &str) -> Result<String, String> {
    let input: Value = serde_json::from_str(input)
        .map_err(|_| refusal("invalid_input", "The input isn't JSON.", json!({})))?;
    let Some(input) = input.as_object() else {
        return Err(refusal(
            "invalid_input",
            "The input must be a JSON object.",
            json!({}),
        ));
    };
    let project = match id {
        "create" => create(input),
        "update" => update(input),
        _ => {
            return Err(refusal(
                "unknown_command",
                &format!("Projects has no command {id}."),
                json!({}),
            ));
        }
    }?;
    Ok(project_json(&project).to_string())
}

fn create(input: &Map<String, Value>) -> Result<Object, String> {
    let title = input
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or_else(|| refusal("title_required", "A project needs a title.", json!({})))?;
    let (mut set, _) = fields(input)?;
    set.entry("status").or_insert_with(|| json!(PLANNED));
    let body = text(input, "body")?.unwrap_or_default();
    new_project(title, &body, set).map_err(|error| failure(&error))
}

/// Create a project titled `title`, in a folder of its own.
pub(crate) fn new_project(
    title: &str,
    body: &str,
    properties: Map<String, Value>,
) -> Result<Object, Error> {
    objects::create(&NewObject {
        object_type: PROJECT.to_owned(),
        title: title.to_owned(),
        path: None,
        body: body.to_owned(),
        properties: Value::Object(properties).to_string(),
    })
}

fn update(input: &Map<String, Value>) -> Result<Object, String> {
    let (id, revision) = target(input)?;
    let (set, remove) = fields(input)?;
    let body = text(input, "body")?;
    objects::update(
        &id,
        &revision,
        &Changes {
            body,
            properties: Value::Object(set).to_string(),
            remove,
        },
    )
    .map_err(|error| failure(&error))
}

/// The project's stable ID and the revision the caller read.
fn target(input: &Map<String, Value>) -> Result<(String, String), String> {
    let id = text(input, "id")?;
    let revision = text(input, "expected_revision")?;
    match (id, revision) {
        (Some(id), Some(revision)) => Ok((id, revision)),
        _ => Err(refusal(
            "target_required",
            "Changing a project needs its stable ID and the revision you read.",
            json!({}),
        )),
    }
}

/// The project fields `input` sets, and the ones it clears with `null`.
fn fields(input: &Map<String, Value>) -> Result<(Map<String, Value>, Vec<String>), String> {
    let mut set = Map::new();
    let mut remove = Vec::new();
    match input.get("properties") {
        None | Some(Value::Null) => {}
        Some(Value::Object(properties)) => {
            for (key, value) in properties {
                if !FIELDS.contains(&key.as_str()) {
                    return Err(refusal(
                        "unknown_property",
                        &format!("{key} isn't a project field."),
                        json!({ "property": key }),
                    ));
                }
                if value.is_null() {
                    remove.push(key.clone());
                } else {
                    set.insert(key.clone(), value.clone());
                }
            }
        }
        Some(_) => {
            return Err(refusal(
                "invalid_input",
                "properties must be a JSON object.",
                json!({}),
            ));
        }
    }
    Ok((set, remove))
}

fn text(input: &Map<String, Value>, key: &str) -> Result<Option<String>, String> {
    match input.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(text)) => Ok(Some(text.clone())),
        Some(_) => Err(refusal(
            "invalid_input",
            &format!("{key} must be a string."),
            json!({}),
        )),
    }
}

/// A project as the commands answer with it.
fn project_json(project: &Object) -> Value {
    let properties: Value = serde_json::from_str(&project.properties).unwrap_or_else(|_| json!({}));
    json!({
        "id": project.id,
        "title": project.title,
        "path": project.path,
        "revision": project.revision,
        "properties": properties,
        "body": project.body,
        "created": project.created,
        "updated": project.updated,
    })
}

/// A refusal as JSON: its `code`, a `message` and what `details` adds.
fn refusal(code: &str, message: &str, details: Value) -> String {
    let mut answer = json!({ "code": code, "message": message });
    if let (Some(answer), Value::Object(details)) = (answer.as_object_mut(), details) {
        answer.extend(details);
    }
    answer.to_string()
}

/// A write the host refused, as JSON.
fn failure(error: &Error) -> String {
    match error {
        Error::RevisionConflict(id) => refusal(
            "revision_conflict",
            "The project changed since you read it. Nothing was written.",
            json!({ "id": id }),
        ),
        Error::InvalidProperty(problem) => refusal(
            "invalid_property",
            &format!(
                "{} doesn't fit the project fields: {}.",
                problem.property, problem.code
            ),
            json!({ "property": problem.property, "problem": problem.code }),
        ),
        Error::CapabilityNotDeclared(capability) => refusal(
            "not_a_project",
            "Projects changes only projects.",
            json!({ "capability": capability }),
        ),
        Error::NotFound(what) => refusal(
            "not_found",
            "No project has this ID.",
            json!({ "id": what }),
        ),
        Error::IdentityConflict(id) => refusal(
            "identity_conflict",
            "More than one file holds this ID.",
            json!({ "id": id }),
        ),
        Error::AlreadyExists(path) => refusal(
            "already_exists",
            "A file already has this name.",
            json!({ "path": path }),
        ),
        Error::InvalidPath(path) => refusal(
            "invalid_path",
            "The path isn't one Projects can write.",
            json!({ "path": path }),
        ),
        Error::UnsupportedPath => refusal(
            "unsupported_path",
            "The project's file name can't cross the plugin boundary.",
            json!({}),
        ),
        Error::Failed(message) => refusal("failed", message, json!({})),
        _ => refusal("failed", "Noura couldn't change the project.", json!({})),
    }
}
