//! A project as its file holds it: today's project status, read from the
//! object the host returns.

use plugin_sdk::objects::Object;
use serde_json::{Map, Value};

/// The object type Projects registers.
pub(crate) const PROJECT: &str = "project";

/// The project type's properties in the workspace format's schema
/// declaration: today's project status. The plugin's fixture,
/// `fixtures/project-v1.json`, holds the same schema with the cases it
/// accepts and refuses.
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

/// The project type's fixture, checked through `workspace-format`'s
/// generic validator and serializer with the schema the plugin registers.
/// The kernel holds no project rules: these cases say what a valid project
/// is.
#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use workspace_format::{
        ObjectTypeRegistry, ParsedMarkdown, PropertySchema, WorkspaceObject, parse_markdown,
    };

    use super::{PROJECT, SCHEMA};

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../fixtures/project-v1.json")).unwrap()
    }

    fn cases<'a>(fixture: &'a Value, section: &str) -> &'a [Value] {
        fixture[section].as_array().unwrap()
    }

    fn name(case: &Value) -> &str {
        case["name"].as_str().unwrap()
    }

    /// A registry holding only the project schema the plugin registers.
    fn registry() -> ObjectTypeRegistry {
        let mut types = ObjectTypeRegistry::new();
        types
            .register(PROJECT, serde_json::from_str(SCHEMA).unwrap())
            .unwrap();
        types
    }

    fn project_with(properties: &Value) -> WorkspaceObject {
        WorkspaceObject {
            id: "project_01j00000000000000000000000".into(),
            object_type: PROJECT.into(),
            title: "Example".into(),
            body: String::new(),
            relative_path: "projects/Example/Example.md".into(),
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

    /// The format spec shows the project schema second under Task and
    /// project types, after the task schema.
    #[test]
    fn the_format_spec_shows_the_declared_schema() {
        let spec = include_str!("../../../../docs/workspace-format/v1.md");
        let section = &spec[spec.find("### Task and project types").unwrap()..];
        let blocks: Vec<&str> = section
            .split("```json\n")
            .skip(1)
            .map(|block| &block[..block.find("```").unwrap()])
            .collect();
        let shown: PropertySchema = serde_json::from_str(blocks[1]).unwrap();
        let declared: PropertySchema = serde_json::from_str(SCHEMA).unwrap();
        assert_eq!(shown, declared);
    }

    #[test]
    fn project_properties_pass_or_fail_the_declared_schema_as_the_fixture_says() {
        let fixture = fixture();
        let types = registry();
        for case in cases(&fixture, "properties") {
            let mut project = project_with(&case["properties"]);
            match types.normalize(&mut project) {
                Ok(()) => assert_eq!(
                    json!(project.properties),
                    case["expected"],
                    "{}",
                    name(case)
                ),
                Err(error) => {
                    assert_eq!(
                        serde_json::to_value(error).unwrap(),
                        case["error"],
                        "{}",
                        name(case)
                    );
                    assert_eq!(
                        json!(project.properties),
                        case["properties"],
                        "{}: a refused project stays as it was",
                        name(case)
                    );
                }
            }
        }
    }

    #[test]
    fn project_files_serialize_in_the_declared_order() {
        let fixture = fixture();
        let types = registry();
        for case in cases(&fixture, "serialization") {
            let markdown = case["markdown"].as_str().unwrap();
            let ParsedMarkdown::Managed(project) =
                parse_markdown("projects/Example/Example.md", markdown.as_bytes())
            else {
                panic!("{}: a managed project expected", name(case));
            };
            let written = String::from_utf8(types.serialize(&project).unwrap()).unwrap();
            assert_eq!(
                written,
                case["canonical"].as_str().unwrap(),
                "{}",
                name(case)
            );
        }
    }
}
