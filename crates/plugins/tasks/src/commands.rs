//! The commands Tasks offers other plugins: `create`, `update`,
//! `complete`, `reopen` and `list`. Each takes a JSON object. The first
//! four answer with the task as its file holds it afterwards:
//!
//! - `create`: `title`, and optionally `properties` and `body`.
//! - `update`: `id`, `expected_revision`, and `properties` to set, a
//!   `null` clearing one, and optionally a new `body`.
//! - `complete` and `reopen`: `id` and `expected_revision`. They mark the
//!   task done, or back to todo.
//!
//! `list` takes an optional `project`, a project's stable ID, and answers
//! with `tasks`: every task, or the tasks of that project, each without
//! its body. It writes nothing.
//!
//! `properties` holds only task fields: `status`, `priority`, `project`,
//! `due` and `kanban_order`. A command that fails answers with a JSON
//! object whose `code` says why, such as `revision_conflict` when the file
//! no longer holds the expected revision, or `invalid_property` with the
//! `property` and the workspace format's `problem` code. Nothing is
//! written then.

use plugin_sdk::{
    Error,
    objects::{self, Changes, NewObject, Object},
};
use serde_json::{Map, Value, json};

use crate::task::{FIELDS, TASK};

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
    if id == "list" {
        return list(input).map(|tasks| json!({ "tasks": tasks }).to_string());
    }
    let task = match id {
        "create" => create(input),
        "update" => update(input),
        "complete" => set_status(input, "done"),
        "reopen" => set_status(input, "todo"),
        _ => {
            return Err(refusal(
                "unknown_command",
                &format!("Tasks has no command {id}."),
                json!({}),
            ));
        }
    }?;
    Ok(task_json(&task).to_string())
}

/// Every task, or the tasks of the project `input` names, each as the
/// commands answer with it, without its body.
fn list(input: &Map<String, Value>) -> Result<Vec<Value>, String> {
    let project = text(input, "project")?;
    let tasks = objects::list(TASK).map_err(|error| failure(&error))?;
    Ok(tasks
        .iter()
        .map(task_json)
        .filter(|task| {
            project
                .as_deref()
                .is_none_or(|project| task["properties"]["project"] == project)
        })
        .map(|mut task| {
            if let Some(task) = task.as_object_mut() {
                task.remove("body");
            }
            task
        })
        .collect())
}

fn create(input: &Map<String, Value>) -> Result<Object, String> {
    let title = input
        .get("title")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .ok_or_else(|| refusal("title_required", "A task needs a title.", json!({})))?;
    let (set, _) = fields(input)?;
    let body = text(input, "body")?.unwrap_or_default();
    objects::create(&NewObject {
        object_type: TASK.to_owned(),
        title: title.to_owned(),
        path: None,
        body,
        properties: Value::Object(set).to_string(),
    })
    .map_err(|error| failure(&error))
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

fn set_status(input: &Map<String, Value>, status: &str) -> Result<Object, String> {
    let (id, revision) = target(input)?;
    objects::update(
        &id,
        &revision,
        &Changes {
            body: None,
            properties: json!({ "status": status }).to_string(),
            remove: Vec::new(),
        },
    )
    .map_err(|error| failure(&error))
}

/// The task's stable ID and the revision the caller read.
fn target(input: &Map<String, Value>) -> Result<(String, String), String> {
    let id = text(input, "id")?;
    let revision = text(input, "expected_revision")?;
    match (id, revision) {
        (Some(id), Some(revision)) => Ok((id, revision)),
        _ => Err(refusal(
            "target_required",
            "Changing a task needs its stable ID and the revision you read.",
            json!({}),
        )),
    }
}

/// The task fields `input` sets, and the ones it clears with `null`.
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
                        &format!("{key} isn't a task field."),
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

/// A task as the commands answer with it.
pub(crate) fn task_json(task: &Object) -> Value {
    let properties: Value = serde_json::from_str(&task.properties).unwrap_or_else(|_| json!({}));
    json!({
        "id": task.id,
        "title": task.title,
        "path": task.path,
        "revision": task.revision,
        "properties": properties,
        "body": task.body,
        "created": task.created,
        "updated": task.updated,
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
pub(crate) fn failure(error: &Error) -> String {
    match error {
        Error::RevisionConflict(id) => refusal(
            "revision_conflict",
            "The task changed since you read it. Nothing was written.",
            json!({ "id": id }),
        ),
        Error::InvalidProperty(problem) => refusal(
            "invalid_property",
            &format!(
                "{} doesn't fit the task fields: {}.",
                problem.property, problem.code
            ),
            json!({ "property": problem.property, "problem": problem.code }),
        ),
        Error::CapabilityNotDeclared(capability) => refusal(
            "not_a_task",
            "Tasks changes only tasks.",
            json!({ "capability": capability }),
        ),
        Error::NotFound(what) => {
            refusal("not_found", "No task has this ID.", json!({ "id": what }))
        }
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
            "The path isn't one Tasks can write.",
            json!({ "path": path }),
        ),
        Error::UnsupportedPath => refusal(
            "unsupported_path",
            "The task's file name can't cross the plugin boundary.",
            json!({}),
        ),
        Error::Failed(message) => refusal("failed", message, json!({})),
        other => refusal("failed", &format!("{other:?}"), json!({})),
    }
}
