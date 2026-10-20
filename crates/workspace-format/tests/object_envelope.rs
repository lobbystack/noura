//! The generic managed-object envelope and registered object types, checked
//! against the shared fixture in
//! `docs/workspace-format/fixtures/object-envelope-v1.json`.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use workspace_format::{
    FormatError, ObjectTypeRegistry, ParsedMarkdown, PropertySchema, WorkspaceObject,
    normalize_project_properties, normalize_task_properties, parse_markdown, serialize_object,
};

fn fixture() -> Value {
    serde_json::from_str(include_str!(
        "../../../docs/workspace-format/fixtures/object-envelope-v1.json"
    ))
    .unwrap()
}

fn cases<'a>(fixture: &'a Value, section: &str) -> &'a [Value] {
    fixture[section]
        .as_array()
        .unwrap_or_else(|| panic!("the fixture has no {section} section"))
}

fn name(case: &Value) -> &str {
    case["name"].as_str().unwrap()
}

fn managed(case: &Value) -> WorkspaceObject {
    let markdown = case["markdown"].as_str().unwrap();
    match parse_markdown("objects/example.md", markdown.as_bytes()) {
        ParsedMarkdown::Managed(object) => object,
        other => panic!("{}: expected a managed object, got {other:?}", name(case)),
    }
}

fn envelope_fields(object: &WorkspaceObject) -> Value {
    json!({
        "id": object.id,
        "type": object.object_type,
        "title": object.title,
        "body": object.body,
        "created": object.created,
        "updated": object.updated,
        "properties": object.properties,
    })
}

fn text(bytes: Vec<u8>) -> String {
    String::from_utf8(bytes).unwrap()
}

#[test]
fn any_managed_object_parses_and_serializes_through_the_envelope() {
    let fixture = fixture();
    let types = ObjectTypeRegistry::new();
    for case in cases(&fixture, "envelope") {
        let object = managed(case);
        assert_eq!(envelope_fields(&object), case["object"], "{}", name(case));
        let canonical = text(types.serialize(&object).unwrap());
        assert_eq!(
            canonical,
            case["canonical"].as_str().unwrap(),
            "{}",
            name(case)
        );
        let ParsedMarkdown::Managed(reparsed) =
            parse_markdown("objects/example.md", canonical.as_bytes())
        else {
            panic!("{}: canonical bytes must stay managed", name(case));
        };
        assert_eq!(
            text(types.serialize(&reparsed).unwrap()),
            canonical,
            "{}",
            name(case)
        );
    }
}

fn schema(value: &Value) -> PropertySchema {
    serde_json::from_value(value.clone()).unwrap()
}

#[test]
fn schema_declarations_decode_as_the_fixture_says() {
    let fixture = fixture();
    for case in cases(&fixture, "schema_declaration") {
        let decoded = serde_json::from_value::<PropertySchema>(case["schema"].clone());
        assert_eq!(
            decoded.is_ok(),
            case["valid"].as_bool().unwrap(),
            "{}: {decoded:?}",
            name(case)
        );
    }
}

#[test]
fn registration_accepts_or_rejects_with_a_structured_error() {
    let fixture = fixture();
    for case in cases(&fixture, "registration") {
        let mut types = ObjectTypeRegistry::new();
        let steps = case["register"].as_array().unwrap();
        let (last, earlier) = steps.split_last().unwrap();
        for step in earlier {
            types
                .register(step["type"].as_str().unwrap(), schema(&step["schema"]))
                .unwrap_or_else(|error| panic!("{}: {error}", name(case)));
        }
        let result = types.register(last["type"].as_str().unwrap(), schema(&last["schema"]));
        let error = result
            .err()
            .map(|error| serde_json::to_value(error).unwrap());
        assert_eq!(
            error.unwrap_or(Value::Null),
            case["error"],
            "{}",
            name(case)
        );
    }
}

/// A registry holding every schema in the fixture's `schemas` section.
fn registry(fixture: &Value) -> ObjectTypeRegistry {
    let mut types = ObjectTypeRegistry::new();
    for (object_type, declaration) in fixture["schemas"].as_object().unwrap() {
        types.register(object_type, schema(declaration)).unwrap();
    }
    types
}

fn object_with(object_type: &str, properties: &Value) -> WorkspaceObject {
    WorkspaceObject {
        id: format!("{object_type}_01j00000000000000000000000"),
        object_type: object_type.into(),
        title: "Example".into(),
        body: String::new(),
        relative_path: "objects/example.md".into(),
        revision: String::new(),
        created: Some("2026-08-27T18:00:00Z".into()),
        updated: Some("2026-08-27T18:00:00Z".into()),
        properties: serde_json::from_value(properties.clone()).unwrap(),
    }
}

#[test]
fn validation_applies_defaults_and_names_the_failing_property() {
    let fixture = fixture();
    let types = registry(&fixture);
    for case in cases(&fixture, "validation") {
        let mut object = object_with(case["type"].as_str().unwrap(), &case["properties"]);
        match types.normalize(&mut object) {
            Ok(()) => assert_eq!(json!(object.properties), case["expected"], "{}", name(case)),
            Err(error) => {
                assert_eq!(
                    serde_json::to_value(error).unwrap(),
                    case["error"],
                    "{}",
                    name(case)
                );
                assert_eq!(
                    json!(object.properties),
                    case["properties"],
                    "{}: a rejected object stays unchanged",
                    name(case)
                );
            }
        }
    }
}

fn conformance_v1() -> Value {
    serde_json::from_str(include_str!(
        "../../../docs/workspace-format/fixtures/conformance-v1.json"
    ))
    .unwrap()
}

