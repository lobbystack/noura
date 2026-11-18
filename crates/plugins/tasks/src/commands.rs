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
//! `list` takes an optional `project`, a link to a project such as
//! `[[Garden]]` or its stable ID, and an optional `status`, and answers
//! with `tasks`: every task, or the tasks of that project or with that
//! status, each without its body. It writes nothing.
//!
//! `properties` holds only task fields: `status`, `priority`, `project`,
//! `source`, `due` and `kanban_order`. `project` links to a project and
//! `source` to the object the task came from, of any type. Each takes a
//! link, `[[Garden]]`, or a stable ID, which the task's file holds as the
//! link Noura gives for it. Each task in an answer carries `project_id`,
//! the stable ID of the project its link leads to, or `null`. A command that fails answers with a JSON
//! object whose `code` says why, such as `revision_conflict` when the file
//! no longer holds the expected revision, or `invalid_property` with the
//! `property` and the workspace format's `problem` code. Nothing is
//! written then.

use plugin_sdk::{
    Error,
    objects::{self, Changes, NewObject, Object},
};
use serde_json::{Map, Value, json};

use crate::links::{self, LINK_FIELDS, Resolver};
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
    Ok(task_json(&task, &mut Resolver::default()).to_string())
}

/// Every task, or the tasks of the project and with the status `input`
/// names, each as the commands answer with it, without its body.
fn list(input: &Map<String, Value>) -> Result<Vec<Value>, String> {
    let project = match text(input, "project")? {
        Some(project) if links::is_stable_id(&project) => Some(Some(project)),
        Some(link) => Some(
            plugin_sdk::workspace::resolve_link(&link, None)
                .map_err(|error| failure(&error))?
                .map(|object| object.id),
        ),
        None => None,
    };
    let status = text(input, "status")?;
    let tasks = objects::list(TASK).map_err(|error| failure(&error))?;
    let mut resolver = Resolver::default();
    Ok(tasks
        .iter()
        .map(|task| task_json(task, &mut resolver))
        .filter(|task| {
            project.as_ref().is_none_or(|project| {
                project
                    .as_deref()
                    .is_some_and(|project| task["project_id"] == project)
            }) && status
                .as_deref()
                .is_none_or(|status| task["properties"]["status"] == status)
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
                } else if let (true, Some(link)) =
                    (LINK_FIELDS.contains(&key.as_str()), value.as_str())
                {
                    let link = links::link_for(link).map_err(|error| failure(&error))?;
                    set.insert(key.clone(), Value::String(link));
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

/// A task as the commands answer with it, with `project_id`, the stable
/// ID of the project its `project` link leads to.
pub(crate) fn task_json(task: &Object, resolver: &mut Resolver) -> Value {
    let properties: Value = serde_json::from_str(&task.properties).unwrap_or_else(|_| json!({}));
    let project_id = properties["project"]
        .as_str()
        .and_then(|link| resolver.project(link, &task.path));
    json!({
        "id": task.id,
        "title": task.title,
        "path": task.path,
        "revision": task.revision,
        "properties": properties,
        "project_id": project_id,
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
