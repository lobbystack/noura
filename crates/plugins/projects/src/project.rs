//! A project as its file holds it: today's project status, read from the
//! object the host returns.

use plugin_sdk::objects::Object;
use serde_json::{Map, Value};

/// The object type Projects registers.
pub(crate) const PROJECT: &str = "project";

/// The project type's properties in the workspace format's schema
/// declaration: today's project status. The shared fixtures under
/// `docs/workspace-format/fixtures` hold the same schema.
pub(crate) const SCHEMA: &str = r#"{"properties":[
  {"name":"status","kind":{"enum":["planned","active","on-hold","completed","cancelled"]},"default":"planned"}
]}"#;

/// The properties a project has, which the update command accepts.
pub(crate) const FIELDS: [&str; 1] = ["status"];

/// The status a new project starts with.
pub(crate) const PLANNED: &str = "planned";

/// Each status's value and the name people see, in the order the panel
/// lists them.
pub(crate) const STATUSES: [(&str, &str); 5] = [
    ("planned", "Planned"),
    ("active", "Active"),
    ("on-hold", "On hold"),
    ("completed", "Completed"),
    ("cancelled", "Cancelled"),
];

/// A project, read from its file.
#[derive(Debug, Clone)]
pub(crate) struct Project {
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) path: String,
    pub(crate) revision: String,
    pub(crate) status: String,
    /// The frontmatter's properties, for the project's own dated work.
    pub(crate) properties: Map<String, Value>,
}

impl Project {
    pub(crate) fn of(object: &Object) -> Self {
        let properties: Map<String, Value> =
            serde_json::from_str(&object.properties).unwrap_or_default();
        let status = properties
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or(PLANNED)
            .to_owned();
        Self {
            id: object.id.clone(),
            title: object.title.clone(),
            path: object.path.clone(),
            revision: object.revision.clone(),
            status,
            properties,
        }
    }

    /// The folder the project's file sits in, which holds its notes and
    /// files, as `projects/Garden/`. A project file at the top level has
    /// none, so it holds nothing.
    pub(crate) fn folder(&self) -> Option<String> {
        self.path
            .rsplit_once('/')
            .map(|(folder, _)| format!("{folder}/"))
    }
}

/// The name people see for a status.
pub(crate) fn status_label(value: &str) -> String {
    STATUSES
        .iter()
        .find(|(id, _)| *id == value)
        .map_or_else(|| value.to_owned(), |(_, label)| (*label).to_owned())
}