/// Runs one of conformance-v1's property sections through today's built-in
/// rules and through the declared schema, and requires the same verdicts and
/// the same properties afterward.
fn assert_schema_matches_built_in_rules(
    object_type: &str,
    section: &str,
    built_in: fn(&mut BTreeMap<String, Value>) -> Result<(), FormatError>,
) {
    let fixture = fixture();
    let types = registry(&fixture);
    let conformance = conformance_v1();
    for case in cases(&conformance, section) {
        let valid = case["valid"].as_bool().unwrap();
        let mut built_in_properties: BTreeMap<String, Value> =
            serde_json::from_value(case["value"].clone()).unwrap();
        let built_in_result = built_in(&mut built_in_properties);
        let mut object = object_with(object_type, &case["value"]);
        let schema_result = types.normalize(&mut object);
        assert_eq!(
            built_in_result.is_ok(),
            valid,
            "{}: built-in rules",
            name(case)
        );
        assert_eq!(
            schema_result.is_ok(),
            valid,
            "{}: {schema_result:?}",
            name(case)
        );
        if valid {
            assert_eq!(object.properties, built_in_properties, "{}", name(case));
        }
    }
}

#[test]
fn a_declared_task_schema_matches_the_task_property_fixtures() {
    assert_schema_matches_built_in_rules("task", "task_properties", normalize_task_properties);
}

#[test]
fn a_declared_project_schema_matches_the_project_property_fixtures() {
    assert_schema_matches_built_in_rules(
        "project",
        "project_properties",
        normalize_project_properties,
    );
}

/// Serializes, parses and serializes again, and requires the same bytes.
fn assert_canonical(
    types: &ObjectTypeRegistry,
    object: &WorkspaceObject,
    expected: &str,
    case: &str,
) {
    let canonical = text(types.serialize(object).unwrap());
    assert_eq!(canonical, expected, "{case}");
    let ParsedMarkdown::Managed(reparsed) =
        parse_markdown(&object.relative_path, canonical.as_bytes())
    else {
        panic!("{case}: canonical bytes must stay managed");
    };
    assert_eq!(
        text(types.serialize(&reparsed).unwrap()),
        canonical,
        "{case}"
    );
}

#[test]
fn registered_types_serialize_declared_properties_in_declared_order() {
    let fixture = fixture();
    let types = registry(&fixture);
    for case in cases(&fixture, "registered_serialization") {
        assert_canonical(
            &types,
            &managed(case),
            case["canonical"].as_str().unwrap(),
            name(case),
        );
    }
}

#[test]
fn registered_types_keep_todays_body_rules() {
    let fixture = fixture();
    let types = registry(&fixture);
    let conformance = conformance_v1();
    for case in cases(&conformance, "object_body") {
        let mut object = object_with("recipe", &json!({}));
        object.body = case["body"].as_str().unwrap().into();
        let bytes = text(types.serialize(&object).unwrap());
        assert!(
            bytes.ends_with(case["file_ends_with"].as_str().unwrap()),
            "{}",
            name(case)
        );
        let ParsedMarkdown::Managed(parsed) =
            parse_markdown(&object.relative_path, bytes.as_bytes())
        else {
            panic!("{}: managed object expected", name(case));
        };
        assert_eq!(
            parsed.body,
            case["expected_body"].as_str().unwrap(),
            "{}",
            name(case)
        );
    }
}

#[test]
fn an_unregistered_type_stays_a_managed_object_with_every_property() {
    let fixture = fixture();
    for case in cases(&fixture, "unregistered") {
        let types = match case["registry"].as_str().unwrap() {
            "schemas" => registry(&fixture),
            "empty" => ObjectTypeRegistry::new(),
            other => panic!("{}: unknown registry {other}", name(case)),
        };
        let mut object = managed(case);
        assert_eq!(
            json!(object.properties),
            case["properties"],
            "{}",
            name(case)
        );
        types.normalize(&mut object).unwrap();
        assert_eq!(
            json!(object.properties),
            case["properties"],
            "{}",
            name(case)
        );
        assert_canonical(
            &types,
            &object,
            case["canonical"].as_str().unwrap(),
            name(case),
        );
    }
}

#[test]
fn malformed_input_stays_malformed_whether_or_not_its_type_is_registered() {
    let fixture = fixture();
    for case in cases(&fixture, "malformed") {
        let bytes: Vec<u8> = match case.get("bytes") {
            Some(bytes) => serde_json::from_value(bytes.clone()).unwrap(),
            None => case["markdown"].as_str().unwrap().as_bytes().to_vec(),
        };
        match parse_markdown("objects/example.md", &bytes) {
            ParsedMarkdown::Malformed { error, .. } => assert!(
                error.starts_with(case["error_starts_with"].as_str().unwrap()),
                "{}: {error}",
                name(case)
            ),
            other => panic!("{}: expected malformed, got {other:?}", name(case)),
        }
    }
}

#[test]
fn the_format_spec_schema_example_registers() {
    let spec = include_str!("../../../docs/workspace-format/v1.md");
    let section = &spec[spec.find("### Schema declaration").unwrap()..];
    let start = section.find("```json\n").unwrap() + "```json\n".len();
    let end = start + section[start..].find("```").unwrap();
    let example: PropertySchema = serde_json::from_str(&section[start..end]).unwrap();
    ObjectTypeRegistry::new().register("task", example).unwrap();
}

#[test]
fn note_task_and_project_files_serialize_to_the_same_bytes_as_before() {
    let fixture = fixture();
    for case in cases(&fixture, "existing_serialization") {
        let bytes = serialize_object(&managed(case)).unwrap();
        assert_eq!(
            String::from_utf8(bytes).unwrap(),
            case["markdown"].as_str().unwrap(),
            "{}",
            name(case)
        );
    }
}
