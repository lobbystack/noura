//! The object types the plugin registers: the Transcription, its
//! Transcript, and the settings file. `fixtures/transcription-v1.json`
//! holds the same schemas with the cases they accept and refuse.

/// A Transcription: your notes and their Summary, in
/// `Meetings/<title>/<title>.md` or a project's folder.
pub(crate) const TRANSCRIPTION: &str = "transcription";

/// The Transcript beside it, `Transcript.md`, which links back to it.
pub(crate) const TRANSCRIPT: &str = "transcript";

/// The plugin's settings, with the Summary's prompt as its body.
pub(crate) const SETTINGS: &str = "transcription-settings";

/// When it started and ended, so Calendar shows it, the project it
/// belongs to, and who took part.
pub(crate) const TRANSCRIPTION_SCHEMA: &str = r#"{"properties":[
  {"name":"start","kind":"date","nullable":true},
  {"name":"end","kind":"date","nullable":true},
  {"name":"project","kind":{"reference":"project"},"nullable":true},
  {"name":"attendees","kind":{"list":"string"},"default":[]}
]}"#;

pub(crate) const TRANSCRIPT_SCHEMA: &str = r#"{"properties":[
  {"name":"transcription","kind":{"reference":"transcription"},"nullable":true}
]}"#;

pub(crate) const SETTINGS_SCHEMA: &str = r#"{"properties":[
  {"name":"folder","kind":{"enum":["project","meetings"]},"default":"project"},
  {"name":"provider","kind":"string","nullable":true},
  {"name":"language","kind":"string","nullable":true},
  {"name":"audio","kind":{"enum":["none","folder","device"]},"default":"none"},
  {"name":"audio_days","kind":"number","default":30}
]}"#;

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use workspace_format::{
        ObjectTypeRegistry, ParsedMarkdown, PropertySchema, WorkspaceObject, parse_markdown,
    };

    use super::*;

    fn fixture() -> Value {
        serde_json::from_str(include_str!("../fixtures/transcription-v1.json")).unwrap()
    }

    const DECLARED: [(&str, &str); 3] = [
        (TRANSCRIPTION, TRANSCRIPTION_SCHEMA),
        (TRANSCRIPT, TRANSCRIPT_SCHEMA),
        (SETTINGS, SETTINGS_SCHEMA),
    ];

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

    /// The format spec shows the Transcription's schema under its own
    /// heading.
    #[test]
    fn the_format_spec_shows_the_declared_schemas() {
        let spec = include_str!("../../../../docs/workspace-format/v1.md");
        let section = &spec[spec.find("### Transcription types").unwrap()..];
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
        assert!(cases.len() >= 8);
        for case in cases {
            let name = case["name"].as_str().unwrap();
            let object_type = case["type"].as_str().unwrap();
            let mut object = WorkspaceObject {
                id: format!("{object_type}_01j00000000000000000000000"),
                object_type: object_type.into(),
                title: "Example".into(),
                body: String::new(),
                relative_path: "Meetings/Example/Example.md".into(),
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
    fn files_serialize_in_the_declared_order() {
        let fixture = fixture();
        let types = registry();
        for case in fixture["serialization"].as_array().unwrap() {
            let name = case["name"].as_str().unwrap();
            let markdown = case["markdown"].as_str().unwrap();
            let ParsedMarkdown::Managed(object) =
                parse_markdown(case["path"].as_str().unwrap(), markdown.as_bytes())
            else {
                panic!("{name}: a managed object expected");
            };
            let written = String::from_utf8(types.serialize(&object).unwrap()).unwrap();
            assert_eq!(written, case["canonical"].as_str().unwrap(), "{name}");
        }
    }
}
