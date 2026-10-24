//! A project's tasks, which Projects reads and changes only through the
//! Tasks plugin's commands. It never writes a task file itself: the host
//! refuses Projects any write to an object of another plugin's type.

use std::collections::BTreeSet;

use plugin_sdk::{Error, command_calls};
use serde_json::{Map, Value, json};

/// The plugin whose commands Projects calls.
const TASKS: &str = "tasks";

/// Each task status's value and the name people see, in the order the
/// board's columns show them: the statuses the Tasks plugin declares.
pub(crate) const STATUSES: [(&str, &str); 4] = [
    ("todo", "To do"),
    ("in-progress", "In progress"),
    ("done", "Done"),
    ("cancelled", "Cancelled"),
];

/// A task, as the Tasks plugin's `list` answers with it.
#[derive(Debug, Clone)]
pub(crate) struct Task {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) revision: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    /// Where the board puts the task among its column's cards: its board
    /// order key, or its stable ID when its file holds none.
    pub(crate) order: String,
    pub(crate) properties: Map<String, Value>,
}

impl Task {
    fn of(value: &Value) -> Option<Self> {
        let text = |key: &str| value.get(key).and_then(Value::as_str).map(str::to_owned);
        let properties = value
            .get("properties")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        let property = |key: &str| {
            properties
                .get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        let id = text("id")?;
        Some(Self {
            title: text("title").unwrap_or_else(|| id.clone()),
            revision: text("revision")?,
            status: property("status").unwrap_or_else(|| "todo".to_owned()),
            priority: property("priority").unwrap_or_else(|| "medium".to_owned()),
            order: property("kanban_order").unwrap_or_else(|| id.clone()),
            properties,
            id,
        })
    }
}

/// Why a call to Tasks didn't do what Projects asked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Refused {
    /// The Tasks plugin is off.
    Off,
    /// The task's file no longer holds the revision Projects read, so
    /// nothing was written.
    Changed,
    /// Anything else, in words.
    Other(String),
}

/// Run the Tasks command `command` with `input`.
fn call(command: &str, input: &Value) -> Result<Value, Refused> {
    match command_calls::call(TASKS, command, &input.to_string()) {
        Ok(answer) => Ok(serde_json::from_str(&answer).unwrap_or(Value::Null)),
        Err(Error::TargetUnavailable(_)) => Err(Refused::Off),
        Err(Error::Failed(answer)) => {
            let answer: Value = serde_json::from_str(&answer).unwrap_or(Value::Null);
            if answer["code"] == "revision_conflict" {
                return Err(Refused::Changed);
            }
            let message = answer["message"]
                .as_str()
                .unwrap_or("Tasks couldn't finish the change.")
                .to_owned();
            Err(Refused::Other(message))
        }
        Err(Error::Busy(_)) => Err(Refused::Other(
            "Tasks is busy. Try again in a moment.".to_owned(),
        )),
        Err(Error::CapabilityNotDeclared(_) | Error::TargetNotDeclared(_)) => Err(Refused::Other(
            "Projects isn't allowed to use the Tasks commands.".to_owned(),
        )),
        Err(_) => Err(Refused::Other(
            "Tasks couldn't finish the change.".to_owned(),
        )),
    }
}

/// Every task, or the tasks whose project is the project `project`, each
/// stable ID once, as Tasks first lists it: two cards with one node ID
/// would stop the board from drawing.
pub(crate) fn list(project: Option<&str>) -> Result<Vec<Task>, Refused> {
    let input = project.map_or_else(|| json!({}), |project| json!({ "project": project }));
    let answer = call("list", &input)?;
    let mut seen = BTreeSet::new();
    Ok(answer["tasks"]
        .as_array()
        .map(|tasks| {
            tasks
                .iter()
                .filter_map(Task::of)
                .filter(|task| seen.insert(task.id.clone()))
                .collect()
        })
        .unwrap_or_default())
}

/// Create a task titled `title` with `properties`.
pub(crate) fn create(title: &str, properties: Value) -> Result<(), Refused> {
    call(
        "create",
        &json!({ "title": title, "properties": properties }),
    )
    .map(|_| ())
}

/// Set `properties` of the task `id`, whose file the board read at
/// `revision`.
pub(crate) fn update(id: &str, revision: &str, properties: Value) -> Result<(), Refused> {
    call(
        "update",
        &json!({ "id": id, "expected_revision": revision, "properties": properties }),
    )
    .map(|_| ())
}

/// The name people see for a task status.
pub(crate) fn status_label(value: &str) -> String {
    STATUSES
        .iter()
        .find(|(id, _)| *id == value)
        .map_or_else(|| value.to_owned(), |(_, label)| (*label).to_owned())
}
