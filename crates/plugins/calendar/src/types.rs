//! The object types the plugin registers: calendars, their events, and
//! the settings file. `fixtures/calendar-types-v1.json` holds the same
//! schemas with the cases they accept and refuse.

/// A calendar: its name is its file's title, in `Calendars/`.
pub(crate) const CALENDAR: &str = "calendar";

/// An event in one calendar, in `Events/`.
pub(crate) const EVENT: &str = "event";

/// The plugin's settings: the calendars you hid.
pub(crate) const SETTINGS: &str = "calendar-settings";

/// The color a calendar draws its events in.
pub(crate) const CALENDAR_SCHEMA: &str = r#"{"properties":[
  {"name":"color","kind":{"enum":["red","orange","yellow","green","blue","purple","brown","gray"]},"default":"blue"}
]}"#;

/// The calendar an event belongs to, by stable ID, and when it starts
/// and ends. A date start makes an all-day event.
pub(crate) const EVENT_SCHEMA: &str = r#"{"properties":[
  {"name":"calendar","kind":{"id":"calendar"}},
  {"name":"start","kind":"date"},
  {"name":"end","kind":"date","nullable":true}
]}"#;

/// The calendars you hid, by stable ID, and the object types whose
/// built-in calendars you hid.
pub(crate) const SETTINGS_SCHEMA: &str = r#"{"properties":[
  {"name":"hidden_calendars","kind":{"list":{"id":"calendar"}},"default":[]},
  {"name":"hidden_types","kind":{"list":"string"},"default":[]}
]}"#;

/// The types the plugin registers, with their schemas.
pub(crate) const DECLARED: [(&str, &str); 3] = [
    (CALENDAR, CALENDAR_SCHEMA),
    (EVENT, EVENT_SCHEMA),
    (SETTINGS, SETTINGS_SCHEMA),
];

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use workspace_format::{
        ObjectTypeRegistry, ParsedMarkdown, PropertySchema, WorkspaceObject, parse_markdown,
    };

    use super::DECLARED;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../fixtures/calendar-types-v1.json")).unwrap()
    }

    fn registry() -> ObjectTypeRegistry {
        let mut types = ObjectTypeRegistry::new();
        for (name, schema) in DECLARED {
            types
                .register(name, serde_json::from_str(schema).unwrap())
                .unwrap();
        }
        types
    }

    #[test]
    fn the_plugin_declares_the_fixtures_schemas() {
        let fixture = fixture();
        for (name, schema) in DECLARED {
            let declared: PropertySchema = serde_json::from_str(schema).unwrap();
            let fixed: PropertySchema =
                serde_json::from_value(fixture["schemas"][name].clone()).unwrap();
            assert_eq!(declared, fixed, "{name}");
        }
    }

    /// The format spec shows the three schemas under Calendar types, in
    /// the order the plugin declares them.
    #[test]
    fn the_format_spec_shows_the_declared_schemas() {
        let spec = include_str!("../../../../docs/workspace-format/v1.md");
        let section = &spec[spec.find("### Calendar types").unwrap()..];
        let section = &section[..section[4..]
            .find("\n### ")
            .map_or(section.len(), |at| at + 4)];
        let blocks: Vec<PropertySchema> = section
            .split("```json\n")
            .skip(1)
            .map(|block| serde_json::from_str(&block[..block.find("```").unwrap()]).unwrap())
            .collect();
        let declared: Vec<PropertySchema> = DECLARED
            .iter()
            .map(|(_, schema)| serde_json::from_str(schema).unwrap())
            .collect();
        assert_eq!(blocks, declared);
    }

    #[test]
    fn properties_pass_or_fail_as_the_fixture_says() {
        let fixture = fixture();
        let types = registry();
        let cases = fixture["properties"].as_array().unwrap();
        assert!(cases.len() >= 10);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let object_type = case["type"].as_str().unwrap();
            let mut object = WorkspaceObject {
                id: format!("{object_type}_01j00000000000000000000000"),
                object_type: object_type.into(),
                title: "Example".into(),
                body: String::new(),
                relative_path: "Events/Example.md".into(),
                revision: String::new(),
                created: Some("2026-11-16T14:00:00Z".into()),
                updated: Some("2026-11-16T14:00:00Z".into()),
                properties: serde_json::from_value(case["properties"].clone()).unwrap(),
            };
            match types.normalize(&mut object) {
                Ok(()) => assert_eq!(json!(object.properties), case["expected"], "{name}"),
                Err(error) => {
                    assert_eq!(
                        serde_json::to_value(error).unwrap(),
                        case["error"],
                        "{name}"
                    );
                    assert_eq!(json!(object.properties), case["properties"], "{name}");
                }
            }
        }
    }

    #[test]
    fn files_serialize_in_the_declared_order_and_round_trip() {
        let fixture = fixture();
        let types = registry();
        for case in fixture["serialization"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let markdown = case["markdown"].as_str().unwrap();
            let path = case["path"].as_str().unwrap();
            let ParsedMarkdown::Managed(object) = parse_markdown(path, markdown.as_bytes()) else {
                panic!("{name}: a managed object expected");
            };
            let canonical = case["canonical"].as_str().unwrap();
            let written = String::from_utf8(types.serialize(&object).unwrap()).unwrap();
            assert_eq!(written, canonical, "{name}");
            // Canonical bytes read back and write the same bytes again.
            let ParsedMarkdown::Managed(again) = parse_markdown(path, canonical.as_bytes()) else {
                panic!("{name}: the canonical file reads back");
            };
            assert_eq!(again.properties, object.properties, "{name}");
            let rewritten = String::from_utf8(types.serialize(&again).unwrap()).unwrap();
            assert_eq!(rewritten, canonical, "{name}");
        }
    }

    /// Malformed calendar files stay what the format says they are,
    /// whichever plugin registered their type.
    #[test]
    fn malformed_files_read_as_the_fixture_says() {
        let fixture = fixture();
        for case in fixture["malformed"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let parsed = parse_markdown(
                case["path"].as_str().unwrap(),
                case["markdown"].as_str().unwrap().as_bytes(),
            );
            assert!(
                !matches!(parsed, ParsedMarkdown::Managed(_)),
                "{name}: a malformed file isn't an object"
            );
        }
    }
}
