//! A task as its file holds it: the v1 fields Tasks declares, read from
//! the object the host returns.

use jiff::{Timestamp, civil::Date, tz::TimeZone};
use plugin_sdk::objects::Object;
use serde_json::{Map, Value};

/// The object type Tasks registers.
pub(crate) const TASK: &str = "task";

/// The task type's properties in the workspace format's schema
/// declaration, in their canonical order: today's v1 task fields. The
/// plugin's fixture, `fixtures/task-v1.json`, holds the same schema with
/// the cases it accepts and refuses.
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

/// The task type's fixture, checked through `workspace-format`'s generic
/// validator and serializer with the schema the plugin registers. The
/// kernel holds no task rules: these cases say what a valid task is.
#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use workspace_format::{
        ObjectTypeRegistry, ParsedMarkdown, PropertySchema, WorkspaceObject, parse_markdown,
    };

    use super::{SCHEMA, TASK};

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../fixtures/task-v1.json")).unwrap()
    }

    fn cases<'a>(fixture: &'a Value, section: &str) -> &'a [Value] {
        fixture[section].as_array().unwrap()
    }

    fn name(case: &Value) -> &str {
        case["name"].as_str().unwrap()
    }

    /// A registry holding only the task schema the plugin registers.
    fn registry() -> ObjectTypeRegistry {
        let mut types = ObjectTypeRegistry::new();
        types
            .register(TASK, serde_json::from_str(SCHEMA).unwrap())
            .unwrap();
        types
    }

    fn task_with(properties: &Value) -> WorkspaceObject {
        WorkspaceObject {
            id: "task_01j00000000000000000000000".into(),
            object_type: TASK.into(),
            title: "Example".into(),
            body: String::new(),
            relative_path: "tasks/Example.md".into(),
            revision: String::new(),
            created: Some("2026-08-27T18:00:00Z".into()),
            updated: Some("2026-08-27T18:00:00Z".into()),
            properties: serde_json::from_value(properties.clone()).unwrap(),
        }
    }

    #[test]
    fn the_plugin_declares_the_fixtures_schema() {
        let declared: PropertySchema = serde_json::from_str(SCHEMA).unwrap();
        let fixture: PropertySchema = serde_json::from_value(fixture()["schema"].clone()).unwrap();
        assert_eq!(declared, fixture);
    }

    /// The format spec shows the task schema first under Task and project
    /// types.
    #[test]
    fn the_format_spec_shows_the_declared_schema() {
        let spec = include_str!("../../../../docs/workspace-format/v1.md");
        let section = &spec[spec.find("### Task and project types").unwrap()..];
        let start = section.find("```json\n").unwrap() + "```json\n".len();
        let end = start + section[start..].find("```").unwrap();
        let shown: PropertySchema = serde_json::from_str(&section[start..end]).unwrap();
        let declared: PropertySchema = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(shown, declared);
    }

    #[test]
    fn task_properties_pass_or_fail_the_declared_schema_as_the_fixture_says() {
        let fixture = fixture();
        let types = registry();
        for case in cases(&fixture, "properties") {
            let mut task = task_with(&case["properties"]);
            match types.normalize(&mut task) {
                Ok(()) => assert_eq!(json!(task.properties), case["expected"], "{}", name(case)),
                Err(error) => {
                    assert_eq!(
                        serde_json::to_value(error).unwrap(),
                        case["error"],
                        "{}",
                        name(case)
                    );
                    assert_eq!(
                        json!(task.properties),
                        case["properties"],
                        "{}: a refused task stays as it was",
                        name(case)
                    );
                }
            }
        }
    }

    #[test]
    fn task_files_serialize_in_the_declared_order() {
        let fixture = fixture();
        let types = registry();
        for case in cases(&fixture, "serialization") {
            let markdown = case["markdown"].as_str().unwrap();
            let ParsedMarkdown::Managed(task) =
                parse_markdown("tasks/Example.md", markdown.as_bytes())
            else {
                panic!("{}: a managed task expected", name(case));
            };
            let written = String::from_utf8(types.serialize(&task).unwrap()).unwrap();
            assert_eq!(
                written,
                case["canonical"].as_str().unwrap(),
                "{}",
                name(case)
            );
        }
    }
}
