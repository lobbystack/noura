//! A task as its file holds it: the v1 fields Tasks declares, read from
//! the object the host returns.

use jiff::{Timestamp, civil::Date, tz::TimeZone};
use plugin_sdk::objects::Object;
use serde_json::{Map, Value};

/// The object type Tasks registers.
pub(crate) const TASK: &str = "task";

/// The task type's properties in the workspace format's schema
/// declaration, in their canonical order: today's v1 task fields. The
/// shared fixtures under `docs/workspace-format/fixtures` hold the same
/// schema.
pub(crate) const SCHEMA: &str = r#"{"properties":[
  {"name":"status","kind":{"enum":["todo","in-progress","done","cancelled"]},"default":"todo"},
  {"name":"priority","kind":{"enum":["low","medium","high","urgent"]},"default":"medium"},
  {"name":"project","kind":{"reference":"project"},"nullable":true},
  {"name":"due","kind":"date","nullable":true},
  {"name":"kanban_order","kind":"string"}
]}"#;

/// The properties a task has, which the update command accepts.
pub(crate) const FIELDS: [&str; 5] = ["status", "priority", "project", "due", "kanban_order"];

/// Each status's value and the name people see, in the order the panel
/// lists them.
pub(crate) const STATUSES: [(&str, &str); 4] = [
    ("todo", "To do"),
    ("in-progress", "In progress"),
    ("done", "Done"),
    ("cancelled", "Cancelled"),
];

/// Each priority's value and name, from lowest to highest.
pub(crate) const PRIORITIES: [(&str, &str); 4] = [
    ("low", "Low"),
    ("medium", "Medium"),
    ("high", "High"),
    ("urgent", "Urgent"),
];

/// When a task is due: on a day, or at an instant.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Due {
    Day(Date),
    At(Timestamp),
}

impl Due {
    /// A due value as the workspace format writes one: `YYYY-MM-DD`, or
    /// RFC 3339 with an explicit offset.
    pub(crate) fn parse(value: &str) -> Option<Self> {
        if value.len() == 10 {
            value.parse().ok().map(Self::Day)
        } else {
            value.parse().ok().map(Self::At)
        }
    }

    /// The day it falls on in `zone`.
    pub(crate) fn day(self, zone: &TimeZone) -> Date {
        match self {
            Self::Day(day) => day,
            Self::At(at) => at.to_zoned(zone.clone()).date(),
        }
    }

    /// Where it sorts: a day at its first instant in `zone`.
    pub(crate) fn instant(self, zone: &TimeZone) -> Timestamp {
        match self {
            Self::Day(day) => day
                .to_zoned(zone.clone())
                .map_or(Timestamp::MAX, |start| start.timestamp()),
            Self::At(at) => at,
        }
    }
}

/// A task, read from its file.
#[derive(Debug, Clone)]
pub(crate) struct Task {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) path: String,
    pub(crate) revision: String,
    pub(crate) status: String,
    pub(crate) priority: String,
    pub(crate) due: Option<Due>,
    /// The project's stable ID, when the task names one.
    pub(crate) project: Option<String>,
    /// When the file says the task last changed, in RFC 3339.
    pub(crate) updated: String,
}

impl Task {
    pub(crate) fn of(object: &Object) -> Self {
        let properties: Map<String, Value> =
            serde_json::from_str(&object.properties).unwrap_or_default();
        let text = |key: &str| properties.get(key).and_then(Value::as_str);
        Self {
            id: object.id.clone(),
            title: object.title.clone(),
            path: object.path.clone(),
            revision: object.revision.clone(),
            status: text("status").unwrap_or("todo").to_owned(),
            priority: text("priority").unwrap_or("medium").to_owned(),
            due: text("due").and_then(Due::parse),
            project: text("project").map(str::to_owned),
            updated: object.updated.clone().unwrap_or_default(),
        }
    }

    pub(crate) fn done(&self) -> bool {
        self.status == "done"
    }

    /// The folder that holds the task's file: empty for the top level.
    pub(crate) fn folder(&self) -> &str {
        self.path.rsplit_once('/').map_or("", |(folder, _)| folder)
    }
}

/// The name people see for a status or a priority value.
pub(crate) fn label(choices: &[(&str, &'static str)], value: &str) -> String {
    choices
        .iter()
        .find(|(id, _)| *id == value)
        .map_or_else(|| value.to_owned(), |(_, label)| (*label).to_owned())
}
